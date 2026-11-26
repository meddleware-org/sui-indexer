//! `ingest` — the sui-indexer writer.
//!
//! - `ingest` runs the poll loop.
//! - `ingest --healthcheck` is the container's exec liveness probe.

use std::process::ExitCode;
use std::time::Duration;

use sui_indexer_ingest::source::GrpcSource;
use sui_indexer_ingest::{healthcheck, run_loop, start, stdout_logger};
use sui_indexer_store::config::{IngestConfig, describe_url};
use sui_indexer_store::time::{iso, now_ms};

/// Print one timestamped line to standard output, where the container log collects it.
fn log(m: &str) {
    println!("{} {m}", iso(now_ms()));
}

/// Start the ingester: read the configuration, then either run the liveness check or run the poll loop until a termination signal.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // One thread is enough: the ingester is a single poll loop writing over one connection.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    // A bad or missing setting stops the process, with a message that names the variable (and never the database URL).
    let cfg = match IngestConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{} refusing to start: {e}", iso(now_ms()));
            return ExitCode::FAILURE;
        }
    };
    // The container probe runs this binary with --healthcheck: it reads the heartbeat from the database and exits 0 or 1.
    if args.first().map(String::as_str) == Some("--healthcheck") {
        return rt.block_on(async {
            match healthcheck(&cfg.database, cfg.poll_ms, now_ms()).await {
                // Healthy: print what was checked for the probe log.
                Ok(m) => {
                    println!("ok: {m}");
                    ExitCode::SUCCESS
                }
                // Unhealthy: the probe fails, so the pod is restarted.
                Err(m) => {
                    eprintln!("unhealthy: {m}");
                    ExitCode::FAILURE
                }
            }
        });
    }
    // Any other argument is a mistake; exit 2 rather than start a loop the operator did not mean to start.
    if let Some(unknown) = args.first() {
        eprintln!("unknown argument {unknown:?} (use --healthcheck or none)");
        return ExitCode::from(2);
    }
    rt.block_on(async {
        // Open the full-node client; the error names the node by its origin only, because a configured URL can carry a credential.
        let source = match GrpcSource::new(&cfg.grpc_url) {
            Ok(s) => s,
            Err(e) => {
                log(&format!("refusing to start: GRPC_URL {}: {e}", describe_url(&cfg.grpc_url)));
                return ExitCode::FAILURE;
            }
        };
        // Connect, migrate, bind the packages and check the node serves this network, before the first poll.
        let mut ingestor = match start(&cfg, source, stdout_logger()).await {
            Ok(i) => i,
            Err(e) => {
                log(&e);
                return ExitCode::FAILURE;
            }
        };
        // Poll until a termination signal; the loop returns when it is told to stop or hits a fatal error.
        let outcome = run_loop(&mut ingestor, Duration::from_millis(cfg.poll_ms), shutdown_signal()).await;
        // Close the database connection (releasing the writer lock) before the process reports its outcome.
        drop(ingestor.into_store());
        match outcome {
            // Told to stop: a clean exit.
            Ok(()) => {
                log("stopped");
                ExitCode::SUCCESS
            }
            Err(e) => {
                // A fatal error in the loop ends the process, so Kubernetes restarts it with a fresh connection.
                log(&format!("stopping: {e}"));
                ExitCode::FAILURE
            }
        }
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
