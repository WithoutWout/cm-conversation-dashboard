# Token relay — setup

One file you upload once. After that the Analytics API import works on its own:
tokens are fetched and refreshed automatically, and there is nothing to paste.

## Why this file has to exist

The Analytics API needs two requests. A browser can make one of them and not the
other, and that is not a limitation of this app:

| Request | Can a browser do it? |
| --- | --- |
| `GET analytics.digitalcx.com/…/interactions` | **yes** — the host sends `Access-Control-Allow-Origin: *` |
| `POST login.microsoftonline.com/…/oauth2/token` with `grant_type=client_credentials` | **no** — no CORS headers, at any origin |

Microsoft *does* allow the token endpoint cross-origin for browser sign-in flows,
and deliberately refuses it for the client-secret flow the Analytics API requires
— a browser cannot hold a secret safely. No setting changes this.

The desktop app is unaffected because CORS is a rule **browsers** enforce, not
servers, and it uses a native HTTP client.

So the client secret has to live somewhere that is not a browser. This is the
smallest such place: it holds the secret, makes the one request a browser may not,
and hands back only the short-lived token.

---

## What you need

Three values. **Two come from CM.com, one you make up yourself.**

| Value | Where it comes from |
| --- | --- |
| `CLIENT_ID` | CM.com — the same one the desktop app uses |
| `CLIENT_SECRET` | CM.com — the same one the desktop app uses |
| `SHARED_KEY` | **You invent it.** See below. |

### The shared key, specifically

It is a password **you choose yourself**. Nobody issues it, and it has nothing to
do with your CM.com credentials.

It exists because the relay's address is guessable — if the app lives at
`https://example.com/dashboard/`, then the relay is at
`https://example.com/dashboard/cai-token.php`. Without a key, anyone who
guessed that URL could make it mint working bearer tokens for your CM.com
project. The key means only your browser can use it.

You write the same value in **two places**, and they must match exactly:

1. in `cai-token.php`, on the `SHARED_KEY` line
2. in the app, in the **Relay key (SHARED_KEY)** field

Generate one with any of these, or use a password manager:

```bash
openssl rand -base64 32
```

Length is what matters, not cleverness — 30+ random characters. Avoid `'`
characters, since the value sits inside single quotes in the PHP file.

---

## Setup, step by step

### 1. Edit the file

Open `cai-token.php` and replace the three placeholder values. They are near the
top, at lines 38, 39 and 48:

```php
const CLIENT_ID     = 'PUT-YOUR-CLIENT-ID-HERE';
const CLIENT_SECRET = 'PUT-YOUR-CLIENT-SECRET-HERE';
const SHARED_KEY    = 'PUT-A-LONG-RANDOM-STRING-HERE';
```

becomes, for example:

```php
const CLIENT_ID     = '8f3c1e42-...';
const CLIENT_SECRET = 'abc123~...';
const SHARED_KEY    = 'Qk9pQm5UdGhpc0lzUmFuZG9tMzJCeXRlcw==';
```

Leave `ALLOWED_ORIGIN` empty — you only need it if the relay lives on a
*different* domain than the app.

### 2. Upload it

Put it in the **same folder as `index.html`**, alongside the other files from
`dist-web/`.

> Upload it separately, by hand. It is deliberately not part of `dist-web/`,
> because you upload that folder wholesale on every update — a template copy in
> there would overwrite your configured file each time.

### 3. Check it works, before touching the app

```bash
curl -s -X POST -H 'x-proxy-key: YOUR_SHARED_KEY' https://YOUR-SITE/cai-token.php
```

A working relay answers with a token:

```json
{"access_token":"eyJ0eXAiOiJKV1Qi...","expires_in":86399}
```

### 4. Enter it in the app

**Settings** (gear, top right) → **Conversations** tab → scroll to **Token relay**:

| Field | Value |
| --- | --- |
| first field | `cai-token.php` — just the filename, if it sits next to `index.html`. A full URL also works. |
| **Relay key (SHARED_KEY)** | the same shared key you put in the file |

The line underneath should turn green and read *"Relay active. A token is fetched
automatically on the first import."*

Then click **Test connection** to confirm the whole chain end to end.

---

## If something goes wrong

The message tells you which step failed.

| What you see | What it means |
| --- | --- |
| `Relay is not configured: CLIENT_ID is still a placeholder.` | One of the three values wasn't replaced. The message names which. |
| `403` / `Forbidden: missing or incorrect proxy key.` | The key in the app doesn't match the one in the file. Check for a trailing space or a partial copy. |
| `Relay set, but no relay key` | The **Relay key** field in Settings is empty. |
| `The token relay did not return JSON` | The host is serving the file as plain text instead of running it — PHP isn't enabled for that folder. Use the Cloudflare Worker version instead. |
| `AADSTS700016: Application with identifier '…' was not found` | `CLIENT_ID` is wrong. |
| `AADSTS7000215: Invalid client secret provided` | `CLIENT_SECRET` is wrong or expired. |
| `404` | Wrong path. Open `https://YOUR-SITE/cai-token.php` in a browser — you should get JSON (an error is fine), not your host's 404 page. |

---

## Security notes

- **The shared key is not a CM.com credential.** Losing it lets someone mint
  tokens for your project until you change it; it does not expose your secret.
  To rotate it, change both places.
- **The client secret never reaches the browser.** Only the short-lived token
  does. That is the point, and it is stricter than the desktop app, which keeps
  the secret in a local file.
- **Don't commit your configured copy** to a public repository — it contains the
  secret. The template here is safe to commit; your filled-in copy is not.
- The relay only ever accepts `POST`, only answers with a token or an error, and
  compares the key with `hash_equals` so it cannot be guessed a byte at a time.

---

## No PHP on your host?

Use `cloudflare-worker.js` instead — same idea, runs on Cloudflare's free tier,
no server of your own. Its header comment has the deploy steps. Because it lives
on a different domain than the app, that one *does* need `ALLOWED_ORIGIN` set to
the exact origin serving the dashboard.

## Neither is possible?

Then Settings → **Can't run any server-side code? Paste a token manually instead**
is the fallback: mint a token in your terminal and paste it. It works, but tokens
last about 24 hours, so it has to be repeated roughly daily.
