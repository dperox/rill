//! JSON access records emitted when the application produces response headers.
use axum::http::uri::Authority;
use axum::{
    Router,
    extract::{ConnectInfo, Request},
    middleware::{self, Next},
    response::Response,
};
use std::{
    env,
    io::{self, Write},
    net::SocketAddr,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Enabled;

pub fn configure(app: Router) -> io::Result<(Router, bool)> {
    match env::var("RILL_ACCESS_LOG").as_deref() {
        Err(env::VarError::NotPresent) | Ok("0") => Ok((app, false)),
        Ok("1") => Ok((app.layer(middleware::from_fn(record)), true)),
        _ => Err(io::Error::other("RILL_ACCESS_LOG must be 0 or 1")),
    }
}

async fn record(mut req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    let uri = req.uri().clone();
    let host = crate::routing::authority(&req);
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|v| v.0.ip());
    req.extensions_mut().insert(Enabled);
    let response = next.run(req).await;
    let record = serde_json::json!({
        "event": "access", "unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
        "method": method.as_str(), "path": uri.path(), "host": host.as_ref().map(Authority::host),
        "client_ip": peer, "status": response.status().as_u16(), "header_latency_us": start.elapsed().as_micros() as u64,
        "upstream": response.extensions().get::<Authority>().map(Authority::as_str),
    });
    let _ = writeln!(io::stderr().lock(), "{record}");
    response
}
