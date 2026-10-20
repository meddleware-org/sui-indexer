//! `ingest` — the sui-indexer writer.
//!
//! - `ingest` runs the poll loop.
//! - `ingest --healthcheck` is the container's exec liveness probe.
//! - `ingest import-sqlite <path> [--replace-existing]` is the one-off import of a 0.0.13 SQLite index
//!   (only in an image built with `--features import-sqlite`; removed in 0.0.15).

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    if args.first().map(String::as_str) == Some("import-sqlite") {
        return import(&rt, &args[1..]);
    }
    let cfg = match IngestConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{} refusing to start: {e}", iso(now_ms()));
            return ExitCode::FAILURE;
        }
    };
    if args.first().map(String::as_str) == Some("--healthcheck") {
        return rt.block_on(async {
            match healthcheck(&cfg.database, cfg.poll_ms, now_ms()).await {
                Ok(m) => {
                    println!("ok: {m}");
                    ExitCode::SUCCESS
                }
                Err(m) => {
                    eprintln!("unhealthy: {m}");
                    ExitCode::FAILURE
                }
            }
        });
    }
    if let Some(unknown) = args.first() {
        eprintln!("unknown argument {unknown:?} (use --healthcheck, import-sqlite <path>, or none)");
        return ExitCode::from(2);
    }
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
        let outcome = run_loop(&mut ingestor, Duration::from_millis(cfg.poll_ms), shutdown_signal()).await;
        drop(ingestor.into_store());
        match outcome {
            Ok(()) => {
                log("stopped");
                ExitCode::SUCCESS
            }
            Err(e) => {
                log(&format!("stopping: {e}"));
                ExitCode::FAILURE
            }
        }
    })
}

#[cfg(feature = "import-sqlite")]
fn import(rt: &tokio::runtime::Runtime, args: &[String]) -> ExitCode {
    rt.block_on(sui_indexer_ingest::import_cmd::run(args))
}

#[cfg(not(feature = "import-sqlite"))]
fn import(_: &tokio::runtime::Runtime, _: &[String]) -> ExitCode {
    eprintln!(
        "this build has no SQLite importer (it exists only in the 0.0.14 image, built with --features import-sqlite)"
    );
    ExitCode::from(2)
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
