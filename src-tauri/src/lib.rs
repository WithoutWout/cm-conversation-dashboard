use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
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
        Self { conn: None, path: None }
    }
}

type SharedDbState = Arc<Mutex<DbState>>;

// ── Flagged DB state ─────────────────────────────────────────────────────────

struct FlaggedDbState {
    conn: Option<Connection>,
    path: Option<String>,
}

impl Default for FlaggedDbState {
    fn default() -> Self {
        Self { conn: None, path: None }
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

#[derive(Deserialize)]
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

fn extract_dialogs(content: &str) -> (serde_json::Value, serde_json::Value, serde_json::Value, serde_json::Value) {
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

    if watcher.watch(&watch_folder, RecursiveMode::NonRecursive).is_ok() {
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
            let (loaded_dialogs, loaded_t_dialogs, loaded_conv_vars, loaded_ctx_vars) = extract_dialogs(&content);
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
            let filename = source_paths
                .get(definition.key)
                .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()));
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
fn open_url(app: tauri::AppHandle, url: String) {
    use tauri_plugin_opener::OpenerExt;
    if url.starts_with("https://") || url.starts_with("http://") || url.starts_with("tel:") {
        let _ = app.opener().open_url(url, None::<String>);
    }
}

#[tauri::command]
fn open_preview_window(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("Invalid URL: only http/https allowed".to_string());
    }
    let parsed: tauri::Url = url.parse().map_err(|e: <tauri::Url as std::str::FromStr>::Err| e.to_string())?;
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
CREATE INDEX IF NOT EXISTS idx_feedback      ON interactions(feedback_info) WHERE feedback_info IS NOT NULL AND feedback_info != '';
CREATE INDEX IF NOT EXISTS idx_recog_quality ON interactions(recognition_quality) WHERE recognition_quality > 0;
CREATE TABLE IF NOT EXISTS context_index (
    name         TEXT NOT NULL,
    value        TEXT NOT NULL,
    session_uuid TEXT NOT NULL,
    PRIMARY KEY (name, value, session_uuid)
);
CREATE INDEX IF NOT EXISTS idx_ctx_session ON context_index(session_uuid);
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
    folder_id         INTEGER REFERENCES flagged_folders(folder_id) ON DELETE SET NULL
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

fn open_flagged_db(path: &str) -> Result<Connection, String> {
    if let Some(parent) = Path::new(path).parent() {
        let _ = fs::create_dir_all(parent);
    }
    let conn = Connection::open(path).map_err(|e| format!("Cannot open flagged DB: {e}"))?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .map_err(|e| format!("PRAGMA error: {e}"))?;
    conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")
        .map_err(|e| format!("PRAGMA error: {e}"))?;
    conn.execute_batch(FLAGGED_DB_SCHEMA)
        .map_err(|e| format!("Schema error: {e}"))?;
    // Migrations for existing DBs (ignore errors if column already exists)
    let _ = conn.execute_batch("ALTER TABLE flagged_sessions ADD COLUMN folder_id INTEGER REFERENCES flagged_folders(folder_id) ON DELETE SET NULL");
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
        let in_year: u64 = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) { 366 } else { 365 };
        if rem < in_year { break; }
        rem -= in_year;
        year += 1;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days: [u64; 12] = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    for &d in &month_days {
        if rem < d { break; }
        rem -= d;
        month += 1;
    }
    let day = rem + 1;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, h, m, s)
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

fn open_db(path: &str) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("Cannot open DB: {e}"))?;
    // PRAGMA journal_mode returns a result row, so it must be run via query_row.
    // PRAGMA synchronous is a pure setter and works via execute_batch.
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .map_err(|e| format!("PRAGMA error: {e}"))?;
    conn.execute_batch("PRAGMA synchronous=NORMAL;")
        .map_err(|e| format!("PRAGMA error: {e}"))?;
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
        ).ok();
    }

    // Optional: FTS5 virtual table — failure here must never prevent the DB from opening.
    if conn.execute_batch(FTS_SCHEMA).is_ok() {
        // Populate FTS5 index if it has no rows yet (first open after upgrade, or fresh DB).
        // We count rows in the FTS table itself, not in sqlite_master, because the shadow
        // tables are always created together with the virtual table — checking sqlite_master
        // would always return 1 and the bulk-insert would never run.
        let fts_row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM interactions_fts",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if fts_row_count == 0 {
            conn.execute_batch(
                "INSERT INTO interactions_fts(rowid, interaction_value, output_text, article_ids, dialog_paths) \
                 SELECT log_id, COALESCE(interaction_value,''), COALESCE(output_text,''), \
                        COALESCE(article_ids,''), COALESCE(dialog_paths,'') \
                 FROM interactions",
            )
            .ok();
        }
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

fn purge_old(conn: &Connection) -> i64 {
    // Keep 12 days of data
    let cutoff = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // 12 days in seconds
        secs.saturating_sub(12 * 24 * 3600)
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
            let days_in_year = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) { 366 } else { 365 };
            if rem_days < days_in_year { break; }
            rem_days -= days_in_year;
            year += 1;
        }
        let month_days: [u32; 12] = [31, if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut month = 1u32;
        for &d in &month_days {
            if rem_days < d { break; }
            rem_days -= d;
            month += 1;
        }
        let day = rem_days + 1;
        let _ = t; // suppress unused
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", year, month, day, hrs, mins, s_secs)
    };
    // Remove stale FTS5 entries before deleting from interactions
    let _ = conn.execute(
        "DELETE FROM interactions_fts WHERE rowid IN \
         (SELECT log_id FROM interactions WHERE timestamp_start < ?1)",
        params![cutoff_dt],
    );
    conn.execute(
        "DELETE FROM interactions WHERE timestamp_start < ?1",
        params![cutoff_dt],
    )
    .unwrap_or(0) as i64
}

// ── Conversation Tauri commands ───────────────────────────────────────────────

#[tauri::command]
async fn set_db_path(
    db_state: State<'_, SharedDbState>,
    path: String,
) -> Result<(), String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&path)?;
        let mut state = db.lock().map_err(|e| e.to_string())?;
        state.conn = Some(conn);
        state.path = Some(path);
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
        Ok(paths) if !paths.is_empty() => FileDialogResult { ok: true, canceled: false, paths },
        _ => FileDialogResult { ok: false, canceled: true, paths: vec![] },
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
        None => FileSaveResult { ok: false, canceled: true, path: None },
    }
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
        None => FileSaveResult { ok: false, canceled: true, path: None },
    }
}

#[tauri::command]
async fn import_interactions_csv(
    db_state: State<'_, SharedDbState>,
    file_path: String,
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
            let result = tx.execute(
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
                    let _ = tx.execute(
                        "INSERT INTO interactions_fts(rowid, interaction_value, output_text, article_ids, dialog_paths) \
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            log_id,
                            get_r(c_value),
                            get_r(c_output),
                            get_r(c_article_ids),
                            get_r(c_dialog_paths),
                        ],
                    );
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
                                        let _ = tx.execute(
                                            "INSERT OR IGNORE INTO context_index(name, value, session_uuid) VALUES (?1, ?2, ?3)",
                                            params![name, value, session_id],
                                        );
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
                        let _ = tx.execute(
                            "UPDATE interactions SET recognition_details = ?1 WHERE log_id = ?2 AND (recognition_details IS NULL OR recognition_details = '')",
                            params![rd, log_id],
                        );
                    }
                    *skipped += 1
                }
                Err(e) => errors.push(format!("Row {log_id}: {e}")),
            }
        }
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

    let purged = purge_old(conn);

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
    query_regex: Option<bool>,   // treat query as a regex
    query_scope: Option<String>, // "both" | "user" | "bot"
    query_ids: Option<bool>,     // also search article_ids and dialog_paths columns
    query_ids_only: Option<bool>, // search ONLY article_ids and dialog_paths, not message text
    query_id_type: Option<String>, // "article" | "dialog" | "node" — which ID column/pattern to use
    low_recog_threshold: Option<i64>, // threshold for "low recognition" filter (default 60, range 1–99)
    context_filters: Option<Vec<ContextFilter>>, // [{name, value}] filter by context values
}

#[tauri::command]
async fn get_sessions(
    db_state: State<'_, SharedDbState>,
    args: GetSessionsArgs,
) -> Result<SessionsPage, String> {
    let db = db_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
    let state = db.lock().map_err(|e| e.to_string())?;
    let conn = state.conn.as_ref().ok_or("No database open.")?;

    let page = args.page.unwrap_or(1).max(1);
    let limit = 50i64;
    let offset = (page - 1) * limit;

    let filter = args.filter.as_deref().unwrap_or("all");
    let query = args.query.as_deref().unwrap_or("").trim().to_string();
    let query_regex = args.query_regex.unwrap_or(false);
    let query_scope = args.query_scope.as_deref().unwrap_or("both").to_string();
    let query_ids = args.query_ids.unwrap_or(false);
    let query_ids_only = args.query_ids_only.unwrap_or(false);
    let query_id_type = args.query_id_type.as_deref().unwrap_or("article").to_string();
    let low_recog_threshold = args.low_recog_threshold.unwrap_or(60).clamp(1, 99);

    // Register a custom REGEXP function for this connection when regex mode is on
    if query_regex && !query.is_empty() {
        use regex::Regex;
        use std::sync::Arc;
        let compiled = Arc::new(
            Regex::new(&query).map_err(|e| format!("Invalid regex: {e}"))?
        );
        conn.create_scalar_function(
            "regexp",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8 | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            move |ctx: &rusqlite::functions::Context<'_>| {
                let text: String = ctx.get(1).unwrap_or_default();
                Ok(compiled.is_match(&text) as i32)
            },
        ).ok();
    }

    // Collect parameterized values alongside conditions
    let mut conditions: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 0usize;

    // Helper to get next parameter placeholder
    let next_param = |idx: &mut usize| -> String {
        *idx += 1;
        format!("?{}", *idx)
    };

    // Always exclude sessions that have no real user input
    conditions.push(
        "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions \
         WHERE interaction_value != '' \
           AND interaction_value NOT LIKE '#%#' \
           AND LOWER(interaction_value) != 'continue' \
           AND main_interaction_type NOT IN ('Event', 'LinkClick'))".to_string(),
    );
    if filter == "genai" {
        conditions.push(
            "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE main_interaction_type = 'GenerativeAI')".to_string(),
        );
    } else if filter == "neg_feedback" {
        conditions.push(
            "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE feedback_info LIKE '%\"score\": -1%' OR feedback_info LIKE '%\"score\":-1%')".to_string(),
        );
    } else if filter == "pos_feedback" {
        conditions.push(
            "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE (feedback_info LIKE '%\"score\": 1%' OR feedback_info LIKE '%\"score\":1%') AND feedback_info NOT LIKE '%\"score\": -1%' AND feedback_info NOT LIKE '%\"score\":-1%')".to_string(),
        );
    } else if filter == "low_recog" {
        conditions.push(
            format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE recognition_quality > 0 AND recognition_quality < {low_recog_threshold} AND main_interaction_type != 'GenerativeAI' AND (recognition_type IS NULL OR recognition_type != 'GenerativeAI'))")
        );
    } else if filter == "zero_recog" {
        conditions.push(
            "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE recognition_quality = 0 AND recognition_type IS NOT NULL AND recognition_type != '' AND recognition_type != 'GenerativeAI' AND main_interaction_type != 'GenerativeAI')".to_string(),
        );
    }
    if let Some(ref df) = args.date_from {
        if !df.is_empty() {
            let p = next_param(&mut param_idx);
            conditions.push(format!("timestamp_start >= {p}"));
            param_values.push(Box::new(df.clone()));
        }
    }
    if let Some(ref dt) = args.date_to {
        if !dt.is_empty() {
            let p = next_param(&mut param_idx);
            conditions.push(format!("timestamp_start <= {p}"));
            param_values.push(Box::new(dt.clone()));
        }
    }
    if !query.is_empty() {
        if query_ids_only {
            // ID-type mode: search using the type-specific pattern
            let cond = match query_id_type.as_str() {
                "dialog" => {
                    // Dialog ID appears as `"<id>:` in the dialog_paths JSON value.
                    // Pattern: dialog_paths LIKE '%"<id>:%'
                    let like_val = format!("%\"{}:%", query.replace('%', "\\%").replace('_', "\\_"));
                    let p = next_param(&mut param_idx);
                    param_values.push(Box::new(like_val));
                    format!(
                        "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions \
                         WHERE dialog_paths LIKE {p} ESCAPE '\\')"
                    )
                }
                "node" => {
                    // Node IDs are NOT globally unique — the same node.id can appear in
                    // multiple dialogs. The interaction log uses the composite format
                    // `dn-{dialogId}-{nodeId}` in article_ids (e.g. `dn-6391-6`).
                    // The query is expected to be the composite "{dialogId}-{nodeId}" string
                    // so we can search for the exact dn-{dialogId}-{nodeId} token.
                    let escaped = query.replace('%', "\\%").replace('_', "\\_");
                    let like_val = format!("%dn-{}%", escaped);
                    let p = next_param(&mut param_idx);
                    param_values.push(Box::new(like_val));
                    format!(
                        "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions \
                         WHERE article_ids LIKE {p} ESCAPE '\\')"
                    )
                }
                _ => {
                    // "article" (default): article ID stored as `qa-<id>` in article_ids.
                    let like_val = format!("%qa-{}%", query.replace('%', "\\%").replace('_', "\\_"));
                    let p = next_param(&mut param_idx);
                    param_values.push(Box::new(like_val));
                    format!(
                        "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions \
                         WHERE article_ids LIKE {p} ESCAPE '\\')"
                    )
                }
            };
            conditions.push(cond);
        } else if query_regex {
            // Regex mode: use the registered REGEXP function
            let p = next_param(&mut param_idx);
            let text_cond = match query_scope.as_str() {
                "user" => format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE regexp({p}, interaction_value))"),
                "bot"  => format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE regexp({p}, output_text))"),
                _      => format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE regexp({p}, interaction_value) OR regexp({p}, output_text))"),
            };
            param_values.push(Box::new(query.clone()));
            let cond = if query_ids {
                let p2 = next_param(&mut param_idx);
                param_values.push(Box::new(query.clone()));
                let ids_subq = format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE regexp({p2}, article_ids) OR regexp({p2}, dialog_paths))");
                format!("({text_cond} OR {ids_subq})")
            } else {
                text_cond
            };
            conditions.push(cond);
        } else {
            // Tokenize a query segment into terms, respecting "quoted phrases" as single tokens.
            // `"de Efteling" attractie` → ["de Efteling", "attractie"]
            fn tokenize_segment(s: &str) -> Vec<String> {
                let mut tokens = Vec::new();
                let mut chars = s.chars().peekable();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() {
                        chars.next();
                    } else if c == '"' {
                        chars.next(); // consume opening quote
                        let phrase: String = chars.by_ref().take_while(|&ch| ch != '"').collect();
                        if !phrase.is_empty() {
                            tokens.push(phrase);
                        }
                    } else {
                        let word: String = chars.by_ref().take_while(|&ch| !ch.is_whitespace() && ch != '"').collect();
                        if !word.is_empty() {
                            tokens.push(word);
                        }
                    }
                }
                tokens
            }

            // Plain text mode: try FTS5 full-text search; fall back to LIKE if unavailable.
            let fts_available: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='interactions_fts'",
                    [],
                    |_| Ok(true),
                )
                .unwrap_or(false);

            if fts_available {
                // Parse OR groups: split by '|', each group's tokens become FTS5 AND terms.
                // Quoted phrases ("de Efteling") are kept as single FTS5 PHRASE tokens.
                let or_groups: Vec<Vec<String>> = query
                    .split('|')
                    .map(|g| {
                        tokenize_segment(g.trim())
                            .into_iter()
                            .filter_map(|t| {
                                // If the token contains spaces it's a quoted phrase → use FTS5 phrase syntax: "word1 word2"
                                if t.contains(' ') {
                                    let phrase_terms: Vec<String> = t.split_whitespace()
                                        .map(|w| {
                                            w.chars()
                                                .filter(|c| c.is_alphanumeric() || matches!(*c, '-' | '_' | '.'))
                                                .collect::<String>()
                                        })
                                        .filter(|w| !w.is_empty())
                                        .collect();
                                    if phrase_terms.is_empty() { None } else { Some(format!("\"{}\"", phrase_terms.join(" "))) }
                                } else {
                                    let clean: String = t
                                        .chars()
                                        .filter(|c| c.is_alphanumeric() || matches!(*c, '-' | '_' | '.'))
                                        .collect();
                                    if clean.is_empty() { None } else { Some(format!("{}*", clean)) }
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .filter(|g| !g.is_empty())
                    .collect();

                if !or_groups.is_empty() {
                    let fts_inner = or_groups
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

                    let fts_query = fts_inner;
                    let fts_match_expr = match (query_scope.as_str(), query_ids) {
                        ("user", false) => format!("interaction_value : {fts_query}"),
                        ("user", true)  => format!("{{interaction_value article_ids dialog_paths}} : {fts_query}"),
                        ("bot",  false) => format!("output_text : {fts_query}"),
                        ("bot",  true)  => format!("{{output_text article_ids dialog_paths}} : {fts_query}"),
                        (_,      _)     => fts_query,
                    };
                    let p = next_param(&mut param_idx);
                    param_values.push(Box::new(fts_match_expr));
                    conditions.push(format!(
                        "session_uuid IN (\
                            SELECT DISTINCT i.session_uuid FROM interactions i \
                            WHERE i.log_id IN (SELECT rowid FROM interactions_fts WHERE interactions_fts MATCH {p})\
                        )"
                    ));
                }
            } else {
                // FTS5 not available — fall back to LIKE per AND-term, OR between groups.
                // Quoted phrases are kept as single literal terms for LIKE matching.
                let or_groups: Vec<Vec<String>> = query
                    .split('|')
                    .map(|g| tokenize_segment(g.trim()).into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>())
                    .filter(|g| !g.is_empty())
                    .collect();

                if !or_groups.is_empty() {
                    // Build one subquery per AND-group; join with UNION (= SQL OR on session_uuid).
                    let or_subqueries: Vec<String> = or_groups
                        .iter()
                        .map(|and_terms| {
                            // Each AND term becomes a LIKE condition on the scope-selected columns.
                            let and_clauses: Vec<String> = and_terms
                                .iter()
                                .map(|term| {
                                    let like_val = format!("%{}%", term.replace('%', "\\%").replace('_', "\\_"));
                                    let text_filters = match query_scope.as_str() {
                                        "user" => {
                                            let p = next_param(&mut param_idx);
                                            param_values.push(Box::new(like_val.clone()));
                                            format!("interaction_value LIKE {p} ESCAPE '\\'")
                                        }
                                        "bot" => {
                                            let p = next_param(&mut param_idx);
                                            param_values.push(Box::new(like_val.clone()));
                                            format!("output_text LIKE {p} ESCAPE '\\'")
                                        }
                                        _ => {
                                            let p1 = next_param(&mut param_idx);
                                            param_values.push(Box::new(like_val.clone()));
                                            let p2 = next_param(&mut param_idx);
                                            param_values.push(Box::new(like_val.clone()));
                                            format!("(interaction_value LIKE {p1} ESCAPE '\\' OR output_text LIKE {p2} ESCAPE '\\')")
                                        }
                                    };
                                    if query_ids {
                                        let pi1 = next_param(&mut param_idx);
                                        param_values.push(Box::new(like_val.clone()));
                                        let pi2 = next_param(&mut param_idx);
                                        param_values.push(Box::new(like_val.clone()));
                                        format!("({text_filters} OR article_ids LIKE {pi1} ESCAPE '\\' OR dialog_paths LIKE {pi2} ESCAPE '\\')")
                                    } else {
                                        text_filters
                                    }
                                })
                                .collect();
                            // All AND terms must match in this group — intersect via nested subquery.
                            // Simplest: emit as a subquery with multiple WHERE AND conditions.
                            let where_clause = and_clauses.join(" AND ");
                            format!("SELECT DISTINCT session_uuid FROM interactions WHERE {where_clause}")
                        })
                        .collect();

                    // UNION of all OR-group subqueries
                    let union_sql = or_subqueries.join(" UNION ");
                    conditions.push(format!(
                        "session_uuid IN ({union_sql})"
                    ));
                }
            }
        }
    }

    // Context filters — group by name, OR values within a group, AND between groups
    if let Some(ref ctx_filters) = args.context_filters {
        if !ctx_filters.is_empty() {
            use std::collections::HashMap;
            let mut groups: HashMap<String, Vec<String>> = HashMap::new();
            for f in ctx_filters {
                groups.entry(f.name.clone()).or_default().push(f.value.clone());
            }
            let exists_clauses: Vec<String> = groups
                .iter()
                .map(|(name, values)| {
                    let has_not_set = values.iter().any(|v| v == "__not_set__");
                    let regular_values: Vec<&String> = values.iter().filter(|v| v.as_str() != "__not_set__").collect();

                    let mut subclauses: Vec<String> = Vec::new();

                    // "not set" — sessions with NO entry for this name in context_index
                    if has_not_set {
                        let pn = next_param(&mut param_idx);
                        param_values.push(Box::new(name.clone()));
                        subclauses.push(format!(
                            "session_uuid NOT IN (SELECT DISTINCT session_uuid FROM context_index WHERE name = {pn})"
                        ));
                    }

                    // Regular value filter — sessions that have this name with specific values
                    if !regular_values.is_empty() {
                        let pn = next_param(&mut param_idx);
                        param_values.push(Box::new(name.clone()));
                        let value_placeholders: Vec<String> = regular_values
                            .iter()
                            .map(|v| {
                                let pv = next_param(&mut param_idx);
                                param_values.push(Box::new((*v).clone()));
                                pv
                            })
                            .collect();
                        let in_list = value_placeholders.join(", ");
                        // Use context_index (pre-computed at import) instead of json_each for speed
                        subclauses.push(format!(
                            "session_uuid IN (SELECT DISTINCT session_uuid FROM context_index \
                             WHERE name = {pn} AND value IN ({in_list}))"
                        ));
                    }

                    if subclauses.len() == 1 {
                        subclauses.into_iter().next().unwrap_or_default()
                    } else {
                        format!("({})", subclauses.join(" OR "))
                    }
                })
                .filter(|s| !s.is_empty())
                .collect();
            if !exists_clauses.is_empty() {
                conditions.extend(exists_clauses);
            }
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();

    // Count total sessions
    let count_sql = format!(
        "SELECT COUNT(DISTINCT session_uuid) FROM interactions {where_clause}"
    );
    let total: i64 = conn
        .query_row(&count_sql, params_ref.as_slice(), |row| row.get(0))
        .unwrap_or(0);

    // Get session summaries
    let p_limit = next_param(&mut param_idx);
    let p_offset = next_param(&mut param_idx);
    param_values.push(Box::new(limit));
    param_values.push(Box::new(offset));
    let params_ref2: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();

    let sql = format!(
        r#"SELECT
            s.session_uuid,
            MIN(s.timestamp_start) as first_ts,
            MAX(s.timestamp_end) as last_ts,
            COUNT(*) as cnt,
            MAX(CASE WHEN s.main_interaction_type = 'GenerativeAI'
                          OR s.all_interaction_types LIKE '%GenerativeAI%' THEN 1 ELSE 0 END) as has_gen_ai,
            MIN(s.culture) as culture,
            (SELECT interaction_value FROM interactions i2
             WHERE i2.session_uuid = s.session_uuid
               AND i2.interaction_value != ''
               AND i2.interaction_value NOT LIKE '#%#'
               AND LOWER(i2.interaction_value) != 'continue'
               AND i2.main_interaction_type NOT IN ('Event', 'LinkClick')
             ORDER BY i2.log_id ASC LIMIT 1) as preview,
            MAX(CASE WHEN s.feedback_info LIKE '%"score": -1%'
                      OR s.feedback_info LIKE '%"score":-1%' THEN 1 ELSE 0 END) as has_neg_feedback,
            MAX(CASE WHEN s.feedback_info LIKE '%"score": 1%'
                      OR s.feedback_info LIKE '%"score":1%' THEN 1 ELSE 0 END) as has_pos_feedback,
            (SELECT i2.contexts FROM interactions i2
             WHERE i2.session_uuid = s.session_uuid
               AND i2.contexts IS NOT NULL AND i2.contexts != ''
               AND i2.contexts != '[]' AND i2.contexts != 'null'
             ORDER BY i2.log_id DESC LIMIT 1) as contexts_snapshot
        FROM interactions s
        {where_clause}
        GROUP BY s.session_uuid
        ORDER BY first_ts DESC
        LIMIT {p_limit} OFFSET {p_offset}"#
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| format!("Query error: {e}"))?;
    let sessions = stmt
        .query_map(params_ref2.as_slice(), |row| {
            Ok(SessionSummary {
                session_uuid: row.get::<_, String>(0)?,
                first_ts: row.get::<_, String>(1).unwrap_or_default(),
                last_ts: row.get::<_, String>(2).unwrap_or_default(),
                interaction_count: row.get::<_, i64>(3)?,
                has_gen_ai: row.get::<_, i64>(4)? == 1,
                culture: row.get::<_, String>(5).unwrap_or_default(),
                user_message_preview: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                has_neg_feedback: row.get::<_, i64>(7).unwrap_or(0) == 1,
                has_pos_feedback: row.get::<_, i64>(8).unwrap_or(0) == 1,
                contexts: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
            })
        })
        .map_err(|e| format!("Query error: {e}"))?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();

    Ok(SessionsPage { sessions, total, page })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_context_options(db_state: State<'_, SharedDbState>) -> Result<Vec<ContextOption>, String> {
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
    let mut stmt2 = conn
        .prepare(
            "SELECT ci.name, \
              (SELECT COUNT(DISTINCT session_uuid) FROM interactions) - COUNT(DISTINCT ci.session_uuid) \
             FROM context_index ci \
             GROUP BY ci.name \
             HAVING (SELECT COUNT(DISTINCT session_uuid) FROM interactions) - COUNT(DISTINCT ci.session_uuid) > 0 \
             ORDER BY ci.name ASC",
        )
        .map_err(|e| format!("Prepare error: {e}"))?;

    let not_set_opts: Vec<ContextOption> = stmt2
        .query_map([], |row| {
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

    let rows = stmt
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

        // Get log_ids to delete from FTS
        let log_ids: Vec<i64> = {
            let mut stmt = tx
                .prepare(&format!(
                    "SELECT log_id FROM interactions WHERE DATE(timestamp_start) IN ({placeholders})"
                ))
                .map_err(|e| format!("Prepare error: {e}"))?;
            let ids = stmt.query_map(params_refs.as_slice(), |row| row.get::<_, i64>(0))
                .map_err(|e| format!("Query error: {e}"))?
                .filter_map(|r| r.ok())
                .collect();
            ids
        };

        let params_refs2: Vec<&dyn rusqlite::types::ToSql> =
            args.dates.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();

        // Delete sessions from context_index that belong exclusively to the deleted date range
        tx.execute_batch(&format!(
            "DELETE FROM context_index WHERE session_uuid IN (\
               SELECT DISTINCT session_uuid FROM interactions \
               WHERE DATE(timestamp_start) IN ({placeholders}) \
               AND session_uuid NOT IN (\
                 SELECT DISTINCT session_uuid FROM interactions \
                 WHERE DATE(timestamp_start) NOT IN ({placeholders})\
               )\
             )",
        )).ok();

        // Delete from interactions
        let deleted = tx
            .execute(
                &format!("DELETE FROM interactions WHERE DATE(timestamp_start) IN ({placeholders})"),
                params_refs2.as_slice(),
            )
            .map_err(|e| format!("Delete error: {e}"))? as i64;

        // Clean up FTS index for deleted rows
        for log_id in &log_ids {
            let _ = tx.execute(
                "DELETE FROM interactions_fts WHERE rowid = ?1",
                params![log_id],
            );
        }

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

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
            for row in &rows {
                let is_flagged = if flagged_set.contains(&row.log_id) { 1i64 } else { 0i64 };
                tx.execute(
                    "INSERT INTO flagged_interactions \
                     (flag_id, log_id, interaction_uuid, session_uuid, timestamp_start, timestamp_end, \
                      culture, main_interaction_type, all_interaction_types, interaction_value, output_text, \
                      article_ids, dialog_paths, tdialog_status, recognition_type, recognition_quality, \
                      generative_ai_sources, articles, faqs_found, contexts, pages, link_click_info, \
                      feedback_info, output_metadata, recognition_details, is_flagged) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",
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
            tx.commit().map_err(|e| format!("Commit error: {e}"))?;
        }

        Ok(flag_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Flagged folder commands ──────────────────────────────────────────────────

#[tauri::command]
async fn get_flagged_folders(flagged_db: State<'_, SharedFlaggedDb>) -> Result<Vec<FlaggedFolder>, String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("Flagged database not initialized.")?;
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
                    folder_id:     row.get(0)?,
                    name:          row.get::<_, String>(1).unwrap_or_default(),
                    created_at:    row.get::<_, String>(2).unwrap_or_default(),
                    sort_order:    row.get::<_, i64>(3).unwrap_or(0),
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
        let conn = state.conn.as_ref().ok_or("Flagged database not initialized.")?;
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
        let conn = state.conn.as_ref().ok_or("Flagged database not initialized.")?;
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
        let conn = state.conn.as_ref().ok_or("Flagged database not initialized.")?;
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
async fn get_flagged_sessions(flagged_db: State<'_, SharedFlaggedDb>) -> Result<Vec<FlaggedSessionSummary>, String> {
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
                        fs.folder_id \
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
        let conn = state.conn.as_ref().ok_or("Flagged database not initialized.")?;
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
                    log_id:                  row.get::<_, i64>(0).unwrap_or(0),
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
                    is_flagged:              row.get::<_, i64>(24).unwrap_or(0) != 0,
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
async fn unflag_session(
    flagged_db: State<'_, SharedFlaggedDb>,
    flag_id: i64,
) -> Result<(), String> {
    let fdb = flagged_db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = fdb.lock().map_err(|e| e.to_string())?;
        let conn = state.conn.as_ref().ok_or("Flagged database not initialized.")?;
        conn.execute("DELETE FROM flagged_sessions WHERE flag_id = ?1", params![flag_id])
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
            get_data,
            open_url,
            open_preview_window,
            select_data_folder,
            check_for_updates,
            get_version,
            set_db_path,
            get_db_path,
            select_csv_files,
            select_db_save_path,
            select_db_open_path,
            import_interactions_csv,
            get_sessions,
            get_session_interactions,
            get_date_range,
            get_context_options,
            get_db_daily_stats,
            delete_interactions_by_dates,
            flag_session,
            get_flagged_sessions,
            get_flagged_session_interactions,
            unflag_session,
            get_flagged_folders,
            create_flagged_folder,
            rename_flagged_folder,
            delete_flagged_folder,
            move_to_flagged_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application")
}
