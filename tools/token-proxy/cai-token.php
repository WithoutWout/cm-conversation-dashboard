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
 * A password of your own invention, also entered in the app's Settings.
 *
 * This is not optional. Without it the URL is a public token vending machine for
 * your CM.com project — the path is guessable and the endpoint would mint a
 * working bearer token for anyone who asked.
 */
const SHARED_KEY = 'PUT-A-LONG-RANDOM-STRING-HERE';

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
 * string, and an earlier version compared `SHARED_KEY` against that same literal
 * — so the replace rewrote the guard too, and every request was rejected with a
 * "Forbidden" that pointed at the key rather than at the real cause. Any
 * replacement of the value necessarily removes the prefix, so this cannot repeat.
 */
function is_placeholder(string $value): bool
{
    return $value === '' || str_starts_with($value, 'PUT-');
}

// Fail loudly on a half-configured relay, rather than forwarding placeholders to
// Microsoft and reporting its "unauthorized_client" as if the credentials were
// merely wrong.
foreach (['CLIENT_ID' => CLIENT_ID, 'CLIENT_SECRET' => CLIENT_SECRET, 'SHARED_KEY' => SHARED_KEY] as $name => $value) {
    if (is_placeholder($value)) {
        http_response_code(500);
        echo json_encode(['error' => "Relay is not configured: $name is still a placeholder."]);
        exit;
    }
}

$provided = $_SERVER['HTTP_X_PROXY_KEY'] ?? '';
// hash_equals, not ===, so a wrong key cannot be discovered a byte at a time.
if (!hash_equals(SHARED_KEY, $provided)) {
    http_response_code(403);
    echo json_encode(['error' => 'Forbidden: missing or incorrect proxy key.']);
    exit;
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
