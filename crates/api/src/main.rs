//! `api` — the sui-indexer read API (no chain access, read-only database).

use std::process::ExitCode;
use std::sync::Arc;

use sui_indexer_api::{context, log, server};
use sui_indexer_store::config::ApiConfig;

fn main() -> ExitCode {
    let cfg = match ApiConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            log(&format!("refusing to start: {e}"));
            return ExitCode::FAILURE;
        }
    };
    // Worker threads follow the CPUs the container may use (cgroup quota aware), at most 4: each
    // request is a few sub-millisecond SQLite reads.
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(4);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async {
        let listener = match server::bind(cfg.port).await {
            Ok(l) => l,
            Err(e) => {
                log(&format!("cannot listen on :{}: {e}", cfg.port));
                return ExitCode::FAILURE;
            }
        };
        let ctx = Arc::new(context(&cfg));
        log(&format!(
            "sui-indexer api {} {} on :{} — database {} (read-only){}",
            env!("CARGO_PKG_VERSION"),
            cfg.network,
            cfg.port,
            cfg.db_path,
            if ctx.db.ready() { "" } else { "; not created yet, answering 503 until the ingester creates it" }
        ));
        server::serve(listener, ctx, shutdown_signal()).await;
        log("stopped");
        ExitCode::SUCCESS
    })
}

/// SIGTERM (Kubernetes) or SIGINT.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let name = tokio::select! {
        _ = term.recv() => "SIGTERM",
        _ = tokio::signal::ctrl_c() => "SIGINT",
    };
    log(&format!("{name}: shutting down"));
}
