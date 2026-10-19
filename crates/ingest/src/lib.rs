//! sui-indexer ingester: the **sole writer** of the index.
//!
//! Startup order (each step can refuse to start, exiting non-zero):
//!
//! 1. open the database (creating it and its schema on a fresh volume);
//! 2. on the first open of a database written by the TypeScript indexer (≤ 0.0.11), copy it to
//!    `<DB_PATH>.bak-pre-rust-<ms>` (`VACUUM INTO`) and record `meta schema_writer = rust`;
//! 3. bind the streams to their packages (a change needs `REINDEX_CONFIRM`, see the store);
//! 4. check the node's chain identity against `NETWORK`;
//! 5. poll every `POLL_MS` until SIGTERM/SIGINT.

pub mod chain;
pub mod decode;
pub mod ingestor;
pub mod source;

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use sui_indexer_store::config::IngestConfig;
use sui_indexer_store::time::{iso, now_ms};
use sui_indexer_store::{ALIVE_KEY, BindError, Reader, Store, Stream};

use crate::chain::{known_chain_id, short_chain_id};
use crate::decode::Packages;
use crate::ingestor::{IngestOptions, Ingestor, Logger};
use crate::source::EventSource;

/// How long the startup chain-identity request may take.
pub const CHAIN_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// A log line: `<ISO time> <message>` on stdout (the TypeScript indexer's format).
pub fn stdout_logger() -> Logger {
    Box::new(|m| println!("{} {m}", iso(now_ms())))
}

/// Open the store and run steps 2–4 of the startup order. Errors are the refusal messages.
pub async fn start<S: EventSource>(cfg: &IngestConfig, source: S, log: Logger) -> Result<Ingestor<S>, String> {
    let mut store = Store::open(&cfg.db_path).map_err(|e| format!("cannot open {}: {e}", cfg.db_path))?;
    store.touch_alive(now_ms()).map_err(|e| e.to_string())?;
    let pre_rust = format!("{}.bak-pre-rust-{}", cfg.db_path, now_ms());
    if store.mark_rust_writer(&pre_rust).map_err(|e| format!("cutover backup failed: {e}"))? {
        log(&format!("first start on a database written by the previous indexer: backed it up to {pre_rust}"));
    }
    let packages = [
        (Stream::AccessGate, cfg.access_gate_original_id.clone()),
        (Stream::SealedContent, cfg.seal_original_id.clone()),
    ];
    let backup = format!("{}.bak-{}", cfg.db_path, now_ms());
    let cleared = match store.bind_packages(&packages, &cfg.reindex_confirm, Some(&backup)) {
        Ok(c) => c,
        Err(BindError::PackageChanged(e)) => return Err(format!("refusing to start: {e}")),
        Err(BindError::Sql(e)) => return Err(e.to_string()),
    };
    if !cleared.is_empty() {
        let list = cleared.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
        log(&format!("package changed for {list}: backed up, cleared and re-indexing from the start"));
    }
    // The node must serve the network this instance is configured for: a mainnet node behind a
    // testnet instance would index nothing and stay green.
    if let Some(want) = known_chain_id(&cfg.network) {
        let id = tokio::time::timeout(CHAIN_CHECK_TIMEOUT, source.chain_identifier())
            .await
            .map_err(|_| format!("refusing to start: {} did not report its chain identifier in time", cfg.grpc_url))?
            .map_err(|e| format!("refusing to start: cannot read the chain identifier of {}: {e}", cfg.grpc_url))?;
        let got = short_chain_id(&id).map_err(|e| format!("refusing to start: {e}"))?;
        if got != want {
            return Err(format!(
                "refusing to start: {} serves chain {got}, not {} ({want})",
                cfg.grpc_url, cfg.network
            ));
        }
    }
    log(&format!(
        "sui-indexer ingest {} {} — access_gate {}, seal_policies {}, generation {}",
        env!("CARGO_PKG_VERSION"),
        cfg.network,
        cfg.access_gate_original_id,
        cfg.seal_original_id,
        store.generation()
    ));
    let opts = IngestOptions::new(Packages {
        access_gate_original_id: cfg.access_gate_original_id.clone(),
        seal_original_id: cfg.seal_original_id.clone(),
    });
    Ok(Ingestor::new(store, source, opts, log))
}

/// Poll now and then every `interval` until `shutdown` resolves; a failed poll is logged and retried
/// next interval. A shutdown during a poll cancels it between requests (a page and its cursor commit
/// in one synchronous transaction, so nothing is half-written).
pub async fn run_loop<S: EventSource>(
    ingestor: &mut Ingestor<S>,
    interval: Duration,
    shutdown: impl Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = ingestor.poll() => {}
            () = &mut shutdown => return,
        }
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = &mut shutdown => return,
        }
    }
}

/// The exec liveness probe (`ingest --healthcheck`): healthy while the ingester refreshed
/// `meta ingest:alive` within `5 × POLL_MS + 30 s` (it does so before and after every full-node
/// request, and each request is bounded by the 30 s timeout). A full node that is down does NOT
/// fail it — restarting would not help, and `/healthz` reports that — only a stuck process does.
pub fn healthcheck(db_path: &Path, poll_ms: u64, now: i64) -> Result<String, String> {
    let reader = Reader::open(db_path).map_err(|e| e.to_string())?;
    let alive = reader
        .meta(ALIVE_KEY)
        .map_err(|e| e.to_string())?
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or("no liveness heartbeat recorded")?;
    let limit = i64::try_from(poll_ms.saturating_mul(5)).unwrap_or(i64::MAX).saturating_add(30_000);
    let age = now - alive;
    if age <= limit {
        Ok(format!("alive {age} ms ago"))
    } else {
        Err(format!("last heartbeat {age} ms ago (limit {limit} ms)"))
    }
}

#[cfg(test)]
mod tests;
