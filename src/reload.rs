//! Publish routes and both TLS transports as one validated configuration.
use crate::{edge, policy, routing, sites};
use serde::Deserialize;
use std::{env, fs, io, path::PathBuf, sync::Arc};
use tokio::sync::watch;

pub struct Generation {
    pub sites: Vec<(String, sites::Site)>,
    pub policies: policy::Policies,
    pub routes: Vec<routing::Route>,
    pub tls: Option<Arc<edge::Credentials>>,
}

pub type Live = watch::Receiver<Generation>;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    sites: Vec<sites::Definition>,
    routes: Option<Vec<routing::Definition>>,
    tls: Option<TlsConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TlsConfig {
    cert: PathBuf,
    key: PathBuf,
    #[serde(default)]
    hosts: Vec<HostCertificate>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostCertificate {
    pub host: String,
    pub cert: PathBuf,
    pub key: PathBuf,
}

struct Source {
    path: Option<PathBuf>,
    chunk: usize,
    secure: bool,
    check: bool,
    fixed_upstream: bool,
}

impl Source {
    fn load(&self) -> io::Result<Generation> {
        let file: FileConfig = match &self.path {
            Some(path) => {
                let bytes = fs::read(path).map_err(|e| {
                    io::Error::new(e.kind(), format!("JAPKA_CONFIG ({}): {e}", path.display()))
                })?;
                serde_json::from_slice(&bytes).map_err(|e| {
                    io::Error::other(format!("JAPKA_CONFIG ({}): {e}", path.display()))
                })?
            }
            None => FileConfig::default(),
        };
        crate::upstream::Transport::validate_env()?;
        let base = self
            .path
            .as_deref()
            .and_then(|p| p.parent())
            .unwrap_or(std::path::Path::new("."));
        let sites = sites::load(file.sites, base, self.chunk)?;
        let paths = if let Some(paths) = file.tls {
            Some((base.join(paths.cert), base.join(paths.key), paths.hosts))
        } else {
            match (env::var_os("TLS_CERT"), env::var_os("TLS_KEY")) {
                (None, None) => None,
                (Some(cert), Some(key)) => Some((cert.into(), key.into(), Vec::new())),
                _ => return Err(io::Error::other("set TLS_CERT and TLS_KEY together")),
            }
        };
        let tls = match (self.secure, paths) {
            (false, None) => None,
            (true, Some((cert, key, hosts))) => {
                Some(Arc::new(edge::credentials(&cert, &key, base, hosts)?))
            }
            _ => {
                return Err(io::Error::other(
                    "set TLS_LISTEN and TLS credentials together",
                ));
            }
        };
        let definitions = match file.routes {
            Some(routes) => routes,
            None => routing::from_env()?,
        };
        // Each generation owns its cache, including fills still running on retired routes.
        let policies = policy::Policies::from_env()?;
        let routes = routing::load(definitions, &policies, !self.check, self.fixed_upstream)?;
        Ok(Generation {
            sites,
            routes,
            tls,
            policies,
        })
    }
}

pub fn start(
    chunk: usize,
    check: bool,
    fixed_upstream: bool,
) -> io::Result<(Live, Option<String>)> {
    let address = env::var_os("TLS_LISTEN")
        .map(|s| {
            s.into_string()
                .map_err(|_| io::Error::other("invalid TLS_LISTEN"))
        })
        .transpose()?;
    let source = Arc::new(Source {
        path: crate::settings::var_os("JAPKA_CONFIG").map(PathBuf::from),
        chunk,
        check,
        fixed_upstream,
        secure: address.is_some(),
    });
    if check {
        return Ok((watch::channel(source.load()?).1, address));
    }
    #[cfg(unix)]
    let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let (publisher, live) = watch::channel(source.load()?);
    #[cfg(unix)]
    tokio::spawn(async move {
        while hup.recv().await.is_some() {
            let source = source.clone();
            match tokio::task::spawn_blocking(move || source.load()).await {
                Ok(Ok(candidate)) => {
                    publisher.send_replace(candidate);
                    eprintln!("Japka configuration reloaded");
                }
                Ok(Err(error)) => eprintln!("Japka reload rejected: {error}"),
                Err(error) => eprintln!("Japka reload rejected: {error}"),
            }
        }
    });
    #[cfg(not(unix))]
    drop(publisher);
    Ok((live, address))
}
