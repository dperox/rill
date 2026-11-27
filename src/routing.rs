use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    #[serde(default)]
    host: Option<String>,
    prefix: String,
    upstreams: Vec<String>,
    #[serde(default)]
    failover: bool,
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
        Ok(json) => serde_json::from_str(&json)
            .map_err(|e| io::Error::other(format!("RILL_ROUTES: {e}")))?,
        Err(env::VarError::NotPresent) => vec![],
        Err(e) => return Err(io::Error::other(format!("RILL_ROUTES: {e}"))),
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

pub fn normalize_path(path: &str) -> String {
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
    out
}

pub fn has_traversal_segment(path: &str) -> bool {
    path.split('/').any(|s| s == "..")
}

pub fn load(
    definitions: Vec<Definition>,
    policy: &policy::Policies,
    start_probes: bool,
) -> io::Result<Vec<Route>> {
    let least = match env::var("RILL_HTTP_BALANCE").as_deref() {
        Ok("round_robin") | Err(env::VarError::NotPresent) => false,
        Ok("least_connections") => true,
        _ => {
            return Err(io::Error::other(
                "RILL_HTTP_BALANCE must be round_robin or least_connections",
            ));
        }
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
        let host = d
            .host
            .as_deref()
            .map(|s| validate_hostname(s, "route host"))
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

pub fn pick(routes: &[Route], req: &Request) -> Option<Arc<Pool>> {
    let path = normalize_path(req.uri().path());
    let host_header = std::cell::OnceCell::new();
    for route in routes {
        if !path.starts_with(&route.prefix) {
            continue;
        }
        if let Some(expected) = &route.host {
            let authority = host_header.get_or_init(|| authority(req)).as_ref();
            if !authority.is_some_and(|a| a.host().eq_ignore_ascii_case(expected)) {
                continue;
            }
        }
        return Some(route.pool.clone());
    }
    None
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
