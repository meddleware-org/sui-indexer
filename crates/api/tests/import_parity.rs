//! After `ingest import-sqlite` the PostgreSQL API answers exactly as the 0.0.13 SQLite API did for the same
//! data: every route of `tests/golden/routes.tsv` (heads, older pages, filters, stats, validation errors,
//! redirects, 304s, preflight) is compared status by status, header by header and byte by byte with
//! `tests/golden/0.0.13.json`, recorded from a real 0.0.13 `api` binary serving the same fixture index
//! (`scripts/golden-from-0.0.13.sh`). Run with `--features import-sqlite`.
#![cfg(feature = "import-sqlite")]

use std::sync::Mutex;

use serde::Deserialize;
use sui_indexer_api::handler::{ApiContext, ApiRequest, Db, handle};
use sui_indexer_api::ratelimit::RateLimiter;
use sui_indexer_store::import::{ImportOptions, import_sqlite};
use sui_indexer_store::{PollState, Reader, Stream, require_db};
use sui_indexer_testkit::sqlite::build;

#[derive(Deserialize)]
struct Golden {
    method: String,
    path: String,
    #[serde(rename = "requestHeader")]
    request_header: String,
    status: u16,
    headers: std::collections::BTreeMap<String, String>,
    body: String,
}

const CONTRACT_HEADERS: [&str; 10] = [
    "cache-control",
    "etag",
    "location",
    "content-type",
    "access-control-allow-origin",
    "access-control-allow-methods",
    "access-control-max-age",
    "x-content-type-options",
    "allow",
    "retry-after",
];

#[tokio::test]
async fn the_imported_index_answers_like_the_sqlite_one() {
    let db = require_db!();
    let golden: Vec<Golden> = serde_json::from_str(include_str!("../../../tests/golden/0.0.13.json")).unwrap();
    assert!(golden.len() > 40);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    build(&path);
    let mut store = db.store().await;
    import_sqlite(&mut store, &path, &ImportOptions { replace_existing: false, backup_schema: "bak_1".into() })
        .await
        .unwrap();
    // Heartbeats are not imported (the ingester writes its own on its first poll); install the values the
    // fixture's SQLite index carried so that `polledAt` can be compared too.
    for s in Stream::ALL {
        let hb = PollState { last_success_at: 1_760_000_000_000, last_error: None, last_attempt_at: 1_760_000_000_000 };
        store.record_poll(s, &hb).await.unwrap();
    }
    let ctx = ApiContext {
        db: Db::new(Reader::new(&db.api, 2).unwrap()),
        network: "testnet".into(),
        limiter: Mutex::new(RateLimiter::new(10_000, 10_000)),
        poll_ms: 15_000,
        now: sui_indexer_store::time::now_ms,
    };

    let mut checked = 0;
    for g in &golden {
        let (p, q) = match g.path.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (g.path.as_str(), None),
        };
        let inm = g.request_header.strip_prefix("If-None-Match: ");
        let req = ApiRequest {
            method: &g.method,
            path: p,
            query: q,
            cf_connecting_ip: None,
            if_none_match: inm,
            remote_address: Some("10.0.0.1"),
        };
        let res = handle(&ctx, &req).await.unwrap_or_else(|e| panic!("{} {}: internal error {e}", g.method, g.path));
        let what = format!("{} {}", g.method, g.path);
        assert_eq!(res.status, g.status, "{what}: status");
        let got: std::collections::BTreeMap<String, String> = res
            .headers
            .iter()
            .filter(|(k, _)| CONTRACT_HEADERS.contains(&k.to_ascii_lowercase().as_str()))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect();
        assert_eq!(got, g.headers, "{what}: headers");
        assert_eq!(res.body, g.body, "{what}: body");
        checked += 1;
    }
    assert_eq!(checked, golden.len());
}
