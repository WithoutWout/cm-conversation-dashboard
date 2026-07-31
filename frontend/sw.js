// Service worker for the static build.
//
// BUILD_ID is rewritten by build-web.sh on every build. It is the whole cache
// strategy: a new build means a new cache name, so the old one is dropped
// wholesale rather than relying on per-file revalidation.
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
  "cmwhitelogo.svg",
  "icons/icon-128.png",
  "icons/icon-256.png",
  "icons/icon-512.png",
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
      await self.skipWaiting()
    })()
  )
})

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const names = await caches.keys()
      await Promise.all(names.filter((n) => n !== CACHE).map((n) => caches.delete(n)))
      await self.clients.claim()
    })()
  )
})

self.addEventListener("fetch", (event) => {
  const req = event.request
  if (req.method !== "GET") return

  const url = new URL(req.url)
  if (url.origin !== self.location.origin) return // never touch the Analytics API

  // Navigations go network-first so a deploy is picked up on the next load, with
  // the cached shell as the offline fallback.
  if (req.mode === "navigate") {
    event.respondWith(
      (async () => {
        try {
          const fresh = await fetch(req)
          const cache = await caches.open(CACHE)
          cache.put(req, fresh.clone())
          return fresh
        } catch {
          return (
            (await caches.match(req)) ||
            (await caches.match("index.html")) ||
            Response.error()
          )
        }
      })()
    )
    return
  }

  // Everything else is cache-first: these are build artefacts, and the cache name
  // already changed if the build did.
  event.respondWith(
    (async () => {
      const hit = await caches.match(req)
      if (hit) return hit
      const fresh = await fetch(req)
      if (fresh.ok && fresh.type === "basic") {
        const cache = await caches.open(CACHE)
        cache.put(req, fresh.clone())
      }
      return fresh
    })()
  )
})
