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

/// Runs arbitrary read-only SQL. Diagnostics for the port — not a renderer API.
#[wasm_bindgen]
pub fn scalar_query(sql: &str) -> Result<String, JsValue> {
    with_conn(|conn| {
        conn.query_row(sql, [], |r| r.get::<_, rusqlite::types::Value>(0))
            .map(|v| format!("{v:?}"))
            .map_err(|e| e.to_string())
    })
}
