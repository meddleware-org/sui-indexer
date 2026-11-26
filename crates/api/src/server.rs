//! The HTTP/1.1 transport: hyper with the TypeScript server's limits (5 s to receive the request
//! headers, 10 s for the whole request) and a graceful stop on shutdown (in-flight requests get up
//! to 5 s).

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;

use crate::handler::{ApiContext, ApiRequest, ApiResponse, handle, internal_error, metrics};
use crate::log;

/// How long a client may take to send its request head before the connection is closed (slow-header protection).
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one request may take to answer before it is abandoned.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long in-flight requests are given to finish after shutdown starts.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// Open connections served at once; further ones wait in the listen backlog until one closes.
/// Bodies are never read, so a connection costs its read buffer (at most [`MAX_BUF`]) plus a task:
/// 1024 × 32 KiB bounds header buffering at 32 MiB, inside the container's 128 MiB limit.
pub const MAX_CONNECTIONS: usize = 1024;
/// Largest request head buffered (hyper answers `431` beyond it). nginx in front allows 8 KiB lines.
pub const MAX_BUF: usize = 32 * 1024;

/// All values of `name`, joined by `, ` (how Node.js reports a repeated header).
fn header(req: &Request<Incoming>, name: &str) -> Option<String> {
    let values: Vec<&str> = req.headers().get_all(name).iter().filter_map(|v| v.to_str().ok()).collect();
    (!values.is_empty()).then(|| values.join(", "))
}

/// Convert the handler's answer into a hyper response; a header that is not valid is skipped rather than failing the response.
fn to_hyper(res: ApiResponse) -> Response<Full<Bytes>> {
    let mut out = Response::new(Full::new(Bytes::from(res.body)));
    *out.status_mut() = StatusCode::from_u16(res.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    for (k, v) in res.headers {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(&v)) {
            out.headers_mut().append(name, value);
        }
    }
    out
}

/// Answer one request (the transport-independent [`handle`] plus the 500 fallback).
pub async fn respond(ctx: &ApiContext, req: &Request<Incoming>, peer: &SocketAddr) -> Response<Full<Bytes>> {
    // `CF-Connecting-IP` is the client address the Cloudflare edge saw; the socket peer is only the edge or the ingress.
    let cf = header(req, "cf-connecting-ip");
    let inm = header(req, "if-none-match");
    let remote = peer.ip().to_string();
    let api = ApiRequest {
        method: req.method().as_str(),
        path: req.uri().path(),
        query: req.uri().query(),
        cf_connecting_ip: cf.as_deref(),
        if_none_match: inm.as_deref(),
        remote_address: Some(&remote),
    };
    // Only an internal failure comes back as an error: its detail is logged here and the client gets a generic 500.
    match handle(ctx, &api).await {
        Ok(res) => to_hyper(res),
        Err(detail) => {
            log(&format!("request failed: {} {}: {detail}", api.method, api.path));
            to_hyper(internal_error())
        }
    }
}

/// Bind `[::]:port` (dual stack, as Node.js does), or `0.0.0.0:port` where IPv6 is unavailable.
pub async fn bind(port: u16) -> std::io::Result<TcpListener> {
    // A dual-stack socket takes IPv4 and IPv6 on one port; hosts without IPv6 fall back to IPv4 only.
    match TcpListener::bind(("::", port)).await {
        Ok(l) => Ok(l),
        Err(_) => TcpListener::bind(("0.0.0.0", port)).await,
    }
}

/// The metrics listener's one route: `GET /metrics`; anything else is 404.
pub async fn respond_metrics(ctx: &ApiContext, req: &Request<Incoming>) -> Response<Full<Bytes>> {
    if req.method() == hyper::Method::GET && req.uri().path() == "/metrics" {
        to_hyper(metrics(ctx).await)
    } else {
        let mut r = Response::new(Full::new(Bytes::new()));
        *r.status_mut() = StatusCode::NOT_FOUND;
        r
    }
}

/// Serve the public API until `shutdown` resolves, then stop accepting and give in-flight requests
/// the grace period.
pub async fn serve(listener: TcpListener, ctx: Arc<ApiContext>, shutdown: impl Future<Output = ()>) {
    serve_inner(listener, ctx, shutdown, false).await;
}

/// Serve `/metrics` alone on its own listener (`METRICS_PORT`), with the same limits and shutdown.
pub async fn serve_metrics(listener: TcpListener, ctx: Arc<ApiContext>, shutdown: impl Future<Output = ()>) {
    serve_inner(listener, ctx, shutdown, true).await;
}

/// The accept loop shared by the public and the metrics listener: take a connection slot, accept, serve it with the request timeouts, and stop on the shutdown signal after letting in-flight requests finish. `metrics_only` selects which routes the listener answers.
async fn serve_inner(
    listener: TcpListener,
    ctx: Arc<ApiContext>,
    shutdown: impl Future<Output = ()>,
    metrics_only: bool,
) {
    // Tracks open connections so shutdown can wait for them.
    let graceful = GracefulShutdown::new();
    let mut builder = http1::Builder::new();
    // A header-read timeout needs a timer; the buffer cap makes hyper answer 431 beyond the largest head we accept.
    builder.timer(TokioTimer::new()).header_read_timeout(HEADER_TIMEOUT).max_buf_size(MAX_BUF);
    // One permit per open connection; when none is free the accept loop waits and clients queue in the backlog.
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    tokio::pin!(shutdown);
    loop {
        // Wait for a free slot, or for the shutdown signal, whichever comes first.
        let permit = tokio::select! {
            p = slots.clone().acquire_owned() => p.expect("the semaphore is never closed"),
            () = &mut shutdown => break,
        };
        // Accept the next connection, or stop on shutdown.
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(a) => a,
                Err(e) => {
                    // A failed accept (out of descriptors, say) is logged and retried after a pause instead of ending the server.
                    log(&format!("accept failed: {e}"));
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let ctx = ctx.clone();
        // One service per connection: each request gets the request timeout, and a timeout answers 408.
        let service = service_fn(move |req: Request<Incoming>| {
            let ctx = ctx.clone();
            async move {
                let res = if metrics_only {
                    tokio::time::timeout(REQUEST_TIMEOUT, respond_metrics(&ctx, &req)).await
                } else {
                    tokio::time::timeout(REQUEST_TIMEOUT, respond(&ctx, &req, &peer)).await
                };
                Ok::<_, Infallible>(res.unwrap_or_else(|_| {
                    let mut r = Response::new(Full::new(Bytes::new()));
                    *r.status_mut() = StatusCode::REQUEST_TIMEOUT;
                    r
                }))
            }
        });
        let conn = builder.serve_connection(TokioIo::new(stream), service);
        let conn = graceful.watch(conn);
        // The permit is held until the connection ends, which frees its slot.
        tokio::spawn(async move {
            let _ = conn.await;
            drop(permit);
        });
    }
    // Stop accepting new connections; the open ones drain below.
    drop(listener);
    tokio::select! {
        () = graceful.shutdown() => {}
        () = tokio::time::sleep(SHUTDOWN_GRACE) => log("shutdown grace period over; closing open connections"),
    }
}
