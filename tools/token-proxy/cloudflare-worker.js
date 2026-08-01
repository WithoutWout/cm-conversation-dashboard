// Token relay for the CAI Dashboard web build — Cloudflare Worker variant.
//
// Use this when your web host cannot run server-side code at all (a pure static
// bucket, GitHub Pages, S3). It needs no server of your own and fits inside
// Cloudflare's free tier. See cai-token.php for the PHP version and for why a
// relay is needed at all — in short, Entra ID answers `authorization_code`
// cross-origin but refuses `client_credentials`, because that grant carries a
// client secret and a browser cannot hold one safely. The SOP mandates
// `client_credentials`.
//
// DEPLOY
//   1. npx wrangler init cai-token   (or paste this into the dashboard editor)
//   2. Set the two CM.com secrets — never put them in wrangler.toml:
//        npx wrangler secret put CM_CLIENT_ID
//        npx wrangler secret put CM_CLIENT_SECRET
//   3. Set ALLOWED_ORIGIN as a plain var to the exact origin serving the app,
//      e.g. https://dashboard.example.com
//   4. In the app: Settings → Conversations → Token relay = the worker URL.
//
// No password anywhere: ALLOWED_ORIGIN is what restricts this, and nobody using
// the dashboard has to know or enter anything.
//
// ALLOWED_ORIGIN is REQUIRED here, unlike in the PHP version. This worker is on a
// different origin than the app, so it cannot use the PHP file's same-origin
// check; the Origin header is the only signal it has. With it unset the worker
// refuses everything rather than defaulting to open.
//
// SHARED_KEY is optional. Set it as a third secret only if you want a second lock,
// and note that every browser using the dashboard then needs the same value
// entered in Settings.

const TOKEN_URL =
  "https://login.microsoftonline.com/digitalcx.onmicrosoft.com/oauth2/token"
const TOKEN_RESOURCE = "https://digitalcx.onmicrosoft.com/external-api"

/// Echoed only for the one configured origin. Never `*`: with `*` any page on the
/// internet could spend the shared key if it ever leaked.
function corsHeaders(env) {
  return {
    "Access-Control-Allow-Origin": env.ALLOWED_ORIGIN || "null",
    Vary: "Origin",
    "Access-Control-Allow-Headers": "x-proxy-key, content-type",
    "Access-Control-Allow-Methods": "POST, OPTIONS",
  }
}

const json = (body, status, env) =>
  new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
      ...corsHeaders(env),
    },
  })

export default {
  async fetch(request, env) {
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: corsHeaders(env) })
    }
    if (request.method !== "POST") {
      return json({ error: "Use POST." }, 405, env)
    }
    if (!env.CM_CLIENT_ID || !env.CM_CLIENT_SECRET) {
      return json({ error: "Relay is not configured (missing CM.com secrets)." }, 500, env)
    }
    // Refuse rather than default to open: without a configured origin this worker
    // has no way at all to tell the dashboard from anyone else.
    if (!env.ALLOWED_ORIGIN) {
      return json({ error: "Relay is not configured (ALLOWED_ORIGIN is not set)." }, 500, env)
    }

    // The default protection, and why no password is needed. The browser sets
    // Origin and page JavaScript cannot forge it, so this is what restricts the
    // worker to the dashboard's own origin.
    const origin = request.headers.get("origin") || ""
    if (origin.toLowerCase() !== env.ALLOWED_ORIGIN.toLowerCase()) {
      return json(
        { error: "Forbidden: this relay only answers the configured dashboard origin." },
        403,
        env
      )
    }

    // Optional second lock; skipped entirely when SHARED_KEY is unset.
    if (env.SHARED_KEY) {
      const provided = request.headers.get("x-proxy-key") || ""
      if (!timingSafeEqual(provided, env.SHARED_KEY)) {
        return json({ error: "Forbidden: missing or incorrect relay key." }, 403, env)
      }
    }

    let upstream
    try {
      upstream = await fetch(TOKEN_URL, {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          grant_type: "client_credentials",
          client_id: env.CM_CLIENT_ID,
          client_secret: env.CM_CLIENT_SECRET,
          resource: TOKEN_RESOURCE,
        }),
      })
    } catch (e) {
      return json({ error: `Token request failed: ${e.message}` }, 502, env)
    }

    const text = await upstream.text()
    let parsed
    try {
      parsed = JSON.parse(text)
    } catch (_) {
      parsed = null
    }
    if (!parsed?.access_token) {
      return json(
        {
          error: "No access_token in the response.",
          status: upstream.status,
          detail: text.slice(0, 300),
        },
        upstream.status >= 400 ? upstream.status : 502,
        env
      )
    }

    // Only the token and its lifetime leave the relay.
    return json(
      {
        access_token: parsed.access_token,
        expires_in: Number(parsed.expires_in) || 0,
      },
      200,
      env
    )
  },
}

/// Length is intentionally compared first and separately: the lengths are not
/// secret, and `crypto.subtle.timingSafeEqual` requires equal-sized buffers.
function timingSafeEqual(a, b) {
  const ea = new TextEncoder().encode(a)
  const eb = new TextEncoder().encode(b)
  if (ea.length !== eb.length) return false
  let diff = 0
  for (let i = 0; i < ea.length; i++) diff |= ea[i] ^ eb[i]
  return diff === 0
}
