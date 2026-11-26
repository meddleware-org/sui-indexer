//! sui-indexer API: cacheable GET pages over the index in PostgreSQL, read through the **SELECT-only**
//! role (the grants are the control; every connection and every request is additionally read-only). It
//! never migrates and has no chain access: freshness comes from the ingester's heartbeat rows
//! (`heartbeats poll:<stream>`).

/// Request handling: routing, validation, cache headers and the rate limit.
pub mod handler;
/// The query string and path canonicalisation (one cache entry per query).
pub mod query;
/// Per-client token buckets.
pub mod ratelimit;
/// The HTTP listeners.
pub mod server;

use std::sync::Mutex;

use sui_indexer_store::Reader;
use sui_indexer_store::config::ApiConfig;
use sui_indexer_store::time::{iso, now_ms};

use crate::handler::{ApiContext, Db};
use crate::ratelimit::RateLimiter;

/// A log line: `<ISO time> <message>` on stdout.
pub fn log(m: &str) {
    println!("{} {m}", iso(now_ms()));
}

/// The request context for `cfg`. Opens no connection (the pool connects on demand), so the API starts
/// and answers `503` while the database or the ingester is not up; fails only on unusable TLS material.
pub fn context(cfg: &ApiConfig) -> Result<ApiContext, sui_indexer_store::StoreError> {
    Ok(ApiContext {
        db: Db::new(Reader::new(&cfg.database, cfg.pool_size)?),
        network: cfg.network.clone(),
        limiter: Mutex::new(RateLimiter::new(cfg.rate_limit_per_sec, cfg.rate_limit_burst)),
        poll_ms: cfg.poll_ms,
        now: now_ms,
    })
}

/// The handler and server tests (they need a database; see `store::testdb`).
#[cfg(test)]
mod tests;
