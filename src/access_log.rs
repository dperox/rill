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
    sync::{
        OnceLock,
        mpsc::{self, SyncSender},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Enabled;

const LOG_QUEUE_CAPACITY: usize = 4096;

enum LogMessage {
    Line(String),
    Flush(mpsc::SyncSender<()>),
}

struct LogWriter {
    sender: SyncSender<LogMessage>,
}

static WRITER: OnceLock<LogWriter> = OnceLock::new();

fn writer() -> &'static SyncSender<LogMessage> {
    &WRITER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<LogMessage>(LOG_QUEUE_CAPACITY);
            let _ = std::thread::Builder::new()
                .name("rill-access-log".into())
                .spawn(move || {
                    let stderr = io::stderr();
                    while let Ok(msg) = receiver.recv() {
                        match msg {
                            LogMessage::Line(line) => {
                                let mut lock = stderr.lock();
                                let _ = writeln!(lock, "{line}");
                            }
                            LogMessage::Flush(done) => {
                                let mut lock = stderr.lock();
                                let _ = lock.flush();
                                let _ = done.send(());
                            }
                        }
                    }
                });
            LogWriter { sender }
        })
        .sender
}

pub fn flush() {
    if let Some(log) = WRITER.get() {
        let (tx, rx) = mpsc::sync_channel(1);
        if log.sender.send(LogMessage::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(std::time::Duration::from_millis(500));
        }
    }
}

pub fn configure(app: Router) -> io::Result<(Router, bool)> {
    match env::var("RILL_ACCESS_LOG").as_deref() {
        Err(env::VarError::NotPresent) | Ok("0") => Ok((app, false)),
        Ok("1") => {
            let _ = writer();
            Ok((app.layer(middleware::from_fn(record)), true))
        }
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
        .map(|v| v.0.ip().to_canonical());
    req.extensions_mut().insert(Enabled);
    let response = next.run(req).await;
    let record = serde_json::json!({
        "event": "access", "unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
        "method": method.as_str(), "path": uri.path(), "host": host.as_ref().map(Authority::host),
        "client_ip": peer, "status": response.status().as_u16(), "header_latency_us": start.elapsed().as_micros() as u64,
        "upstream": response.extensions().get::<Authority>().map(Authority::as_str),
    });
    let _ = writer().try_send(LogMessage::Line(record.to_string()));
    response
}
