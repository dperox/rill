//! Plaintext HTTP/1 with ordered file regions and prior-knowledge HTTP/2 selection.
use crate::PlainApp;
use axum::extract::ConnectInfo;
use http_kit::{Body, FileRegion, Request, Response};
use http_kit_h1::{ServerConfig, ServerConnection, ServerOutcome, Upgraded};
use http_kit_io::{BufResult, FromStd, Listener, Read, Split, Write, tokio::Tokio};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    service::TowerToHyperService,
};
use std::{io, net::SocketAddr, time::Duration};
#[cfg(any(windows, target_os = "linux"))]
mod file_writer;
mod prefetched;
mod short;
mod workers;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub use workers::WorkerReactors;

pub(crate) fn is_transient_accept_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    )
}

#[derive(Clone, Copy)]
pub struct Transport {
    pub static_only: bool,
    pub regions: bool,
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    app: PlainApp,
    transport: Transport,
) -> io::Result<()> {
    let listener = Tokio.tcp_listener(listener.into_std()?)?;
    let runtime = tokio::runtime::Handle::current();
    let workers = runtime.metrics().num_workers();
    let mut senders = Vec::with_capacity(workers - 1);
    for index in 1..workers {
        let (sender, mut incoming) =
            tokio::sync::mpsc::channel::<(http_kit_io::tokio::TcpStream, SocketAddr)>(64);
        let runtime = runtime.clone();
        let app = app.clone();
        // http-kit's owned-I/O futures are thread-local. Share Tokio's existing
        // reactor and bounded blocking pool across these connection executors.
        std::thread::Builder::new()
            .name(format!("japka-h1-{index}"))
            .spawn(move || {
                runtime.block_on(tokio::task::LocalSet::new().run_until(async move {
                    while let Some((stream, peer)) = incoming.recv().await {
                        spawn_socket(stream, peer, app.clone(), transport);
                    }
                }));
            })?;
        senders.push(sender);
    }
    let mut next = 0;
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) if is_transient_accept_error(&error) => continue,
            Err(error) => {
                eprintln!("Plaintext accept error: {error}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        if senders.is_empty() || next == senders.len() {
            spawn_socket(stream, peer, app.clone(), transport);
            next = 0;
        } else {
            let mut pair = Some((stream, peer));
            for offset in 0..senders.len() {
                let target = (next + offset) % senders.len();
                let item = pair.take().unwrap();
                match senders[target].try_send(item) {
                    Ok(()) => {
                        next = target + 1;
                        break;
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(returned)) => {
                        pair = Some(returned);
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        return Err(io::Error::other("HTTP/1 worker stopped"));
                    }
                }
            }
            if let Some((stream, peer)) = pair {
                spawn_socket(stream, peer, app.clone(), transport);
                next = 0;
            }
        }
    }
}

fn spawn_socket(
    stream: http_kit_io::tokio::TcpStream,
    peer: SocketAddr,
    app: PlainApp,
    transport: Transport,
) {
    tokio::task::spawn_local(async move {
        let _ = serve_socket(stream, peer, app, transport).await;
    });
}

async fn serve_socket(
    stream: http_kit_io::tokio::TcpStream,
    peer: SocketAddr,
    app: PlainApp,
    transport: Transport,
) -> io::Result<()> {
    stream.get_ref().set_nodelay(true)?;
    let mut config = ServerConfig::default();
    config.read_buffer_size = 1024;
    let idle = config.keep_alive_timeout.unwrap();
    let mut pending = bytes::BytesMut::with_capacity(1024);
    let preface_timeout = idle.min(Duration::from_secs(30));
    match tokio::time::timeout(preface_timeout, sniff_h2(stream.get_ref(), &mut pending)).await {
        Ok(Ok(Sniff::Http2)) => {
            let service = tower::ServiceBuilder::new()
                .map_request(move |mut req: hyper::Request<_>| {
                    req.extensions_mut().insert(ConnectInfo(peer));
                    if transport.static_only {
                        req.extensions_mut().insert(Transport {
                            regions: false,
                            static_only: true,
                        });
                    }
                    req
                })
                .service(app.router.into_service::<hyper::body::Incoming>());
            // Replay the bytes consumed during protocol detection before reading the socket.
            let mut socket = stream.into_inner();
            let (read, write) = socket.split();
            let io = tokio::io::join(pending.as_ref().chain(read), write);
            let mut h2 = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
            h2.max_concurrent_streams(256);
            return h2
                .serve_connection(TokioIo::new(io), TowerToHyperService::new(service))
                .await
                .map_err(io::Error::other);
        }
        Ok(Ok(Sniff::Closed)) => return Ok(()),
        Ok(Ok(Sniff::Http1)) => {}
        Ok(Err(error)) => return Err(error),
        Err(_) => return Ok(()),
    }
    #[cfg(windows)]
    let socket = crate::transmitfile::socket(stream.get_ref())?;
    let (read, write) = stream.split();
    #[cfg(any(windows, target_os = "linux"))]
    let write = file_writer::FileWriter {
        inner: write,
        #[cfg(windows)]
        socket,
    };
    let mut write = write;
    let tcp = read.get_ref().get_ref();
    loop {
        if pending.is_empty() {
            pending.reserve(1024);
            let read = async {
                loop {
                    tcp.readable().await?;
                    match tcp.try_read_buf(&mut pending) {
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                        result => return result,
                    }
                }
            };
            if tokio::time::timeout(idle, read).await?? == 0 {
                return Ok(());
            }
        }
        // Keep the transient exchange state out of each parked task.
        match Box::pin(short::short_request(
            &mut pending,
            &mut write,
            app.clone(),
            peer,
            transport,
        ))
        .await?
        {
            Some(true) => continue,
            Some(false) => return Ok(()),
            None => break,
        }
    }
    Box::pin(async move {
        let read = prefetched::Reader::new(pending.freeze(), read);
        let connection = ServerConnection::new(read, write, Tokio, config);
        if let ServerOutcome::Upgraded {
            upgraded,
            mut response,
        } = connection
            .serve(&|req| Box::pin(respond(app.clone(), peer, transport, req)))
            .await
            .map_err(io::Error::other)?
            && let Some(tunnel) = response
                .extensions
                .remove::<crate::websocket::Tunnel>()
                .and_then(|t| t.take())
        {
            let (upstream, _lease) = tunnel;
            relay(
                upgraded,
                TokioIo::new(upstream.await.map_err(io::Error::other)?),
            )
            .await?;
        }
        Ok(())
    })
    .await
}

enum Sniff {
    Http1,
    Http2,
    Closed,
}

async fn sniff_h2(
    socket: &tokio::net::TcpStream,
    pending: &mut bytes::BytesMut,
) -> io::Result<Sniff> {
    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    loop {
        socket.readable().await?;
        let n = match socket.try_read_buf(pending) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(e),
        };
        if n == 0 {
            return Ok(if pending.is_empty() {
                Sniff::Closed
            } else {
                Sniff::Http1
            });
        }
        let check_len = pending.len().min(PREFACE.len());
        if pending[..check_len] != PREFACE[..check_len] {
            return Ok(Sniff::Http1);
        }
        if pending.len() >= PREFACE.len() {
            return Ok(Sniff::Http2);
        }
    }
}

async fn relay<R: Read, W: Write>(
    mut client: Upgraded<R, W>,
    origin: TokioIo<hyper::upgrade::Upgraded>,
) -> io::Result<()> {
    let (mut read, mut write) = tokio::io::split(origin);
    let uplink = async {
        write.write_all(&client.read_buf).await?;
        let mut buf = Vec::with_capacity(16384);
        loop {
            let BufResult(n, returned) = client.read.read(buf).await;
            buf = returned;
            let n = n?;
            if n == 0 {
                break;
            }
            write.write_all(&buf[..n]).await?;
        }
        write.shutdown().await
    };
    let downlink = async {
        let mut buf = vec![0u8; 16384];
        loop {
            buf.resize(16384, 0);
            let n = read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            buf.truncate(n);
            let BufResult(result, returned) = client.write.write_all(buf).await;
            result?;
            buf = returned;
        }
        client.write.shutdown().await
    };
    tokio::try_join!(uplink, downlink)?;
    Ok(())
}

async fn dispatch(
    app: PlainApp,
    peer: SocketAddr,
    transport: Transport,
    req: Request,
) -> axum::response::Response {
    let (mut parts, incoming) = req.into_parts();
    parts.extensions.insert(ConnectInfo(peer));
    parts.extensions.insert(transport);
    app.respond(axum::extract::Request::from_parts(
        parts,
        axum::body::Body::new(incoming),
    ))
    .await
}

async fn respond(app: PlainApp, peer: SocketAddr, transport: Transport, req: Request) -> Response {
    let response = dispatch(app, peer, transport, req).await;
    let (mut parts, body) = response.into_parts();
    let body = match parts.extensions.remove::<FileRegion>() {
        Some(region) => Body::from_file_region(region),
        None if axum::body::HttpBody::is_end_stream(&body) => Body::empty(),
        None => {
            use http_body_util::{BodyExt, BodyStream, StreamBody};
            // Axum bodies are Send. Wrap their frame stream for http-kit's Sync body.
            let frames = sync_wrapper::SyncStream::new(BodyStream::new(body));
            Body::new(StreamBody::new(frames).map_err(io::Error::other))
        }
    };
    Response::from_parts(parts, body)
}
