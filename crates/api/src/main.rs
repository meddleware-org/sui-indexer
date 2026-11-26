//! `api` — the sui-indexer read API (no chain access; the SELECT-only database role).

use std::process::ExitCode;
use std::sync::Arc;

use sui_indexer_api::{context, log, server};
use sui_indexer_store::config::ApiConfig;

/// Start the API: read the configuration, bind the public and metrics ports, serve until a termination signal, then drain.
fn main() -> ExitCode {
    // A bad or missing setting stops the process before it listens, with a message that names the variable.
    let cfg = match ApiConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            // Refuse to start: a service that cannot read its configuration would only answer wrongly.
            log(&format!("refusing to start: {e}"));
            return ExitCode::FAILURE;
        }
    };
    // Worker threads follow the CPUs the container may use (cgroup quota aware), at most 4: each
    // request is one small indexed read.
    // Use the CPUs the container may use.
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(4);
    // A multi-threaded runtime: unlike the ingester, the API serves many connections at once.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .expect("tokio runtime");
    // Everything after the runtime is built runs inside it: bind, build the context, serve.
    rt.block_on(async {
        // Bind first, so a port in use is reported before the database is touched.
        let listener = match server::bind(cfg.port).await {
            Ok(l) => l,
            Err(e) => {
                log(&format!("cannot listen on :{}: {e}", cfg.port));
                return ExitCode::FAILURE;
            }
        };
        // Build the request context: the read-only pool (it connects lazily), the rate limiter and the clock.
        let ctx = match context(&cfg) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                log(&format!("refusing to start: {e}"));
                return ExitCode::FAILURE;
            }
        };
        log(&format!(
            "sui-indexer api {} {} on :{} — database {} at {}:{} as {} (read-only, pool {}){}",
            env!("CARGO_PKG_VERSION"),
            cfg.network,
            cfg.port,
            cfg.database.dbname(),
            cfg.database.host(),
            cfg.database.port(),
            cfg.database.user(),
            cfg.pool_size,
            // Ask the database once now, only to say in the log whether the first requests will be answered 503.
            match ctx.db.check().await {
                Ok(()) => String::new(),
                Err(e) => format!("; {}: answering 503 until the ingester has migrated and started", e.message),
            }
        ));
        // `/metrics` on its own port (never in the Service or the Ingress), stopped with the API.
        // A watch channel carries the shutdown to the metrics listener, which runs as its own task.
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(());
        // The metrics port is optional: 0 turns it off.
        if cfg.metrics_port != 0 {
            match server::bind(cfg.metrics_port).await {
                Ok(metrics_listener) => {
                    // The metrics task gets its own handle on the context and its own shutdown receiver.
                    let (ctx, mut stop) = (ctx.clone(), stop_rx.clone());
                    log(&format!("metrics on :{}/metrics", cfg.metrics_port));
                    tokio::spawn(async move {
                        server::serve_metrics(metrics_listener, ctx, async move {
                            let _ = stop.changed().await;
                        })
                        .await;
                    });
                }
                Err(e) => log(&format!(
                    "cannot listen on :{} for metrics: {e} (serving the API without them)",
                    cfg.metrics_port
                )),
            }
        }
        // Serve the public API until a termination signal, then tell the metrics listener to stop too.
        server::serve(listener, ctx, async move {
            shutdown_signal().await;
            let _ = stop_tx.send(());
        })
        .await;
        // Reached only after in-flight requests have drained (or the grace period ended).
        log("stopped");
        ExitCode::SUCCESS
    })
}

/// SIGTERM (Kubernetes) or SIGINT.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    // Kubernetes stops a pod with SIGTERM; SIGINT is for a terminal.
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    // Whichever signal arrives first ends the wait.
    let name = tokio::select! {
        _ = term.recv() => "SIGTERM",
        _ = tokio::signal::ctrl_c() => "SIGINT",
    };
    // Say which signal ended the process, so a restart can be told from a manual stop.
    log(&format!("{name}: shutting down"));
}
