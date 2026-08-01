// Service worker for the static build.
//
// BUILD_ID is rewritten by build-web.sh on every build. It is the whole cache
// strategy: a new build means a new cache name, so the old one is dropped
// wholesale rather than relying on per-file revalidation.
//
// ## One strategy for everything, and why that matters more than freshness
//
// Navigations used to be network-first while assets stayed cache-first. That
// combination can serve a **mismatched pair**: a fresh `index.html` from the
// network, then its scripts from the *previous* build's cache, because a
// cache-first hit for `wasm-bridge.js` does not know the HTML asking for it has
// changed. Half-new is worse than uniformly old — the app is a single 22k-line
// renderer plus a wasm module that must agree with it.
//
// So every request, navigation included, is cache-first within one build-scoped
// cache. A cache therefore only ever holds one self-consistent build, and no page
// load can straddle two. Freshness is handled explicitly instead: a new worker
// installs the next build alongside, then waits, and `web-update.js` offers the
// user a reload. Nothing swaps under a running page.
//
// This is also why `skipWaiting()`/`clients.claim()` are absent. They made the
// new worker adopt already-open pages, which is exactly the straddle above.
const BUILD_ID = "__BUILD_ID__"
const CACHE = `cai-dashboard-${BUILD_ID}`

// The shell. The .wasm is 2.7 MB and is the single most important thing to hold:
// without it the app cannot open its database at all, so an offline launch that
// misses it is not a degraded app, it is a blank one.
const SHELL = [
  ".",
  "index.html",
  "manifest.json",
  "wasm-bridge.js",
  "db-worker.js",
  "search-worker.js",
  "pkg/cai_dashboard_lib.js",
  "pkg/cai_dashboard_lib_bg.wasm",
  "vendor/vis-network.min.js",
  "analytics-web.js",
  "analytics-fetch.js",
  "web-update.js",
  "version.json",
  "cmwhitelogo.svg",
  "icons/icon-128.png",
  "icons/icon-192.png",
  "icons/icon-256.png",
  "icons/icon-512.png",
  "icons/icon-maskable-512.png",
]

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE)
      // addAll would fail the whole install on one 404; the shell list is
      // maintained by hand, so degrade to caching what actually exists.
      await Promise.all(
        SHELL.map((url) =>
          cache.add(new Request(url, { cache: "reload" })).catch((e) => {
            console.warn("sw: could not precache", url, e.message)
          })
        )
      )
      // Deliberately no skipWaiting() — see the module note. This worker waits
      // until the pages running the previous build are gone, or until one of them
      // asks to be taken over below.
    })()
  )
})

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const names = await caches.keys()
      await Promise.all(names.filter((n) => n !== CACHE).map((n) => caches.delete(n)))
      // clients.claim() *is* wanted here, but only because reaching activate now
      // means either every old page closed or one explicitly asked for the swap.
      // Either way there is no page mid-flight on the old build.
      await self.clients.claim()
    })()
  )
})

// The page's half of the opt-in update: `web-update.js` posts this when the user
// accepts the reload prompt, and reloads once `controllerchange` fires.
self.addEventListener("message", (event) => {
  if (event.data?.type === "SKIP_WAITING") self.skipWaiting()
})

self.addEventListener("fetch", (event) => {
  const req = event.request
  if (req.method !== "GET") return

  const url = new URL(req.url)
  if (url.origin !== self.location.origin) return // never touch the Analytics API

  // `version.json` is the update check's own probe and must never be answered
  // from the cache — a cached copy would report the running build as the latest
  // one forever, which is the one thing this file cannot get wrong.
  if (url.pathname.endsWith("/version.json")) {
    event.respondWith(
      fetch(req, { cache: "no-store" }).catch(
        async () => (await caches.match(req)) || Response.error()
      )
    )
    return
  }

  // Everything else — navigations included — is cache-first inside one
  // build-scoped cache, so a page load can never straddle two builds. A deploy is
  // picked up by the worker update below, not by racing individual requests.
  event.respondWith(
    (async () => {
      const hit = await caches.match(req, { ignoreSearch: true })
      if (hit) return hit
      try {
        const fresh = await fetch(req)
        if (fresh.ok && fresh.type === "basic") {
          const cache = await caches.open(CACHE)
          cache.put(req, fresh.clone())
        }
        return fresh
      } catch (e) {
        // Offline and not precached. A navigation still has somewhere to go.
        if (req.mode === "navigate") {
          return (await caches.match("index.html")) || Response.error()
        }
        throw e
      }
    })()
  )
})
