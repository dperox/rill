//! Outbound HTTP transport and optional health probes.
mod fixed;
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

pub enum RequestError {
    Connect,
    Exchange,
}

impl From<hyper_util::client::legacy::Error> for RequestError {
    fn from(value: hyper_util::client::legacy::Error) -> Self {
        if value.is_connect() {
            Self::Connect
        } else {
            Self::Exchange
        }
    }
}

pub enum Transport {
    Plain(Client<HttpConnector, Body>, Option<fixed::Fixed>),
    Secure(Client<HttpsConnector<HttpConnector>, Body>),
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_IDLE_PER_HOST: usize = 64;

impl Transport {
    fn build_tls_config() -> io::Result<rustls::ClientConfig> {
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?;
        let tls = if let Some(ca) = env::var_os("RILL_UPSTREAM_CA") {
            let mut roots = RootCertStore::empty();
            for cert in CertificateDer::pem_file_iter(&ca).map_err(|e| {
                io::Error::other(format!("RILL_UPSTREAM_CA ({}): {e}", ca.to_string_lossy()))
            })? {
                roots
                    .add(cert.map_err(|e| {
                        io::Error::other(format!(
                            "RILL_UPSTREAM_CA ({}): {e}",
                            ca.to_string_lossy()
                        ))
                    })?)
                    .map_err(|e| io::Error::other(format!("RILL_UPSTREAM_CA: {e}")))?;
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
                    CertificateDer::pem_file_iter(&cert)
                        .map_err(|e| {
                            io::Error::other(format!(
                                "RILL_UPSTREAM_CERT ({}): {e}",
                                cert.to_string_lossy()
                            ))
                        })?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| {
                            io::Error::other(format!(
                                "RILL_UPSTREAM_CERT ({}): {e}",
                                cert.to_string_lossy()
                            ))
                        })?,
                    PrivateKeyDer::from_pem_file(&key).map_err(|e| {
                        io::Error::other(format!(
                            "RILL_UPSTREAM_KEY ({}): {e}",
                            key.to_string_lossy()
                        ))
                    })?,
                )
                .map_err(|e| io::Error::other(format!("RILL_UPSTREAM_CERT/KEY: {e}")))?,
            (None, None) => tls.with_no_client_auth(),
            _ => {
                return Err(io::Error::other(
                    "set RILL_UPSTREAM_CERT and RILL_UPSTREAM_KEY together",
                ));
            }
        };
        Ok(tls)
    }

    pub fn validate_env() -> io::Result<()> {
        if env::var_os("RILL_UPSTREAM_CA").is_some()
            || env::var_os("RILL_UPSTREAM_CERT").is_some()
            || env::var_os("RILL_UPSTREAM_KEY").is_some()
        {
            let _ = Self::build_tls_config()?;
        }
        Ok(())
    }

    pub fn new(peers: &[Endpoint]) -> io::Result<Self> {
        let mut builder = Client::builder(TokioExecutor::new());
        let fixed = match peers {
            [peer] if peer.scheme == Scheme::HTTP => {
                peer.authority.as_str().parse().ok().map(fixed::Fixed::new)
            }
            _ => None,
        };
        // Share the existing per-host idle budget with the general HTTP client.
        let reserved = if fixed.is_some() { fixed::CAPACITY } else { 0 };
        builder
            .pool_max_idle_per_host(MAX_IDLE_PER_HOST - reserved)
            .pool_timer(hyper_util::rt::TokioTimer::new());
        let mut http = HttpConnector::new();
        http.set_connect_timeout(Some(CONNECT_TIMEOUT));
        http.set_nodelay(true);
        if peers.iter().all(|p| p.scheme == Scheme::HTTP) {
            return Ok(Self::Plain(builder.build(http), fixed));
        }
        http.enforce_http(false);
        let tls = Self::build_tls_config()?;
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_or_http()
            .enable_http1()
            .wrap_connector(http);
        Ok(Self::Secure(builder.build(connector)))
    }

    pub fn fixed(&self) -> Option<&fixed::Fixed> {
        if let Self::Plain(_, fixed) = self {
            fixed.as_ref()
        } else {
            None
        }
    }

    pub async fn request(
        &self,
        req: Request,
    ) -> Result<hyper::Response<hyper::body::Incoming>, RequestError> {
        match self {
            Self::Plain(client, _) => client.request(req).await.map_err(Into::into),
            Self::Secure(client) => client.request(req).await.map_err(Into::into),
        }
    }
}

pub struct Health {
    interval: Duration,
    path: Option<axum::http::uri::PathAndQuery>,
}

impl Health {
    pub fn from_env() -> io::Result<Option<Self>> {
        let seconds: u32 = match env::var("RILL_HEALTH_INTERVAL_SECS") {
            Ok(value) => value.parse().map_err(|_| {
                io::Error::other("RILL_HEALTH_INTERVAL_SECS must be an unsigned integer")
            })?,
            Err(env::VarError::NotPresent) => 0,
            Err(e) => return Err(io::Error::other(format!("RILL_HEALTH_INTERVAL_SECS: {e}"))),
        };
        let path = match env::var("RILL_HEALTH_PATH") {
            Ok(s) => {
                if !s.starts_with('/') {
                    return Err(io::Error::other("RILL_HEALTH_PATH must start with /"));
                }
                Some(
                    s.parse()
                        .map_err(|e| io::Error::other(format!("RILL_HEALTH_PATH: {e}")))?,
                )
            }
            Err(env::VarError::NotPresent) => None,
            Err(e) => return Err(io::Error::other(format!("RILL_HEALTH_PATH: {e}"))),
        };
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
    pub fn start(&self, pool: &Arc<Pool>) -> Vec<tokio::task::AbortHandle> {
        let mut tasks = Vec::new();
        for i in 0..pool.servers.len() {
            let (pool, interval, path) = (pool.clone(), self.interval, self.path.clone());
            let task = tokio::spawn(async move {
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
            tasks.push(task.abort_handle());
        }
        tasks
    }
}

impl Pool {
    pub fn select(self: &Arc<Self>, exclude: Option<usize>) -> Option<(usize, Option<Lease>)> {
        let len = self.servers.len();
        // A fixed peer needs neither round-robin bookkeeping nor a least-connections lease.
        if len == 1 {
            return (exclude.is_none()
                && self
                    .healthy
                    .first()
                    .is_none_or(|h| h.load(Ordering::Relaxed)))
            .then_some((0, None));
        }
        let start = self.next.fetch_add(1, Ordering::Relaxed) % len;
        if exclude.is_none() && self.healthy.is_empty() && self.active.is_none() {
            return Some((start, None));
        }
        let mut eligible = (0..len).map(|n| (n + start) % len).filter(|&i| {
            Some(i) != exclude
                && self
                    .healthy
                    .get(i)
                    .is_none_or(|v: &AtomicBool| v.load(Ordering::Relaxed))
        });
        if let Some(active) = &self.active {
            let mut counts = active.lock().unwrap_or_else(|e| e.into_inner());
            let i = eligible.min_by_key(|&i| counts[i])?;
            counts[i] += 1;
            Some((i, Some(Lease(self.clone(), i))))
        } else {
            eligible.next().map(|i| (i, None))
        }
    }
}
