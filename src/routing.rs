use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    #[serde(default)]
    host: Option<String>,
    prefix: String,
    upstreams: Vec<String>,
}

pub struct Route {
    host: Option<String>,
    prefix: String,
    pool: Arc<Pool>,
    probes: Vec<tokio::task::AbortHandle>,
}

impl Drop for Route {
    fn drop(&mut self) {
        for task in &self.probes {
            task.abort();
        }
    }
}

pub fn from_env() -> io::Result<Vec<Definition>> {
    let mut definitions: Vec<Definition> = match env::var("RILL_ROUTES") {
        Ok(json) => serde_json::from_str(&json).map_err(io::Error::other)?,
        Err(env::VarError::NotPresent) => vec![],
        Err(e) => return Err(io::Error::other(e)),
    };
    if let Ok(list) = env::var("UPSTREAMS") {
        definitions.push(Definition {
            host: None,
            prefix: "/api/".into(),
            upstreams: list.split(',').map(|s| s.trim().to_owned()).collect(),
        });
    }
    Ok(definitions)
}

pub fn load(definitions: Vec<Definition>, policy: &policy::Policies) -> io::Result<Vec<Route>> {
    let least = match env::var("RILL_HTTP_BALANCE").as_deref() {
        Ok("round_robin") | Err(env::VarError::NotPresent) => false,
        Ok("least_connections") => true,
        _ => return Err(io::Error::other("invalid RILL_HTTP_BALANCE")),
    };
    let health = upstream::Health::from_env()?;
    let mut routes = Vec::new();
    for d in definitions {
        if !d.prefix.starts_with('/')
            || !d.prefix.ends_with('/')
            || d.prefix.contains(['?', '#'])
            || d.upstreams.is_empty()
        {
            return Err(io::Error::other(
                "each route needs a /prefix/ and at least one upstream",
            ));
        }
        let host = d.host.map(|s| s.to_ascii_lowercase());
        if routes
            .iter()
            .any(|r: &Route| r.prefix == d.prefix && r.host == host)
        {
            return Err(io::Error::other("duplicate virtual host and prefix"));
        }
        let servers = d
            .upstreams
            .iter()
            .map(|s| upstream::Endpoint::parse(s))
            .collect::<io::Result<Vec<_>>>()?;
        let client = upstream::Transport::new(
            servers
                .iter()
                .any(|s| s.scheme == axum::http::uri::Scheme::HTTPS),
        )?;
        let healthy = if health.is_some() {
            (0..servers.len()).map(|_| true.into()).collect()
        } else {
            vec![]
        };
        let active = least.then(|| std::sync::Mutex::new(vec![0; servers.len()]));
        routes.push(Route {
            host,
            probes: Vec::new(),
            prefix: d.prefix,
            pool: Arc::new(Pool {
                servers,
                active,
                policy: policy.clone(),
                next: AtomicUsize::new(0),
                client,
                healthy,
            }),
        });
    }
    if let Some(health) = health {
        for route in &mut routes {
            route.probes = health.start(&route.pool);
        }
    }
    routes.sort_by_key(|r| {
        (
            std::cmp::Reverse(r.host.is_some()),
            std::cmp::Reverse(r.prefix.len()),
        )
    });
    Ok(routes)
}

pub fn pick(routes: &[Route], req: &Request) -> Option<Arc<Pool>> {
    let host_header = std::cell::OnceCell::new();
    for route in routes {
        if !req.uri().path().starts_with(&route.prefix) {
            continue;
        }
        if let Some(expected) = &route.host {
            let authority = req.uri().authority().or_else(|| {
                host_header
                    .get_or_init(|| {
                        req.headers()
                            .get(header::HOST)?
                            .to_str()
                            .ok()?
                            .parse::<Authority>()
                            .ok()
                    })
                    .as_ref()
            });
            if !authority.is_some_and(|a| a.host().eq_ignore_ascii_case(expected)) {
                continue;
            }
        }
        return Some(route.pool.clone());
    }
    None
}
