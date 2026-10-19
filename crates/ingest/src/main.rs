//! `ingest` — the sui-indexer writer. `ingest --healthcheck` is the container's exec liveness probe.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use sui_indexer_ingest::source::GrpcSource;
use sui_indexer_ingest::{healthcheck, run_loop, start, stdout_logger};
use sui_indexer_store::config::IngestConfig;
use sui_indexer_store::time::{iso, now_ms};

fn log(m: &str) {
    println!("{} {m}", iso(now_ms()));
}

fn main() -> ExitCode {
    let cfg = match IngestConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{} refusing to start: {e}", iso(now_ms()));
            return ExitCode::FAILURE;
        }
    };
    if std::env::args().nth(1).as_deref() == Some("--healthcheck") {
        return match healthcheck(Path::new(&cfg.db_path), cfg.poll_ms, now_ms()) {
            Ok(m) => {
                println!("ok: {m}");
                ExitCode::SUCCESS
            }
            Err(m) => {
                eprintln!("unhealthy: {m}");
                ExitCode::FAILURE
            }
        };
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    rt.block_on(async {
        let source = match GrpcSource::new(&cfg.grpc_url) {
            Ok(s) => s,
            Err(e) => {
                log(&format!("refusing to start: GRPC_URL {}: {e}", cfg.grpc_url));
                return ExitCode::FAILURE;
            }
        };
        let mut ingestor = match start(&cfg, source, stdout_logger()).await {
            Ok(i) => i,
            Err(e) => {
                log(&e);
                return ExitCode::FAILURE;
            }
        };
        run_loop(&mut ingestor, Duration::from_millis(cfg.poll_ms), shutdown_signal()).await;
        drop(ingestor.into_store());
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
