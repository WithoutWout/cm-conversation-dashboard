//! Tauri desktop host: the `#[tauri::command]` surface, the native file and
//! folder dialogs, the `notify` folder watcher, and `run()`.
//!
//! Split out of `lib.rs` so everything else in this crate — the SQLite layer,
//! the search query builder, the CSV import and the AI export — also compiles
//! for `wasm32-unknown-unknown`. Nothing in this file can follow it there:
//! `tauri` and `notify` have no wasm target, and the whole module is gated
//! `#[cfg(not(target_arch = "wasm32"))]` at its declaration in `lib.rs`.
//!
//! Being a child module is what makes the split cheap: private items in the
//! parent are visible to its descendants, so `use super::*` reaches the entire
//! core without a single item needing to become `pub(crate)`.

use super::*;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, Manager, State};

// The folder watcher's poll interval — the only `Duration` left in the crate
// once the host moved out here.
use std::time::Duration;

use crate::analytics_api::{
    AnalyticsConfig, AnalyticsConfigView, AnalyticsState, FetchError, FetchOutcome,
};

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

fn emit_watch_event(app: &AppHandle, folder: &Path, reason: &str) {
    let payload = FolderWatchEvent {
        reason: reason.to_string(),
        folder: folder.to_string_lossy().into_owned(),
    };
    let _ = app.emit(WATCH_EVENT_NAME, payload);
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
        "queryEntities": args.query_entities.unwrap_or(false),
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

    // From here on the command is one long await with two genuinely slow phases
    // in it, and the renderer cannot see the boundary between them — or tell
    // "the save dialog is open" from "it is working" — without being told.
    let _ = app.emit(
        AI_EXPORT_PROGRESS_EVENT,
        serde_json::json!({ "phase": "querying" }),
    );

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
    let _ = app.emit(
        AI_EXPORT_PROGRESS_EVENT,
        serde_json::json!({
            "phase": "writing",
            "sessionCount": sessions.len(),
            "interactionCount": planned_turns,
        }),
    );

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

        // Remove stale FTS5 and entity entries in set-based statements before
        // deleting the interactions they point at.
        let _ = tx.execute(
            &format!(
                "DELETE FROM interactions_fts WHERE rowid IN \
                 (SELECT log_id FROM interactions WHERE DATE(timestamp_start) IN ({placeholders}))"
            ),
            params_refs.as_slice(),
        );
        let _ = tx.execute(
            &format!(
                "DELETE FROM entity_index WHERE log_id IN \
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

