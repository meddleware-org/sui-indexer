//! API tests: every case of the TypeScript `tests/api.test.ts` (the client-contract cases run in
//! CI's `contract` job against the binary, with the real client packages), plus the read-only
//! database lifecycle and the socket wiring.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use sui_indexer_store::time::now_ms;
use sui_indexer_store::{PollState, Store, Stream};
use sui_indexer_testkit::{self as kit, gate_a, gate_b, minted};

use crate::handler::{ApiContext, ApiRequest, ApiResponse, Db, handle};
use crate::ratelimit::RateLimiter;

struct Setup {
    _dir: tempfile::TempDir,
    store: Store,
    ctx: ApiContext,
}

fn setup_with(limiter: RateLimiter) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    let mut store = Store::open(&path).unwrap();
    store.bind_packages(&[(Stream::AccessGate, kit::ag()), (Stream::SealedContent, kit::seal())], &[], None).unwrap();
    kit::seed(&store);
    let ctx = ApiContext {
        db: Db::new(&path),
        network: "testnet".into(),
        limiter: Mutex::new(limiter),
        poll_ms: 15_000,
        now: now_ms,
    };
    let s = Setup { _dir: dir, store, ctx };
    s.set_last_poll(now_ms(), Stream::AccessGate);
    s.set_last_poll(now_ms(), Stream::SealedContent);
    s
}

fn setup() -> Setup {
    setup_with(RateLimiter::new(1000, 1000))
}

impl Setup {
    fn set_last_poll(&self, t: i64, stream: Stream) {
        let mut st = self.store.poll_state(stream).unwrap();
        st.last_success_at = t;
        self.store.record_poll(stream, &st).unwrap();
    }

    fn request(&self, method: &str, url: &str, headers: &[(&str, &str)]) -> ApiResponse {
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
        .expect("no internal error")
    }

    fn get(&self, url: &str) -> ApiResponse {
        self.request("GET", url, &[])
    }

    fn get_h(&self, url: &str, headers: &[(&str, &str)]) -> ApiResponse {
        self.request("GET", url, headers)
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

#[test]
fn serves_the_newest_page_with_short_cache_headers_and_an_etag() {
    let s = setup();
    let r = s.get("/v1/testnet/access-gate/events?limit=2");
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
    assert_eq!(s.get_h("/v1/testnet/access-gate/events?limit=2", &[("if-none-match", &etag)]).status, 304);
}

#[test]
fn serves_older_pages_as_immutable_and_ends_with_a_null_cursor() {
    let s = setup();
    let first = body(&s.get("/v1/testnet/access-gate/events?limit=2"));
    let second = s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={}", first["cursor"].as_str().unwrap()));
    assert_eq!(second.header("Cache-Control"), Some("public, max-age=31536000, immutable"));
    let b2 = body(&second);
    assert_eq!(checkpoints(&b2), ["3", "2"]);
    let b3 = body(&s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={}", b2["cursor"].as_str().unwrap())));
    assert_eq!(checkpoints(&b3), ["1"]);
    assert!(b3["cursor"].is_null());
}

#[test]
fn filters_by_kinds_and_gate() {
    let s = setup();
    let b = body(&s.get(&format!("/v1/testnet/access-gate/events?kinds=AccessConsumed&gate={}", gate_a())));
    let events = b["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0]["eventType"].as_str().unwrap().ends_with("AccessConsumedEvent"));
}

#[test]
fn refuses_a_cursor_from_another_index_generation() {
    let s = setup();
    let r = s.get("/v1/testnet/access-gate/events?before=000000000000.5");
    assert_eq!(r.status, 410);
    assert_eq!(body(&r)["error"], "cursor is from an earlier index; start from the newest page");
    assert_eq!(r.header("Cache-Control"), Some("no-store"));
}

#[test]
fn validates_strictly() {
    let s = setup();
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
        let r = s.get(url);
        assert_eq!(r.status, 400, "{url}");
        assert_eq!(body(&r)["error"], msg, "{url}");
        assert_eq!(r.header("Cache-Control"), Some("no-store"), "{url}");
    }
    let r = s.get(&format!("/v1/testnet/sealed-content?gate={}&limit=201", full_a()));
    assert_eq!((r.status, body(&r)["error"].as_str().unwrap()), (400, "limit must be in [1, 200]"));
    let r = s.get("/v1/mainnet/access-gate/events");
    assert_eq!((r.status, body(&r)["error"].as_str().unwrap()), (404, "this indexer serves testnet only"));
    assert_eq!(r.header("Cache-Control"), Some("public, max-age=60"));
    for url in ["/v1/testnet/nothing", "/", "/v2/testnet/coverage", "/v1/testnet", "/v1/testnet/gates/0xa/other"] {
        let r = s.get(url);
        assert_eq!((r.status, body(&r)["error"].as_str().unwrap()), (404, "not found"), "{url}");
    }
    // Every published kind is accepted.
    let all = crate::handler::ACCESS_GATE_EVENT_KINDS.join(",");
    let mut sorted: Vec<&str> = crate::handler::ACCESS_GATE_EVENT_KINDS.to_vec();
    sorted.sort();
    assert_eq!(s.get(&format!("/v1/testnet/access-gate/events?kinds={}", sorted.join("%2C"))).status, 200);
    assert_eq!(s.get(&format!("/v1/testnet/access-gate/events?kinds={all}")).status, 301);
}

#[test]
fn lists_sealed_content_for_one_gate() {
    let s = setup();
    let b = body(&s.get(&format!("/v1/testnet/sealed-content?gate={}", gate_a())));
    assert_eq!(b["events"].as_array().unwrap().len(), 2);
    assert_eq!(checkpoints(&b), ["8", "6"]);
}

#[test]
fn rows_are_served_verbatim_in_the_clients_shape() {
    let s = setup();
    let r = s.get(&format!("/v1/testnet/sealed-content?gate={}&limit=1", gate_b()));
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

#[test]
fn counts_a_gates_events_and_sums_commission_exactly() {
    let s = setup();
    let big = 1u64 << 60;
    s.store
        .ingest(
            Stream::AccessGate,
            &[minted(&gate_a(), 10, big).indexed(), minted(&gate_a(), 11, big).indexed()],
            Some("c2"),
        )
        .unwrap();
    let r = s.get(&format!("/v1/testnet/gates/{}/stats", gate_a()));
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

#[test]
fn stats_of_an_unknown_gate_are_zero() {
    let s = setup();
    let b = body(&s.get(&format!("/v1/testnet/gates/0x{}/stats", "f".repeat(64))));
    assert_eq!((b["minted"].as_i64(), b["commissionMist"].as_str()), (Some(0), Some("0")));
}

#[test]
fn reports_coverage_per_stream() {
    let s = setup();
    let r = s.get("/v1/testnet/coverage");
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

#[test]
fn healthz_is_503_before_the_first_poll_and_after_polls_stall_and_names_the_stream_and_its_error() {
    let s = setup();
    assert_eq!(s.get("/healthz").status, 200);
    s.set_last_poll(0, Stream::AccessGate);
    assert_eq!(s.get("/healthz").status, 503);
    s.store
        .record_poll(
            Stream::AccessGate,
            &PollState {
                last_success_at: now_ms() - 5 * 15_000 - 1,
                last_error: Some("requested data below earliest available".into()),
                last_attempt_at: now_ms(),
            },
        )
        .unwrap();
    let res = s.get("/healthz");
    assert_eq!(res.status, 503);
    assert_eq!(res.header("Cache-Control"), Some("no-store"));
    let b = body(&res);
    assert_eq!(b["ok"], false);
    assert_eq!(b["streams"]["access-gate"]["ok"], false);
    assert_eq!(b["streams"]["access-gate"]["lastError"], "requested data below earliest available");
    assert!(b["streams"]["access-gate"]["lastPollAgeMs"].as_i64().unwrap() > 75_000);
    assert_eq!(b["streams"]["sealed-content"]["ok"], true); // the other stream is fine
    assert!(b["streams"]["sealed-content"]["lastError"].is_null());
    assert_eq!(s.get("/readyz").status, 200); // still serving
    assert_eq!(s.get("/readyz").body, r#"{"ok":true}"#);
}

#[test]
fn head_pages_say_how_fresh_the_index_is_and_older_pages_stay_free_of_it() {
    let s = setup();
    let head = body(&s.get("/v1/testnet/access-gate/events?limit=2"));
    assert_eq!(head["latestCheckpoint"], "5");
    assert!(head["polledAt"].as_i64().unwrap() > 0);
    let older =
        body(&s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={}", head["cursor"].as_str().unwrap())));
    let o = older.as_object().unwrap();
    assert!(!o.contains_key("polledAt") && !o.contains_key("latestCheckpoint"));
    // Before any poll, polledAt is null (not 0).
    s.set_last_poll(0, Stream::AccessGate);
    assert!(body(&s.get("/v1/testnet/access-gate/events"))["polledAt"].is_null());
}

#[test]
fn an_empty_stream_has_no_coverage_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    drop(Store::open(&path).unwrap());
    let ctx = ApiContext {
        db: Db::new(&path),
        network: "testnet".into(),
        limiter: Mutex::new(RateLimiter::new(100, 100)),
        poll_ms: 15_000,
        now: now_ms,
    };
    let r = handle(&ctx, &req("GET", "/v1/testnet/access-gate/events", None)).unwrap();
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

#[test]
fn redirects_other_spellings_of_a_query_to_the_canonical_form_cacheably() {
    let s = setup();
    let full = full_a();
    assert_eq!(
        loc(&s.get("/v1/testnet/access-gate/events?gate=0xA")),
        Some(format!("/v1/testnet/access-gate/events?gate={full}"))
    );
    assert_eq!(
        loc(&s.get("/v1/testnet/access-gate/events?gate=0x0a")),
        Some(format!("/v1/testnet/access-gate/events?gate={full}"))
    );
    assert_eq!(
        loc(&s.get(&format!("/v1/testnet/access-gate/events?gate=0x{}", full[2..].to_uppercase()))),
        Some(format!("/v1/testnet/access-gate/events?gate={full}"))
    );
    assert_eq!(
        loc(&s.get("/v1/testnet/access-gate/events?limit=05")).as_deref(),
        Some("/v1/testnet/access-gate/events?limit=5")
    );
    assert_eq!(
        loc(&s.get("/v1/testnet/access-gate/events?kinds=AccessMinted,AccessConsumed,AccessMinted")).as_deref(),
        Some("/v1/testnet/access-gate/events?kinds=AccessConsumed%2CAccessMinted")
    );
    assert_eq!(loc(&s.get("/v1/testnet/gates/0xA/stats")), Some(format!("/v1/testnet/gates/{full}/stats")));
    let r = s.get("/v1/testnet/access-gate/events?limit=05");
    assert_eq!(r.header("Cache-Control"), Some("public, max-age=86400"));
    assert_eq!(r.header("Access-Control-Allow-Origin"), Some("*"));
    assert_eq!(r.body, "");
}

#[test]
fn serves_canonical_queries_without_a_redirect() {
    let s = setup();
    let g = full_a();
    for url in [
        format!("/v1/testnet/access-gate/events?kinds=AccessConsumed%2CAccessMinted&gate={g}&limit=10"),
        format!("/v1/testnet/access-gate/events?gate={g}&limit=5"),
        // A raw comma serialises the same as %2C: not a different spelling.
        format!("/v1/testnet/access-gate/events?kinds=AccessConsumed,AccessMinted&gate={g}"),
        "/v1/testnet/coverage".to_owned(),
        format!("/v1/testnet/gates/{g}/stats"),
    ] {
        assert_eq!(s.get(&url).status, 200, "{url}");
    }
}

#[test]
fn the_redirect_target_is_itself_canonical() {
    let s = setup();
    let first = s.get("/v1/testnet/access-gate/events?limit=05&gate=0xA");
    assert_eq!(first.status, 301);
    assert_eq!(s.get(first.header("Location").unwrap()).status, 200);
}

#[test]
fn dot_segments_resolve_like_the_url_parser() {
    let s = setup();
    assert_eq!(s.get("/v1/./testnet/x/../coverage").status, 200);
    assert_eq!(s.get("//v1//testnet//coverage").status, 200); // empty segments are ignored by the router
}

// ── transport rules ─────────────────────────────────────────────────────────

#[test]
fn revalidates_with_weak_listed_or_wildcard_if_none_match() {
    let s = setup();
    let etag = s.get("/v1/testnet/access-gate/events").header("ETag").unwrap().to_owned();
    for h in [etag.clone(), format!("W/{etag}"), format!("\"zzz\", {etag}"), "*".into()] {
        let r = s.get_h("/v1/testnet/access-gate/events", &[("if-none-match", &h)]);
        assert_eq!(r.status, 304, "{h}");
        assert_eq!(r.header("ETag"), Some(etag.as_str()));
        assert_eq!(r.header("Cache-Control"), Some("public, max-age=10, s-maxage=30"));
        assert_eq!(r.body, "");
    }
    assert_eq!(s.get_h("/v1/testnet/access-gate/events", &[("if-none-match", "\"other\"")]).status, 200);
    // Older pages carry a `b<id>` ETag.
    let first = body(&s.get("/v1/testnet/access-gate/events?limit=2"));
    let older = s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={}", first["cursor"].as_str().unwrap()));
    assert!(older.header("ETag").unwrap().contains("-b"));
}

#[test]
fn a_new_event_changes_the_head_etag() {
    let s = setup();
    let before = s.get("/v1/testnet/access-gate/events").header("ETag").unwrap().to_owned();
    s.store.ingest(Stream::AccessGate, &[minted(&gate_a(), 99, 1).indexed()], Some("c3")).unwrap();
    let after = s.get_h("/v1/testnet/access-gate/events", &[("if-none-match", &before)]);
    assert_eq!(after.status, 200);
    assert_ne!(after.header("ETag").unwrap(), before);
}

#[test]
fn rate_limits_per_client_ip_preferring_cf_connecting_ip_and_never_limits_health() {
    let s = setup_with(RateLimiter::new(1, 2));
    let cf = [("cf-connecting-ip", "1.1.1.1")];
    assert_eq!(s.get_h("/v1/testnet/coverage", &cf).status, 200);
    assert_eq!(s.get_h("/v1/testnet/coverage", &cf).status, 200);
    let limited = s.get_h("/v1/testnet/coverage", &cf);
    assert_eq!(limited.status, 429);
    assert!(limited.header("Retry-After").unwrap().parse::<u64>().unwrap() >= 1);
    assert_eq!(limited.header("Cache-Control"), Some("no-store"));
    assert_eq!(body(&limited)["error"], "rate limited");
    assert_eq!(s.get_h("/v1/testnet/coverage", &[("cf-connecting-ip", "2.2.2.2")]).status, 200);
    assert_eq!(s.get_h("/healthz", &cf).status, 200);
    assert_eq!(s.get_h("/readyz", &cf).status, 200);
    // Without the header the socket address is the key.
    assert_eq!(s.get("/v1/testnet/coverage").status, 200);
}

#[test]
fn answers_preflight_refuses_writes_and_sends_no_body_for_head() {
    let s = setup();
    let pre = s.request("OPTIONS", "/v1/testnet/coverage", &[]);
    assert_eq!(pre.status, 204);
    assert_eq!(pre.header("Access-Control-Allow-Methods"), Some("GET, HEAD, OPTIONS"));
    assert_eq!(pre.header("Access-Control-Max-Age"), Some("86400"));
    let post = s.request("POST", "/v1/testnet/coverage", &[]);
    assert_eq!(post.status, 405);
    assert_eq!(post.header("Allow"), Some("GET, HEAD, OPTIONS"));
    assert_eq!(body(&post)["error"], "method not allowed");
    let head = s.request("HEAD", "/v1/testnet/coverage", &[]);
    assert_eq!(head.status, 200);
    assert_eq!(head.body, "");
    assert_eq!(s.request("HEAD", "/v1/testnet/nothing", &[]).body, "");
}

// ── the database lifecycle (read-only API, ingester creates the database) ──

#[test]
fn before_the_ingester_creates_the_database_everything_but_options_is_503() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    let ctx = ApiContext {
        db: Db::new(&path),
        network: "testnet".into(),
        limiter: Mutex::new(RateLimiter::new(100, 100)),
        poll_ms: 15_000,
        now: now_ms,
    };
    let ready = handle(&ctx, &req("GET", "/readyz", None)).unwrap();
    assert_eq!((ready.status, ready.body.as_str()), (503, r#"{"ok":false,"error":"index not created yet"}"#));
    let health = handle(&ctx, &req("GET", "/healthz", None)).unwrap();
    assert_eq!(health.status, 503);
    assert_eq!(
        health.body,
        r#"{"ok":false,"streams":{"access-gate":{"ok":false,"lastPollAgeMs":null,"lastError":"index not created yet"},"sealed-content":{"ok":false,"lastPollAgeMs":null,"lastError":"index not created yet"}}}"#
    );
    for url in ["/v1/testnet/coverage", "/v1/testnet/access-gate/events", "/v1/testnet/gates/0xa/stats"] {
        let path = url.replace("/0xa/", &format!("/{}/", full_a()));
        let r = handle(&ctx, &req("GET", &path, None)).unwrap();
        assert_eq!((r.status, r.header("Cache-Control")), (503, Some("no-store")), "{url}");
        assert_eq!(r.body, r#"{"error":"index not created yet"}"#);
    }
    assert!(!path.exists(), "the API never creates the database");
    // The ingester creates it: the same context starts serving.
    let mut store = Store::open(&path).unwrap();
    store.bind_packages(&[(Stream::AccessGate, kit::ag()), (Stream::SealedContent, kit::seal())], &[], None).unwrap();
    kit::seed(&store);
    assert_eq!(handle(&ctx, &req("GET", "/readyz", None)).unwrap().status, 200);
    assert_eq!(handle(&ctx, &req("GET", "/v1/testnet/coverage", None)).unwrap().status, 200);
}

#[test]
fn a_reindex_by_the_ingester_rotates_the_generation_the_api_serves() {
    let mut s = setup();
    let first = body(&s.get("/v1/testnet/access-gate/events?limit=2"));
    let cursor = first["cursor"].as_str().unwrap().to_owned();
    let other = vec![(Stream::AccessGate, kit::addr("0x77")), (Stream::SealedContent, kit::seal())];
    s.store.bind_packages(&other, &[Stream::AccessGate], None).unwrap();
    assert_eq!(s.get(&format!("/v1/testnet/access-gate/events?limit=2&before={cursor}")).status, 410);
    assert_eq!(body(&s.get("/v1/testnet/coverage"))["generation"], s.store.generation());
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
    let s = setup();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let Setup { _dir, store, ctx } = s;
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
    let s = setup();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let Setup { _dir, store: _store, ctx } = s;
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

#[test]
fn the_context_follows_the_configuration() {
    let cfg = sui_indexer_store::config::ApiConfig::load(&|k| match k {
        "NETWORK" => Some("mainnet".into()),
        "DB_PATH" => Some("/nonexistent/indexer.db".into()),
        "RATE_LIMIT_BURST" => Some("1".into()),
        _ => None,
    })
    .unwrap();
    let ctx = crate::context(&cfg);
    assert_eq!(ctx.network, "mainnet");
    assert!(!ctx.db.ready());
    let r = handle(&ctx, &req("GET", "/v1/testnet/coverage", None)).unwrap();
    assert_eq!(r.status, 404); // routing precedes the database
    let r = handle(&ctx, &req("GET", "/v1/mainnet/coverage", None)).unwrap();
    assert_eq!(r.status, 429); // burst 1, the first request consumed it
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_request_head_is_refused() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let s = setup();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let Setup { _dir, store: _store, ctx } = s;
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
