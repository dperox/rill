use axum::{
    Router,
    body::{Body, Bytes, HttpBody},
    extract::ConnectInfo,
    http::{
        Method, Request, Response, StatusCode,
        header::{ALT_SVC, CONTENT_LENGTH},
    },
};
use bytes::Buf;
use http_body_util::{BodyExt, StreamBody};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{io, path::Path, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use tokio_rustls::{TlsAcceptor, rustls};
use tower::ServiceExt;

pub struct Credentials {
    tcp: TlsAcceptor,
    quic: Arc<quinn::ServerConfig>,
}

use rustls::{
    server::{ClientHello, ResolvesServerCert, ResolvesServerCertUsingSni},
    sign::CertifiedKey,
};

#[derive(Debug)]
struct Certificates {
    default: Arc<CertifiedKey>,
    named: ResolvesServerCertUsingSni,
}

impl ResolvesServerCert for Certificates {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.named
            .resolve(hello)
            .or_else(|| Some(self.default.clone()))
    }
}

fn load_key(certificate: &Path, key: &Path) -> io::Result<CertifiedKey> {
    let certs = CertificateDer::pem_file_iter(certificate)
        .map_err(io::Error::other)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io::Error::other)?;
    let key = PrivateKeyDer::from_pem_file(key).map_err(io::Error::other)?;
    CertifiedKey::from_der(certs, key, &rustls::crypto::ring::default_provider())
        .map_err(io::Error::other)
}

pub fn credentials(
    cert: &Path,
    key: &Path,
    base: &Path,
    hosts: Vec<crate::reload::HostCertificate>,
) -> io::Result<Credentials> {
    let default = Arc::new(load_key(cert, key)?);
    let mut named = ResolvesServerCertUsingSni::new();
    let mut names = std::collections::HashSet::new();
    for host in hosts {
        let name = host.host.to_ascii_lowercase();
        if !names.insert(name.clone()) {
            return Err(io::Error::other("duplicate TLS hostname"));
        }
        named
            .add(
                &name,
                load_key(&base.join(host.cert), &base.join(host.key))?,
            )
            .map_err(io::Error::other)?;
    }
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(io::Error::other)?
    .with_no_client_auth()
    .with_cert_resolver(Arc::new(Certificates { default, named }));
    config.alpn_protocols = vec![b"h3".to_vec()];
    let quic = quinn::crypto::rustls::QuicServerConfig::try_from(config.clone())
        .map_err(io::Error::other)?;
    let quic = Arc::new(quinn::ServerConfig::with_crypto(Arc::new(quic)));
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Credentials {
        tcp: TlsAcceptor::from(Arc::new(config)),
        quic,
    })
}

pub fn redirect_from_env(app: Router) -> io::Result<Router> {
    use axum::http::uri::Authority;
    use axum::{
        extract::State,
        middleware::{self, Next},
        response::{IntoResponse, Redirect},
    };
    let target = match std::env::var("RILL_HTTPS_REDIRECT") {
        Err(std::env::VarError::NotPresent) => return Ok(app),
        value => value.map_err(io::Error::other)?,
    };
    let authority: Authority = target.parse().map_err(io::Error::other)?;
    if authority.host().is_empty()
        || target.contains(['@', '%', '\\'])
        || (target.len() != authority.host().len() && authority.port_u16().is_none())
    {
        return Err(io::Error::other(
            "RILL_HTTPS_REDIRECT needs a public host and optional port",
        ));
    }
    async fn redirect(
        State(target): State<Authority>,
        req: axum::extract::Request,
        next: Next,
    ) -> axum::response::Response {
        if req
            .extensions()
            .get::<crate::policy::SecureTransport>()
            .is_some()
        {
            return next.run(req).await;
        }
        let path = req.uri().path_and_query().map_or("/", |v| v.as_str());
        if !path.starts_with('/') {
            return crate::error_page(StatusCode::BAD_REQUEST);
        }
        Redirect::permanent(&format!("https://{target}{path}")).into_response()
    }
    Ok(app.layer(middleware::from_fn_with_state(authority, redirect)))
}

pub async fn serve(address: &str, app: Router, live: crate::reload::Live) -> io::Result<()> {
    let tcp = TcpListener::bind(address).await?;
    let local = tcp.local_addr()?;
    let tls = || {
        live.borrow()
            .tls
            .clone()
            .expect("TLS listener always has credentials")
    };
    let mut updates = live.clone();
    let endpoint = quinn::Endpoint::server((*tls().quic).clone(), local)?;
    let alt = format!("h3=\":{}\"", local.port())
        .parse::<axum::http::HeaderValue>()
        .map_err(io::Error::other)?;
    let app = app.layer(axum::Extension(crate::policy::SecureTransport));
    let app = app.layer(axum::middleware::map_response(
        move |mut res: axum::response::Response| {
            res.headers_mut().insert(ALT_SVC, alt.clone());
            async move { res }
        },
    ));
    eprintln!("Rill listening on https://{local} (HTTP/1.1, HTTP/2 and HTTP/3)");
    loop {
        tokio::select! {
            // Quinn consults the endpoint configuration for initial packet handling.
            // Keep it current and release its reference to the previous private key.
            Ok(()) = updates.changed(), if cfg!(unix) => {
                endpoint.set_server_config(Some((*tls().quic).clone()));
            }
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { return Err(io::Error::other("QUIC endpoint closed")); };
                let app = app.clone();
                let Ok(connecting) = incoming.accept_with(tls().quic.clone()) else { continue };
                tokio::spawn(async move {
                    let Ok(conn) = connecting.await else {return};
                    let app = app.layer(axum::Extension(ConnectInfo(conn.remote_address())));
                    if let Err(error) = serve_h3(conn, app).await { eprintln!("HTTP/3 connection: {error}"); }
                });
            }
            incoming = tcp.accept() => {
                let (stream, peer) = incoming?;
                tokio::spawn(serve_tcp(stream, peer, tls().tcp.clone(), app.clone()));
            }
        }
    }
}

async fn serve_tcp(
    stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    tls: TlsAcceptor,
    app: Router,
) {
    if let Err(error) = stream.set_nodelay(true) {
        eprintln!("TCP_NODELAY for {peer}: {error}");
    }
    let Ok(Ok(stream)) = tokio::time::timeout(Duration::from_secs(10), tls.accept(stream)).await
    else {
        return;
    };
    let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2");
    let app = app
        .layer(axum::Extension(ConnectInfo(peer)))
        .into_service::<hyper::body::Incoming>()
        .map_response(normalize_headers);
    let io = TokioIo::new(stream);
    let service = TowerToHyperService::new(app);
    let result = if h2 {
        Builder::new(TokioExecutor::new())
            .http2_only()
            .serve_connection(io, service)
            .await
    } else {
        hyper::server::conn::http1::Builder::new()
            .serve_connection(io, service)
            .with_upgrades()
            .await
            .map_err(Into::into)
    };
    if let Err(error) = result {
        eprintln!("HTTPS connection from {peer}: {error}");
    }
}

async fn serve_h3(conn: quinn::Connection, app: Router) -> Result<(), h3::error::ConnectionError> {
    let mut server = h3::server::Connection::new(h3_quinn::Connection::new(conn)).await?;
    while let Some(resolver) = server.accept().await? {
        let app = app.clone();
        tokio::spawn(async move {
            let Ok((request, stream)) = resolver.resolve_request().await else {
                return;
            };
            if let Err(error) = h3_request(request, stream, app).await {
                eprintln!("HTTP/3 stream: {error}");
            }
        });
    }
    Ok(())
}

// Axum infers a zero content length from empty bodies. On a 304 that
// field would describe the selected representation, and on a 204 it is forbidden.
fn normalize_headers(mut res: axum::response::Response) -> axum::response::Response {
    use axum::http::{StatusCode, header::CONTENT_LENGTH};
    if matches!(
        res.status(),
        StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED
    ) {
        res.headers_mut().remove(CONTENT_LENGTH);
    }
    res
}

struct RequestBody(h3::server::RequestStream<h3_quinn::RecvStream, Bytes>);

impl Drop for RequestBody {
    fn drop(&mut self) {
        // A handler can answer without polling its request body. Tell the
        // client the upload is no longer needed using HTTP/3's success code.
        self.0.stop_sending(h3::error::Code::H3_NO_ERROR);
    }
}

async fn h3_request(
    request: Request<()>,
    stream: h3::server::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
    app: Router,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut send, recv) = stream.split();
    let recv = RequestBody(recv);
    let frames = async_stream::try_stream! {
        let mut recv = recv;
        while let Some(mut data) = recv.0.recv_data().await? {
            yield hyper::body::Frame::data(data.copy_to_bytes(data.remaining()));
        }
        if let Some(trailers) = recv.0.recv_trailers().await? {
            yield hyper::body::Frame::trailers(trailers);
        }
    };
    fn body<S>(frames: S) -> Body
    where
        S: futures_core::Stream<Item = Result<hyper::body::Frame<Bytes>, h3::error::StreamError>>
            + Send
            + 'static,
    {
        Body::new(StreamBody::new(frames))
    }
    let is_head = request.method() == Method::HEAD;
    let response = app.oneshot(request.map(|_| body(frames))).await?;
    let mut response = normalize_headers(response);
    if !response.headers().contains_key(axum::http::header::DATE) {
        let date = httpdate::fmt_http_date(std::time::SystemTime::now());
        response
            .headers_mut()
            .insert(axum::http::header::DATE, date.parse()?);
    }
    let status = response.status();
    let bodyless_status = status.is_informational()
        || matches!(status, StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED);
    if is_head
        && !bodyless_status
        && !response.headers().contains_key(CONTENT_LENGTH)
        && let Some(length) = response.body().size_hint().exact()
    {
        response.headers_mut().insert(CONTENT_LENGTH, length.into());
    }
    let (parts, mut body) = response.into_parts();
    send.send_response(Response::from_parts(parts, ())).await?;
    if is_head || bodyless_status {
        send.finish().await?;
        return Ok(());
    }
    while let Some(frame) = body.frame().await {
        let frame = frame.inspect_err(|_| {
            send.stop_stream(h3::error::Code::H3_INTERNAL_ERROR);
        })?;
        match frame.into_data() {
            Ok(data) => send.send_data(data).await?,
            Err(frame) => {
                if let Ok(trailers) = frame.into_trailers() {
                    send.send_trailers(trailers).await?;
                    break;
                }
            }
        }
    }
    send.finish().await?;
    Ok(())
}
