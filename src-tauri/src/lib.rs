mod analytics_api;

use analytics_api::{AnalyticsConfig, AnalyticsConfigView, AnalyticsState, FetchError, FetchOutcome};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::types::ToSql;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, Manager, State};

const WATCH_EVENT_NAME: &str = "data-folder-updated";

/// Rows buffered before an import commits a transaction.
///
/// Each flush is one commit, so this is the commit interval as much as a buffer
/// size. Held down by the fact that the batch owns that many `csv::StringRecord`s
/// at once (~2 MiB per 1000 on real portal data).
const IMPORT_BATCH_ROWS: usize = 5000;

// ── Return types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct DataFiles {
    articles: Option<String>,
    dialogs: Option<String>,
    entities: Option<String>,
}

#[derive(Serialize)]
struct SourceFiles {
    articles: Option<String>,
    dialogs: Option<String>,
    entities: Option<String>,
}

#[derive(Serialize)]
struct SourceStatus {
    key: String,
    label: String,
    filename: Option<String>,
    found: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DataSourceInfo {
    selected_folder: Option<String>,
    active_folder: Option<String>,
    using_selected_folder: bool,
    watched_folder: Option<String>,
    missing_sources: Vec<String>,
    statuses: Vec<SourceStatus>,
}

#[derive(Serialize)]
struct AppData {
    articles: serde_json::Value,
    dialogs: serde_json::Value,
    #[serde(rename = "tDialogs")]
    t_dialogs: serde_json::Value,
    entities: serde_json::Value,
    #[serde(rename = "convVars")]
    conv_vars: serde_json::Value,
    #[serde(rename = "ctxVars")]
    ctx_vars: serde_json::Value,
    files: DataFiles,
    #[serde(rename = "sourceFiles")]
    source_files: SourceFiles,
    #[serde(rename = "dataSource")]
    data_source: DataSourceInfo,
}

#[derive(Serialize)]
struct UpdateResult {
    status: String,
    version: Option<String>,
    message: Option<String>,
}

#[derive(Serialize)]
struct FolderSelectionResult {
    ok: bool,
    canceled: bool,
    path: Option<String>,
}

#[derive(Serialize, Clone)]
struct FolderWatchEvent {
    reason: String,
    folder: String,
}

#[derive(Deserialize)]
struct GetDataArgs {
    selected_folder: Option<String>,
}

struct SourceDefinition {
    key: &'static str,
    label: &'static str,
    pattern: &'static str,
}

// ── Conversation DB state ───────────────────────────────────────────────────

struct DbState {
    conn: Option<Connection>,
    path: Option<String>,
}

impl Default for DbState {
    fn default() -> Self {
        Self {
            conn: None,
            path: None,
        }
    }
}

type SharedDbState = Arc<Mutex<DbState>>;
type SharedSearchInterrupt = Arc<Mutex<Option<Arc<rusqlite::InterruptHandle>>>>;

// ── Flagged DB state ─────────────────────────────────────────────────────────

struct FlaggedDbState {
    conn: Option<Connection>,
    path: Option<String>,
}

impl Default for FlaggedDbState {
    fn default() -> Self {
        Self {
            conn: None,
            path: None,
        }
    }
}

type SharedFlaggedDb = Arc<Mutex<FlaggedDbState>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FlaggedFolder {
    folder_id: i64,
    name: String,
    created_at: String,
    sort_order: i64,
    session_count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FlaggedSessionSummary {
    flag_id: i64,
    session_uuid: String,
    flagged_at: String,
    source_db_path: String,
    culture: String,
    first_ts: String,
    interaction_count: i64,
    flagged_count: i64,
    folder_id: Option<i64>,
    notes: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FlaggedInteractionRow {
    log_id: i64,
    interaction_uuid: String,
    session_uuid: String,
    timestamp_start: String,
    timestamp_end: String,
    culture: String,
    main_interaction_type: String,
    all_interaction_types: String,
    interaction_value: String,
    output_text: String,
    article_ids: String,
    dialog_paths: String,
    tdialog_status: String,
    recognition_type: String,
    recognition_quality: f64,
    generative_ai_sources: String,
    articles: String,
    faqs_found: String,
    contexts: String,
    pages: String,
    link_click_info: String,
    feedback_info: String,
    output_metadata: String,
    recognition_details: String,
    is_flagged: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportResult {
    inserted: i64,
    skipped: i64,
    purged: i64,
    errors: Vec<String>,
    /// Per-phase wall clock, so the import modal's Details log can show where
    /// the time actually went instead of us guessing at it.
    timings: ImportTimings,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FinalizeResult {
    purged: i64,
    timings: ImportTimings,
}

/// Wall-clock milliseconds per import phase.
///
/// The tail phases (`purge_ms` and after) are zero on a deferred import — they
/// move to [`finalize_import_run`], which reports them separately.
#[derive(Serialize, Default, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct ImportTimings {
    /// CSV parse plus the row loop, including its per-batch commits.
    rows_ms: u64,
    purge_ms: u64,
    summary_ms: u64,
    fts_optimize_ms: u64,
    pragma_optimize_ms: u64,
    total_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionSummary {
    session_uuid: String,
    first_ts: String,
    last_ts: String,
    interaction_count: i64,
    user_message_preview: String,
    culture: String,
    has_gen_ai: bool,
    has_neg_feedback: bool,
    has_pos_feedback: bool,
    contexts: String, // JSON from most recent interaction that has context data
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextOption {
    name: String,
    value: String,
    count: i64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextFilter {
    name: String,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionsPage {
    sessions: Vec<SessionSummary>,
    total: i64,
    page: i64,
    timing_ms: i64,
    search_mode: String,
}

struct SessionFilterQuery {
    base_where: String,
    search_cte: String,
    filtered_from: String,
    param_values: Vec<Box<dyn ToSql>>,
    search_mode: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InteractionRow {
    log_id: i64,
    interaction_uuid: String,
    session_uuid: String,
    timestamp_start: String,
    timestamp_end: String,
    culture: String,
    main_interaction_type: String,
    all_interaction_types: String,
    interaction_value: String,
    output_text: String,
    article_ids: String,
    dialog_paths: String,
    tdialog_status: String,
    recognition_type: String,
    recognition_quality: f64,
    generative_ai_sources: String,
    articles: String,
    faqs_found: String,
    contexts: String,
    pages: String,
    link_click_info: String,
    feedback_info: String,
    output_metadata: String,
    recognition_details: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileDialogResult {
    ok: bool,
    canceled: bool,
    paths: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSaveResult {
    ok: bool,
    canceled: bool,
    path: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationAiExportResult {
    ok: bool,
    canceled: bool,
    jsonl_path: Option<String>,
    session_count: i64,
    feedback_count: i64,
    interaction_count: i64,
    bytes: i64,
    /// Rough, from the real byte count — enough to tell whether the file fits a
    /// context window, not a substitute for a tokenizer.
    estimated_tokens: i64,
}

struct WatchState {
    watcher: Option<RecommendedWatcher>,
    watched_folder: Option<PathBuf>,
    last_reload_signal: Option<Instant>,
}

impl Default for WatchState {
    fn default() -> Self {
        Self {
            watcher: None,
            watched_folder: None,
            last_reload_signal: None,
        }
    }
}

type SharedWatchState = Arc<Mutex<WatchState>>;

// ── Helpers ─────────────────────────────────────────────────────────────────

fn source_definitions() -> &'static [SourceDefinition] {
    &[
        SourceDefinition {
            key: "articles",
            label: "Articles",
            pattern: "ArticlesExport",
        },
        SourceDefinition {
            key: "dialogs",
            label: "Dialogs",
            pattern: "DialogsExport",
        },
    ]
}

fn resolve_selected_folder(path: &Option<String>) -> Option<PathBuf> {
    path.as_ref()
        .map(PathBuf::from)
        .filter(|folder| folder.is_dir())
}

fn selected_folder_dirs(selected_folder: Option<&Path>) -> Vec<PathBuf> {
    selected_folder
        .map(|folder| vec![folder.to_path_buf()])
        .unwrap_or_default()
}

fn list_matching_files(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    list_matching_files_ext(dir, pattern, "json")
}

fn list_matching_files_ext(dir: &Path, pattern: &str, ext: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let suffix = format!(".{}", ext);
    entries
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(pattern) && name.ends_with(suffix.as_str()) {
                Some(entry.path())
            } else {
                None
            }
        })
        .collect()
}

fn find_entities_file(dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        let mut matches = list_matching_files_ext(dir, "EntitiesExport", "csv");
        matches.sort_by(|left, right| file_sort_key(right).cmp(&file_sort_key(left)));
        if let Some(path) = matches.into_iter().next() {
            return Some(path);
        }
    }
    None
}

fn file_sort_key(path: &Path) -> (SystemTime, String) {
    let modified = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    (modified, name)
}

fn newest_matching_file(dir: &Path, pattern: &str) -> Option<PathBuf> {
    let mut matches = list_matching_files(dir, pattern);
    matches.sort_by(|left, right| file_sort_key(right).cmp(&file_sort_key(left)));
    matches.into_iter().next()
}

fn find_source_files(dirs: &[PathBuf]) -> HashMap<&'static str, PathBuf> {
    let mut found = HashMap::new();
    for definition in source_definitions() {
        for dir in dirs {
            if let Some(path) = newest_matching_file(dir, definition.pattern) {
                found.insert(definition.key, path);
                break;
            }
        }
    }
    found
}

fn extract_articles(content: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|json| json.get("Articles").cloned())
        .unwrap_or(serde_json::Value::Array(vec![]))
}

fn extract_entities(content: &str) -> serde_json::Value {
    // Pipe-delimited CSV with header:
    // Name|Type|Description|Words|WordFixed|WordInBetween|WordOptionPosition|Expression
    let mut lines = content.lines();
    // Skip header row
    let _ = lines.next();
    // Use a Vec to preserve insertion order and a HashMap for O(1) lookup
    let mut ordered: Vec<(String, String, Vec<serde_json::Value>)> = Vec::new();
    let mut order_index: HashMap<String, usize> = HashMap::new();
    for line in lines {
        let cols: Vec<&str> = line.splitn(8, '|').collect();
        if cols.len() < 4 {
            continue;
        }
        let name = cols[0].trim().to_string();
        let entity_type = cols[1].trim().to_string();
        let words_text = cols[3].trim().to_string();
        if name.is_empty() || words_text.is_empty() {
            continue;
        }
        let word_fixed = cols.get(4).map(|s| s.trim()).unwrap_or("").to_string();
        let word_in_between = cols.get(5).map(|s| s.trim()).unwrap_or("").to_string();
        let word_option_position = cols.get(6).map(|s| s.trim()).unwrap_or("").to_string();
        let expression = cols.get(7).map(|s| s.trim()).unwrap_or("").to_string();
        let word_obj = serde_json::json!({
            "text": words_text,
            "wordFixed": word_fixed,
            "wordInBetween": word_in_between,
            "wordOptionPosition": word_option_position,
            "expression": expression,
        });
        if let Some(&idx) = order_index.get(&name) {
            ordered[idx].2.push(word_obj);
        } else {
            let idx = ordered.len();
            order_index.insert(name.clone(), idx);
            ordered.push((name, entity_type, vec![word_obj]));
        }
    }
    let result: Vec<serde_json::Value> = ordered
        .into_iter()
        .map(|(name, entity_type, words)| {
            serde_json::json!({
                "name": name,
                "type": entity_type,
                "words": words,
            })
        })
        .collect();
    serde_json::Value::Array(result)
}

fn extract_dialogs(
    content: &str,
) -> (
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
) {
    let json = serde_json::from_str::<serde_json::Value>(content)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
    let dialogs = json
        .pointer("/dialogs/result")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    let t_dialogs = match json.get("tDialogs") {
        Some(serde_json::Value::Array(arr)) => serde_json::Value::Array(arr.clone()),
        Some(obj) => obj
            .pointer("/result")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![])),
        None => serde_json::Value::Array(vec![]),
    };
    let conv_vars = json
        .get("conversationVariables")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    let ctx_vars = json
        .get("contextVariables")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    (dialogs, t_dialogs, conv_vars, ctx_vars)
}

fn emit_watch_event(app: &AppHandle, folder: &Path, reason: &str) {
    let payload = FolderWatchEvent {
        reason: reason.to_string(),
        folder: folder.to_string_lossy().into_owned(),
    };
    let _ = app.emit(WATCH_EVENT_NAME, payload);
}

fn matches_any_source(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|value| value.to_string_lossy()) else {
        return false;
    };
    if name.ends_with(".json")
        && source_definitions()
            .iter()
            .any(|definition| name.contains(definition.pattern))
    {
        return true;
    }
    // Also watch for the optional EntitiesExport CSV
    name.ends_with(".csv") && name.contains("EntitiesExport")
}

fn should_emit_for_event(event: &notify::Event) -> bool {
    if !matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    ) {
        return false;
    }
    event.paths.iter().any(|path| matches_any_source(path))
}

fn path_uses_selected_folder(path: &Path, selected_folder: Option<&Path>) -> bool {
    selected_folder
        .and_then(|folder| path.parent().map(|parent| parent == folder))
        .unwrap_or(false)
}

fn configure_folder_watch(
    app: &AppHandle,
    watch_state: &State<SharedWatchState>,
    selected_folder: Option<PathBuf>,
) {
    let mut state = watch_state.lock().expect("watch state lock poisoned");

    state.watcher = None;
    state.watched_folder = None;
    state.last_reload_signal = None;

    let Some(folder) = selected_folder.filter(|path| path.is_dir()) else {
        return;
    };

    let app_handle = app.clone();
    let state_handle = Arc::clone(&*watch_state);
    let watch_folder = folder.clone();
    let event_folder = folder.clone();

    let watcher_result = RecommendedWatcher::new(
        move |result: notify::Result<notify::Event>| {
            let Ok(event) = result else {
                return;
            };
            if !should_emit_for_event(&event) {
                return;
            }

            let mut state = state_handle.lock().expect("watch state lock poisoned");
            let now = Instant::now();
            if state
                .last_reload_signal
                .map(|instant| now.duration_since(instant) < Duration::from_millis(700))
                .unwrap_or(false)
            {
                return;
            }
            state.last_reload_signal = Some(now);
            drop(state);

            emit_watch_event(&app_handle, &event_folder, "filesystem-change");
        },
        Config::default(),
    );

    let Ok(mut watcher) = watcher_result else {
        return;
    };

    if watcher
        .watch(&watch_folder, RecursiveMode::NonRecursive)
        .is_ok()
    {
        state.watched_folder = Some(watch_folder);
        state.watcher = Some(watcher);
    }
}

// ── Commands ─────────────────────────────────────────────────────────────────

#[tauri::command]
fn get_data(
    app: AppHandle,
    watch_state: State<SharedWatchState>,
    args: Option<GetDataArgs>,
) -> AppData {
    let selected_folder = args.and_then(|value| value.selected_folder);
    let selected_folder_path = resolve_selected_folder(&selected_folder);
    let dirs = selected_folder_dirs(selected_folder_path.as_deref());
    let source_paths = find_source_files(&dirs);

    let mut articles = serde_json::Value::Array(vec![]);
    let mut dialogs = serde_json::Value::Array(vec![]);
    let mut t_dialogs = serde_json::Value::Array(vec![]);
    let mut entities = serde_json::Value::Array(vec![]);
    let mut conv_vars = serde_json::Value::Array(vec![]);
    let mut ctx_vars = serde_json::Value::Array(vec![]);
    let mut files = DataFiles {
        articles: None,
        dialogs: None,
        entities: None,
    };
    let mut source_files = SourceFiles {
        articles: None,
        dialogs: None,
        entities: None,
    };

    if let Some(path) = source_paths.get("articles") {
        if let Ok(content) = fs::read_to_string(path) {
            articles = extract_articles(&content);
            let filename = path.file_name().map(|n| n.to_string_lossy().into_owned());
            files.articles = filename.clone();
            source_files.articles = filename;
        }
    }

    if let Some(path) = source_paths.get("dialogs") {
        if let Ok(content) = fs::read_to_string(path) {
            let (loaded_dialogs, loaded_t_dialogs, loaded_conv_vars, loaded_ctx_vars) =
                extract_dialogs(&content);
            dialogs = loaded_dialogs;
            t_dialogs = loaded_t_dialogs;
            conv_vars = loaded_conv_vars;
            ctx_vars = loaded_ctx_vars;
            let filename = path.file_name().map(|n| n.to_string_lossy().into_owned());
            files.dialogs = filename.clone();
            source_files.dialogs = filename;
        }
    }

    if let Some(path) = find_entities_file(&dirs) {
        if let Ok(content) = fs::read_to_string(&path) {
            entities = extract_entities(&content);
            let filename = path.file_name().map(|n| n.to_string_lossy().into_owned());
            files.entities = filename.clone();
            source_files.entities = filename;
        }
    }

    let selected_folder_ref = selected_folder_path.as_deref();
    let using_selected_folder = selected_folder_path.is_some()
        && source_paths
            .values()
            .all(|path| path_uses_selected_folder(path, selected_folder_ref));

    let active_folder = selected_folder_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());

    configure_folder_watch(&app, &watch_state, selected_folder_path.clone());

    let watched_folder = watch_state
        .lock()
        .ok()
        .and_then(|state| state.watched_folder.clone())
        .map(|path| path.to_string_lossy().into_owned());

    let statuses = source_definitions()
        .iter()
        .map(|definition| {
            let filename = source_paths.get(definition.key).and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
            SourceStatus {
                key: definition.key.to_string(),
                label: definition.label.to_string(),
                found: filename.is_some(),
                filename,
            }
        })
        .collect::<Vec<_>>();

    let missing_sources = statuses
        .iter()
        .filter(|status| !status.found)
        .map(|status| status.label.clone())
        .collect::<Vec<_>>();

    AppData {
        articles,
        dialogs,
        t_dialogs,
        entities,
        conv_vars,
        ctx_vars,
        files,
        source_files,
        data_source: DataSourceInfo {
            selected_folder,
            active_folder,
            using_selected_folder,
            watched_folder,
            missing_sources,
            statuses,
        },
    }
}

#[tauri::command]
fn resize_to_available_height(app: tauri::AppHandle, height: f64, y: f64) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let scale = win.scale_factor().map_err(|e| e.to_string())?;
    let outer = win.outer_size().map_err(|e| e.to_string())?;
    let inner = win.inner_size().map_err(|e| e.to_string())?;
    let outer_pos = win.outer_position().map_err(|e| e.to_string())?;
    let current_w = outer.width as f64 / scale;
    let current_x = outer_pos.x as f64 / scale;
    // set_size sets the inner (client area) size, not the outer size.
    // Subtract the non-client chrome height (title bar + borders) so the
    // outer frame stays within the available area and does not overlap the taskbar.
    let chrome_h = (outer.height as f64 - inner.height as f64) / scale;
    let inner_h = (height - chrome_h).max(100.0);
    win.set_size(tauri::Size::Logical(tauri::LogicalSize {
        width: current_w,
        height: inner_h,
    }))
    .map_err(|e| e.to_string())?;
    win.set_position(tauri::Position::Logical(tauri::LogicalPosition {
        x: current_x,
        y,
    }))
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) {
    use tauri_plugin_opener::OpenerExt;
    if url.starts_with("https://") || url.starts_with("http://") || url.starts_with("tel:") {
        // Spawn in a detached thread so a blocking OS shell call (e.g. Windows
        // ShellExecute waiting on a security policy or UAC prompt) never
        // freezes the Tauri command executor or the UI.
        std::thread::spawn(move || {
            let _ = app.opener().open_url(url, None::<String>);
        });
    }
}

#[tauri::command]
fn open_preview_window(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("Invalid URL: only http/https allowed".to_string());
    }
    let parsed: tauri::Url = url
        .parse()
        .map_err(|e: <tauri::Url as std::str::FromStr>::Err| e.to_string())?;
    let label = "url-preview";
    // If a preview window is already open, close it first so we re-open fresh
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.close();
    }
    let truncated;
    let title = if url.len() > 80 {
        truncated = format!("...{}", &url[url.len() - 80..]);
        &truncated
    } else {
        &url
    };
    tauri::WebviewWindowBuilder::new(&app, label, tauri::WebviewUrl::External(parsed))
        .title(title)
        .inner_size(1200.0, 800.0)
        .resizable(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn select_data_folder(app: AppHandle) -> FolderSelectionResult {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();

    app.dialog().file().pick_folder(move |path| {
        let p: Option<PathBuf> = path.and_then(|folder| folder.into_path().ok());
        let _ = tx.send(p);
    });

    match rx.await.ok().flatten() {
        Some(path) => FolderSelectionResult {
            ok: true,
            canceled: false,
            path: Some(path.to_string_lossy().into_owned()),
        },
        None => FolderSelectionResult {
            ok: false,
            canceled: true,
            path: None,
        },
    }
}

#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle) -> UpdateResult {
    let current = app.package_info().version.to_string();

    let client = match reqwest::Client::builder()
        .user_agent("cm-conversation-dashboard")
        .timeout(std::time::Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return UpdateResult {
                status: "error".into(),
                version: None,
                message: Some(format!("Client error: {}", e)),
            }
        }
    };

    let resp = match client
        .get("https://api.github.com/repos/WithoutWout/cm-conversation-dashboard/releases/latest")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return UpdateResult {
                status: "error".into(),
                version: None,
                message: Some(format!("Network: {}", e)),
            }
        }
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            return UpdateResult {
                status: "error".into(),
                version: None,
                message: Some(format!("Parse error: {}", e)),
            }
        }
    };

    let latest = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches('v').to_string())
        .unwrap_or_default();

    if latest.is_empty() {
        let msg = json
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("no tag_name");
        return UpdateResult {
            status: "error".into(),
            version: None,
            message: Some(format!("API: {}", msg)),
        };
    }

    let current_ver = semver::Version::parse(&current).ok();
    let latest_ver = semver::Version::parse(&latest).ok();

    let is_newer = match (latest_ver, current_ver) {
        (Some(l), Some(c)) => l > c,
        // Fall back to string equality if either is unparseable
        _ => latest != current,
    };

    if is_newer {
        UpdateResult {
            status: "available".into(),
            version: Some(latest),
            message: None,
        }
    } else {
        UpdateResult {
            status: "up-to-date".into(),
            version: None,
            message: None,
        }
    }
}

#[tauri::command]
fn get_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

// ── DB helpers ───────────────────────────────────────────────────────────────

const DB_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS interactions (
    log_id                  INTEGER PRIMARY KEY,
    interaction_uuid        TEXT NOT NULL,
    session_uuid            TEXT NOT NULL,
    timestamp_start         TEXT NOT NULL,
    timestamp_end           TEXT,
    culture                 TEXT,
    main_interaction_type   TEXT,
    all_interaction_types   TEXT,
    interaction_value       TEXT,
    output_text             TEXT,
    article_ids             TEXT,
    dialog_paths            TEXT,
    tdialog_status          TEXT,
    recognition_type        TEXT,
    recognition_quality     REAL,
    generative_ai_sources   TEXT,
    articles                TEXT,
    faqs_found              TEXT,
    contexts                TEXT,
    pages                   TEXT,
    link_click_info         TEXT,
    feedback_info           TEXT,
    output_metadata         TEXT,
    recognition_details     TEXT,
    imported_at             INTEGER NOT NULL
);
-- Every index here is maintained on every inserted row, so each one is a tax on
-- import speed. Three former indexes were removed after checking the plans of
-- the queries that were supposed to use them; see DROP_DEAD_INDEXES below.
CREATE INDEX IF NOT EXISTS idx_timestamp     ON interactions(timestamp_start);
CREATE INDEX IF NOT EXISTS idx_session_ts    ON interactions(session_uuid, timestamp_start);
CREATE INDEX IF NOT EXISTS idx_session_log   ON interactions(session_uuid, log_id);
CREATE INDEX IF NOT EXISTS idx_recog_quality ON interactions(recognition_quality) WHERE recognition_quality > 0;
CREATE TABLE IF NOT EXISTS context_index (
    name         TEXT NOT NULL,
    value        TEXT NOT NULL,
    session_uuid TEXT NOT NULL,
    PRIMARY KEY (name, value, session_uuid)
);
CREATE INDEX IF NOT EXISTS idx_ctx_session ON context_index(session_uuid);
CREATE INDEX IF NOT EXISTS idx_ctx_name_session ON context_index(name, session_uuid);
CREATE TABLE IF NOT EXISTS session_summary (
    session_uuid                     TEXT PRIMARY KEY,
    first_ts                         TEXT NOT NULL,
    last_ts                          TEXT NOT NULL,
    interaction_count                INTEGER NOT NULL DEFAULT 0,
    culture                          TEXT NOT NULL DEFAULT '',
    first_user_message               TEXT NOT NULL DEFAULT '',
    contexts_snapshot                TEXT NOT NULL DEFAULT '',
    has_real_user_input              INTEGER NOT NULL DEFAULT 0,
    has_gen_ai                       INTEGER NOT NULL DEFAULT 0,
    has_neg_feedback                 INTEGER NOT NULL DEFAULT 0,
    has_pos_feedback                 INTEGER NOT NULL DEFAULT 0,
    min_positive_recognition_quality REAL NOT NULL DEFAULT 0,
    has_zero_recog                   INTEGER NOT NULL DEFAULT 0,
    updated_at                       INTEGER NOT NULL DEFAULT 0,
    last_log_id                      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_summary_first_ts ON session_summary(first_ts DESC);
CREATE INDEX IF NOT EXISTS idx_summary_real_first ON session_summary(has_real_user_input, first_ts DESC);
CREATE INDEX IF NOT EXISTS idx_summary_genai_first ON session_summary(has_gen_ai, first_ts DESC);
CREATE INDEX IF NOT EXISTS idx_summary_neg_first ON session_summary(has_neg_feedback, first_ts DESC);
CREATE INDEX IF NOT EXISTS idx_summary_pos_first ON session_summary(has_pos_feedback, first_ts DESC);
CREATE INDEX IF NOT EXISTS idx_summary_zero_first ON session_summary(has_zero_recog, first_ts DESC);
CREATE INDEX IF NOT EXISTS idx_summary_recog_first ON session_summary(min_positive_recognition_quality, first_ts DESC);
CREATE TABLE IF NOT EXISTS app_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- Which UTC hours were actually *requested* from the Analytics API, as opposed
-- to which ones happen to hold rows.
--
-- Coverage used to be inferred purely from row presence, which cannot tell
-- "we asked and the API had nothing" apart from "we never asked". An hour with
-- genuinely zero interactions — a quiet night, a maintenance window — then read
-- as a permanent gap: the day never showed as fully imported and was
-- re-downloaded on every run, forever. Recording the window closes that.
--
-- Row presence still counts (see `get_db_hour_coverage`, which returns the
-- union), so manually imported portal CSVs — which have no request window —
-- keep marking the hours they cover exactly as before.
CREATE TABLE IF NOT EXISTS imported_windows (
    day   TEXT PRIMARY KEY,           -- UTC 'YYYY-MM-DD'
    hours INTEGER NOT NULL DEFAULT 0  -- bitmask of the UTC hours 0..23 fetched
);
"#;

/// Indexes that cost a b-tree write on every imported row and bought nothing.
///
/// `DB_SCHEMA` uses `CREATE INDEX IF NOT EXISTS`, so removing the lines there
/// only helps databases created from now on — existing files keep paying for
/// them forever unless they are dropped explicitly.
///
/// Verified with `EXPLAIN QUERY PLAN` against the real queries before removal:
/// - `idx_feedback` — every feedback filter is a leading-wildcard
///   `LIKE '%"score"…%'`, which no b-tree can serve, and the planner confirms a
///   full `SCAN`. Its key was the entire feedback JSON blob, making it the
///   widest and most expensive of the set for zero benefit.
/// - `idx_session_uuid` — a strict prefix of `idx_session_ts` and
///   `idx_session_log`; the planner picks one of those for a bare
///   `session_uuid = ?` either way.
/// - `idx_type` — the only equality on `main_interaction_type` is ORed with a
///   `LIKE '%…%'` (which forces a scan of the other branch); every other use is
///   a `!=` or `NOT IN` negation, or wrapped in `COALESCE`. The planner never
///   chose it.
const DROP_DEAD_INDEXES: &str = "\
DROP INDEX IF EXISTS idx_session_uuid;\
DROP INDEX IF EXISTS idx_feedback;\
DROP INDEX IF EXISTS idx_type;";

/// Set for the duration of an import run and cleared by `finalize_import_run`.
///
/// The touched-session table is per-connection and dies with the process, so a
/// run that never finalizes leaves no in-database trace of which sessions went
/// stale. This durable flag is that trace: [`open_db`] sees it and does a full
/// summary rebuild. See [`ensure_session_summary`] for why its own two
/// invariants are not enough on their own.
const META_PENDING_FINALIZE: &str = "pending_finalize";

// ── Flagged DB schema ────────────────────────────────────────────────────────

const FLAGGED_DB_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS flagged_folders (
    folder_id   INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    sort_order  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS flagged_sessions (
    flag_id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_uuid      TEXT NOT NULL,
    flagged_at        TEXT NOT NULL,
    source_db_path    TEXT NOT NULL DEFAULT '',
    culture           TEXT NOT NULL DEFAULT '',
    first_ts          TEXT NOT NULL DEFAULT '',
    interaction_count INTEGER NOT NULL DEFAULT 0,
    folder_id         INTEGER REFERENCES flagged_folders(folder_id) ON DELETE SET NULL,
    notes             TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS flagged_interactions (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    flag_id               INTEGER NOT NULL REFERENCES flagged_sessions(flag_id) ON DELETE CASCADE,
    log_id                INTEGER,
    interaction_uuid      TEXT NOT NULL DEFAULT '',
    session_uuid          TEXT NOT NULL DEFAULT '',
    timestamp_start       TEXT NOT NULL DEFAULT '',
    timestamp_end         TEXT NOT NULL DEFAULT '',
    culture               TEXT NOT NULL DEFAULT '',
    main_interaction_type TEXT NOT NULL DEFAULT '',
    all_interaction_types TEXT NOT NULL DEFAULT '',
    interaction_value     TEXT NOT NULL DEFAULT '',
    output_text           TEXT NOT NULL DEFAULT '',
    article_ids           TEXT NOT NULL DEFAULT '',
    dialog_paths          TEXT NOT NULL DEFAULT '',
    tdialog_status        TEXT NOT NULL DEFAULT '',
    recognition_type      TEXT NOT NULL DEFAULT '',
    recognition_quality   REAL NOT NULL DEFAULT 0.0,
    generative_ai_sources TEXT NOT NULL DEFAULT '',
    articles              TEXT NOT NULL DEFAULT '',
    faqs_found            TEXT NOT NULL DEFAULT '',
    contexts              TEXT NOT NULL DEFAULT '',
    pages                 TEXT NOT NULL DEFAULT '',
    link_click_info       TEXT NOT NULL DEFAULT '',
    feedback_info         TEXT NOT NULL DEFAULT '',
    output_metadata       TEXT NOT NULL DEFAULT '',
    recognition_details   TEXT NOT NULL DEFAULT '',
    is_flagged            INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_fi_flag_id ON flagged_interactions(flag_id);
"#;

// Best-effort performance pragmas. Some pragma setters return a result row,
// so each runs via query_row; a failure only costs speed, never correctness.
fn apply_perf_pragmas(conn: &Connection) {
    // 64 MiB page cache (negative value = KiB units)
    let _ = conn.query_row("PRAGMA cache_size = -65536", [], |_| Ok(()));
    // Keep temp b-trees (CTE materialization, GROUP BY, ORDER BY) in memory
    let _ = conn.query_row("PRAGMA temp_store = MEMORY", [], |_| Ok(()));
    // 256 MiB memory-mapped I/O window for read-heavy scans
    let _ = conn.query_row("PRAGMA mmap_size = 268435456", [], |_| Ok(()));
    // Bound ANALYZE so "PRAGMA optimize" *samples* indexes instead of fully
    // scanning every one of them. The limit defaults to 0 (unlimited), so
    // without this every post-import optimize walked all of interactions'
    // indexes end to end. 400 is SQLite's own recommended value.
    let _ = conn.query_row("PRAGMA analysis_limit = 400", [], |_| Ok(()));
}

fn open_flagged_db(path: &str) -> Result<Connection, String> {
    if let Some(parent) = Path::new(path).parent() {
        let _ = fs::create_dir_all(parent);
    }
    let conn = Connection::open(path).map_err(|e| format!("Cannot open flagged DB: {e}"))?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .map_err(|e| format!("PRAGMA error: {e}"))?;
    conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")
        .map_err(|e| format!("PRAGMA error: {e}"))?;
    apply_perf_pragmas(&conn);
    conn.execute_batch(FLAGGED_DB_SCHEMA)
        .map_err(|e| format!("Schema error: {e}"))?;
    // Migrations for existing DBs (ignore errors if column already exists)
    let _ = conn.execute_batch("ALTER TABLE flagged_sessions ADD COLUMN folder_id INTEGER REFERENCES flagged_folders(folder_id) ON DELETE SET NULL");
    let _ = conn
        .execute_batch("ALTER TABLE flagged_sessions ADD COLUMN notes TEXT NOT NULL DEFAULT ''");
    Ok(conn)
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let mut rem = secs / 86400;
    let mut year = 1970u64;
    loop {
        let in_year: u64 = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            366
        } else {
            365
        };
        if rem < in_year {
            break;
        }
        rem -= in_year;
        year += 1;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for &d in &month_days {
        if rem < d {
            break;
        }
        rem -= d;
        month += 1;
    }
    let day = rem + 1;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, h, m, s
    )
}

// FTS5 schema is kept separate so a missing fts5 module never prevents the DB from opening.
//
// `content = ''` makes this a contentless index: it stores the tokens needed to
// answer a MATCH and nothing else. The previous form was a standalone table,
// which also kept a full second copy of all four columns in
// `interactions_fts_content` — roughly a third of the database file, written
// again on every imported row, and never once read back. Nothing in this crate
// selects an FTS column value or calls snippet()/highlight()/bm25(); every use
// is a MATCH plus a rowid join.
//
// `contentless_delete = 1` (SQLite 3.43+, and 3.45 is bundled) is what keeps
// the plain `DELETE FROM interactions_fts WHERE rowid IN (…)` in purge_old and
// delete_interactions_by_dates working unchanged.
//
// Measured on 200k synthetic rows: 23% faster to insert, 37% smaller on disk,
// and search timings equal or slightly better — including after a large delete,
// where the tombstones a contentless index keeps might have cost something.
// See `fts_bench::contentless_vs_standalone`.
//
// One consequence to keep in mind: inserting a duplicate rowid no longer raises
// a constraint error, it silently double-indexes. That makes the `Ok(1)` gate
// in the import loop load-bearing for index correctness, not just for speed.
const FTS_SCHEMA: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS interactions_fts USING fts5(
    interaction_value,
    output_text,
    article_ids,
    dialog_paths,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 1'
);
"#;

/// Marker distinguishing the current FTS schema from the pre-migration one.
///
/// Matched against `sqlite_master.sql`, which preserves the literal option text
/// a virtual table was created with. Deliberately matches the *new* option
/// rather than the absence of an old one: the legacy DDL contains no
/// distinguishing token of its own.
const FTS_CONTENTLESS_MARKER: &str = "contentless_delete";

/// Per-connection temp table listing the sessions an import — and any purge that
/// import triggered — actually touched. Populated during the CSV batches and by
/// [`purge_old`], consumed by [`rebuild_session_summary_touched`].
const TOUCHED_TABLE: &str = "temp.import_touched_sessions";

/// (Re)create an empty touched-session table for a fresh import run.
fn reset_touched_sessions(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(&format!(
        "DROP TABLE IF EXISTS {TOUCHED_TABLE};\
         CREATE TEMP TABLE import_touched_sessions (session_uuid TEXT PRIMARY KEY);"
    ))
    .map_err(|e| format!("Touched-session table error: {e}"))
}

/// Create the touched-session table only if it isn't already there.
///
/// Used by every file in a deferred run, so the set *accumulates* across the
/// whole run and one finalize covers all of it. Deliberately not a reset: the
/// run's earlier files must stay in the set, and leaving the table in place
/// also spares the cached statements a `SQLITE_SCHEMA` re-prepare.
fn ensure_touched_sessions(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS import_touched_sessions (session_uuid TEXT PRIMARY KEY);",
    )
    .map_err(|e| format!("Touched-session table error: {e}"))
}

fn drop_touched_sessions(conn: &Connection) {
    let _ = conn.execute_batch(&format!("DROP TABLE IF EXISTS {TOUCHED_TABLE};"));
}

/// True when the touched-session table exists and holds at least one session.
fn has_touched_sessions(conn: &Connection) -> bool {
    conn.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {TOUCHED_TABLE})"),
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

fn set_meta_flag(conn: &Connection, key: &str) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO app_meta(key, value) VALUES (?1, '1')",
        params![key],
    );
}

fn clear_meta_flag(conn: &Connection, key: &str) {
    let _ = conn.execute("DELETE FROM app_meta WHERE key = ?1", params![key]);
}

fn meta_flag_set(conn: &Connection, key: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_meta WHERE key = ?1)",
        params![key],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

/// The `SELECT` that derives one `session_summary` row per session.
///
/// Shared by the full rebuild and the scoped per-import rebuild so the two can
/// never drift apart. `scope` is either empty (every session) or an extra
/// `AND …` clause narrowing it to the sessions an import touched.
fn session_summary_insert_sql(scope: &str) -> String {
    format!(
        r#"
INSERT INTO session_summary (
    session_uuid,
    first_ts,
    last_ts,
    interaction_count,
    culture,
    first_user_message,
    contexts_snapshot,
    has_real_user_input,
    has_gen_ai,
    has_neg_feedback,
    has_pos_feedback,
    min_positive_recognition_quality,
    has_zero_recog,
    updated_at,
    last_log_id
)
SELECT
    s.session_uuid,
    MIN(s.timestamp_start) AS first_ts,
    MAX(COALESCE(NULLIF(s.timestamp_end, ''), s.timestamp_start)) AS last_ts,
    COUNT(*) AS interaction_count,
    COALESCE(MIN(NULLIF(s.culture, '')), '') AS culture,
    COALESCE((
        SELECT i2.interaction_value
        FROM interactions i2
        WHERE i2.session_uuid = s.session_uuid
          AND i2.interaction_value != ''
          AND i2.interaction_value NOT LIKE '#%#'
          AND LOWER(i2.interaction_value) != 'continue'
          AND COALESCE(i2.main_interaction_type, '') NOT IN ('Event', 'LinkClick')
        ORDER BY i2.log_id ASC
        LIMIT 1
    ), '') AS first_user_message,
    COALESCE((
        SELECT i3.contexts
        FROM interactions i3
        WHERE i3.session_uuid = s.session_uuid
          AND i3.contexts IS NOT NULL
          AND i3.contexts != ''
          AND i3.contexts != '[]'
          AND i3.contexts != 'null'
        ORDER BY i3.log_id DESC
        LIMIT 1
    ), '') AS contexts_snapshot,
    MAX(CASE
        WHEN s.interaction_value != ''
         AND s.interaction_value NOT LIKE '#%#'
         AND LOWER(s.interaction_value) != 'continue'
         AND COALESCE(s.main_interaction_type, '') NOT IN ('Event', 'LinkClick')
        THEN 1 ELSE 0 END) AS has_real_user_input,
    MAX(CASE
        WHEN s.main_interaction_type = 'GenerativeAI'
          OR s.all_interaction_types LIKE '%GenerativeAI%'
        THEN 1 ELSE 0 END) AS has_gen_ai,
    MAX(CASE
        WHEN s.feedback_info LIKE '%"score": -1%'
          OR s.feedback_info LIKE '%"score":-1%'
        THEN 1 ELSE 0 END) AS has_neg_feedback,
    MAX(CASE
        WHEN (s.feedback_info LIKE '%"score": 1%'
           OR s.feedback_info LIKE '%"score":1%')
          AND s.feedback_info NOT LIKE '%"score": -1%'
          AND s.feedback_info NOT LIKE '%"score":-1%'
        THEN 1 ELSE 0 END) AS has_pos_feedback,
    COALESCE(MIN(CASE
        WHEN s.recognition_quality > 0
         AND COALESCE(s.main_interaction_type, '') != 'GenerativeAI'
         AND COALESCE(s.recognition_type, '') != 'GenerativeAI'
        THEN s.recognition_quality END), 0) AS min_positive_recognition_quality,
    MAX(CASE
        WHEN s.recognition_quality = 0
         AND s.recognition_type IS NOT NULL
         AND s.recognition_type != ''
         AND s.recognition_type != 'GenerativeAI'
         AND COALESCE(s.main_interaction_type, '') != 'GenerativeAI'
        THEN 1 ELSE 0 END) AS has_zero_recog,
    CAST(strftime('%s', 'now') AS INTEGER) AS updated_at,
    MAX(s.log_id) AS last_log_id
FROM interactions s
WHERE s.session_uuid IS NOT NULL AND s.session_uuid != '' {scope}
GROUP BY s.session_uuid;
"#
    )
}

/// Recompute `session_summary` for every session in the database.
///
/// Used when opening a database and after bulk deletions that are not scoped to
/// a known set of sessions. Cost is proportional to the whole database, so the
/// import path uses [`rebuild_session_summary_touched`] instead.
fn rebuild_session_summary(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("DELETE FROM session_summary;")
        .map_err(|e| format!("Session summary rebuild error: {e}"))?;
    conn.execute_batch(&session_summary_insert_sql(""))
        .map_err(|e| format!("Session summary rebuild error: {e}"))
}

/// Recompute `session_summary` for only the sessions in [`TOUCHED_TABLE`].
///
/// A session's summary is derived entirely from that session's own rows in
/// `interactions`, so recomputing just the touched sessions is exact — every
/// untouched summary was already correct. Unlike the full rebuild, the cost is
/// proportional to the size of the import rather than to the size of the whole
/// database, which is what keeps import time flat as the database grows.
///
/// Sessions whose rows were all purged are deleted here and produce no group in
/// the re-insert, so they correctly drop out of the summary.
fn rebuild_session_summary_touched(conn: &Connection) -> Result<(), String> {
    conn.execute(
        &format!(
            "DELETE FROM session_summary \
             WHERE session_uuid IN (SELECT session_uuid FROM {TOUCHED_TABLE})"
        ),
        [],
    )
    .map_err(|e| format!("Session summary rebuild error: {e}"))?;
    conn.execute_batch(&session_summary_insert_sql(&format!(
        "AND s.session_uuid IN (SELECT session_uuid FROM {TOUCHED_TABLE})"
    )))
    .map_err(|e| format!("Session summary rebuild error: {e}"))
}

/// Cheap consistency check run on every database open, rebuilding the summary
/// in full when it is obviously out of step with `interactions`.
///
/// Deliberately cheap rather than exhaustive: it catches sessions that appeared
/// or vanished, and rows appended past the highest known `log_id`. It does *not*
/// catch rows added to already-known sessions below that high-water mark — an
/// abandoned import run backfilling an older window can do exactly that. The
/// [`META_PENDING_FINALIZE`] flag covers that case; see [`open_db`].
fn ensure_session_summary(conn: &Connection) -> Result<(), String> {
    let interaction_sessions: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT session_uuid) FROM interactions WHERE session_uuid IS NOT NULL AND session_uuid != ''",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let summary_sessions: i64 = conn
        .query_row("SELECT COUNT(*) FROM session_summary", [], |r| r.get(0))
        .unwrap_or(0);
    let max_interaction_log: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(log_id), 0) FROM interactions",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let max_summary_log: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(last_log_id), 0) FROM session_summary",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if interaction_sessions != summary_sessions || max_interaction_log != max_summary_log {
        rebuild_session_summary(conn)?;
    }
    Ok(())
}

fn cleanup_orphan_contexts(conn: &Connection) {
    let _ = conn.execute_batch(
        "DELETE FROM context_index \
         WHERE session_uuid NOT IN (SELECT DISTINCT session_uuid FROM interactions)",
    );
}

/// Drop context rows whose session no longer has any interactions, limited to
/// the sessions in [`TOUCHED_TABLE`].
///
/// Only deletions can orphan a context row, so a purge is the only thing in the
/// import path that needs this — an import on its own strictly adds sessions and
/// can never orphan anything.
fn cleanup_orphan_contexts_touched(conn: &Connection) {
    let _ = conn.execute(
        &format!(
            "DELETE FROM context_index \
             WHERE session_uuid IN (SELECT session_uuid FROM {TOUCHED_TABLE}) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM interactions i \
                   WHERE i.session_uuid = context_index.session_uuid)"
        ),
        [],
    );
}

/// Bring the FTS index in step with `interactions`, migrating its schema first
/// if the database still has the old content-storing table.
///
/// Returns true when it actually reindexed, so the caller can tell the user why
/// opening took a while.
fn repair_fts_index(conn: &Connection) -> bool {
    // A database written before the contentless migration carries a table that
    // duplicates every indexed column. Drop it and let the count check below
    // rebuild from `interactions`, which is the authoritative copy either way.
    let legacy = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='interactions_fts' \
               AND sql NOT LIKE '%' || ?1 || '%'",
            params![FTS_CONTENTLESS_MARKER],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if legacy {
        // Only drop once the replacement is known to be creatable, so a build
        // without fts5 leaves the working old index alone rather than losing it.
        if conn.execute_batch("DROP TABLE interactions_fts;").is_err() {
            return false;
        }
    }
    if conn.execute_batch(FTS_SCHEMA).is_err() {
        return false;
    }
    let interaction_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM interactions", [], |r| r.get(0))
        .unwrap_or(0);
    let fts_row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM interactions_fts", [], |r| r.get(0))
        .unwrap_or(-1);
    if interaction_count != fts_row_count {
        let started = Instant::now();
        let ok = conn
            .execute_batch(
                "DELETE FROM interactions_fts; \
                 INSERT INTO interactions_fts(rowid, interaction_value, output_text, article_ids, dialog_paths) \
                 SELECT log_id, COALESCE(interaction_value,''), COALESCE(output_text,''), \
                        COALESCE(article_ids,''), COALESCE(dialog_paths,'') \
                 FROM interactions",
            )
            .is_ok();
        if ok {
            log::info!(
                target: "import",
                "reindexed {interaction_count} rows for search in {}ms{}",
                started.elapsed().as_millis(),
                if legacy { " (contentless migration)" } else { "" }
            );
        }
        return ok && interaction_count > 0;
    }
    false
}

fn open_db(path: &str) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("Cannot open DB: {e}"))?;
    // PRAGMA journal_mode returns a result row, so it must be run via query_row.
    // PRAGMA synchronous is a pure setter and works via execute_batch.
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .map_err(|e| format!("PRAGMA error: {e}"))?;
    conn.execute_batch("PRAGMA synchronous=NORMAL;")
        .map_err(|e| format!("PRAGMA error: {e}"))?;
    apply_perf_pragmas(&conn);
    conn.execute_batch(DB_SCHEMA)
        .map_err(|e| format!("Schema error: {e}"))?;
    // Migrate existing databases: drop the indexes that only ever cost import
    // time. Dropping is a page-deallocation, so it is fast and needs no VACUUM
    // to stop the per-insert write cost.
    let _ = conn.execute_batch(DROP_DEAD_INDEXES);
    // Migrate existing databases: add recognition_details column if absent
    let _ = conn.execute_batch("ALTER TABLE interactions ADD COLUMN recognition_details TEXT");
    // Backfill context_index from existing interactions (one-time migration).
    // Uses json_each, but only runs once — subsequent imports maintain the index incrementally.
    let ctx_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM context_index", [], |r| r.get(0))
        .unwrap_or(0);
    if ctx_count == 0 {
        conn.execute_batch(
            "INSERT OR IGNORE INTO context_index(name, value, session_uuid) \
             SELECT json_extract(c.value, '$.name'), \
                    json_extract(c.value, '$.value'), \
                    i.session_uuid \
             FROM interactions i, json_each(i.contexts) c \
             WHERE i.contexts IS NOT NULL \
               AND i.contexts != '' \
               AND i.contexts != '[]' \
               AND i.contexts != 'null' \
               AND json_extract(c.value, '$.name') IS NOT NULL \
               AND json_extract(c.value, '$.name') != ''",
        )
        .ok();
    }

    // Optional FTS5 and materialized summaries are repairable caches.
    repair_fts_index(&conn);
    // An import run that never reached its finalize step left session_summary
    // stale. Only a full rebuild can fix it: the touched-session table was
    // per-connection and died with the process that abandoned the run.
    if meta_flag_set(&conn, META_PENDING_FINALIZE) {
        log::warn!(
            target: "import",
            "previous import run did not finalize — rebuilding session_summary in full"
        );
        rebuild_session_summary(&conn)?;
        clear_meta_flag(&conn, META_PENDING_FINALIZE);
    }
    ensure_session_summary(&conn)?;
    // One-time ANALYZE so the query planner has statistics for the
    // session_summary/context_index indexes; "PRAGMA optimize" after imports
    // keeps them fresh. The sampling bound comes from apply_perf_pragmas, which
    // sets analysis_limit for the whole connection.
    let has_stats = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sqlite_stat1'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_stats {
        let _ = conn.execute_batch("ANALYZE;");
    }
    Ok(conn)
}

/// Convert MM/DD/YYYY HH:MM:SS to ISO-8601 (YYYY-MM-DDTHH:MM:SS)
///
/// Also normalizes timestamps that already arrive in ISO form. The Analytics
/// API can return `2026-03-25T09:30:22.605Z`; truncating to seconds keeps rows
/// imported from the API byte-identical to rows imported from a portal CSV,
/// which every `DATE(timestamp_start)` and range comparison in this app relies
/// on.
/// Allocating convenience form. The import path uses [`parse_ts_into`] with a
/// reused buffer; this exists so tests can assert on a plain `String`.
#[cfg(test)]
fn parse_ts(s: &str) -> String {
    let mut out = String::new();
    parse_ts_into(s, &mut out);
    out
}

/// [`parse_ts`] writing into a caller-owned buffer.
///
/// Called twice per imported row, so the allocations matter: the old shape
/// built a `Vec<&str>` per `split` plus a `String` per `format!`, four heap
/// allocations per row for what is fixed-width byte shuffling. The buffer is
/// reused across rows and only ever grows once.
fn parse_ts_into(s: &str, out: &mut String) {
    // expected: "03/25/2026 09:30:22"
    let s = s.trim();
    out.clear();
    let b = s.as_bytes();
    if b.len() >= 19 {
        // Portal form: MM/DD/YYYY followed by a space and HH:MM:SS. Every field
        // is fixed width, so index rather than split.
        if b[2] == b'/'
            && b[5] == b'/'
            && b[10] == b' '
            && b[..10]
                .iter()
                .enumerate()
                .all(|(i, c)| i == 2 || i == 5 || c.is_ascii_digit())
        {
            out.push_str(&s[6..10]); // YYYY
            out.push('-');
            out.push_str(&s[0..2]); // MM
            out.push('-');
            out.push_str(&s[3..5]); // DD
            out.push('T');
            // Deliberately the rest of the string, not just 8 bytes: the old
            // `splitn(2, ' ')` kept everything after the space, and some rows
            // carry sub-second or offset text that callers have always seen.
            out.push_str(&s[11..]);
            return;
        }
        // Same shape but not zero-padded ("3/5/2026 09:30:22"). Rare enough not
        // to optimise, but the original accepted it and the output must not
        // change for input this function already handled.
        if let Some((date, time)) = s.split_once(' ') {
            let mut it = date.split('/');
            if let (Some(mm), Some(dd), Some(yyyy), None) =
                (it.next(), it.next(), it.next(), it.next())
            {
                out.push_str(yyyy);
                out.push('-');
                out.push_str(mm);
                out.push('-');
                out.push_str(dd);
                out.push('T');
                out.push_str(time);
                return;
            }
        }
        if is_iso_second_prefix(s) {
            // Normalize the date/time separator to 'T' so a space-separated
            // ISO timestamp stores identically to a 'T'-separated one.
            out.push_str(&s[..10]);
            out.push('T');
            out.push_str(&s[11..19]);
            return;
        }
    }
    out.push_str(s);
}

/// True when the first 19 bytes look exactly like `YYYY-MM-DDTHH:MM:SS`.
fn is_iso_second_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 19 {
        return false;
    }
    let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    let dashes = [4, 7];
    let colons = [13, 16];
    digits.iter().all(|&i| b[i].is_ascii_digit())
        && dashes.iter().all(|&i| b[i] == b'-')
        && colons.iter().all(|&i| b[i] == b':')
        && (b[10] == b'T' || b[10] == b' ')
}

/// Delete interactions older than `max_days`.
///
/// Records every affected session in [`TOUCHED_TABLE`] rather than rebuilding
/// `session_summary` itself, so one scoped rebuild in the caller covers both the
/// import and the purge instead of running two full rebuilds back to back.
///
/// Callers must have created the touched-session table via
/// [`reset_touched_sessions`] and must run [`rebuild_session_summary_touched`]
/// afterwards when this returns a non-zero count.
fn purge_old(conn: &Connection, max_days: u64) -> i64 {
    let cutoff = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        secs.saturating_sub(max_days * 24 * 3600)
    };
    // timestamp_start stored as ISO text "YYYY-MM-DDTHH:MM:SS"
    // We compare against an ISO cutoff string
    let cutoff_dt = {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(cutoff);
        let secs = cutoff;
        let s_secs = secs % 60;
        let mins = secs / 60 % 60;
        let hrs = secs / 3600 % 24;
        let days_since_epoch = secs / 86400;
        // Simple date calc from epoch
        let mut year = 1970u32;
        let mut rem_days = days_since_epoch as u32;
        loop {
            let days_in_year = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                366
            } else {
                365
            };
            if rem_days < days_in_year {
                break;
            }
            rem_days -= days_in_year;
            year += 1;
        }
        let month_days: [u32; 12] = [
            31,
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let mut month = 1u32;
        for &d in &month_days {
            if rem_days < d {
                break;
            }
            rem_days -= d;
            month += 1;
        }
        let day = rem_days + 1;
        let _ = t; // suppress unused
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            year, month, day, hrs, mins, s_secs
        )
    };
    // Record the sessions about to lose rows *before* they are deleted, so the
    // caller's scoped rebuild covers them.
    let _ = conn.execute(
        &format!(
            "INSERT OR IGNORE INTO {TOUCHED_TABLE}(session_uuid) \
             SELECT DISTINCT session_uuid FROM interactions \
             WHERE timestamp_start < ?1 AND session_uuid != ''"
        ),
        params![cutoff_dt],
    );
    // Remove stale FTS5 entries before deleting from interactions
    let _ = conn.execute(
        "DELETE FROM interactions_fts WHERE rowid IN \
         (SELECT log_id FROM interactions WHERE timestamp_start < ?1)",
        params![cutoff_dt],
    );
    let deleted = conn
        .execute(
            "DELETE FROM interactions WHERE timestamp_start < ?1",
            params![cutoff_dt],
        )
        .unwrap_or(0) as i64;
    // Drop the coverage record for whatever was purged, or those hours would
    // read as "already imported" while holding no rows. The cutoff falls
    // mid-day, so the day it lands in keeps the hours at or after it.
    let cutoff_day = &cutoff_dt[..10];
    let cutoff_hour: u32 = cutoff_dt[11..13].parse().unwrap_or(0);
    let _ = conn.execute(
        "DELETE FROM imported_windows WHERE day < ?1",
        params![cutoff_day],
    );
    if cutoff_hour > 0 {
        let below_cutoff: i64 = (1 << cutoff_hour) - 1;
        let _ = conn.execute(
            "UPDATE imported_windows SET hours = hours & ~?2 WHERE day = ?1",
            params![cutoff_day, below_cutoff],
        );
    }
    if deleted > 0 {
        cleanup_orphan_contexts_touched(conn);
    }
    deleted
}

// ── Conversation Tauri commands ───────────────────────────────────────────────

#[tauri::command]
async fn set_db_path(
    db_state: State<'_, SharedDbState>,
    search_interrupt: State<'_, SharedSearchInterrupt>,
    path: String,
) -> Result<(), String> {
    let db = db_state.inner().clone();
    let interrupt_state = search_interrupt.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&path)?;
        let interrupt_handle = Arc::new(conn.get_interrupt_handle());
        let mut state = db.lock().map_err(|e| e.to_string())?;
        state.conn = Some(conn);
        state.path = Some(path);
        let mut ih = interrupt_state.lock().map_err(|e| e.to_string())?;
        *ih = Some(interrupt_handle);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_db_path(db_state: State<SharedDbState>) -> Option<String> {
    db_state.lock().ok().and_then(|s| s.path.clone())
}

#[tauri::command]
fn cancel_session_search(search_interrupt: State<SharedSearchInterrupt>) -> Result<(), String> {
    if let Some(handle) = search_interrupt.lock().map_err(|e| e.to_string())?.as_ref() {
        handle.interrupt();
    }
    Ok(())
}

#[tauri::command]
async fn select_csv_files(app: AppHandle) -> FileDialogResult {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel::<Vec<String>>();

    app.dialog()
        .file()
        .add_filter("CSV files", &["csv"])
        .pick_files(move |paths| {
            let result = paths
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.into_path().ok())
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let _ = tx.send(result);
        });

    match rx.await {
        Ok(paths) if !paths.is_empty() => FileDialogResult {
            ok: true,
            canceled: false,
            paths,
        },
        _ => FileDialogResult {
            ok: false,
            canceled: true,
            paths: vec![],
        },
    }
}

#[tauri::command]
async fn select_db_save_path(app: AppHandle) -> FileSaveResult {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();

    app.dialog()
        .file()
        .add_filter("SQLite Database", &["db"])
        .set_file_name("conversations.db")
        .save_file(move |path| {
            let p = path.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(p);
        });

    match rx.await.ok().flatten() {
        Some(path) => FileSaveResult {
            ok: true,
            canceled: false,
            path: Some(path.to_string_lossy().into_owned()),
        },
        None => FileSaveResult {
            ok: false,
            canceled: true,
            path: None,
        },
    }
}

#[tauri::command]
async fn save_collection_export(
    app: AppHandle,
    default_name: String,
    content: String,
) -> Result<FileSaveResult, String> {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();

    app.dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name(&default_name)
        .save_file(move |path| {
            let p = path.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(p);
        });

    let Some(mut path) = rx.await.ok().flatten() else {
        return Ok(FileSaveResult {
            ok: false,
            canceled: true,
            path: None,
        });
    };
    if path.extension().and_then(|e| e.to_str()) != Some("json") {
        path.set_extension("json");
    }
    fs::write(&path, content).map_err(|e| format!("Cannot write export file: {e}"))?;

    Ok(FileSaveResult {
        ok: true,
        canceled: false,
        path: Some(path.to_string_lossy().into_owned()),
    })
}

#[tauri::command]
async fn select_db_open_path(app: AppHandle) -> FileSaveResult {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();

    app.dialog()
        .file()
        .add_filter("SQLite Database", &["db"])
        .pick_file(move |path| {
            let p = path.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(p);
        });

    match rx.await.ok().flatten() {
        Some(path) => FileSaveResult {
            ok: true,
            canceled: false,
            path: Some(path.to_string_lossy().into_owned()),
        },
        None => FileSaveResult {
            ok: false,
            canceled: true,
            path: None,
        },
    }
}

/// Open an import run: one user action that may feed many CSV files through
/// [`import_interactions_csv`] before [`finalize_import_run`] closes it.
///
/// Everything the old per-file tail did — purge, summary rebuild, FTS merge,
/// planner stats — is derived state that only has to be correct once the run is
/// over. A 90-day API import used to pay all of it 90 times.
#[tauri::command]
async fn begin_import_run(db_state: State<'_, SharedDbState>) -> Result<(), String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut state = db.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_mut()
            .ok_or("No database open. Set a database path first.")?;
        reset_touched_sessions(conn)?;
        set_meta_flag(conn, META_PENDING_FINALIZE);
        // A run pushes far more than the default 1000-page (~4 MiB) WAL
        // threshold, so SQLite would otherwise checkpoint — fsync plus
        // copy-back — repeatedly *during* the import. finalize restores it.
        let _ = conn.query_row("PRAGMA wal_autocheckpoint = 20000", [], |_| Ok(()));
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Close an import run: do once what used to happen after every file.
///
/// Safe to call when no run is open (the touched table is simply absent), so
/// the renderer can put it in a `finally` without tracking whether the run got
/// far enough to need it.
#[tauri::command]
async fn finalize_import_run(
    db_state: State<'_, SharedDbState>,
    max_age_days: Option<i64>,
) -> Result<FinalizeResult, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut state = db.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_mut()
            .ok_or("No database open. Set a database path first.")?;
        finalize_import_run_into(conn, max_age_days)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The finalize step, against an open connection, so tests can drive a whole
/// deferred run without a Tauri `State`.
fn finalize_import_run_into(
    conn: &mut Connection,
    max_age_days: Option<i64>,
) -> Result<FinalizeResult, String> {
    let started = Instant::now();
    let mut timings = ImportTimings::default();

    let t = Instant::now();
    let purged = purge_old(conn, max_age_days.unwrap_or(90).max(1) as u64);
    timings.purge_ms = t.elapsed().as_millis() as u64;

    let mut rebuild = Ok(());
    if purged > 0 || has_touched_sessions(conn) {
        let t = Instant::now();
        rebuild = rebuild_session_summary_touched(conn);
        timings.summary_ms = t.elapsed().as_millis() as u64;

        // Merge FTS5 b-tree segments so MATCH queries read fewer pages, and
        // refresh planner statistics for the tables this run touched.
        let t = Instant::now();
        let _ = conn.execute_batch("INSERT INTO interactions_fts(interactions_fts) VALUES('optimize');");
        timings.fts_optimize_ms = t.elapsed().as_millis() as u64;

        let t = Instant::now();
        let _ = conn.execute_batch("PRAGMA optimize;");
        timings.pragma_optimize_ms = t.elapsed().as_millis() as u64;
    }
    drop_touched_sessions(conn);

    // Restore the normal checkpoint threshold and fold the run's WAL back into
    // the main database, so a big import doesn't leave a big WAL behind.
    let _ = conn.query_row("PRAGMA wal_autocheckpoint = 1000", [], |_| Ok(()));
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    clear_meta_flag(conn, META_PENDING_FINALIZE);
    rebuild?;

    timings.total_ms = started.elapsed().as_millis() as u64;
    log::info!(
        target: "import",
        "finalize: {purged} purged, purge {}ms, summary {}ms, fts optimize {}ms, pragma optimize {}ms, total {}ms",
        timings.purge_ms, timings.summary_ms, timings.fts_optimize_ms,
        timings.pragma_optimize_ms, timings.total_ms
    );
    Ok(FinalizeResult { purged, timings })
}

#[tauri::command]
async fn import_interactions_csv(
    db_state: State<'_, SharedDbState>,
    file_path: String,
    max_age_days: Option<i64>,
    delimiter: Option<String>,
    defer_finalize: Option<bool>,
) -> Result<ImportResult, String> {
    // Portal exports are pipe-delimited; the Analytics API client sniffs the
    // delimiter off the response header and passes it through.
    let delim = match delimiter.as_deref() {
        None | Some("") => b'|',
        Some(d) => {
            let bytes = d.as_bytes();
            if bytes.len() != 1 {
                return Err(format!("Invalid CSV delimiter: {d:?}"));
            }
            bytes[0]
        }
    };
    // Defaults to false, so any caller that hasn't opted into a run keeps the
    // original self-contained behaviour.
    let finalize = !defer_finalize.unwrap_or(false);
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut state = db.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_mut()
            .ok_or("No database open. Set a database path first.")?;
        import_csv_into(conn, &file_path, max_age_days, delim, finalize)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One entry of a row's `Contexts` JSON array, as the import loop reads it.
///
/// `Cow` rather than `&str` on purpose: borrowed `&str` deserialization fails
/// outright on any JSON string containing an escape (`\"`, `\uXXXX`), which real
/// context values do contain. Unknown fields are ignored, matching the previous
/// `serde_json::Value` path, which simply never looked at them.
#[derive(Deserialize)]
struct CsvContext<'a> {
    #[serde(borrow, default)]
    name: Option<std::borrow::Cow<'a, str>>,
    #[serde(borrow, default)]
    value: Option<std::borrow::Cow<'a, str>>,
}

/// The whole import, against an open connection.
///
/// Split out of the command so tests can drive a real CSV into a real database
/// without a Tauri `State`.
///
/// `finalize` false means this file is one of several in a run: the touched-session
/// set accumulates instead of resetting, and the tail (purge, summary rebuild, FTS
/// merge, planner stats) is left to [`finalize_import_run_into`]. The result is
/// identical either way — the tail only recomputes derived state.
fn import_csv_into(
    conn: &mut Connection,
    file_path: &str,
    max_age_days: Option<i64>,
    delim: u8,
    finalize: bool,
) -> Result<ImportResult, String> {
    let started = Instant::now();
    let mut timings = ImportTimings::default();
    let file = fs::File::open(file_path).map_err(|e| format!("Cannot open CSV: {e}"))?;
    let buf = std::io::BufReader::with_capacity(4 * 1024 * 1024, file);

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim)
        .quoting(true)
        .double_quote(true)
        .flexible(true)
        .from_reader(buf);

    // Build column index map from header
    let headers = rdr.headers().map_err(|e| format!("Header error: {e}"))?.clone();
    let col = |name: &str| -> Option<usize> {
        headers.iter().position(|h| h.eq_ignore_ascii_case(name))
    };

    let c_log_id          = col("LogId");
    let c_uuid            = col("InteractionUuid");
    let c_session         = col("SessionUuid");
    let c_ts_start        = col("TimestampStart");
    let c_ts_end          = col("TimestampEnd");
    let c_culture         = col("Culture");
    let c_main_type       = col("MainInteractionType");
    let c_all_types       = col("AllInteractionTypes");
    let c_value           = col("InteractionValue");
    let c_output          = col("OutputText");
    let c_article_ids     = col("ArticleIds");
    let c_dialog_paths    = col("DialogPaths");
    let c_tdialog_status  = col("TDialogStatus");
    let c_recog_type      = col("RecognitionType");
    let c_recog_quality   = col("RecognitionQuality");
    let c_recog_details   = col("RecognitionDetails");
    let c_genai           = col("GenerativeAISources");
    let c_articles        = col("Articles");
    let c_faqs            = col("FaqsFound");
    let c_contexts        = col("Contexts");
    let c_pages           = col("Pages");
    let c_link_click      = col("LinkclickInfo");
    let c_feedback        = col("FeedbackInfo");
    let c_output_meta     = col("OutputMetadata");

    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // Sessions this import (and any purge it triggers) touches, so the summary
    // rebuild at the end costs the size of the import instead of the size of
    // the whole database. Inside a run the set accumulates across files; a
    // standalone import owns it and starts clean.
    if finalize {
        reset_touched_sessions(conn)?;
    } else {
        ensure_touched_sessions(conn)?;
    }

    // Whether any pre-existing row still has an empty recognition_details, i.e.
    // whether the per-duplicate backfill in the loop below can do anything at
    // all. Checked once here rather than per row: on a mature database the
    // answer is almost always no, and every duplicate row was paying an indexed
    // UPDATE to rediscover that.
    //
    // Sound because the check reads pre-import state. A row that already
    // existed with an empty value makes this true; a row inserted by *this*
    // import carries its recognition_details in the INSERT and never needs
    // backfilling. So a false result can never skip a needed update.
    let needs_recog_backfill = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM interactions \
             WHERE recognition_details IS NULL OR recognition_details = '')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n != 0)
        .unwrap_or(true);

    let mut inserted: i64 = 0;
    let mut skipped: i64 = 0;
    let mut errors: Vec<String> = Vec::new();
    let mut batch: Vec<csv::StringRecord> = Vec::with_capacity(IMPORT_BATCH_ROWS);
    // Sessions already written to the touched table. A session spans many rows,
    // so without this the loop runs one b-tree insert per *row* to record a fact
    // it recorded on the session's first row.
    let mut touched_seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let flush_batch = |conn: &mut Connection, batch: &[csv::StringRecord], now_secs: i64,
        c_log_id: Option<usize>, c_uuid: Option<usize>, c_session: Option<usize>,
        c_ts_start: Option<usize>, c_ts_end: Option<usize>, c_culture: Option<usize>,
        c_main_type: Option<usize>, c_all_types: Option<usize>, c_value: Option<usize>,
        c_output: Option<usize>, c_article_ids: Option<usize>, c_dialog_paths: Option<usize>,
        c_tdialog_status: Option<usize>, c_recog_type: Option<usize>, c_recog_quality: Option<usize>,
        c_recog_details: Option<usize>,
        c_genai: Option<usize>, c_articles: Option<usize>, c_faqs: Option<usize>,
        c_contexts: Option<usize>, c_pages: Option<usize>, c_link_click: Option<usize>,
        c_feedback: Option<usize>, c_output_meta: Option<usize>,
        needs_recog_backfill: bool,
        touched_seen: &mut std::collections::HashSet<String>,
        inserted: &mut i64, skipped: &mut i64, errors: &mut Vec<String>|
    {
        let tx = match conn.transaction() {
            Ok(t) => t,
            Err(e) => { errors.push(format!("Transaction error: {e}")); return; }
        };
        // Prepare each statement once per batch (cached on the connection
        // across batches) instead of re-parsing the SQL for every row.
        let mut ins_stmt = match tx.prepare_cached(
            r#"INSERT OR IGNORE INTO interactions (
                log_id, interaction_uuid, session_uuid,
                timestamp_start, timestamp_end, culture,
                main_interaction_type, all_interaction_types,
                interaction_value, output_text,
                article_ids, dialog_paths, tdialog_status,
                recognition_type, recognition_quality,
                generative_ai_sources, articles, faqs_found,
                contexts, pages, link_click_info, feedback_info,
                output_metadata, recognition_details, imported_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)"#,
        ) {
            Ok(s) => s,
            Err(e) => { errors.push(format!("Prepare error: {e}")); return; }
        };
        let mut fts_stmt = match tx.prepare_cached(
            "INSERT INTO interactions_fts(rowid, interaction_value, output_text, article_ids, dialog_paths) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        ) {
            Ok(s) => s,
            Err(e) => { errors.push(format!("Prepare error: {e}")); return; }
        };
        let mut ctx_stmt = match tx.prepare_cached(
            "INSERT OR IGNORE INTO context_index(name, value, session_uuid) VALUES (?1, ?2, ?3)",
        ) {
            Ok(s) => s,
            Err(e) => { errors.push(format!("Prepare error: {e}")); return; }
        };
        let mut backfill_stmt = match tx.prepare_cached(
            "UPDATE interactions SET recognition_details = ?1 WHERE log_id = ?2 AND (recognition_details IS NULL OR recognition_details = '')",
        ) {
            Ok(s) => s,
            Err(e) => { errors.push(format!("Prepare error: {e}")); return; }
        };
        let touched_sql =
            format!("INSERT OR IGNORE INTO {TOUCHED_TABLE}(session_uuid) VALUES (?1)");
        let mut touched_stmt = match tx.prepare_cached(&touched_sql) {
            Ok(s) => s,
            Err(e) => { errors.push(format!("Prepare error: {e}")); return; }
        };
        // Reused across every row in the batch, so the two timestamp
        // normalizations per row cost no allocation after the first.
        let mut ts_start = String::with_capacity(32);
        let mut ts_end = String::with_capacity(32);
        for record in batch {
            let get_r = |idx: Option<usize>| -> &str {
                idx.and_then(|i| record.get(i)).unwrap_or("")
            };
            let log_id_str = get_r(c_log_id);
            let log_id: i64 = match log_id_str.parse() {
                Ok(v) => v,
                Err(_) => { *skipped += 1; continue; }
            };
            parse_ts_into(get_r(c_ts_start), &mut ts_start);
            parse_ts_into(get_r(c_ts_end), &mut ts_end);
            let quality: f64 = get_r(c_recog_quality).parse().unwrap_or(0.0);
            let result = ins_stmt.execute(
                params![
                    log_id,
                    get_r(c_uuid),
                    get_r(c_session),
                    ts_start.as_str(),
                    ts_end.as_str(),
                    get_r(c_culture),
                    get_r(c_main_type),
                    get_r(c_all_types),
                    get_r(c_value),
                    get_r(c_output),
                    get_r(c_article_ids),
                    get_r(c_dialog_paths),
                    get_r(c_tdialog_status),
                    get_r(c_recog_type),
                    quality,
                    get_r(c_genai),
                    get_r(c_articles),
                    get_r(c_faqs),
                    get_r(c_contexts),
                    get_r(c_pages),
                    get_r(c_link_click),
                    get_r(c_feedback),
                    get_r(c_output_meta),
                    get_r(c_recog_details),
                    now_secs,
                ],
            );
            match result {
                Ok(1) => {
                    // Also index in FTS5
                    let _ = fts_stmt.execute(params![
                        log_id,
                        get_r(c_value),
                        get_r(c_output),
                        get_r(c_article_ids),
                        get_r(c_dialog_paths),
                    ]);
                    // This session's summary is now stale — mark it for recompute.
                    // Duplicates are deliberately not marked: an ignored row
                    // changes nothing the summary is derived from.
                    let session_id = get_r(c_session);
                    if !session_id.is_empty() && !touched_seen.contains(session_id) {
                        touched_seen.insert(session_id.to_string());
                        let _ = touched_stmt.execute(params![session_id]);
                    }
                    // Index context (name, value) pairs for fast context-filter
                    // lookups. Deserializing into a typed Vec rather than a
                    // serde_json::Value skips building a Map<String, Value> per
                    // context item; anything that doesn't fit the shape fails to
                    // parse and is skipped, exactly as the Value path did.
                    let ctx_str = get_r(c_contexts);
                    if !ctx_str.is_empty() && ctx_str != "[]" && ctx_str != "null" {
                        if let Ok(items) = serde_json::from_str::<Vec<CsvContext>>(ctx_str) {
                            for item in &items {
                                let name = item.name.as_deref().unwrap_or("");
                                let value = item.value.as_deref().unwrap_or("");
                                if !name.is_empty() {
                                    let _ = ctx_stmt.execute(params![name, value, session_id]);
                                }
                            }
                        }
                    }
                    *inserted += 1;
                }
                Ok(_) => {
                    // Row already exists — backfill recognition_details if it was NULL.
                    // Skipped entirely when no row in the database has an empty
                    // one, which is the common case and used to cost an indexed
                    // UPDATE per duplicate row on every re-import.
                    if needs_recog_backfill {
                        let rd = get_r(c_recog_details);
                        if !rd.is_empty() {
                            let _ = backfill_stmt.execute(params![rd, log_id]);
                        }
                    }
                    *skipped += 1
                }
                Err(e) => errors.push(format!("Row {log_id}: {e}")),
            }
        }
        // Cached statements borrow the transaction; drop them before commit.
        drop(ins_stmt);
        drop(fts_stmt);
        drop(ctx_stmt);
        drop(backfill_stmt);
        drop(touched_stmt);
        let _ = tx.commit();
    };

    for result in rdr.records() {
        match result {
            Ok(record) => {
                batch.push(record);
                if batch.len() >= IMPORT_BATCH_ROWS {
                    flush_batch(conn, &batch, now_secs,
                        c_log_id, c_uuid, c_session, c_ts_start, c_ts_end, c_culture,
                        c_main_type, c_all_types, c_value, c_output, c_article_ids,
                        c_dialog_paths, c_tdialog_status, c_recog_type, c_recog_quality,
                        c_recog_details,
                        c_genai, c_articles, c_faqs, c_contexts, c_pages, c_link_click,
                        c_feedback, c_output_meta,
                        needs_recog_backfill, &mut touched_seen,
                        &mut inserted, &mut skipped, &mut errors);
                    batch.clear();
                }
            }
            Err(e) => {
                errors.push(format!("CSV parse error: {e}"));
            }
        }
    }
    // Flush remaining
    if !batch.is_empty() {
        flush_batch(conn, &batch, now_secs,
            c_log_id, c_uuid, c_session, c_ts_start, c_ts_end, c_culture,
            c_main_type, c_all_types, c_value, c_output, c_article_ids,
            c_dialog_paths, c_tdialog_status, c_recog_type, c_recog_quality,
            c_recog_details,
            c_genai, c_articles, c_faqs, c_contexts, c_pages, c_link_click,
            c_feedback, c_output_meta,
            needs_recog_backfill, &mut touched_seen,
                        &mut inserted, &mut skipped, &mut errors);
    }

    timings.rows_ms = started.elapsed().as_millis() as u64;

    // Deferred: the run's finalize step owns the tail. Everything it does is
    // derived state, so leaving it until the run ends changes nothing but when
    // the work happens.
    if !finalize {
        timings.total_ms = timings.rows_ms;
        log::info!(
            target: "import",
            "{file_path}: {inserted} new, {skipped} duplicate, rows {}ms (finalize deferred)",
            timings.rows_ms
        );
        return Ok(ImportResult { inserted, skipped, purged: 0, errors, timings });
    }

    // purge_old adds the sessions it stripped to the same touched-session table
    // and cleans up their contexts, so a single scoped rebuild below covers the
    // import and the purge together. No orphan sweep is needed for the import
    // itself: adding rows can never orphan a context row.
    let t = Instant::now();
    let purged = purge_old(conn, max_age_days.unwrap_or(90).max(1) as u64);
    timings.purge_ms = t.elapsed().as_millis() as u64;

    let mut rebuild = Ok(());
    if inserted > 0 || purged > 0 {
        let t = Instant::now();
        rebuild = rebuild_session_summary_touched(conn);
        timings.summary_ms = t.elapsed().as_millis() as u64;

        // Merge FTS5 b-tree segments so MATCH queries read fewer pages, and
        // refresh planner statistics for the tables this import touched.
        let t = Instant::now();
        let _ = conn.execute_batch(
            "INSERT INTO interactions_fts(interactions_fts) VALUES('optimize');",
        );
        timings.fts_optimize_ms = t.elapsed().as_millis() as u64;

        let t = Instant::now();
        let _ = conn.execute_batch("PRAGMA optimize;");
        timings.pragma_optimize_ms = t.elapsed().as_millis() as u64;
    }
    drop_touched_sessions(conn);
    rebuild?;

    timings.total_ms = started.elapsed().as_millis() as u64;
    log::info!(
        target: "import",
        "{file_path}: {inserted} new, {skipped} duplicate, {purged} purged — rows {}ms, purge {}ms, summary {}ms, fts optimize {}ms, pragma optimize {}ms, total {}ms",
        timings.rows_ms, timings.purge_ms, timings.summary_ms,
        timings.fts_optimize_ms, timings.pragma_optimize_ms, timings.total_ms
    );
    Ok(ImportResult { inserted, skipped, purged, errors, timings })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DateRange {
    min: String,
    max: String,
}

#[tauri::command]
async fn get_date_range(db_state: State<'_, SharedDbState>) -> Result<DateRange, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = db.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("No database open.")?;
        let result: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT MIN(DATE(timestamp_start)), MAX(DATE(timestamp_start)) FROM interactions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        Ok(DateRange {
            min: result.0.unwrap_or_default(),
            max: result.1.unwrap_or_default(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetSessionsArgs {
    page: Option<i64>,
    date_from: Option<String>,
    date_to: Option<String>,
    filter: Option<String>, // "all" | "genai" | "neg_feedback" | "low_recog" | "zero_recog"
    query: Option<String>,
    query_regex: Option<bool>,                   // treat query as a regex
    query_scope: Option<String>,                 // "both" | "user" | "bot"
    query_ids: Option<bool>,                     // also search article_ids and dialog_paths columns
    query_ids_only: Option<bool>, // search ONLY article_ids and dialog_paths, not message text
    query_id_type: Option<String>, // "article" | "dialog" | "node" — which ID column/pattern to use
    low_recog_threshold: Option<i64>, // threshold for "low recognition" filter (default 60, range 1–99)
    context_filters: Option<Vec<ContextFilter>>, // [{name, value}] filter by context values
}

fn build_session_filter_query(
    conn: &Connection,
    args: &GetSessionsArgs,
) -> Result<SessionFilterQuery, String> {
    let filter = args.filter.as_deref().unwrap_or("all");
    let query = args.query.as_deref().unwrap_or("").trim().to_string();
    let query_regex = args.query_regex.unwrap_or(false);
    let query_scope = args.query_scope.as_deref().unwrap_or("both").to_string();
    let query_ids = args.query_ids.unwrap_or(false);
    let query_ids_only = args.query_ids_only.unwrap_or(false);
    let query_id_type = args
        .query_id_type
        .as_deref()
        .unwrap_or("article")
        .to_string();
    let low_recog_threshold = args.low_recog_threshold.unwrap_or(60).clamp(1, 99);

    fn tokenize_segment(s: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut chars = s.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else if c == '"' {
                chars.next();
                let phrase: String = chars.by_ref().take_while(|&ch| ch != '"').collect();
                if !phrase.is_empty() {
                    tokens.push(phrase);
                }
            } else {
                let word: String = chars
                    .by_ref()
                    .take_while(|&ch| !ch.is_whitespace() && ch != '"')
                    .collect();
                if !word.is_empty() {
                    tokens.push(word);
                }
            }
        }
        tokens
    }

    let mut param_values: Vec<Box<dyn ToSql>> = Vec::new();
    let mut param_idx = 0usize;
    let next_param = |idx: &mut usize| -> String {
        *idx += 1;
        format!("?{}", *idx)
    };
    let is_feedback_filter = matches!(filter, "neg_feedback" | "pos_feedback");
    if is_feedback_filter {
        conn.create_scalar_function(
            "feedback_origin",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx: &rusqlite::functions::Context<'_>| {
                // Borrow the column text directly — no per-row String allocation
                let text = ctx.get_raw(0).as_str().unwrap_or("");
                let origin = serde_json::from_str::<serde_json::Value>(text)
                    .ok()
                    .and_then(|v| {
                        v.get("originatingInteractionId")
                            .and_then(|id| id.as_str())
                            .map(|id| id.to_string())
                    })
                    .unwrap_or_default();
                Ok(origin)
            },
        )
        .ok();
    }

    let mut base_conditions = vec!["s.has_real_user_input = 1".to_string()];
    match filter {
        "genai" => base_conditions.push("s.has_gen_ai = 1".to_string()),
        "neg_feedback" => base_conditions.push("s.has_neg_feedback = 1".to_string()),
        "pos_feedback" => base_conditions.push("s.has_pos_feedback = 1".to_string()),
        "low_recog" => {
            base_conditions.push(format!(
                "s.min_positive_recognition_quality > 0 AND s.min_positive_recognition_quality < {low_recog_threshold}"
            ));
        }
        "zero_recog" => base_conditions.push("s.has_zero_recog = 1".to_string()),
        _ => {}
    }

    if let Some(ref df) = args.date_from {
        if !df.is_empty() {
            let p = next_param(&mut param_idx);
            base_conditions.push(format!("s.last_ts >= {p}"));
            param_values.push(Box::new(df.clone()));
        }
    }
    if let Some(ref dt) = args.date_to {
        if !dt.is_empty() {
            let p = next_param(&mut param_idx);
            base_conditions.push(format!("s.first_ts <= {p}"));
            param_values.push(Box::new(dt.clone()));
        }
    }

    if let Some(ref ctx_filters) = args.context_filters {
        if !ctx_filters.is_empty() {
            let mut groups: HashMap<String, Vec<String>> = HashMap::new();
            for f in ctx_filters {
                groups
                    .entry(f.name.clone())
                    .or_default()
                    .push(f.value.clone());
            }
            for (name, values) in groups {
                let has_not_set = values.iter().any(|v| v == "__not_set__");
                let regular_values: Vec<String> =
                    values.into_iter().filter(|v| v != "__not_set__").collect();
                let mut subclauses = Vec::new();
                if has_not_set {
                    let pn = next_param(&mut param_idx);
                    param_values.push(Box::new(name.clone()));
                    subclauses.push(format!(
                        "NOT EXISTS (SELECT 1 FROM context_index ci WHERE ci.session_uuid = s.session_uuid AND ci.name = {pn})"
                    ));
                }
                if !regular_values.is_empty() {
                    let pn = next_param(&mut param_idx);
                    param_values.push(Box::new(name.clone()));
                    let value_placeholders = regular_values
                        .iter()
                        .map(|v| {
                            let pv = next_param(&mut param_idx);
                            param_values.push(Box::new(v.clone()));
                            pv
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    subclauses.push(format!(
                        "EXISTS (SELECT 1 FROM context_index ci WHERE ci.session_uuid = s.session_uuid AND ci.name = {pn} AND ci.value IN ({value_placeholders}))"
                    ));
                }
                if subclauses.len() == 1 {
                    base_conditions.push(subclauses.remove(0));
                } else if !subclauses.is_empty() {
                    base_conditions.push(format!("({})", subclauses.join(" OR ")));
                }
            }
        }
    }

    let base_where = format!("WHERE {}", base_conditions.join(" AND "));
    let mut search_mode = "none".to_string();
    let mut search_cte = String::new();
    let mut filtered_from = "SELECT b.*, NULL AS match_log_id FROM base_sessions b".to_string();
    let is_recognition_filter = matches!(filter, "low_recog" | "zero_recog");
    let search_row_filter = match filter {
        "genai" => {
            " AND (i.main_interaction_type = 'GenerativeAI' OR i.all_interaction_types LIKE '%GenerativeAI%')".to_string()
        }
        "low_recog" => format!(
            " AND i.recognition_quality > 0 \
              AND i.recognition_quality < {low_recog_threshold} \
              AND COALESCE(i.recognition_type, '') != 'GenerativeAI' \
              AND COALESCE(i.main_interaction_type, '') != 'GenerativeAI'"
        ),
        "zero_recog" => {
            " AND i.recognition_quality = 0 \
              AND COALESCE(i.recognition_type, '') != '' \
              AND COALESCE(i.recognition_type, '') != 'GenerativeAI' \
              AND COALESCE(i.main_interaction_type, '') != 'GenerativeAI'".to_string()
        }
        _ => String::new(),
    };
    let search_row_filter = search_row_filter.as_str();
    let feedback_score_filter = match filter {
        "neg_feedback" => {
            "AND (fb.feedback_info LIKE '%\"score\": -1%' OR fb.feedback_info LIKE '%\"score\":-1%')"
        }
        "pos_feedback" => {
            "AND (fb.feedback_info LIKE '%\"score\": 1%' OR fb.feedback_info LIKE '%\"score\":1%') \
             AND fb.feedback_info NOT LIKE '%\"score\": -1%' \
             AND fb.feedback_info NOT LIKE '%\"score\":-1%'"
        }
        _ => "",
    };
    let feedback_origins_cte = if is_feedback_filter {
        format!(
            ", feedback_origins AS (\
                SELECT \
                    fb.session_uuid, \
                    COALESCE(origin.log_id, (\
                        SELECT prev.log_id \
                        FROM interactions prev \
                        WHERE prev.session_uuid = fb.session_uuid \
                          AND prev.log_id < fb.log_id \
                          AND COALESCE(prev.output_text, '') != '' \
                          AND COALESCE(prev.main_interaction_type, '') != 'Feedback' \
                        ORDER BY prev.log_id DESC \
                        LIMIT 1\
                    ), fb.log_id) AS match_log_id \
                FROM interactions fb \
                JOIN base_sessions b ON b.session_uuid = fb.session_uuid \
                LEFT JOIN interactions origin \
                  ON origin.session_uuid = fb.session_uuid \
                 AND origin.interaction_uuid = feedback_origin(fb.feedback_info) \
                WHERE COALESCE(fb.feedback_info, '') != '' {feedback_score_filter}\
            )"
        )
    } else {
        String::new()
    };
    let recognition_matches_cte = if is_recognition_filter {
        format!(
            ", recognition_matches AS (\
                SELECT i.session_uuid, i.log_id AS match_log_id \
                FROM interactions i \
                JOIN base_sessions b ON b.session_uuid = i.session_uuid \
                WHERE 1 = 1{search_row_filter}\
            )"
        )
    } else {
        String::new()
    };

    if !query.is_empty() {
        if query_ids_only {
            search_mode = "id".to_string();
            let (column, like_val) = match query_id_type.as_str() {
                "dialog" => (
                    "i.dialog_paths",
                    format!("%\"{}:%", query.replace('%', "\\%").replace('_', "\\_")),
                ),
                "node" => (
                    "i.article_ids",
                    format!("%dn-{}%", query.replace('%', "\\%").replace('_', "\\_")),
                ),
                _ => (
                    "i.article_ids",
                    format!("%qa-{}%", query.replace('%', "\\%").replace('_', "\\_")),
                ),
            };
            let p = next_param(&mut param_idx);
            param_values.push(Box::new(like_val));
            let search_from = if is_feedback_filter {
                "feedback_origins fo JOIN interactions i ON i.log_id = fo.match_log_id"
            } else {
                "interactions i JOIN base_sessions b ON b.session_uuid = i.session_uuid"
            };
            let row_filter = if is_feedback_filter {
                ""
            } else {
                search_row_filter
            };
            search_cte = format!(
                "{feedback_origins_cte}, search_matches AS (\
                    SELECT i.session_uuid, i.log_id AS match_log_id \
                    FROM {search_from} \
                    WHERE {column} LIKE {p} ESCAPE '\\'{row_filter}\
                ), search_sessions AS (\
                    SELECT session_uuid, MIN(match_log_id) AS match_log_id \
                    FROM search_matches \
                    GROUP BY session_uuid\
                )"
            );
            filtered_from =
                "SELECT b.*, ss.match_log_id FROM base_sessions b JOIN search_sessions ss ON ss.session_uuid = b.session_uuid".to_string();
        } else if query_regex {
            search_mode = "regex".to_string();
            use regex::Regex;
            let compiled = Arc::new(Regex::new(&query).map_err(|e| format!("Invalid regex: {e}"))?);
            conn.create_scalar_function(
                "regexp",
                2,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8
                    | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
                move |ctx: &rusqlite::functions::Context<'_>| {
                    // Borrow the column text directly — no per-row String allocation
                    let text = ctx.get_raw(1).as_str().unwrap_or("");
                    Ok(compiled.is_match(text) as i32)
                },
            )
            .ok();

            let p = next_param(&mut param_idx);
            param_values.push(Box::new(query.clone()));
            let text_cond = match query_scope.as_str() {
                "user" => format!("regexp({p}, i.interaction_value)"),
                "bot" => format!("regexp({p}, i.output_text)"),
                _ => format!("(regexp({p}, i.interaction_value) OR regexp({p}, i.output_text))"),
            };
            let final_cond = if query_ids {
                let p2 = next_param(&mut param_idx);
                param_values.push(Box::new(query.clone()));
                format!(
                    "({text_cond} OR regexp({p2}, i.article_ids) OR regexp({p2}, i.dialog_paths))"
                )
            } else {
                text_cond
            };
            let search_from = if is_feedback_filter {
                "feedback_origins fo JOIN interactions i ON i.log_id = fo.match_log_id"
            } else {
                "interactions i JOIN base_sessions b ON b.session_uuid = i.session_uuid"
            };
            let row_filter = if is_feedback_filter {
                ""
            } else {
                search_row_filter
            };
            search_cte = format!(
                "{feedback_origins_cte}, search_matches AS (\
                    SELECT i.session_uuid, i.log_id AS match_log_id \
                    FROM {search_from} \
                    WHERE {final_cond}{row_filter}\
                ), search_sessions AS (\
                    SELECT session_uuid, MIN(match_log_id) AS match_log_id \
                    FROM search_matches \
                    GROUP BY session_uuid\
                )"
            );
            filtered_from =
                "SELECT b.*, ss.match_log_id FROM base_sessions b JOIN search_sessions ss ON ss.session_uuid = b.session_uuid".to_string();
        } else {
            let fts_available = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='interactions_fts'",
                    [],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            let or_groups: Vec<Vec<String>> = query
                .split('|')
                .map(|g| tokenize_segment(g.trim()))
                .filter(|g| !g.is_empty())
                .collect();

            if fts_available {
                let fts_groups = or_groups
                    .iter()
                    .map(|group| {
                        group
                            .iter()
                            .filter_map(|t| {
                                if t.contains(' ') {
                                    let phrase_terms = t
                                        .split_whitespace()
                                        .map(|w| {
                                            w.chars()
                                                .filter(|c| {
                                                    c.is_alphanumeric()
                                                        || matches!(*c, '-' | '_' | '.')
                                                })
                                                .collect::<String>()
                                        })
                                        .filter(|w| !w.is_empty())
                                        .collect::<Vec<_>>();
                                    if phrase_terms.is_empty() {
                                        None
                                    } else {
                                        Some(format!("\"{}\"", phrase_terms.join(" ")))
                                    }
                                } else {
                                    let clean = t
                                        .chars()
                                        .filter(|c| {
                                            c.is_alphanumeric() || matches!(*c, '-' | '_' | '.')
                                        })
                                        .collect::<String>();
                                    if clean.is_empty() {
                                        None
                                    } else {
                                        Some(format!("{clean}*"))
                                    }
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .filter(|g| !g.is_empty())
                    .collect::<Vec<_>>();

                if !fts_groups.is_empty() {
                    search_mode = "fts".to_string();
                    let fts_query = fts_groups
                        .iter()
                        .map(|terms| {
                            if terms.len() == 1 {
                                terms[0].clone()
                            } else {
                                format!("({})", terms.join(" "))
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" OR ");
                    let fts_match_expr = match (query_scope.as_str(), query_ids) {
                        ("user", false) => format!("interaction_value : {fts_query}"),
                        ("user", true) => {
                            format!("{{interaction_value article_ids dialog_paths}} : {fts_query}")
                        }
                        ("bot", false) => format!("output_text : {fts_query}"),
                        ("bot", true) => {
                            format!("{{output_text article_ids dialog_paths}} : {fts_query}")
                        }
                        (_, _) => fts_query,
                    };
                    let p = next_param(&mut param_idx);
                    param_values.push(Box::new(fts_match_expr));
                    let search_from = if is_feedback_filter {
                        "feedback_origins fo JOIN interactions i ON i.log_id = fo.match_log_id JOIN interactions_fts ON interactions_fts.rowid = i.log_id"
                    } else {
                        "interactions_fts JOIN interactions i ON i.log_id = interactions_fts.rowid JOIN base_sessions b ON b.session_uuid = i.session_uuid"
                    };
                    let row_filter = if is_feedback_filter {
                        ""
                    } else {
                        search_row_filter
                    };
                    search_cte = format!(
                        "{feedback_origins_cte}, search_matches AS (\
                            SELECT i.session_uuid, i.log_id AS match_log_id \
                            FROM {search_from} \
                            WHERE interactions_fts MATCH {p}{row_filter}\
                        ), search_sessions AS (\
                            SELECT session_uuid, MIN(match_log_id) AS match_log_id \
                            FROM search_matches \
                            GROUP BY session_uuid\
                        )"
                    );
                    filtered_from =
                        "SELECT b.*, ss.match_log_id FROM base_sessions b JOIN search_sessions ss ON ss.session_uuid = b.session_uuid".to_string();
                }
            } else if !or_groups.is_empty() {
                search_mode = "like".to_string();
                let or_clauses = or_groups
                    .iter()
                    .map(|and_terms| {
                        and_terms
                            .iter()
                            .map(|term| {
                                let like_val =
                                    format!("%{}%", term.replace('%', "\\%").replace('_', "\\_"));
                                let text_cond = match query_scope.as_str() {
                                    "user" => {
                                        let p = next_param(&mut param_idx);
                                        param_values.push(Box::new(like_val.clone()));
                                        format!("i.interaction_value LIKE {p} ESCAPE '\\'")
                                    }
                                    "bot" => {
                                        let p = next_param(&mut param_idx);
                                        param_values.push(Box::new(like_val.clone()));
                                        format!("i.output_text LIKE {p} ESCAPE '\\'")
                                    }
                                    _ => {
                                        let p1 = next_param(&mut param_idx);
                                        param_values.push(Box::new(like_val.clone()));
                                        let p2 = next_param(&mut param_idx);
                                        param_values.push(Box::new(like_val.clone()));
                                        format!("(i.interaction_value LIKE {p1} ESCAPE '\\' OR i.output_text LIKE {p2} ESCAPE '\\')")
                                    }
                                };
                                if query_ids {
                                    let pi1 = next_param(&mut param_idx);
                                    param_values.push(Box::new(like_val.clone()));
                                    let pi2 = next_param(&mut param_idx);
                                    param_values.push(Box::new(like_val));
                                    format!("({text_cond} OR i.article_ids LIKE {pi1} ESCAPE '\\' OR i.dialog_paths LIKE {pi2} ESCAPE '\\')")
                                } else {
                                    text_cond
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" AND ")
                    })
                    .map(|g| format!("({g})"))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                let search_from = if is_feedback_filter {
                    "feedback_origins fo JOIN interactions i ON i.log_id = fo.match_log_id"
                } else {
                    "interactions i JOIN base_sessions b ON b.session_uuid = i.session_uuid"
                };
                let row_filter = if is_feedback_filter {
                    ""
                } else {
                    search_row_filter
                };
                search_cte = format!(
                    "{feedback_origins_cte}, search_matches AS (\
                        SELECT i.session_uuid, i.log_id AS match_log_id \
                        FROM {search_from} \
                        WHERE {or_clauses}{row_filter}\
                    ), search_sessions AS (\
                        SELECT session_uuid, MIN(match_log_id) AS match_log_id \
                        FROM search_matches \
                        GROUP BY session_uuid\
                    )"
                );
                filtered_from =
                    "SELECT b.*, ss.match_log_id FROM base_sessions b JOIN search_sessions ss ON ss.session_uuid = b.session_uuid".to_string();
            }
        }
    } else if is_feedback_filter {
        search_cte = format!(
            "{feedback_origins_cte}, feedback_sessions AS (\
                SELECT session_uuid, MIN(match_log_id) AS match_log_id \
                FROM feedback_origins \
                GROUP BY session_uuid\
            )"
        );
        filtered_from =
            "SELECT b.*, fs.match_log_id FROM base_sessions b JOIN feedback_sessions fs ON fs.session_uuid = b.session_uuid".to_string();
    } else if is_recognition_filter {
        search_cte = format!(
            "{recognition_matches_cte}, recognition_sessions AS (\
                SELECT session_uuid, MIN(match_log_id) AS match_log_id \
                FROM recognition_matches \
                GROUP BY session_uuid\
            )"
        );
        filtered_from =
            "SELECT b.*, rs.match_log_id FROM base_sessions b JOIN recognition_sessions rs ON rs.session_uuid = b.session_uuid".to_string();
    }

    Ok(SessionFilterQuery {
        base_where,
        search_cte,
        filtered_from,
        param_values,
        search_mode,
    })
}

#[tauri::command]
async fn get_sessions(
    db_state: State<'_, SharedDbState>,
    args: GetSessionsArgs,
) -> Result<SessionsPage, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let state = db.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("No database open.")?;

        let page = args.page.unwrap_or(1).max(1);
        let limit = 50i64;
        let offset = (page - 1) * limit;

        let mut filter_query = build_session_filter_query(conn, &args)?;

        filter_query.param_values.push(Box::new(limit));
        filter_query.param_values.push(Box::new(offset));
        let p_limit = format!("?{}", filter_query.param_values.len() - 1);
        let p_offset = format!("?{}", filter_query.param_values.len());
        let params_ref: Vec<&dyn ToSql> = filter_query
            .param_values
            .iter()
            .map(|b| b.as_ref())
            .collect();

        let sql = format!(
            r#"WITH
base_sessions AS (
    SELECT s.*
    FROM session_summary s
	    {base_where}
	)
	{search_cte},
filtered_sessions AS (
    {filtered_from}
),
total AS (
    SELECT COUNT(*) AS total_count FROM filtered_sessions
),
page_rows AS (
    SELECT *
    FROM filtered_sessions
    ORDER BY first_ts DESC
    LIMIT {p_limit} OFFSET {p_offset}
)
SELECT
    p.session_uuid,
    p.first_ts,
    p.last_ts,
    p.interaction_count,
    p.has_gen_ai,
    p.culture,
    COALESCE(NULLIF((
        SELECT i_match.interaction_value
        FROM interactions i_match
        WHERE i_match.session_uuid = p.session_uuid
          AND i_match.log_id <= p.match_log_id
          AND i_match.interaction_value != ''
          AND i_match.interaction_value NOT LIKE '#%#'
          AND LOWER(i_match.interaction_value) != 'continue'
          AND COALESCE(i_match.main_interaction_type, '') NOT IN ('Event', 'LinkClick')
        ORDER BY i_match.log_id DESC
        LIMIT 1
    ), ''), p.first_user_message) AS user_message_preview,
    p.has_neg_feedback,
    p.has_pos_feedback,
    p.contexts_snapshot,
    t.total_count
FROM total t
LEFT JOIN page_rows p ON 1 = 1
ORDER BY p.first_ts DESC"#,
            base_where = filter_query.base_where.as_str(),
            search_cte = filter_query.search_cte.as_str(),
            filtered_from = filter_query.filtered_from.as_str(),
            p_limit = p_limit,
            p_offset = p_offset
        );

        // Cached: pagination and repeated searches with the same filter shape
        // reuse the already-compiled statement.
        let mut stmt = conn
            .prepare_cached(&sql)
            .map_err(|e| format!("Query error: {e}"))?;
        let mut rows = stmt
            .query(params_ref.as_slice())
            .map_err(|e| format!("Query error: {e}"))?;
        let mut sessions = Vec::new();
        let mut total = 0i64;
        while let Some(row) = rows.next().map_err(|e| format!("Query error: {e}"))? {
            total = row.get::<_, i64>(10).unwrap_or(0);
            let session_uuid = row
                .get::<_, Option<String>>(0)
                .unwrap_or(None)
                .unwrap_or_default();
            if session_uuid.is_empty() {
                continue;
            }
            sessions.push(SessionSummary {
                session_uuid,
                first_ts: row
                    .get::<_, Option<String>>(1)
                    .unwrap_or(None)
                    .unwrap_or_default(),
                last_ts: row
                    .get::<_, Option<String>>(2)
                    .unwrap_or(None)
                    .unwrap_or_default(),
                interaction_count: row.get::<_, Option<i64>>(3).unwrap_or(None).unwrap_or(0),
                has_gen_ai: row.get::<_, Option<i64>>(4).unwrap_or(None).unwrap_or(0) == 1,
                culture: row
                    .get::<_, Option<String>>(5)
                    .unwrap_or(None)
                    .unwrap_or_default(),
                user_message_preview: row
                    .get::<_, Option<String>>(6)
                    .unwrap_or(None)
                    .unwrap_or_default(),
                has_neg_feedback: row.get::<_, Option<i64>>(7).unwrap_or(None).unwrap_or(0) == 1,
                has_pos_feedback: row.get::<_, Option<i64>>(8).unwrap_or(None).unwrap_or(0) == 1,
                contexts: row
                    .get::<_, Option<String>>(9)
                    .unwrap_or(None)
                    .unwrap_or_default(),
            });
        }

        Ok(SessionsPage {
            sessions,
            total,
            page,
            timing_ms: started.elapsed().as_millis() as i64,
            search_mode: filter_query.search_mode.clone(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn json_or_text(text: &str) -> serde_json::Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str::<serde_json::Value>(trimmed)
            .unwrap_or_else(|_| serde_json::Value::String(text.to_string()))
    }
}

fn feedback_origin_id(feedback_info: &str) -> String {
    serde_json::from_str::<serde_json::Value>(feedback_info)
        .ok()
        .and_then(|v| {
            v.get("originatingInteractionId")
                .and_then(|id| id.as_str())
                .map(|id| id.to_string())
        })
        .unwrap_or_default()
}

fn feedback_score(feedback_info: &str) -> Option<i64> {
    let value = serde_json::from_str::<serde_json::Value>(feedback_info).ok()?;
    value
        .get("score")
        .and_then(|score| score.as_i64().or_else(|| score.as_str()?.parse().ok()))
}

fn is_empty_json(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.trim().is_empty(),
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn prune_empty_json(value: serde_json::Value) -> Option<serde_json::Value> {
    match value {
        serde_json::Value::Array(items) => {
            let pruned = items
                .into_iter()
                .filter_map(prune_empty_json)
                .collect::<Vec<_>>();
            if pruned.is_empty() {
                None
            } else {
                Some(serde_json::Value::Array(pruned))
            }
        }
        serde_json::Value::Object(map) => {
            let pruned = map
                .into_iter()
                .filter_map(|(key, value)| prune_empty_json(value).map(|value| (key, value)))
                .collect::<serde_json::Map<_, _>>();
            if pruned.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(pruned))
            }
        }
        other if is_empty_json(&other) => None,
        other => Some(other),
    }
}

fn insert_if_useful(
    map: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) {
    if !is_empty_json(&value) {
        map.insert(key.to_string(), value);
    }
}

fn strip_html_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            '&' if !in_tag => {
                let mut entity = String::new();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next == ';' || entity.len() > 12 {
                        break;
                    }
                    entity.push(next);
                }
                match entity.as_str() {
                    "nbsp" => out.push(' '),
                    "amp" => out.push('&'),
                    "lt" => out.push('<'),
                    "gt" => out.push('>'),
                    "quot" => out.push('"'),
                    _ => {
                        out.push('&');
                        out.push_str(&entity);
                        out.push(';');
                    }
                }
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    // `__` is a template-separator artifact in CAI answer text. A single `_` is
    // left alone — it is usually part of a real word or identifier.
    out.replace("__", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The DB stores naive UTC ("2026-03-25T09:30:22"). Exports mark it explicitly so
/// a reader — a person or a model — cannot mistake it for local time.
fn utc_iso(ts: &str) -> serde_json::Value {
    let ts = ts.trim();
    if ts.is_empty() {
        serde_json::Value::Null
    } else if ts.ends_with('Z') {
        serde_json::Value::String(ts.to_string())
    } else {
        serde_json::Value::String(format!("{ts}Z"))
    }
}

fn compact_entity_matches(recognition_details: &serde_json::Value) -> serde_json::Value {
    let Some(matches) = recognition_details
        .get("entityMatches")
        .and_then(|value| value.as_array())
    else {
        return serde_json::Value::Array(Vec::new());
    };
    serde_json::Value::Array(
        matches
            .iter()
            .filter_map(|item| {
                let mut map = serde_json::Map::new();
                insert_if_useful(
                    &mut map,
                    "entity_id",
                    item.get("entityId")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                );
                insert_if_useful(
                    &mut map,
                    "display_name",
                    item.get("displayName")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                );
                insert_if_useful(
                    &mut map,
                    "name",
                    item.get("name").cloned().unwrap_or(serde_json::Value::Null),
                );
                insert_if_useful(
                    &mut map,
                    "matched_text",
                    item.get("match")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                );
                if map.is_empty() {
                    None
                } else {
                    Some(serde_json::Value::Object(map))
                }
            })
            .collect(),
    )
}

fn compact_triggered_content(
    article_ids: &serde_json::Value,
    dialog_paths: &serde_json::Value,
    articles: &serde_json::Value,
) -> serde_json::Value {
    let mut article_list = Vec::new();
    let mut dialog_list = Vec::new();
    let mut event_list = Vec::new();

    if let Some(ids) = article_ids.as_array() {
        for id_value in ids {
            let Some(id) = id_value.as_str() else {
                continue;
            };
            if let Some(rest) = id.strip_prefix("qa-") {
                if !article_list.iter().any(|item: &serde_json::Value| {
                    item.get("id").and_then(|v| v.as_str()) == Some(rest)
                }) {
                    article_list.push(serde_json::json!({ "id": rest }));
                }
            } else if let Some(rest) = id.strip_prefix("dn-") {
                let mut parts = rest.split('-');
                let dialog_id = parts.next().unwrap_or("");
                let node_id = parts.next().unwrap_or("");
                let mut map = serde_json::Map::new();
                insert_if_useful(
                    &mut map,
                    "dialog_id",
                    serde_json::Value::String(dialog_id.to_string()),
                );
                insert_if_useful(
                    &mut map,
                    "node_id",
                    serde_json::Value::String(node_id.to_string()),
                );
                if !map.is_empty() {
                    dialog_list.push(serde_json::Value::Object(map));
                }
            } else if let Some(rest) = id.strip_prefix("e-") {
                event_list.push(serde_json::json!({ "id": rest }));
            }
        }
    }

    if let Some(dialogs) = articles.get("dialog").and_then(|value| value.as_array()) {
        for dialog in dialogs {
            let mut map = serde_json::Map::new();
            insert_if_useful(
                &mut map,
                "dialog_id",
                dialog
                    .get("dialogId")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            insert_if_useful(
                &mut map,
                "dialog_name",
                dialog
                    .get("dialogName")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            insert_if_useful(
                &mut map,
                "node_id",
                dialog
                    .get("nodeId")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            insert_if_useful(
                &mut map,
                "node_name",
                dialog
                    .get("nodeName")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            insert_if_useful(
                &mut map,
                "status",
                dialog
                    .get("dialogStatus")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            insert_if_useful(
                &mut map,
                "node_type",
                dialog
                    .get("nodeType")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            if !map.is_empty() {
                dialog_list.push(serde_json::Value::Object(map));
            }
        }
    }

    if let Some(qas) = articles.get("qa").and_then(|value| value.as_array()) {
        for qa in qas {
            let mut map = serde_json::Map::new();
            insert_if_useful(
                &mut map,
                "id",
                qa.get("articleId")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            if !map.is_empty() {
                article_list.push(serde_json::Value::Object(map));
            }
        }
    }

    if let Some(events) = articles.get("event").and_then(|value| value.as_array()) {
        for event in events {
            let mut map = serde_json::Map::new();
            insert_if_useful(
                &mut map,
                "id",
                event
                    .get("eventId")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            insert_if_useful(
                &mut map,
                "name",
                event
                    .get("eventName")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            if !map.is_empty() {
                event_list.push(serde_json::Value::Object(map));
            }
        }
    }

    let mut map = serde_json::Map::new();
    insert_if_useful(&mut map, "articles", serde_json::Value::Array(article_list));
    insert_if_useful(&mut map, "dialogs", serde_json::Value::Array(dialog_list));
    insert_if_useful(&mut map, "events", serde_json::Value::Array(event_list));
    insert_if_useful(&mut map, "dialog_paths", dialog_paths.clone());
    serde_json::Value::Object(map)
}

fn compact_turn(row: &serde_json::Value, is_feedback_target: bool) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let interaction_type = row
        .get("interactionType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let answer_text = row
        .get("botOutput")
        .and_then(|v| v.as_str())
        .map(strip_html_text)
        .unwrap_or_default();
    let triggered_content = compact_triggered_content(
        row.get("articleIds").unwrap_or(&serde_json::Value::Null),
        row.get("dialogPaths").unwrap_or(&serde_json::Value::Null),
        row.get("articles").unwrap_or(&serde_json::Value::Null),
    );
    let entity_matches = compact_entity_matches(
        row.get("recognitionDetails")
            .unwrap_or(&serde_json::Value::Null),
    );

    insert_if_useful(
        &mut map,
        "log_id",
        row.get("logId").cloned().unwrap_or(serde_json::Value::Null),
    );
    insert_if_useful(
        &mut map,
        "turn_kind",
        row.get("turnKind")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    insert_if_useful(
        &mut map,
        "type",
        serde_json::Value::String(interaction_type.to_string()),
    );
    insert_if_useful(
        &mut map,
        "user_text",
        row.get("userText")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    insert_if_useful(
        &mut map,
        "answer_text",
        serde_json::Value::String(answer_text),
    );
    insert_if_useful(
        &mut map,
        "recognition_type",
        row.get("recognitionType")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    insert_if_useful(
        &mut map,
        "recognition_quality",
        row.get("recognitionQuality")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    insert_if_useful(&mut map, "triggered_content", triggered_content);
    insert_if_useful(&mut map, "entity_matches", entity_matches);
    // Only ever emitted as `true`. Writing `false` on every other turn cost a lot
    // of repeated tokens to say nothing, and `feedback_targets` already lists them.
    if is_feedback_target {
        map.insert(
            "is_feedback_target".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    serde_json::Value::Object(map)
}

fn build_feedback_targets(rows: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut targets = Vec::new();
    for (idx, row) in rows.iter().enumerate() {
        let feedback_info = row
            .get("feedbackInfoRaw")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if feedback_info.trim().is_empty() {
            continue;
        }
        let Some(score) = feedback_score(feedback_info) else {
            continue;
        };
        if score != -1 && score != 1 {
            continue;
        }

        let origin_uuid = feedback_origin_id(feedback_info);
        let origin_idx = if origin_uuid.is_empty() {
            None
        } else {
            rows.iter().position(|candidate| {
                candidate
                    .get("interactionUuid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    == origin_uuid
            })
        };
        let target_idx = origin_idx.or_else(|| {
            rows[..idx].iter().rposition(|candidate| {
                !candidate
                    .get("botOutput")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .is_empty()
                    && candidate
                        .get("interactionType")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        != "Feedback"
            })
        });
        let target = target_idx.and_then(|i| rows.get(i));
        let nearest_user_question = target_idx.and_then(|target_i| {
            rows[..=target_i]
                .iter()
                .rposition(|candidate| {
                    !candidate
                        .get("userText")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .is_empty()
                        && candidate
                            .get("interactionType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            != "Feedback"
                })
                .and_then(|user_i| rows.get(user_i))
                .and_then(|candidate| candidate.get("userText"))
                .cloned()
        });

        targets.push(serde_json::json!({
            "feedbackLogId": row.get("logId").cloned().unwrap_or(serde_json::Value::Null),
            "feedbackScore": score,
            "feedbackType": if score < 0 { "negative" } else { "positive" },
            "feedbackInfo": row.get("feedbackInfo").cloned().unwrap_or(serde_json::Value::Null),
            "originatingInteractionUuid": if origin_uuid.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(origin_uuid) },
            "targetIndex": target_idx.map(|i| i as i64),
            "targetLogId": target.and_then(|t| t.get("logId")).cloned().unwrap_or(serde_json::Value::Null),
            "targetInteractionUuid": target.and_then(|t| t.get("interactionUuid")).cloned().unwrap_or(serde_json::Value::Null),
            "targetUserQuestion": nearest_user_question.unwrap_or(serde_json::Value::Null),
            "targetBotAnswer": target.and_then(|t| t.get("botOutput")).cloned().unwrap_or(serde_json::Value::Null),
            "targetRecognitionType": target.and_then(|t| t.get("recognitionType")).cloned().unwrap_or(serde_json::Value::Null),
            "targetRecognitionQuality": target.and_then(|t| t.get("recognitionQuality")).cloned().unwrap_or(serde_json::Value::Null),
            "targetArticleIds": target.and_then(|t| t.get("articleIds")).cloned().unwrap_or(serde_json::Value::Null),
            "targetDialogPaths": target.and_then(|t| t.get("dialogPaths")).cloned().unwrap_or(serde_json::Value::Null),
            "targetTriggeredContent": target.map(|t| compact_triggered_content(
                t.get("articleIds").unwrap_or(&serde_json::Value::Null),
                t.get("dialogPaths").unwrap_or(&serde_json::Value::Null),
                t.get("articles").unwrap_or(&serde_json::Value::Null),
            )).unwrap_or(serde_json::Value::Null),
            "targetEntityMatches": target.map(|t| compact_entity_matches(
                t.get("recognitionDetails").unwrap_or(&serde_json::Value::Null),
            )).unwrap_or(serde_json::Value::Null),
            "targetResolution": if target_idx.is_some() { if origin_idx.is_some() { "originatingInteractionId" } else { "previousBotOutputFallback" } } else { "none" },
        }));
    }
    targets
}

/// Deliberately *not* called `role` with `user`/`assistant` values: a CAI row
/// usually holds a question **and** its answer, so it is not a chat message. The
/// old naming looked like the chat convention while mostly meaning "both".
fn turn_kind_for_interaction(
    interaction_type: &str,
    user_text: &str,
    bot_output: &str,
) -> &'static str {
    if interaction_type == "Feedback" {
        "feedback"
    } else if !user_text.trim().is_empty() && bot_output.trim().is_empty() {
        "user_only"
    } else if user_text.trim().is_empty() && !bot_output.trim().is_empty() {
        "bot_only"
    } else if !user_text.trim().is_empty() || !bot_output.trim().is_empty() {
        "user_and_bot"
    } else {
        "system"
    }
}

/// Turns free text into a filename-safe slug: "Opening hours?" → "opening-hours".
/// `max_len` is a byte cap, applied only after a whole char has been pushed, so
/// the result is never cut mid-character.
fn filename_slug(input: &str, max_len: usize) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= max_len {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// Bytes per token, the usual rough ratio for prose. Only used to tell the user
/// whether a file will fit in a context window.
const AI_EXPORT_BYTES_PER_TOKEN: i64 = 4;
/// Crude per-turn size used *before* the export runs, when only the session and
/// interaction counts are known. The post-export figure uses real bytes.
const AI_EXPORT_EST_BYTES_PER_TURN: i64 = 450;
/// Above this the export is warned about — larger than most context windows.
const AI_EXPORT_LARGE_TOKENS: i64 = 200_000;

/// A `record_type: "export_header"` first line. Everything that is identical for
/// the whole export lives here instead of being repeated on every session line,
/// and the legend documents the fields a reader would otherwise have to guess at.
fn ai_export_header(
    exported_at: &str,
    search_context: &serde_json::Value,
    session_count: usize,
) -> serde_json::Value {
    serde_json::json!({
        "record_type": "export_header",
        "schema_version": 4,
        "exported_at": exported_at,
        "session_count": session_count,
        "search_context": search_context,
        "legend": {
            "format": "JSONL. This header is line 1; each following line is one conversation, as record_type 'session' with a 0-based session_index. Conversations are ordered newest first; turns within a conversation are oldest first.",
            "completeness": "session_count is how many conversation lines to expect. Fewer means the file was truncated.",
            "timestamps": "UTC, ISO-8601 with a trailing Z.",
            "turn_kind": "user_and_bot = one row holding a question and the answer to it (the common case, because that is how the source logs one exchange); user_only / bot_only = only one side present; feedback = a thumbs up/down row; system = neither.",
            "type": "The source system's own interaction type, e.g. 'QA' or 'Feedback'.",
            "recognition_quality": "How confident the bot was that it matched the user's question, 0-100. Compare against search_context.lowRecogThreshold.",
            "recognition_type": "How the answer was found, e.g. 'Entity Recognition'. Absent when nothing matched — that is a failed answer.",
            "triggered_content": "Which published content produced the answer: articles[].id, dialogs[] (dialog and node; status 'End' means the conversation finished there), events[].",
            "entity_matches": "Entities the recognizer found in the user's text, with the text that matched.",
            "feedback_targets": "Each thumbs up/down already joined to the answer it rated, so this join does not need redoing. target_resolution says how certain that join is: 'originatingInteractionId' = the log stated it; 'previousBotOutputFallback' = inferred from the preceding answer, so treat it as probable rather than certain.",
            "is_feedback_target": "Present as true only on turns that received feedback; absent on all others.",
            "conventions": "Empty fields are omitted rather than sent as null, so a missing key means no value. Answer HTML has been stripped to plain text."
        },
    })
}

/// Save-dialog default name: the search term, so exports are identifiable without
/// opening them. Depends on nothing but the args, which is why the save dialog can
/// open before the result set is queried.
fn suggested_ai_export_name(query: &str) -> String {
    let term = filename_slug(query, 60);
    if term.is_empty() {
        "conversation-analysis-export.jsonl".to_string()
    } else {
        format!("{term}.jsonl")
    }
}

#[tauri::command]
async fn export_conversations_for_ai(
    app: AppHandle,
    db_state: State<'_, SharedDbState>,
    args: GetSessionsArgs,
) -> Result<ConversationAiExportResult, String> {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    // Identical for every record, so build it once instead of per session.
    let mut search_context = serde_json::json!({
        "filter": args.filter.clone(),
        "query": args.query.clone(),
        "queryRegex": args.query_regex.unwrap_or(false),
        "queryScope": args.query_scope.clone(),
        "queryIds": args.query_ids.unwrap_or(false),
        "queryIdsOnly": args.query_ids_only.unwrap_or(false),
        "queryIdType": args.query_id_type.clone(),
        "dateFrom": args.date_from.clone(),
        "dateTo": args.date_to.clone(),
        "contextFilters": args.context_filters.clone(),
        "lowRecogThreshold": args.low_recog_threshold.unwrap_or(60).clamp(1, 99),
    });
    let query_text = args.query.clone().unwrap_or_default();

    let canceled = || ConversationAiExportResult {
        ok: false,
        canceled: true,
        jsonl_path: None,
        session_count: 0,
        feedback_count: 0,
        interaction_count: 0,
        bytes: 0,
        estimated_tokens: 0,
    };

    // Ask where to save first. The suggested name needs only the search term, so
    // the dialog opens immediately rather than after querying the whole result set
    // — and cancelling here costs no query at all.
    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();
    app.dialog()
        .file()
        .add_filter("JSONL", &["jsonl"])
        .set_file_name(suggested_ai_export_name(&query_text))
        .save_file(move |path| {
            let p = path.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(p);
        });
    let Some(mut jsonl_path) = rx.await.ok().flatten() else {
        return Ok(canceled());
    };
    if jsonl_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        jsonl_path.set_extension("jsonl");
    }

    let db = db_state.inner().clone();
    let (sessions, search_mode, planned_turns) = tauri::async_runtime::spawn_blocking(move || {
        let state = db.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("No database open.")?;
        let filter_query = build_session_filter_query(conn, &args)?;
        let params_ref: Vec<&dyn ToSql> =
            filter_query.param_values.iter().map(|b| b.as_ref()).collect();
        let sql = format!(
            r#"WITH
base_sessions AS (
    SELECT s.*
    FROM session_summary s
    {base_where}
)
{search_cte},
filtered_sessions AS (
    {filtered_from}
)
SELECT
    p.session_uuid,
    p.first_ts,
    p.last_ts,
    p.interaction_count,
    p.culture,
    p.first_user_message,
    p.has_neg_feedback,
    p.has_pos_feedback
FROM filtered_sessions p
ORDER BY p.first_ts DESC"#,
            base_where = filter_query.base_where.as_str(),
            search_cte = filter_query.search_cte.as_str(),
            filtered_from = filter_query.filtered_from.as_str(),
        );

        let sessions = {
            let mut stmt = conn.prepare(&sql).map_err(|e| format!("Export query error: {e}"))?;
            let mut rows = stmt
                .query(params_ref.as_slice())
                .map_err(|e| format!("Export query error: {e}"))?;
            let mut sessions = Vec::new();
            while let Some(row) = rows.next().map_err(|e| format!("Export query error: {e}"))? {
                let session_uuid = row.get::<_, Option<String>>(0).unwrap_or(None).unwrap_or_default();
                if session_uuid.is_empty() {
                    continue;
                }
                sessions.push(serde_json::json!({
                    "sessionUuid": session_uuid,
                    "firstTs": row.get::<_, Option<String>>(1).unwrap_or(None).unwrap_or_default(),
                    "lastTs": row.get::<_, Option<String>>(2).unwrap_or(None).unwrap_or_default(),
                    "interactionCount": row.get::<_, Option<i64>>(3).unwrap_or(None).unwrap_or(0),
                    "culture": row.get::<_, Option<String>>(4).unwrap_or(None).unwrap_or_default(),
                    "firstUserMessage": row.get::<_, Option<String>>(5).unwrap_or(None).unwrap_or_default(),
                    "hasNegFeedback": row.get::<_, Option<i64>>(6).unwrap_or(None).unwrap_or(0) == 1,
                    "hasPosFeedback": row.get::<_, Option<i64>>(7).unwrap_or(None).unwrap_or(0) == 1,
                }));
            }
            sessions
        };

        let planned_turns = sessions
            .iter()
            .filter_map(|s| s.get("interactionCount").and_then(|v| v.as_i64()))
            .sum::<i64>();
        Ok::<_, String>((sessions, filter_query.search_mode.clone(), planned_turns))
    })
    .await
    .map_err(|e| e.to_string())??;

    // A search can match far more than any model can read. Say so before the write,
    // which is the part that actually costs time.
    let planned_tokens = (planned_turns * AI_EXPORT_EST_BYTES_PER_TURN) / AI_EXPORT_BYTES_PER_TOKEN;
    if planned_tokens > AI_EXPORT_LARGE_TOKENS {
        let (ask_tx, ask_rx) = oneshot::channel::<bool>();
        app.dialog()
            .message(format!(
                "This export covers {} conversations and about {} interactions — very roughly {}k tokens.\n\nThat is larger than most model context windows. Narrowing the search first, or analysing the file with a script instead of pasting it, will work better.",
                sessions.len(),
                planned_turns,
                planned_tokens / 1000,
            ))
            .title("Large export")
            .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
                "Export anyway".to_string(),
                "Cancel".to_string(),
            ))
            .show(move |confirmed| {
                let _ = ask_tx.send(confirmed);
            });
        if !ask_rx.await.unwrap_or(false) {
            return Ok(canceled());
        }
    }

    if let Some(map) = search_context.as_object_mut() {
        map.insert(
            "resolvedSearchMode".to_string(),
            serde_json::Value::String(search_mode),
        );
    }
    let jsonl_path_for_work = jsonl_path.clone();
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = db.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("No database open.")?;
        // Buffered on purpose. `serde_json::to_writer` emits many tiny writes per
        // record, and straight onto an unbuffered File that is one syscall each —
        // measured at 8.5s vs 2.4s for 5k sessions, i.e. ~73% of the export was
        // syscall overhead. The remaining time is the query and JSON build.
        let file = fs::File::create(&jsonl_path_for_work)
            .map_err(|e| format!("Cannot create export file: {e}"))?;
        let mut out = std::io::BufWriter::new(file);
        let counts = write_ai_export(conn, &sessions, &search_context, &mut out)?;

        out.flush()
            .map_err(|e| format!("Cannot finish export file: {e}"))?;
        let bytes = out
            .get_ref()
            .metadata()
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        Ok(ConversationAiExportResult {
            ok: true,
            canceled: false,
            jsonl_path: Some(jsonl_path_for_work.to_string_lossy().into_owned()),
            session_count: sessions.len() as i64,
            feedback_count: counts.feedback_count,
            interaction_count: counts.interaction_count,
            bytes,
            estimated_tokens: bytes / AI_EXPORT_BYTES_PER_TOKEN,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

struct AiExportCounts {
    interaction_count: i64,
    feedback_count: i64,
}

/// Writes the JSONL body: one header line, then one line per session. Split out
/// from the command so the on-disk format is testable without a save dialog.
fn write_ai_export(
    conn: &Connection,
    sessions: &[serde_json::Value],
    search_context: &serde_json::Value,
    out: &mut impl Write,
) -> Result<AiExportCounts, String> {
    let exported_at = now_iso();
    let mut interaction_total = 0i64;
    let mut feedback_total = 0i64;

    // Line 1 carries everything that is constant for the export, so the
    // session lines below hold only what actually varies.
    let header = ai_export_header(&exported_at, search_context, sessions.len());
    serde_json::to_writer(&mut *out, &header)
        .map_err(|e| format!("Cannot write export header: {e}"))?;
    writeln!(out).map_err(|e| format!("Cannot write export file: {e}"))?;

    {
        // Prepared once; re-run per session instead of re-parsing the SQL.
        let mut inter_stmt = conn
            .prepare_cached(
                r#"SELECT
                log_id, interaction_uuid, timestamp_start,
                main_interaction_type, interaction_value, output_text,
                article_ids, dialog_paths, recognition_type, recognition_quality,
                articles, feedback_info, recognition_details
            FROM interactions
            WHERE session_uuid = ?1
            ORDER BY log_id ASC"#,
            )
            .map_err(|e| format!("Prepare interactions export error: {e}"))?;
        for (session_index, session) in sessions.iter().enumerate() {
            let session_uuid = session
                .get("sessionUuid")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mapped = inter_stmt
                .query_map(params![session_uuid], |row| {
                    let interaction_type = row.get::<_, String>(3).unwrap_or_default();
                    let user_text = row.get::<_, String>(4).unwrap_or_default();
                    let bot_output = row.get::<_, String>(5).unwrap_or_default();
                    let feedback_info = row.get::<_, String>(11).unwrap_or_default();
                    Ok(serde_json::json!({
                        "logId": row.get::<_, i64>(0).unwrap_or(0),
                        "interactionUuid": row.get::<_, String>(1).unwrap_or_default(),
                        "timestampStart": row.get::<_, String>(2).unwrap_or_default(),
                        "turnKind": turn_kind_for_interaction(&interaction_type, &user_text, &bot_output),
                        "interactionType": interaction_type,
                        "userText": user_text,
                        "botOutput": bot_output,
                        "articleIds": json_or_text(&row.get::<_, String>(6).unwrap_or_default()),
                        "dialogPaths": json_or_text(&row.get::<_, String>(7).unwrap_or_default()),
                        "recognitionType": row.get::<_, String>(8).unwrap_or_default(),
                        "recognitionQuality": row.get::<_, f64>(9).unwrap_or(0.0),
                        "articles": json_or_text(&row.get::<_, String>(10).unwrap_or_default()),
                        "feedbackInfo": json_or_text(&feedback_info),
                        "feedbackInfoRaw": feedback_info,
                        "recognitionDetails": json_or_text(&row.get::<_, String>(12).unwrap_or_default()),
                    }))
                })
                .map_err(|e| format!("Query interactions export error: {e}"))?;

            let mut conversation = Vec::new();
            for item in mapped {
                conversation.push(item.map_err(|e| format!("Read interactions export error: {e}"))?);
            }
            interaction_total += conversation.len() as i64;
            let feedback_targets = build_feedback_targets(&conversation);
            let feedback_count = feedback_targets.len() as i64;
            let target_indexes = feedback_targets
                .iter()
                .filter_map(|target| {
                    target
                        .get("targetIndex")
                        .and_then(|v| v.as_i64())
                        .map(|i| i as usize)
                })
                .collect::<std::collections::HashSet<_>>();
            let chat_trace = || -> Vec<serde_json::Value> {
                conversation
                    .iter()
                    .enumerate()
                    .map(|(idx, row)| compact_turn(row, target_indexes.contains(&idx)))
                    .collect()
            };

            feedback_total += feedback_count;
            let compact_feedback_targets = feedback_targets
                .into_iter()
                .map(|target| {
                    let feedback_info = target
                        .get("feedbackInfo")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let target_answer = target
                        .get("targetBotAnswer")
                        .and_then(|v| v.as_str())
                        .map(strip_html_text)
                        .unwrap_or_default();
                    serde_json::json!({
                        "feedback": {
                            "score": target.get("feedbackScore").cloned().unwrap_or(serde_json::Value::Null),
                            "label": feedback_info.get("label").cloned().unwrap_or(serde_json::Value::Null),
                            "comment": feedback_info.get("comment").cloned().unwrap_or(serde_json::Value::Null),
                            "log_id": target.get("feedbackLogId").cloned().unwrap_or(serde_json::Value::Null),
                            "target_resolution": target.get("targetResolution").cloned().unwrap_or(serde_json::Value::Null),
                        },
                        "target": {
                            "log_id": target.get("targetLogId").cloned().unwrap_or(serde_json::Value::Null),
                            "user_question": target.get("targetUserQuestion").cloned().unwrap_or(serde_json::Value::Null),
                            "answer_text": target_answer,
                            "recognition_type": target.get("targetRecognitionType").cloned().unwrap_or(serde_json::Value::Null),
                            "recognition_quality": target.get("targetRecognitionQuality").cloned().unwrap_or(serde_json::Value::Null),
                            "triggered_content": target.get("targetTriggeredContent").cloned().unwrap_or(serde_json::Value::Null),
                            "entity_matches": target.get("targetEntityMatches").cloned().unwrap_or(serde_json::Value::Null),
                        },
                    })
                })
                .filter_map(prune_empty_json)
                .collect::<Vec<_>>();

            let record = serde_json::json!({
                "record_type": "session",
                "session_index": session_index,
                "session": {
                    "session_uuid": session.get("sessionUuid").cloned().unwrap_or(serde_json::Value::Null),
                    "first_ts": utc_iso(session.get("firstTs").and_then(|v| v.as_str()).unwrap_or("")),
                    "last_ts": utc_iso(session.get("lastTs").and_then(|v| v.as_str()).unwrap_or("")),
                    "culture": session.get("culture").cloned().unwrap_or(serde_json::Value::Null),
                    "feedback_count": feedback_count,
                },
                "feedback_targets": compact_feedback_targets,
                "chat_trace": chat_trace(),
            });
            // `session_index: 0` must survive pruning — it is how a reader checks
            // nothing is missing against the header's session_count.
            let record = prune_empty_json(record).unwrap_or(serde_json::Value::Null);
            serde_json::to_writer(&mut *out, &record)
                .map_err(|e| format!("Cannot write export JSON: {e}"))?;
            writeln!(out).map_err(|e| format!("Cannot write export file: {e}"))?;
        }
    }

    Ok(AiExportCounts {
        interaction_count: interaction_total,
        feedback_count: feedback_total,
    })
}

#[tauri::command]
async fn get_context_options(
    db_state: State<'_, SharedDbState>,
) -> Result<Vec<ContextOption>, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
    let state = db.lock().map_err(|e| e.to_string())?;
    let conn = state.conn.as_ref().ok_or("No database open.")?;

    // Regular options: name × value with per-value session counts
    let mut stmt = conn
        .prepare(
            "SELECT name, value, COUNT(DISTINCT session_uuid) as session_count \
             FROM context_index \
             GROUP BY name, value \
             ORDER BY name ASC, value ASC \
             LIMIT 500",
        )
        .map_err(|e| format!("Prepare error: {e}"))?;

    let mut opts: Vec<ContextOption> = stmt
        .query_map([], |row| {
            Ok(ContextOption {
                name:  row.get(0)?,
                value: row.get(1)?,
                count: row.get(2)?,
            })
        })
        .map_err(|e| format!("Query error: {e}"))?
        .filter_map(|r| r.ok())
        .filter(|o| !o.name.is_empty())
        .collect();

    // "Not set" options: for each known name, count sessions that have NO entry for that name.
    // not_set_count = total_sessions - sessions_with_that_name
    // Total computed once here instead of as a scalar subquery in both the
    // SELECT and the HAVING clause.
    let total_sessions: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT session_uuid) FROM interactions",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let mut stmt2 = conn
        .prepare(
            "SELECT ci.name, \
              ?1 - COUNT(DISTINCT ci.session_uuid) \
             FROM context_index ci \
             GROUP BY ci.name \
             HAVING ?1 - COUNT(DISTINCT ci.session_uuid) > 0 \
             ORDER BY ci.name ASC",
        )
        .map_err(|e| format!("Prepare error: {e}"))?;

    let not_set_opts: Vec<ContextOption> = stmt2
        .query_map(params![total_sessions], |row| {
            Ok(ContextOption {
                name:  row.get(0)?,
                value: "__not_set__".to_string(),
                count: row.get(1)?,
            })
        })
        .map_err(|e| format!("Query error: {e}"))?
        .filter_map(|r| r.ok())
        .filter(|o| !o.name.is_empty())
        .collect();

    opts.extend(not_set_opts);
    Ok(opts)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_session_interactions(
    db_state: State<'_, SharedDbState>,
    session_uuid: String,
) -> Result<Vec<InteractionRow>, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = db.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("No database open.")?;

        let mut stmt = conn
            .prepare_cached(
                r#"SELECT
                log_id, interaction_uuid, session_uuid,
                timestamp_start, timestamp_end, culture,
                main_interaction_type, all_interaction_types,
                interaction_value, output_text,
                article_ids, dialog_paths, tdialog_status,
                recognition_type, recognition_quality,
                generative_ai_sources, articles, faqs_found,
                contexts, pages, link_click_info, feedback_info,
                output_metadata, recognition_details
            FROM interactions
            WHERE session_uuid = ?1
            ORDER BY log_id ASC"#,
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map(params![session_uuid], |row| {
                Ok(InteractionRow {
                    log_id: row.get(0)?,
                    interaction_uuid: row.get::<_, String>(1).unwrap_or_default(),
                    session_uuid: row.get::<_, String>(2).unwrap_or_default(),
                    timestamp_start: row.get::<_, String>(3).unwrap_or_default(),
                    timestamp_end: row.get::<_, String>(4).unwrap_or_default(),
                    culture: row.get::<_, String>(5).unwrap_or_default(),
                    main_interaction_type: row.get::<_, String>(6).unwrap_or_default(),
                    all_interaction_types: row.get::<_, String>(7).unwrap_or_default(),
                    interaction_value: row.get::<_, String>(8).unwrap_or_default(),
                    output_text: row.get::<_, String>(9).unwrap_or_default(),
                    article_ids: row.get::<_, String>(10).unwrap_or_default(),
                    dialog_paths: row.get::<_, String>(11).unwrap_or_default(),
                    tdialog_status: row.get::<_, String>(12).unwrap_or_default(),
                    recognition_type: row.get::<_, String>(13).unwrap_or_default(),
                    recognition_quality: row.get::<_, f64>(14).unwrap_or(0.0),
                    generative_ai_sources: row.get::<_, String>(15).unwrap_or_default(),
                    articles: row.get::<_, String>(16).unwrap_or_default(),
                    faqs_found: row.get::<_, String>(17).unwrap_or_default(),
                    contexts: row.get::<_, String>(18).unwrap_or_default(),
                    pages: row.get::<_, String>(19).unwrap_or_default(),
                    link_click_info: row.get::<_, String>(20).unwrap_or_default(),
                    feedback_info: row.get::<_, String>(21).unwrap_or_default(),
                    output_metadata: row.get::<_, String>(22).unwrap_or_default(),
                    recognition_details: row.get::<_, String>(23).unwrap_or_default(),
                })
            })
            .map_err(|e| format!("Query error: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rows)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Database management commands ─────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DayStats {
    date: String,
    count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DbDailyStats {
    total: i64,
    days: Vec<DayStats>,
}

#[tauri::command]
async fn get_db_daily_stats(db_state: State<'_, SharedDbState>) -> Result<DbDailyStats, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = db.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("No database open.")?;

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM interactions", [], |row| row.get(0))
            .unwrap_or(0);

        // substr rather than DATE(): timestamp_start is always stored as
        // "YYYY-MM-DDTHH:MM:SS", so this is exact and skips a function call per
        // row. get_db_hour_coverage already slices it the same way.
        let mut stmt = conn
            .prepare(
                "SELECT substr(timestamp_start, 1, 10) AS day, COUNT(*) AS cnt \
                 FROM interactions \
                 GROUP BY day \
                 ORDER BY day DESC",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let days = stmt
            .query_map([], |row| {
                Ok(DayStats {
                    date: row.get::<_, String>(0).unwrap_or_default(),
                    count: row.get::<_, i64>(1).unwrap_or(0),
                })
            })
            .map_err(|e| format!("Query error: {e}"))?
            .filter_map(|r| r.ok())
            .filter(|d| !d.date.is_empty())
            .collect();

        Ok(DbDailyStats { total, days })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct DayCoverage {
    date: String,
    /// Bitmask of the UTC hours (0..23) this day is *covered* for: the union of
    /// the hours holding at least one interaction and the hours an Analytics API
    /// window explicitly requested.
    ///
    /// The union is the point. Row presence alone cannot tell an hour the API
    /// answered with nothing apart from an hour never requested, so a quiet
    /// night left a day permanently short of 24 and it was re-downloaded on
    /// every run. Rows still count on their own, which is what keeps manually
    /// imported portal CSVs — which have no request window — working unchanged.
    hours: i64,
    count: i64,
}

#[tauri::command]
async fn get_db_hour_coverage(
    db_state: State<'_, SharedDbState>,
    since_date: Option<String>,
) -> Result<Vec<DayCoverage>, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = db.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("No database open.")?;
        hour_coverage(conn, since_date)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Split out of the command so the union can be tested against a real
/// connection without a Tauri `State`.
fn hour_coverage(conn: &Connection, since_date: Option<String>) -> Result<Vec<DayCoverage>, String> {
    {
        // substr rather than DATE()/strftime: timestamp_start is always stored
        // as "YYYY-MM-DDTHH:MM:SS", so this is both exact and index-friendly.
        //
        // `since_date` bounds an otherwise full-table aggregate. The importer is
        // its only caller that needs hour granularity and it never looks past
        // the API's retention window, so passing that cutoff turns this from a
        // scan of the whole table into a range seek on idx_timestamp. The
        // `length(...)` guard stays but is no longer the leading predicate,
        // which is what made the old form unsargable.
        let since_date_for_windows = since_date.clone();
        let (bound, params): (&str, Vec<Box<dyn ToSql>>) = match since_date {
            Some(d) if !d.is_empty() => (
                "WHERE timestamp_start >= ?1 AND length(timestamp_start) >= 13 ",
                vec![Box::new(d)],
            ),
            _ => ("WHERE length(timestamp_start) >= 13 ", Vec::new()),
        };
        let mut stmt = conn
            .prepare(&format!(
                "SELECT substr(timestamp_start, 1, 10) AS day, \
                        CAST(substr(timestamp_start, 12, 2) AS INTEGER) AS hr, \
                        COUNT(*) \
                 FROM interactions \
                 {bound}\
                 GROUP BY day, hr \
                 ORDER BY day"
            ))
            .map_err(|e| format!("Prepare error: {e}"))?;

        let refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0).unwrap_or_default(),
                    row.get::<_, i64>(1).unwrap_or(-1),
                    row.get::<_, i64>(2).unwrap_or(0),
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        // Keyed rather than appended, because the recorded-window pass below
        // merges into the same days and can introduce days of its own.
        let mut by_day: BTreeMap<String, DayCoverage> = BTreeMap::new();
        for r in rows.flatten() {
            let (day, hr, count) = r;
            if day.is_empty() || !(0..24).contains(&hr) {
                continue;
            }
            let e = by_day.entry(day.clone()).or_insert_with(|| DayCoverage {
                date: day,
                hours: 0,
                count: 0,
            });
            e.hours |= 1 << hr;
            e.count += count;
        }

        // Union in the hours we actually requested. An hour the API answered
        // with zero interactions is covered, and without this it would look
        // identical to an hour that was never fetched — the whole reason a
        // quiet night used to leave a day permanently orange.
        //
        // A day can appear here with no rows at all (a window fetched that
        // returned nothing), so this pass may add days the first one never saw.
        let (w_bound, w_params): (&str, Vec<Box<dyn ToSql>>) = match &since_date_for_windows {
            Some(d) if !d.is_empty() => ("WHERE day >= ?1 ", vec![Box::new(d.clone())]),
            _ => ("", Vec::new()),
        };
        let mut w_stmt = conn
            .prepare(&format!(
                "SELECT day, hours FROM imported_windows {w_bound}"
            ))
            .map_err(|e| format!("Prepare error: {e}"))?;
        let w_refs: Vec<&dyn ToSql> = w_params.iter().map(|p| p.as_ref()).collect();
        let w_rows = w_stmt
            .query_map(w_refs.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0).unwrap_or_default(),
                    row.get::<_, i64>(1).unwrap_or(0),
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;
        for (day, hours) in w_rows.flatten() {
            if day.is_empty() || hours == 0 {
                continue;
            }
            let e = by_day.entry(day.clone()).or_insert_with(|| DayCoverage {
                date: day,
                hours: 0,
                count: 0,
            });
            e.hours |= hours & ALL_HOURS_MASK;
        }

        Ok(by_day.into_values().collect())
    }
}

/// Every UTC hour bit set — the mask a fully covered day carries.
const ALL_HOURS_MASK: i64 = (1 << 24) - 1;

/// Split an ISO window into its UTC day and the inclusive hour range it spans.
///
/// Deliberately strict: the importer only ever produces windows inside a single
/// UTC day (see `buildImportQueue`), so anything else is a caller bug worth
/// surfacing rather than silently recording against the wrong day.
fn window_day_hours(start_utc: &str, end_utc: &str) -> Result<(String, u32, u32), String> {
    let parse = |s: &str| -> Result<(String, u32), String> {
        let mut t = String::new();
        parse_ts_into(s, &mut t);
        if t.len() < 13 {
            return Err(format!("Unparseable window bound: {s}"));
        }
        let hour: u32 = t[11..13]
            .parse()
            .map_err(|_| format!("Unparseable hour in: {s}"))?;
        if hour > 23 {
            return Err(format!("Hour out of range in: {s}"));
        }
        Ok((t[..10].to_string(), hour))
    };
    let (start_day, h0) = parse(start_utc)?;
    let (end_day, h1) = parse(end_utc)?;
    if start_day != end_day {
        return Err(format!(
            "A window must stay inside one UTC day ({start_day} → {end_day})"
        ));
    }
    if h1 < h0 {
        return Err(format!("Window ends before it starts ({start_utc} → {end_utc})"));
    }
    Ok((start_day, h0, h1))
}

/// Mark every UTC hour a successfully imported window covered.
///
/// Called once per downloaded window, after its rows are in — never for a
/// manual CSV import, which has no request window to record.
#[tauri::command]
async fn record_imported_window(
    db_state: State<'_, SharedDbState>,
    start_utc: String,
    end_utc: String,
) -> Result<(), String> {
    let (day, h0, h1) = window_day_hours(&start_utc, &end_utc)?;
    let mut mask: i64 = 0;
    for h in h0..=h1 {
        mask |= 1 << h;
    }
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = db.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("No database open.")?;
        conn.execute(
            "INSERT INTO imported_windows(day, hours) VALUES (?1, ?2) \
             ON CONFLICT(day) DO UPDATE SET hours = hours | excluded.hours",
            params![day, mask],
        )
        .map_err(|e| format!("Cannot record the imported window: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteByDatesArgs {
    dates: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteResult {
    deleted: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompactResult {
    bytes_before: u64,
    bytes_after: u64,
    duration_ms: u64,
}

/// Rebuild the database file, returning free pages to the filesystem.
///
/// Deleting rows — and dropping the old FTS content table and the dead indexes
/// on upgrade — frees pages inside the file but never shrinks it. Only VACUUM
/// does that, and it needs roughly the file's own size again in temp space
/// while it runs, so it stays a deliberate user action rather than something
/// that happens silently on open.
#[tauri::command]
async fn compact_database(db_state: State<'_, SharedDbState>) -> Result<CompactResult, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut state = db.lock().map_err(|e| e.to_string())?;
        let path = state.path.clone().ok_or("No database open.")?;
        let conn = state.conn.as_mut().ok_or("No database open.")?;
        let size_of = |p: &str| fs::metadata(p).map(|m| m.len()).unwrap_or(0);

        // Fold the WAL back in first, or "before" understates the real size and
        // VACUUM has less to reclaim.
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        let bytes_before = size_of(&path);

        let started = Instant::now();
        conn.execute_batch("VACUUM;")
            .map_err(|e| format!("Could not compact the database: {e}"))?;
        // VACUUM leaves journal_mode intact in WAL, but checkpoint again so the
        // reported size is the file the user will see.
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        let duration_ms = started.elapsed().as_millis() as u64;
        let bytes_after = size_of(&path);
        log::info!(
            target: "import",
            "compacted database: {bytes_before} → {bytes_after} bytes in {duration_ms}ms"
        );
        Ok(CompactResult { bytes_before, bytes_after, duration_ms })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn delete_interactions_by_dates(
    db_state: State<'_, SharedDbState>,
    args: DeleteByDatesArgs,
) -> Result<DeleteResult, String> {
    if args.dates.is_empty() {
        return Ok(DeleteResult { deleted: 0 });
    }
    // Validate each date looks like YYYY-MM-DD to prevent injection
    for d in &args.dates {
        if d.len() != 10 || !d.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return Err(format!("Invalid date format: {d}"));
        }
    }
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut state = db.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_mut().ok_or("No database open.")?;

        let tx = conn.transaction().map_err(|e| e.to_string())?;

        // Collect log_ids to delete (for FTS cleanup)
        let placeholders = args.dates.iter().enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            args.dates.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();

        // Remove stale FTS5 entries in one set-based statement before deleting
        let _ = tx.execute(
            &format!(
                "DELETE FROM interactions_fts WHERE rowid IN \
                 (SELECT log_id FROM interactions WHERE DATE(timestamp_start) IN ({placeholders}))"
            ),
            params_refs.as_slice(),
        );

        // Delete from interactions
        let deleted = tx
            .execute(
                &format!("DELETE FROM interactions WHERE DATE(timestamp_start) IN ({placeholders})"),
                params_refs.as_slice(),
            )
            .map_err(|e| format!("Delete error: {e}"))? as i64;

        // Forget that these days were ever fetched. Leaving the record behind
        // would mark a day the user just deleted as fully imported, so the
        // importer would refuse to download it again.
        let _ = tx.execute(
            &format!("DELETE FROM imported_windows WHERE day IN ({placeholders})"),
            params_refs.as_slice(),
        );

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;
        if deleted > 0 {
            cleanup_orphan_contexts(conn);
            rebuild_session_summary(conn)?;
        }

        Ok(DeleteResult { deleted })
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Analytics API commands ────────────────────────────────────────────────────

type SharedAnalytics = Arc<AnalyticsState>;

fn analytics_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve the app data directory: {e}"))
}

fn analytics_cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_cache_dir()
        .map_err(|e| format!("Cannot resolve the app cache directory: {e}"))
}

#[tauri::command]
fn get_analytics_config(app: AppHandle) -> Result<AnalyticsConfigView, String> {
    let dir = analytics_data_dir(&app)?;
    Ok((&analytics_api::load_config(&dir)).into())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveAnalyticsConfigArgs {
    client_id: String,
    /// Blank means "keep the stored secret" so the renderer never has to hold it.
    client_secret: Option<String>,
    customer_key: String,
    project_key: String,
    culture: String,
    environment: String,
    active_session_only: bool,
}

#[tauri::command]
fn save_analytics_config(
    app: AppHandle,
    analytics: State<'_, SharedAnalytics>,
    args: SaveAnalyticsConfigArgs,
) -> Result<AnalyticsConfigView, String> {
    if !matches!(args.environment.as_str(), "Production" | "Staging") {
        return Err("Environment must be Production or Staging".into());
    }
    let dir = analytics_data_dir(&app)?;
    let existing = analytics_api::load_config(&dir);
    let secret = match args.client_secret {
        Some(s) if !s.is_empty() => s,
        _ => existing.client_secret.clone(),
    };
    let cfg = AnalyticsConfig {
        client_id: args.client_id.trim().to_string(),
        client_secret: secret,
        customer_key: args.customer_key.trim().to_string(),
        project_key: args.project_key.trim().to_string(),
        culture: args.culture.trim().to_string(),
        environment: args.environment,
        active_session_only: args.active_session_only,
    };
    analytics_api::save_config(&dir, &cfg)?;
    // Credentials changed — the cached bearer token may no longer apply.
    analytics.clear_token();
    log::info!(target: "analytics", "credentials saved (configured: {})", cfg.is_complete());
    Ok((&cfg).into())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionTestResult {
    ok: bool,
    message: String,
}

#[tauri::command]
async fn test_analytics_connection(
    app: AppHandle,
    analytics: State<'_, SharedAnalytics>,
) -> Result<ConnectionTestResult, String> {
    let dir = analytics_data_dir(&app)?;
    let cfg = analytics_api::load_config(&dir);
    if !cfg.is_complete() {
        return Ok(ConnectionTestResult {
            ok: false,
            message: "Fill in every field before testing the connection.".into(),
        });
    }
    let state = analytics.inner().clone();
    // Force a real round-trip rather than reporting on a cached token.
    state.clear_token();
    match state.token(&cfg).await {
        Ok(_) => Ok(ConnectionTestResult {
            ok: true,
            message: "Connected — access token received.".into(),
        }),
        Err(e) => Ok(ConnectionTestResult {
            ok: false,
            message: e.message,
        }),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchWindowArgs {
    start_utc: String,
    end_utc: String,
}

#[tauri::command]
async fn fetch_analytics_window(
    app: AppHandle,
    analytics: State<'_, SharedAnalytics>,
    args: FetchWindowArgs,
) -> Result<FetchOutcome, FetchError> {
    let dir = analytics_data_dir(&app).map_err(|e| {
        FetchError {
            kind: analytics_api::FetchErrorKind::Config,
            message: e,
            retryable: false,
            retry_after_secs: None,
        }
    })?;
    let cache = analytics_cache_dir(&app).map_err(|e| FetchError {
        kind: analytics_api::FetchErrorKind::Config,
        message: e,
        retryable: false,
        retry_after_secs: None,
    })?;
    let cfg = analytics_api::load_config(&dir);
    let state = analytics.inner().clone();
    state
        .fetch_window(&cfg, &cache, &args.start_utc, &args.end_utc)
        .await
}

#[tauri::command]
fn cleanup_analytics_temp(app: AppHandle, paths: Option<Vec<String>>) -> Result<u32, String> {
    let cache = analytics_cache_dir(&app)?;
    Ok(analytics_api::cleanup_temp(&cache, &paths.unwrap_or_default()))
}

// ── Flagged conversations commands ────────────────────────────────────────────

#[tauri::command]
async fn flag_session(
    db_state: State<'_, SharedDbState>,
    flagged_db: State<'_, SharedFlaggedDb>,
    session_uuid: String,
    flagged_log_ids: Vec<i64>,
    source_db_path: String,
) -> Result<i64, String> {
    let db = db_state.inner().clone();
    let fdb = flagged_db.inner().clone();
    let flagged_set: std::collections::HashSet<i64> = flagged_log_ids.into_iter().collect();

    tauri::async_runtime::spawn_blocking(move || {
        // 1. Read all interactions for the session from the regular DB
        let (rows, culture, first_ts) = {
            let state = db.lock().map_err(|e| e.to_string())?;
            let conn = state.conn.as_ref().ok_or("No database open.")?;
            let mut stmt = conn
                .prepare(
                    r#"SELECT
                        log_id, interaction_uuid, session_uuid,
                        timestamp_start, timestamp_end, culture,
                        main_interaction_type, all_interaction_types,
                        interaction_value, output_text,
                        article_ids, dialog_paths, tdialog_status,
                        recognition_type, recognition_quality,
                        generative_ai_sources, articles, faqs_found,
                        contexts, pages, link_click_info, feedback_info,
                        output_metadata, recognition_details
                    FROM interactions
                    WHERE session_uuid = ?1
                    ORDER BY log_id ASC"#,
                )
                .map_err(|e| format!("Prepare error: {e}"))?;
            let rows: Vec<InteractionRow> = stmt
                .query_map(params![session_uuid], |row| {
                    Ok(InteractionRow {
                        log_id:                  row.get(0)?,
                        interaction_uuid:        row.get::<_, String>(1).unwrap_or_default(),
                        session_uuid:            row.get::<_, String>(2).unwrap_or_default(),
                        timestamp_start:         row.get::<_, String>(3).unwrap_or_default(),
                        timestamp_end:           row.get::<_, String>(4).unwrap_or_default(),
                        culture:                 row.get::<_, String>(5).unwrap_or_default(),
                        main_interaction_type:   row.get::<_, String>(6).unwrap_or_default(),
                        all_interaction_types:   row.get::<_, String>(7).unwrap_or_default(),
                        interaction_value:       row.get::<_, String>(8).unwrap_or_default(),
                        output_text:             row.get::<_, String>(9).unwrap_or_default(),
                        article_ids:             row.get::<_, String>(10).unwrap_or_default(),
                        dialog_paths:            row.get::<_, String>(11).unwrap_or_default(),
                        tdialog_status:          row.get::<_, String>(12).unwrap_or_default(),
                        recognition_type:        row.get::<_, String>(13).unwrap_or_default(),
                        recognition_quality:     row.get::<_, f64>(14).unwrap_or(0.0),
                        generative_ai_sources:   row.get::<_, String>(15).unwrap_or_default(),
                        articles:                row.get::<_, String>(16).unwrap_or_default(),
                        faqs_found:              row.get::<_, String>(17).unwrap_or_default(),
                        contexts:                row.get::<_, String>(18).unwrap_or_default(),
                        pages:                   row.get::<_, String>(19).unwrap_or_default(),
                        link_click_info:         row.get::<_, String>(20).unwrap_or_default(),
                        feedback_info:           row.get::<_, String>(21).unwrap_or_default(),
                        output_metadata:         row.get::<_, String>(22).unwrap_or_default(),
                        recognition_details:     row.get::<_, String>(23).unwrap_or_default(),
                    })
                })
                .map_err(|e| format!("Query error: {e}"))?
                .filter_map(|r| r.ok())
                .collect();
            let culture = rows.first().map(|r| r.culture.clone()).unwrap_or_default();
            let first_ts = rows.first().map(|r| r.timestamp_start.clone()).unwrap_or_default();
            (rows, culture, first_ts)
        };

        // 2. Write to flagged DB
        let mut fstate = fdb.lock().map_err(|e| e.to_string())?;
        let fconn = fstate.conn.as_mut().ok_or("Flagged database not initialized.")?;

        let flagged_at = now_iso();
        let interaction_count = rows.len() as i64;

        fconn
            .execute(
                "INSERT INTO flagged_sessions (session_uuid, flagged_at, source_db_path, culture, first_ts, interaction_count) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![session_uuid, flagged_at, source_db_path, culture, first_ts, interaction_count],
            )
            .map_err(|e| format!("Insert session error: {e}"))?;

        let flag_id = fconn.last_insert_rowid();

        {
            let tx = fconn.transaction().map_err(|e| format!("Transaction error: {e}"))?;
            let mut ins_stmt = tx
                .prepare_cached(
                    "INSERT INTO flagged_interactions \
                     (flag_id, log_id, interaction_uuid, session_uuid, timestamp_start, timestamp_end, \
                      culture, main_interaction_type, all_interaction_types, interaction_value, output_text, \
                      article_ids, dialog_paths, tdialog_status, recognition_type, recognition_quality, \
                      generative_ai_sources, articles, faqs_found, contexts, pages, link_click_info, \
                      feedback_info, output_metadata, recognition_details, is_flagged) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;
            for row in &rows {
                let is_flagged = if flagged_set.contains(&row.log_id) { 1i64 } else { 0i64 };
                ins_stmt.execute(
                    params![
                        flag_id,
                        row.log_id,
                        row.interaction_uuid,
                        row.session_uuid,
                        row.timestamp_start,
                        row.timestamp_end,
                        row.culture,
                        row.main_interaction_type,
                        row.all_interaction_types,
                        row.interaction_value,
                        row.output_text,
                        row.article_ids,
                        row.dialog_paths,
                        row.tdialog_status,
                        row.recognition_type,
                        row.recognition_quality,
                        row.generative_ai_sources,
                        row.articles,
                        row.faqs_found,
                        row.contexts,
                        row.pages,
                        row.link_click_info,
                        row.feedback_info,
                        row.output_metadata,
                        row.recognition_details,
                        is_flagged,
                    ],
                )
                .map_err(|e| format!("Insert interaction error: {e}"))?;
            }
            drop(ins_stmt);
            tx.commit().map_err(|e| format!("Commit error: {e}"))?;
        }

        Ok(flag_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Flagged folder commands ──────────────────────────────────────────────────

#[tauri::command]
async fn get_flagged_folders(
    flagged_db: State<'_, SharedFlaggedDb>,
) -> Result<Vec<FlaggedFolder>, String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_ref()
            .ok_or("Flagged database not initialized.")?;
        let mut stmt = conn
            .prepare(
                "SELECT ff.folder_id, ff.name, ff.created_at, ff.sort_order, \
                        COUNT(fs.flag_id) AS session_count \
                 FROM flagged_folders ff \
                 LEFT JOIN flagged_sessions fs ON fs.folder_id = ff.folder_id \
                 GROUP BY ff.folder_id \
                 ORDER BY ff.sort_order ASC, ff.created_at ASC",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FlaggedFolder {
                    folder_id: row.get(0)?,
                    name: row.get::<_, String>(1).unwrap_or_default(),
                    created_at: row.get::<_, String>(2).unwrap_or_default(),
                    sort_order: row.get::<_, i64>(3).unwrap_or(0),
                    session_count: row.get::<_, i64>(4).unwrap_or(0),
                })
            })
            .map_err(|e| format!("Query error: {e}"))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn create_flagged_folder(
    flagged_db: State<'_, SharedFlaggedDb>,
    name: String,
) -> Result<FlaggedFolder, String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("Flagged database not initialized.")?;
        let now = now_iso();
        conn.execute(
            "INSERT INTO flagged_folders (name, created_at, sort_order) VALUES (?1, ?2, (SELECT COALESCE(MAX(sort_order),0)+1 FROM flagged_folders))",
            params![name, now],
        )
        .map_err(|e| format!("Insert error: {e}"))?;
        let folder_id = conn.last_insert_rowid();
        let folder = conn
            .query_row(
                "SELECT folder_id, name, created_at, sort_order, 0 FROM flagged_folders WHERE folder_id = ?1",
                params![folder_id],
                |row| Ok(FlaggedFolder {
                    folder_id:     row.get(0)?,
                    name:          row.get::<_, String>(1).unwrap_or_default(),
                    created_at:    row.get::<_, String>(2).unwrap_or_default(),
                    sort_order:    row.get::<_, i64>(3).unwrap_or(0),
                    session_count: 0,
                }),
            )
            .map_err(|e| format!("Fetch error: {e}"))?;
        Ok(folder)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn rename_flagged_folder(
    flagged_db: State<'_, SharedFlaggedDb>,
    folder_id: i64,
    name: String,
) -> Result<(), String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_ref()
            .ok_or("Flagged database not initialized.")?;
        conn.execute(
            "UPDATE flagged_folders SET name = ?1 WHERE folder_id = ?2",
            params![name, folder_id],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn delete_flagged_folder(
    flagged_db: State<'_, SharedFlaggedDb>,
    folder_id: i64,
) -> Result<(), String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_ref()
            .ok_or("Flagged database not initialized.")?;
        // Sessions are moved to "unfiled" (folder_id = NULL) via ON DELETE SET NULL
        conn.execute(
            "DELETE FROM flagged_folders WHERE folder_id = ?1",
            params![folder_id],
        )
        .map_err(|e| format!("Delete error: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn move_to_flagged_folder(
    flagged_db: State<'_, SharedFlaggedDb>,
    flag_id: i64,
    folder_id: Option<i64>,
) -> Result<(), String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_ref()
            .ok_or("Flagged database not initialized.")?;
        conn.execute(
            "UPDATE flagged_sessions SET folder_id = ?1 WHERE flag_id = ?2",
            params![folder_id, flag_id],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_flagged_sessions(
    flagged_db: State<'_, SharedFlaggedDb>,
) -> Result<Vec<FlaggedSessionSummary>, String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("Flagged database not initialized.")?;
        let mut stmt = conn
            .prepare(
                "SELECT fs.flag_id, fs.session_uuid, fs.flagged_at, fs.source_db_path, \
                        fs.culture, fs.first_ts, fs.interaction_count, \
                        COALESCE((SELECT COUNT(*) FROM flagged_interactions fi \
                                  WHERE fi.flag_id = fs.flag_id AND fi.is_flagged = 1), 0) AS flagged_count, \
                        fs.folder_id, COALESCE(fs.notes, '') \
                 FROM flagged_sessions fs \
                 ORDER BY fs.flagged_at DESC",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FlaggedSessionSummary {
                    flag_id:           row.get(0)?,
                    session_uuid:      row.get::<_, String>(1).unwrap_or_default(),
                    flagged_at:        row.get::<_, String>(2).unwrap_or_default(),
                    source_db_path:    row.get::<_, String>(3).unwrap_or_default(),
                    culture:           row.get::<_, String>(4).unwrap_or_default(),
                    first_ts:          row.get::<_, String>(5).unwrap_or_default(),
                    interaction_count: row.get::<_, i64>(6).unwrap_or(0),
                    flagged_count:     row.get::<_, i64>(7).unwrap_or(0),
                    folder_id:         row.get::<_, Option<i64>>(8).unwrap_or(None),
                    notes:             row.get::<_, String>(9).unwrap_or_default(),
                })
            })
            .map_err(|e| format!("Query error: {e}"))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_flagged_session_interactions(
    flagged_db: State<'_, SharedFlaggedDb>,
    flag_id: i64,
) -> Result<Vec<FlaggedInteractionRow>, String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_ref()
            .ok_or("Flagged database not initialized.")?;
        let mut stmt = conn
            .prepare(
                r#"SELECT
                    log_id, interaction_uuid, session_uuid,
                    timestamp_start, timestamp_end, culture,
                    main_interaction_type, all_interaction_types,
                    interaction_value, output_text,
                    article_ids, dialog_paths, tdialog_status,
                    recognition_type, recognition_quality,
                    generative_ai_sources, articles, faqs_found,
                    contexts, pages, link_click_info, feedback_info,
                    output_metadata, recognition_details, is_flagged
                FROM flagged_interactions
                WHERE flag_id = ?1
                ORDER BY id ASC"#,
            )
            .map_err(|e| format!("Prepare error: {e}"))?;
        let rows = stmt
            .query_map(params![flag_id], |row| {
                Ok(FlaggedInteractionRow {
                    log_id: row.get::<_, i64>(0).unwrap_or(0),
                    interaction_uuid: row.get::<_, String>(1).unwrap_or_default(),
                    session_uuid: row.get::<_, String>(2).unwrap_or_default(),
                    timestamp_start: row.get::<_, String>(3).unwrap_or_default(),
                    timestamp_end: row.get::<_, String>(4).unwrap_or_default(),
                    culture: row.get::<_, String>(5).unwrap_or_default(),
                    main_interaction_type: row.get::<_, String>(6).unwrap_or_default(),
                    all_interaction_types: row.get::<_, String>(7).unwrap_or_default(),
                    interaction_value: row.get::<_, String>(8).unwrap_or_default(),
                    output_text: row.get::<_, String>(9).unwrap_or_default(),
                    article_ids: row.get::<_, String>(10).unwrap_or_default(),
                    dialog_paths: row.get::<_, String>(11).unwrap_or_default(),
                    tdialog_status: row.get::<_, String>(12).unwrap_or_default(),
                    recognition_type: row.get::<_, String>(13).unwrap_or_default(),
                    recognition_quality: row.get::<_, f64>(14).unwrap_or(0.0),
                    generative_ai_sources: row.get::<_, String>(15).unwrap_or_default(),
                    articles: row.get::<_, String>(16).unwrap_or_default(),
                    faqs_found: row.get::<_, String>(17).unwrap_or_default(),
                    contexts: row.get::<_, String>(18).unwrap_or_default(),
                    pages: row.get::<_, String>(19).unwrap_or_default(),
                    link_click_info: row.get::<_, String>(20).unwrap_or_default(),
                    feedback_info: row.get::<_, String>(21).unwrap_or_default(),
                    output_metadata: row.get::<_, String>(22).unwrap_or_default(),
                    recognition_details: row.get::<_, String>(23).unwrap_or_default(),
                    is_flagged: row.get::<_, i64>(24).unwrap_or(0) != 0,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn save_flagged_note(
    flagged_db: State<'_, SharedFlaggedDb>,
    flag_id: i64,
    notes: String,
) -> Result<(), String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_ref()
            .ok_or("Flagged database not initialized.")?;
        conn.execute(
            "UPDATE flagged_sessions SET notes = ?1 WHERE flag_id = ?2",
            params![notes, flag_id],
        )
        .map_err(|e| format!("Update error: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn unflag_session(
    flagged_db: State<'_, SharedFlaggedDb>,
    flag_id: i64,
) -> Result<(), String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state
            .conn
            .as_ref()
            .ok_or("Flagged database not initialized.")?;
        conn.execute(
            "DELETE FROM flagged_sessions WHERE flag_id = ?1",
            params![flag_id],
        )
        .map_err(|e| format!("Delete error: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(Mutex::new(WatchState::default())))
        .manage(Arc::new(Mutex::new(DbState::default())) as SharedDbState)
        .manage(Arc::new(Mutex::new(None)) as SharedSearchInterrupt)
        .manage(Arc::new(Mutex::new(FlaggedDbState::default())) as SharedFlaggedDb)
        .manage(Arc::new(AnalyticsState::default()) as SharedAnalytics)
        .setup(|app| {
            // Registered in release too: the Analytics API import logs each
            // step here, and those logs are what make a failed overnight
            // import diagnosable in a shipped build.
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info)
                    .build(),
            )?;
            // Remove any Analytics API downloads orphaned by a crash or a
            // force-quit mid-import, so temp CSVs never accumulate on disk.
            if let Ok(cache_dir) = app.path().app_cache_dir() {
                analytics_api::cleanup_temp(&cache_dir, &[]);
            }
            // Initialize flagged database in app data directory
            if let Ok(data_dir) = app.path().app_data_dir() {
                let flagged_path = data_dir.join("flagged.db");
                let path_str = flagged_path.to_string_lossy().into_owned();
                if let Ok(conn) = open_flagged_db(&path_str) {
                    let state = app.state::<SharedFlaggedDb>();
                    let mut lock = state.lock().expect("flagged db mutex");
                    lock.conn = Some(conn);
                    lock.path = Some(path_str);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            resize_to_available_height,
            get_data,
            open_url,
            open_preview_window,
            select_data_folder,
            check_for_updates,
            get_version,
            save_collection_export,
            set_db_path,
            get_db_path,
            select_csv_files,
            select_db_save_path,
            select_db_open_path,
            import_interactions_csv,
            begin_import_run,
            finalize_import_run,
            compact_database,
            get_sessions,
            export_conversations_for_ai,
            cancel_session_search,
            get_session_interactions,
            get_date_range,
            get_context_options,
            get_db_daily_stats,
            get_db_hour_coverage,
            record_imported_window,
            delete_interactions_by_dates,
            get_analytics_config,
            save_analytics_config,
            test_analytics_connection,
            fetch_analytics_window,
            cleanup_analytics_temp,
            flag_session,
            get_flagged_sessions,
            get_flagged_session_interactions,
            unflag_session,
            save_flagged_note,
            get_flagged_folders,
            create_flagged_folder,
            rename_flagged_folder,
            delete_flagged_folder,
            move_to_flagged_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(DB_SCHEMA).expect("schema");
        conn
    }

    /// One interaction row, covering every column `session_summary` derives from.
    struct Row<'a> {
        log_id: i64,
        session: &'a str,
        ts: &'a str,
        value: &'a str,
        main_type: &'a str,
        all_types: &'a str,
        feedback: &'a str,
        quality: f64,
        recog_type: &'a str,
        contexts: &'a str,
    }

    fn insert_row(conn: &Connection, r: Row) {
        conn.execute(
            "INSERT INTO interactions (log_id, interaction_uuid, session_uuid, timestamp_start, \
             timestamp_end, culture, interaction_value, main_interaction_type, \
             all_interaction_types, feedback_info, recognition_quality, recognition_type, \
             contexts, imported_at) \
             VALUES (?1,'u',?2,?3,?3,'nl',?4,?5,?6,?7,?8,?9,?10,0)",
            params![
                r.log_id, r.session, r.ts, r.value, r.main_type, r.all_types, r.feedback,
                r.quality, r.recog_type, r.contexts
            ],
        )
        .expect("insert interaction");
    }

    /// Every summary row as comparable text. `updated_at` is excluded — it is a
    /// wall-clock stamp, not derived state.
    fn summary_snapshot(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT session_uuid, first_ts, last_ts, interaction_count, culture, \
                 first_user_message, contexts_snapshot, has_real_user_input, has_gen_ai, \
                 has_neg_feedback, has_pos_feedback, min_positive_recognition_quality, \
                 has_zero_recog, last_log_id FROM session_summary ORDER BY session_uuid",
            )
            .expect("prepare snapshot");
        let rows = stmt
            .query_map([], |row| {
                let mut out = String::new();
                for i in 0..14 {
                    let v: rusqlite::types::Value = row.get(i)?;
                    out.push_str(&format!("{v:?}|"));
                }
                Ok(out)
            })
            .expect("snapshot query")
            .collect::<Result<Vec<_>, _>>()
            .expect("snapshot rows");
        rows
    }

    fn mark_touched(conn: &Connection, sessions: &[&str]) {
        for s in sessions {
            conn.execute(
                &format!("INSERT OR IGNORE INTO {TOUCHED_TABLE}(session_uuid) VALUES (?1)"),
                params![s],
            )
            .expect("mark touched");
        }
    }

    /// A varied starting corpus: multiple sessions, each exercising a different
    /// branch of the summary SELECT.
    fn seed_corpus(conn: &Connection) {
        insert_row(conn, Row { log_id: 1, session: "s1", ts: "2026-06-01T09:00:00", value: "hoe laat open", main_type: "Question", all_types: "Question", feedback: "", quality: 0.9, recog_type: "Faq", contexts: r#"[{"name":"park","value":"efteling"}]"# });
        insert_row(conn, Row { log_id: 2, session: "s1", ts: "2026-06-01T09:01:00", value: "continue", main_type: "Event", all_types: "Event", feedback: r#"{"score": 1}"#, quality: 0.0, recog_type: "", contexts: "[]" });
        insert_row(conn, Row { log_id: 3, session: "s2", ts: "2026-06-01T10:00:00", value: "#start#", main_type: "Event", all_types: "Event", feedback: "", quality: 0.0, recog_type: "Faq", contexts: "" });
        insert_row(conn, Row { log_id: 4, session: "s2", ts: "2026-06-01T10:05:00", value: "parkeren", main_type: "GenerativeAI", all_types: "GenerativeAI", feedback: r#"{"score": -1}"#, quality: 0.4, recog_type: "GenerativeAI", contexts: r#"[{"name":"lang","value":"nl"}]"# });
        insert_row(conn, Row { log_id: 5, session: "s3", ts: "2026-06-02T11:00:00", value: "kaartjes", main_type: "Question", all_types: "Question", feedback: "", quality: 0.55, recog_type: "Faq", contexts: r#"[{"name":"park","value":"efteling"}]"# });
    }

    /// The scoped rebuild is only safe if it is indistinguishable from the full
    /// rebuild. This is the property the whole import speed-up rests on: adding
    /// rows to some sessions must leave the summary exactly as a from-scratch
    /// rebuild would, for touched and untouched sessions alike.
    #[test]
    fn scoped_summary_rebuild_matches_a_full_rebuild() {
        let conn = test_conn();
        seed_corpus(&conn);
        rebuild_session_summary(&conn).expect("initial full rebuild");
        let before = summary_snapshot(&conn);
        assert_eq!(before.len(), 3);

        reset_touched_sessions(&conn).expect("touched table");

        // A later import: new rows for an existing session, plus a brand new one.
        insert_row(&conn, Row { log_id: 6, session: "s1", ts: "2026-06-03T08:00:00", value: "en hotel?", main_type: "Question", all_types: "Question,GenerativeAI", feedback: r#"{"score": -1}"#, quality: 0.2, recog_type: "Faq", contexts: r#"[{"name":"stay","value":"hotel"}]"# });
        insert_row(&conn, Row { log_id: 7, session: "s4", ts: "2026-06-03T08:30:00", value: "annuleren", main_type: "Question", all_types: "Question", feedback: "", quality: 0.0, recog_type: "Faq", contexts: "[]" });
        mark_touched(&conn, &["s1", "s4"]);
        rebuild_session_summary_touched(&conn).expect("scoped rebuild");
        let scoped = summary_snapshot(&conn);

        // The full rebuild is the oracle.
        rebuild_session_summary(&conn).expect("full rebuild");
        let full = summary_snapshot(&conn);

        assert_eq!(scoped.len(), 4, "new session must appear");
        assert_eq!(
            scoped, full,
            "scoped rebuild diverged from a full rebuild — import would corrupt session_summary"
        );
        // s2 and s3 were untouched and must be byte-identical to before.
        assert!(before[1..].iter().all(|r| scoped.contains(r)));
    }

    /// End-to-end: run a real portal CSV through the real import and confirm the
    /// summary it leaves behind is exactly what a full rebuild would produce.
    ///
    /// This is the one test that exercises the touched-session collection inside
    /// the batch loop, rather than marking sessions by hand. Skips when the
    /// gitignored sample export isn't present.
    #[test]
    fn a_real_import_leaves_the_same_summary_as_a_full_rebuild() {
        let csv_path =
            std::path::Path::new("../Efteling_EFTELING_nl_InteractionLog_2026-03-25-2.csv");
        if !csv_path.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cai-import-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let db_path = dir.join("t.db");
        let _ = fs::remove_file(&db_path);
        let mut conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(DB_SCHEMA).expect("schema");
        conn.execute_batch(FTS_SCHEMA).expect("fts schema");

        // Retention has to be wide enough that the 2026 sample isn't purged.
        let res = import_csv_into(&mut conn, csv_path.to_str().unwrap(), Some(36500), b'|', true)
            .expect("import succeeds");
        assert!(res.inserted > 0, "sample export produced no rows");

        let after_import = summary_snapshot(&conn);
        rebuild_session_summary(&conn).expect("full rebuild");
        assert_eq!(
            after_import,
            summary_snapshot(&conn),
            "a real import left session_summary different from a full rebuild"
        );

        // Re-importing the same file is a pure duplicate run: nothing inserted,
        // and the summary must still be correct afterwards.
        let again = import_csv_into(&mut conn, csv_path.to_str().unwrap(), Some(36500), b'|', true)
            .expect("re-import succeeds");
        assert_eq!(again.inserted, 0, "re-import should insert nothing");
        assert_eq!(
            again.skipped,
            res.inserted + res.skipped,
            "every row should be skipped the second time"
        );
        assert_eq!(
            summary_snapshot(&conn),
            after_import,
            "a duplicate re-import changed the summary"
        );

        // Third run with rows genuinely missing, so the batch loop actually
        // writes to the touched-session table again. Each import drops and
        // recreates that temp table, so this is also what proves the cached
        // INSERT statement survives the recreate.
        let cut: i64 = conn
            .query_row("SELECT log_id FROM interactions ORDER BY log_id LIMIT 1", [], |r| r.get(0))
            .expect("pick a row");
        conn.execute("DELETE FROM interactions WHERE log_id <= ?1", params![cut])
            .expect("delete a row");
        conn.execute("DELETE FROM interactions_fts WHERE rowid <= ?1", params![cut])
            .expect("delete fts row");
        let refill = import_csv_into(&mut conn, csv_path.to_str().unwrap(), Some(36500), b'|', true)
            .expect("third import succeeds");
        assert!(refill.inserted > 0, "deleted rows should come back");
        let after_refill = summary_snapshot(&conn);
        rebuild_session_summary(&conn).expect("full rebuild");
        assert_eq!(
            after_refill,
            summary_snapshot(&conn),
            "summary diverged after re-inserting deleted rows"
        );

        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Split a portal CSV into `parts` files, each carrying the shared header.
    ///
    /// Used to drive a multi-file import run the way the Analytics API path does,
    /// where each downloaded window arrives as its own temp CSV.
    fn split_csv(src: &std::path::Path, dir: &std::path::Path, parts: usize) -> Vec<PathBuf> {
        let text = fs::read_to_string(src).expect("read sample csv");
        let mut lines = text.lines();
        let header = lines.next().expect("header line");
        let body: Vec<&str> = lines.collect();
        let per = body.len().div_ceil(parts.max(1));
        body.chunks(per.max(1))
            .enumerate()
            .map(|(i, chunk)| {
                let path = dir.join(format!("part-{i}.csv"));
                let mut out = String::with_capacity(header.len() + chunk.len() * 200);
                out.push_str(header);
                for line in chunk {
                    out.push('\n');
                    out.push_str(line);
                }
                fs::write(&path, out).expect("write part");
                path
            })
            .collect()
    }

    /// The property the whole run-scoped finalize rests on: importing N files
    /// with the tail deferred, then finalizing once, must leave `session_summary`
    /// exactly as a from-scratch rebuild would.
    ///
    /// If this ever fails, deferring the tail is corrupting derived state and the
    /// optimisation has to come out — not be papered over with a full rebuild.
    #[test]
    fn a_deferred_import_run_finalized_once_matches_a_full_rebuild() {
        let csv_path =
            std::path::Path::new("../Efteling_EFTELING_nl_InteractionLog_2026-03-25-2.csv");
        if !csv_path.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cai-deferred-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        let db_path = dir.join("t.db");
        let mut conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(DB_SCHEMA).expect("schema");
        conn.execute_batch(FTS_SCHEMA).expect("fts schema");

        let parts = split_csv(csv_path, &dir, 3);
        assert_eq!(parts.len(), 3, "sample export too small to split");

        // One run: reset the touched set once, import every part with the tail
        // deferred, finalize once.
        reset_touched_sessions(&conn).expect("begin run");
        set_meta_flag(&conn, META_PENDING_FINALIZE);
        let mut inserted = 0;
        for p in &parts {
            let r = import_csv_into(&mut conn, p.to_str().unwrap(), Some(36500), b'|', false)
                .expect("deferred import succeeds");
            assert_eq!(r.purged, 0, "a deferred import must not purge");
            inserted += r.inserted;
        }
        assert!(inserted > 0, "sample export produced no rows");
        finalize_import_run_into(&mut conn, Some(36500)).expect("finalize succeeds");

        assert!(
            !meta_flag_set(&conn, META_PENDING_FINALIZE),
            "finalize must clear the crash marker"
        );

        let after_run = summary_snapshot(&conn);
        rebuild_session_summary(&conn).expect("full rebuild");
        assert_eq!(
            after_run,
            summary_snapshot(&conn),
            "a deferred run left session_summary different from a full rebuild"
        );

        // And the deferred path must land exactly the same rows as the
        // self-contained one — the split is about *when* work happens, not what.
        let fts_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM interactions_fts", [], |r| r.get(0))
            .expect("count fts");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM interactions", [], |r| r.get(0))
            .expect("count rows");
        assert_eq!(rows, inserted, "row count should match what was inserted");
        assert_eq!(fts_rows, rows, "every row must be indexed exactly once");

        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The per-run `needs_recog_backfill` probe skips an indexed UPDATE for
    /// every duplicate row. It is only safe if a false result can never hide a
    /// row that genuinely needed filling in.
    ///
    /// Drives both directions: a database with an empty `recognition_details`
    /// must still get it filled, and one without must be left alone.
    #[test]
    fn the_recognition_details_probe_never_skips_a_needed_backfill() {
        let dir = std::env::temp_dir().join(format!("cai-recog-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        let csv = dir.join("in.csv");
        let header = "LogId|InteractionUuid|SessionUuid|TimestampStart|TimestampEnd|Culture|\
                      MainInteractionType|AllInteractionTypes|InteractionValue|OutputText|\
                      RecognitionType|RecognitionQuality|RecognitionDetails|Contexts";
        let row = "1|u1|s1|03/25/2026 09:30:22|03/25/2026 09:30:25|nl|\
                   Question|Question|openingstijden|Wij zijn open|Faq|88|{\"intent\":\"hours\"}|[]";
        fs::write(&csv, format!("{header}\n{row}\n")).expect("write csv");

        let mut conn = test_conn();
        conn.execute_batch(FTS_SCHEMA).expect("fts schema");
        // A pre-existing row for the same log_id, with the details missing —
        // exactly what the backfill exists for.
        conn.execute_batch(
            "INSERT INTO interactions (log_id, interaction_uuid, session_uuid, timestamp_start, \
             recognition_details, imported_at) VALUES (1,'u1','s1','2026-03-25T09:30:22','',0)",
        )
        .expect("seed row");

        let res = import_csv_into(&mut conn, csv.to_str().unwrap(), Some(36500), b'|', true)
            .expect("import");
        assert_eq!(res.inserted, 0, "the row already exists");
        assert_eq!(res.skipped, 1);
        let filled: String = conn
            .query_row(
                "SELECT recognition_details FROM interactions WHERE log_id = 1",
                [],
                |r| r.get(0),
            )
            .expect("read back");
        assert_eq!(
            filled, r#"{"intent":"hours"}"#,
            "the probe skipped a backfill that was genuinely needed"
        );

        // Second run: nothing is empty now, so the probe turns the backfill off.
        // The value must survive untouched rather than being cleared.
        let again = import_csv_into(&mut conn, csv.to_str().unwrap(), Some(36500), b'|', true)
            .expect("re-import");
        assert_eq!(again.skipped, 1);
        let still: String = conn
            .query_row(
                "SELECT recognition_details FROM interactions WHERE log_id = 1",
                [],
                |r| r.get(0),
            )
            .expect("read back");
        assert_eq!(still, r#"{"intent":"hours"}"#);

        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The dead-index migration must not take a live index with it.
    ///
    /// `idx_session_uuid` was only safe to drop because the composite indexes
    /// cover a bare `session_uuid = ?`. If a future change removes those, this
    /// fails rather than quietly turning every session lookup into a scan.
    #[test]
    fn dropping_the_dead_indexes_leaves_session_lookups_indexed() {
        let dir = std::env::temp_dir().join(format!("cai-index-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        let db_path = dir.join("t.db");
        let db_str = db_path.to_str().unwrap();

        // Start from the old schema, so this exercises the migration path an
        // existing database takes rather than just the new CREATE statements.
        {
            let conn = Connection::open(db_str).expect("open");
            conn.execute_batch(DB_SCHEMA).expect("schema");
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_session_uuid ON interactions(session_uuid);\
                 CREATE INDEX IF NOT EXISTS idx_type ON interactions(main_interaction_type);\
                 CREATE INDEX IF NOT EXISTS idx_feedback ON interactions(feedback_info) \
                   WHERE feedback_info IS NOT NULL AND feedback_info != '';",
            )
            .expect("legacy indexes");
        }

        let conn = open_db(db_str).expect("open_db migrates");
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='interactions'")
            .expect("prepare");
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("names");
        for dead in ["idx_session_uuid", "idx_type", "idx_feedback"] {
            assert!(!names.iter().any(|n| n == dead), "{dead} should have been dropped");
        }

        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT log_id FROM interactions WHERE session_uuid = 'x'",
                [],
                |r| r.get(3),
            )
            .expect("explain");
        // Either composite is fine, covering or not — what matters is that it
        // is not a SCAN.
        assert!(
            plan.contains("idx_session_") && !plan.contains("SCAN"),
            "session lookups must stay indexed after the migration, got: {plan}"
        );

        drop(stmt);
        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A contentless FTS table accepts a duplicate rowid silently instead of
    /// raising, so the only thing standing between a re-import and a
    /// double-indexed row is the `Ok(1)` gate in the import loop — and the FTS
    /// insert is wrapped in `let _ =`, which would swallow the error anyway.
    ///
    /// A double-indexed row is not visibly broken; it just quietly inflates
    /// every match. This is the test that would catch it.
    #[test]
    fn a_duplicate_row_is_never_indexed_twice() {
        let csv_path =
            std::path::Path::new("../Efteling_EFTELING_nl_InteractionLog_2026-03-25-2.csv");
        if !csv_path.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("cai-fts-dup-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        let db_path = dir.join("t.db");
        let mut conn = open_db(db_path.to_str().unwrap()).expect("open");

        let first = import_csv_into(&mut conn, csv_path.to_str().unwrap(), Some(36500), b'|', true)
            .expect("import");
        assert!(first.inserted > 0);
        let count_fts = |c: &Connection| -> i64 {
            c.query_row("SELECT COUNT(*) FROM interactions_fts", [], |r| r.get(0))
                .expect("count fts")
        };
        let count_rows = |c: &Connection| -> i64 {
            c.query_row("SELECT COUNT(*) FROM interactions", [], |r| r.get(0))
                .expect("count rows")
        };
        assert_eq!(count_fts(&conn), count_rows(&conn));

        // Pick a term that exists, and record how many rows match it.
        let term = "openingstijden";
        let hits_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM interactions_fts WHERE interactions_fts MATCH ?1",
                params![term],
                |r| r.get(0),
            )
            .expect("match");

        // Re-import the identical file three times.
        for _ in 0..3 {
            let again =
                import_csv_into(&mut conn, csv_path.to_str().unwrap(), Some(36500), b'|', true)
                    .expect("re-import");
            assert_eq!(again.inserted, 0, "a re-import must insert nothing");
        }

        assert_eq!(
            count_fts(&conn),
            count_rows(&conn),
            "re-importing duplicated rows in the search index"
        );
        let hits_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM interactions_fts WHERE interactions_fts MATCH ?1",
                params![term],
                |r| r.get(0),
            )
            .expect("match");
        assert_eq!(hits_after, hits_before, "match count drifted after re-imports");

        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A database written before the contentless migration must be detected,
    /// migrated and reindexed on open — without losing a single row.
    #[test]
    fn a_legacy_standalone_fts_table_is_migrated_and_reindexed_on_open() {
        let dir = std::env::temp_dir().join(format!("cai-fts-mig-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        let db_path = dir.join("t.db");
        let db_str = db_path.to_str().unwrap();

        {
            let conn = Connection::open(db_str).expect("open");
            conn.execute_batch(DB_SCHEMA).expect("schema");
            conn.execute_batch(
                "CREATE VIRTUAL TABLE interactions_fts USING fts5(\
                   interaction_value, output_text, article_ids, dialog_paths, \
                   tokenize = 'unicode61 remove_diacritics 1');",
            )
            .expect("legacy fts");
            seed_corpus(&conn);
            conn.execute_batch(
                "INSERT INTO interactions_fts(rowid, interaction_value, output_text, article_ids, dialog_paths) \
                 SELECT log_id, COALESCE(interaction_value,''), '', '', '' FROM interactions",
            )
            .expect("populate legacy fts");
            // The shadow content table is what the migration exists to remove.
            let has_content: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE name='interactions_fts_content'",
                    [],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            assert!(has_content, "test setup: legacy table should store content");
        }

        let conn = open_db(db_str).expect("open migrates");

        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='interactions_fts'",
                [],
                |r| r.get(0),
            )
            .expect("read ddl");
        assert!(sql.contains(FTS_CONTENTLESS_MARKER), "not migrated: {sql}");
        let has_content: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE name='interactions_fts_content'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(!has_content, "the duplicate content table should be gone");

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM interactions", [], |r| r.get(0))
            .expect("count");
        let indexed: i64 = conn
            .query_row("SELECT COUNT(*) FROM interactions_fts", [], |r| r.get(0))
            .expect("count fts");
        assert_eq!(indexed, rows, "migration must reindex every row");
        // And the reindexed content must actually be searchable.
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM interactions_fts WHERE interactions_fts MATCH 'parkeren'",
                [],
                |r| r.get(0),
            )
            .expect("match");
        assert!(hits > 0, "reindexed rows should be findable");

        // Opening again is a no-op — it must not reindex on every launch.
        drop(conn);
        let conn = open_db(db_str).expect("reopen");
        assert!(!repair_fts_index(&conn), "a migrated database must not reindex again");

        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A run that dies before finalizing must repair itself on the next open.
    ///
    /// Deliberately built in the case `ensure_session_summary` cannot see: rows
    /// added only to sessions it already knows about, below the highest `log_id`
    /// it has recorded. Both of its invariants hold, so without the durable
    /// `pending_finalize` marker nothing would trigger a rebuild and the stale
    /// counts would survive indefinitely.
    #[test]
    fn an_abandoned_run_is_repaired_on_open() {
        let dir = std::env::temp_dir().join(format!("cai-abandoned-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        let db_path = dir.join("t.db");
        let db_str = db_path.to_str().unwrap();

        {
            let conn = open_db(db_str).expect("open");
            seed_corpus(&conn);
            // A later row on an existing session, so the high-water mark sits
            // above the gap the abandoned run will fill.
            insert_row(&conn, Row { log_id: 9, session: "s1", ts: "2026-06-04T09:00:00", value: "tot ziens", main_type: "Question", all_types: "Question", feedback: "", quality: 0.8, recog_type: "Faq", contexts: "" });
            rebuild_session_summary(&conn).expect("baseline rebuild");
        }

        // A run starts, adds a row to an existing session with a log_id below the
        // recorded maximum, then the process dies before finalizing.
        {
            let conn = open_db(db_str).expect("reopen");
            reset_touched_sessions(&conn).expect("begin run");
            set_meta_flag(&conn, META_PENDING_FINALIZE);
            insert_row(&conn, Row { log_id: 6, session: "s1", ts: "2026-06-01T09:02:00", value: "en de prijzen?", main_type: "Question", all_types: "Question", feedback: "", quality: 0.9, recog_type: "Faq", contexts: "" });
            mark_touched(&conn, &["s1"]);
            // No finalize: the connection just goes away.
        }

        // Confirm the hole is real — this is what makes the marker necessary.
        {
            let conn = Connection::open(db_str).expect("raw open");
            let sessions: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT session_uuid) FROM interactions WHERE session_uuid != ''",
                    [], |r| r.get(0),
                )
                .expect("count");
            let summaries: i64 = conn
                .query_row("SELECT COUNT(*) FROM session_summary", [], |r| r.get(0))
                .expect("count");
            let max_log: i64 = conn
                .query_row("SELECT MAX(log_id) FROM interactions", [], |r| r.get(0))
                .expect("max");
            let max_summary: i64 = conn
                .query_row("SELECT MAX(last_log_id) FROM session_summary", [], |r| r.get(0))
                .expect("max");
            assert_eq!(sessions, summaries, "test setup: session counts must match");
            assert_eq!(max_log, max_summary, "test setup: high-water marks must match");
        }

        // Opening the database sees the marker and rebuilds in full.
        let conn = open_db(db_str).expect("open after crash");
        assert!(
            !meta_flag_set(&conn, META_PENDING_FINALIZE),
            "the marker must be cleared once repaired"
        );
        let repaired = summary_snapshot(&conn);
        rebuild_session_summary(&conn).expect("oracle rebuild");
        assert_eq!(
            repaired,
            summary_snapshot(&conn),
            "an abandoned run left session_summary stale after reopening"
        );

        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The same equivalence, against a real database when one is available.
    ///
    /// Synthetic corpora can't cover the shapes real interaction logs contain, so
    /// point `CAI_TEST_DB` at a copy of a real conversations database to check
    /// the scoped rebuild against a full one across every session in it. Skips
    /// silently when the variable is unset, like the portal-CSV regression test.
    #[test]
    fn scoped_rebuild_matches_a_full_rebuild_on_a_real_database() {
        let Ok(path) = std::env::var("CAI_TEST_DB") else {
            return;
        };
        let conn = Connection::open(&path).expect("open test database");
        rebuild_session_summary(&conn).expect("baseline full rebuild");

        // Treat the most recent UTC day as "the day just imported".
        let day: String = conn
            .query_row(
                "SELECT MAX(DATE(timestamp_start)) FROM interactions",
                [],
                |r| r.get(0),
            )
            .expect("latest day");
        reset_touched_sessions(&conn).expect("touched table");
        let touched = conn
            .execute(
                &format!(
                    "INSERT OR IGNORE INTO {TOUCHED_TABLE}(session_uuid) \
                     SELECT DISTINCT session_uuid FROM interactions \
                     WHERE DATE(timestamp_start) = ?1 AND session_uuid != ''"
                ),
                params![day],
            )
            .expect("mark touched");
        assert!(touched > 0, "no sessions on {day}");

        rebuild_session_summary_touched(&conn).expect("scoped rebuild");
        let scoped = summary_snapshot(&conn);
        rebuild_session_summary(&conn).expect("full rebuild");
        let full = summary_snapshot(&conn);
        assert_eq!(
            scoped.len(),
            full.len(),
            "scoped rebuild changed the session count"
        );
        assert_eq!(
            scoped, full,
            "scoped rebuild diverged from a full rebuild on real data ({touched} sessions touched on {day})"
        );
    }

    /// `ensure_session_summary` treats a mismatch in session count or max log_id
    /// as corruption and triggers a full rebuild on open. A scoped rebuild must
    /// therefore leave both invariants intact, or every launch would rebuild.
    #[test]
    fn scoped_rebuild_keeps_the_open_time_consistency_check_satisfied() {
        let conn = test_conn();
        seed_corpus(&conn);
        rebuild_session_summary(&conn).expect("full rebuild");
        reset_touched_sessions(&conn).expect("touched table");

        insert_row(&conn, Row { log_id: 99, session: "s5", ts: "2026-06-04T09:00:00", value: "nieuw", main_type: "Question", all_types: "Question", feedback: "", quality: 0.7, recog_type: "Faq", contexts: "[]" });
        mark_touched(&conn, &["s5"]);
        rebuild_session_summary_touched(&conn).expect("scoped rebuild");

        let sessions: i64 = conn.query_row("SELECT COUNT(DISTINCT session_uuid) FROM interactions WHERE session_uuid != ''", [], |r| r.get(0)).unwrap();
        let summaries: i64 = conn.query_row("SELECT COUNT(*) FROM session_summary", [], |r| r.get(0)).unwrap();
        let max_log: i64 = conn.query_row("SELECT MAX(log_id) FROM interactions", [], |r| r.get(0)).unwrap();
        let max_summary_log: i64 = conn.query_row("SELECT MAX(last_log_id) FROM session_summary", [], |r| r.get(0)).unwrap();
        assert_eq!(sessions, summaries);
        assert_eq!(max_log, max_summary_log);
    }

    /// A purge strips a session's rows entirely; its summary must disappear
    /// rather than linger as a phantom session in search results.
    #[test]
    fn purge_drops_fully_removed_sessions_from_the_summary() {
        let conn = test_conn();
        seed_corpus(&conn);
        rebuild_session_summary(&conn).expect("full rebuild");
        reset_touched_sessions(&conn).expect("touched table");

        // Everything before 2026-06-02 goes: s1 and s2 vanish, s3 survives.
        mark_touched(&conn, &["s1", "s2"]);
        conn.execute("DELETE FROM interactions WHERE timestamp_start < '2026-06-02'", []).unwrap();
        rebuild_session_summary_touched(&conn).expect("scoped rebuild");

        let left: Vec<String> = conn
            .prepare("SELECT session_uuid FROM session_summary ORDER BY 1")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(left, vec!["s3".to_string()]);
        // And it still matches a full rebuild.
        let scoped = summary_snapshot(&conn);
        rebuild_session_summary(&conn).expect("full rebuild");
        assert_eq!(scoped, summary_snapshot(&conn));
    }

    /// Dropping the orphan-context sweep from the import path is only safe if an
    /// import genuinely cannot orphan a context row. It can't: it only ever adds
    /// sessions. A purge can, and the scoped sweep must catch exactly those.
    #[test]
    fn only_deletions_orphan_contexts_and_the_scoped_sweep_catches_them() {
        let conn = test_conn();
        seed_corpus(&conn);
        let add_ctx = |session: &str| {
            conn.execute(
                "INSERT OR IGNORE INTO context_index(name, value, session_uuid) VALUES ('park','efteling',?1)",
                params![session],
            )
            .unwrap();
        };
        for s in ["s1", "s2", "s3"] {
            add_ctx(s);
        }
        reset_touched_sessions(&conn).expect("touched table");

        // An import: new session + new context. Nothing may be swept away.
        insert_row(&conn, Row { log_id: 8, session: "s9", ts: "2026-06-05T09:00:00", value: "x", main_type: "Question", all_types: "Question", feedback: "", quality: 0.5, recog_type: "Faq", contexts: "[]" });
        add_ctx("s9");
        mark_touched(&conn, &["s9"]);
        cleanup_orphan_contexts_touched(&conn);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM context_index", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 4, "an import must never orphan a context row");

        // A purge: s1's rows go, so its context must go with them — and only its.
        mark_touched(&conn, &["s1"]);
        conn.execute("DELETE FROM interactions WHERE session_uuid = 's1'", []).unwrap();
        cleanup_orphan_contexts_touched(&conn);
        let left: Vec<String> = conn
            .prepare("SELECT session_uuid FROM context_index ORDER BY 1")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(left, vec!["s2".to_string(), "s3".to_string(), "s9".to_string()]);
    }

    fn insert_interaction(conn: &Connection, log_id: i64, ts: &str) {
        conn.execute(
            "INSERT INTO interactions (log_id, interaction_uuid, session_uuid, \
             timestamp_start, imported_at) VALUES (?1, 'u', 's', ?2, 0)",
            params![log_id, ts],
        )
        .expect("insert");
    }

    fn record_window(conn: &Connection, start: &str, end: &str) {
        let (day, h0, h1) = window_day_hours(start, end).expect("valid window");
        let mut mask: i64 = 0;
        for h in h0..=h1 {
            mask |= 1 << h;
        }
        conn.execute(
            "INSERT INTO imported_windows(day, hours) VALUES (?1, ?2) \
             ON CONFLICT(day) DO UPDATE SET hours = hours | excluded.hours",
            params![day, mask],
        )
        .expect("record window");
    }

    fn coverage_of(conn: &Connection, day: &str) -> Option<DayCoverage> {
        hour_coverage(conn, None)
            .expect("coverage")
            .into_iter()
            .find(|d| d.date == day)
    }

    /// The bug this table exists for: an hour with genuinely zero interactions
    /// is indistinguishable from an hour that was never requested if coverage
    /// is inferred from row presence alone. Such a day could never reach 24/24,
    /// so it stayed marked "partly imported" and was re-downloaded forever.
    #[test]
    fn an_hour_the_api_answered_with_nothing_still_counts_as_covered() {
        let conn = test_conn();
        // Every hour but 02:00 has traffic — a plausibly quiet night hour.
        for h in 0..24 {
            if h == 2 {
                continue;
            }
            insert_interaction(&conn, 100 + h, &format!("2026-07-24T{h:02}:30:00"));
        }

        // Rows alone: 23 of 24 hours, forever short of complete.
        let before = coverage_of(&conn, "2026-07-24").expect("day present");
        assert_ne!(
            before.hours, ALL_HOURS_MASK,
            "row presence alone cannot know hour 02 was fetched and empty"
        );
        assert_eq!(before.hours & (1 << 2), 0);

        // Now record that the full day was actually requested.
        record_window(&conn, "2026-07-24T00:00:00Z", "2026-07-24T23:59:59Z");

        let after = coverage_of(&conn, "2026-07-24").expect("day present");
        assert_eq!(
            after.hours, ALL_HOURS_MASK,
            "a fetched window covers its hours even where the API had no rows"
        );
        assert_eq!(after.count, 23, "the row count must not be inflated");
    }

    /// A window that returned nothing at all still has to register, or the day
    /// would be invisible to the calendar and re-downloaded on every run.
    #[test]
    fn a_fetched_window_with_zero_rows_still_reports_coverage() {
        let conn = test_conn();
        record_window(&conn, "2026-07-24T00:00:00Z", "2026-07-24T23:59:59Z");
        let day = coverage_of(&conn, "2026-07-24").expect("day present despite no rows");
        assert_eq!(day.hours, ALL_HOURS_MASK);
        assert_eq!(day.count, 0);
    }

    /// Partial windows must stay partial — recording must never round up to a
    /// whole day, or the importer would skip hours it never asked for.
    #[test]
    fn a_partial_window_covers_only_the_hours_it_requested() {
        let conn = test_conn();
        record_window(&conn, "2026-07-24T00:00:00Z", "2026-07-24T11:59:59Z");
        let day = coverage_of(&conn, "2026-07-24").expect("day present");
        for h in 0..12 {
            assert_ne!(day.hours & (1 << h), 0, "hour {h} was requested");
        }
        for h in 12..24 {
            assert_eq!(day.hours & (1 << h), 0, "hour {h} was not requested");
        }
        // A second window unions in rather than replacing.
        record_window(&conn, "2026-07-24T12:00:00Z", "2026-07-24T23:59:59Z");
        assert_eq!(
            coverage_of(&conn, "2026-07-24").unwrap().hours,
            ALL_HOURS_MASK
        );
    }

    /// Windows are per UTC day by construction. A cross-day window would record
    /// against the wrong day, so it is rejected rather than silently truncated.
    #[test]
    fn window_parsing_is_strict_about_staying_inside_one_utc_day() {
        assert_eq!(
            window_day_hours("2026-07-24T00:00:00Z", "2026-07-24T23:59:59Z").unwrap(),
            ("2026-07-24".to_string(), 0, 23)
        );
        // Both timestamp shapes parse_ts accepts must work.
        assert_eq!(
            window_day_hours("2026-07-24T05:00:00.123Z", "2026-07-24T06:59:59Z").unwrap(),
            ("2026-07-24".to_string(), 5, 6)
        );
        assert!(window_day_hours("2026-07-24T22:00:00Z", "2026-07-25T01:00:00Z").is_err());
        assert!(window_day_hours("2026-07-24T10:00:00Z", "2026-07-24T09:00:00Z").is_err());
        assert!(window_day_hours("", "2026-07-24T09:00:00Z").is_err());
        assert!(window_day_hours("nonsense", "also nonsense").is_err());
    }

    /// The retention purge cuts mid-day, so the day it lands in must keep the
    /// hours at or after the cutoff and lose only the ones whose rows went.
    /// Pins the bit arithmetic in `purge_old` — `~` on a bound parameter is
    /// exactly the kind of thing that silently does nothing.
    #[test]
    fn purging_clears_coverage_only_for_the_hours_it_removed() {
        let conn = test_conn();
        for day in ["2026-07-22", "2026-07-23", "2026-07-24"] {
            record_window(
                &conn,
                &format!("{day}T00:00:00Z"),
                &format!("{day}T23:59:59Z"),
            );
        }
        // Cutoff: 2026-07-23T09:00:00 — everything before it is purged.
        let cutoff_day = "2026-07-23";
        let cutoff_hour: u32 = 9;
        conn.execute(
            "DELETE FROM imported_windows WHERE day < ?1",
            params![cutoff_day],
        )
        .unwrap();
        let below_cutoff: i64 = (1 << cutoff_hour) - 1;
        conn.execute(
            "UPDATE imported_windows SET hours = hours & ~?2 WHERE day = ?1",
            params![cutoff_day, below_cutoff],
        )
        .unwrap();

        assert!(
            coverage_of(&conn, "2026-07-22").is_none(),
            "a wholly purged day keeps no coverage"
        );
        let cut = coverage_of(&conn, cutoff_day).expect("the cutoff day survives");
        for h in 0..9 {
            assert_eq!(cut.hours & (1 << h), 0, "hour {h} was purged");
        }
        for h in 9..24 {
            assert_ne!(cut.hours & (1 << h), 0, "hour {h} is still covered");
        }
        assert_eq!(
            coverage_of(&conn, "2026-07-24").unwrap().hours,
            ALL_HOURS_MASK,
            "days after the cutoff are untouched"
        );
    }

    /// Deleting a day must forget it was fetched, or the importer would refuse
    /// to download data the user just deliberately removed.
    #[test]
    fn deleting_a_day_forgets_that_it_was_ever_fetched() {
        let conn = test_conn();
        insert_interaction(&conn, 1, "2026-07-24T10:00:00");
        record_window(&conn, "2026-07-24T00:00:00Z", "2026-07-24T23:59:59Z");
        assert_eq!(
            coverage_of(&conn, "2026-07-24").unwrap().hours,
            ALL_HOURS_MASK
        );

        // Mirrors delete_interactions_by_dates' two statements.
        conn.execute(
            "DELETE FROM interactions WHERE DATE(timestamp_start) = ?1",
            params!["2026-07-24"],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM imported_windows WHERE day IN (?1)",
            params!["2026-07-24"],
        )
        .unwrap();

        assert!(
            coverage_of(&conn, "2026-07-24").is_none(),
            "a deleted day must not linger as covered"
        );
    }

    /// Hour coverage is what tells a partially imported day apart from a
    /// complete one, so the importer doesn't skip the rest of a day just
    /// because a local-time range already pulled in its tail.
    #[test]
    fn hour_coverage_marks_a_partial_day_as_incomplete() {
        let conn = test_conn();
        let insert = |log_id: i64, ts: &str| {
            conn.execute(
                "INSERT INTO interactions (log_id, interaction_uuid, session_uuid, \
                 timestamp_start, imported_at) VALUES (?1, 'u', 's', ?2, 0)",
                params![log_id, ts],
            )
            .expect("insert");
        };
        // Day A: only the 22:00 and 23:00 hours — the tail a UTC+2 local day leaves.
        insert(1, "2026-06-01T22:15:00");
        insert(2, "2026-06-01T23:45:00");
        // Day B: every hour present.
        for h in 0..24 {
            insert(100 + h, &format!("2026-06-02T{h:02}:30:00"));
        }

        let mut stmt = conn
            .prepare(
                "SELECT substr(timestamp_start, 1, 10), \
                        CAST(substr(timestamp_start, 12, 2) AS INTEGER) \
                 FROM interactions WHERE length(timestamp_start) >= 13",
            )
            .unwrap();
        let mut masks: HashMap<String, i64> = HashMap::new();
        for r in stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .flatten()
        {
            *masks.entry(r.0).or_insert(0) |= 1 << r.1;
        }

        const ALL_HOURS: i64 = (1 << 24) - 1;
        let partial = masks["2026-06-01"];
        let full = masks["2026-06-02"];
        assert_ne!(partial, ALL_HOURS, "tail-only day must not look complete");
        assert_eq!(partial, (1 << 22) | (1 << 23));
        assert_eq!(full, ALL_HOURS, "every-hour day must look complete");
        // The 00:00–23:59 chunk over the partial day is not covered.
        assert!((0..24).any(|h| partial & (1 << h) == 0));
    }

    #[test]
    fn parse_ts_normalizes_portal_and_api_timestamps_identically() {
        // The portal CSV and the Analytics API describe the same instant in
        // different formats; both must land in the DB byte-identical, because
        // DATE(timestamp_start) and range comparisons compare these as text.
        let expected = "2026-03-25T09:30:22";
        assert_eq!(parse_ts("03/25/2026 09:30:22"), expected);
        assert_eq!(parse_ts("2026-03-25T09:30:22.605Z"), expected);
        assert_eq!(parse_ts("2026-03-25T09:30:22Z"), expected);
        assert_eq!(parse_ts("2026-03-25T09:30:22"), expected);
        assert_eq!(parse_ts("2026-03-25 09:30:22"), expected);
        assert_eq!(parse_ts("  2026-03-25T09:30:22.605Z  "), expected);
    }

    /// Regression guard for the manual import path against a real portal
    /// export. The interaction-log CSVs are gitignored, so this is a no-op
    /// when one isn't present next to the repo.
    #[test]
    fn real_portal_csv_still_parses_with_the_default_delimiter() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let Some(csv_path) = std::fs::read_dir(&dir).ok().and_then(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .find(|p| {
                    p.to_string_lossy().contains("InteractionLog")
                        && p.extension().map(|e| e == "csv").unwrap_or(false)
                })
        }) else {
            return;
        };

        let file = fs::File::open(&csv_path).expect("open export");
        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(b'|') // the default `import_interactions_csv` uses
            .quoting(true)
            .double_quote(true)
            .flexible(true)
            .from_reader(std::io::BufReader::new(file));

        let headers = rdr.headers().expect("headers").clone();
        // The API client validates the same header shape before importing.
        let header_line = headers.iter().collect::<Vec<_>>().join("|");
        assert_eq!(
            analytics_api::validate_csv_header(&format!("{header_line}\n")).unwrap(),
            '|'
        );

        let col = |name: &str| headers.iter().position(|h| h.eq_ignore_ascii_case(name));
        let c_log_id = col("LogId").expect("LogId column");
        let c_ts = col("TimestampStart").expect("TimestampStart column");

        let mut rows = 0;
        for record in rdr.records().take(500) {
            let record = record.expect("row parses");
            rows += 1;
            record
                .get(c_log_id)
                .unwrap()
                .parse::<i64>()
                .expect("LogId is numeric — a wrong delimiter would break this");
            let ts = parse_ts(record.get(c_ts).unwrap());
            assert_eq!(ts.len(), 19, "unexpected timestamp {ts:?}");
            assert_eq!(&ts[4..5], "-");
            assert_eq!(&ts[10..11], "T");
        }
        assert!(rows > 0, "export contained no rows");
    }

    #[test]
    fn parse_ts_passes_through_unrecognized_input() {
        assert_eq!(parse_ts(""), "");
        assert_eq!(parse_ts("not a timestamp at all"), "not a timestamp at all");
        // Right length, wrong separators — left untouched rather than truncated.
        assert_eq!(parse_ts("2026-03-25X09:30:22"), "2026-03-25X09:30:22");
    }

    /// The zero-allocation rewrite indexes fixed byte offsets, so anything that
    /// isn't exactly the expected shape has to fall through rather than slice
    /// into the middle of a character.
    #[test]
    fn parse_ts_handles_odd_shapes_without_panicking() {
        let cases = [
            // Too short to be either format.
            ("2026-03-25", "2026-03-25"),
            ("03/25/2026", "03/25/2026"),
            // Under 19 bytes is left alone whatever it looks like — the length
            // guard predates this rewrite and callers depend on it.
            ("3/5/2026 09:30:22", "3/5/2026 09:30:22"),
            // Portal shape, not zero-padded, long enough to be parsed — the old
            // split-based path accepted this and the output must not change.
            ("3/5/2026 09:30:22.1234", "2026-3-5T09:30:22.1234"),
            // Sub-second and zone text after the seconds field is preserved on
            // the portal path, exactly as the old splitn(2, ' ') did.
            ("03/25/2026 09:30:22.605", "2026-03-25T09:30:22.605"),
            // ISO with a space separator normalizes to 'T' and truncates to
            // seconds.
            ("2026-03-25 09:30:22.605Z", "2026-03-25T09:30:22"),
            // Four date components is not the portal format.
            ("03/25/20/26 09:30:22", "03/25/20/26 09:30:22"),
            // Non-digits where digits belong.
            ("ab/cd/efgh 09:30:22", "efgh-ab-cdT09:30:22"),
            // Multi-byte input long enough to reach the byte indexing.
            ("日本語のタイムスタンプです", "日本語のタイムスタンプです"),
            ("émoji 🎢 in a long enough string", "émoji 🎢 in a long enough string"),
        ];
        for (input, want) in cases {
            assert_eq!(parse_ts(input), want, "parse_ts({input:?})");
        }
    }

    /// The buffered form must be indistinguishable from the allocating one,
    /// including that it clears whatever the buffer held before.
    #[test]
    fn parse_ts_into_reuses_a_buffer_without_leaking_the_previous_value() {
        let mut buf = String::from("stale contents that must not survive");
        parse_ts_into("03/25/2026 09:30:22", &mut buf);
        assert_eq!(buf, "2026-03-25T09:30:22");
        parse_ts_into("", &mut buf);
        assert_eq!(buf, "");
        parse_ts_into("2026-03-25T09:30:22.605Z", &mut buf);
        assert_eq!(buf, parse_ts("2026-03-25T09:30:22.605Z"));
    }

    #[test]
    fn session_summary_rebuild_materializes_search_flags() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO interactions (
                log_id, interaction_uuid, session_uuid, timestamp_start, timestamp_end,
                culture, main_interaction_type, all_interaction_types, interaction_value,
                output_text, article_ids, dialog_paths, feedback_info, recognition_type,
                recognition_quality, contexts, imported_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                1i64,
                "iu-1",
                "session-a",
                "2026-01-01T10:00:00",
                "2026-01-01T10:00:01",
                "nl-NL",
                "Question",
                "",
                "waar is de Python?",
                "",
                "",
                "",
                "",
                "",
                0.0f64,
                "",
                1i64
            ],
        )
        .expect("insert user");
        conn.execute(
            "INSERT INTO interactions (
                log_id, interaction_uuid, session_uuid, timestamp_start, timestamp_end,
                culture, main_interaction_type, all_interaction_types, interaction_value,
                output_text, article_ids, dialog_paths, feedback_info, recognition_type,
                recognition_quality, contexts, imported_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                2i64,
                "iu-2",
                "session-a",
                "2026-01-01T10:00:02",
                "2026-01-01T10:00:03",
                "nl-NL",
                "GenerativeAI",
                "Dialog,GenerativeAI",
                "",
                "antwoord",
                "",
                "",
                "{\"score\": -1}",
                "Faq",
                42.0f64,
                "[{\"name\":\"channel\",\"value\":\"app\"}]",
                1i64
            ],
        )
        .expect("insert bot");

        rebuild_session_summary(&conn).expect("summary rebuild");

        let row: (i64, i64, i64, String, String, i64) = conn
            .query_row(
                "SELECT has_real_user_input, has_gen_ai, has_neg_feedback, first_user_message, contexts_snapshot, interaction_count FROM session_summary WHERE session_uuid = 'session-a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .expect("summary row");
        assert_eq!(row.0, 1);
        assert_eq!(row.1, 1);
        assert_eq!(row.2, 1);
        assert_eq!(row.3, "waar is de Python?");
        assert!(row.4.contains("channel"));
        assert_eq!(row.5, 2);
    }

    #[test]
    fn ensure_session_summary_repairs_stale_cache() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO interactions (
                log_id, interaction_uuid, session_uuid, timestamp_start,
                interaction_value, output_text, imported_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                1i64,
                "iu-1",
                "session-b",
                "2026-01-01T11:00:00",
                "hoi",
                "",
                1i64
            ],
        )
        .expect("insert");

        ensure_session_summary(&conn).expect("ensure summary");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_summary", [], |r| r.get(0))
            .expect("summary count");
        assert_eq!(count, 1);
    }

    #[test]
    fn feedback_targets_use_origin_and_previous_bot_fallback() {
        let rows = vec![
            serde_json::json!({
                "logId": 1,
                "interactionUuid": "user-1",
                "interactionType": "Question",
                "userText": "What time does the park close?",
                "botOutput": "",
                "feedbackInfoRaw": "",
            }),
            serde_json::json!({
                "logId": 2,
                "interactionUuid": "bot-1",
                "interactionType": "Answer",
                "userText": "",
                "botOutput": "The park closes at 18:00.",
                "articleIds": ["qa-1"],
                "dialogPaths": null,
                "articles": { "qa": [{ "articleId": 1, "categories": [{ "name": "noise" }] }] },
                "recognitionDetails": {
                    "entityMatches": [
                        { "entityId": 7, "displayName": "OPENINGSTIJD", "name": "OPENINGSTIJD_1", "match": "time" }
                    ],
                    "missingWords": "noise"
                },
                "feedbackInfoRaw": "",
            }),
            serde_json::json!({
                "logId": 3,
                "interactionUuid": "feedback-1",
                "interactionType": "Feedback",
                "userText": "",
                "botOutput": "",
                "feedbackInfo": { "score": -1, "originatingInteractionId": "bot-1" },
                "feedbackInfoRaw": "{\"score\":-1,\"originatingInteractionId\":\"bot-1\"}",
            }),
            serde_json::json!({
                "logId": 4,
                "interactionUuid": "feedback-2",
                "interactionType": "Feedback",
                "userText": "",
                "botOutput": "",
                "feedbackInfo": { "score": 1 },
                "feedbackInfoRaw": "{\"score\":1}",
            }),
        ];

        let targets = build_feedback_targets(&rows);

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0]["targetLogId"], 2);
        assert_eq!(targets[0]["targetResolution"], "originatingInteractionId");
        assert_eq!(
            targets[0]["targetUserQuestion"],
            "What time does the park close?"
        );
        assert_eq!(
            targets[0]["targetTriggeredContent"]["articles"][0]["id"],
            "1"
        );
        assert_eq!(
            targets[0]["targetEntityMatches"][0]["display_name"],
            "OPENINGSTIJD"
        );
        assert_eq!(targets[1]["targetLogId"], 2);
        assert_eq!(targets[1]["targetResolution"], "previousBotOutputFallback");
    }

    #[test]
    fn export_name_is_the_search_term() {
        assert_eq!(
            suggested_ai_export_name("openingstijden"),
            "openingstijden.jsonl"
        );
        assert_eq!(
            suggested_ai_export_name("Waar is de ingang?"),
            "waar-is-de-ingang.jsonl"
        );
        // No search term → the static default.
        assert_eq!(
            suggested_ai_export_name(""),
            "conversation-analysis-export.jsonl"
        );
        // A term of nothing but punctuation slugs to empty, so it falls back too.
        assert_eq!(
            suggested_ai_export_name("\"???\""),
            "conversation-analysis-export.jsonl"
        );
    }

    /// The whole point of the format work: assert the bytes that actually land on
    /// disk, not just the helpers that build them.
    #[test]
    fn exported_jsonl_puts_constants_in_a_header_and_never_repeats_them() {
        let conn = test_conn();
        let insert = |log_id: i64, session: &str, ts: &str, value: &str, output: &str,
                      main_type: &str, feedback: &str| {
            conn.execute(
                "INSERT INTO interactions (log_id, interaction_uuid, session_uuid, \
                 timestamp_start, timestamp_end, culture, interaction_value, output_text, \
                 main_interaction_type, all_interaction_types, feedback_info, \
                 recognition_quality, recognition_type, contexts, imported_at) \
                 VALUES (?1,?2,?3,?4,?4,'nl',?5,?6,?7,?7,?8,88.0,'Entity Recognition','',0)",
                params![
                    log_id,
                    format!("uuid-{log_id}"),
                    session,
                    ts,
                    value,
                    output,
                    main_type,
                    feedback
                ],
            )
            .expect("insert interaction");
        };
        insert(1, "s-a", "2026-03-25T09:30:22", "Openingstijden?", "Open van 10:00.", "QA", "");
        insert(
            2,
            "s-a",
            "2026-03-25T09:31:00",
            "",
            "",
            "Feedback",
            r#"{"score":-1,"comment":"onduidelijk","originatingInteractionId":"uuid-1"}"#,
        );
        insert(3, "s-b", "2026-03-26T11:00:00", "Waar is de ingang", "Bij de hoofdpoort.", "QA", "");

        let sessions = vec![
            serde_json::json!({
                "sessionUuid": "s-b", "firstTs": "2026-03-26T11:00:00",
                "lastTs": "2026-03-26T11:00:00", "interactionCount": 1,
                "culture": "nl", "firstUserMessage": "Waar is de ingang",
            }),
            serde_json::json!({
                "sessionUuid": "s-a", "firstTs": "2026-03-25T09:30:22",
                "lastTs": "2026-03-25T09:31:00", "interactionCount": 2,
                "culture": "nl", "firstUserMessage": "Openingstijden?",
            }),
        ];
        let search_context = serde_json::json!({ "query": "ingang", "lowRecogThreshold": 60 });

        let mut buf: Vec<u8> = Vec::new();
        let counts =
            write_ai_export(&conn, &sessions, &search_context, &mut buf).expect("write export");
        assert_eq!(counts.interaction_count, 3);
        assert_eq!(counts.feedback_count, 1);

        let text = String::from_utf8(buf).expect("utf8");
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).expect("each line is valid JSON"))
            .collect();
        assert_eq!(lines.len(), 3, "one header plus one line per session");

        assert_eq!(lines[0]["record_type"], "export_header");
        assert_eq!(lines[0]["session_count"], 2);
        assert_eq!(lines[0]["search_context"]["query"], "ingang");

        for (i, line) in lines[1..].iter().enumerate() {
            assert_eq!(line["record_type"], "session");
            assert_eq!(line["session_index"], i, "index must be contiguous from 0");
            // The whole reason the header exists.
            for constant in ["schema_version", "exported_at", "search_context", "legend"] {
                assert!(
                    line.get(constant).is_none(),
                    "session line {i} still repeats {constant}"
                );
            }
            for key in ["first_ts", "last_ts"] {
                assert!(
                    line["session"][key].as_str().is_some_and(|t| t.ends_with('Z')),
                    "session {i} {key} is not marked UTC"
                );
            }
            for turn in line["chat_trace"].as_array().expect("chat_trace") {
                assert_ne!(
                    turn["is_feedback_target"], false,
                    "false is never written, only omitted"
                );
                assert!(turn["turn_kind"].is_string() || turn.get("turn_kind").is_none());
                assert!(turn.get("role").is_none(), "role was renamed to turn_kind");
            }
        }

        // Session s-a: the negative feedback is joined to the answer it rated.
        let s_a = &lines[2];
        assert_eq!(s_a["session"]["session_uuid"], "s-a");
        assert_eq!(s_a["session"]["first_ts"], "2026-03-25T09:30:22Z");
        let target = &s_a["feedback_targets"][0];
        assert_eq!(target["feedback"]["score"], -1);
        assert_eq!(target["feedback"]["comment"], "onduidelijk");
        assert_eq!(target["target"]["answer_text"], "Open van 10:00.");
        // How certain the feedback→answer join is, recorded alongside the feedback.
        assert_eq!(
            target["feedback"]["target_resolution"],
            "originatingInteractionId"
        );
        assert_eq!(s_a["chat_trace"][0]["is_feedback_target"], true);
        assert_eq!(s_a["chat_trace"][0]["turn_kind"], "user_and_bot");
    }

    #[test]
    fn header_carries_the_constant_fields_and_documents_the_rest() {
        let search_context = serde_json::json!({ "query": "openingstijden", "lowRecogThreshold": 60 });
        let header = ai_export_header("2026-07-25T10:00:00Z", &search_context, 42);

        assert_eq!(header["record_type"], "export_header");
        assert_eq!(header["schema_version"], 4);
        // The count is what lets a reader detect a truncated file.
        assert_eq!(header["session_count"], 42);
        assert_eq!(header["search_context"]["query"], "openingstijden");
        // Every field a reader would otherwise have to guess at is explained once.
        for key in [
            "format",
            "completeness",
            "timestamps",
            "turn_kind",
            "recognition_quality",
            "triggered_content",
            "feedback_targets",
            "is_feedback_target",
            "conventions",
        ] {
            assert!(
                header["legend"][key].as_str().is_some_and(|s| !s.is_empty()),
                "legend is missing {key}"
            );
        }
        // The header survives pruning intact.
        assert_eq!(
            prune_empty_json(header.clone()).as_ref(),
            Some(&header),
            "header must not be pruned"
        );
    }

    #[test]
    fn turn_kind_never_pretends_a_combined_row_is_a_chat_role() {
        assert_eq!(
            turn_kind_for_interaction("QA", "Openingstijden?", "Wij zijn open van 10:00."),
            "user_and_bot"
        );
        assert_eq!(turn_kind_for_interaction("QA", "Openingstijden?", ""), "user_only");
        assert_eq!(turn_kind_for_interaction("QA", "", "Welkom!"), "bot_only");
        assert_eq!(turn_kind_for_interaction("Feedback", "", ""), "feedback");
        assert_eq!(turn_kind_for_interaction("QA", "", ""), "system");
    }

    #[test]
    fn exported_timestamps_are_explicitly_utc() {
        assert_eq!(utc_iso("2026-03-25T09:30:22"), "2026-03-25T09:30:22Z");
        // Already-marked values are not double-suffixed.
        assert_eq!(utc_iso("2026-03-25T09:30:22Z"), "2026-03-25T09:30:22Z");
        assert!(utc_iso("").is_null());
        // now_iso already marks itself, so exported_at needs no fixing up.
        assert!(now_iso().ends_with('Z'));
    }

    #[test]
    fn html_stripping_keeps_single_underscores() {
        // `__` is a real separator artifact in CAI answers...
        assert_eq!(strip_html_text("shops__please"), "shops please");
        // ...but a lone underscore is part of the text and must survive.
        assert_eq!(strip_html_text("Use ENTITY_NAME here"), "Use ENTITY_NAME here");
    }

    #[test]
    fn filename_slug_is_safe_and_bounded() {
        assert_eq!(filename_slug("Opening hours?", 60), "opening-hours");
        assert_eq!(filename_slug("a/b\\c:*?\"<>|", 60), "a-b-c");
        assert_eq!(filename_slug("   ", 60), "");
        // Truncation never splits a multi-byte char.
        let long = filename_slug(&"é".repeat(50), 10);
        assert!(long.len() <= 11 && long.chars().all(|c| c == 'é'));
    }

    #[test]
    fn compact_turn_keeps_fix_signals_and_drops_noisy_raw_fields() {
        let row = serde_json::json!({
            "logId": 2,
            "turnKind": "user_and_bot",
            "interactionType": "QA",
            "userText": "Where can I buy a souvenir?",
            "botOutput": "Go to <a href=\"https://example.com\">shops</a>__please.",
            "recognitionType": "Entity Recognition",
            "recognitionQuality": 88.0,
            "articleIds": ["qa-42", "dn-12-34"],
            "dialogPaths": { "DropOut": "12:34" },
            "articles": {
                "qa": [{ "articleId": 42, "categories": [{ "name": "noise" }] }],
                "dialog": [{
                    "dialogId": 12,
                    "dialogName": "Retail",
                    "nodeId": 34,
                    "nodeName": "Souvenirs",
                    "dialogStatus": "End",
                    "nodeType": "Output",
                    "categories": [{ "name": "noise" }]
                }]
            },
            "recognitionDetails": {
                "entityMatches": [
                    { "entityId": 5, "displayName": "SOUVENIR", "name": "SOUVENIR_1", "match": "souvenir" }
                ],
                "missingWords": "noise"
            },
            "contexts": [{ "name": "noise", "value": "noise" }],
            "pages": { "originatingPage": "https://example.com" },
            "faqsFound": { "noise": true }
        });

        let compact = compact_turn(&row, true);

        assert_eq!(compact["answer_text"], "Go to shops please.");
        assert_eq!(compact["turn_kind"], "user_and_bot");
        assert_eq!(compact["is_feedback_target"], true);
        // ...and omitted entirely on the turns that did not get feedback.
        assert!(compact_turn(&row, false).get("is_feedback_target").is_none());
        assert_eq!(compact["triggered_content"]["articles"][0]["id"], "42");
        let dialogs = compact["triggered_content"]["dialogs"]
            .as_array()
            .expect("dialogs array");
        assert!(dialogs
            .iter()
            .any(|d| d.get("dialog_name").and_then(|v| v.as_str()) == Some("Retail")));
        assert_eq!(compact["entity_matches"][0]["matched_text"], "souvenir");
        assert!(compact.get("contexts").is_none());
        assert!(compact.get("pages").is_none());
        assert!(compact.get("faqsFound").is_none());
        assert!(compact.get("articles").is_none());
        assert!(compact.get("recognitionDetails").is_none());
    }
}


/// Performance harnesses. All `#[ignore]`d — they take seconds and measure
/// rather than assert, except where a correctness check rides along.
///
///   cargo test --release perf:: -- --nocapture --ignored
#[cfg(test)]
mod perf {
    use super::*;

    /// Not an assertion — a measurement harness. Run with:
    ///   cargo test --release perf::import_cost -- --nocapture --ignored
    ///
    /// Seeds a database of realistic size first, because the per-file tail
    /// (FTS 'optimize', PRAGMA optimize, purge scans) costs the size of the
    /// *database*, not the size of the import — which is exactly why paying it
    /// once per downloaded window is the problem.
    #[test]
    #[ignore]
    fn import_cost() {
        const BASELINE_ROWS: i64 = 120_000;
        const WINDOWS: usize = 90;
        const ROWS_PER_WINDOW: i64 = 1_500;

        let dir = std::env::temp_dir().join("cai-bench");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);

        let header = "LogId|InteractionUuid|SessionUuid|TimestampStart|TimestampEnd|Culture|\
                      MainInteractionType|AllInteractionTypes|InteractionValue|OutputText|\
                      ArticleIds|DialogPaths|RecognitionType|RecognitionQuality|\
                      RecognitionDetails|Contexts|FeedbackInfo";
        let row = |id: i64| -> String {
            let sess = id / 9;
            let day = 1 + (id % 28);
            format!(
                "{id}|u{id}|s{sess}|03/{day:02}/2026 09:30:22|03/{day:02}/2026 09:30:25|nl|\
                 Question|Question|wat zijn de openingstijden van het park {id}|\
                 Het park is open van 10 tot 18 uur, zie de website voor uitzonderingen {id}|\
                 12{id}|dn-4/node-9|Faq|88|{{\"intent\":\"hours\",\"score\":0.88}}|\
                 [{{\"name\":\"lang\",\"value\":\"nl\"}},{{\"name\":\"park\",\"value\":\"efteling\"}}]|"
            )
            .replace("\n", "")
        };
        let write_csv = |path: &PathBuf, from: i64, count: i64| {
            let mut s = String::with_capacity((count as usize) * 260 + header.len());
            s.push_str(header);
            for id in from..from + count {
                s.push('\n');
                s.push_str(&row(id));
            }
            fs::write(path, s).expect("write csv");
        };

        let baseline = dir.join("baseline.csv");
        write_csv(&baseline, 0, BASELINE_ROWS);
        let windows: Vec<PathBuf> = (0..WINDOWS)
            .map(|w| {
                let p = dir.join(format!("w{w}.csv"));
                write_csv(&p, BASELINE_ROWS + (w as i64) * ROWS_PER_WINDOW, ROWS_PER_WINDOW);
                p
            })
            .collect();

        println!(
            "baseline {BASELINE_ROWS} rows, then {WINDOWS} windows x {ROWS_PER_WINDOW} rows\n"
        );

        for tail_per_file in [true, false] {
            let db_path = dir.join(if tail_per_file { "old.db" } else { "new.db" });
            let _ = fs::remove_file(&db_path);
            let mut conn = open_db(db_path.to_str().unwrap()).expect("open");
            import_csv_into(&mut conn, baseline.to_str().unwrap(), Some(36500), b'|', true)
                .expect("baseline import");

            let t0 = Instant::now();
            let mut rows_ms = 0u64;
            let mut tail_ms = 0u64;
            if !tail_per_file {
                reset_touched_sessions(&conn).expect("begin run");
                let _ = conn.query_row("PRAGMA wal_autocheckpoint = 20000", [], |_| Ok(()));
            }
            for p in &windows {
                let r = import_csv_into(
                    &mut conn, p.to_str().unwrap(), Some(36500), b'|', tail_per_file,
                )
                .expect("import");
                assert_eq!(r.inserted, ROWS_PER_WINDOW, "window should insert every row");
                rows_ms += r.timings.rows_ms;
                tail_ms += r.timings.total_ms - r.timings.rows_ms;
            }
            if !tail_per_file {
                let f = finalize_import_run_into(&mut conn, Some(36500)).expect("finalize");
                tail_ms += f.timings.total_ms;
            }
            println!(
                "{:>22}: total {:>6}ms   rows {:>6}ms   tail {:>6}ms",
                if tail_per_file { "tail per file" } else { "tail once per run" },
                t0.elapsed().as_millis(), rows_ms, tail_ms
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    // ── FTS shape: contentless vs the old content-storing table ──

    const STANDALONE: &str = "CREATE VIRTUAL TABLE interactions_fts USING fts5(\
        interaction_value, output_text, article_ids, dialog_paths, \
        tokenize = 'unicode61 remove_diacritics 1');";
    const CONTENTLESS: &str = "CREATE VIRTUAL TABLE interactions_fts USING fts5(\
        interaction_value, output_text, article_ids, dialog_paths, \
        content = '', contentless_delete = 1, \
        tokenize = 'unicode61 remove_diacritics 1');";

    const WORDS: [&str; 24] = [
        "openingstijden", "parkeren", "kaartjes", "hotel", "efteling", "attractie",
        "wachttijd", "korting", "restaurant", "plattegrond", "openingsdatum", "jaarkaart",
        "annuleren", "betalen", "adres", "route", "kinderen", "baby", "rolstoel",
        "honden", "weer", "regen", "storing", "onderhoud",
    ];

    /// Deterministic word picker — no rand dependency, and reproducible runs.
    fn sentence(seed: &mut u64, n: usize) -> String {
        let mut s = String::with_capacity(n * 10);
        for i in 0..n {
            *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            if i > 0 {
                s.push(' ');
            }
            s.push_str(WORDS[(*seed >> 33) as usize % WORDS.len()]);
        }
        s
    }

    /// The load-bearing shape of the real conversation search: MATCH on the FTS
    /// table, joined back to interactions on rowid, collapsed per session.
    const SEARCH_SQL: &str = "SELECT session_uuid, MIN(log_id) FROM (\
         SELECT i.session_uuid AS session_uuid, i.log_id AS log_id \
         FROM interactions_fts JOIN interactions i ON i.log_id = interactions_fts.rowid \
         WHERE interactions_fts MATCH ?1) GROUP BY session_uuid";

    fn time_searches(conn: &Connection, label: &str) {
        let queries = [
            ("common term", "openingstijden"),
            ("rare-ish term", "rolstoel"),
            ("AND of two", "hotel AND korting"),
            ("phrase", "\"parkeren kaartjes\""),
            ("prefix", "openings*"),
            ("column filter", "{output_text article_ids dialog_paths} : storing"),
        ];
        for (name, q) in queries {
            // Warm the cache, then take the best of 5 — run-to-run noise on a
            // laptop is larger than the effect being measured.
            let mut best = u128::MAX;
            let mut hits = 0i64;
            for _ in 0..5 {
                let t = Instant::now();
                let mut stmt = conn.prepare(SEARCH_SQL).expect("prepare");
                let n = stmt
                    .query_map(params![q], |r| r.get::<_, String>(0))
                    .expect("query")
                    .count() as i64;
                best = best.min(t.elapsed().as_micros());
                hits = n;
            }
            println!("  {label:<12} {name:<14} {:>8}us  ({hits} sessions)", best);
        }
    }

    /// Run with:
    ///   cargo test --release perf::contentless_vs_standalone -- --nocapture --ignored
    ///
    /// Answers two questions with the SQLite the app actually ships: does
    /// dropping the duplicate content copy make imports cheaper, and does it
    /// make search slower? The second is the gate — search must not regress.
    #[test]
    #[ignore]
    fn contentless_vs_standalone() {
        const ROWS: i64 = 200_000;
        let dir = std::env::temp_dir().join("cai-fts-bench");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);

        println!("\n{ROWS} rows, bundled SQLite {}\n", rusqlite::version());

        for (label, schema) in [("standalone", STANDALONE), ("contentless", CONTENTLESS)] {
            let path = dir.join(format!("{label}.db"));
            let mut conn = Connection::open(&path).expect("open");
            conn.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-65536;",
            )
            .expect("pragmas");
            conn.execute_batch(
                "CREATE TABLE interactions (\
                   log_id INTEGER PRIMARY KEY, interaction_uuid TEXT, session_uuid TEXT, \
                   timestamp_start TEXT, interaction_value TEXT, output_text TEXT, \
                   article_ids TEXT, dialog_paths TEXT, imported_at INTEGER DEFAULT 0);\
                 CREATE INDEX idx_timestamp ON interactions(timestamp_start);\
                 CREATE INDEX idx_session_ts ON interactions(session_uuid, timestamp_start);\
                 CREATE INDEX idx_session_log ON interactions(session_uuid, log_id);",
            )
            .expect("schema");
            conn.execute_batch(schema).expect("fts schema");

            let mut seed = 7u64;
            let t0 = Instant::now();
            {
                let tx = conn.transaction().expect("tx");
                {
                    let mut ins = tx
                        .prepare(
                            "INSERT INTO interactions(log_id, interaction_uuid, session_uuid, \
                             timestamp_start, interaction_value, output_text, article_ids, dialog_paths) \
                             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                        )
                        .expect("prepare ins");
                    let mut fts = tx
                        .prepare(
                            "INSERT INTO interactions_fts(rowid, interaction_value, output_text, \
                             article_ids, dialog_paths) VALUES (?1,?2,?3,?4,?5)",
                        )
                        .expect("prepare fts");
                    for i in 0..ROWS {
                        let value = sentence(&mut seed, 8);
                        let output = sentence(&mut seed, 40);
                        let arts = format!("12{}", i % 5000);
                        ins.execute(params![
                            i, format!("u{i}"), format!("s{}", i / 9),
                            format!("2026-{:02}-{:02}T09:30:22", 1 + i % 12, 1 + i % 28),
                            value, output, arts, "dn-4/node-9"
                        ])
                        .expect("insert");
                        fts.execute(params![i, value, output, arts, "dn-4/node-9"])
                            .expect("fts insert");
                    }
                }
                tx.commit().expect("commit");
            }
            let insert_s = t0.elapsed().as_secs_f64();

            let t0 = Instant::now();
            conn.execute_batch("INSERT INTO interactions_fts(interactions_fts) VALUES('optimize');")
                .expect("optimize");
            let optimize_s = t0.elapsed().as_secs_f64();

            let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
            let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            println!(
                "{label:<12}: insert {insert_s:6.2}s   optimize {optimize_s:5.2}s   file {:7.1} MB",
                bytes as f64 / 1e6
            );
            time_searches(&conn, label);

            // Now the tombstone case: delete a chunk without merging, which is
            // the one way contentless could plausibly slow search down.
            conn.execute_batch(
                "DELETE FROM interactions_fts WHERE rowid < 40000;\
                 DELETE FROM interactions WHERE log_id < 40000;",
            )
            .expect("delete");
            println!("  -- after deleting 20% of rows, before any merge --");
            time_searches(&conn, label);
            println!();
        }
        let _ = fs::remove_dir_all(&dir);
    }

    // ── the whole thing, end to end ──

    /// End-to-end shape of the user's actual complaint: a database that already
    /// holds data, then a multi-day import through the real command path.
    ///
    ///   cargo test --release perf::whole_run -- --nocapture --ignored
    #[test]
    #[ignore]
    fn whole_run() {
        const BASELINE: i64 = 150_000;
        const DAYS: usize = 60;
        const PER_DAY: i64 = 2_000;

        let dir = std::env::temp_dir().join("cai-e2e-bench");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);

        let header = "LogId|InteractionUuid|SessionUuid|TimestampStart|TimestampEnd|Culture|\
MainInteractionType|AllInteractionTypes|InteractionValue|OutputText|ArticleIds|DialogPaths|\
RecognitionType|RecognitionQuality|RecognitionDetails|Contexts|FeedbackInfo";
        let make = |path: &PathBuf, from: i64, count: i64| {
            let mut s = String::with_capacity(count as usize * 300);
            s.push_str(header);
            for id in from..from + count {
                s.push('\n');
                s.push_str(&format!(
                    "{id}|u{id}|s{}|03/{:02}/2026 09:30:22|03/{:02}/2026 09:30:25|nl|\
Question|Question|wat zijn de openingstijden van het park {id}|\
Het park is open van 10 tot 18 uur, zie de website voor uitzonderingen {id}|12{}|dn-4/node-9|\
Faq|88|{{\"intent\":\"hours\"}}|[{{\"name\":\"lang\",\"value\":\"nl\"}},{{\"name\":\"park\",\"value\":\"efteling\"}}]|",
                    id / 9, 1 + id % 28, 1 + id % 28, id % 5000
                ));
            }
            fs::write(path, s).expect("write");
        };

        let base_csv = dir.join("base.csv");
        make(&base_csv, 0, BASELINE);
        let days: Vec<PathBuf> = (0..DAYS)
            .map(|d| {
                let p = dir.join(format!("d{d}.csv"));
                make(&p, BASELINE + d as i64 * PER_DAY, PER_DAY);
                p
            })
            .collect();

        let db = dir.join("t.db");
        let mut conn = open_db(db.to_str().unwrap()).expect("open");
        import_csv_into(&mut conn, base_csv.to_str().unwrap(), Some(36500), b'|', true)
            .expect("baseline");
        let base_bytes = fs::metadata(&db).map(|m| m.len()).unwrap_or(0);

        let t0 = Instant::now();
        reset_touched_sessions(&conn).expect("begin");
        set_meta_flag(&conn, META_PENDING_FINALIZE);
        let _ = conn.query_row("PRAGMA wal_autocheckpoint = 20000", [], |_| Ok(()));
        let mut inserted = 0i64;
        for p in &days {
            inserted += import_csv_into(&mut conn, p.to_str().unwrap(), Some(36500), b'|', false)
                .expect("import")
                .inserted;
        }
        let f = finalize_import_run_into(&mut conn, Some(36500)).expect("finalize");
        let total = t0.elapsed();

        println!(
            "\n{BASELINE} existing rows + {DAYS} days x {PER_DAY} rows ({inserted} imported)\n\
             wall clock {:.2}s   finalize {}ms   baseline file {:.1} MB\n",
            total.as_secs_f64(),
            f.timings.total_ms,
            base_bytes as f64 / 1e6
        );

        // The whole point: correctness is unchanged.
        let after = {
            let mut stmt = conn
                .prepare("SELECT session_uuid, interaction_count, last_log_id FROM session_summary ORDER BY session_uuid")
                .expect("prepare");
            let v: Vec<String> = stmt
                .query_map([], |r| {
                    Ok(format!("{}|{}|{}", r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
                })
                .expect("q").collect::<Result<_, _>>().expect("rows");
            v
        };
        rebuild_session_summary(&conn).expect("oracle");
        let oracle = {
            let mut stmt = conn
                .prepare("SELECT session_uuid, interaction_count, last_log_id FROM session_summary ORDER BY session_uuid")
                .expect("prepare");
            let v: Vec<String> = stmt
                .query_map([], |r| {
                    Ok(format!("{}|{}|{}", r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
                })
                .expect("q").collect::<Result<_, _>>().expect("rows");
            v
        };
        assert_eq!(after, oracle, "the fast path produced a different summary");
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM interactions", [], |r| r.get(0)).unwrap();
        let fts: i64 = conn.query_row("SELECT COUNT(*) FROM interactions_fts", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, BASELINE + inserted);
        assert_eq!(fts, rows, "search index out of step with the data");
        println!("verified: {rows} rows, {fts} indexed, summary matches a full rebuild\n");

        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod fts_semantics {
    use super::*;

    /// repair_fts_index's staleness check and both delete paths must behave the
    /// same on a contentless table as on the old standalone one. Verified here
    /// against the bundled SQLite rather than assumed from documentation.
    #[test]
    fn contentless_supports_count_delete_and_column_filters() {
        let conn = Connection::open_in_memory().expect("mem db");
        conn.execute_batch(
            "CREATE VIRTUAL TABLE f USING fts5(a, b, content='', contentless_delete=1);",
        )
        .expect("create");
        for i in 1..=5i64 {
            conn.execute(
                "INSERT INTO f(rowid, a, b) VALUES (?1, ?2, ?3)",
                params![i, format!("alpha{i}"), "shared beta"],
            )
            .expect("insert");
        }
        // COUNT(*) must report the indexed row count — repair_fts_index compares
        // it against COUNT(*) FROM interactions to detect a stale index.
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM f", [], |r| r.get(0)).expect("count");
        assert_eq!(n, 5, "COUNT(*) must work on a contentless table");

        // Plain DELETE by rowid — what purge_old and delete_interactions_by_dates do.
        conn.execute("DELETE FROM f WHERE rowid IN (1,2)", []).expect("delete");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM f", [], |r| r.get(0)).expect("count");
        assert_eq!(n, 3);
        let hits: i64 = conn
            .query_row("SELECT COUNT(*) FROM f WHERE f MATCH 'beta'", [], |r| r.get(0))
            .expect("match");
        assert_eq!(hits, 3, "deleted rows must stop matching");

        // Column-filtered MATCH, the syntax get_sessions builds for user/bot scoping.
        let hits: i64 = conn
            .query_row("SELECT COUNT(*) FROM f WHERE f MATCH '{b} : beta'", [], |r| r.get(0))
            .expect("column filter");
        assert_eq!(hits, 3);
        let hits: i64 = conn
            .query_row("SELECT COUNT(*) FROM f WHERE f MATCH '{a} : beta'", [], |r| r.get(0))
            .expect("column filter");
        assert_eq!(hits, 0, "column filter must still scope to one column");

        // The full DELETE-then-reinsert repair must work (only the 'rebuild'
        // command is unavailable on a contentless table, and we don't use it).
        conn.execute_batch("DELETE FROM f;").expect("clear");
        conn.execute("INSERT INTO f(rowid, a, b) VALUES (9, 'x', 'beta')", [])
            .expect("reinsert");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM f", [], |r| r.get(0)).expect("count");
        assert_eq!(n, 1);
    }
}
