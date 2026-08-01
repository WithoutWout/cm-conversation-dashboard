<?php
/**
 * Token relay for the CAI Dashboard web build.
 *
 * WHY THIS FILE EXISTS
 * --------------------
 * The Analytics API's data endpoint is readable straight from a browser — it
 * sends `Access-Control-Allow-Origin: *`. Its *token* endpoint is not, and that
 * is deliberate on Microsoft's part rather than an oversight:
 *
 *   POST /oauth2/v2.0/token, grant_type=authorization_code  -> ACAO: *   (allowed)
 *   POST /oauth2/v2.0/token, grant_type=client_credentials   -> no ACAO   (refused)
 *
 * `client_credentials` carries a client *secret*, and a browser cannot hold one
 * safely, so Entra ID will not answer that grant cross-origin at any origin. The
 * CM.com SOP mandates exactly that grant. The desktop build is unaffected because
 * it uses a native HTTP client, where CORS does not exist — CORS is enforced by
 * the browser, never by the server.
 *
 * So this is the smallest possible piece of server code that removes the manual
 * token paste: it holds the secret, performs the one request a browser may not,
 * and returns only the resulting short-lived token.
 *
 * INSTALL
 * -------
 * 1. Fill in the four constants below.
 * 2. Upload next to index.html (same folder). Same-origin is the point: no CORS
 *    headers are needed and no preflight happens on a simple request.
 * 3. In the app: Settings -> Analytics API -> Token relay URL = "cai-token.php",
 *    and paste the same SHARED_KEY.
 *
 * Requires PHP 7.4+ with curl. Nothing else, no composer, no build step.
 */
declare(strict_types=1);

// ── Configure ───────────────────────────────────────────────────────────────
/** Provided by CM.com. */
const CLIENT_ID     = 'PUT-YOUR-CLIENT-ID-HERE';
const CLIENT_SECRET = 'PUT-YOUR-CLIENT-SECRET-HERE';

/**
 * OPTIONAL, and empty is the normal setting. Leave it alone unless you have a
 * specific reason.
 *
 * With it empty, the relay accepts requests that the *browser itself* marks as
 * coming from a page on this same origin (see the `Sec-Fetch-Site` check below).
 * Nobody using the dashboard has to know or enter anything — which is the point,
 * because a shared key would have to be handed to every person and typed into
 * every browser that uses the app.
 *
 * Set it only if you want a second lock on top of that, and understand that every
 * user then needs the same value entered in
 * Settings -> Conversations -> Token relay -> "Relay key". Generate one with
 * `openssl rand -base64 32`; avoid apostrophes, since it sits in single quotes.
 */
const SHARED_KEY = '';

/**
 * Leave empty when this file sits beside index.html (the normal case).
 *
 * Only set it if the app is served from a *different* origin than this file, in
 * which case put that origin here exactly, e.g. 'https://dashboard.example.com'.
 * Never '*': that would let any page on the internet spend your shared key if it
 * ever leaked.
 */
const ALLOWED_ORIGIN = '';

// ── Endpoint (from the SOP; keep in step with analytics_api.rs) ─────────────
const TOKEN_URL      = 'https://login.microsoftonline.com/digitalcx.onmicrosoft.com/oauth2/token';
const TOKEN_RESOURCE = 'https://digitalcx.onmicrosoft.com/external-api';

// ── Relay ───────────────────────────────────────────────────────────────────
header('Content-Type: application/json');
// A token must never be cached by a proxy or the browser.
header('Cache-Control: no-store, no-cache, must-revalidate');
header('X-Content-Type-Options: nosniff');

if (ALLOWED_ORIGIN !== '') {
    header('Access-Control-Allow-Origin: ' . ALLOWED_ORIGIN);
    header('Vary: Origin');
    header('Access-Control-Allow-Headers: x-proxy-key, content-type');
    header('Access-Control-Allow-Methods: POST, OPTIONS');
}

$method = $_SERVER['REQUEST_METHOD'] ?? 'GET';

// The custom x-proxy-key header makes this a preflighted request when the app is
// cross-origin, so OPTIONS has to be answered before anything else.
if ($method === 'OPTIONS') {
    http_response_code(ALLOWED_ORIGIN === '' ? 405 : 204);
    exit;
}
if ($method !== 'POST') {
    http_response_code(405);
    echo json_encode(['error' => 'Use POST.']);
    exit;
}

/**
 * Still a placeholder? Detected by the `PUT-` prefix rather than by comparing
 * against the placeholder text.
 *
 * The obvious way to configure this file is a find-and-replace of the placeholder
 * string, and an earlier version compared a constant against that same literal —
 * so the replace rewrote the guard too and every request was rejected with a
 * misleading error. Any replacement of the value necessarily removes the prefix.
 */
function is_placeholder(string $value): bool
{
    return $value === '' || str_starts_with($value, 'PUT-');
}

// Only the two CM.com values are required. SHARED_KEY is deliberately optional.
foreach (['CLIENT_ID' => CLIENT_ID, 'CLIENT_SECRET' => CLIENT_SECRET] as $name => $value) {
    if (is_placeholder($value)) {
        http_response_code(500);
        echo json_encode(['error' => "Relay is not configured: $name is still a placeholder."]);
        exit;
    }
}

/**
 * The default protection, and the reason no password is needed.
 *
 * `Sec-Fetch-Site` is set by the browser and cannot be set by page JavaScript, so
 * `same-origin` means the request genuinely came from a page served from this same
 * host — i.e. from the dashboard. A page on another domain gets `cross-site`, and
 * a bare `curl` or crawler sends no such header at all. Both are refused.
 *
 * Combined with sending no `Access-Control-Allow-Origin` (the same-origin default
 * below), a page on another domain also cannot *read* a response even if it made
 * the request, so a cross-origin web attacker cannot lift the token.
 *
 * What this does not stop is someone who knows this URL and forges the header from
 * a script. It is a lock on the door, not a guard: the dashboard's own access
 * control is what actually keeps strangers out, since anyone who can load the app
 * can use the relay by design. See README.md, "How protected is this?".
 */
$fetchSite = $_SERVER['HTTP_SEC_FETCH_SITE'] ?? '';
if ($fetchSite === '') {
    // Older browsers don't send Sec-Fetch-Site. Fall back to comparing the host
    // that the request claims to come from against the host serving this file.
    $claimed = $_SERVER['HTTP_ORIGIN'] ?? $_SERVER['HTTP_REFERER'] ?? '';
    $claimedHost = $claimed === '' ? '' : (parse_url($claimed, PHP_URL_HOST) ?? '');
    $selfHost = parse_url('http://' . ($_SERVER['HTTP_HOST'] ?? ''), PHP_URL_HOST) ?? '';
    $sameOrigin = $claimedHost !== '' && strcasecmp($claimedHost, $selfHost) === 0;
} else {
    $sameOrigin = $fetchSite === 'same-origin';
}

// ALLOWED_ORIGIN being set means the app is deliberately on another origin, so a
// cross-site request is expected — that case is gated by the CORS headers above
// plus the Origin check here instead.
if (ALLOWED_ORIGIN !== '') {
    $origin = $_SERVER['HTTP_ORIGIN'] ?? '';
    $sameOrigin = $origin !== '' && strcasecmp($origin, ALLOWED_ORIGIN) === 0;
}

if (!$sameOrigin) {
    http_response_code(403);
    echo json_encode([
        'error' => 'Forbidden: this relay only answers the dashboard served from the same site.',
    ]);
    exit;
}

// The optional second lock. Skipped entirely when SHARED_KEY is empty, which is
// the normal configuration.
if (SHARED_KEY !== '') {
    $provided = $_SERVER['HTTP_X_PROXY_KEY'] ?? '';
    // hash_equals, not ===, so a wrong key cannot be discovered a byte at a time.
    if (!hash_equals(SHARED_KEY, $provided)) {
        http_response_code(403);
        echo json_encode(['error' => 'Forbidden: missing or incorrect relay key.']);
        exit;
    }
}

$ch = curl_init(TOKEN_URL);
curl_setopt_array($ch, [
    CURLOPT_POST           => true,
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_TIMEOUT        => 30,
    CURLOPT_HTTPHEADER     => ['Content-Type: application/x-www-form-urlencoded'],
    CURLOPT_POSTFIELDS     => http_build_query([
        'grant_type'    => 'client_credentials',
        'client_id'     => CLIENT_ID,
        'client_secret' => CLIENT_SECRET,
        'resource'      => TOKEN_RESOURCE,
    ]),
]);
$body   = curl_exec($ch);
$status = (int) curl_getinfo($ch, CURLINFO_RESPONSE_CODE);
$err    = curl_error($ch);
curl_close($ch);

if ($body === false) {
    http_response_code(502);
    echo json_encode(['error' => 'Token request failed: ' . $err]);
    exit;
}

$parsed = json_decode($body, true);
if (!is_array($parsed) || !isset($parsed['access_token'])) {
    // Pass the upstream status through, and only a short excerpt of the body —
    // enough to diagnose a wrong client id without echoing an unbounded response.
    http_response_code($status >= 400 ? $status : 502);
    echo json_encode([
        'error'  => 'No access_token in the response.',
        'status' => $status,
        'detail' => mb_substr((string) $body, 0, 300),
    ]);
    exit;
}

// Only the token and its lifetime go back. Not the secret, not the raw response.
echo json_encode([
    'access_token' => $parsed['access_token'],
    'expires_in'   => isset($parsed['expires_in']) ? (int) $parsed['expires_in'] : 0,
]);
