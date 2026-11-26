//! Request → response: routing, validation, cache headers and the rate limit, as the TypeScript
//! `src/api.ts` (0.0.11) did, over the read-only PostgreSQL role. Pure with respect to the transport, so it
//! is tested without a socket.

use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Serialize;
use sui_indexer_store::sui::{is_hex_id, normalize_sui_address};
use sui_indexer_store::{EventRow, EventsSnapshot, PageQuery, ReadError, Reader, Stream};

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

/// What a client sees while the ingester has not created the schema yet.
pub const NOT_READY: &str = "index not created yet";
/// What a client sees while the database cannot be reached (the cause is logged, not sent).
pub const UNAVAILABLE: &str = "database unavailable";

/// One request as the transport hands it over: only the parts the handler reads, so the handler is tested without a socket.
pub struct ApiRequest<'a> {
    /// The HTTP method, upper case (`GET`, `HEAD`, `OPTIONS`; anything else is refused with 405).
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

/// One answer: a status, its headers and a body (empty for HEAD, 204 and 304).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The response headers, in the order they are sent.
    pub headers: Vec<(&'static str, String)>,
    /// The response body; empty for HEAD, 204, 301 and 304.
    pub body: String,
}

impl ApiResponse {
    /// The value of a response header, matched case-insensitively (header names are, in HTTP).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The read-only database: a pool over the SELECT-only role. Until the ingester has migrated and started
/// the API answers `503` (and `/readyz` keeps the pod out of the Service), and the same while the database
/// is unreachable; nothing here ever writes or migrates.
pub struct Db {
    /// The pool over the read-only role.
    reader: Reader,
    /// Epoch ms of the last "database unavailable" log line (one per 10 s, not one per request).
    last_logged: AtomicI64,
}

impl Db {
    /// Wrap the read-only reader; nothing is queried until the first request.
    pub fn new(reader: Reader) -> Db {
        Db { reader, last_logged: AtomicI64::new(0) }
    }

    /// Fail with `503` unless the schema is there and the database answers (a query only until the index has been
    /// found ready once).
    pub async fn ensure_ready(&self) -> Result<(), HttpError> {
        self.reader.ensure_ready().await.map_err(|e| self.http(e))
    }

    /// Whether the schema is there and the database answers (`/readyz`): always asks the database.
    pub async fn check(&self) -> Result<(), HttpError> {
        self.reader.check().await.map_err(|e| self.http(e))
    }

    /// The reader (one statement per request, see `sui_indexer_store::Reader`).
    pub fn reader(&self) -> &Reader {
        &self.reader
    }

    /// Map a read failure to the answer: `503` for "not there yet" and "cannot reach it" (the cause is logged
    /// at most every 10 s, never sent), `500` for a failed query.
    pub fn http(&self, e: ReadError) -> HttpError {
        match e {
            ReadError::NotReady(_) => HttpError::new(503, NOT_READY),
            ReadError::Unavailable(detail) => {
                let now = sui_indexer_store::time::now_ms();
                let last = self.last_logged.load(Ordering::Relaxed);
                if now - last > 10_000
                    && self.last_logged.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
                {
                    crate::log(&format!("database unavailable: {detail}"));
                }
                HttpError::new(503, UNAVAILABLE)
            }
            ReadError::Store(e) => HttpError::internal(e.to_string()),
        }
    }
}

/// Everything a request needs besides the request itself: the database, the served network, the rate limiter, the poll interval and a clock.
pub struct ApiContext {
    /// The read-only database.
    pub db: Db,
    /// The network this indexer serves (`testnet` or `mainnet`); the first path segment after `v1` must equal it.
    pub network: String,
    /// The per-client rate limiter, behind a lock because requests run concurrently.
    pub limiter: Mutex<RateLimiter>,
    /// The ingester poll interval in milliseconds; a stream is healthy if it polled within five of these.
    pub poll_ms: u64,
    /// The clock, as epoch milliseconds; a function so tests can fix the time.
    pub now: fn() -> i64,
}

/// An error answered as `{ "error": message }`; status 500 is logged and answered generically.
#[derive(Debug)]
pub struct HttpError {
    /// The HTTP status to answer with.
    pub status: u16,
    /// The message sent as the error body (for a 500, the detail that is logged instead).
    pub message: String,
}

impl HttpError {
    /// An error answered with this status and message.
    pub fn new(status: u16, message: impl Into<String>) -> HttpError {
        HttpError { status, message: message.into() }
    }
    /// An internal failure: logged with its detail, answered as a generic 500.
    fn internal(message: String) -> HttpError {
        HttpError { status: 500, message }
    }
}

/// The headers every answer carries: open CORS (the API is public and read-only) and no content sniffing.
fn base_headers() -> Vec<(&'static str, String)> {
    vec![("Access-Control-Allow-Origin", "*".into()), ("X-Content-Type-Options", "nosniff".into())]
}

/// A JSON answer with the base headers, the content type and any extra headers (cache policy, ETag).
fn json(status: u16, body: &impl Serialize, extra: Vec<(&'static str, String)>) -> ApiResponse {
    let mut headers = base_headers();
    headers.push(("Content-Type", "application/json; charset=utf-8".into()));
    headers.extend(extra);
    ApiResponse { status, headers, body: serde_json::to_string(body).expect("response serialises") }
}

/// The JSON shape of an error answer: an object with one `error` field.
#[derive(Serialize)]
struct ErrorBody<'a> {
    /// The error message.
    error: &'a str,
}

/// The answer for an error: a 404 may be cached for a minute (the path will not appear), every other error is not stored.
fn error_response(e: &HttpError) -> ApiResponse {
    let cache = if e.status == 404 { "public, max-age=60" } else { "no-store" };
    json(e.status, &ErrorBody { error: &e.message }, vec![("Cache-Control", cache.into())])
}

/// Refuse a query parameter the route does not take, so a misspelt filter is a 400 and not silently ignored.
fn only_params(q: &Params, allowed: &[&str]) -> Result<(), HttpError> {
    match q.keys().find(|k| !allowed.contains(k)) {
        Some(k) => Err(HttpError::new(400, format!("unknown query parameter: {k}"))),
        None => Ok(()),
    }
}

/// Parse the page size: an integer in `[1, max]`, or the default when absent.
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

/// Parse an optional `0x` hex id and normalise it to the canonical long form the database stores.
fn parse_id(raw: Option<&str>, what: &str) -> Result<Option<String>, HttpError> {
    let Some(raw) = raw else { return Ok(None) };
    if !is_hex_id(raw) {
        return Err(HttpError::new(400, format!("{what} must be a 0x-prefixed hex id")));
    }
    Ok(Some(normalize_sui_address(raw)))
}

/// Cursor = `<generation>.<row id>` (`^([0-9a-f]{12})\.(\d{1,15})$`): its syntax. Whether the generation is the
/// current one is decided after the database has been read ([`check_generation`]).
fn parse_cursor(raw: Option<&str>) -> Result<Option<(String, i64)>, HttpError> {
    let Some(raw) = raw else { return Ok(None) };
    let bad = || HttpError::new(400, "before must be a cursor from a previous page");
    let (g, id) = raw.split_once('.').ok_or_else(bad)?;
    let gen_ok = g.len() == 12 && g.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let id_ok = (1..=15).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit());
    if !gen_ok || !id_ok {
        return Err(bad());
    }
    Ok(Some((g.to_owned(), id.parse().expect("1-15 digits"))))
}

/// A cursor from another database generation is refused with 410.
fn check_generation(cursor: &Option<(String, i64)>, generation: &str) -> Result<(), HttpError> {
    match cursor {
        Some((g, _)) if g != generation => {
            Err(HttpError::new(410, "cursor is from an earlier index; start from the newest page"))
        }
        _ => Ok(()),
    }
}

/// One event as the API serves it: the same camelCase shape the client library reads, with the BCS bytes in base64.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventJson<'a> {
    /// The fully qualified Move event type.
    event_type: &'a str,
    /// The Sui address that emitted the event.
    sender: &'a str,
    /// The event's BCS bytes, base64-encoded.
    bcs: String,
    /// The checkpoint the event was in, as a decimal string, if known.
    checkpoint: &'a Option<String>,
    /// The digest of the transaction that emitted the event.
    transaction_digest: &'a str,
    /// The event's position within its transaction.
    event_index: i64,
}

/// Convert a stored row to its served shape (the bytes are encoded, nothing is reinterpreted).
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

/// One page of events: the rows, the cursor for the next older page and, on the head page only, how fresh the index is.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventsPage<'a> {
    /// The events of this page, newest first.
    events: Vec<EventJson<'a>>,
    /// The cursor to pass as `before` for the next older page, or null at the end.
    cursor: Option<String>,
    /// The first checkpoint the index covers, so a client knows how far back it can read.
    #[serde(skip_serializing_if = "Option::is_none")]
    indexed_from_checkpoint: Option<String>,
    /// Head page only: the latest checkpoint indexed (null when none yet).
    // Only the head page says how fresh the index is (older pages are immutable and cached for a
    // year): a client treats a head page whose `polledAt` is old as unavailable and falls back.
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_checkpoint: Option<Option<String>>,
    /// Head page only: when the ingester last polled successfully, in epoch milliseconds (null when it never has).
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

/// The response for one events page, from the snapshot the database answered.
fn events_response(req: &ApiRequest<'_>, before_id: Option<i64>, snap: EventsSnapshot) -> ApiResponse {
    let generation = snap.generation.unwrap_or_default();
    let head = before_id.is_none();
    let cache_control = if head { HEAD } else { IMMUTABLE };
    let etag = match before_id {
        None => format!("\"{generation}-{}\"", snap.head_id),
        Some(b) => format!("\"{generation}-b{b}\""),
    };
    if etag_matches(req.if_none_match, &etag) {
        let mut headers = base_headers();
        headers.push(("Cache-Control", cache_control.into()));
        headers.push(("ETag", etag));
        return ApiResponse { status: 304, headers, body: String::new() };
    }
    let polled = head.then_some(if snap.poll.last_success_at == 0 { None } else { Some(snap.poll.last_success_at) });
    let body = EventsPage {
        events: snap.page.rows.iter().map(serialize_row).collect(),
        cursor: snap.page.next.map(|n| format!("{generation}.{n}")),
        indexed_from_checkpoint: snap.coverage.from_checkpoint,
        latest_checkpoint: head.then_some(snap.coverage.latest_checkpoint),
        polled_at: polled,
    };
    json(200, &body, vec![("Cache-Control", cache_control.into()), ("ETag", etag)])
}

/// One events route: the order of checks is: not ready `503`, then the cursor's syntax `400`,
/// then its generation `410`, then the limit `400`, with one statement answering generation, head and page.
async fn events_route(
    ctx: &ApiContext,
    req: &ApiRequest<'_>,
    stream: Stream,
    q: &Params,
    (kinds, gate_id): (Option<Vec<String>>, Option<String>),
    (default_limit, max_limit): (u32, u32),
) -> Result<ApiResponse, HttpError> {
    ctx.db.ensure_ready().await?;
    let cursor = parse_cursor(q.get("before"))?;
    let limit = match parse_limit(q.get("limit"), default_limit, max_limit) {
        Ok(l) => l,
        Err(bad_limit) => {
            // A cursor of another generation is `410` whatever the limit: ask the generation first.
            if cursor.is_some() {
                let generation = ctx.db.reader().generation().await.map_err(|e| ctx.db.http(e))?.unwrap_or_default();
                check_generation(&cursor, &generation)?;
            }
            return Err(bad_limit);
        }
    };
    let before_id = cursor.as_ref().map(|(_, id)| *id);
    let snap = ctx
        .db
        .reader()
        .events(stream, &PageQuery { kinds, gate_id, before_id, limit })
        .await
        .map_err(|e| ctx.db.http(e))?;
    check_generation(&cursor, snap.generation.as_deref().unwrap_or_default())?;
    Ok(events_response(req, before_id, snap))
}

/// What one stream has indexed: the first and latest checkpoint, the event count and the counts of undecodable events and gaps.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamCoverage {
    /// The first checkpoint indexed for this stream.
    from_checkpoint: Option<String>,
    /// The latest checkpoint indexed for this stream.
    latest_checkpoint: Option<String>,
    /// How many events the stream holds.
    events: i64,
    /// How many events could not be decoded (kept as raw bytes and counted, never dropped).
    undecodable: i64,
    /// How many gaps the stream has, where the source could not supply a range.
    gaps: i64,
}

/// The JSON body of the coverage route: the network, the index generation and both streams.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CoverageBody<'a> {
    /// The network served.
    network: &'a str,
    /// The index generation; a cursor from another generation is refused.
    generation: &'a str,
    /// Coverage of the access-gate stream.
    access_gate: StreamCoverage,
    /// Coverage of the sealed-content stream.
    sealed_content: StreamCoverage,
}

/// The JSON body of a gate statistics route: counts by kind and the commission total as a decimal string (it can exceed 53 bits).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsBody {
    /// The gate id, normalised.
    gate_id: String,
    /// How many access NFTs were minted.
    minted: i64,
    /// How many were consumed.
    consumed: i64,
    /// How many were burned.
    burned: i64,
    /// The commission total in MIST as a decimal string, because it can exceed 53 bits.
    commission_mist: String,
    /// The first checkpoint the index covers.
    indexed_from_checkpoint: Option<String>,
}

/// One stream health: whether its last poll is recent, how old that poll is and the last error text.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamHealthJson {
    /// Whether the last poll is within five poll intervals.
    ok: bool,
    /// The age of the last successful poll in milliseconds (null if never).
    last_poll_age_ms: Option<i64>,
    /// The last error the ingester recorded for this stream.
    last_error: Option<String>,
}

/// Both streams health, under the names the client uses.
#[derive(Serialize)]
struct Streams {
    /// The access-gate stream.
    #[serde(rename = "access-gate")]
    access_gate: StreamHealthJson,
    /// The sealed-content stream.
    #[serde(rename = "sealed-content")]
    sealed_content: StreamHealthJson,
}

/// The JSON body of the health route: overall health and each stream.
#[derive(Serialize)]
struct HealthBody {
    /// Whether every stream is healthy.
    ok: bool,
    /// Each stream's health.
    streams: Streams,
}

/// The JSON body of the readiness route: whether the database answers and, if not, why in a fixed phrase.
#[derive(Serialize)]
struct ReadyBody<'a> {
    /// Whether the database answers.
    ok: bool,
    /// The reason when it does not, from a fixed set of phrases (never a database message).
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

/// Prometheus text for the monitoring stack (served on `METRICS_PORT` only, never on the public port):
/// per stream, the time of the last successful poll and whether it is fresh by the same rule as
/// `/healthz`; and whether the database answered. An alert on a stale stream (the access-gate stream
/// failed unnoticed from 2026-11-10 to 2026-11-19) reads these.
pub async fn metrics(ctx: &ApiContext) -> ApiResponse {
    let now = (ctx.now)();
    let limit = i64::try_from(ctx.poll_ms.saturating_mul(5)).unwrap_or(i64::MAX);
    let mut out = String::from(
        "# HELP sui_indexer_stream_last_success_timestamp_seconds Unix time of the stream's last successful poll (0: never).\n\
         # TYPE sui_indexer_stream_last_success_timestamp_seconds gauge\n",
    );
    let mut up = String::from(
        "# HELP sui_indexer_stream_up 1 when the stream's last successful poll is within five poll intervals (as /healthz).\n\
         # TYPE sui_indexer_stream_up gauge\n",
    );
    let db_up = match ctx.db.reader().polls().await {
        Ok(states) => {
            for (stream, h) in Stream::ALL.iter().zip(states) {
                let labels = format!("network=\"{}\",stream=\"{}\"", ctx.network, stream.as_str());
                let secs = h.last_success_at as f64 / 1000.0;
                let fresh = h.last_success_at != 0 && now - h.last_success_at < limit;
                out.push_str(&format!("sui_indexer_stream_last_success_timestamp_seconds{{{labels}}} {secs}\n"));
                up.push_str(&format!("sui_indexer_stream_up{{{labels}}} {}\n", u8::from(fresh)));
            }
            1
        }
        Err(_) => 0,
    };
    out.push_str(&up);
    out.push_str(&format!(
        "# HELP sui_indexer_database_up 1 when the API could read the ingester's heartbeats.\n\
         # TYPE sui_indexer_database_up gauge\nsui_indexer_database_up{{network=\"{}\"}} {db_up}\n",
        ctx.network
    ));
    ApiResponse {
        status: 200,
        body: out,
        headers: vec![
            ("Content-Type", "text/plain; version=0.0.4; charset=utf-8".into()),
            ("Cache-Control", "no-store".into()),
        ],
    }
}

/// The health answer: 200 only if both streams have polled successfully within five poll intervals, else 503 naming the stream and its error.
async fn healthz(ctx: &ApiContext) -> ApiResponse {
    let now = (ctx.now)();
    let limit = i64::try_from(ctx.poll_ms.saturating_mul(5)).unwrap_or(i64::MAX);
    let one = |h: sui_indexer_store::PollState| {
        let age = (h.last_success_at != 0).then(|| now - h.last_success_at);
        StreamHealthJson { ok: age.is_some_and(|a| a < limit), last_poll_age_ms: age, last_error: h.last_error }
    };
    let streams = match ctx.db.reader().polls().await {
        Ok([a, b]) => Streams { access_gate: one(a), sealed_content: one(b) },
        Err(e) => {
            let msg = ctx.db.http(e).message;
            let down = || StreamHealthJson { ok: false, last_poll_age_ms: None, last_error: Some(msg.clone()) };
            Streams { access_gate: down(), sealed_content: down() }
        }
    };
    let ok = streams.access_gate.ok && streams.sealed_content.ok;
    json(if ok { 200 } else { 503 }, &HealthBody { ok, streams }, vec![("Cache-Control", "no-store".into())])
}

/// Dispatch one validated GET or HEAD to its handler by path: readiness, health, coverage, the two event streams and gate statistics.
async fn route(ctx: &ApiContext, req: &ApiRequest<'_>, pathname: &str, q: &Params) -> Result<ApiResponse, HttpError> {
    // Split into non-empty segments, so a doubled or trailing slash cannot change which route matches.
    let parts: Vec<&str> = pathname.split('/').filter(|s| !s.is_empty()).collect();

    if parts == ["readyz"] {
        // Serving works while polls stall (stale data is still useful, and clients fall back), but
        // not before the ingester has created the database.
        only_params(q, &[])?;
        return Ok(match ctx.db.check().await {
            Ok(()) => json(200, &ReadyBody { ok: true, error: None }, vec![("Cache-Control", "no-store".into())]),
            Err(e) if e.status == 503 => {
                let message = if e.message == UNAVAILABLE { UNAVAILABLE } else { NOT_READY };
                json(503, &ReadyBody { ok: false, error: Some(message) }, vec![("Cache-Control", "no-store".into())])
            }
            Err(e) => return Err(e),
        });
    }

    if parts == ["healthz"] {
        only_params(q, &[])?;
        return Ok(healthz(ctx).await);
    }

    // Every data route is /v1/<network>/...; anything else is unknown.
    if parts.len() < 3 || parts[0] != "v1" {
        return Err(HttpError::new(404, "not found"));
    }
    // One indexer serves one network, and says so rather than answering for another.
    if parts[1] != ctx.network {
        return Err(HttpError::new(404, format!("this indexer serves {} only", ctx.network)));
    }
    // The route below the network, for the exact-match checks that follow.
    let rest = parts[2..].join("/");

    if rest == "coverage" {
        only_params(q, &[])?;
        ctx.db.ensure_ready().await?;
        let snap = ctx.db.reader().coverage().await.map_err(|e| ctx.db.http(e))?;
        // The snapshot carries both streams in a fixed order; the two destructured here are the access-gate and sealed-content streams.
        let [(_, ag_cov, ag_health), (_, sc_cov, sc_health)] = snap.streams;
        let cov = |c: sui_indexer_store::Coverage, h: sui_indexer_store::StreamHealth| StreamCoverage {
            from_checkpoint: c.from_checkpoint,
            latest_checkpoint: c.latest_checkpoint,
            events: c.events,
            undecodable: h.undecodable,
            gaps: h.gaps,
        };
        let generation = snap.generation.unwrap_or_default();
        let body = CoverageBody {
            network: &ctx.network,
            generation: &generation,
            access_gate: cov(ag_cov, ag_health),
            sealed_content: cov(sc_cov, sc_health),
        };
        return Ok(json(200, &body, vec![("Cache-Control", HEAD.into())]));
    }

    if rest == "access-gate/events" {
        only_params(q, &["kinds", "gate", "before", "limit"])?;
        // `kinds` is an optional comma list of event kinds; an unknown or empty list is a 400 naming what was wrong.
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
        return events_route(ctx, req, Stream::AccessGate, q, (kinds, gate_id), (20, 100)).await;
    }

    if rest == "sealed-content" {
        only_params(q, &["gate", "before", "limit"])?;
        let gate_id = parse_id(q.get("gate"), "gate")?.ok_or_else(|| HttpError::new(400, "gate is required"))?;
        return events_route(ctx, req, Stream::SealedContent, q, (None, Some(gate_id)), (50, 200)).await;
    }

    if parts.len() == 5 && parts[2] == "gates" && parts[4] == "stats" {
        only_params(q, &[])?;
        let gate_id = parse_id(Some(parts[3]), "gate id")?.ok_or_else(|| HttpError::new(400, "gate id is required"))?;
        ctx.db.ensure_ready().await?;
        let snap = ctx.db.reader().gate(&gate_id).await.map_err(|e| ctx.db.http(e))?;
        let body = StatsBody {
            minted: snap.stats.count("AccessMinted"),
            consumed: snap.stats.count("AccessConsumed"),
            burned: snap.stats.count("AccessBurned"),
            commission_mist: snap.stats.commission_mist.to_string(),
            indexed_from_checkpoint: snap.from_checkpoint,
            gate_id,
        };
        return Ok(json(200, &body, vec![("Cache-Control", HEAD.into())]));
    }

    Err(HttpError::new(404, "not found"))
}

/// Handle one request. Errors of status 500 are returned as `Err` (with the detail to log).
pub async fn handle(ctx: &ApiContext, req: &ApiRequest<'_>) -> Result<ApiResponse, String> {
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
    // Probes are never rate limited or redirected: the cluster and the monitoring stack must always get an answer.
    let exempt = pathname == "/healthz" || pathname == "/readyz";
    if !exempt {
        let client = client_key(req.cf_connecting_ip.or(req.remote_address).unwrap_or("unknown"));
        let now = (ctx.now)();
        // A poisoned lock still holds a usable limiter, so a panic elsewhere does not take rate limiting down.
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
    // A 500 is returned to the caller to log; every other error becomes its JSON answer.
    let res = match route(ctx, req, &pathname, &q).await {
        Ok(r) => r,
        Err(e) if e.status == 500 => return Err(e.message),
        Err(e) => error_response(&e),
    };
    // HEAD gets the same status and headers as GET with no body.
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
