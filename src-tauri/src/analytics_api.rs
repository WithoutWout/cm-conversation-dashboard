//! CM.com Analytics API client — interaction log export.
//!
//! Implements the flow documented in `CM_Analytics_API_SOP.md`: an OAuth2
//! client-credentials token (reused until it expires) followed by GETs against
//! the interactions endpoint. Each response is validated and streamed to a temp
//! CSV that the existing `import_interactions_csv` command then imports, so the
//! API path and the manual portal-download path share one import pipeline.
//!
//! Two constraints are enforced here rather than left to callers: only one API
//! request may be in flight at a time (`fetch_lock`), and a single request may
//! never span 24 hours or more.
//!
//! Neither comes from the SOP, despite what earlier comments here claimed. The
//! SOP documents no rate limit and no concurrency limit; single-flight is our
//! own politeness cap. The 24-hour rule is our invariant too — the SOP says a
//! full-day request *frequently times out*, not that it is rejected — and it
//! exists so every window maps 1:1 onto a UTC day the importer can reason about.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TOKEN_URL: &str = "https://login.microsoftonline.com/digitalcx.onmicrosoft.com/oauth2/token";
const TOKEN_RESOURCE: &str = "https://digitalcx.onmicrosoft.com/external-api";
const API_BASE: &str = "https://analytics.digitalcx.com";

/// Subdirectory of the app cache dir holding in-flight downloads.
pub const TEMP_DIR_NAME: &str = "analytics-tmp";
const CONFIG_FILE_NAME: &str = "analytics-api.json";

/// A single request must stay strictly under 24 hours (`00:00:00`–`23:59:59`).
///
/// Our own invariant, not an API rule: it keeps every window inside one UTC day
/// so a request maps 1:1 onto a day the calendar and the hour-coverage skip
/// logic can reason about.
const MAX_WINDOW_SECS: i64 = 24 * 3600;
/// SOP: data can only be queried up to 90 days back.
const RETENTION_DAYS: i64 = 90;

const TOKEN_TIMEOUT_SECS: u64 = 30;
const FETCH_TIMEOUT_SECS: u64 = 300;
/// Treat a token as expired this long before it actually is.
///
/// **It must exceed `FETCH_TIMEOUT_SECS`**, and that is the whole point. A
/// request is authorised once, when it is sent; if the token dies while the
/// server is still producing a multi-megabyte CSV, what comes back is not a
/// clean `401` we could refresh and retry — it is a reset connection or a
/// truncated body, which classifies as `Network` and is not retryable. The day
/// then failed, and pressing Retry worked, because by then the token was stale
/// enough to be replaced up front. The skew was 120s against a 300s request
/// timeout, so any token with 121–300s left started a request it could not
/// finish. See `a_token_is_never_handed_out_with_less_life_than_a_fetch_needs`.
const TOKEN_SKEW_SECS: u64 = FETCH_TIMEOUT_SECS + 60;

// ── Config ───────────────────────────────────────────────────────────────────

// `PartialEq` so a settings backup can tell an untouched config from a real one
// and skip writing a block of empty strings — see `export_settings_backup`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct AnalyticsConfig {
    pub client_id: String,
    pub client_secret: String,
    pub customer_key: String,
    pub project_key: String,
    pub culture: String,
    pub environment: String,
    pub active_session_only: bool,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            customer_key: String::new(),
            project_key: String::new(),
            culture: String::new(),
            environment: "Production".into(),
            active_session_only: false,
        }
    }
}

impl AnalyticsConfig {
    /// Every field the API needs is present.
    pub fn is_complete(&self) -> bool {
        !self.client_id.trim().is_empty()
            && !self.client_secret.is_empty()
            && !self.customer_key.trim().is_empty()
            && !self.project_key.trim().is_empty()
            && !self.culture.trim().is_empty()
            && !self.environment.trim().is_empty()
    }
}

/// What the renderer is allowed to see — never includes the client secret.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsConfigView {
    pub configured: bool,
    pub has_secret: bool,
    pub client_id: String,
    pub customer_key: String,
    pub project_key: String,
    pub culture: String,
    pub environment: String,
    pub active_session_only: bool,
}

impl From<&AnalyticsConfig> for AnalyticsConfigView {
    fn from(c: &AnalyticsConfig) -> Self {
        Self {
            configured: c.is_complete(),
            has_secret: !c.client_secret.is_empty(),
            client_id: c.client_id.clone(),
            customer_key: c.customer_key.clone(),
            project_key: c.project_key.clone(),
            culture: c.culture.clone(),
            environment: c.environment.clone(),
            active_session_only: c.active_session_only,
        }
    }
}

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONFIG_FILE_NAME)
}

pub fn load_config(data_dir: &Path) -> AnalyticsConfig {
    match std::fs::read_to_string(config_path(data_dir)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => AnalyticsConfig::default(),
    }
}

pub fn save_config(data_dir: &Path, cfg: &AnalyticsConfig) -> Result<(), String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("Cannot create config directory: {e}"))?;
    let path = config_path(data_dir);
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("Cannot write config: {e}"))?;
    // The file holds a client secret — keep it owner-readable only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ── Shared state ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct CachedToken {
    access_token: String,
    /// Wall-clock second the server said the token dies.
    expires_at: u64,
    /// Wall-clock second we stop handing it out — `expires_at` minus the skew,
    /// floored so a short-lived token is still usable at all. Precomputed so
    /// the read path is a comparison rather than a policy decision.
    usable_until: u64,
}

pub struct AnalyticsState {
    token: Mutex<CachedToken>,
    /// Caps in-flight API requests at one. Self-imposed politeness, not an API
    /// rule — the SOP documents no rate or concurrency limit. The renderer's
    /// scheduler already serialises downloads; this makes it impossible for a
    /// scheduler bug to produce concurrent calls.
    fetch_lock: tokio::sync::Semaphore,
    temp_counter: AtomicU64,
    /// One client for the whole process, so TLS handshakes and connections are
    /// reused across a 90-day import. Built once because `reqwest::Client`
    /// *is* the connection pool — constructing a fresh one per request threw
    /// the pool away every time and paid a full handshake per window.
    client: OnceLock<reqwest::Client>,
}

impl Default for AnalyticsState {
    fn default() -> Self {
        Self {
            token: Mutex::new(CachedToken::default()),
            fetch_lock: tokio::sync::Semaphore::new(1),
            temp_counter: AtomicU64::new(0),
            client: OnceLock::new(),
        }
    }
}

// ── Diagnostics ──────────────────────────────────────────────────────────────

/// A step-by-step account of one `fetch_window` call, returned to the renderer
/// on success *and* on failure and appended verbatim to the import modal's
/// Details log.
///
/// The Rust `log` output already records all of this, but it lands in a file
/// most users will never find — and a failed overnight import on someone
/// else's Windows machine is exactly the case that has to be diagnosable from
/// what is on screen. Every line goes to both places.
#[derive(Default)]
pub struct Trace {
    lines: Vec<String>,
}

impl Trace {
    fn push(&mut self, line: impl Into<String>) {
        let line = line.into();
        log::info!(target: "analytics", "{line}");
        self.lines.push(line);
    }

    fn warn(&mut self, line: impl Into<String>) {
        let line = line.into();
        log::warn!(target: "analytics", "{line}");
        self.lines.push(line);
    }

    pub fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }
}

/// Show enough of a key to tell two projects apart, never enough to reuse one.
/// Applied to the customer/project keys and the client id, which are the only
/// credential-ish values a trace ever names — the secret is never traced at all.
fn mask(v: &str) -> String {
    let v = v.trim();
    match v.chars().count() {
        0 => "(empty)".into(),
        n if n <= 4 => "*".repeat(n),
        n => {
            let head: String = v.chars().take(2).collect();
            let tail: String = v.chars().skip(n - 2).collect();
            format!("{head}{}{tail}", "*".repeat(n - 4))
        }
    }
}

// ── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FetchErrorKind {
    /// The window was too large for the server to answer in time. The scheduler
    /// responds by splitting it into smaller ones, which is the documented
    /// remedy — the SOP recommends querying in smaller slices.
    Timeout,
    /// The server asked us to slow down. Splitting would make this *worse* by
    /// issuing more requests, so the scheduler backs off and retries the same
    /// window instead.
    RateLimited,
    /// A transient upstream failure. Like `RateLimited`, retry the same window
    /// after a delay rather than assuming it was too big.
    ServerError,
    Unauthorized,
    BadRequest,
    Network,
    Http,
    InvalidResponse,
    Config,
}

impl FetchErrorKind {
    /// Whether trying this window again — after a delay, or split smaller — has
    /// any chance of succeeding.
    fn is_retryable(self) -> bool {
        matches!(self, Self::Timeout | Self::RateLimited | Self::ServerError)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchError {
    pub kind: FetchErrorKind,
    pub message: String,
    /// True when this window is worth another attempt. `kind` decides *how*:
    /// `Timeout` splits, the others wait and retry unchanged.
    pub retryable: bool,
    /// Seconds the server asked us to wait, from a `Retry-After` header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    /// Everything that happened before this failed. Empty for errors raised
    /// before a trace exists (bad arguments, missing config).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trace: Vec<String>,
}

impl FetchError {
    fn new(kind: FetchErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable: kind.is_retryable(),
            retry_after_secs: None,
            trace: Vec::new(),
        }
    }

    fn with_retry_after(mut self, secs: Option<u64>) -> Self {
        self.retry_after_secs = secs;
        self
    }

    /// Attach the trace collected so far. Called at the single exit point in
    /// `fetch_window` so no error can escape without its own history.
    fn with_trace(mut self, trace: &mut Trace) -> Self {
        self.trace = trace.take();
        self
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// ── Time helpers (no chrono dependency) ──────────────────────────────────────

/// Days since the Unix epoch for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse `YYYY-MM-DDTHH:MM:SS` (with an optional trailing `Z`) as UTC epoch
/// seconds. Deliberately strict — these values come from the renderer and are
/// forwarded verbatim to the API.
pub fn parse_utc_iso(s: &str) -> Option<i64> {
    let s = s.trim();
    let core = s.strip_suffix('Z').unwrap_or(s);
    let b = core.as_bytes();
    if b.len() != 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':'
    {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> { core[range].parse().ok() };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec)
}

fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Validate a requested window against the SOP's constraints.
pub fn validate_window(start: &str, end: &str) -> Result<(), FetchError> {
    let s = parse_utc_iso(start).ok_or_else(|| {
        FetchError::new(
            FetchErrorKind::BadRequest,
            format!("Invalid start timestamp: {start:?} (expected YYYY-MM-DDTHH:MM:SSZ)"),
        )
    })?;
    let e = parse_utc_iso(end).ok_or_else(|| {
        FetchError::new(
            FetchErrorKind::BadRequest,
            format!("Invalid end timestamp: {end:?} (expected YYYY-MM-DDTHH:MM:SSZ)"),
        )
    })?;
    if e <= s {
        return Err(FetchError::new(
            FetchErrorKind::BadRequest,
            "End timestamp must be after the start timestamp",
        ));
    }
    if e - s >= MAX_WINDOW_SECS {
        return Err(FetchError::new(
            FetchErrorKind::BadRequest,
            format!(
                "Window spans {}s; the Analytics API requires less than 24 hours per request",
                e - s
            ),
        ));
    }
    let oldest = now_epoch_secs() - RETENTION_DAYS * 86400;
    if s < oldest {
        return Err(FetchError::new(
            FetchErrorKind::BadRequest,
            format!("{start} is more than {RETENTION_DAYS} days old; the Analytics API only retains {RETENTION_DAYS} days"),
        ));
    }
    Ok(())
}

// ── Response validation ──────────────────────────────────────────────────────

/// Delimiters we can recognise, most likely first. The portal export is
/// pipe-delimited; the API is checked rather than assumed.
const CANDIDATE_DELIMITERS: [char; 4] = ['|', ';', '\t', ','];

/// Inspect the first line of a CSV response.
///
/// Returns the detected delimiter, or an error explaining why the body is not
/// the interaction-log CSV we expect — which catches JSON error objects and
/// HTML error pages served with a `200`.
pub fn validate_csv_header(body: &str) -> Result<char, FetchError> {
    let trimmed = body.trim_start_matches(['\u{feff}', '\r', '\n', ' ']);
    if trimmed.is_empty() {
        return Err(FetchError::new(
            FetchErrorKind::InvalidResponse,
            "Analytics API returned an empty response",
        ));
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.starts_with('<') {
        let preview: String = trimmed.chars().take(200).collect();
        return Err(FetchError::new(
            FetchErrorKind::InvalidResponse,
            format!("Expected CSV but the response looks like JSON/HTML: {preview}"),
        ));
    }
    let header = trimmed.lines().next().unwrap_or("");
    let has = |name: &str| header.to_ascii_lowercase().contains(&name.to_ascii_lowercase());
    if !has("LogId") || !has("SessionUuid") {
        let preview: String = header.chars().take(200).collect();
        return Err(FetchError::new(
            FetchErrorKind::InvalidResponse,
            format!("CSV header is missing the expected LogId/SessionUuid columns: {preview}"),
        ));
    }
    let delimiter = CANDIDATE_DELIMITERS
        .iter()
        .copied()
        .find(|d| header.contains(*d))
        .ok_or_else(|| {
            FetchError::new(
                FetchErrorKind::InvalidResponse,
                "Could not determine the CSV delimiter from the response header",
            )
        })?;
    Ok(delimiter)
}

/// Response headers that would mean the payload is only one page of the result.
/// The SOP requires confirming the pagination mechanism before implementing it,
/// so rather than guess we refuse to import what may be a partial day.
fn detect_pagination(headers: &reqwest::header::HeaderMap) -> Option<String> {
    for name in ["link", "x-next-link", "x-continuation-token", "x-has-more", "x-next-page"] {
        if let Some(v) = headers.get(name) {
            let value = v.to_str().unwrap_or("");
            if name == "x-has-more" && value.eq_ignore_ascii_case("false") {
                continue;
            }
            return Some(format!("{name}: {value}"));
        }
    }
    None
}

/// Seconds to wait, from a `Retry-After` header.
///
/// Only the delta-seconds form is honoured. The HTTP-date form is legal but
/// needs a date parser this crate deliberately doesn't carry; the scheduler's
/// own exponential backoff covers that case.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    // Cap it: a server asking us to wait an hour should stall the run, not hide
    // a hang behind a progress bar.
    raw.trim().parse::<u64>().ok().map(|s| s.min(300))
}

// ── Token ────────────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// How long a freshly issued token may actually be handed out for.
///
/// Normally `lifetime - TOKEN_SKEW_SECS`, but never less than half its life:
/// a server that issued a token shorter than the skew would otherwise make
/// every cached token instantly unusable and turn one import into two requests
/// per window forever.
fn usable_lifetime(expires_in: u64) -> u64 {
    expires_in
        .saturating_sub(TOKEN_SKEW_SECS)
        .max(expires_in / 2)
}

impl AnalyticsState {
    /// The process-wide HTTP client. No global timeout — the two call sites
    /// want very different ones and set them per request.
    fn client(&self) -> Result<&reqwest::Client, FetchError> {
        if let Some(c) = self.client.get() {
            return Ok(c);
        }
        let built = reqwest::Client::builder()
            .user_agent("cm-conversation-dashboard")
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| {
                FetchError::new(FetchErrorKind::Network, format!("HTTP client error: {e}"))
            })?;
        // A racing initialiser already winning is fine — either client works.
        let _ = self.client.set(built);
        Ok(self.client.get().expect("client just set"))
    }

    fn cached_token(&self) -> Option<(String, u64)> {
        let cache = self.token.lock().ok()?;
        let now = now_secs();
        if !cache.access_token.is_empty() && now < cache.usable_until {
            Some((
                cache.access_token.clone(),
                cache.expires_at.saturating_sub(now),
            ))
        } else {
            None
        }
    }

    fn store_token(&self, access_token: String, expires_in: u64) {
        if let Ok(mut cache) = self.token.lock() {
            let now = now_secs();
            cache.access_token = access_token;
            cache.expires_at = now + expires_in;
            cache.usable_until = now + usable_lifetime(expires_in);
        }
    }

    pub fn clear_token(&self) {
        if let Ok(mut cache) = self.token.lock() {
            *cache = CachedToken::default();
        }
    }

    /// Return a valid bearer token, requesting a new one only when the cached
    /// one is missing or too close to expiry to outlive a full-length fetch.
    pub async fn token(
        &self,
        cfg: &AnalyticsConfig,
        trace: &mut Trace,
    ) -> Result<String, FetchError> {
        if let Some((t, remaining)) = self.cached_token() {
            trace.push(format!(
                "token: reusing cached token ({remaining}s of life left, {TOKEN_SKEW_SECS}s reserved for the request)"
            ));
            return Ok(t);
        }
        trace.push(format!(
            "token: requesting a new OAuth2 access token (client_id {}, resource {TOKEN_RESOURCE})",
            mask(&cfg.client_id)
        ));
        let started = SystemTime::now();
        let resp = self
            .client()?
            .post(TOKEN_URL)
            .timeout(Duration::from_secs(TOKEN_TIMEOUT_SECS))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", cfg.client_id.trim()),
                ("client_secret", cfg.client_secret.as_str()),
                ("resource", TOKEN_RESOURCE),
            ])
            .send()
            .await
            .map_err(|e| {
                let err = classify_reqwest_error(&e, "token request");
                trace.warn(format!("token: {}", err.message));
                err
            })?;

        let status = resp.status();
        let elapsed = started.elapsed().map(|d| d.as_millis()).unwrap_or(0);
        let body = resp.text().await.unwrap_or_default();
        trace.push(format!(
            "token: {TOKEN_URL} responded {status} in {elapsed}ms ({} bytes)",
            body.len()
        ));
        if !status.is_success() {
            let detail = extract_oauth_error(&body);
            trace.warn(format!("token: rejected — {detail}"));
            return Err(FetchError::new(
                if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::BAD_REQUEST {
                    FetchErrorKind::Unauthorized
                } else {
                    FetchErrorKind::Http
                },
                format!("Token request failed ({status}): {detail}"),
            ));
        }
        let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            FetchError::new(
                FetchErrorKind::InvalidResponse,
                format!("Token response was not JSON: {e}"),
            )
        })?;
        let access_token = json
            .get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if access_token.is_empty() {
            return Err(FetchError::new(
                FetchErrorKind::InvalidResponse,
                "Token response did not contain an access_token",
            ));
        }
        // `expires_in` is seconds, and arrives as a string from this endpoint.
        let expires_in = json
            .get("expires_in")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()).or_else(|| v.as_u64()))
            .unwrap_or(3600);
        self.store_token(access_token.clone(), expires_in);
        trace.push(format!(
            "token: acquired — valid {expires_in}s, reused for the next {}s",
            usable_lifetime(expires_in)
        ));
        Ok(access_token)
    }
}

fn extract_oauth_error(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let desc = v
            .get("error_description")
            .and_then(|d| d.as_str())
            .or_else(|| v.get("error").and_then(|d| d.as_str()));
        if let Some(d) = desc {
            // These descriptions are long and multi-line; keep the first line.
            return d.lines().next().unwrap_or(d).trim().to_string();
        }
    }
    body.chars().take(200).collect()
}

fn elapsed_ms(since: &SystemTime) -> u128 {
    since.elapsed().map(|d| d.as_millis()).unwrap_or(0)
}

/// Flatten an error and everything underneath it.
///
/// `reqwest::Error`'s own `Display` is almost always the useless outer layer
/// (`error sending request for url (…)`); the sentence that names the actual
/// problem — a TLS failure, a refused connection, a proxy rejection, a reset
/// mid-body — is two or three `source()` hops down. On Windows, where these
/// failures are the ones we cannot reproduce locally, that chain *is* the
/// diagnosis.
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![e.to_string()];
    let mut src = e.source();
    // Bounded: a cyclic `source()` chain would otherwise hang here.
    while let Some(s) = src {
        let text = s.to_string();
        if !parts.contains(&text) {
            parts.push(text);
        }
        if parts.len() >= 6 {
            break;
        }
        src = s.source();
    }
    parts.join(" ← ")
}

fn classify_reqwest_error(e: &reqwest::Error, context: &str) -> FetchError {
    // Every branch carries the flattened chain: the outer Display alone reads
    // the same for a DNS failure, a TLS rejection and a corporate proxy.
    let detail = error_chain(e);
    if e.is_timeout() {
        FetchError::new(
            FetchErrorKind::Timeout,
            format!("{context} timed out after {FETCH_TIMEOUT_SECS}s — the window may be too large ({detail})"),
        )
    } else if e.is_connect() {
        FetchError::new(
            FetchErrorKind::Network,
            format!("{context} could not connect: {detail}"),
        )
    } else if e.is_request() {
        FetchError::new(
            FetchErrorKind::Network,
            format!("{context} could not be sent: {detail}"),
        )
    } else {
        FetchError::new(FetchErrorKind::Network, format!("{context} failed: {detail}"))
    }
}

// ── Fetch ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchOutcome {
    pub temp_path: String,
    pub delimiter: String,
    pub row_count: i64,
    pub bytes: u64,
    pub duration_ms: u64,
    /// The same step-by-step account a failure carries. Surfaced in the import
    /// modal's Details log so a *successful* run still shows what it did —
    /// which is what makes the next failure comparable against a good one.
    pub trace: Vec<String>,
}

impl AnalyticsState {
    /// Download one window into a temp CSV. Holds the single-request permit for
    /// the whole call, so concurrent callers queue rather than overlap. See
    /// `fetch_lock` — that cap is ours, not the API's.
    pub async fn fetch_window(
        &self,
        cfg: &AnalyticsConfig,
        cache_dir: &Path,
        start_utc: &str,
        end_utc: &str,
    ) -> Result<FetchOutcome, FetchError> {
        let mut trace = Trace::default();
        match self
            .fetch_window_traced(cfg, cache_dir, start_utc, end_utc, &mut trace)
            .await
        {
            Ok(mut outcome) => {
                outcome.trace = trace.take();
                Ok(outcome)
            }
            // The single exit point for failures, so no error can reach the
            // renderer without the history that explains it.
            Err(e) => Err(e.with_trace(&mut trace)),
        }
    }

    async fn fetch_window_traced(
        &self,
        cfg: &AnalyticsConfig,
        cache_dir: &Path,
        start_utc: &str,
        end_utc: &str,
        trace: &mut Trace,
    ) -> Result<FetchOutcome, FetchError> {
        if !cfg.is_complete() {
            return Err(FetchError::new(
                FetchErrorKind::Config,
                "Analytics API is not configured — add your credentials in Settings",
            ));
        }
        validate_window(start_utc, end_utc)?;

        trace.push(format!(
            "window {start_utc} → {end_utc} — customer {}, project {}, culture {}, environment {}, activeSessionOnly {}",
            mask(&cfg.customer_key),
            mask(&cfg.project_key),
            cfg.culture.trim(),
            cfg.environment.trim(),
            cfg.active_session_only
        ));

        let queue_started = SystemTime::now();
        let _permit = self.fetch_lock.acquire().await.map_err(|_| {
            FetchError::new(FetchErrorKind::Network, "Analytics client is shutting down")
        })?;
        let waited = queue_started.elapsed().map(|d| d.as_millis()).unwrap_or(0);
        if waited > 50 {
            trace.push(format!("waited {waited}ms for the previous request to finish"));
        }

        let started = SystemTime::now();

        let mut token = self.token(cfg, trace).await?;
        let mut body = match self.request_csv(cfg, &token, start_utc, end_utc, trace).await {
            Ok(b) => b,
            Err(e) if e.kind == FetchErrorKind::Unauthorized => {
                // A 401 means the token expired or was never sent. Drop the
                // cached token and try once more before giving up. With the
                // skew now larger than the request timeout this should be
                // unreachable in practice — if it fires, the trace says so.
                trace.warn("401 from the interactions endpoint — discarding the cached token and retrying once");
                self.clear_token();
                token = self.token(cfg, trace).await?;
                self.request_csv(cfg, &token, start_utc, end_utc, trace)
                    .await?
            }
            Err(e) if e.kind == FetchErrorKind::Network => {
                // One retry for a connection-level failure. The client is
                // pooled now, and a keep-alive connection the server closed
                // while idle fails on first use and succeeds immediately after.
                trace.warn(format!(
                    "{} — retrying once on a fresh connection",
                    e.message
                ));
                self.request_csv(cfg, &token, start_utc, end_utc, trace)
                    .await?
            }
            Err(e) => return Err(e),
        };

        let delimiter = validate_csv_header(&body).inspect_err(|e| {
            let preview: String = body.chars().take(300).collect();
            trace.warn(format!("response rejected: {}", e.message));
            trace.warn(format!("first 300 characters: {preview}"));
        })?;
        // Strip a leading BOM so the csv reader sees a clean first column name.
        if body.starts_with('\u{feff}') {
            body = body.trim_start_matches('\u{feff}').to_string();
        }
        let row_count = body.lines().filter(|l| !l.trim().is_empty()).count() as i64 - 1;
        let bytes = body.len() as u64;
        trace.push(format!(
            "response is CSV, delimiter {delimiter:?}, {row_count} data rows, {bytes} bytes"
        ));

        let temp_path = self.write_temp_csv(cache_dir, &body).inspect_err(|e| {
            trace.warn(format!("could not stage the download: {}", e.message));
        })?;
        let duration_ms = started.elapsed().map(|d| d.as_millis() as u64).unwrap_or(0);
        trace.push(format!(
            "staged at {} — {row_count} rows in {duration_ms}ms",
            temp_path.display()
        ));

        Ok(FetchOutcome {
            temp_path: temp_path.to_string_lossy().into_owned(),
            delimiter: delimiter.to_string(),
            row_count: row_count.max(0),
            bytes,
            duration_ms,
            trace: Vec::new(),
        })
    }

    async fn request_csv(
        &self,
        cfg: &AnalyticsConfig,
        token: &str,
        start_utc: &str,
        end_utc: &str,
        trace: &mut Trace,
    ) -> Result<String, FetchError> {
        let client = self.client()?;
        let url = format!(
            "{API_BASE}/{}/projects/{}/interactions",
            cfg.customer_key.trim(),
            cfg.project_key.trim()
        );
        let mut query: Vec<(&str, String)> = vec![
            ("culture", cfg.culture.trim().to_string()),
            ("startDate", start_utc.to_string()),
            ("endDate", end_utc.to_string()),
            ("environment", cfg.environment.trim().to_string()),
        ];
        if cfg.active_session_only {
            query.push(("activeSessionOnly", "true".to_string()));
        }
        // `paginateData` is deliberately not sent: the SOP requires confirming
        // the pagination mechanism before implementing it.

        trace.push(format!(
            "GET {url}?{}",
            query
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        ));

        let sent = SystemTime::now();
        let resp = client
            .get(&url)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "text/csv")
            .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
            .query(&query)
            .send()
            .await
            .map_err(|e| {
                let err = classify_reqwest_error(&e, "Interaction log request");
                // The chain is where the actual cause lives — reqwest's own
                // Display is usually just "error sending request for url".
                trace.warn(format!("request failed after {}ms: {}", elapsed_ms(&sent), error_chain(&e)));
                err
            })?;

        let status = resp.status();
        let header_of = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-")
                .to_string()
        };
        trace.push(format!(
            "HTTP {status} in {}ms — content-type {}, content-length {}",
            elapsed_ms(&sent),
            header_of("content-type"),
            header_of("content-length")
        ));

        // Status first. A pagination header on an *error* response used to be
        // reported as "the response appears paginated", hiding the real HTTP
        // failure behind a message about a feature that isn't implemented.
        if !status.is_success() {
            // Read Retry-After before consuming the response body.
            let retry_after = parse_retry_after(resp.headers());
            let body = resp.text().await.unwrap_or_default();
            let preview: String = body.chars().take(300).collect();
            let kind = match status.as_u16() {
                401 => FetchErrorKind::Unauthorized,
                400 => FetchErrorKind::BadRequest,
                // Genuinely "this took too long" — splitting the window helps.
                408 | 504 => FetchErrorKind::Timeout,
                // Splitting a rate-limited request just sends more requests.
                429 => FetchErrorKind::RateLimited,
                500 | 502 | 503 => FetchErrorKind::ServerError,
                _ => FetchErrorKind::Http,
            };
            let hint = match status.as_u16() {
                400 => " (a 400 is usually a missing or invalid parameter — check culture)",
                403 => " (a 403 means the credentials are valid but not entitled to this customer/project)",
                404 => " (a 404 usually means the customer key or project key is wrong)",
                _ => "",
            };
            trace.warn(format!(
                "error body ({} bytes): {preview}",
                body.len()
            ));
            if let Some(secs) = retry_after {
                trace.push(format!("server asked us to wait {secs}s (Retry-After)"));
            }
            return Err(FetchError::new(
                kind,
                format!("Analytics API returned {status}{hint}: {preview}"),
            )
            .with_retry_after(retry_after));
        }

        if let Some(marker) = detect_pagination(resp.headers()) {
            trace.warn(format!("pagination marker present: {marker}"));
            return Err(FetchError::new(
                FetchErrorKind::InvalidResponse,
                format!(
                    "Response appears paginated ({marker}); pagination is not implemented, \
                     so {start_utc} → {end_utc} was not imported"
                ),
            ));
        }

        resp.text().await.map_err(|e| {
            // A body that dies mid-transfer is the classic long-download
            // failure, and it is *not* the same thing as never connecting.
            trace.warn(format!(
                "the response body stopped after {}ms: {}",
                elapsed_ms(&sent),
                error_chain(&e)
            ));
            FetchError::new(
                FetchErrorKind::Network,
                format!("Could not read the response body: {e}"),
            )
        })
    }

    fn write_temp_csv(&self, cache_dir: &Path, body: &str) -> Result<PathBuf, FetchError> {
        let dir = temp_dir(cache_dir);
        std::fs::create_dir_all(&dir).map_err(|e| {
            FetchError::new(
                FetchErrorKind::Network,
                format!("Cannot create temp directory: {e}"),
            )
        })?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let n = self.temp_counter.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("interactions-{stamp}-{n}.csv"));
        std::fs::write(&path, body).map_err(|e| {
            FetchError::new(
                FetchErrorKind::Network,
                format!("Cannot write temp CSV: {e}"),
            )
        })?;
        Ok(path)
    }
}

// ── Temp file management ─────────────────────────────────────────────────────

pub fn temp_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(TEMP_DIR_NAME)
}

/// True when `candidate` is a file directly inside the temp directory.
///
/// Guards the cleanup command: it accepts paths from the renderer, so it must
/// never be able to delete anything outside its own scratch directory.
pub fn is_temp_path(cache_dir: &Path, candidate: &Path) -> bool {
    let dir = temp_dir(cache_dir);
    // Compare against the canonical temp dir when it exists, so `..` segments
    // in the candidate cannot escape it.
    let dir_real = dir.canonicalize().unwrap_or(dir);
    let parent = match candidate.parent() {
        Some(p) => p.canonicalize().unwrap_or_else(|_| p.to_path_buf()),
        None => return false,
    };
    parent == dir_real
        && candidate
            .extension()
            .map(|e| e.eq_ignore_ascii_case("csv"))
            .unwrap_or(false)
}

/// Delete the given temp files, or every file in the temp directory when
/// `paths` is empty. Returns how many files were removed.
pub fn cleanup_temp(cache_dir: &Path, paths: &[String]) -> u32 {
    let mut removed = 0;
    if paths.is_empty() {
        let dir = temp_dir(cache_dir);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && std::fs::remove_file(&p).is_ok() {
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            log::info!(target: "analytics", "swept {removed} orphaned temp file(s)");
        }
        return removed;
    }
    for p in paths {
        let path = PathBuf::from(p);
        if !is_temp_path(cache_dir, &path) {
            log::warn!(target: "analytics", "refusing to delete non-temp path: {p}");
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 429 used to be folded into `Timeout`, which made the scheduler respond
    /// to "you are sending too many requests" by splitting the window and
    /// sending *more*. The kinds must stay distinct so backoff and subdivision
    /// can't be confused for one another again.
    #[test]
    fn rate_limiting_is_not_mistaken_for_a_timeout() {
        use FetchErrorKind::*;
        // Splitting is only ever the right answer for a genuine timeout.
        assert_ne!(RateLimited, Timeout);
        assert_ne!(ServerError, Timeout);
        // ...but all three are worth another attempt.
        for kind in [Timeout, RateLimited, ServerError] {
            assert!(
                FetchError::new(kind, "x").retryable,
                "{kind:?} should be retryable"
            );
        }
        for kind in [Unauthorized, BadRequest, Network, Http, InvalidResponse, Config] {
            assert!(
                !FetchError::new(kind, "x").retryable,
                "{kind:?} should not be retryable"
            );
        }
        // The renderer branches on the serialized kind, so the wire names matter.
        let json = serde_json::to_string(&FetchError::new(RateLimited, "slow down"))
            .expect("serialize");
        assert!(json.contains(r#""kind":"rateLimited""#), "got {json}");
        // Absent Retry-After must not appear at all rather than as null.
        assert!(!json.contains("retryAfterSecs"), "got {json}");
    }

    /// The bug behind "the first download fails, the retry works".
    ///
    /// A request is authorised once, when it is sent. If the token dies while
    /// the server is still producing the CSV, the failure is a reset or a
    /// truncated body — `Network`, not retryable — so the day failed; pressing
    /// Retry then worked because the token had by then aged past the skew and
    /// was replaced up front. A cached token must never be handed out with
    /// less life left than a request is allowed to take.
    #[test]
    fn a_token_is_never_handed_out_with_less_life_than_a_fetch_needs() {
        assert!(
            TOKEN_SKEW_SECS > FETCH_TIMEOUT_SECS,
            "a token reused with {TOKEN_SKEW_SECS}s of reserve can expire during a {FETCH_TIMEOUT_SECS}s request"
        );

        let state = AnalyticsState::default();
        // A token issued for an hour is reused, and always with more than a
        // full request's worth of life left.
        state.store_token("t".into(), 3600);
        let (_, remaining) = state.cached_token().expect("fresh token is usable");
        assert!(
            remaining > FETCH_TIMEOUT_SECS,
            "{remaining}s left is not enough for a {FETCH_TIMEOUT_SECS}s request"
        );

        // A token with only the skew left is refused rather than reused.
        state.store_token("t".into(), TOKEN_SKEW_SECS);
        state
            .token
            .lock()
            .map(|mut c| c.usable_until = now_secs())
            .unwrap();
        assert!(state.cached_token().is_none());

        state.clear_token();
        assert!(state.cached_token().is_none());
    }

    /// The floor exists so a server issuing tokens shorter than the skew can't
    /// make every cached token instantly unusable — that would turn one import
    /// into two requests per window, forever.
    #[test]
    fn a_short_lived_token_is_still_worth_caching() {
        assert_eq!(usable_lifetime(3600), 3600 - TOKEN_SKEW_SECS);
        assert_eq!(usable_lifetime(60), 30);
        assert_eq!(usable_lifetime(0), 0);

        let state = AnalyticsState::default();
        state.store_token("t".into(), 60);
        assert!(
            state.cached_token().is_some(),
            "a 60s token must still be reusable for its first half"
        );
    }

    /// Masking is applied to every key a trace names. It must never be
    /// reversible, and must never leak a short value whole.
    #[test]
    fn masked_keys_never_show_more_than_their_ends() {
        assert_eq!(mask(""), "(empty)");
        assert_eq!(mask("abcd"), "****");
        assert_eq!(mask("EFTELING"), "EF****NG");
        // The client secret is never traced at all — nothing calls mask on it.
        assert!(!mask("supersecretvalue").contains("secret"));
    }

    /// `reqwest`'s own Display is the outer wrapper; the sentence naming the
    /// real cause is a `source()` hop or two down, and that is what a Windows
    /// failure we cannot reproduce locally has to arrive with.
    #[test]
    fn the_error_chain_reaches_past_the_outer_message() {
        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "certificate has expired")
            }
        }
        impl std::error::Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "error sending request")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let text = error_chain(&Outer(Inner));
        assert!(text.contains("error sending request"), "got {text}");
        assert!(text.contains("certificate has expired"), "got {text}");
    }

    /// A trace must reach the renderer with the error, or a failed overnight
    /// import is diagnosable only from a log file on a machine we don't have.
    #[test]
    fn an_error_carries_its_trace_to_the_renderer() {
        let mut trace = Trace::default();
        trace.push("token: reusing cached token");
        trace.warn("HTTP 504 in 300012ms");
        let err = FetchError::new(FetchErrorKind::Timeout, "timed out").with_trace(&mut trace);
        assert_eq!(err.trace.len(), 2);
        // Taken, not copied: a second exit point can't re-emit the same lines.
        assert!(trace.take().is_empty());

        let json = serde_json::to_string(&err).expect("serialize");
        assert!(json.contains(r#""trace":["#), "got {json}");
        // An empty trace is omitted rather than sent as [].
        let bare = serde_json::to_string(&FetchError::new(FetchErrorKind::Config, "x")).unwrap();
        assert!(!bare.contains("trace"), "got {bare}");
    }

    #[test]
    fn retry_after_is_read_in_seconds_and_capped() {
        let mut h = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&h), None);
        h.insert(reqwest::header::RETRY_AFTER, "12".parse().unwrap());
        assert_eq!(parse_retry_after(&h), Some(12));
        // A very long wait is clamped — the run should fail visibly rather than
        // sit on a progress bar for an hour.
        h.insert(reqwest::header::RETRY_AFTER, "3600".parse().unwrap());
        assert_eq!(parse_retry_after(&h), Some(300));
        // The HTTP-date form is legal but unparsed here; the caller's own
        // exponential backoff covers it.
        h.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn parses_utc_iso_timestamps() {
        assert_eq!(parse_utc_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc_iso("2026-03-25T09:30:22Z"), Some(1774431022));
        // The trailing Z is optional.
        assert_eq!(
            parse_utc_iso("2026-03-25T09:30:22"),
            parse_utc_iso("2026-03-25T09:30:22Z")
        );
        assert_eq!(parse_utc_iso("2026-03-25 09:30:22Z"), None);
        assert_eq!(parse_utc_iso("2026-03-25T09:30:22.605Z"), None);
        assert_eq!(parse_utc_iso("not a date"), None);
        assert_eq!(parse_utc_iso("2026-13-25T00:00:00Z"), None);
    }

    fn today_window(h1: &str, h2: &str) -> (String, String) {
        // Anchor to "now" so the 90-day retention check never trips the test.
        let secs = now_epoch_secs();
        let days = secs / 86400;
        let mut y = 1970i64;
        let mut rem = days;
        loop {
            let dy = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
            if rem < dy { break; }
            rem -= dy;
            y += 1;
        }
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let md = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut m = 1;
        for &d in &md {
            if rem < d { break; }
            rem -= d;
            m += 1;
        }
        let day = rem + 1;
        let date = format!("{y:04}-{m:02}-{day:02}");
        (format!("{date}T{h1}Z"), format!("{date}T{h2}Z"))
    }

    #[test]
    fn accepts_a_full_day_window_that_stays_under_24h() {
        let (s, e) = today_window("00:00:00", "23:59:59");
        assert!(validate_window(&s, &e).is_ok());
    }

    #[test]
    fn rejects_a_window_of_exactly_24h_or_more() {
        // Midnight → next midnight is exactly 24h, which the API rejects; the
        // scheduler must emit 00:00:00 → 23:59:59 instead.
        let err = validate_window("2026-03-25T00:00:00Z", "2026-03-26T00:00:00Z").unwrap_err();
        assert_eq!(err.kind, FetchErrorKind::BadRequest);
        assert!(err.message.contains("less than 24 hours"), "{}", err.message);
        assert!(validate_window("2026-03-25T00:00:00Z", "2026-03-27T00:00:00Z").is_err());
    }

    #[test]
    fn rejects_reversed_and_out_of_retention_windows() {
        let (s, e) = today_window("10:00:00", "09:00:00");
        assert_eq!(
            validate_window(&s, &e).unwrap_err().kind,
            FetchErrorKind::BadRequest
        );
        let err = validate_window("2000-01-01T00:00:00Z", "2000-01-01T23:59:59Z").unwrap_err();
        assert!(err.message.contains("90 days"), "{}", err.message);
    }

    #[test]
    fn validates_a_real_portal_csv_header() {
        let header = "LogId|InteractionUuid|OriginatingInteractionUuid|SessionUuid|SessionIsActive|TimestampStart|TimestampEnd|Culture\n5328928970|abc||def|True|03/25/2026 09:30:22|03/25/2026 09:30:22|nl\n";
        assert_eq!(validate_csv_header(header).unwrap(), '|');
    }

    #[test]
    fn detects_a_comma_delimited_header() {
        let header = "LogId,InteractionUuid,SessionUuid,TimestampStart\n1,a,b,c\n";
        assert_eq!(validate_csv_header(header).unwrap(), ',');
    }

    #[test]
    fn rejects_json_html_and_empty_bodies() {
        let err = validate_csv_header(r#"{"error":"unauthorized","logId":1}"#).unwrap_err();
        assert_eq!(err.kind, FetchErrorKind::InvalidResponse);
        assert!(err.message.contains("JSON/HTML"));

        assert_eq!(
            validate_csv_header("<html><body>502 Bad Gateway</body></html>")
                .unwrap_err()
                .kind,
            FetchErrorKind::InvalidResponse
        );
        assert_eq!(
            validate_csv_header("   \n").unwrap_err().kind,
            FetchErrorKind::InvalidResponse
        );
        // Right shape, wrong columns.
        assert!(validate_csv_header("Foo|Bar|Baz\n1|2|3\n").is_err());
    }

    #[test]
    fn temp_path_guard_only_allows_csvs_in_the_temp_dir() {
        // Process-scoped, matching the tests in lib.rs: the fixed name meant
        // two concurrent `cargo test` runs deleted each other's fixtures.
        let cache = std::env::temp_dir()
            .join(format!("cai-analytics-guard-test-{}", std::process::id()));
        let dir = temp_dir(&cache);
        std::fs::create_dir_all(&dir).unwrap();

        let good = dir.join("interactions-1.csv");
        std::fs::write(&good, "x").unwrap();
        assert!(is_temp_path(&cache, &good));

        // Wrong extension, wrong directory, and traversal attempts all refused.
        assert!(!is_temp_path(&cache, &dir.join("secrets.json")));
        assert!(!is_temp_path(&cache, Path::new("/etc/passwd")));
        assert!(!is_temp_path(&cache, &dir.join("..").join("escape.csv")));

        // cleanup_temp honours the same guard.
        let outside = cache.join("keep-me.csv");
        std::fs::write(&outside, "x").unwrap();
        let removed = cleanup_temp(
            &cache,
            &[
                good.to_string_lossy().into_owned(),
                outside.to_string_lossy().into_owned(),
            ],
        );
        assert_eq!(removed, 1);
        assert!(!good.exists());
        assert!(outside.exists(), "a file outside the temp dir must survive");

        let _ = std::fs::remove_dir_all(&cache);
    }
}
