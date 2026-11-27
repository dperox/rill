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
    tls: Option<PemPaths>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PemPaths {
    cert: PathBuf,
    key: PathBuf,
}

struct Source {
    path: Option<PathBuf>,
    chunk: usize,
    secure: bool,
}

impl Source {
    fn load(&self) -> io::Result<Generation> {
        let file: FileConfig = match &self.path {
            Some(path) => serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)?,
            None => FileConfig::default(),
        };
        let base = self
            .path
            .as_deref()
            .and_then(|p| p.parent())
            .unwrap_or(std::path::Path::new("."));
        let sites = sites::load(file.sites, base, self.chunk)?;
        let paths = if let Some(paths) = file.tls {
            let directory = self.path.as_ref().unwrap().parent().unwrap();
            Some((directory.join(paths.cert), directory.join(paths.key)))
        } else {
            match (env::var_os("TLS_CERT"), env::var_os("TLS_KEY")) {
                (None, None) => None,
                (Some(cert), Some(key)) => Some((cert.into(), key.into())),
                _ => return Err(io::Error::other("set TLS_CERT and TLS_KEY together")),
            }
        };
        let tls = match (self.secure, paths) {
            (false, None) => None,
            (true, Some((cert, key))) => Some(Arc::new(edge::credentials(&cert, &key)?)),
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
        let routes = routing::load(definitions, &policies)?;
        Ok(Generation {
            sites,
            routes,
            tls,
            policies,
        })
    }
}

pub fn start(chunk: usize) -> io::Result<(Live, Option<String>)> {
    let address = env::var_os("TLS_LISTEN")
        .map(|s| {
            s.into_string()
                .map_err(|_| io::Error::other("invalid TLS_LISTEN"))
        })
        .transpose()?;
    let source = Arc::new(Source {
        path: env::var_os("RILL_CONFIG").map(PathBuf::from),
        chunk,
        secure: address.is_some(),
    });
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
                    eprintln!("Rill configuration reloaded");
                }
                Ok(Err(error)) => eprintln!("Rill reload rejected: {error}"),
                Err(error) => eprintln!("Rill reload rejected: {error}"),
            }
        }
    });
    #[cfg(not(unix))]
    drop(publisher);
    Ok((live, address))
}
