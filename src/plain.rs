//! Plaintext HTTP/1 with ordered file regions and prior-knowledge HTTP/2 selection.
use axum::{Router, extract::ConnectInfo};
use http_kit::{Body, BodyFrame, FileRegion, Request, Response};
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
use tower::ServiceExt;
pub use workers::WorkerReactors;

#[derive(Clone, Copy)]
pub struct Transport {
    pub static_only: bool,
    pub regions: bool,
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    app: Router,
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
            .name(format!("rill-h1-{index}"))
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
        let (stream, peer) = listener.accept().await?;
        if next == senders.len() {
            spawn_socket(stream, peer, app.clone(), transport);
            next = 0;
        } else {
            senders[next]
                .send((stream, peer))
                .await
                .map_err(|_| io::Error::other("HTTP/1 worker stopped"))?;
            next += 1;
        }
    }
}

fn spawn_socket(
    stream: http_kit_io::tokio::TcpStream,
    peer: SocketAddr,
    app: Router,
    transport: Transport,
) {
    tokio::task::spawn_local(async move {
        let _ = serve_socket(stream, peer, app, transport).await;
    });
}

async fn serve_socket(
    stream: http_kit_io::tokio::TcpStream,
    peer: SocketAddr,
    app: Router,
    transport: Transport,
) -> io::Result<()> {
    stream.get_ref().set_nodelay(true)?;
    if tokio::time::timeout(Duration::from_secs(30), uses_h2(stream.get_ref())).await?? {
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
            .service(app.into_service::<hyper::body::Incoming>());
        return hyper::server::conn::http2::Builder::new(TokioExecutor::new())
            .serve_connection(
                TokioIo::new(stream.into_inner()),
                TowerToHyperService::new(service),
            )
            .await
            .map_err(io::Error::other);
    }
    let mut config = ServerConfig::default();
    config.read_buffer_size = 1024;
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
    let mut pending = bytes::BytesMut::with_capacity(1024);
    let idle = config.keep_alive_timeout.unwrap();
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

async fn uses_h2(socket: &tokio::net::TcpStream) -> io::Result<bool> {
    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    let mut buf = [0; 24];
    loop {
        let n = socket.peek(&mut buf).await?;
        if n == 0 || buf[..n] != PREFACE[..n] {
            return Ok(false);
        }
        if n == PREFACE.len() {
            return Ok(true);
        }
        // peek leaves a partial prefix readable until the rest arrives.
        tokio::time::sleep(Duration::from_millis(1)).await;
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
    app: Router,
    peer: SocketAddr,
    transport: Transport,
    req: Request,
) -> axum::response::Response {
    let (mut parts, incoming) = req.into_parts();
    parts.extensions.insert(ConnectInfo(peer));
    parts.extensions.insert(transport);
    app.oneshot(axum::extract::Request::from_parts(
        parts,
        axum::body::Body::new(incoming),
    ))
    .await
    .unwrap()
}

async fn respond(app: Router, peer: SocketAddr, transport: Transport, req: Request) -> Response {
    let response = dispatch(app, peer, transport, req).await;
    let (mut parts, body) = response.into_parts();
    let body = match parts.extensions.remove::<FileRegion>() {
        Some(region) => Body::from_file_region(region),
        None if axum::body::HttpBody::is_end_stream(&body) => Body::empty(),
        None => {
            use futures_util::StreamExt;
            let frames = http_body_util::BodyStream::new(body).map(|frame| {
                let frame = frame.map_err(io::Error::other)?;
                Ok::<_, io::Error>(match frame.into_data() {
                    Ok(bytes) => BodyFrame::Data(bytes),
                    Err(frame) => {
                        BodyFrame::Trailers(frame.into_trailers().expect("http-body frame"))
                    }
                })
            });
            Body::from_frames(sync_wrapper::SyncStream::new(frames))
        }
    };
    Response::from_parts(parts, body)
}
