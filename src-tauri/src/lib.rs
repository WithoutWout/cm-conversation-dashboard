use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
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
}

#[derive(Serialize)]
struct SourceFiles {
    articles: Option<String>,
    dialogs: Option<String>,
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
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(pattern) && name.ends_with(".json") {
                Some(entry.path())
            } else {
                None
            }
        })
        .collect()
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
    name.ends_with(".json")
        && source_definitions()
            .iter()
            .any(|definition| name.contains(definition.pattern))
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
    let mut files = DataFiles {
        articles: None,
        dialogs: None,
    };
    let mut source_files = SourceFiles {
        articles: None,
        dialogs: None,
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
    if url.starts_with("https://") || url.starts_with("http://") {
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

// ── Entry point ──────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(Mutex::new(WatchState::default())))
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
            get_version
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application")
}
