//! Request → response: routing, validation, cache headers and the rate limit, as the TypeScript
//! `src/api.ts` (0.0.11) did, over a read-only database. Pure with respect to the transport, so it
//! is tested without a socket.

use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Serialize;
use sui_indexer_store::sui::{is_hex_id, normalize_sui_address};
use sui_indexer_store::{EventRow, OpenError, PageQuery, Reader, Stream};

use crate::query::{Params, canonical_path, canonical_query, distinct_items, is_limit_syntax, normalize_path};
use crate::ratelimit::{RateLimiter, client_key};

/// Every `access_gate` event kind (`ACCESS_GATE_EVENT_KINDS` of `@meddleware/access-gate-client`).
pub const ACCESS_GATE_EVENT_KINDS: [&str; 8] = [
    "GateCreated",
    "AccessMinted",
    "AccessConsumed",
    "AccessBurned",
    "GateFrozen",
    "GateMadeFree",
    "PlatformConfigUpdated",
    "PlatformMigrated",
];

/// Older pages never change once served (row ids follow ledger order).
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// The newest page changes as events land: short browser TTL, slightly longer at the edge.
pub const HEAD: &str = "public, max-age=10, s-maxage=30";

/// What a client sees while the ingester has not created the database yet.
pub const NOT_READY: &str = "index not created yet";

pub struct ApiRequest<'a> {
    pub method: &'a str,
    /// The request target's path (as the transport parsed it).
    pub path: &'a str,
    /// The raw query string (without `?`), if any.
    pub query: Option<&'a str>,
    /// `CF-Connecting-IP`, when present (all values joined by `, ` as Node.js reports a repeated header).
    pub cf_connecting_ip: Option<&'a str>,
    /// `If-None-Match`, when present.
    pub if_none_match: Option<&'a str>,
    /// Socket address, used when no `CF-Connecting-IP` header is present.
    pub remote_address: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

impl ApiResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The read-only database, opened on demand: until the ingester creates it the API answers `503`
/// (and `/readyz` keeps the pod out of the Service), then connections are pooled.
pub struct Db {
    path: PathBuf,
    idle: Mutex<Vec<Reader>>,
}

/// Idle read-only connections kept per process.
const MAX_IDLE: usize = 32;

pub enum DbError {
    NotReady,
    Sql(String),
}

impl Db {
    pub fn new(path: impl Into<PathBuf>) -> Db {
        Db { path: path.into(), idle: Mutex::new(Vec::new()) }
    }

    /// Run `f` on a pooled connection inside one read transaction (one snapshot for the request).
    pub fn with<T>(&self, f: impl FnOnce(&Reader) -> Result<T, HttpError>) -> Result<T, HttpError> {
        let pooled = self.idle.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let reader = match pooled {
            Some(r) => r,
            None => match Reader::open(&self.path) {
                Ok(r) => r,
                Err(OpenError::NotReady(_)) => return Err(HttpError::new(503, NOT_READY)),
                Err(OpenError::Sql(e)) => return Err(HttpError::internal(e.to_string())),
            },
        };
        reader.begin().map_err(|e| HttpError::internal(e.to_string()))?;
        let out = f(&reader);
        let ended = reader.end();
        if ended.is_ok() {
            let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
            if idle.len() < MAX_IDLE {
                idle.push(reader);
            }
        }
        out
    }

    /// Whether the database opens and has its schema (`/readyz`).
    pub fn ready(&self) -> bool {
        self.with(|r| r.generation().map_err(sql).map(|g| g.is_some())).unwrap_or(false)
    }
}

pub struct ApiContext {
    pub db: Db,
    pub network: String,
    pub limiter: Mutex<RateLimiter>,
    pub poll_ms: u64,
    pub now: fn() -> i64,
}

/// An error answered as `{ "error": message }`; status 500 is logged and answered generically.
#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
}

impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> HttpError {
        HttpError { status, message: message.into() }
    }
    fn internal(message: String) -> HttpError {
        HttpError { status: 500, message }
    }
}

fn sql(e: sui_indexer_store::SqlError) -> HttpError {
    HttpError::internal(e.to_string())
}

fn base_headers() -> Vec<(&'static str, String)> {
    vec![("Access-Control-Allow-Origin", "*".into()), ("X-Content-Type-Options", "nosniff".into())]
}

fn json(status: u16, body: &impl Serialize, extra: Vec<(&'static str, String)>) -> ApiResponse {
    let mut headers = base_headers();
    headers.push(("Content-Type", "application/json; charset=utf-8".into()));
    headers.extend(extra);
    ApiResponse { status, headers, body: serde_json::to_string(body).expect("response serialises") }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

fn error_response(e: &HttpError) -> ApiResponse {
    let cache = if e.status == 404 { "public, max-age=60" } else { "no-store" };
    json(e.status, &ErrorBody { error: &e.message }, vec![("Cache-Control", cache.into())])
}

fn only_params(q: &Params, allowed: &[&str]) -> Result<(), HttpError> {
    match q.keys().find(|k| !allowed.contains(k)) {
        Some(k) => Err(HttpError::new(400, format!("unknown query parameter: {k}"))),
        None => Ok(()),
    }
}

fn parse_limit(raw: Option<&str>, fallback: u32, max: u32) -> Result<u32, HttpError> {
    let Some(raw) = raw else { return Ok(fallback) };
    if !is_limit_syntax(raw) {
        return Err(HttpError::new(400, "limit must be an integer"));
    }
    let v: u32 = raw.parse().expect("1-4 digits");
    if v < 1 || v > max {
        return Err(HttpError::new(400, format!("limit must be in [1, {max}]")));
    }
    Ok(v)
}

fn parse_id(raw: Option<&str>, what: &str) -> Result<Option<String>, HttpError> {
    let Some(raw) = raw else { return Ok(None) };
    if !is_hex_id(raw) {
        return Err(HttpError::new(400, format!("{what} must be a 0x-prefixed hex id")));
    }
    Ok(Some(normalize_sui_address(raw)))
}

/// Cursor = `<generation>.<row id>` (`^([0-9a-f]{12})\.(\d{1,15})$`); a cursor from another
/// database generation is refused with 410.
fn parse_before(raw: Option<&str>, generation: &str) -> Result<Option<i64>, HttpError> {
    let Some(raw) = raw else { return Ok(None) };
    let bad = || HttpError::new(400, "before must be a cursor from a previous page");
    let (g, id) = raw.split_once('.').ok_or_else(bad)?;
    let gen_ok = g.len() == 12 && g.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let id_ok = (1..=15).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit());
    if !gen_ok || !id_ok {
        return Err(bad());
    }
    if g != generation {
        return Err(HttpError::new(410, "cursor is from an earlier index; start from the newest page"));
    }
    Ok(Some(id.parse().expect("1-15 digits")))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventJson<'a> {
    event_type: &'a str,
    sender: &'a str,
    bcs: String,
    checkpoint: &'a Option<String>,
    transaction_digest: &'a str,
    event_index: i64,
}

fn serialize_row(row: &EventRow) -> EventJson<'_> {
    EventJson {
        event_type: &row.event_type,
        sender: &row.sender,
        bcs: B64.encode(&row.bcs),
        checkpoint: &row.checkpoint,
        transaction_digest: &row.transaction_digest,
        event_index: row.event_index,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventsPage<'a> {
    events: Vec<EventJson<'a>>,
    cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    indexed_from_checkpoint: Option<String>,
    // Only the head page says how fresh the index is (older pages are immutable and cached for a
    // year): a client treats a head page whose `polledAt` is old as unavailable and falls back.
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_checkpoint: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    polled_at: Option<Option<i64>>,
}

/// RFC 9110 `If-None-Match`: a list of entity tags (weak or strong, weak comparison) or `*`.
/// Cloudflare weakens ETags when it compresses, so an exact string comparison would never match.
fn etag_matches(header: Option<&str>, etag: &str) -> bool {
    let Some(h) = header.filter(|h| !h.is_empty()) else { return false };
    if h.trim() == "*" {
        return true;
    }
    h.split(',').any(|t| {
        let t = t.trim();
        t.strip_prefix("W/").unwrap_or(t) == etag
    })
}

fn events_page(
    req: &ApiRequest,
    stream: Stream,
    q: PageQuery,
    r: &Reader,
    generation: &str,
) -> Result<ApiResponse, HttpError> {
    let head = q.before_id.is_none();
    let cache_control = if head { HEAD } else { IMMUTABLE };
    let etag = match q.before_id {
        None => format!("\"{generation}-{}\"", r.head_id(stream).map_err(sql)?),
        Some(b) => format!("\"{generation}-b{b}\""),
    };
    if etag_matches(req.if_none_match, &etag) {
        let mut headers = base_headers();
        headers.push(("Cache-Control", cache_control.into()));
        headers.push(("ETag", etag));
        return Ok(ApiResponse { status: 304, headers, body: String::new() });
    }
    let page = r.page(stream, &q).map_err(sql)?;
    let coverage = r.coverage(stream).map_err(sql)?;
    let polled = if head {
        let at = r.poll_state(stream).map_err(sql)?.last_success_at;
        Some(if at == 0 { None } else { Some(at) })
    } else {
        None
    };
    let body = EventsPage {
        events: page.rows.iter().map(serialize_row).collect(),
        cursor: page.next.map(|n| format!("{generation}.{n}")),
        indexed_from_checkpoint: coverage.from_checkpoint,
        latest_checkpoint: head.then_some(coverage.latest_checkpoint),
        polled_at: polled,
    };
    Ok(json(200, &body, vec![("Cache-Control", cache_control.into()), ("ETag", etag)]))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamCoverage {
    from_checkpoint: Option<String>,
    latest_checkpoint: Option<String>,
    events: i64,
    undecodable: i64,
    gaps: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CoverageBody<'a> {
    network: &'a str,
    generation: &'a str,
    access_gate: StreamCoverage,
    sealed_content: StreamCoverage,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsBody {
    gate_id: String,
    minted: i64,
    consumed: i64,
    burned: i64,
    commission_mist: String,
    indexed_from_checkpoint: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamHealthJson {
    ok: bool,
    last_poll_age_ms: Option<i64>,
    last_error: Option<String>,
}

#[derive(Serialize)]
struct Streams {
    #[serde(rename = "access-gate")]
    access_gate: StreamHealthJson,
    #[serde(rename = "sealed-content")]
    sealed_content: StreamHealthJson,
}

#[derive(Serialize)]
struct HealthBody {
    ok: bool,
    streams: Streams,
}

#[derive(Serialize)]
struct ReadyBody<'a> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

fn healthz(ctx: &ApiContext) -> ApiResponse {
    let now = (ctx.now)();
    let limit = i64::try_from(ctx.poll_ms.saturating_mul(5)).unwrap_or(i64::MAX);
    let read = ctx.db.with(|r| {
        let one = |s: Stream| -> Result<StreamHealthJson, HttpError> {
            let h = r.poll_state(s).map_err(sql)?;
            let age = (h.last_success_at != 0).then(|| now - h.last_success_at);
            Ok(StreamHealthJson { ok: age.is_some_and(|a| a < limit), last_poll_age_ms: age, last_error: h.last_error })
        };
        Ok(Streams { access_gate: one(Stream::AccessGate)?, sealed_content: one(Stream::SealedContent)? })
    });
    let streams = match read {
        Ok(s) => s,
        Err(e) => {
            let msg = if e.status == 503 { NOT_READY.to_owned() } else { e.message };
            let down = || StreamHealthJson { ok: false, last_poll_age_ms: None, last_error: Some(msg.clone()) };
            Streams { access_gate: down(), sealed_content: down() }
        }
    };
    let ok = streams.access_gate.ok && streams.sealed_content.ok;
    json(if ok { 200 } else { 503 }, &HealthBody { ok, streams }, vec![("Cache-Control", "no-store".into())])
}

fn route(ctx: &ApiContext, req: &ApiRequest, pathname: &str, q: &Params) -> Result<ApiResponse, HttpError> {
    let parts: Vec<&str> = pathname.split('/').filter(|s| !s.is_empty()).collect();

    if parts == ["readyz"] {
        // Serving works while polls stall (stale data is still useful, and clients fall back), but
        // not before the ingester has created the database.
        only_params(q, &[])?;
        return Ok(if ctx.db.ready() {
            json(200, &ReadyBody { ok: true, error: None }, vec![("Cache-Control", "no-store".into())])
        } else {
            json(503, &ReadyBody { ok: false, error: Some(NOT_READY) }, vec![("Cache-Control", "no-store".into())])
        });
    }

    if parts == ["healthz"] {
        only_params(q, &[])?;
        return Ok(healthz(ctx));
    }

    if parts.len() < 3 || parts[0] != "v1" {
        return Err(HttpError::new(404, "not found"));
    }
    if parts[1] != ctx.network {
        return Err(HttpError::new(404, format!("this indexer serves {} only", ctx.network)));
    }
    let rest = parts[2..].join("/");

    if rest == "coverage" {
        only_params(q, &[])?;
        return ctx.db.with(|r| {
            let generation = r.generation().map_err(sql)?.unwrap_or_default();
            let cov = |s: Stream| -> Result<StreamCoverage, HttpError> {
                let c = r.coverage(s).map_err(sql)?;
                let h = r.health(s).map_err(sql)?;
                Ok(StreamCoverage {
                    from_checkpoint: c.from_checkpoint,
                    latest_checkpoint: c.latest_checkpoint,
                    events: c.events,
                    undecodable: h.undecodable,
                    gaps: h.gaps,
                })
            };
            let body = CoverageBody {
                network: &ctx.network,
                generation: &generation,
                access_gate: cov(Stream::AccessGate)?,
                sealed_content: cov(Stream::SealedContent)?,
            };
            Ok(json(200, &body, vec![("Cache-Control", HEAD.into())]))
        });
    }

    if rest == "access-gate/events" {
        only_params(q, &["kinds", "gate", "before", "limit"])?;
        let kinds = match q.get("kinds") {
            None => None,
            Some(raw) => {
                let kinds = distinct_items(raw);
                let unknown: Vec<&str> =
                    kinds.iter().map(String::as_str).filter(|k| !ACCESS_GATE_EVENT_KINDS.contains(k)).collect();
                if kinds.is_empty() || !unknown.is_empty() {
                    let list = unknown.join(", ");
                    return Err(HttpError::new(
                        400,
                        format!("unknown kinds: {}", if list.is_empty() { "(empty)" } else { &list }),
                    ));
                }
                Some(kinds)
            }
        };
        let gate_id = parse_id(q.get("gate"), "gate")?;
        return ctx.db.with(|r| {
            let generation = r.generation().map_err(sql)?.unwrap_or_default();
            let before_id = parse_before(q.get("before"), &generation)?;
            let limit = parse_limit(q.get("limit"), 20, 100)?;
            events_page(req, Stream::AccessGate, PageQuery { kinds, gate_id, before_id, limit }, r, &generation)
        });
    }

    if rest == "sealed-content" {
        only_params(q, &["gate", "before", "limit"])?;
        let gate_id = parse_id(q.get("gate"), "gate")?.ok_or_else(|| HttpError::new(400, "gate is required"))?;
        return ctx.db.with(|r| {
            let generation = r.generation().map_err(sql)?.unwrap_or_default();
            let before_id = parse_before(q.get("before"), &generation)?;
            let limit = parse_limit(q.get("limit"), 50, 200)?;
            let q = PageQuery { kinds: None, gate_id: Some(gate_id), before_id, limit };
            events_page(req, Stream::SealedContent, q, r, &generation)
        });
    }

    if parts.len() == 5 && parts[2] == "gates" && parts[4] == "stats" {
        only_params(q, &[])?;
        let gate_id = parse_id(Some(parts[3]), "gate id")?.ok_or_else(|| HttpError::new(400, "gate id is required"))?;
        return ctx.db.with(|r| {
            let stats = r.gate_stats(&gate_id).map_err(sql)?;
            let from = r.coverage(Stream::AccessGate).map_err(sql)?.from_checkpoint;
            let body = StatsBody {
                minted: stats.count("AccessMinted"),
                consumed: stats.count("AccessConsumed"),
                burned: stats.count("AccessBurned"),
                commission_mist: stats.commission_mist.to_string(),
                indexed_from_checkpoint: from,
                gate_id,
            };
            Ok(json(200, &body, vec![("Cache-Control", HEAD.into())]))
        });
    }

    Err(HttpError::new(404, "not found"))
}

/// Handle one request. Errors of status 500 are returned as `Err` (with the detail to log).
pub fn handle(ctx: &ApiContext, req: &ApiRequest) -> Result<ApiResponse, String> {
    if req.method == "OPTIONS" {
        let mut headers = base_headers();
        headers.push(("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS".into()));
        headers.push(("Access-Control-Max-Age", "86400".into()));
        return Ok(ApiResponse { status: 204, headers, body: String::new() });
    }
    if req.method != "GET" && req.method != "HEAD" {
        return Ok(json(405, &ErrorBody { error: "method not allowed" }, vec![("Allow", "GET, HEAD, OPTIONS".into())]));
    }
    // `new URL(target, base)`: a target that is not an absolute path (`*`) resolves against `/`.
    let pathname =
        if req.path.starts_with('/') { normalize_path(req.path) } else { normalize_path(&format!("/{}", req.path)) };
    let q = Params::parse(req.query.unwrap_or(""));
    let exempt = pathname == "/healthz" || pathname == "/readyz";
    if !exempt {
        let client = client_key(req.cf_connecting_ip.or(req.remote_address).unwrap_or("unknown"));
        let now = (ctx.now)();
        let mut limiter = ctx.limiter.lock().unwrap_or_else(|e| e.into_inner());
        if !limiter.take(&client, now) {
            let retry = limiter.retry_after(&client).max(1);
            return Ok(json(
                429,
                &ErrorBody { error: "rate limited" },
                vec![("Retry-After", retry.to_string()), ("Cache-Control", "no-store".into())],
            ));
        }
    }
    // Canonical spellings only: another spelling of the same query gets a cacheable 301 to the
    // canonical URL (after the rate limit, so redirects cannot be used to probe for free).
    if !exempt {
        let path = canonical_path(&pathname);
        let canonical = canonical_query(&q).serialize();
        if path != pathname || canonical != q.serialize() {
            let mut headers = base_headers();
            let location = if canonical.is_empty() { path } else { format!("{path}?{canonical}") };
            headers.push(("Location", location));
            headers.push(("Cache-Control", "public, max-age=86400".into()));
            return Ok(ApiResponse { status: 301, headers, body: String::new() });
        }
    }
    let res = match route(ctx, req, &pathname, &q) {
        Ok(r) => r,
        Err(e) if e.status == 500 => return Err(e.message),
        Err(e) => error_response(&e),
    };
    Ok(if req.method == "HEAD" { ApiResponse { body: String::new(), ..res } } else { res })
}

/// The answer to a request that failed internally (the detail is logged, not sent).
pub fn internal_error() -> ApiResponse {
    ApiResponse {
        status: 500,
        headers: vec![("Content-Type", "application/json".into())],
        body: "{\"error\":\"internal error\"}".into(),
    }
}
