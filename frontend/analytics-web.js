// Analytics API settings and token handling for the browser build.
//
// The desktop app keeps credentials in `app_data_dir()/analytics-api.json` at
// 0600 and mints its own OAuth2 token. A browser cannot do the second part, and
// the reason is specific enough to be worth writing down, because "the desktop
// manages it" is the obvious objection:
//
//   CORS is enforced by the *browser*, never by the server. The desktop build
//   uses `reqwest`, a native HTTP client where CORS does not exist as a concept,
//   so the identical request to the identical server succeeds there.
//
//   And Entra ID's refusal is deliberate, not incidental. Measured against the
//   live endpoint:
//
//     POST /oauth2/v2.0/token  grant_type=authorization_code  -> ACAO: *
//     POST /oauth2/v2.0/token  grant_type=client_credentials  -> no ACAO
//
//   Same URL, same host, opposite answers. `client_credentials` carries a client
//   *secret*, which a browser cannot hold safely, so it is refused cross-origin
//   at every origin — there is no configuration that changes this. The CM.com SOP
//   mandates exactly that grant. (The *data* endpoint sends `*`, which is why the
//   interaction log itself is readable from a page.)
//
// So the token has to come from somewhere that is not a browser. Two ways, in
// preference order:
//
//   1. **A token relay** (preferred, and the default in the UI). One file the
//      user uploads beside the app — see `tools/token-proxy/`. It holds the
//      secret, performs the one request a browser may not, and returns only the
//      short-lived token. Tokens then refresh on their own and there is nothing
//      to paste, ever.
//   2. **Paste a token** (fallback). For a host that cannot run any server-side
//      code at all. Tokens last ~24h per the SOP, so it is a once-a-day action.
//
// Either way the client secret never enters the browser, which is why this build
// has no client-secret field: in mode 1 it lives on the relay, in mode 2 it never
// leaves the user's terminal. That is strictly better than the desktop's local
// file, not a compromise.
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
    // Sent as `x-proxy-key`. Without it the relay URL is a public token vending
    // machine for the project — the path is guessable and it would mint a working
    // bearer token for anyone who asked.
    tokenProxyKey: "",
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

  /// Describes a cached token only. Whether the *absence* of one is a problem
  /// depends on the mechanism in use, so that judgement belongs to `tokenStatus`
  /// rather than here — this used to end in "paste one below", which read as an
  /// instruction even when a relay was set up and nothing needed pasting.
  function describeToken(t) {
    if (!t.accessToken) return { ok: false, text: "No token cached yet." }
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

  /// Mint a token through the user's relay. The relay holds the credentials; this
  /// side only ever sees the short-lived token it returns.
  async function tokenFromProxy(url, key) {
    const resp = await fetch(url, {
      method: "POST",
      headers: {
        Accept: "application/json",
        // Only sent when set. An empty custom header would still trigger a
        // preflight cross-origin while conveying nothing.
        ...(key ? { "x-proxy-key": key } : {}),
      },
      cache: "no-store",
    })
    const text = await resp.text()
    if (!resp.ok) {
      // 403 is overwhelmingly the shared key not matching, and saying so beats
      // making the user read a JSON body to find out.
      const hint =
        resp.status === 403
          ? " — check that the relay key here matches SHARED_KEY in the relay file"
          : ""
      throw new Error(`Token relay returned ${resp.status}${hint}: ${text.slice(0, 200)}`)
    }
    let body
    try {
      body = JSON.parse(text)
    } catch (_) {
      throw new Error(
        "The token relay did not return JSON. If the URL is right, the host may " +
          "be serving the file as plain text instead of executing it."
      )
    }
    const accessToken = body.access_token || body.accessToken
    if (!accessToken) throw new Error("The token relay returned no access_token")
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
    const { tokenProxyUrl, tokenProxyKey } = loadCfg()
    if (tokenProxyUrl) {
      t = await tokenFromProxy(tokenProxyUrl, tokenProxyKey)
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

  /// Set by `injectSettingsUi`, so opening Settings can re-read the token state
  /// without rebuilding the block. A token expires while the page stays open, and
  /// the status line is the only thing that says so.
  let refreshTokenStatus = () => {}

  function injectSettingsUi() {
    const grid = document.querySelector(".analytics-cfg-grid")
    if (!grid || document.getElementById("analyticsWebBlock")) {
      refreshTokenStatus()
      return
    }

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
        #analyticsWebBlock .analytics-web-label {
          display: block;
          margin-top: 12px;
        }
        #analyticsWebBlock .analytics-web-tag {
          display: inline-block;
          margin-left: 5px;
          padding: 1px 6px;
          border-radius: 3px;
          background: color-mix(in srgb, var(--green) 18%, transparent);
          color: var(--green);
          font-size: 0.62rem;
          font-weight: 600;
          letter-spacing: 0.02em;
          text-transform: none;
        }
      </style>
      <label class="analytics-web-label" for="setting-analytics-token-proxy"
        >Token relay <span class="analytics-web-tag">set this up once</span></label
      >
      <p class="hint" style="margin:2px 0 6px">
        Upload <code>cai-token.php</code> from <code>tools/token-proxy/</code> next
        to this app. It holds your client secret and mints tokens for you, so they
        refresh automatically and there is nothing to paste — ever. Full setup
        steps are in <code>tools/token-proxy/README.md</code>; a Cloudflare Worker
        version is included for hosts that cannot run PHP.
      </p>
      <p class="hint" style="margin:2px 0 6px">
        <strong>Relay key</strong> is a password <em>you</em> invent — not
        something CM.com gives you. Put the same value in the relay file's
        <code>SHARED_KEY</code> line and in the second field below, so only your
        browser can use the relay. Generate one with
        <code>openssl rand -base64 32</code>.
      </p>
      <div style="display:flex;gap:8px;flex-wrap:wrap">
        <input id="setting-analytics-token-proxy" type="text" autocomplete="off"
          spellcheck="false" placeholder="cai-token.php"
          value="${esc(cfg.tokenProxyUrl)}" style="flex:2 1 200px;min-width:0" />
        <input id="setting-analytics-token-key" type="password" autocomplete="new-password"
          spellcheck="false" placeholder="Relay key (SHARED_KEY)"
          value="${esc(cfg.tokenProxyKey)}" style="flex:1 1 150px;min-width:0" />
      </div>

      <details style="margin-top:10px">
        <summary class="hint" style="cursor:pointer">
          Why can't the browser get a token by itself?
        </summary>
        <div class="analytics-web-note" style="margin-top:6px">
          Microsoft answers the token endpoint cross-origin for browser sign-in
          flows, but refuses it for the client-secret flow the Analytics API
          requires — at every origin, with no setting that changes it. The desktop
          app is unaffected because CORS is a rule browsers enforce, not servers,
          and it uses a native HTTP client. So the secret has to live somewhere
          that is not a browser. The relay is the smallest such place: one file,
          and your secret never reaches this page.
        </div>
      </details>

      <details style="margin-top:6px">
        <summary class="hint" style="cursor:pointer">
          Can't run any server-side code? Paste a token manually instead
        </summary>
        <p class="hint" style="margin:6px 0">
          A last resort for a purely static host. Tokens last about 24 hours, so
          this has to be repeated roughly daily — set up the relay above instead
          wherever that is possible.
        </p>
        <textarea id="setting-analytics-token" rows="3" spellcheck="false"
          autocomplete="off" placeholder="Paste an access token (eyJ0eXAi…)"
          style="width:100%;font-family:var(--mono,monospace);font-size:11px"></textarea>
        <div style="display:flex;align-items:center;gap:10px;margin-top:6px;flex-wrap:wrap">
          <button class="btn-secondary" id="analyticsCopyCurlBtn" type="button">Copy token command</button>
          <button class="btn-secondary" id="analyticsClearTokenBtn" type="button">Clear token</button>
        </div>
      </details>
      <p class="hint" id="analyticsTokenStatus" style="margin-top:8px;min-height:1.1em"></p>
    `
    grid.insertAdjacentElement("afterend", block)

    /// Says which mechanism is in play, not merely whether a token is cached.
    ///
    /// Two things this must not do: report "no token" as a problem when a relay is
    /// configured (none has simply been fetched yet), and tell an unconfigured user
    /// to paste something. The relay is the setup step, so that is what an empty
    /// state points at.
    const tokenStatus = () => {
      const el = document.getElementById("analyticsTokenStatus")
      if (!el) return
      const c = loadCfg()
      const t = loadTok()
      const d = describeToken(t)

      if (c.tokenProxyUrl) {
        if (!c.tokenProxyKey) {
          el.textContent = "Relay set, but no relay key — the relay will refuse the request."
          el.style.color = "var(--orange)"
          return
        }
        el.textContent = t.accessToken
          ? `Relay active. ${d.text} Refreshes automatically.`
          : "Relay active. A token is fetched automatically on the first import."
        el.style.color = "var(--green)"
        return
      }
      if (t.accessToken) {
        el.textContent = `Using a manually pasted token. ${d.text} Set up a relay above to stop repeating this.`
        el.style.color = d.ok ? "var(--muted)" : "var(--orange)"
        return
      }
      el.textContent = "Not set up yet — add a token relay above."
      el.style.color = "var(--muted)"
    }
    refreshTokenStatus = tokenStatus

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
        tokenStatus()
      })
    document
      .getElementById("setting-analytics-token-key")
      .addEventListener("change", (e) => {
        const c = loadCfg()
        c.tokenProxyKey = e.target.value
        saveCfg(c)
        tokenStatus()
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
        return {
          ok: false,
          message:
            "No access token. Set a token relay URL and key above (recommended), " +
            "or paste a token under the fallback option.",
        }
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
