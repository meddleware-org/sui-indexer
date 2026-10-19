//! sui-indexer API: cacheable GET pages over the index, which it opens **read-only**
//! (`SQLITE_OPEN_READ_ONLY` + `PRAGMA query_only`; the volume is also mounted read-only). It has no
//! chain access: freshness comes from the ingester's heartbeat rows (`meta poll:<stream>`).

pub mod handler;
pub mod query;
pub mod ratelimit;
pub mod server;

use std::sync::Mutex;

use sui_indexer_store::config::ApiConfig;
use sui_indexer_store::time::{iso, now_ms};

use crate::handler::{ApiContext, Db};
use crate::ratelimit::RateLimiter;

/// A log line: `<ISO time> <message>` on stdout.
pub fn log(m: &str) {
    println!("{} {m}", iso(now_ms()));
}

/// The request context for `cfg`.
pub fn context(cfg: &ApiConfig) -> ApiContext {
    ApiContext {
        db: Db::new(&cfg.db_path),
        network: cfg.network.clone(),
        limiter: Mutex::new(RateLimiter::new(cfg.rate_limit_per_sec, cfg.rate_limit_burst)),
        poll_ms: cfg.poll_ms,
        now: now_ms,
    }
}

#[cfg(test)]
mod tests;
