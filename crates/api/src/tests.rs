//! API tests: every case of the TypeScript `tests/api.test.ts` (the client-contract cases run in
//! CI's `contract` job against the binary, with the real client packages), plus the database lifecycle
//! (schema not there yet, database down, connection loss) and the socket wiring. They run against a real
//! PostgreSQL provisioned like production (`TEST_DATABASE_URL`, see `sui_indexer_store::testdb`): the
//! ingester's role writes, the API reads through the SELECT-only role.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use sui_indexer_store::testdb::TestDb;
use sui_indexer_store::time::now_ms;
use sui_indexer_store::{PollState, Reader, Store, Stream, require_db};
use sui_indexer_testkit::{self as kit, gate_a, gate_b, minted};

use crate::handler::{ApiContext, ApiRequest, ApiResponse, Db, handle};
use crate::ratelimit::RateLimiter;

struct Setup {
    db: TestDb,
    store: Store,
    ctx: ApiContext,
}

fn context(db: &TestDb, limiter: RateLimiter) -> ApiContext {
    ApiContext {
        db: Db::new(Reader::new(&db.api, 4).unwrap()),
        network: "testnet".into(),
        limiter: Mutex::new(limiter),
        poll_ms: 15_000,
        now: now_ms,
    }
}

async fn bound_store(db: &TestDb) -> Store {
    let mut store = db.store().await;
    store
        .bind_packages(&[(Stream::AccessGate, kit::ag()), (Stream::SealedContent, kit::seal())], &[], None)
        .await
        .unwrap();
    store
}

async fn setup_with(limiter: RateLimiter) -> Option<Setup> {
    let db = TestDb::create().await?;
    let mut store = bound_store(&db).await;
    kit::seed(&mut store).await;
    let ctx = context(&db, limiter);
    let s = Setup { db, store, ctx };
    s.set_last_poll(now_ms(), Stream::AccessGate).await;
    s.set_last_poll(now_ms(), Stream::SealedContent).await;
    Some(s)
}

async fn setup() -> Option<Setup> {
    setup_with(RateLimiter::new(1000, 1000)).await
}

/// `let s = setup!();` — the seeded database and context, or return (skipped) without a test database.
macro_rules! setup {
    () => {
        match setup().await {
            Some(s) => s,
            None => return,
        }
    };
    ($limiter:expr) => {
        match setup_with($limiter).await {
            Some(s) => s,
            None => return,
        }
    };
}

impl Setup {
    async fn set_last_poll(&self, t: i64, stream: Stream) {
        let mut st = self.store.poll_state(stream).await.unwrap();
        st.last_success_at = t;
        self.store.record_poll(stream, &st).await.unwrap();
    }

    async fn request(&self, method: &str, url: &str, headers: &[(&str, &str)]) -> ApiResponse {
        let (path, query) = match url.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (url, None),
        };
        let h = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| *v);
        handle(
            &self.ctx,
            &ApiRequest {
                method,
                path,
                query,
                cf_connecting_ip: h("cf-connecting-ip"),
                if_none_match: h("if-none-match"),
                remote_address: Some("10.0.0.1"),
            },
        )
        .await
        .expect("no internal error")
    }

    async fn get(&self, url: &str) -> ApiResponse {
        self.request("GET", url, &[]).await
    }

    async fn get_h(&self, url: &str, headers: &[(&str, &str)]) -> ApiResponse {
        self.request("GET", url, headers).await
    }
}

fn body(r: &ApiResponse) -> Value {
    serde_json::from_str(&r.body).unwrap()
}

fn checkpoints(b: &Value) -> Vec<String> {
    b["events"].as_array().unwrap().iter().map(|e| e["checkpoint"].as_str().unwrap().to_owned()).collect()
}

fn full_a() -> String {
    format!("0x{}a", "0".repeat(63))
}

// ── events pages ────────────────────────────────────────────────────────────

#[tokio::test]
async fn serves_the_newest_page_with_short_cache_headers_and_an_etag() {
    let s = setup!();
    let r = s.get("/v1/testnet/access-gate/events?limit=2").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Cache-Control"), Some("public, max-age=10, s-maxage=30"));
    assert_eq!(r.header("Access-Control-Allow-Origin"), Some("*"));
    assert_eq!(r.header("X-Content-Type-Options"), Some("nosniff"));
    assert_eq!(r.header("Content-Type"), Some("application/json; charset=utf-8"));
    let b = body(&r);
    assert_eq!(checkpoints(&b), ["5", "4"]);
    let cursor = b["cursor"].as_str().unwrap();
    let (g, id) = cursor.split_once('.').unwrap();
    assert!(g.len() == 12 && g.bytes().all(|c| c.is_ascii_hexdigit()) && id.parse::<u64>().is_ok());
    assert_eq!(b["indexedFromCheckpoint"], "1");
    let etag = r.header("ETag").expect("the newest page must carry an ETag").to_owned();
    assert_eq!(s.get_h("/v1/testnet/access-gate/events?limit=2", &[("if-none-match", &etag)]).await.status, 304);
}

#[tokio::test]
async fn serves_older_pages_as_immutable_and_ends_with_a_null_cursor() {
    let s = setup!();
    let first = body(&s.get("/v1/testnet/access-gate/events?limit=2").await);
    let second =
        s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={}", first["cursor"].as_str().unwrap())).await;
    assert_eq!(second.header("Cache-Control"), Some("public, max-age=31536000, immutable"));
    let b2 = body(&second);
    assert_eq!(checkpoints(&b2), ["3", "2"]);
    let b3 = body(
        &s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={}", b2["cursor"].as_str().unwrap())).await,
    );
    assert_eq!(checkpoints(&b3), ["1"]);
    assert!(b3["cursor"].is_null());
}

#[tokio::test]
async fn filters_by_kinds_and_gate() {
    let s = setup!();
    let b = body(&s.get(&format!("/v1/testnet/access-gate/events?kinds=AccessConsumed&gate={}", gate_a())).await);
    let events = b["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0]["eventType"].as_str().unwrap().ends_with("AccessConsumedEvent"));
}

#[tokio::test]
async fn refuses_a_cursor_from_another_index_generation() {
    let s = setup!();
    let r = s.get("/v1/testnet/access-gate/events?before=000000000000.5").await;
    assert_eq!(r.status, 410);
    assert_eq!(body(&r)["error"], "cursor is from an earlier index; start from the newest page");
    assert_eq!(r.header("Cache-Control"), Some("no-store"));
}

#[tokio::test]
async fn validates_strictly() {
    let s = setup!();
    for (url, msg) in [
        ("/v1/testnet/access-gate/events?limit=0", "limit must be in [1, 100]"),
        ("/v1/testnet/access-gate/events?limit=101", "limit must be in [1, 100]"),
        ("/v1/testnet/access-gate/events?limit=abc", "limit must be an integer"),
        ("/v1/testnet/access-gate/events?gate=zz", "gate must be a 0x-prefixed hex id"),
        ("/v1/testnet/access-gate/events?kinds=Nope", "unknown kinds: Nope"),
        ("/v1/testnet/access-gate/events?kinds=", "unknown kinds: (empty)"),
        ("/v1/testnet/access-gate/events?kinds=AccessMinted%2CNope", "unknown kinds: Nope"),
        ("/v1/testnet/access-gate/events?before=garbage", "before must be a cursor from a previous page"),
        ("/v1/testnet/access-gate/events?cachebuster=1", "unknown query parameter: cachebuster"),
        ("/v1/testnet/sealed-content", "gate is required"),
        ("/v1/testnet/coverage?x=1", "unknown query parameter: x"),
        ("/healthz?x=1", "unknown query parameter: x"),
    ] {
        let r = s.get(url).await;
        assert_eq!(r.status, 400, "{url}");
        assert_eq!(body(&r)["error"], msg, "{url}");
        assert_eq!(r.header("Cache-Control"), Some("no-store"), "{url}");
    }
    let r = s.get(&format!("/v1/testnet/sealed-content?gate={}&limit=201", full_a())).await;
    assert_eq!((r.status, body(&r)["error"].as_str().unwrap()), (400, "limit must be in [1, 200]"));
    let r = s.get("/v1/mainnet/access-gate/events").await;
    assert_eq!((r.status, body(&r)["error"].as_str().unwrap()), (404, "this indexer serves testnet only"));
    assert_eq!(r.header("Cache-Control"), Some("public, max-age=60"));
    for url in ["/v1/testnet/nothing", "/", "/v2/testnet/coverage", "/v1/testnet", "/v1/testnet/gates/0xa/other"] {
        let r = s.get(url).await;
        assert_eq!((r.status, body(&r)["error"].as_str().unwrap()), (404, "not found"), "{url}");
    }
    // Every published kind is accepted.
    let all = crate::handler::ACCESS_GATE_EVENT_KINDS.join(",");
    let mut sorted: Vec<&str> = crate::handler::ACCESS_GATE_EVENT_KINDS.to_vec();
    sorted.sort();
    assert_eq!(s.get(&format!("/v1/testnet/access-gate/events?kinds={}", sorted.join("%2C"))).await.status, 200);
    assert_eq!(s.get(&format!("/v1/testnet/access-gate/events?kinds={all}")).await.status, 301);
}

#[tokio::test]
async fn lists_sealed_content_for_one_gate() {
    let s = setup!();
    let b = body(&s.get(&format!("/v1/testnet/sealed-content?gate={}", gate_a())).await);
    assert_eq!(b["events"].as_array().unwrap().len(), 2);
    assert_eq!(checkpoints(&b), ["8", "6"]);
}

#[tokio::test]
async fn rows_are_served_verbatim_in_the_clients_shape() {
    let s = setup!();
    let r = s.get(&format!("/v1/testnet/sealed-content?gate={}&limit=1", gate_b())).await;
    let f = kit::sealed(&gate_b(), 7, "other");
    let expected_bcs = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&f.bcs)
    };
    let b = body(&r);
    let e = &b["events"][0];
    assert_eq!(e["bcs"], expected_bcs.as_str());
    assert_eq!(e["eventType"], f.event_type.as_str());
    assert_eq!(e["sender"], kit::addr("0xa11ce").as_str());
    assert_eq!(e["eventIndex"], 0);
    // Key order and types as JSON.stringify produced them (the clients check field types).
    let keys: Vec<&str> =
        r.body.match_indices("\":").map(|(i, _)| &r.body[r.body[..i].rfind('"').unwrap() + 1..i]).collect();
    assert_eq!(
        keys,
        [
            "events",
            "eventType",
            "sender",
            "bcs",
            "checkpoint",
            "transactionDigest",
            "eventIndex",
            "cursor",
            "indexedFromCheckpoint",
            "latestCheckpoint",
            "polledAt"
        ]
    );
}

// ── stats, coverage and health ──────────────────────────────────────────────

#[tokio::test]
async fn counts_a_gates_events_and_sums_commission_exactly() {
    let mut s = setup!();
    let big = 1u64 << 60;
    s.store
        .ingest(
            Stream::AccessGate,
            &[minted(&gate_a(), 10, big).indexed(), minted(&gate_a(), 11, big).indexed()],
            Some("c2"),
        )
        .await
        .unwrap();
    let r = s.get(&format!("/v1/testnet/gates/{}/stats", gate_a())).await;
    assert_eq!(r.header("Cache-Control"), Some("public, max-age=10, s-maxage=30"));
    assert_eq!(
        r.body,
        format!(
            r#"{{"gateId":"{}","minted":4,"consumed":1,"burned":0,"commissionMist":"{}","indexedFromCheckpoint":"1"}}"#,
            gate_a(),
            2 * u128::from(big) + 4000
        )
    );
}

#[tokio::test]
async fn stats_of_an_unknown_gate_are_zero() {
    let s = setup!();
    let b = body(&s.get(&format!("/v1/testnet/gates/0x{}/stats", "f".repeat(64))).await);
    assert_eq!((b["minted"].as_i64(), b["commissionMist"].as_str()), (Some(0), Some("0")));
}

#[tokio::test]
async fn reports_coverage_per_stream() {
    let s = setup!();
    let r = s.get("/v1/testnet/coverage").await;
    let b = body(&r);
    assert_eq!(b["network"], "testnet");
    assert_eq!(b["generation"], s.store.generation());
    assert_eq!(
        b["accessGate"],
        serde_json::json!({ "fromCheckpoint": "1", "latestCheckpoint": "5", "events": 5, "undecodable": 0, "gaps": 0 })
    );
    assert_eq!(b["sealedContent"]["events"], 3);
    assert!(r.body.starts_with(r#"{"network":"testnet","generation":""#));
}

#[tokio::test]
async fn healthz_is_503_before_the_first_poll_and_after_polls_stall_and_names_the_stream_and_its_error() {
    let s = setup!();
    assert_eq!(s.get("/healthz").await.status, 200);
    s.set_last_poll(0, Stream::AccessGate).await;
    assert_eq!(s.get("/healthz").await.status, 503);
    s.store
        .record_poll(
            Stream::AccessGate,
            &PollState {
                last_success_at: now_ms() - 5 * 15_000 - 1,
                last_error: Some("requested data below earliest available".into()),
                last_attempt_at: now_ms(),
            },
        )
        .await
        .unwrap();
    let res = s.get("/healthz").await;
    assert_eq!(res.status, 503);
    assert_eq!(res.header("Cache-Control"), Some("no-store"));
    let b = body(&res);
    assert_eq!(b["ok"], false);
    assert_eq!(b["streams"]["access-gate"]["ok"], false);
    assert_eq!(b["streams"]["access-gate"]["lastError"], "requested data below earliest available");
    assert!(b["streams"]["access-gate"]["lastPollAgeMs"].as_i64().unwrap() > 75_000);
    assert_eq!(b["streams"]["sealed-content"]["ok"], true); // the other stream is fine
    assert!(b["streams"]["sealed-content"]["lastError"].is_null());
    assert_eq!(s.get("/readyz").await.status, 200); // still serving
    assert_eq!(s.get("/readyz").await.body, r#"{"ok":true}"#);
}

#[tokio::test]
async fn head_pages_say_how_fresh_the_index_is_and_older_pages_stay_free_of_it() {
    let s = setup!();
    let head = body(&s.get("/v1/testnet/access-gate/events?limit=2").await);
    assert_eq!(head["latestCheckpoint"], "5");
    assert!(head["polledAt"].as_i64().unwrap() > 0);
    let older = body(
        &s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={}", head["cursor"].as_str().unwrap())).await,
    );
    let o = older.as_object().unwrap();
    assert!(!o.contains_key("polledAt") && !o.contains_key("latestCheckpoint"));
    // Before any poll, polledAt is null (not 0).
    s.set_last_poll(0, Stream::AccessGate).await;
    assert!(body(&s.get("/v1/testnet/access-gate/events").await)["polledAt"].is_null());
}

#[tokio::test]
async fn an_empty_stream_has_no_coverage_fields() {
    let db = require_db!();
    let _store = db.store().await;
    let ctx = context(&db, RateLimiter::new(100, 100));
    let r = handle(&ctx, &req("GET", "/v1/testnet/access-gate/events", None)).await.unwrap();
    assert_eq!(r.body, r#"{"events":[],"cursor":null,"latestCheckpoint":null,"polledAt":null}"#);
    assert_eq!(r.header("ETag").unwrap().split('-').nth(1), Some("0\""));
}

fn req<'a>(method: &'a str, path: &'a str, query: Option<&'a str>) -> ApiRequest<'a> {
    ApiRequest { method, path, query, cf_connecting_ip: None, if_none_match: None, remote_address: None }
}

// ── canonical URLs (one cache entry per query) ─────────────────────────────

fn loc(r: &ApiResponse) -> Option<String> {
    (r.status == 301).then(|| r.header("Location").unwrap().to_owned())
}

#[tokio::test]
async fn redirects_other_spellings_of_a_query_to_the_canonical_form_cacheably() {
    let s = setup!();
    let full = full_a();
    assert_eq!(
        loc(&s.get("/v1/testnet/access-gate/events?gate=0xA").await),
        Some(format!("/v1/testnet/access-gate/events?gate={full}"))
    );
    assert_eq!(
        loc(&s.get("/v1/testnet/access-gate/events?gate=0x0a").await),
        Some(format!("/v1/testnet/access-gate/events?gate={full}"))
    );
    assert_eq!(
        loc(&s.get(&format!("/v1/testnet/access-gate/events?gate=0x{}", full[2..].to_uppercase())).await),
        Some(format!("/v1/testnet/access-gate/events?gate={full}"))
    );
    assert_eq!(
        loc(&s.get("/v1/testnet/access-gate/events?limit=05").await).as_deref(),
        Some("/v1/testnet/access-gate/events?limit=5")
    );
    assert_eq!(
        loc(&s.get("/v1/testnet/access-gate/events?kinds=AccessMinted,AccessConsumed,AccessMinted").await).as_deref(),
        Some("/v1/testnet/access-gate/events?kinds=AccessConsumed%2CAccessMinted")
    );
    assert_eq!(loc(&s.get("/v1/testnet/gates/0xA/stats").await), Some(format!("/v1/testnet/gates/{full}/stats")));
    let r = s.get("/v1/testnet/access-gate/events?limit=05").await;
    assert_eq!(r.header("Cache-Control"), Some("public, max-age=86400"));
    assert_eq!(r.header("Access-Control-Allow-Origin"), Some("*"));
    assert_eq!(r.body, "");
}

#[tokio::test]
async fn serves_canonical_queries_without_a_redirect() {
    let s = setup!();
    let g = full_a();
    for url in [
        format!("/v1/testnet/access-gate/events?kinds=AccessConsumed%2CAccessMinted&gate={g}&limit=10"),
        format!("/v1/testnet/access-gate/events?gate={g}&limit=5"),
        // A raw comma serialises the same as %2C: not a different spelling.
        format!("/v1/testnet/access-gate/events?kinds=AccessConsumed,AccessMinted&gate={g}"),
        "/v1/testnet/coverage".to_owned(),
        format!("/v1/testnet/gates/{g}/stats"),
    ] {
        assert_eq!(s.get(&url).await.status, 200, "{url}");
    }
}

#[tokio::test]
async fn the_redirect_target_is_itself_canonical() {
    let s = setup!();
    let first = s.get("/v1/testnet/access-gate/events?limit=05&gate=0xA").await;
    assert_eq!(first.status, 301);
    assert_eq!(s.get(first.header("Location").unwrap()).await.status, 200);
}

#[tokio::test]
async fn dot_segments_resolve_like_the_url_parser() {
    let s = setup!();
    assert_eq!(s.get("/v1/./testnet/x/../coverage").await.status, 200);
    assert_eq!(s.get("//v1//testnet//coverage").await.status, 200); // empty segments are ignored by the router
}

// ── transport rules ─────────────────────────────────────────────────────────

#[tokio::test]
async fn revalidates_with_weak_listed_or_wildcard_if_none_match() {
    let s = setup!();
    let etag = s.get("/v1/testnet/access-gate/events").await.header("ETag").unwrap().to_owned();
    for h in [etag.clone(), format!("W/{etag}"), format!("\"zzz\", {etag}"), "*".into()] {
        let r = s.get_h("/v1/testnet/access-gate/events", &[("if-none-match", &h)]).await;
        assert_eq!(r.status, 304, "{h}");
        assert_eq!(r.header("ETag"), Some(etag.as_str()));
        assert_eq!(r.header("Cache-Control"), Some("public, max-age=10, s-maxage=30"));
        assert_eq!(r.body, "");
    }
    assert_eq!(s.get_h("/v1/testnet/access-gate/events", &[("if-none-match", "\"other\"")]).await.status, 200);
    // Older pages carry a `b<id>` ETag.
    let first = body(&s.get("/v1/testnet/access-gate/events?limit=2").await);
    let older =
        s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={}", first["cursor"].as_str().unwrap())).await;
    assert!(older.header("ETag").unwrap().contains("-b"));
}

#[tokio::test]
async fn a_new_event_changes_the_head_etag() {
    let mut s = setup!();
    let before = s.get("/v1/testnet/access-gate/events").await.header("ETag").unwrap().to_owned();
    s.store.ingest(Stream::AccessGate, &[minted(&gate_a(), 99, 1).indexed()], Some("c3")).await.unwrap();
    let after = s.get_h("/v1/testnet/access-gate/events", &[("if-none-match", &before)]).await;
    assert_eq!(after.status, 200);
    assert_ne!(after.header("ETag").unwrap(), before);
}

#[tokio::test]
async fn rate_limits_per_client_ip_preferring_cf_connecting_ip_and_never_limits_health() {
    let s = setup!(RateLimiter::new(1, 2));
    let cf = [("cf-connecting-ip", "1.1.1.1")];
    assert_eq!(s.get_h("/v1/testnet/coverage", &cf).await.status, 200);
    assert_eq!(s.get_h("/v1/testnet/coverage", &cf).await.status, 200);
    let limited = s.get_h("/v1/testnet/coverage", &cf).await;
    assert_eq!(limited.status, 429);
    assert!(limited.header("Retry-After").unwrap().parse::<u64>().unwrap() >= 1);
    assert_eq!(limited.header("Cache-Control"), Some("no-store"));
    assert_eq!(body(&limited)["error"], "rate limited");
    assert_eq!(s.get_h("/v1/testnet/coverage", &[("cf-connecting-ip", "2.2.2.2")]).await.status, 200);
    assert_eq!(s.get_h("/healthz", &cf).await.status, 200);
    assert_eq!(s.get_h("/readyz", &cf).await.status, 200);
    // Without the header the socket address is the key.
    assert_eq!(s.get("/v1/testnet/coverage").await.status, 200);
}

#[tokio::test]
async fn answers_preflight_refuses_writes_and_sends_no_body_for_head() {
    let s = setup!();
    let pre = s.request("OPTIONS", "/v1/testnet/coverage", &[]).await;
    assert_eq!(pre.status, 204);
    assert_eq!(pre.header("Access-Control-Allow-Methods"), Some("GET, HEAD, OPTIONS"));
    assert_eq!(pre.header("Access-Control-Max-Age"), Some("86400"));
    let post = s.request("POST", "/v1/testnet/coverage", &[]).await;
    assert_eq!(post.status, 405);
    assert_eq!(post.header("Allow"), Some("GET, HEAD, OPTIONS"));
    assert_eq!(body(&post)["error"], "method not allowed");
    let head = s.request("HEAD", "/v1/testnet/coverage", &[]).await;
    assert_eq!(head.status, 200);
    assert_eq!(head.body, "");
    assert_eq!(s.request("HEAD", "/v1/testnet/nothing", &[]).await.body, "");
}

// ── the database lifecycle (read-only API, ingester creates the database) ──

#[tokio::test]
async fn before_the_ingester_migrates_everything_but_options_is_503_and_the_api_creates_nothing() {
    let db = require_db!();
    let ctx = context(&db, RateLimiter::new(100, 100));
    let ready = handle(&ctx, &req("GET", "/readyz", None)).await.unwrap();
    assert_eq!((ready.status, ready.body.as_str()), (503, r#"{"ok":false,"error":"index not created yet"}"#));
    assert_eq!(ready.header("Cache-Control"), Some("no-store"));
    let health = handle(&ctx, &req("GET", "/healthz", None)).await.unwrap();
    assert_eq!(health.status, 503);
    assert_eq!(
        health.body,
        r#"{"ok":false,"streams":{"access-gate":{"ok":false,"lastPollAgeMs":null,"lastError":"index not created yet"},"sealed-content":{"ok":false,"lastPollAgeMs":null,"lastError":"index not created yet"}}}"#
    );
    for url in ["/v1/testnet/coverage", "/v1/testnet/access-gate/events", "/v1/testnet/gates/0xa/stats"] {
        let path = url.replace("/0xa/", &format!("/{}/", full_a()));
        let r = handle(&ctx, &req("GET", &path, None)).await.unwrap();
        assert_eq!((r.status, r.header("Cache-Control")), (503, Some("no-store")), "{url}");
        assert_eq!(r.body, r#"{"error":"index not created yet"}"#);
    }
    // The API's role cannot create the schema, and did not try to: nothing exists.
    let none: Option<String> =
        db.admin().await.query_one("SELECT to_regclass('public.schema_migrations')::text", &[]).await.unwrap().get(0);
    assert_eq!(none, None, "the API never creates the schema");
    // The schema alone (migrated, but the ingester has not started: no generation) is not ready either.
    let mut c = db.owner.connect().await.unwrap();
    sui_indexer_store::migrate::run(&mut c).await.unwrap();
    assert_eq!(handle(&ctx, &req("GET", "/readyz", None)).await.unwrap().status, 503);
    // The ingester starts: the same context starts serving.
    let mut store = bound_store(&db).await;
    kit::seed(&mut store).await;
    assert_eq!(handle(&ctx, &req("GET", "/readyz", None)).await.unwrap().status, 200);
    assert_eq!(handle(&ctx, &req("GET", "/v1/testnet/coverage", None)).await.unwrap().status, 200);
}

#[tokio::test]
async fn a_database_that_cannot_be_reached_is_503_database_unavailable_and_recovers() {
    let s = setup!();
    assert_eq!(s.get("/v1/testnet/coverage").await.status, 200);
    // Take every connection of the API role away, then stop it reconnecting: ALTER ROLE ... NOLOGIN.
    let admin = s.db.admin().await;
    admin.batch_execute(&format!("ALTER ROLE {} NOLOGIN", s.db.api_role)).await.unwrap();
    admin
        .execute("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = $1", &[&s.db.api_role])
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let down = s.get("/v1/testnet/coverage").await;
    assert_eq!((down.status, down.header("Cache-Control")), (503, Some("no-store")));
    assert_eq!(down.body, r#"{"error":"database unavailable"}"#);
    let ready = s.get("/readyz").await;
    assert_eq!((ready.status, ready.body.as_str()), (503, r#"{"ok":false,"error":"database unavailable"}"#));
    let health = s.get("/healthz").await;
    assert_eq!(health.status, 503);
    assert!(health.body.contains(r#""lastError":"database unavailable""#), "{}", health.body);
    // The database comes back: the pool reconnects by itself.
    admin.batch_execute(&format!("ALTER ROLE {} LOGIN", s.db.api_role)).await.unwrap();
    assert_eq!(s.get("/v1/testnet/coverage").await.status, 200);
    assert_eq!(s.get("/readyz").await.status, 200);
}

#[tokio::test]
async fn a_connection_the_server_closed_is_replaced_not_served() {
    let s = setup!();
    assert_eq!(s.get("/v1/testnet/coverage").await.status, 200);
    s.db.admin()
        .await
        .execute("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = $1", &[&s.db.api_role])
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(s.get("/v1/testnet/coverage").await.status, 200);
}

#[tokio::test]
async fn a_request_that_times_out_does_not_poison_the_pool() {
    let s = setup!();
    // One slot only: a cancelled request must not leave its open transaction behind.
    let ctx =
        ApiContext { db: Db::new(Reader::new(&s.db.api, 1).unwrap()), ..context(&s.db, RateLimiter::new(1000, 1000)) };
    let _ = tokio::time::timeout(
        std::time::Duration::from_micros(1),
        handle(&ctx, &req("GET", "/v1/testnet/coverage", None)),
    )
    .await;
    for _ in 0..3 {
        assert_eq!(handle(&ctx, &req("GET", "/v1/testnet/coverage", None)).await.unwrap().status, 200);
    }
}

#[tokio::test]
async fn a_reindex_by_the_ingester_rotates_the_generation_the_api_serves() {
    let mut s = setup!();
    let first = body(&s.get("/v1/testnet/access-gate/events?limit=2").await);
    let cursor = first["cursor"].as_str().unwrap().to_owned();
    let other = vec![(Stream::AccessGate, kit::addr("0x77")), (Stream::SealedContent, kit::seal())];
    s.store.bind_packages(&other, &[Stream::AccessGate], None).await.unwrap();
    assert_eq!(s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={cursor}")).await.status, 410);
    assert_eq!(body(&s.get("/v1/testnet/coverage").await)["generation"], s.store.generation());
}

// ── socket wiring (main) ────────────────────────────────────────────────────

async fn http_get(
    addr: std::net::SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> (u16, Vec<(String, String)>, String) {
    use http_body_util::{BodyExt, Empty};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    let client: Client<_, Empty<bytes::Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let mut b = hyper::Request::builder().method(method).uri(format!("http://{addr}{target}"));
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let res = client.request(b.body(Empty::new()).unwrap()).await.unwrap();
    let status = res.status().as_u16();
    let hs = res.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned())).collect();
    let body = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    (status, hs, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serves_over_http_and_stops_on_shutdown() {
    let s = setup!();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let Setup { db: _db, store, ctx } = s;
    let ctx = Arc::new(ctx);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(crate::server::serve(listener, ctx, async {
        let _ = rx.await;
    }));
    let (status, headers, b) = http_get(addr, "GET", "/v1/testnet/coverage", &[]).await;
    assert_eq!(status, 200);
    assert!(headers.iter().any(|(k, v)| k == "access-control-allow-origin" && v == "*"));
    assert!(headers.iter().any(|(k, v)| k == "content-length" && v == &b.len().to_string()));
    assert!(b.contains(store.generation()));
    let (status, _, b) = http_get(addr, "HEAD", "/v1/testnet/coverage", &[]).await;
    assert_eq!((status, b.as_str()), (200, ""));
    // The socket address keys the rate limit when there is no CF-Connecting-IP; a header is preferred.
    let (status, _, _) = http_get(addr, "GET", "/v1/testnet/coverage", &[("cf-connecting-ip", "2001:db8::1")]).await;
    assert_eq!(status, 200);
    let (status, headers, _) = http_get(addr, "GET", "/v1/testnet/access-gate/events?limit=05", &[]).await;
    assert_eq!(status, 301);
    assert!(headers.iter().any(|(k, v)| k == "location" && v == "/v1/testnet/access-gate/events?limit=5"));
    tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(7), server)
        .await
        .expect("stops within the grace period")
        .unwrap();
    drop(store);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_never_finishes_its_headers_is_cut_off() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let s = setup!();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let Setup { db: _db, store: _store, ctx } = s;
    tokio::spawn(crate::server::serve(listener, Arc::new(ctx), std::future::pending()));
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GET /readyz HTTP/1.1\r\nHost: x\r\n").await.unwrap();
    let t = std::time::Instant::now();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(9), sock.read_to_end(&mut buf))
        .await
        .expect("closed by the server");
    let waited = t.elapsed();
    assert!(waited >= std::time::Duration::from_secs(4) && waited < std::time::Duration::from_secs(8), "{waited:?}");
}

#[tokio::test]
async fn the_context_follows_the_configuration() {
    // No database needed: routing and the rate limit precede it, and the pool connects on demand.
    let cfg = sui_indexer_store::config::ApiConfig::load(&|k| match k {
        "NETWORK" => Some("mainnet".into()),
        "DATABASE_URL" => Some("postgres://u:p@127.0.0.1:1/db?sslmode=disable".into()),
        "ALLOW_INSECURE_DATABASE" => Some("1".into()),
        "DB_POOL_SIZE" => Some("2".into()),
        "RATE_LIMIT_BURST" => Some("1".into()),
        _ => None,
    })
    .unwrap();
    let ctx = crate::context(&cfg).unwrap();
    assert_eq!(ctx.network, "mainnet");
    assert!(ctx.db.check().await.is_err());
    let r = handle(&ctx, &req("GET", "/v1/testnet/coverage", None)).await.unwrap();
    assert_eq!(r.status, 404); // routing precedes the database
    let r = handle(&ctx, &req("GET", "/v1/mainnet/coverage", None)).await.unwrap();
    assert_eq!(r.status, 429); // burst 1, the first request consumed it
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_request_head_is_refused() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let s = setup!();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let Setup { db: _db, store: _store, ctx } = s;
    tokio::spawn(crate::server::serve(listener, Arc::new(ctx), std::future::pending()));
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let big = "a".repeat(4 * crate::server::MAX_BUF);
    let req = format!("GET /readyz HTTP/1.1\r\nHost: x\r\nX-Big: {big}\r\n\r\n");
    let _ = sock.write_all(req.as_bytes()).await;
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), sock.read_to_end(&mut buf)).await;
    let head = String::from_utf8_lossy(&buf);
    assert!(head.starts_with("HTTP/1.1 431") || buf.is_empty(), "{head}");
}
