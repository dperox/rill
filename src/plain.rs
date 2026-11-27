//! Plain TCP keeps one registered socket for Hyper and its ordered HTTP/1 file writer.
use axum::{Router, extract::ConnectInfo, http::Version, serve::Listener};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{
    io::{self, IoSlice},
    net::Shutdown,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{
        TcpListener,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
};

tokio::task_local! {
    static SOCKET: Arc<OwnedWriteHalf>;
}

pub fn socket(version: Version) -> Option<Arc<OwnedWriteHalf>> {
    if matches!(version, Version::HTTP_10 | Version::HTTP_11) {
        SOCKET.try_with(Arc::clone).ok()
    } else {
        None
    }
}

pub async fn serve(mut listener: TcpListener, app: Router, include_peer: bool) -> io::Result<()> {
    loop {
        let (stream, peer) = Listener::accept(&mut listener).await;
        if let Err(error) = stream.set_nodelay(true) {
            eprintln!("could not set TCP_NODELAY on client socket: {error}");
        }
        let (read, write) = stream.into_split();
        let socket = Arc::new(write);
        let io = TokioIo::new(SharedTcp {
            read,
            write: socket.clone(),
        });
        let service = app.clone().into_service::<hyper::body::Incoming>();
        tokio::spawn(SOCKET.scope(socket, async move {
            use tower::ServiceExt;
            let service = service.map_request(move |mut req: hyper::Request<_>| {
                if include_peer {
                    req.extensions_mut().insert(ConnectInfo(peer));
                }
                req
            });
            let builder = Builder::new(TokioExecutor::new());
            let _ = builder
                .serve_connection_with_upgrades(io, TowerToHyperService::new(service))
                .await;
        }));
    }
}

// Tokio's readiness-aware socket operations accept shared references. Only the
// dispatcher writes, including when it polls the file transfer for this socket.
struct SharedTcp {
    read: OwnedReadHalf,
    write: Arc<OwnedWriteHalf>,
}

impl AsyncRead for SharedTcp {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().read).poll_read(cx, buf)
    }
}

impl AsyncWrite for SharedTcp {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.write_ready(cx, || self.write.try_write(buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.write_ready(cx, || self.write.try_write_vectored(bufs))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(socket2::SockRef::from(self.write.as_ref().as_ref()).shutdown(Shutdown::Write))
    }
}

impl SharedTcp {
    fn write_ready(
        &self,
        cx: &mut Context<'_>,
        write: impl Fn() -> io::Result<usize>,
    ) -> Poll<io::Result<usize>> {
        loop {
            ready!(self.write.as_ref().as_ref().poll_write_ready(cx))?;
            match write() {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                result => return Poll::Ready(result),
            }
        }
    }
}
