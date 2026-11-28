//! Publish routes and both TLS transports as one validated configuration.
use crate::{config_review::Snapshot, edge, policy, routing, sites};
use serde::Deserialize;
use std::{env, fs, io, path::PathBuf, sync::Arc};
use tokio::sync::watch;

pub struct Generation {
    file_sha256: Option<String>,
    pub sites: Vec<(String, sites::Site)>,
    pub policies: policy::Policies,
    pub routes: Vec<routing::Route>,
    pub tls: Option<Arc<edge::Credentials>>,
}

pub type Live = watch::Receiver<Generation>;

#[derive(Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileConfig {
    /// Host-specific static roots. Omission clears all named sites.
    #[serde(default)]
    sites: Vec<sites::Definition>,
    /// Omitted or null inherits JAPKA_ROUTES and UPSTREAMS. An empty array clears all routes.
    routes: Option<Vec<routing::Definition>>,
    /// HTTP pool balancing and backend probes. Omitted fields inherit their startup settings.
    #[serde(default)]
    proxy: routing::config::ProxyConfig,
    /// Omitted or null inherits the startup TLS_CERT and TLS_KEY pair.
    tls: Option<TlsConfig>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct TlsConfig {
    /// PEM certificate chain path, relative to this configuration file or absolute.
    cert: PathBuf,
    /// Matching PEM private key path.
    key: PathBuf,
    /// Additional certificate pairs selected by SNI hostname.
    #[serde(default)]
    hosts: Vec<HostCertificate>,
}

#[derive(Deserialize, schemars::JsonSchema)]
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
    fn log_active(&self, generation: &Generation) {
        if let (Some(path), Some(sha256)) = (&self.path, &generation.file_sha256) {
            eprintln!(
                "Japka configuration active {}",
                serde_json::json!({
                    "pid": std::process::id(), "target": std::path::absolute(path).ok().as_deref().and_then(std::path::Path::to_str),
                    "file_sha256": sha256, "environment_sha256": crate::config_review::environment_digest(),
                    "version": env!("CARGO_PKG_VERSION"), "working_directory": std::env::current_dir().ok().as_deref().and_then(std::path::Path::to_str),
                })
            );
        }
    }

    fn load(&self, mut snapshot: Option<&mut Snapshot>) -> io::Result<Generation> {
        let mut file_sha256 = None;
        let file: FileConfig = match &self.path {
            Some(path) => {
                let bytes = fs::read(path).map_err(|e| {
                    io::Error::new(e.kind(), format!("JAPKA_CONFIG ({}): {e}", path.display()))
                })?;
                file_sha256 = Some(crate::config_review::digest(&bytes));
                crate::settings::parse_json(&bytes).map_err(|e| {
                    crate::settings::context(e, &format!("JAPKA_CONFIG ({})", path.display()))
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
        if let Some(view) = snapshot.as_deref_mut() {
            view.routes_source = if file.routes.is_some() {
                "file"
            } else {
                "environment"
            };
            view.tls_source = if file.tls.is_some() {
                "file"
            } else {
                "environment"
            };
        }
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
                if let Some(view) = snapshot.as_deref_mut() {
                    view.certificate(None, &cert, &key)?;
                    for host in &hosts {
                        view.certificate(
                            Some(&host.host),
                            &base.join(&host.cert),
                            &base.join(&host.key),
                        )?;
                    }
                }
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
        let proxy = file.proxy.resolve()?;
        let routes = routing::load(
            definitions,
            &policies,
            &proxy,
            !self.check,
            self.fixed_upstream,
        )?;
        if let Some(view) = snapshot {
            view.proxy = proxy.for_review();
            view.file_sha256.clone_from(&file_sha256);
            view.routes = routes.iter().map(routing::Route::definition).collect();
            view.sites = sites
                .iter()
                .map(|(host, site)| {
                    Ok((
                        host.clone(),
                        crate::config_review::path_string(&site.root)?.to_owned(),
                    ))
                })
                .collect::<io::Result<_>>()?;
        }
        Ok(Generation {
            file_sha256,
            sites,
            routes,
            tls,
            policies,
        })
    }
}

pub fn start(
    chunk: usize,
    mode: crate::Mode<'_>,
    fixed_upstream: bool,
) -> io::Result<(Live, Option<String>)> {
    let address = env::var_os("TLS_LISTEN")
        .map(|s| {
            s.into_string()
                .map_err(|_| io::Error::other("invalid TLS_LISTEN"))
        })
        .transpose()?;
    let check = mode.check();
    let path = mode.path();
    let source = Arc::new(Source {
        path,
        chunk,
        check,
        fixed_upstream,
        secure: address.is_some(),
    });
    if check {
        return Ok((watch::channel(source.load(mode.snapshot())?).1, address));
    }
    #[cfg(unix)]
    let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let (publisher, live) = watch::channel(source.load(None)?);
    source.log_active(&live.borrow());
    #[cfg(unix)]
    tokio::spawn(async move {
        while hup.recv().await.is_some() {
            let loader = source.clone();
            match tokio::task::spawn_blocking(move || loader.load(None)).await {
                Ok(Ok(candidate)) => {
                    publisher.send_replace(candidate);
                    source.log_active(&publisher.borrow());
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
