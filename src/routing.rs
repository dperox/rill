pub(crate) mod config;
use super::*;
use serde::Deserialize;

#[derive(Deserialize, serde::Serialize, schemars::JsonSchema)]
#[schemars(rename = "Route")]
#[serde(deny_unknown_fields)]
pub struct Definition {
    /// Exact host, case insensitive. Omit to match any host.
    #[serde(default)]
    pub(crate) host: Option<String>,
    /// Path prefix including its leading and trailing slash.
    #[schemars(regex(pattern = "^/(?:[^?#]*/)?$"))]
    pub(crate) prefix: String,
    /// Ordered HTTP or HTTPS backend authorities.
    #[schemars(length(min = 1))]
    upstreams: Vec<String>,
    /// Retry eligible reads on another peer after a connection failure.
    #[serde(default)]
    failover: bool,
}

pub struct Route {
    host: Option<String>,
    prefix: String,
    pool: Arc<Pool>,
    probes: Vec<tokio::task::AbortHandle>,
}

impl Route {
    pub(crate) fn definition(&self) -> Definition {
        Definition {
            host: self.host.clone(),
            prefix: self.prefix.clone(),
            upstreams: self
                .pool
                .servers
                .iter()
                .map(|s| format!("{}://{}", s.scheme, s.authority))
                .collect(),
            failover: self.pool.failover,
        }
    }
}

impl Drop for Route {
    fn drop(&mut self) {
        for task in &self.probes {
            task.abort();
        }
    }
}

pub fn from_env() -> io::Result<Vec<Definition>> {
    let mut definitions: Vec<Definition> = match crate::settings::var("JAPKA_ROUTES") {
        Ok(json) => crate::settings::parse_json(json.as_bytes())
            .map_err(|e| crate::settings::context(e, "JAPKA_ROUTES"))?,
        Err(env::VarError::NotPresent) => vec![],
        Err(e) => return Err(io::Error::other(format!("JAPKA_ROUTES: {e}"))),
    };
    match env::var("UPSTREAMS") {
        Ok(list) if list.trim().is_empty() => {}
        Ok(list) => {
            let upstreams: Vec<String> = list.split(',').map(|s| s.trim().to_owned()).collect();
            if upstreams.iter().any(String::is_empty) {
                return Err(io::Error::other("UPSTREAMS contains an empty address"));
            }
            definitions.push(Definition {
                host: None,
                prefix: "/api/".into(),
                failover: false,
                upstreams,
            });
        }
        Err(env::VarError::NotPresent) => {}
        Err(e) => return Err(io::Error::other(format!("UPSTREAMS: {e}"))),
    }
    Ok(definitions)
}

pub fn validate_hostname(raw: &str, context: &str) -> io::Result<String> {
    let host = raw.parse::<Authority>().map_err(|_| {
        io::Error::other(format!("{context} needs an exact hostname without a port"))
    })?;
    if host.host().is_empty() || host.host() != raw || raw.contains(['*', '@']) {
        return Err(io::Error::other(format!(
            "{context} needs an exact hostname without a port"
        )));
    }
    Ok(raw.to_ascii_lowercase())
}

pub fn normalize_path(path: &str) -> std::borrow::Cow<'_, str> {
    if path.starts_with('/')
        && !path.contains("//")
        && !path.split('/').any(|s| matches!(s, "." | ".."))
    {
        return path.into();
    }
    let mut segments: Vec<&str> = Vec::new();
    let trailing_slash = path.ends_with('/') && path.len() > 1;
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let mut out = String::with_capacity(path.len().max(1));
    out.push('/');
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            out.push('/');
        }
        out.push_str(segment);
    }
    if trailing_slash && !out.ends_with('/') {
        out.push('/');
    }
    out.into()
}

pub fn has_traversal_segment(path: &str) -> bool {
    path.split('/').any(|s| s == "..")
}

pub fn load(
    definitions: Vec<Definition>,
    policy: &policy::Policies,
    settings: &config::PoolSettings,
    start_probes: bool,
    fixed_upstream: bool,
) -> io::Result<Vec<Route>> {
    let health = settings.health();
    let mut routes = Vec::new();
    for (index, d) in definitions.into_iter().enumerate() {
        if !d.prefix.starts_with('/') || !d.prefix.ends_with('/') || d.prefix.contains(['?', '#']) {
            return Err(crate::settings::field_error(
                format!("routes[{index}].prefix"),
                "expected a /prefix/ without a query or fragment",
            ));
        }
        if d.upstreams.is_empty() {
            return Err(crate::settings::field_error(
                format!("routes[{index}].upstreams"),
                "each route needs at least one upstream",
            ));
        }
        let host = d
            .host
            .as_deref()
            .map(|s| {
                validate_hostname(s, "route host")
                    .map_err(|e| crate::settings::field_error(format!("routes[{index}].host"), e))
            })
            .transpose()?;
        if routes
            .iter()
            .any(|r: &Route| r.prefix == d.prefix && r.host == host)
        {
            return Err(io::Error::other(format!(
                "duplicate virtual host and prefix: host={} prefix={}",
                host.as_deref().unwrap_or("*"),
                d.prefix
            )));
        }
        let servers = d
            .upstreams
            .iter()
            .enumerate()
            .map(|(peer, s)| {
                upstream::Endpoint::parse(s).map_err(|e| {
                    crate::settings::field_error(format!("routes[{index}].upstreams[{peer}]"), e)
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let client = upstream::Transport::new(&servers, fixed_upstream)?;
        let healthy = if health.is_some() {
            (0..servers.len()).map(|_| true.into()).collect()
        } else {
            vec![]
        };
        let active = settings
            .least_connections()
            .then(|| std::sync::Mutex::new(vec![0; servers.len()]));
        routes.push(Route {
            host,
            probes: Vec::new(),
            prefix: d.prefix,
            pool: Arc::new(Pool {
                servers,
                failover: d.failover,
                active,
                policy: policy.clone(),
                next: AtomicUsize::new(0),
                client,
                healthy,
            }),
        });
    }
    if start_probes && let Some(health) = health {
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

pub fn pick(routes: &[Route], path: &str, host: Option<&Authority>) -> Option<Arc<Pool>> {
    if routes.is_empty() {
        return None;
    }
    let path = normalize_path(path);
    routes
        .iter()
        .find(|route| {
            path.starts_with(&route.prefix)
                && route.host.as_ref().is_none_or(|expected| {
                    host.is_some_and(|h| h.host().eq_ignore_ascii_case(expected))
                })
        })
        .map(|route| route.pool.clone())
}

fn parse_clean_authority(raw: &str) -> Option<Authority> {
    if raw.is_empty() || raw.contains('@') {
        return None;
    }
    let authority: Authority = raw.parse().ok()?;
    if authority.host().is_empty() {
        return None;
    }
    Some(authority)
}

pub fn validate_host(req: &Request) -> Result<Option<Authority>, StatusCode> {
    let uri_authority = match req.uri().authority() {
        Some(authority) => {
            Some(parse_clean_authority(authority.as_str()).ok_or(StatusCode::BAD_REQUEST)?)
        }
        None => None,
    };
    let mut host_values = req.headers().get_all(header::HOST).iter();
    let host_authority = match host_values.next() {
        Some(first) => {
            if host_values.next().is_some() {
                return Err(StatusCode::BAD_REQUEST);
            }
            let raw = first.to_str().map_err(|_| StatusCode::BAD_REQUEST)?;
            Some(parse_clean_authority(raw).ok_or(StatusCode::BAD_REQUEST)?)
        }
        None => None,
    };
    if let (Some(uri_auth), Some(host_auth)) = (&uri_authority, &host_authority)
        && (!uri_auth.host().eq_ignore_ascii_case(host_auth.host())
            || uri_auth.port_u16() != host_auth.port_u16())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(uri_authority.or(host_authority))
}

pub fn authority(req: &Request) -> Option<Authority> {
    req.uri()
        .authority()
        .and_then(|a| parse_clean_authority(a.as_str()))
        .or_else(|| parse_clean_authority(req.headers().get(header::HOST)?.to_str().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_validation_rejects_ports_wildcards_and_userinfo() {
        assert_eq!(
            validate_hostname("Docs.Example", "route").unwrap(),
            "docs.example"
        );
        for bad in [
            "",
            "docs.example:8080",
            "*.example",
            "user@docs.example",
            "http://a.test",
        ] {
            assert!(validate_hostname(bad, "route").is_err(), "{bad}");
        }
    }

    #[test]
    fn path_normalization_and_traversal_detection() {
        assert_eq!(normalize_path("/api/./users"), "/api/users");
        assert_eq!(normalize_path("/x/../api/users"), "/api/users");
        assert_eq!(normalize_path("//api//users/"), "/api/users/");
        assert!(has_traversal_segment("/api/../secret"));
        assert!(!has_traversal_segment("/api/..hidden"));
    }

    #[test]
    fn validate_host_rejects_duplicate_userinfo_and_mismatch() {
        let ok = Request::builder()
            .uri("/index.html")
            .header(header::HOST, "app.example:8080")
            .body(Body::empty())
            .unwrap();
        assert_eq!(validate_host(&ok).unwrap().unwrap().host(), "app.example");

        let dup = Request::builder()
            .uri("/")
            .header(header::HOST, "a.example")
            .header(header::HOST, "b.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(validate_host(&dup), Err(StatusCode::BAD_REQUEST));

        let userinfo = Request::builder()
            .uri("/")
            .header(header::HOST, "user@a.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(validate_host(&userinfo), Err(StatusCode::BAD_REQUEST));

        let mismatch = Request::builder()
            .uri("http://a.example/")
            .header(header::HOST, "b.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(validate_host(&mismatch), Err(StatusCode::BAD_REQUEST));
    }
}
