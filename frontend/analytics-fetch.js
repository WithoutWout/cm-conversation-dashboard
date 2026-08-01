// Analytics API transport for the browser build. Runs inside `db-worker.js`.
//
// The native client (`analytics_api.rs`) does token + fetch + temp file. Here the
// work is split in two, and the split is forced by CORS rather than chosen:
//
//   * `https://analytics.digitalcx.com` answers `Access-Control-Allow-Origin: *`,
//     so the interactions endpoint is readable straight from a page.
//   * `https://login.microsoftonline.com/.../oauth2/token` sends no
//     `Access-Control-Allow-Origin` at all (verified on both the v1 and v2.0
//     endpoints), so a browser cannot read a token response. `fetch` rejects with
//     a bare `TypeError` before any credential is even checked.
//
// So the token is supplied *to* this module — pasted by the user, or fetched from
// an optional proxy — and never minted here. `analytics-web.js` owns that; this
// file only spends a token it is handed.
//
// It lives in the worker rather than on the main thread so a day's CSV (tens of
// MB) is fetched, validated and imported without ever being structured-cloned
// across the worker boundary. `bodies` is the browser's answer to the native temp
// directory: same lifecycle, same one-entry-per-part, deleted by the same
// `cleanupAnalyticsTemp` call the renderer already makes.

/// Marks a synthetic "path" as one of ours, so `importInteractionsCsv` knows to
/// read it from `bodies` instead of looking for a picked File.
export const TEMP_SCHEME = "analytics://"

/// Response headers a browser can read cross-origin without
/// `Access-Control-Expose-Headers`. The analytics host sends no such header, so
/// this is the whole readable set — which is why two native checks are absent
/// below rather than merely unported.
const SAFELISTED_HEADERS = "cache-control content-language content-length content-type expires last-modified pragma"

export function createAnalyticsFetcher(wasm) {
  /** tempPath -> CSV text awaiting import. */
  const bodies = new Map()
  let counter = 0
  let endpoints = null
  const eps = () => (endpoints ??= JSON.parse(wasm.analytics_endpoints()))

  /// A `FetchError` in the exact shape the Rust command rejects with, so the
  /// renderer's `_impNormErr` and its split-vs-backoff branch need no web-only
  /// code path.
  const err = (kind, message, retryable = false) => {
    const e = new Error(message)
    e.fetchError = { kind, message, retryable }
    return e
  }

  /// The wasm validators reject with a `JsValue` holding serialized `FetchError`
  /// JSON. wasm-bindgen surfaces that as a bare string, not an Error.
  function fromWasm(raw) {
    const text = typeof raw === "string" ? raw : String(raw?.message ?? raw)
    try {
      const parsed = JSON.parse(text)
      if (parsed && typeof parsed.message === "string") {
        const e = new Error(parsed.message)
        e.fetchError = parsed
        return e
      }
    } catch (_) {
      // Not our JSON — fall through and report it as-is rather than inventing
      // a classification for it.
    }
    return err("invalidResponse", text)
  }

  /// Mirrors the native `match status.as_u16()`, with one deliberate difference:
  /// a **403** is reported as `unauthorized`. The native client never sees one
  /// because it always holds a freshly minted token; here the observed response
  /// to a stale or mistyped pasted token is `403 {"message":"Missing token or
  /// token is incorrect"}`, and calling that a generic `http` error would tell
  /// the user "unexpected error" when the fix is "paste a fresh token".
  function classify(status) {
    if (status === 401 || status === 403) return ["unauthorized", false]
    if (status === 400) return ["badRequest", false]
    if (status === 408 || status === 504) return ["timeout", true]
    if (status === 429) return ["rateLimited", true]
    if (status === 500 || status === 502 || status === 503) return ["serverError", true]
    return ["http", false]
  }

  function buildUrl(cfg, startUtc, endUtc) {
    const { apiBase } = eps()
    const url = new URL(
      `${apiBase}/${encodeURIComponent(String(cfg.customerKey).trim())}` +
        `/projects/${encodeURIComponent(String(cfg.projectKey).trim())}/interactions`
    )
    url.searchParams.set("culture", String(cfg.culture).trim())
    url.searchParams.set("startDate", startUtc)
    url.searchParams.set("endDate", endUtc)
    url.searchParams.set("environment", String(cfg.environment || "Production").trim())
    if (cfg.activeSessionOnly) url.searchParams.set("activeSessionOnly", "true")
    // `paginateData` is deliberately not sent — same reason as the native client:
    // the SOP requires confirming the mechanism before implementing it.
    return url
  }

  /// Download one window, validate it, and park the CSV under a synthetic path.
  ///
  /// Returns the same `FetchOutcome` shape as `fetch_analytics_window`, so
  /// `_impImportParts` and `_impCleanup` work unmodified.
  async function fetchWindow({ startUtc, endUtc, cfg, token }) {
    if (!cfg || !cfg.customerKey || !cfg.projectKey || !cfg.culture) {
      throw err("config", "Analytics API is not configured — add your settings in Settings")
    }
    if (!token) {
      throw err(
        "unauthorized",
        "No Analytics API access token. Paste one in Settings — a browser cannot " +
          "request one itself, because the token endpoint does not allow cross-origin calls."
      )
    }
    // Shared with the desktop build: ordered, parseable, under 24h, inside the
    // 90-day retention. Throws the same FetchError the native path would.
    try {
      wasm.analytics_validate_window(startUtc, endUtc)
    } catch (e) {
      throw fromWasm(e)
    }

    const started = Date.now()
    let resp
    try {
      resp = await fetch(buildUrl(cfg, startUtc, endUtc), {
        method: "GET",
        headers: { Authorization: `Bearer ${token}`, Accept: "text/csv" },
        // The native client's 300 s request timeout. An abort here is reported as
        // `timeout` so the scheduler halves the window, which is the documented
        // remedy for a window the server cannot answer in time.
        signal: AbortSignal.timeout(eps().fetchTimeoutSecs * 1000),
        // No cookies: the host sets an ARRAffinity cookie, and sending credentials
        // is incompatible with its `Access-Control-Allow-Origin: *` anyway.
        credentials: "omit",
        cache: "no-store",
      })
    } catch (e) {
      if (e && (e.name === "TimeoutError" || e.name === "AbortError")) {
        throw err(
          "timeout",
          `Interaction log request timed out after ${eps().fetchTimeoutSecs}s — the window may be too large`,
          true
        )
      }
      // `fetch` rejects with an opaque TypeError for offline, DNS failure and
      // CORS alike, and deliberately tells JS nothing about which. Name all
      // three rather than guessing at one.
      throw err(
        "network",
        `Could not reach the Analytics API: ${e?.message || e}. Check your connection — ` +
          "if this persists, the API host may have stopped allowing browser requests."
      )
    }

    if (!resp.ok) {
      const [kind, retryable] = classify(resp.status)
      const preview = (await resp.text().catch(() => "")).slice(0, 300)
      const hint =
        resp.status === 400
          ? " (a 400 is usually a missing or invalid parameter — check culture)"
          : ""
      // Retry-After is not in the safelist and the host does not expose it, so
      // the scheduler's exponential backoff is the only timing signal available.
      throw err(kind, `Analytics API returned ${resp.status}${hint}: ${preview}`, retryable)
    }

    const body = await resp.text()
    // Shared with the desktop build, and the reason it is shared: this is what
    // stops a JSON error object or an HTML page served with a 200 from being
    // imported as interaction rows.
    let delimiter
    try {
      delimiter = wasm.analytics_validate_csv(body)
    } catch (e) {
      throw fromWasm(e)
    }

    const clean = body.charCodeAt(0) === 0xfeff ? body.slice(1) : body
    const rowCount = Math.max(0, clean.split("\n").filter((l) => l.trim() !== "").length - 1)
    const tempPath = `${TEMP_SCHEME}interactions-${started}-${counter++}.csv`
    bodies.set(tempPath, clean)

    return {
      tempPath,
      delimiter,
      rowCount,
      // TextEncoder rather than String#length: `bytes` is reported to the user
      // and compared against the file the portal produces, and the CSV is full
      // of non-ASCII message text.
      bytes: new TextEncoder().encode(clean).length,
      durationMs: Date.now() - started,
    }
  }

  return {
    fetchWindow,

    /// True when a "path" is one of ours rather than a picked File's name.
    owns: (path) => typeof path === "string" && path.startsWith(TEMP_SCHEME),

    take(path) {
      const body = bodies.get(path)
      if (body === undefined) throw new Error(`No downloaded window named ${path}`)
      return body
    },

    /// Mirrors `cleanup_analytics_temp`: named paths, or everything when called
    /// with none. Returns how many were dropped.
    cleanup(paths) {
      if (!paths || !paths.length) {
        const n = bodies.size
        bodies.clear()
        return n
      }
      let n = 0
      for (const p of paths) if (bodies.delete(p)) n++
      return n
    },

    /// What the browser cannot check, for the import log — stated once per run
    /// rather than guessed at or quietly skipped. Both gaps have the same cause:
    /// the host sends no `Access-Control-Expose-Headers`, so only
    /// `SAFELISTED_HEADERS` are readable.
    limitations: () => [
      `A browser can only read these response headers cross-origin: ${SAFELISTED_HEADERS}. ` +
        "So a paginated response cannot be detected the way the desktop app detects it, " +
        "and Retry-After is ignored in favour of exponential backoff.",
    ],
  }
}
