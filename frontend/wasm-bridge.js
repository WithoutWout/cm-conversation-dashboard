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

  // ── content export (Articles / Dialogs / Entities) ──────────────────────
  //
  // The desktop host scans a folder and picks the newest file matching each
  // pattern. Two things differ here: `showDirectoryPicker` is Chromium-only, so
  // there is a multi-file fallback; and a browser has no mtime-ordered directory
  // listing, so "newest" comes from File.lastModified.
  const SOURCE_PATTERNS = [
    { key: "articles", pattern: "ArticlesExport", ext: ".json" },
    { key: "dialogs", pattern: "DialogsExport", ext: ".json" },
    { key: "entities", pattern: "EntitiesExport", ext: ".csv" },
  ]

  // Newest wins, mirroring newest_matching_file() on the desktop.
  function pickNewest(files, { pattern, ext }) {
    return files
      .filter(
        (f) =>
          f.name.includes(pattern) && f.name.toLowerCase().endsWith(ext)
      )
      .sort((a, b) => b.lastModified - a.lastModified)[0]
  }

  async function readDirectory() {
    const dir = await window.showDirectoryPicker({ id: "cai-content", mode: "read" })
    const files = []
    for await (const entry of dir.values()) {
      if (entry.kind === "file") files.push(await entry.getFile())
    }
    return { files, label: dir.name }
  }

  function readFileList() {
    return new Promise((resolve) => {
      const input = document.createElement("input")
      input.type = "file"
      // webkitdirectory would let a whole folder be chosen, but it is not
      // universally supported either; accepting the export files directly works
      // everywhere and is a clearer ask than "pick a folder, but only in Chrome".
      input.accept = ".json,.csv,application/json,text/csv"
      input.multiple = true
      input.style.display = "none"
      document.body.appendChild(input)

      let settled = false
      const done = (v) => {
        if (settled) return
        settled = true
        input.remove()
        resolve(v)
      }
      input.addEventListener("change", () => {
        const files = Array.from(input.files || [])
        done(files.length ? { files, label: "Selected files" } : null)
      })
      input.addEventListener("cancel", () => done(null))
      window.addEventListener("focus", () => setTimeout(() => done(null), 500), {
        once: true,
      })
      input.click()
    })
  }

  async function chooseContentSource() {
    let picked = null
    if (typeof window.showDirectoryPicker === "function") {
      try {
        picked = await readDirectory()
      } catch (e) {
        // AbortError is the user dismissing the dialog — not a failure.
        if (e && e.name === "AbortError") return { ok: false, canceled: true }
        // Anything else (a permission policy, an unsupported context) should not
        // dead-end the user when the input fallback still works.
        console.warn("directory picker unavailable, falling back:", e.message)
      }
    }
    if (!picked) picked = await readFileList()
    if (!picked) return { ok: false, canceled: true }

    const chosen = {}
    for (const spec of SOURCE_PATTERNS) {
      const file = pickNewest(picked.files, spec)
      if (file) chosen[spec.key] = file
    }
    if (!chosen.articles && !chosen.dialogs) {
      throw new Error(
        "No export files found. Expected file names containing " +
          "ArticlesExport or DialogsExport."
      )
    }

    const bytes = async (f) => (f ? new Uint8Array(await f.arrayBuffer()) : undefined)
    const data = await call("loadContent", {
      articles: await bytes(chosen.articles),
      dialogs: await bytes(chosen.dialogs),
      entities: await bytes(chosen.entities),
      meta: {
        folder: picked.label,
        articles: chosen.articles?.name,
        dialogs: chosen.dialogs?.name,
        entities: chosen.entities?.name,
      },
    })
    // The renderer calls getData() straight after this, which returns the parsed
    // content the worker is now holding.
    void data
    return { ok: true, canceled: false, path: picked.label }
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
    selectDataFolder: () => chooseContentSource(),
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
