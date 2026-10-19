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

use crate::handler::{ApiContext, ApiRequest, ApiResponse, handle, internal_error};
use crate::log;

pub const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
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
pub fn respond(ctx: &ApiContext, req: &Request<Incoming>, peer: &SocketAddr) -> Response<Full<Bytes>> {
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
    match handle(ctx, &api) {
        Ok(res) => to_hyper(res),
        Err(detail) => {
            log(&format!("request failed: {} {}: {detail}", api.method, api.path));
            to_hyper(internal_error())
        }
    }
}

/// Bind `[::]:port` (dual stack, as Node.js does), or `0.0.0.0:port` where IPv6 is unavailable.
pub async fn bind(port: u16) -> std::io::Result<TcpListener> {
    match TcpListener::bind(("::", port)).await {
        Ok(l) => Ok(l),
        Err(_) => TcpListener::bind(("0.0.0.0", port)).await,
    }
}

/// Serve until `shutdown` resolves, then stop accepting and give in-flight requests the grace period.
pub async fn serve(listener: TcpListener, ctx: Arc<ApiContext>, shutdown: impl Future<Output = ()>) {
    let graceful = GracefulShutdown::new();
    let mut builder = http1::Builder::new();
    builder.timer(TokioTimer::new()).header_read_timeout(HEADER_TIMEOUT).max_buf_size(MAX_BUF);
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    tokio::pin!(shutdown);
    loop {
        let permit = tokio::select! {
            p = slots.clone().acquire_owned() => p.expect("the semaphore is never closed"),
            () = &mut shutdown => break,
        };
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(a) => a,
                Err(e) => {
                    log(&format!("accept failed: {e}"));
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let ctx = ctx.clone();
        let service = service_fn(move |req: Request<Incoming>| {
            let ctx = ctx.clone();
            async move {
                let res = tokio::time::timeout(REQUEST_TIMEOUT, async { respond(&ctx, &req, &peer) }).await;
                Ok::<_, Infallible>(res.unwrap_or_else(|_| {
                    let mut r = Response::new(Full::new(Bytes::new()));
                    *r.status_mut() = StatusCode::REQUEST_TIMEOUT;
                    r
                }))
            }
        });
        let conn = builder.serve_connection(TokioIo::new(stream), service);
        let conn = graceful.watch(conn);
        tokio::spawn(async move {
            let _ = conn.await;
            drop(permit);
        });
    }
    drop(listener);
    tokio::select! {
        () = graceful.shutdown() => {}
        () = tokio::time::sleep(SHUTDOWN_GRACE) => log("shutdown grace period over; closing open connections"),
    }
}
