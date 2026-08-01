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

  const banner = () => document.getElementById("updateBanner")

  /// Reuses the desktop update banner's markup and styling rather than adding a
  /// second one. Only the text and the action differ: Reload, not Download.
  function showBanner(text) {
    const el = banner()
    if (!el || el.dataset.webUpdateShown === "1") return
    const textEl = document.getElementById("updateBannerText")
    if (!textEl) return
    el.dataset.webUpdateShown = "1"
    textEl.textContent = text + " "
    const btn = document.createElement("button")
    btn.className = "update-banner-download"
    btn.type = "button"
    btn.textContent = "Reload"
    btn.addEventListener("click", applyUpdate)
    textEl.appendChild(btn)
    el.classList.add("visible")
  }

  /// Hand over to the waiting worker, then reload once it has taken control.
  ///
  /// Reloading *before* the swap would just re-run the current build from the old
  /// cache and look like the button did nothing, so the reload waits for
  /// `controllerchange`. With no waiting worker there is nothing to hand over to
  /// and a plain reload is already correct.
  function applyUpdate() {
    if (reloading) return
    reloading = true
    if (waitingWorker) {
      navigator.serviceWorker.addEventListener(
        "controllerchange",
        () => location.reload(),
        { once: true }
      )
      waitingWorker.postMessage({ type: "SKIP_WAITING" })
      // If the worker never answers, don't leave the user on a dead button.
      setTimeout(() => location.reload(), 3000)
    } else {
      location.reload()
    }
  }

  /// The service-worker signal can fire before `version.json` has answered, so
  /// the new version number is often simply not known yet. Say so generically
  /// rather than interpolating a placeholder — an earlier draft rendered
  /// "Version new is available."
  function announce(newVersion) {
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
