use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::types::ToSql;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, Manager, State};

const WATCH_EVENT_NAME: &str = "data-folder-updated";

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
CREATE INDEX IF NOT EXISTS idx_session_uuid  ON interactions(session_uuid);
CREATE INDEX IF NOT EXISTS idx_timestamp     ON interactions(timestamp_start);
CREATE INDEX IF NOT EXISTS idx_type          ON interactions(main_interaction_type);
CREATE INDEX IF NOT EXISTS idx_session_ts    ON interactions(session_uuid, timestamp_start);
CREATE INDEX IF NOT EXISTS idx_session_log   ON interactions(session_uuid, log_id);
CREATE INDEX IF NOT EXISTS idx_feedback      ON interactions(feedback_info) WHERE feedback_info IS NOT NULL AND feedback_info != '';
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
"#;

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
const FTS_SCHEMA: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS interactions_fts USING fts5(
    interaction_value,
    output_text,
    article_ids,
    dialog_paths,
    tokenize = 'unicode61 remove_diacritics 1'
);
"#;

fn rebuild_session_summary(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
DELETE FROM session_summary;
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
WHERE s.session_uuid IS NOT NULL AND s.session_uuid != ''
GROUP BY s.session_uuid;
"#,
    )
    .map_err(|e| format!("Session summary rebuild error: {e}"))
}

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

fn repair_fts_index(conn: &Connection) {
    if conn.execute_batch(FTS_SCHEMA).is_err() {
        return;
    }
    let interaction_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM interactions", [], |r| r.get(0))
        .unwrap_or(0);
    let fts_row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM interactions_fts", [], |r| r.get(0))
        .unwrap_or(-1);
    if interaction_count != fts_row_count {
        let _ = conn.execute_batch(
            "DELETE FROM interactions_fts; \
             INSERT INTO interactions_fts(rowid, interaction_value, output_text, article_ids, dialog_paths) \
             SELECT log_id, COALESCE(interaction_value,''), COALESCE(output_text,''), \
                    COALESCE(article_ids,''), COALESCE(dialog_paths,'') \
             FROM interactions",
        );
    }
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
    ensure_session_summary(&conn)?;
    // One-time bounded ANALYZE so the query planner has statistics for the
    // session_summary/context_index indexes; "PRAGMA optimize" after imports
    // keeps them fresh.
    let has_stats = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sqlite_stat1'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_stats {
        let _ = conn.query_row("PRAGMA analysis_limit = 1000", [], |_| Ok(()));
        let _ = conn.execute_batch("ANALYZE;");
    }
    Ok(conn)
}

/// Convert MM/DD/YYYY HH:MM:SS to ISO-8601 (YYYY-MM-DDTHH:MM:SS)
fn parse_ts(s: &str) -> String {
    // expected: "03/25/2026 09:30:22"
    let s = s.trim();
    if s.len() >= 19 {
        let parts: Vec<&str> = s.splitn(2, ' ').collect();
        if parts.len() == 2 {
            let date_parts: Vec<&str> = parts[0].split('/').collect();
            if date_parts.len() == 3 {
                return format!(
                    "{}-{}-{}T{}",
                    date_parts[2], date_parts[0], date_parts[1], parts[1]
                );
            }
        }
    }
    s.to_string()
}

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
    if deleted > 0 {
        cleanup_orphan_contexts(conn);
        let _ = rebuild_session_summary(conn);
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

#[tauri::command]
async fn import_interactions_csv(
    db_state: State<'_, SharedDbState>,
    file_path: String,
    max_age_days: Option<i64>,
) -> Result<ImportResult, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
    let mut state = db.lock().map_err(|e| e.to_string())?;
    let conn = state.conn.as_mut().ok_or("No database open. Set a database path first.")?;

    let file = fs::File::open(&file_path).map_err(|e| format!("Cannot open CSV: {e}"))?;
    let buf = std::io::BufReader::with_capacity(4 * 1024 * 1024, file);

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(b'|')
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

    let mut inserted: i64 = 0;
    let mut skipped: i64 = 0;
    let mut errors: Vec<String> = Vec::new();
    let mut batch: Vec<csv::StringRecord> = Vec::with_capacity(1000);

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
        for record in batch {
            let get_r = |idx: Option<usize>| -> &str {
                idx.and_then(|i| record.get(i)).unwrap_or("")
            };
            let log_id_str = get_r(c_log_id);
            let log_id: i64 = match log_id_str.parse() {
                Ok(v) => v,
                Err(_) => { *skipped += 1; continue; }
            };
            let ts_start = parse_ts(get_r(c_ts_start));
            let quality: f64 = get_r(c_recog_quality).parse().unwrap_or(0.0);
            let result = ins_stmt.execute(
                params![
                    log_id,
                    get_r(c_uuid),
                    get_r(c_session),
                    ts_start,
                    parse_ts(get_r(c_ts_end)),
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
                    // Index context (name, value) pairs for fast context-filter lookups
                    let ctx_str = get_r(c_contexts);
                    if !ctx_str.is_empty() && ctx_str != "[]" && ctx_str != "null" {
                        let session_id = get_r(c_session);
                        if let Ok(arr) = serde_json::from_str::<serde_json::Value>(ctx_str) {
                            if let Some(items) = arr.as_array() {
                                for item in items {
                                    let name  = item.get("name") .and_then(|v| v.as_str()).unwrap_or("");
                                    let value = item.get("value").and_then(|v| v.as_str()).unwrap_or("");
                                    if !name.is_empty() {
                                        let _ = ctx_stmt.execute(params![name, value, session_id]);
                                    }
                                }
                            }
                        }
                    }
                    *inserted += 1;
                }
                Ok(_) => {
                    // Row already exists — backfill recognition_details if it was NULL
                    let rd = get_r(c_recog_details);
                    if !rd.is_empty() {
                        let _ = backfill_stmt.execute(params![rd, log_id]);
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
        let _ = tx.commit();
    };

    for result in rdr.records() {
        match result {
            Ok(record) => {
                batch.push(record);
                if batch.len() >= 1000 {
                    flush_batch(conn, &batch, now_secs,
                        c_log_id, c_uuid, c_session, c_ts_start, c_ts_end, c_culture,
                        c_main_type, c_all_types, c_value, c_output, c_article_ids,
                        c_dialog_paths, c_tdialog_status, c_recog_type, c_recog_quality,
                        c_recog_details,
                        c_genai, c_articles, c_faqs, c_contexts, c_pages, c_link_click,
                        c_feedback, c_output_meta,
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
            &mut inserted, &mut skipped, &mut errors);
    }

    let purged = purge_old(conn, max_age_days.unwrap_or(90).max(1) as u64);
    if inserted > 0 || purged > 0 {
        cleanup_orphan_contexts(conn);
        rebuild_session_summary(conn)?;
        // Merge FTS5 b-tree segments so MATCH queries read fewer pages, and
        // refresh planner statistics for the tables this import touched.
        let _ = conn.execute_batch(
            "INSERT INTO interactions_fts(interactions_fts) VALUES('optimize');",
        );
        let _ = conn.execute_batch("PRAGMA optimize;");
    }

    Ok(ImportResult { inserted, skipped, purged, errors })
    })
    .await
    .map_err(|e| e.to_string())?
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
    out.replace("__", " ")
        .replace("_", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
        "role",
        row.get("role").cloned().unwrap_or(serde_json::Value::Null),
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
    map.insert(
        "is_feedback_target".to_string(),
        serde_json::Value::Bool(is_feedback_target),
    );
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

fn role_for_interaction(interaction_type: &str, user_text: &str, bot_output: &str) -> &'static str {
    if interaction_type == "Feedback" {
        "feedback"
    } else if !user_text.trim().is_empty() && bot_output.trim().is_empty() {
        "user"
    } else if user_text.trim().is_empty() && !bot_output.trim().is_empty() {
        "assistant"
    } else if !user_text.trim().is_empty() || !bot_output.trim().is_empty() {
        "turn"
    } else {
        "system"
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

    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();
    app.dialog()
        .file()
        .add_filter("JSONL", &["jsonl"])
        .set_file_name("conversation-analysis-export.jsonl")
        .save_file(move |path| {
            let p = path.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(p);
        });

    let Some(mut jsonl_path) = rx.await.ok().flatten() else {
        return Ok(ConversationAiExportResult {
            ok: false,
            canceled: true,
            jsonl_path: None,
            session_count: 0,
            feedback_count: 0,
            interaction_count: 0,
        });
    };
    if jsonl_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        jsonl_path.set_extension("jsonl");
    }
    let jsonl_path_for_work = jsonl_path.clone();
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let exported_at = now_iso();
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

        let mut out = fs::File::create(&jsonl_path_for_work)
            .map_err(|e| format!("Cannot create export file: {e}"))?;
        let mut interaction_total = 0i64;
        let mut feedback_total = 0i64;

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
        for session in &sessions {
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
                        "role": role_for_interaction(&interaction_type, &user_text, &bot_output),
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
                "schema_version": 3,
                "exported_at": exported_at.clone(),
                "search_context": {
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
                    "resolvedSearchMode": filter_query.search_mode.clone(),
                },
                "session": {
                    "session_uuid": session.get("sessionUuid").cloned().unwrap_or(serde_json::Value::Null),
                    "first_ts": session.get("firstTs").cloned().unwrap_or(serde_json::Value::Null),
                    "last_ts": session.get("lastTs").cloned().unwrap_or(serde_json::Value::Null),
                    "culture": session.get("culture").cloned().unwrap_or(serde_json::Value::Null),
                    "feedback_count": feedback_count,
                },
                "feedback_targets": compact_feedback_targets,
                "chat_trace": chat_trace(),
            });
            let record = prune_empty_json(record).unwrap_or(serde_json::Value::Null);
            serde_json::to_writer(&mut out, &record)
                .map_err(|e| format!("Cannot write export JSON: {e}"))?;
            writeln!(&mut out).map_err(|e| format!("Cannot write export file: {e}"))?;
        }

        Ok(ConversationAiExportResult {
            ok: true,
            canceled: false,
            jsonl_path: Some(jsonl_path_for_work.to_string_lossy().into_owned()),
            session_count: sessions.len() as i64,
            feedback_count: feedback_total,
            interaction_count: interaction_total,
        })
    })
    .await
    .map_err(|e| e.to_string())?
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

        let mut stmt = conn
            .prepare(
                "SELECT DATE(timestamp_start) AS day, COUNT(*) AS cnt \
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
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
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
            get_sessions,
            export_conversations_for_ai,
            cancel_session_search,
            get_session_interactions,
            get_date_range,
            get_context_options,
            get_db_daily_stats,
            delete_interactions_by_dates,
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
    fn compact_turn_keeps_fix_signals_and_drops_noisy_raw_fields() {
        let row = serde_json::json!({
            "logId": 2,
            "role": "assistant",
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
        assert_eq!(compact["is_feedback_target"], true);
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
