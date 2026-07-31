// Owns the SQLite database in the browser build.
//
// This has to be a dedicated worker: the OPFS `createSyncAccessHandle` API the
// SQLite VFS is built on exists nowhere else. That constraint is also what makes
// the interop cheap — the worker boundary is already one coarse message per user
// action, and it is the serialization point, so nothing here needs a lock.
//
// Results cross as JSON strings on purpose. Measured on the real 22 MB content
// payload: a JSON string plus JSON.parse beats a serde-wasm-bindgen object graph
// by 2–3.5x, and is ~20x cheaper to structured-clone out of the worker.
import init, * as wasm from "./pkg/cai_dashboard_lib.js"

let ready = null
const ensureReady = () => (ready ??= init())

// name -> (arg) => result. Anything returning a JSON string is parsed here so the
// renderer gets objects, matching what Tauri's invoke() resolved to.
const json = (s) => (s === undefined || s === "" ? null : JSON.parse(s))

const handlers = {
  openDatabase: (a) => wasm.open_database(a?.name ?? "conversations.db"),
  probeCapabilities: () => wasm.probe_capabilities(),

  getData: () => json(wasm.get_content_data()),
  getSessions: (a) => json(wasm.search_sessions(JSON.stringify(a ?? {}))),
  getSessionInteractions: (a) => json(wasm.get_session_interactions(a.sessionUuid)),
  getContextOptions: () => json(wasm.get_context_options()),
  getDateRange: () => json(wasm.get_date_range()),
  getDbDailyStats: () => json(wasm.get_db_daily_stats()),
  getDbHourCoverage: (a) => json(wasm.get_db_hour_coverage(a?.sinceDate ?? undefined)),
  recordImportedWindow: (a) => wasm.record_imported_window(a.startUtc, a.endUtc),
  beginImportRun: () => wasm.begin_import_run(),
  finalizeImportRun: (a) =>
    json(
      wasm.finalize_import_run(
        a?.maxAgeDays === null || a?.maxAgeDays === undefined
          ? undefined
          : BigInt(a.maxAgeDays)
      )
    ),

  // Bytes come from a File the user picked; there is no path to open.
  importCsvBytes: (a) =>
    json(
      wasm.import_csv(
        a.bytes,
        a.source ?? "import.csv",
        a.maxAgeDays === null || a.maxAgeDays === undefined
          ? undefined
          : BigInt(a.maxAgeDays),
        a.delimiter ?? undefined,
        a.deferFinalize ?? undefined
      )
    ),
}

self.onmessage = async (e) => {
  const { id, cmd, arg } = e.data
  try {
    await ensureReady()
    const fn = handlers[cmd]
    if (!fn) throw new Error(`${cmd} is not available in the web app yet`)
    self.postMessage({ id, ok: true, result: await fn(arg) })
  } catch (err) {
    self.postMessage({ id, ok: false, error: String(err?.message ?? err) })
  }
}

self.postMessage({ id: 0, ready: true })
