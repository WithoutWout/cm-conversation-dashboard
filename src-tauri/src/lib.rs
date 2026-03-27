use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, State};

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

fn extract_dialogs(content: &str) -> (serde_json::Value, serde_json::Value) {
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
    (dialogs, t_dialogs)
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
            let (loaded_dialogs, loaded_t_dialogs) = extract_dialogs(&content);
            dialogs = loaded_dialogs;
            t_dialogs = loaded_t_dialogs;
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

    if latest == current {
        UpdateResult {
            status: "up-to-date".into(),
            version: None,
            message: None,
        }
    } else {
        UpdateResult {
            status: "available".into(),
            version: Some(latest),
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
"#;

fn open_db(path: &str) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("Cannot open DB: {e}"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .map_err(|e| format!("PRAGMA error: {e}"))?;
    conn.execute_batch(DB_SCHEMA)
        .map_err(|e| format!("Schema error: {e}"))?;
    // Migrate existing databases: add recognition_details column if absent
    let _ = conn.execute_batch("ALTER TABLE interactions ADD COLUMN recognition_details TEXT");
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
    conn.execute(
        "DELETE FROM interactions WHERE timestamp_start < ?1",
        params![cutoff_dt],
    )
    .unwrap_or(0) as i64
}

// ── Conversation Tauri commands ───────────────────────────────────────────────

#[tauri::command]
fn set_db_path(
    db_state: State<SharedDbState>,
    path: String,
) -> Result<(), String> {
    let conn = open_db(&path)?;
    let mut state = db_state.lock().map_err(|e| e.to_string())?;
    state.conn = Some(conn);
    state.path = Some(path);
    Ok(())
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
fn import_interactions_csv(
    db_state: State<SharedDbState>,
    file_path: String,
) -> Result<ImportResult, String> {
    let mut state = db_state.lock().map_err(|e| e.to_string())?;
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
                Ok(1) => *inserted += 1,
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DateRange {
    min: String,
    max: String,
}

#[tauri::command]
fn get_date_range(db_state: State<SharedDbState>) -> Result<DateRange, String> {
    let state = db_state.lock().map_err(|e| e.to_string())?;
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetSessionsArgs {
    page: Option<i64>,
    date_from: Option<String>,
    date_to: Option<String>,
    filter: Option<String>, // "all" | "genai" | "neg_feedback" | "low_recog"
    query: Option<String>,
    query_regex: Option<bool>,   // treat query as a regex
    query_scope: Option<String>, // "both" | "user" | "bot"
    query_ids: Option<bool>,     // also search article_ids and dialog_paths columns
}

#[tauri::command]
fn get_sessions(
    db_state: State<SharedDbState>,
    args: GetSessionsArgs,
) -> Result<SessionsPage, String> {
    let state = db_state.lock().map_err(|e| e.to_string())?;
    let conn = state.conn.as_ref().ok_or("No database open.")?;

    let page = args.page.unwrap_or(1).max(1);
    let limit = 50i64;
    let offset = (page - 1) * limit;

    let filter = args.filter.as_deref().unwrap_or("all");
    let query = args.query.as_deref().unwrap_or("").trim().to_string();
    let query_regex = args.query_regex.unwrap_or(false);
    let query_scope = args.query_scope.as_deref().unwrap_or("both").to_string();
    let query_ids = args.query_ids.unwrap_or(false);

    // Register a custom REGEXP function for this connection when regex mode is on
    if query_regex && !query.is_empty() {
        use regex::Regex;
        use std::sync::Arc;
        // Compile the regex once and share it across all row evaluations via Arc
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

    // Build WHERE clauses
    // Always exclude sessions that have no real user input
    let mut conditions: Vec<String> = vec![
        "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions \
         WHERE interaction_value != '' \
           AND interaction_value NOT LIKE '#%#' \
           AND LOWER(interaction_value) != 'continue' \
           AND main_interaction_type NOT IN ('Event', 'LinkClick'))".to_string(),
    ];
    if filter == "genai" {
        conditions.push(
            "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE main_interaction_type = 'GenerativeAI')".to_string(),
        );
    } else if filter == "neg_feedback" {
        conditions.push(
            "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE feedback_info LIKE '%\"score\": -1%' OR feedback_info LIKE '%\"score\":-1%')".to_string(),
        );
    } else if filter == "low_recog" {
        conditions.push(
            "session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE recognition_quality > 0 AND recognition_quality < 60)".to_string(),
        );
    }
    if let Some(ref df) = args.date_from {
        if !df.is_empty() {
            conditions.push(format!("timestamp_start >= '{}'", df.replace('\'', "")));
        }
    }
    if let Some(ref dt) = args.date_to {
        if !dt.is_empty() {
            conditions.push(format!("timestamp_start <= '{}'", dt.replace('\'', "")));
        }
    }
    if !query.is_empty() {
        let q = query.replace('\'', "''");
        let text_cond = if query_regex {
            match query_scope.as_str() {
                "user" => format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE regexp('{q}', interaction_value))"),
                "bot"  => format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE regexp('{q}', output_text))"),
                _      => format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE regexp('{q}', interaction_value) OR regexp('{q}', output_text))"),
            }
        } else {
            match query_scope.as_str() {
                "user" => format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE interaction_value LIKE '%{q}%')"),
                "bot"  => format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE output_text LIKE '%{q}%')"),
                _      => format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE interaction_value LIKE '%{q}%' OR output_text LIKE '%{q}%')"),
            }
        };
        let cond = if query_ids {
            // Also match sessions where article_ids or dialog_paths reference the query term
            let ids_subq = format!("session_uuid IN (SELECT DISTINCT session_uuid FROM interactions WHERE article_ids LIKE '%{q}%' OR dialog_paths LIKE '%{q}%')");
            format!("({text_cond} OR {ids_subq})")
        } else {
            text_cond
        };
        conditions.push(cond);
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    // Count total sessions
    let count_sql = format!(
        "SELECT COUNT(DISTINCT session_uuid) FROM interactions {where_clause}"
    );
    let total: i64 = conn.query_row(&count_sql, [], |row| row.get(0)).unwrap_or(0);

    // Get session summaries - use inner query to build per-session aggregates
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
                      OR s.feedback_info LIKE '%"score":-1%' THEN 1 ELSE 0 END) as has_neg_feedback
        FROM interactions s
        {where_clause}
        GROUP BY s.session_uuid
        ORDER BY first_ts DESC
        LIMIT {limit} OFFSET {offset}"#
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| format!("Query error: {e}"))?;
    let sessions = stmt
        .query_map([], |row| {
            Ok(SessionSummary {
                session_uuid: row.get::<_, String>(0)?,
                first_ts: row.get::<_, String>(1).unwrap_or_default(),
                last_ts: row.get::<_, String>(2).unwrap_or_default(),
                interaction_count: row.get::<_, i64>(3)?,
                has_gen_ai: row.get::<_, i64>(4)? == 1,
                culture: row.get::<_, String>(5).unwrap_or_default(),
                user_message_preview: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                has_neg_feedback: row.get::<_, i64>(7).unwrap_or(0) == 1,
            })
        })
        .map_err(|e| format!("Query error: {e}"))?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();

    Ok(SessionsPage { sessions, total, page })
}

#[tauri::command]
fn get_session_interactions(
    db_state: State<SharedDbState>,
    session_uuid: String,
) -> Result<Vec<InteractionRow>, String> {
    let state = db_state.lock().map_err(|e| e.to_string())?;
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
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(Mutex::new(WatchState::default())))
        .manage(Arc::new(Mutex::new(DbState::default())) as SharedDbState)
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_data,
            open_url,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application")
}
