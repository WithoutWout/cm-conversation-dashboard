# Token relay — setup

One file you upload once. After that the Analytics API import works on its own:
tokens are fetched and refreshed automatically, there is nothing to paste, and
**nobody using the dashboard needs a password.**

You edit **two values**, both of which CM.com gave you: your client ID and your
client secret. That's it.

## Why this file has to exist

The Analytics API needs two requests. A browser can make one and not the other,
and that is not a limitation of this app:

| Request | Can a browser do it? |
| --- | --- |
| `GET analytics.digitalcx.com/…/interactions` | **yes** — the host sends `Access-Control-Allow-Origin: *` |
| `POST login.microsoftonline.com/…/oauth2/token` with `grant_type=client_credentials` | **no** — no CORS headers, at any origin |

Microsoft *does* allow the token endpoint cross-origin for browser sign-in flows,
and deliberately refuses it for the client-secret flow the Analytics API requires —
a browser cannot hold a secret safely. No setting changes this.

The desktop app is unaffected because CORS is a rule **browsers** enforce, not
servers, and it uses a native HTTP client.

So the client secret has to live somewhere that is not a browser. This is the
smallest such place: it holds the secret, makes the one request a browser may not,
and hands back only the short-lived token.

---

## Setup

### 1. Edit two lines

Open `cai-token.php` and replace the two placeholders near the top:

```php
const CLIENT_ID     = 'PUT-YOUR-CLIENT-ID-HERE';
const CLIENT_SECRET = 'PUT-YOUR-CLIENT-SECRET-HERE';
```

Both come from CM.com — the same pair the desktop app uses. Leave `SHARED_KEY`
and `ALLOWED_ORIGIN` empty.

### 2. Upload it

Put it in the **same folder as `index.html`**, alongside the other files from
`dist-web/`.

> Upload it separately, by hand. It is deliberately not part of `dist-web/`,
> because you upload that folder wholesale on every update — a template copy in
> there would overwrite your configured file each time.

### 3. Enter the filename in the app

**Settings** (gear, top right) → **Conversations** tab → **Token relay** → type:

```
cai-token.php
```

Just the filename, if it sits next to `index.html`. A full URL works too.

The line underneath should turn green: *"Relay active. A token is fetched
automatically on the first import."* Click **Test connection** to confirm the
whole chain.

That's the whole setup. Every person who opens the dashboard gets working imports
with nothing to configure.

---

## How protected is this?

There is no password, by design — one would have to be handed to every person and
typed into every browser that uses the dashboard. Instead the relay refuses
anything that did not come from a page on your own site:

- **`Sec-Fetch-Site: same-origin`** is set by the browser and cannot be forged by
  page JavaScript. A page on another domain gets `cross-site`; a bare `curl` or a
  crawler sends no such header at all. Both are refused with a `403`. (Older
  browsers that don't send it fall back to an `Origin`/`Referer` host comparison.)
- **No CORS headers are sent**, so even if another site made the request, the
  browser would not let it *read* the response. A cross-origin web page cannot
  lift the token.

**What this does not stop:** someone who knows the URL and forges the header from a
script. So be clear about what is actually protecting your data:

> Anyone who can load the dashboard can import conversation data — that is the
> point of it. So the dashboard's own access control *is* the access control.
> If the app is on a public URL with no login, treat the conversation logs as
> public too, relay or no relay.

If the dashboard is behind HTTP basic auth, an IP allowlist, a VPN, or an intranet,
the relay sits behind exactly the same protection, because it is on the same host.

### If you do want a second lock

Set `SHARED_KEY` in the relay file to a long random string
(`openssl rand -base64 32`), then enter the same value in the app under
**Token relay → Relay key**. It is off by default because every browser that uses
the dashboard then needs that value entered, which is a real cost for a shared
deployment.

---

## If something goes wrong

| What you see | What it means |
| --- | --- |
| `Relay is not configured: CLIENT_ID is still a placeholder.` | One of the two values wasn't replaced. The message names which. |
| `403` / `only answers the dashboard served from the same site` | The request didn't look like it came from your dashboard. Normal if you tested with `curl`; from the app it means the relay is on a different domain than the app — set `ALLOWED_ORIGIN` to the app's origin. |
| `403` / `missing or incorrect relay key` | You set `SHARED_KEY` in the file but not in the app (or they differ). |
| `The token relay did not return JSON` | The host is serving the file as text instead of running it — PHP isn't enabled for that folder. Use the Cloudflare Worker version. |
| `AADSTS700016: Application with identifier '…' was not found` | `CLIENT_ID` is wrong. |
| `AADSTS7000215: Invalid client secret provided` | `CLIENT_SECRET` is wrong or expired. |
| `404` | Wrong path. Open `https://YOUR-SITE/cai-token.php` in a browser — you should get JSON (a `403` is expected and fine), not your host's 404 page. |

To test from a terminal you have to imitate the browser, since a bare request is
refused on purpose:

```bash
curl -s -X POST -H 'Sec-Fetch-Site: same-origin' https://YOUR-SITE/cai-token.php
```

A working relay answers `{"access_token":"eyJ0eXAi…","expires_in":86399}`.

---

## Security notes

- **The client secret never reaches the browser.** Only the short-lived token
  does. That is stricter than the desktop app, which keeps the secret in a local
  file.
- **Don't commit your configured copy** to a public repository — it contains the
  secret. The template here is safe to commit; your filled-in copy is not.
- If your host lets you, keep the file non-world-readable at the filesystem level
  (`chmod 600` where the web server runs as your user). It only matters against
  other accounts on shared hosting.
- The relay only ever accepts `POST` and only ever answers with a token or an
  error.

---

## No PHP on your host?

Use `cloudflare-worker.js` — same idea, runs on Cloudflare's free tier, no server
of your own. Its header comment has the deploy steps.

One difference: because it lives on a *different* domain than the app, it cannot
use the same-origin check, so `ALLOWED_ORIGIN` **must** be set to the exact origin
serving the dashboard. With it unset the worker refuses everything rather than
defaulting to open. Still no password.

## Neither is possible?

Then Settings → **Can't run any server-side code? Paste a token manually instead**
is the fallback: mint a token in your terminal and paste it. It works, but tokens
last about 24 hours, so it has to be repeated roughly daily.
