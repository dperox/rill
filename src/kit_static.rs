//! Dedicated HTTP/1 listener for bodyless static requests.
use axum::{
    Router,
    extract::ConnectInfo,
    http::{Method, StatusCode, header},
};
use futures_util::TryStreamExt;
use http_kit::{Body, FileRegion, Request, Response};
use http_kit_h1::{ServerConfig, ServerConnection};
use http_kit_io::{FromStd, Listener, tokio::Tokio};
use std::{io, net::SocketAddr};
use tower::ServiceExt;

#[derive(Clone, Copy)]
pub struct StaticRequest {
    pub regions: bool,
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    app: Router,
    regions: bool,
) -> io::Result<()> {
    let listener = Tokio.tcp_listener(listener.into_std()?)?;
    loop {
        let (stream, peer) = listener.accept().await?;
        let app = app.clone();
        tokio::task::spawn_local(async move {
            let connection = ServerConnection::from_stream(stream, Tokio, ServerConfig::default());
            let _ = connection
                .serve(&|req| respond(app.clone(), peer, regions, req))
                .await;
        });
    }
}

async fn respond(app: Router, peer: SocketAddr, regions: bool, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    let status = if parts.method != Method::GET && parts.method != Method::HEAD {
        Some(StatusCode::METHOD_NOT_ALLOWED)
    } else if parts.headers.contains_key(header::TRANSFER_ENCODING)
        || parts
            .headers
            .get(header::CONTENT_LENGTH)
            .is_some_and(|v| v != "0")
    {
        Some(StatusCode::BAD_REQUEST)
    } else {
        None
    };
    if let Some(status) = status {
        return axum::http::Response::builder()
            .status(status)
            .header(header::CONNECTION, "close")
            .body(Body::empty())
            .unwrap();
    }
    parts.extensions.insert(ConnectInfo(peer));
    parts.extensions.insert(StaticRequest { regions });
    let response = app
        .oneshot(axum::extract::Request::from_parts(
            parts,
            axum::body::Body::empty(),
        ))
        .await
        .unwrap();
    let (mut parts, body) = response.into_parts();
    let body = match parts.extensions.remove::<FileRegion>() {
        Some(region) => Body::from_file_region(region),
        None => Body::from_stream(sync_wrapper::SyncStream::new(
            body.into_data_stream().map_err(io::Error::other),
        )),
    };
    Response::from_parts(parts, body)
}
