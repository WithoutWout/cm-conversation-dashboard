//! Browser host: the wasm entry point, mirroring `tauri_host.rs`.
//!
//! Runs inside a **dedicated worker** — not a choice. The OPFS
//! `createSyncAccessHandle` API that the SQLite VFS is built on exists nowhere
//! else, so the database can only live here.
//!
//! Three things differ from the desktop host, and all three are why this file is
//! thin rather than a second implementation of anything:
//!
//! * **No `spawn_blocking`.** There are no threads, and none are wanted: the
//!   worker is already off the UI thread and is itself the serialization point.
//!   SQLite is compiled `SQLITE_THREADSAFE=0` and the VFS allows one connection.
//! * **No connection lock.** Same reason — one worker, one connection, so a
//!   `thread_local` replaces `Arc<Mutex<DbState>>`.
//! * **Results cross as JSON strings.** Measured on the real 22 MB content
//!   payload: a JSON string plus `JSON.parse` beats a `serde-wasm-bindgen`
//!   object graph by 2–3.5×, and is ~20× cheaper to structured-clone out of the
//!   worker. Don't "improve" this into `to_value`.

use std::cell::RefCell;

use wasm_bindgen::prelude::*;

use sqlite_wasm_rs::WasmOsCallback;
use sqlite_wasm_vfs::sahpool::{install, OpfsSAHPoolCfgBuilder, OpfsSAHPoolUtil};

use crate::{
    begin_import_run_into, finalize_import_run_into, get_context_options_into, get_date_range_into,
    get_db_daily_stats_into, get_session_interactions_into, get_sessions_into, hour_coverage,
    import_csv_from_reader, open_db, record_imported_window_into, GetSessionsArgs,
};

/// Where the OPFS pool keeps its files, and the VFS name SQLite registers.
const OPFS_DIR: &str = "/cai-dashboard";
const VFS_NAME: &str = "opfs-sahpool";

thread_local! {
    /// The single open connection. Replaces `SharedDbState` — see the module note.
    static CONN: RefCell<Option<rusqlite::Connection>> = const { RefCell::new(None) };
}

fn js_err(context: &str, e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&format!("{context}: {e}"))
}

/// Registers the OPFS VFS as SQLite's default. Idempotent — `install` hands back
/// the existing pool rather than registering twice.
async fn opfs() -> Result<OpfsSAHPoolUtil, JsValue> {
    let cfg = OpfsSAHPoolCfgBuilder::new()
        .vfs_name(VFS_NAME)
        .directory(OPFS_DIR)
        .clear_on_init(false)
        .build();
    install::<WasmOsCallback>(&cfg, true)
        .await
        .map_err(|e| js_err("OPFS VFS install failed", e))
}

/// Reports what this context can actually do.
///
/// Must be called from the worker: `createSyncAccessHandle` is not exposed on the
/// main thread in every browser, so a main-thread probe wrongly concludes OPFS is
/// unavailable and sends the user down a fallback they don't need.
#[wasm_bindgen]
pub async fn probe_capabilities() -> Result<String, JsValue> {
    let util = opfs().await?;
    Ok(format!(
        "{{\"vfs\":\"{VFS_NAME}\",\"opfsFiles\":{},\"capacity\":{}}}",
        serde_json::to_string(&util.list()).unwrap_or_else(|_| "[]".into()),
        util.get_capacity()
    ))
}

/// Opens (creating if needed) an OPFS-backed database through the real
/// [`open_db`], so the browser gets the same schema, migrations, index drops,
/// FTS repair and summary checks as the desktop app.
#[wasm_bindgen]
pub async fn open_database(name: &str) -> Result<String, JsValue> {
    opfs().await?;
    let conn = open_db(name).map_err(|e| js_err("open_db", e))?;
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap_or_else(|_| "unknown".into());
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM interactions", [], |r| r.get(0))
        .unwrap_or(-1);
    CONN.with(|c| *c.borrow_mut() = Some(conn));
    Ok(format!(
        "{{\"opened\":\"{name}\",\"journalMode\":\"{journal}\",\"interactions\":{count}}}"
    ))
}

/// Runs a helper against the open connection, or reports that there isn't one.
fn with_conn<T>(
    f: impl FnOnce(&mut rusqlite::Connection) -> Result<T, String>,
) -> Result<T, JsValue> {
    CONN.with(|c| {
        let mut borrowed = c.borrow_mut();
        let conn = borrowed
            .as_mut()
            .ok_or_else(|| JsValue::from_str("No database open."))?;
        f(conn).map_err(|e| JsValue::from_str(&e))
    })
}

/// Imports a portal/API CSV from bytes handed over by JS.
///
/// `delimiter` defaults to `|`, the portal export format, matching the native
/// command. `defer_finalize` mirrors `import_interactions_csv`: callers looping
/// over many windows should pass `true` and bracket the loop with
/// [`begin_import_run`](crate::begin_import_run_into) / finalize, or the tail work
/// costs the size of the database once per file.
#[wasm_bindgen]
pub fn import_csv(
    bytes: &[u8],
    source: &str,
    max_age_days: Option<i64>,
    delimiter: Option<char>,
    defer_finalize: Option<bool>,
) -> Result<String, JsValue> {
    let delim = delimiter.unwrap_or('|') as u8;
    let finalize = !defer_finalize.unwrap_or(false);
    let result = with_conn(|conn| {
        import_csv_from_reader(conn, bytes, source, max_age_days, delim, finalize)
    })?;
    serde_json::to_string(&result).map_err(|e| js_err("serialize ImportResult", e))
}

/// Runs the real session search.
///
/// Takes and returns JSON strings: `args` is a `GetSessionsArgs` exactly as the
/// renderer already builds it for `invoke`, and the result is a `SessionsPage`,
/// so the existing `window.electronAPI` shim can keep its shape.
#[wasm_bindgen]
pub fn search_sessions(args_json: &str) -> Result<String, JsValue> {
    let args: GetSessionsArgs =
        serde_json::from_str(args_json).map_err(|e| js_err("bad GetSessionsArgs", e))?;
    let page = with_conn(|conn| get_sessions_into(conn, &args))?;
    serde_json::to_string(&page).map_err(|e| js_err("serialize SessionsPage", e))
}

/// Opens an import run. Pair with [`finalize_import_run`] around a loop of
/// [`import_csv`] calls, or the tail work costs the size of the database once per
/// file rather than once per run.
#[wasm_bindgen]
pub fn begin_import_run() -> Result<(), JsValue> {
    with_conn(begin_import_run_into)
}

/// Closes an import run: purge, scoped summary rebuild, FTS merge, planner stats.
#[wasm_bindgen]
pub fn finalize_import_run(max_age_days: Option<i64>) -> Result<String, JsValue> {
    let res = with_conn(|conn| finalize_import_run_into(conn, max_age_days))?;
    serde_json::to_string(&res).map_err(|e| js_err("serialize FinalizeResult", e))
}

/// The stored date range.
#[wasm_bindgen]
pub fn get_date_range() -> Result<String, JsValue> {
    let r = with_conn(|conn| get_date_range_into(conn))?;
    serde_json::to_string(&r).map_err(|e| js_err("serialize DateRange", e))
}

/// Per-UTC-day interaction counts plus totals — drives the Manage Database calendar.
#[wasm_bindgen]
pub fn get_db_daily_stats() -> Result<String, JsValue> {
    let r = with_conn(|conn| get_db_daily_stats_into(conn))?;
    serde_json::to_string(&r).map_err(|e| js_err("serialize DbDailyStats", e))
}

/// Per-day bitmask of the UTC hours a day is covered for — the union of hours
/// holding interactions and hours an API window explicitly requested.
#[wasm_bindgen]
pub fn get_db_hour_coverage(since_date: Option<String>) -> Result<String, JsValue> {
    let r = with_conn(|conn| hour_coverage(conn, since_date))?;
    serde_json::to_string(&r).map_err(|e| js_err("serialize coverage", e))
}

/// Marks the UTC hours an imported window covered. Call *after* its rows are in.
#[wasm_bindgen]
pub fn record_imported_window(start_utc: &str, end_utc: &str) -> Result<(), JsValue> {
    with_conn(|conn| record_imported_window_into(conn, start_utc, end_utc))
}

/// Every context name/value pair with its session count.
#[wasm_bindgen]
pub fn get_context_options() -> Result<String, JsValue> {
    let r = with_conn(|conn| get_context_options_into(conn))?;
    serde_json::to_string(&r).map_err(|e| js_err("serialize ContextOptions", e))
}

/// Every interaction row of one session, in `log_id` order — opens a chat.
#[wasm_bindgen]
pub fn get_session_interactions(session_uuid: String) -> Result<String, JsValue> {
    let r = with_conn(|conn| get_session_interactions_into(conn, session_uuid))?;
    serde_json::to_string(&r).map_err(|e| js_err("serialize interactions", e))
}

thread_local! {
    /// Parsed content export, held across `get_content_data` calls. The desktop
    /// host re-reads the folder each time; here the bytes arrive once from the
    /// picker, so re-parsing 19 MB of JSON on every call would be pure waste.
    static CONTENT: RefCell<Option<crate::AppData>> = const { RefCell::new(None) };
}

/// Filenames for the bytes passed to [`load_content`], plus the folder they came
/// from. Separate from the bytes because a `Vec<u8>` cannot travel in JSON.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ContentMeta {
    folder: Option<String>,
    articles: Option<String>,
    dialogs: Option<String>,
    entities: Option<String>,
}

/// The "no data folder selected" state — every source `found: false`.
///
/// This is a state the renderer already knows how to draw, which is what makes it
/// the right answer before anything is loaded, rather than an error.
fn empty_app_data() -> crate::AppData {
    let empty = || serde_json::Value::Array(vec![]);
    crate::AppData {
        articles: empty(),
        dialogs: empty(),
        t_dialogs: empty(),
        entities: empty(),
        conv_vars: empty(),
        ctx_vars: empty(),
        files: crate::DataFiles {
            articles: None,
            dialogs: None,
            entities: None,
        },
        source_files: crate::SourceFiles {
            articles: None,
            dialogs: None,
            entities: None,
        },
        data_source: crate::DataSourceInfo {
            selected_folder: None,
            active_folder: None,
            using_selected_folder: false,
            watched_folder: None,
            missing_sources: crate::source_definitions()
                .iter()
                .map(|d| d.key.to_string())
                .collect(),
            statuses: crate::source_definitions()
                .iter()
                .map(|d| crate::SourceStatus {
                    key: d.key.to_string(),
                    label: d.label.to_string(),
                    filename: None,
                    found: false,
                })
                .collect(),
        },
    }
}

/// Parses a content export from bytes the picker read, through the same
/// `extract_articles` / `extract_dialogs` / `extract_entities` the desktop host
/// uses — so the browser gets identical Articles, Dialogs, Transactional Dialogs,
/// entity enrichment and context variables.
///
/// Any of the three may be absent; the returned `dataSource.statuses` reports
/// which, exactly as scanning a folder would.
#[wasm_bindgen]
pub fn load_content(
    articles: Option<Vec<u8>>,
    dialogs: Option<Vec<u8>>,
    entities: Option<Vec<u8>>,
    meta_json: &str,
) -> Result<String, JsValue> {
    let meta: ContentMeta =
        serde_json::from_str(meta_json).map_err(|e| js_err("bad content meta", e))?;

    let as_text = |bytes: &[u8], what: &str| -> Result<String, JsValue> {
        std::str::from_utf8(bytes)
            .map(|s| s.to_owned())
            .map_err(|e| js_err(&format!("{what} is not valid UTF-8"), e))
    };

    let mut data = empty_app_data();

    if let Some(bytes) = articles.as_deref() {
        data.articles = crate::extract_articles(&as_text(bytes, "The Articles export")?);
    }
    if let Some(bytes) = dialogs.as_deref() {
        let (d, t, conv_vars, ctx_vars) =
            crate::extract_dialogs(&as_text(bytes, "The Dialogs export")?);
        data.dialogs = d;
        data.t_dialogs = t;
        data.conv_vars = conv_vars;
        data.ctx_vars = ctx_vars;
    }
    if let Some(bytes) = entities.as_deref() {
        data.entities = crate::extract_entities(&as_text(bytes, "The Entities export")?);
    }

    data.files = crate::DataFiles {
        articles: meta.articles.clone(),
        dialogs: meta.dialogs.clone(),
        entities: meta.entities.clone(),
    };
    data.source_files = crate::SourceFiles {
        articles: meta.articles.clone(),
        dialogs: meta.dialogs.clone(),
        entities: meta.entities.clone(),
    };

    let statuses: Vec<crate::SourceStatus> = crate::source_definitions()
        .iter()
        .map(|d| {
            let filename = match d.key {
                "articles" => meta.articles.clone(),
                "dialogs" => meta.dialogs.clone(),
                _ => None,
            };
            crate::SourceStatus {
                key: d.key.to_string(),
                label: d.label.to_string(),
                found: filename.is_some(),
                filename,
            }
        })
        .collect();
    data.data_source = crate::DataSourceInfo {
        selected_folder: meta.folder.clone(),
        active_folder: meta.folder.clone(),
        using_selected_folder: meta.folder.is_some(),
        // Nothing watches a folder in a browser; there is no notify equivalent.
        watched_folder: None,
        missing_sources: statuses
            .iter()
            .filter(|s| !s.found)
            .map(|s| s.key.clone())
            .collect(),
        statuses,
    };

    let json = serde_json::to_string(&data).map_err(|e| js_err("serialize AppData", e))?;
    CONTENT.with(|c| *c.borrow_mut() = Some(data));
    Ok(json)
}

/// Forgets the loaded content export, returning to the "no data folder" state.
#[wasm_bindgen]
pub fn clear_content() {
    CONTENT.with(|c| *c.borrow_mut() = None);
}

/// Content-export data — whatever [`load_content`] last parsed, or the app's
/// existing "no data folder selected" state.
#[wasm_bindgen]
pub fn get_content_data() -> Result<String, JsValue> {
    CONTENT.with(|c| {
        let borrowed = c.borrow();
        let data = borrowed.as_ref();
        match data {
            Some(d) => serde_json::to_string(d),
            None => serde_json::to_string(&empty_app_data()),
        }
        .map_err(|e| js_err("serialize AppData", e))
    })
}

// ── Analytics API ────────────────────────────────────────────────────────────
//
// Only the validators cross over. `frontend/analytics-web.js` owns the transport,
// because the browser's transport *is* `fetch` — but it must not own the two
// checks below, which are what keeps an HTML error page served with a `200` from
// being imported as interaction rows.

/// The endpoint constants and timeouts, so the browser client has no second copy
/// of them to drift from.
#[wasm_bindgen]
pub fn analytics_endpoints() -> String {
    let a = crate::analytics_api::TOKEN_URL;
    let b = crate::analytics_api::TOKEN_RESOURCE;
    let c = crate::analytics_api::API_BASE;
    format!(
        "{{\"tokenUrl\":{},\"tokenResource\":{},\"apiBase\":{},\
          \"fetchTimeoutSecs\":{},\"tokenSkewSecs\":{}}}",
        serde_json::to_string(a).unwrap_or_default(),
        serde_json::to_string(b).unwrap_or_default(),
        serde_json::to_string(c).unwrap_or_default(),
        crate::analytics_api::FETCH_TIMEOUT_SECS,
        crate::analytics_api::TOKEN_SKEW_SECS,
    )
}

/// A [`FetchError`](crate::analytics_api::FetchError) as JSON, so the JS
/// scheduler sees the same `{kind, message, retryable}` shape the Tauri command
/// rejects with and its timeout-vs-backoff branch needs no second code path.
fn fetch_err_json(e: crate::analytics_api::FetchError) -> JsValue {
    JsValue::from_str(
        &serde_json::to_string(&e).unwrap_or_else(|_| {
            "{\"kind\":\"invalidResponse\",\"message\":\"unserializable error\",\"retryable\":false}"
                .into()
        }),
    )
}

/// Enforces the request-window rules before a request is made: parseable UTC
/// timestamps, ordered, strictly under 24 hours, inside the 90-day retention.
#[wasm_bindgen]
pub fn analytics_validate_window(start_utc: &str, end_utc: &str) -> Result<(), JsValue> {
    crate::analytics_api::validate_window(start_utc, end_utc).map_err(fetch_err_json)
}

/// Confirms a response body really is the interaction-log CSV, returning the
/// delimiter it detected.
#[wasm_bindgen]
pub fn analytics_validate_csv(body: &str) -> Result<String, JsValue> {
    crate::analytics_api::validate_csv_header(body)
        .map(|d| d.to_string())
        .map_err(fetch_err_json)
}

/// Runs arbitrary read-only SQL. Diagnostics for the port — not a renderer API.
#[wasm_bindgen]
pub fn scalar_query(sql: &str) -> Result<String, JsValue> {
    with_conn(|conn| {
        conn.query_row(sql, [], |r| r.get::<_, rusqlite::types::Value>(0))
            .map(|v| format!("{v:?}"))
            .map_err(|e| e.to_string())
    })
}
