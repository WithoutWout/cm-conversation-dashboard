// Version reporting and update handling for the browser build.
//
// The desktop app checks GitHub releases and tells the user to download an
// installer. Neither half of that applies here: there is nothing to download,
// and the new version is already sitting on the user's own web host the moment
// they upload it. So this file answers a different question — *is the copy
// running in this tab still the copy on the server?* — and offers a reload.
//
// Two independent signals, because they fail differently:
//
//   1. **The service worker update.** The browser re-fetches `sw.js` on its own
//      (bypassing the HTTP cache), sees a new `BUILD_ID`, and installs the next
//      build into a second cache. That worker then *waits* — `sw.js` deliberately
//      does not call `skipWaiting()` — so the running page keeps its own
//      self-consistent build until the user accepts a reload.
//   2. **`version.json`, fetched `no-store`.** An explicit probe that works even
//      if the worker never updates (a stuck registration, a host with odd
//      caching, or a tab left open for a week). This is also what makes the
//      version visible in Settings rather than merely knowable.
//
// Signal 1 gives a clean swap; signal 2 guarantees the user is told. Either one
// alone leaves a hole, so both raise the same banner.
;(function () {
  if (window.__TAURI__?.core?.invoke) return // desktop: the Rust updater owns this

  const html = document.documentElement
  // Stamped into index.html by build-web.sh. The fallbacks keep an unstamped or
  // hand-served copy working rather than rendering "undefined".
  const RUNNING = {
    version: html.dataset.appVersion || "dev",
    buildId: html.dataset.buildId || "",
  }

  let latest = null // the deployed build, once version.json has answered
  let waitingWorker = null
  let reloading = false

  // A forced refresh that worked leaves its marker behind pointing at the build we
  // are now running. Clearing it here means a *later* genuine update starts from a
  // clean slate and gets the normal Reload offer rather than the "your host is
  // caching" message.
  try {
    if (sessionStorage.getItem("cm-update-forced-for") === (html.dataset.buildId || "")) {
      sessionStorage.removeItem("cm-update-forced-for")
    }
  } catch (_) {
    /* sessionStorage can throw in a partitioned or locked-down context */
  }

  const banner = () => document.getElementById("updateBanner")

  /// Reuses the desktop update banner's markup and styling rather than adding a
  /// second one. Only the text and the action differ: Reload, not Download.
  ///
  /// `withAction: false` omits the button, for the one case where reloading
  /// provably cannot help — offering it there is what made this look broken.
  function showBanner(text, withAction = true) {
    const el = banner()
    if (!el) return
    // Keyed on the message, not a boolean. The guard exists so the two signals
    // don't both draw the same banner — but a *different* message has to be able
    // to replace it, or the first one shown freezes the banner for the rest of the
    // page's life. That mattered in practice: once the "your host is caching"
    // message appeared, a genuine update arriving later in the same session had
    // nowhere to put its Reload button.
    if (el.dataset.webUpdateKey === text) return
    const textEl = document.getElementById("updateBannerText")
    if (!textEl) return
    el.dataset.webUpdateKey = text
    textEl.textContent = text + " "
    if (withAction) {
      const btn = document.createElement("button")
      btn.className = "update-banner-download"
      btn.type = "button"
      btn.textContent = "Reload"
      btn.addEventListener("click", applyUpdate)
      textEl.appendChild(btn)
    }
    el.classList.add("visible")
  }

  /// Marks that a forced refresh was already attempted for a given build, so a
  /// load that *still* reports the old build can say so instead of offering the
  /// same button again. Session-scoped: a genuinely new session should retry.
  const FORCED_KEY = "cm-update-forced-for"

  /// Hand over to the waiting worker, then reload once it has taken control.
  ///
  /// Reloading *before* the swap would re-run the current build from the old cache
  /// and look like the button did nothing, so it waits for `controllerchange`.
  ///
  /// The no-waiting-worker case is the one that matters, and a plain reload is
  /// **wrong** there. `version.json` can report a new build before (or without)
  /// the worker noticing — a partial upload, a host serving stale HTML, a
  /// registration that never revalidated. Navigation is cache-first, so reloading
  /// re-serves the exact build we are trying to leave: the banner returns, the
  /// button appears broken, and it loops forever. So that path drops the caches
  /// and unregisters, which is what actually lets the next load reach the network.
  async function applyUpdate() {
    if (reloading) return
    reloading = true
    try {
      const reg = await navigator.serviceWorker?.getRegistration?.()
      // A worker may be installable but unnoticed; ask before giving up on the
      // clean hand-over path.
      if (reg && !waitingWorker) {
        try {
          await reg.update()
        } catch (_) {
          /* offline, or the host briefly failed — fall through to the forced path */
        }
        waitingWorker = reg.waiting || waitingWorker
      }

      if (waitingWorker) {
        navigator.serviceWorker.addEventListener(
          "controllerchange",
          () => location.reload(),
          { once: true }
        )
        waitingWorker.postMessage({ type: "SKIP_WAITING" })
        // If the worker never answers, don't leave the user on a dead button.
        setTimeout(() => location.reload(), 3000)
        return
      }

      // Forced path. Deliberately not attempted while offline: it would delete the
      // offline shell and leave nothing to load.
      if (navigator.onLine !== false) {
        sessionStorage.setItem(FORCED_KEY, latest?.buildId || "unknown")
        // Both, and in this order. Dropping caches alone leaves the *old* worker
        // active, which would refill its own build-named cache with the new
        // build's files — a mismatch of exactly the kind cache-first exists to
        // prevent. Unregistering means the next load comes from the network and
        // registers the new worker cleanly.
        for (const key of await caches.keys()) await caches.delete(key)
        if (reg) await reg.unregister()
      }
    } catch (e) {
      console.warn("update: forced refresh failed, reloading anyway:", e?.message || e)
    }
    // Neither the database nor any setting lives in the cache API or the
    // registration, so nothing above touches user data — only the copy of the app.
    location.reload()
  }

  /// The service-worker signal can fire before `version.json` has answered, so
  /// the new version number is often simply not known yet. Say so generically
  /// rather than interpolating a placeholder — an earlier draft rendered
  /// "Version new is available."
  function announce(newVersion) {
    // A forced refresh was already spent on this exact build and the server is
    // *still* handing out the old one. That is not something this page can fix, so
    // it stops offering a button and names the actual cause instead — repeating the
    // same offer is what turned this into an endless loop.
    if (newVersion?.buildId && sessionStorage.getItem(FORCED_KEY) === newVersion.buildId) {
      showBanner(
        `Build ${newVersion.buildId} is on the server, but this page is still being ` +
          `served build ${RUNNING.buildId} after a full refresh. index.html is being ` +
          `cached somewhere outside this browser — re-upload it, or clear your host's ` +
          `or CDN's cache.`,
        false
      )
      return
    }
    const known = newVersion && newVersion.version && newVersion.version !== RUNNING.version
    showBanner(
      known
        ? `Version ${newVersion.version} is available.`
        : "An updated build of this app is available."
    )
  }

  /// Ask the server what it is currently serving.
  ///
  /// `no-store` on both the request and the fetch options: this is the one file
  /// whose whole purpose is to not be cached, and `sw.js` also refuses to answer
  /// it from the cache.
  async function checkVersion() {
    try {
      const resp = await fetch("version.json", {
        cache: "no-store",
        headers: { "Cache-Control": "no-cache" },
      })
      if (!resp.ok) return null
      latest = await resp.json()
      // buildId is the comparison, not version: a rebuild of the same version is
      // still a different deployment, and it is the deployment that matters here.
      if (RUNNING.buildId && latest.buildId && latest.buildId !== RUNNING.buildId) {
        announce(latest)
      }
      return latest
    } catch (_) {
      return null // offline, or version.json not deployed — never a hard failure
    }
  }

  function watchRegistration(reg) {
    const offer = (worker) => {
      if (!worker) return
      waitingWorker = worker
      // `controller` absent means this is the first install, not an update —
      // prompting there would tell a first-time visitor to reload for nothing.
      if (navigator.serviceWorker.controller) announce(latest)
    }

    offer(reg.waiting)
    reg.addEventListener("updatefound", () => {
      const installing = reg.installing
      if (!installing) return
      installing.addEventListener("statechange", () => {
        if (installing.state === "installed") offer(reg.waiting || installing)
      })
    })

    // A tab can stay open for days. Re-check when it regains focus rather than on
    // a timer, so a backgrounded tab costs nothing.
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState !== "visible") return
      reg.update().catch(() => {})
      checkVersion()
    })
  }

  if ("serviceWorker" in navigator) {
    window.addEventListener("load", () => {
      navigator.serviceWorker
        .register("sw.js")
        .then(watchRegistration)
        .catch((e) => console.warn("service worker registration failed:", e.message))
    })
  }
  // Independent of the worker on purpose — see the file header.
  window.addEventListener("load", () => setTimeout(checkVersion, 1200))

  window.__webUpdate = {
    running: () => ({ ...RUNNING }),
    latest: () => (latest ? { ...latest } : null),
    check: checkVersion,
    apply: applyUpdate,
  }
})()
