// Analytics API settings and token handling for the browser build.
//
// The desktop app keeps credentials in `app_data_dir()/analytics-api.json` at
// 0600 and mints its own OAuth2 token. Neither is possible here, and the reason
// is worth stating plainly because it shapes the whole design:
//
//   the token endpoint (`login.microsoftonline.com/.../oauth2/token`) sends no
//   `Access-Control-Allow-Origin`, on both its v1 and v2.0 forms, so a browser
//   cannot read a token response at all — `fetch` rejects before any credential
//   is checked. The interactions endpoint *does* send `*`, so the data itself is
//   reachable.
//
// One request of the two is blocked, so the token is supplied rather than minted:
//
//   1. **Paste a token** (default). The user mints one wherever they like — the
//      Copy button hands them a ready `curl` — and pastes it here. Tokens last
//      24h per the SOP, so this is a once-a-day action.
//   2. **A token proxy** (optional). Any URL that returns `{access_token,
//      expires_in}`. For anyone who can host one small file this makes it
//      automatic again.
//
// Both are a security *improvement* over storing the secret in the browser, and
// that is why there is no client-secret field in the web build: in mode 1 the
// secret never leaves the user's terminal, and in mode 2 it never leaves the
// proxy. Nothing here has to be trusted with it.
;(function () {
  if (window.__TAURI__?.core?.invoke) return // desktop: the Rust client owns this

  const CFG_KEY = "cm-analytics-config"
  const TOK_KEY = "cm-analytics-token"

  const BLANK = {
    customerKey: "",
    projectKey: "",
    culture: "",
    environment: "Production",
    activeSessionOnly: false,
    tokenProxyUrl: "",
  }

  const readJson = (key, fallback) => {
    try {
      const raw = localStorage.getItem(key)
      return raw ? { ...fallback, ...JSON.parse(raw) } : { ...fallback }
    } catch (_) {
      return { ...fallback }
    }
  }

  const loadCfg = () => readJson(CFG_KEY, BLANK)
  const saveCfg = (c) => localStorage.setItem(CFG_KEY, JSON.stringify(c))

  const loadTok = () => readJson(TOK_KEY, { accessToken: "", expiresAt: 0 })
  const saveTok = (t) => localStorage.setItem(TOK_KEY, JSON.stringify(t))
  const clearTok = () => localStorage.removeItem(TOK_KEY)

  /// Skew matches the native client's: treat a token as spent slightly early so
  /// a long download cannot outlive it mid-run.
  let skewSecs = 120
  const nowSecs = () => Math.floor(Date.now() / 1000)
  const tokenValid = (t) => !!t.accessToken && (!t.expiresAt || t.expiresAt > nowSecs() + skewSecs)

  function describeToken(t) {
    if (!t.accessToken) return { ok: false, text: "No token — paste one below." }
    if (!t.expiresAt) return { ok: true, text: "Token stored (no expiry given)." }
    const left = t.expiresAt - nowSecs()
    if (left <= 0) return { ok: false, text: "Token expired — paste a fresh one." }
    const h = Math.floor(left / 3600)
    const m = Math.floor((left % 3600) / 60)
    return { ok: true, text: `Token valid for ${h > 0 ? `${h}h ` : ""}${m}m.` }
  }

  /// A JWT's own `exp` claim, so a pasted token reports a real expiry instead of
  /// an assumed one. Read without verifying: this is a display and refresh hint,
  /// not a security decision — the API is the only thing that actually validates.
  function jwtExpiry(token) {
    try {
      const part = token.split(".")[1]
      if (!part) return 0
      const json = atob(part.replace(/-/g, "+").replace(/_/g, "/"))
      const exp = JSON.parse(json).exp
      return Number.isFinite(exp) ? Number(exp) : 0
    } catch (_) {
      return 0
    }
  }

  /// Mint a token through a user-supplied proxy. The proxy holds the credentials;
  /// we only ever see the token it returns.
  async function tokenFromProxy(url) {
    const resp = await fetch(url, {
      method: "POST",
      headers: { Accept: "application/json" },
      cache: "no-store",
    })
    const text = await resp.text()
    if (!resp.ok) throw new Error(`Token proxy returned ${resp.status}: ${text.slice(0, 200)}`)
    let body
    try {
      body = JSON.parse(text)
    } catch (_) {
      throw new Error("Token proxy did not return JSON")
    }
    const accessToken = body.access_token || body.accessToken
    if (!accessToken) throw new Error("Token proxy response had no access_token")
    const expiresIn = Number(body.expires_in || body.expiresIn || 0)
    return {
      accessToken,
      expiresAt: expiresIn > 0 ? nowSecs() + expiresIn : jwtExpiry(accessToken),
    }
  }

  /// The token the next request should use, refreshing through the proxy when one
  /// is configured and the stored token is spent.
  async function currentToken() {
    let t = loadTok()
    if (tokenValid(t)) return t.accessToken
    const { tokenProxyUrl } = loadCfg()
    if (tokenProxyUrl) {
      t = await tokenFromProxy(tokenProxyUrl)
      saveTok(t)
      return t.accessToken
    }
    // Expired-but-present is still worth sending: the clock could be off, and a
    // 403 from the API is a clearer answer than refusing to try.
    return t.accessToken || ""
  }

  // ── Settings UI ────────────────────────────────────────────────────────────
  //
  // Injected at runtime rather than added to index.html, so the desktop build is
  // untouched and there is no dead markup in it. Everything web-specific about
  // the Analytics panel is in this one function.

  const esc = (s) =>
    String(s ?? "").replace(
      /[&<>"']/g,
      (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]
    )

  // Overwritten from Rust's own constants at startup; these are the fallback for
  // the brief window before the worker answers, and for the copy button if it
  // never does.
  let endpoints = {
    tokenUrl: "https://login.microsoftonline.com/digitalcx.onmicrosoft.com/oauth2/token",
    tokenResource: "https://digitalcx.onmicrosoft.com/external-api",
  }

  function curlCommand() {
    // Shown, not run. The secret goes into the user's own terminal — which is
    // exactly the point of the paste flow. `jq -r .access_token` is left off so
    // the user can see `expires_in` too.
    return (
      `curl -s -X POST '${endpoints.tokenUrl}' \\\n` +
      `  -d grant_type=client_credentials \\\n` +
      `  -d client_id=YOUR_CLIENT_ID \\\n` +
      `  -d client_secret=YOUR_CLIENT_SECRET \\\n` +
      `  -d resource='${endpoints.tokenResource}'`
    )
  }

  function injectSettingsUi() {
    const grid = document.querySelector(".analytics-cfg-grid")
    if (!grid || document.getElementById("analyticsWebBlock")) return

    // Client ID and secret have no job in the web build — see the file header.
    // They are *hidden*, not removed: `loadAnalyticsConfigIntoSettings` and
    // `readAnalyticsConfigFromSettings` both address them by id with no null
    // guard, so removing the nodes would throw before Settings could open.
    for (const id of ["setting-analytics-client-id", "setting-analytics-client-secret"]) {
      const wrap = document.getElementById(id)?.closest("div")
      if (wrap) wrap.style.display = "none"
    }

    const cfg = loadCfg()
    const block = document.createElement("div")
    block.id = "analyticsWebBlock"
    block.style.marginTop = "12px"
    block.innerHTML = `
      <style>
        #analyticsWebBlock .analytics-web-note {
          border-left: 2px solid var(--orange);
          background: var(--surface2);
          padding: 8px 10px;
          border-radius: 4px;
          font-size: 12px;
          color: var(--muted);
          line-height: 1.5;
        }
        #analyticsWebBlock .analytics-web-note strong { color: var(--text); }
      </style>
      <div class="analytics-web-note">
        <strong>Access token</strong> — this browser cannot request one itself.
        The CM.com token endpoint does not permit cross-origin requests, so mint a
        token elsewhere and paste it here. Tokens last about 24 hours.
      </div>
      <textarea id="setting-analytics-token" rows="3" spellcheck="false"
        autocomplete="off" placeholder="Paste an access token (eyJ0eXAi…)"
        style="width:100%;margin-top:8px;font-family:var(--mono,monospace);font-size:11px"></textarea>
      <div style="display:flex;align-items:center;gap:10px;margin-top:6px;flex-wrap:wrap">
        <button class="btn-secondary" id="analyticsCopyCurlBtn" type="button">Copy token command</button>
        <button class="btn-secondary" id="analyticsClearTokenBtn" type="button">Clear token</button>
        <span class="hint" id="analyticsTokenStatus"></span>
      </div>
      <details style="margin-top:10px">
        <summary class="hint" style="cursor:pointer">Automate it with a token proxy (optional)</summary>
        <p class="hint" style="margin:6px 0">
          A URL that returns <code>{"access_token":"…","expires_in":86400}</code>.
          The proxy holds your client secret so this browser never has to, and the
          token then refreshes on its own. Leave empty to keep pasting.
        </p>
        <input id="setting-analytics-token-proxy" type="url" autocomplete="off"
          spellcheck="false" placeholder="https://your-host/cai-token"
          value="${esc(cfg.tokenProxyUrl)}" style="width:100%" />
      </details>
    `
    grid.insertAdjacentElement("afterend", block)

    const tokenStatus = () => {
      const el = document.getElementById("analyticsTokenStatus")
      if (!el) return
      const d = describeToken(loadTok())
      el.textContent = d.text
      el.style.color = d.ok ? "var(--green)" : "var(--muted)"
    }

    const ta = document.getElementById("setting-analytics-token")
    ta.addEventListener("change", () => {
      const raw = ta.value.trim().replace(/^Bearer\s+/i, "")
      if (!raw) return
      saveTok({ accessToken: raw, expiresAt: jwtExpiry(raw) })
      // Never leave a bearer token sitting in a visible field.
      ta.value = ""
      tokenStatus()
    })
    document.getElementById("analyticsClearTokenBtn").addEventListener("click", () => {
      clearTok()
      ta.value = ""
      tokenStatus()
    })
    document.getElementById("analyticsCopyCurlBtn").addEventListener("click", async (e) => {
      const btn = e.currentTarget
      try {
        await navigator.clipboard.writeText(curlCommand(loadCfg()))
        btn.textContent = "Copied"
      } catch (_) {
        btn.textContent = "Copy failed"
      }
      setTimeout(() => (btn.textContent = "Copy token command"), 1600)
    })
    document
      .getElementById("setting-analytics-token-proxy")
      .addEventListener("change", (e) => {
        const c = loadCfg()
        c.tokenProxyUrl = e.target.value.trim()
        saveCfg(c)
      })
    tokenStatus()
  }

  // The Analytics panel only exists once the Settings modal is in the DOM, which
  // it is from the start — but the app rewrites parts of it, so injection is
  // retried on open rather than done once.
  function watchSettings() {
    injectSettingsUi()
    document.getElementById("settingsBtn")?.addEventListener("click", () => {
      setTimeout(injectSettingsUi, 0)
    })
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", watchSettings)
  } else {
    watchSettings()
  }

  // ── The electronAPI surface ────────────────────────────────────────────────

  window.__analyticsWeb = {
    /// Endpoint constants come from Rust so there is one copy of them.
    adoptEndpoints(e) {
      if (!e) return
      endpoints = { ...endpoints, ...e }
      if (Number.isFinite(e.tokenSkewSecs)) skewSecs = e.tokenSkewSecs
    },

    getConfig() {
      const c = loadCfg()
      const t = loadTok()
      return {
        // "Configured" in the browser means the request parameters plus a usable
        // token — there is no stored secret to count, and a complete set of
        // parameters with no token cannot fetch anything.
        configured: !!(c.customerKey && c.projectKey && c.culture && (t.accessToken || c.tokenProxyUrl)),
        hasSecret: false,
        clientId: "",
        customerKey: c.customerKey,
        projectKey: c.projectKey,
        culture: c.culture,
        environment: c.environment || "Production",
        activeSessionOnly: !!c.activeSessionOnly,
      }
    },

    saveConfig(args) {
      const c = loadCfg()
      // clientId/clientSecret are accepted and dropped: the renderer sends the
      // same object it sends the desktop app, and neither has a use here.
      if (args) {
        if (args.customerKey !== undefined) c.customerKey = String(args.customerKey || "").trim()
        if (args.projectKey !== undefined) c.projectKey = String(args.projectKey || "").trim()
        if (args.culture !== undefined) c.culture = String(args.culture || "").trim()
        if (args.environment !== undefined) c.environment = String(args.environment || "Production").trim()
        if (args.activeSessionOnly !== undefined) c.activeSessionOnly = !!args.activeSessionOnly
      }
      saveCfg(c)
      return this.getConfig()
    },

    /// The web counterpart of `test_analytics_connection`. The native command
    /// requests a token only; here a token is the thing we cannot request, so the
    /// equivalent smallest real check is a one-second window against the actual
    /// endpoint — which exercises the token, the keys and the culture together.
    async testConnection(call) {
      const c = loadCfg()
      if (!c.customerKey || !c.projectKey || !c.culture) {
        return { ok: false, message: "Fill in customer key, project key and culture first." }
      }
      let token
      try {
        token = await currentToken()
      } catch (e) {
        return { ok: false, message: `Token proxy failed: ${e.message}` }
      }
      if (!token) {
        return { ok: false, message: "No access token — paste one above, or set a token proxy." }
      }
      // One second, an hour ago: inside retention, trivially small, and still a
      // real authenticated request against the real project.
      const end = new Date(Date.now() - 3600 * 1000)
      const start = new Date(end.getTime() - 1000)
      const iso = (d) => d.toISOString().slice(0, 19) + "Z"
      try {
        const res = await call("analyticsFetchWindow", {
          startUtc: iso(start),
          endUtc: iso(end),
          cfg: c,
          token,
        })
        await call("analyticsCleanup", { paths: [res.tempPath] })
        return { ok: true, message: `Connected — the API answered with ${res.rowCount} rows.` }
      } catch (e) {
        const kind = e?.fetchError?.kind
        if (kind === "unauthorized") {
          return { ok: false, message: `${e.message} The token may have expired.` }
        }
        return { ok: false, message: e?.message || String(e) }
      }
    },

    /// `fetchAnalyticsWindow`, with the token resolved per call so a proxy can
    /// refresh mid-run and a paste can be swapped in without a reload.
    async fetchWindow(call, startUtc, endUtc) {
      const cfg = loadCfg()
      let token
      try {
        token = await currentToken()
      } catch (e) {
        throw { kind: "unauthorized", message: `Could not get a token: ${e.message}`, retryable: false }
      }
      return call("analyticsFetchWindow", { startUtc, endUtc, cfg, token })
    },
  }
})()
