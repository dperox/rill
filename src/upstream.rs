//! Outbound HTTP transport and optional health probes.
use super::*;
use axum::http::uri::Scheme;
use hyper_rustls::{ConfigBuilderExt, HttpsConnector, HttpsConnectorBuilder};
use std::sync::atomic::AtomicBool;
use tokio_rustls::rustls::{
    self, RootCertStore,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};

pub struct Endpoint {
    pub scheme: Scheme,
    pub authority: Authority,
}

impl Endpoint {
    pub fn parse(value: &str) -> io::Result<Self> {
        let (scheme, authority) = match value.split_once("://") {
            Some(("https", rest)) => (Scheme::HTTPS, rest.strip_suffix('/').unwrap_or(rest)),
            Some(("http", rest)) => (Scheme::HTTP, rest.strip_suffix('/').unwrap_or(rest)),
            Some(_) => return Err(io::Error::other("unsupported upstream scheme")),
            None => (Scheme::HTTP, value),
        };
        let authority: Authority = authority.parse().map_err(io::Error::other)?;
        if authority.as_str().contains('@')
            || authority.host().is_empty()
            || (authority.as_str().len() > authority.host().len() && authority.port_u16().is_none())
        {
            return Err(io::Error::other("invalid upstream authority"));
        }
        Ok(Self { scheme, authority })
    }

    pub fn host_header(&self) -> header::HeaderValue {
        if self.scheme == Scheme::HTTPS {
            self.authority
                .as_str()
                .parse()
                .expect("validated authority")
        } else {
            header::HeaderValue::from_static("backend")
        }
    }
}

pub enum Transport {
    Plain(Client<HttpConnector, Body>),
    Secure(Client<HttpsConnector<HttpConnector>, Body>),
}

impl Transport {
    pub fn new(https: bool) -> io::Result<Self> {
        let builder = Client::builder(TokioExecutor::new());
        if !https {
            return Ok(Self::Plain(builder.build_http()));
        }
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?;
        let tls = if let Some(ca) = env::var_os("RILL_UPSTREAM_CA") {
            let mut roots = RootCertStore::empty();
            for cert in CertificateDer::pem_file_iter(ca).map_err(io::Error::other)? {
                roots
                    .add(cert.map_err(io::Error::other)?)
                    .map_err(io::Error::other)?;
            }
            if roots.is_empty() {
                return Err(io::Error::other(
                    "RILL_UPSTREAM_CA contains no certificates",
                ));
            }
            tls.with_root_certificates(roots)
        } else {
            tls.with_native_roots()?
        };
        let tls = match (
            env::var_os("RILL_UPSTREAM_CERT"),
            env::var_os("RILL_UPSTREAM_KEY"),
        ) {
            (Some(cert), Some(key)) => tls
                .with_client_auth_cert(
                    CertificateDer::pem_file_iter(cert)
                        .map_err(io::Error::other)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(io::Error::other)?,
                    PrivateKeyDer::from_pem_file(key).map_err(io::Error::other)?,
                )
                .map_err(io::Error::other)?,
            (None, None) => tls.with_no_client_auth(),
            _ => {
                return Err(io::Error::other(
                    "set RILL_UPSTREAM_CERT and RILL_UPSTREAM_KEY together",
                ));
            }
        };
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self::Secure(builder.build(connector)))
    }

    pub async fn request(
        &self,
        req: Request,
    ) -> Result<hyper::Response<hyper::body::Incoming>, hyper_util::client::legacy::Error> {
        match self {
            Self::Plain(client) => client.request(req).await,
            Self::Secure(client) => client.request(req).await,
        }
    }
}

pub struct Health {
    interval: Duration,
    path: Option<axum::http::uri::PathAndQuery>,
}

impl Health {
    pub fn from_env() -> io::Result<Option<Self>> {
        let seconds: u32 = env::var("RILL_HEALTH_INTERVAL_SECS")
            .unwrap_or_else(|_| "0".into())
            .parse()
            .map_err(io::Error::other)?;
        let path = env::var("RILL_HEALTH_PATH")
            .ok()
            .map(|s| {
                if !s.starts_with('/') {
                    return Err(io::Error::other("RILL_HEALTH_PATH must start with /"));
                }
                s.parse().map_err(io::Error::other)
            })
            .transpose()?;
        if path.is_some() && seconds == 0 {
            return Err(io::Error::other(
                "RILL_HEALTH_PATH requires a positive probe interval",
            ));
        }
        Ok((seconds != 0).then_some(Self {
            interval: Duration::from_secs(seconds.into()),
            path,
        }))
    }
    pub fn start(&self, pool: &Arc<Pool>) {
        for i in 0..pool.servers.len() {
            let (pool, interval, path) = (pool.clone(), self.interval, self.path.clone());
            tokio::spawn(async move {
                let peer = &pool.servers[i];
                loop {
                    let probe = async {
                        if let Some(path) = &path {
                            let uri = Uri::builder()
                                .scheme(peer.scheme.clone())
                                .authority(peer.authority.clone())
                                .path_and_query(path.clone())
                                .build()
                                .expect("validated health URI");
                            let req = Request::builder()
                                .uri(uri)
                                .header(header::HOST, peer.host_header())
                                .body(Body::empty())
                                .unwrap();
                            pool.client
                                .request(req)
                                .await
                                .is_ok_and(|r| r.status().is_success())
                        } else {
                            let port = peer.authority.port_u16().unwrap_or(
                                if peer.scheme == Scheme::HTTPS {
                                    443
                                } else {
                                    80
                                },
                            );
                            let host = peer
                                .authority
                                .host()
                                .trim_start_matches('[')
                                .trim_end_matches(']');
                            tokio::net::TcpStream::connect((host, port)).await.is_ok()
                        }
                    };
                    let healthy = tokio::time::timeout(Duration::from_secs(2), probe)
                        .await
                        .unwrap_or(false);
                    pool.healthy[i].store(healthy, Ordering::Relaxed);
                    tokio::time::sleep(interval).await;
                }
            });
        }
    }
}

impl Pool {
    pub fn select(self: &Arc<Self>) -> Option<(usize, Option<Lease>)> {
        let len = self.servers.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed) % len;
        if self.healthy.is_empty() && self.active.is_none() {
            return Some((start, None));
        }
        let mut eligible = (0..len).map(|n| (n + start) % len).filter(|&i| {
            self.healthy
                .get(i)
                .is_none_or(|v: &AtomicBool| v.load(Ordering::Relaxed))
        });
        if let Some(active) = &self.active {
            let mut counts = active.lock().unwrap();
            let i = eligible.min_by_key(|&i| counts[i])?;
            counts[i] += 1;
            Some((i, Some(Lease(self.clone(), i))))
        } else {
            eligible.next().map(|i| (i, None))
        }
    }
}
