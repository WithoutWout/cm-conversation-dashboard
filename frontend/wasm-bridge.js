// window.electronAPI for the browser build.
//
// Loaded before the inline Tauri shim in index.html. That shim starts with
// `if (!invoke) return`, so on the desktop it wins and this file no-ops; in a
// browser there is no `invoke`, the shim bails, and this stays in place. One
// script tag, and the 43 existing call sites are untouched.
//
// Every method returns a promise that awaits worker readiness internally, so
// load order between this and the app's startup code does not matter.
;(function () {
  if (window.__TAURI__?.core?.invoke) return // desktop: leave the Tauri shim alone

  // The renderer decides whether a database is configured from this key, and on
  // the desktop it holds a path the user picked. In a browser there is exactly
  // one database and it lives in OPFS, so there is no path to pick — seeding the
  // key is what tells the existing UI it is configured, instead of showing a
  // "choose a location" modal for a choice that does not exist here.
  const DB_KEY = "cm-conv-db-path"
  if (!localStorage.getItem(DB_KEY)) {
    localStorage.setItem(DB_KEY, "conversations.db (OPFS)")
  }

  const worker = new Worker("db-worker.js", { type: "module" })
  let seq = 1
  const pending = new Map()
  let bootResolve
  const booted = new Promise((r) => (bootResolve = r))

  worker.onmessage = (e) => {
    const { id, ok, result, error, ready } = e.data
    if (ready) return bootResolve()
    const entry = pending.get(id)
    if (!entry) return
    pending.delete(id)
    ok ? entry.resolve(result) : entry.reject(new Error(error))
  }
  worker.onerror = (e) => {
    console.error("db-worker failed:", e.message, e.filename, e.lineno)
    bootResolve()
  }

  async function call(cmd, arg) {
    await booted
    return new Promise((resolve, reject) => {
      const id = seq++
      pending.set(id, { resolve, reject })
      worker.postMessage({ id, cmd, arg })
    })
  }

  // Files the user picked, keyed by name, standing in for filesystem paths.
  // Two files with the same name in one pick collapse to the last one — the
  // renderer only ever round-trips the key straight back to us, so the cost is
  // that one of the duplicates is imported twice rather than data being lost,
  // and `INSERT OR IGNORE` makes that a no-op.
  const pickedFiles = new Map()

  // A plain <input type="file"> rather than showOpenFilePicker, which is
  // Chromium-only. This works in every browser that can run the app at all.
  function pickCsvFiles() {
    return new Promise((resolve) => {
      const input = document.createElement("input")
      input.type = "file"
      input.accept = ".csv,text/csv"
      input.multiple = true
      input.style.display = "none"
      document.body.appendChild(input)

      let settled = false
      const done = (result) => {
        if (settled) return
        settled = true
        input.remove()
        resolve(result)
      }

      input.addEventListener("change", () => {
        const files = Array.from(input.files || [])
        files.forEach((f) => pickedFiles.set(f.name, f))
        done({
          ok: files.length > 0,
          canceled: files.length === 0,
          paths: files.map((f) => f.name),
        })
      })
      // `cancel` is not universally supported, so a focus fallback keeps the
      // promise from hanging forever if the dialog is dismissed.
      input.addEventListener("cancel", () => done({ ok: false, canceled: true, paths: [] }))
      window.addEventListener(
        "focus",
        () => setTimeout(() => done({ ok: false, canceled: true, paths: [] }), 500),
        { once: true }
      )
      input.click()
    })
  }

  // Commands with no browser equivalent yet. Rejecting with a specific message
  // beats a silent undefined: the renderer's catch blocks surface it, and the
  // text says which feature is missing rather than "something went wrong".
  const notYet = (label) => () =>
    Promise.reject(new Error(`${label} is not available in the web app yet`))

  window.electronAPI = {
    // ── ported ────────────────────────────────────────────────────────────
    getData: () => call("getData"),
    getSessions: (args) => call("getSessions", args),
    getSessionInteractions: (sessionUuid) =>
      call("getSessionInteractions", { sessionUuid }),
    getContextOptions: () => call("getContextOptions"),
    getDateRange: () => call("getDateRange"),
    getDbDailyStats: () => call("getDbDailyStats"),
    getDbHourCoverage: (sinceDate) => call("getDbHourCoverage", { sinceDate }),
    recordImportedWindow: (startUtc, endUtc) =>
      call("recordImportedWindow", { startUtc, endUtc }),
    beginImportRun: () => call("beginImportRun"),
    finalizeImportRun: (maxAgeDays) => call("finalizeImportRun", { maxAgeDays }),

    // Browser-only: the database lives in OPFS, so there is no path to choose.
    setDbPath: () => call("openDatabase", { name: "conversations.db" }),
    getDbPath: () => Promise.resolve("conversations.db (OPFS)"),

    // The renderer's import flow is `selectCsvFiles() -> {paths}` and then
    // `importInteractionsCsv(path, ...)` per path. A browser has no paths, but it
    // does have File objects — so picked files are parked in `pickedFiles` under
    // their name and that name is handed back as the "path". The renderer's loop,
    // its progress UI and its begin/finalize bracketing then work unmodified.
    selectCsvFiles: () => pickCsvFiles(),
    importInteractionsCsv: async (filePath, maxAgeDays, delimiter, deferFinalize) => {
      const file = pickedFiles.get(filePath)
      if (!file) throw new Error(`No picked file named ${filePath}`)
      const bytes = new Uint8Array(await file.arrayBuffer())
      const res = await call("importCsvBytes", {
        bytes,
        source: file.name,
        maxAgeDays,
        delimiter,
        deferFinalize,
      })
      pickedFiles.delete(filePath)
      return res
    },

    // ── host features that behave differently in a browser ────────────────
    openUrl: (url) => {
      // Same https/http restriction the Rust command enforces.
      if (!/^https?:\/\//i.test(String(url))) return Promise.resolve()
      window.open(url, "_blank", "noopener,noreferrer")
      return Promise.resolve()
    },
    openPreviewWindow: (url) => window.electronAPI.openUrl(url),
    getVersion: () => Promise.resolve(document.documentElement.dataset.appVersion || "web"),
    checkForUpdates: () =>
      Promise.resolve({ status: "unsupported", message: "The web app updates itself." }),
    onDataFolderUpdated: () => Promise.resolve(() => {}),

    // ── not ported yet ────────────────────────────────────────────────────
    selectDataFolder: notYet("Choosing a content folder"),
    selectDbSavePath: notYet("Choosing a database location"),
    selectDbOpenPath: notYet("Opening a database file"),
    saveCollectionExport: notYet("Saving a collection export"),
    exportConversationsForAi: notYet("Export for AI"),
    deleteInteractionsByDates: notYet("Deleting stored days"),
    compactDatabase: notYet("Compacting the database"),
    cancelSessionSearch: () => Promise.resolve(),
    flagSession: notYet("Flagging a conversation"),
    unflagSession: notYet("Unflagging a conversation"),
    getFlaggedFolders: notYet("Flagged folders"),
    createFlaggedFolder: notYet("Flagged folders"),
    renameFlaggedFolder: notYet("Flagged folders"),
    deleteFlaggedFolder: notYet("Flagged folders"),
    moveToFlaggedFolder: notYet("Flagged folders"),
    getFlaggedSessions: notYet("Flagged conversations"),
    getFlaggedSessionInteractions: notYet("Flagged conversations"),
    saveFlaggedNote: notYet("Flagged notes"),
    getAnalyticsConfig: notYet("Analytics API settings"),
    saveAnalyticsConfig: notYet("Analytics API settings"),
    testAnalyticsConnection: notYet("Analytics API settings"),
    fetchAnalyticsWindow: notYet("The Analytics API import"),
    cleanupAnalyticsTemp: () => Promise.resolve(0),
    resizeToAvailableHeight: () => Promise.resolve(),
  }

  // Open the OPFS database immediately: unlike the desktop app there is no path
  // to pick first, so waiting for a user action would only delay the first query.
  window.electronAPI.setDbPath().catch((e) => console.error("open database:", e))

  // Register the service worker so the app is installable and works offline.
  // Deliberately after the bridge is in place — registration failing must never
  // stop the app from running, it only costs offline support.
  if ("serviceWorker" in navigator) {
    window.addEventListener("load", () => {
      navigator.serviceWorker
        .register("sw.js")
        .catch((e) => console.warn("service worker registration failed:", e.message))
    })
  }
})()
