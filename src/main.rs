mod access_log;
mod config_review;
mod edge;
mod migration;
mod plain;
#[cfg(windows)]
mod transmitfile;

mod policy;
mod reload;
mod routing;

mod settings;
mod sites;
mod static_io;
mod streams;

mod upstream;
mod websocket;
use axum::serve::ListenerExt;
use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode, Uri, header, uri::Authority},
    response::{IntoResponse, Response},
};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use std::{
    env, io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpListener;

fn error_page(code: StatusCode) -> Response {
    let number = code.as_u16();
    let reason = code.canonical_reason().unwrap_or("Request failed");
    let body = format!(
        r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Japka | {number}</title></head>
<body><main><p>Japka</p><h1>{number} {reason}</h1></main></body>
</html>
"#
    );
    (code, [(header::CONTENT_TYPE, "text/html")], body).into_response()
}

struct Pool {
    client: upstream::Transport,
    failover: bool,
    servers: Vec<upstream::Endpoint>,
    healthy: Vec<std::sync::atomic::AtomicBool>,
    next: AtomicUsize,
    active: Option<std::sync::Mutex<Vec<usize>>>,
    policy: policy::Policies,
}

fn is_protected_header(name: &header::HeaderName) -> bool {
    matches!(
        *name,
        header::CONTENT_LENGTH
            | header::HOST
            | header::CONTENT_TYPE
            | header::CONTENT_ENCODING
            | header::CONTENT_RANGE
            | header::AUTHORIZATION
            | header::COOKIE
            | header::SET_COOKIE
    )
}

fn is_hop_header(name: &header::HeaderName) -> bool {
    matches!(
        *name,
        header::CONNECTION
            | header::PROXY_AUTHENTICATE
            | header::PROXY_AUTHORIZATION
            | header::TE
            | header::TRAILER
            | header::TRANSFER_ENCODING
            | header::UPGRADE
    ) || matches!(name.as_str(), "keep-alive" | "proxy-connection")
}

fn strip_hop_headers(headers: &mut HeaderMap) {
    // Most messages carry no hop-by-hop fields. One scan avoids a hash lookup per name.
    if !headers.keys().any(is_hop_header) {
        return;
    }
    if let header::Entry::Occupied(connection) = headers.entry(header::CONNECTION) {
        let (_, mut values) = connection.remove_entry_mult();
        let first = values.next().unwrap();
        let remaining: Vec<_> = values.collect();
        for value in std::iter::once(first).chain(remaining) {
            for token in value.as_bytes().split(|&b| b == b',') {
                let token = token.trim_ascii();
                if let Ok(name) = header::HeaderName::from_bytes(token)
                    && !is_protected_header(&name)
                {
                    headers.remove(name);
                }
            }
        }
    }
    for name in [
        header::HeaderName::from_static("keep-alive"),
        header::HeaderName::from_static("proxy-connection"),
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
        header::TE,
        header::TRAILER,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
    ] {
        headers.remove(name);
    }
}

struct Lease(Arc<Pool>, usize);
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(active) = &self.0.active {
            active.lock().unwrap_or_else(|e| e.into_inner())[self.1] -= 1;
        }
    }
}

async fn proxy(pool: Arc<Pool>, mut req: Request) -> Response {
    if routing::has_traversal_segment(req.uri().path()) {
        return error_page(StatusCode::BAD_REQUEST);
    }
    let downstream = if websocket::requests_upgrade(req.headers()) {
        if req.method() != axum::http::Method::GET
            || req.version() != axum::http::Version::HTTP_11
            || !websocket::connection_upgrade(req.headers())
            || !axum::body::HttpBody::is_end_stream(req.body())
        {
            return error_page(StatusCode::BAD_REQUEST);
        }
        let Some(upgrade) = websocket::downstream(&mut req) else {
            return error_page(StatusCode::BAD_REQUEST);
        };
        Some(upgrade)
    } else {
        None
    };
    *req.version_mut() = axum::http::Version::HTTP_11;
    strip_hop_headers(req.headers_mut());
    let handshake = if let Some(downstream) = downstream {
        let Some(handshake) = websocket::Handshake::new(downstream, req.headers()) else {
            return error_page(StatusCode::BAD_REQUEST);
        };
        websocket::set_upgrade_headers(req.headers_mut());
        Some(handshake)
    } else {
        None
    };
    let cache_key = if handshake.is_some() {
        None
    } else {
        match pool.policy.cache_lookup(&req) {
            policy::Lookup::Hit(response) => return response,
            policy::Lookup::Miss(key) => key,
            policy::Lookup::Wait(waiter) => {
                if let Some(response) = waiter.cached_response().await {
                    return response;
                }
                None
            }
        }
    };
    let Some((mut index, mut lease)) = pool.select(None) else {
        return error_page(StatusCode::SERVICE_UNAVAILABLE);
    };
    let logging = req.extensions().get::<access_log::Enabled>().is_some();
    let replayable = pool.failover
        && pool.servers.len() > 1
        && handshake.is_none()
        && matches!(
            *req.method(),
            axum::http::Method::GET | axum::http::Method::HEAD
        )
        && axum::body::HttpBody::is_end_stream(req.body());
    let (parts, body) = req.into_parts();
    let mut retry = replayable.then(|| Request::from_parts(parts.clone(), Body::empty()));
    let mut req = Request::from_parts(parts, body);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let result = loop {
        let peer = &pool.servers[index];
        req.headers_mut().insert(header::HOST, peer.host_header());
        let result = if let Some(fixed) = pool.client.fixed()
            && handshake.is_none()
            && req.method() == axum::http::Method::GET
            && req.uri().scheme().is_none()
            && req.uri().authority().is_none()
            && req.uri().path().starts_with('/')
            && axum::body::HttpBody::is_end_stream(req.body())
        {
            tokio::time::timeout_at(deadline, fixed.request(req)).await
        } else {
            let mut parts = std::mem::take(req.uri_mut()).into_parts();
            parts.scheme = Some(peer.scheme.clone());
            parts.authority = Some(peer.authority.clone());
            let Ok(uri) = Uri::from_parts(parts) else {
                return error_page(StatusCode::BAD_REQUEST);
            };
            *req.uri_mut() = uri;
            tokio::time::timeout_at(deadline, pool.client.request(req)).await
        };
        if matches!(&result, Ok(Err(upstream::RequestError::Connect)))
            && let Some(next_request) = retry.take()
            && let Some((next_index, next_lease)) = pool.select(Some(index))
        {
            index = next_index;
            lease = next_lease;
            req = next_request;
            continue;
        }
        break result;
    };
    let mut response = match result {
        Ok(Ok(response)) if response.status() == StatusCode::SWITCHING_PROTOCOLS => {
            websocket::switch_protocols(response, handshake, lease)
        }
        Ok(Ok(mut response)) => {
            strip_hop_headers(response.headers_mut());
            if matches!(
                response.status(),
                StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED
            ) {
                response.headers_mut().remove(header::CONTENT_LENGTH);
            }
            let response = if let Some(lease) = lease {
                use http_body_util::BodyExt;
                response.map(|body| {
                    Body::new(body.map_frame(move |frame| {
                        let _hold = &lease;
                        frame
                    }))
                })
            } else {
                response.map(Body::new)
            };
            pool.policy.cache_response(cache_key, response).await
        }
        Ok(Err(_)) => error_page(StatusCode::BAD_GATEWAY),
        Err(_) => error_page(StatusCode::GATEWAY_TIMEOUT),
    };
    if logging {
        response
            .extensions_mut()
            .insert(pool.servers[index].authority.clone());
    }
    response
}

fn print_help() {
    println!(
        "japka {} - HTTP reverse proxy, load balancer and static file server

Usage:
  japka                         Start the server using environment/JAPKA_CONFIG
  japka config test             Validate configuration and exit 0 (valid) or 1 (invalid)
  japka config list [--json]    List startup settings and the reloadable file schema
  japka config schema           Print the JSON Schema for JAPKA_CONFIG
  japka config show [--json]    Show resolved routes, sites and TLS file references
  japka config plan BASE CANDIDATE [--json]
                                Validate both files in this environment and preview changes
  japka migrate nginx [DUMP|-]  Translate an `nginx -T` dump into a candidate report (exit 0/2/1)
  japka --version | -V          Print version information
  japka --help | -h             Show this help message

Key environment variables:
  LISTEN                HTTP bind address (default: 127.0.0.1:$PORT or 127.0.0.1:8080)
  DOCUMENT_ROOT         Default static site directory (default: public)
  JAPKA_CONFIG           Path to reloadable JSON configuration (sites, routes, tls, proxy)
  TLS_LISTEN            HTTPS/HTTP2/HTTP3 bind address (requires TLS_CERT/TLS_KEY or tls config)
  UPSTREAMS             Comma-separated default /api/ upstream endpoints
  JAPKA_FIXED_UPSTREAM   Set to 1 for the single-peer GET fast path (default: 0)
  JAPKA_ACCESS_LOG       Set to 1 for JSON access records on stderr",
        env!("CARGO_PKG_VERSION")
    );
}

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => runtime(Mode::Serve),
        ["config", "test"] => migration::validation(runtime(Mode::Test)),
        ["config", "list"] | ["config", "list", "--json"] => config_review::list(),
        ["config", "schema"] => config_review::schema(),
        ["config", "show"] | ["config", "show", "--json"] => config_review::show(),
        ["config", "plan", base, candidate] | ["config", "plan", base, candidate, "--json"] => {
            config_review::plan(base, candidate)
        }
        ["migrate", "nginx"] => migration::run("-"),
        ["migrate", "nginx", "--help"]
        | ["migrate", "nginx", "-h"]
        | ["migrate", "--help"]
        | ["migrate", "-h"]
        | ["config", "--help"]
        | ["config", "-h"]
        | [
            "config",
            "test" | "list" | "show" | "plan" | "schema",
            "--help" | "-h",
        ]
        | ["--help"]
        | ["-h"] => {
            print_help();
            Ok(())
        }
        ["--version"] | ["-V"] => {
            println!("japka {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["migrate", "nginx", path] => migration::run(path),
        _ => migration::usage(),
    };
    if let Err(error) = result {
        eprintln!("Japka error: {error}");
        std::process::exit(1);
    }
}

// Linux can give a process two CPUs of time while allowing it to run on many
// more CPUs. A pinned pair and a quota-limited pod measured differently.
fn wider_cpu_affinity(cpus: usize) -> bool {
    let Ok(status) = std::fs::read_to_string("/proc/thread-self/status") else {
        return false;
    };
    let Some(mask) = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed:"))
    else {
        return false;
    };
    let count = mask.trim().split(',').try_fold(0usize, |sum, word| {
        u32::from_str_radix(word, 16).map(|bits| sum + bits.count_ones() as usize)
    });
    count.is_ok_and(|count| count > cpus)
}

enum Mode<'a> {
    Serve,
    Test,
    Inspect {
        path: Option<&'a std::path::Path>,
        snapshot: &'a mut config_review::Snapshot,
    },
}

impl<'a> Mode<'a> {
    fn check(&self) -> bool {
        !matches!(self, Self::Serve)
    }
    fn path(&self) -> Option<PathBuf> {
        match self {
            Self::Inspect {
                path: Some(path), ..
            } => Some(path.to_path_buf()),
            _ => settings::var_os("JAPKA_CONFIG").map(PathBuf::from),
        }
    }
    fn snapshot(self) -> Option<&'a mut config_review::Snapshot> {
        match self {
            Self::Inspect { snapshot, .. } => Some(snapshot),
            _ => None,
        }
    }
}

fn runtime(mode: Mode<'_>) -> io::Result<()> {
    let check = mode.check();
    let cpus = std::thread::available_parallelism().map_or(1, |count| count.get());
    let enable_sendfile = match crate::settings::var("JAPKA_SENDFILE").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        Err(env::VarError::NotPresent) => cfg!(any(windows, target_os = "linux")),
        _ => return Err(io::Error::other("JAPKA_SENDFILE must be 0 or 1")),
    };
    let streams = streams::configured()?;
    let direct_http = cfg!(target_os = "linux")
        && enable_sendfile
        && streams.is_empty()
        && env::var_os("TLS_LISTEN").is_none()
        && crate::settings::var_os("JAPKA_STATIC_LISTEN").is_none()
        && !policy::compression_from_env()?;
    let workers = match env::var("TOKIO_WORKER_THREADS") {
        Ok(value) => value
            .parse::<std::num::NonZeroUsize>()
            .map_err(|_| io::Error::other("TOKIO_WORKER_THREADS must be a positive integer"))?
            .get(),
        Err(env::VarError::NotPresent) if direct_http && cpus == 2 && wider_cpu_affinity(cpus) => 1,
        Err(env::VarError::NotPresent) => cpus,
        Err(error) => return Err(io::Error::other(error)),
    };
    // A larger proxy, including a file-configured server that can gain routes
    // on reload, uses the shared scheduler.
    let independent_io = direct_http
        && workers > 1
        && (workers == 2 || (mode.path().is_none() && routing::from_env()?.is_empty()));
    let blocking_threads = match crate::settings::var("JAPKA_BLOCKING_THREADS") {
        Ok(value) => usize::from(
            value
                .parse::<std::num::NonZeroU16>()
                .map_err(|_| {
                    io::Error::other("JAPKA_BLOCKING_THREADS must be between 1 and 65535")
                })?
                .get(),
        ),
        Err(env::VarError::NotPresent) if direct_http && workers == 1 && cpus == 2 => 2,
        Err(env::VarError::NotPresent) if independent_io && workers <= cpus && workers <= 4 => {
            workers
        }
        Err(env::VarError::NotPresent) => 1,
        Err(error) => return Err(io::Error::other(error)),
    };
    if !check {
        eprintln!("Japka workers: {workers} network, up to {blocking_threads} blocking");
    }
    if independent_io && blocking_threads >= workers {
        let reactors = plain::WorkerReactors::new(workers, blocking_threads);
        return reactors
            .runtime(0)?
            .block_on(run(mode, Some(reactors), enable_sendfile, streams));
    }
    let mut runtime = if workers == 1 {
        tokio::runtime::Builder::new_current_thread()
    } else {
        let mut runtime = tokio::runtime::Builder::new_multi_thread();
        runtime.worker_threads(workers);
        runtime
    };
    runtime
        .enable_all()
        .max_blocking_threads(blocking_threads)
        .build()?
        .block_on(tokio::task::LocalSet::new().run_until(run(mode, None, enable_sendfile, streams)))
}

enum Target {
    Proxy(Arc<Pool>),
    Static(sites::Site),
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).ok();
        let mut int = signal(SignalKind::interrupt()).ok();
        tokio::select! {
            _ = async {
                if let Some(ref mut s) = term {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
            _ = async {
                if let Some(ref mut s) = int {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn env_utf8(name: &str) -> io::Result<Option<String>> {
    match crate::settings::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(io::Error::other(format!("{name} must be valid UTF-8"))),
    }
}

struct Dispatch {
    live: reload::Live,
    default_site: sites::Site,
    stream_chunk: usize,
}

impl Dispatch {
    // Select from one generation before yielding. In-flight requests retain their own target.
    fn handle(&self, req: Request) -> impl Future<Output = Response> + Send + use<> {
        let host_check = routing::validate_host(&req);
        let generation = self.live.borrow();
        let sni_mismatch = match &host_check {
            Ok(Some(authority)) => {
                if let Some(edge::NegotiatedSni(sni)) =
                    req.extensions().get::<edge::NegotiatedSni>()
                    && let Some(tls) = &generation.tls
                {
                    let req_host = authority.host().to_ascii_lowercase();
                    tls.named_hosts.contains(&req_host) && sni.as_deref() != Some(req_host.as_str())
                } else {
                    false
                }
            }
            _ => false,
        };
        let pool = routing::pick(
            &generation.routes,
            req.uri().path(),
            host_check.as_ref().ok().and_then(Option::as_ref),
        );
        let target = if let Some(pool) = pool {
            Target::Proxy(pool)
        } else {
            let matched = if generation.sites.is_empty() {
                None
            } else {
                host_check
                    .as_ref()
                    .ok()
                    .and_then(|h| h.as_ref())
                    .and_then(|host| {
                        generation
                            .sites
                            .iter()
                            .find(|(name, _)| host.host().eq_ignore_ascii_case(name))
                    })
            };
            Target::Static(matched.map_or(&self.default_site, |(_, site)| site).clone())
        };
        let stream_chunk = self.stream_chunk;
        async move {
            if let Err(status) = host_check {
                error_page(status)
            } else if sni_mismatch {
                error_page(StatusCode::MISDIRECTED_REQUEST)
            } else if static_io::has_repeated_condition(req.headers()) {
                error_page(StatusCode::BAD_REQUEST)
            } else {
                match target {
                    Target::Proxy(_)
                        if req
                            .extensions()
                            .get::<plain::Transport>()
                            .is_some_and(|mode| mode.static_only) =>
                    {
                        error_page(StatusCode::NOT_FOUND)
                    }
                    Target::Proxy(pool) => proxy(pool, req).await,
                    Target::Static(site) => {
                        static_io::serve_static(
                            site.root,
                            site.files,
                            stream_chunk,
                            site.cache,
                            req,
                        )
                        .await
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct PlainApp {
    router: Router,
    direct: Option<Arc<Dispatch>>,
}

impl PlainApp {
    // Installing any middleware disables direct dispatch by construction.
    fn with_middleware(self, apply: impl FnOnce(Router) -> Router) -> Self {
        Self {
            router: apply(self.router),
            direct: None,
        }
    }

    async fn respond(self, req: Request) -> Response {
        // Keep Axum's method-specific HEAD and CONNECT response handling.
        if req.method() == axum::http::Method::GET
            && let Some(dispatch) = self.direct
        {
            return dispatch.handle(req).await;
        }
        tower::ServiceExt::oneshot(self.router, req).await.unwrap()
    }
}

async fn run(
    mode: Mode<'_>,
    reactors: Option<plain::WorkerReactors>,
    enable_sendfile: bool,
    streams: Vec<streams::ListenerConfig>,
) -> io::Result<()> {
    let check = mode.check();
    let stream_chunk = static_io::stream_chunk_bytes()?;
    let root = PathBuf::from(env::var_os("DOCUMENT_ROOT").unwrap_or_else(|| "public".into()));
    let default_site = sites::Site::open(&root, stream_chunk)?;
    let fixed_upstream = match env_utf8("JAPKA_FIXED_UPSTREAM")?.as_deref() {
        None | Some("0") => false,
        Some("1") => true,
        _ => return Err(io::Error::other("JAPKA_FIXED_UPSTREAM must be 0 or 1")),
    };
    let (live, tls_address) = reload::start(stream_chunk, mode, fixed_upstream)?;
    let dispatch = Arc::new(Dispatch {
        live: live.clone(),
        default_site,
        stream_chunk,
    });
    let fallback = dispatch.clone();
    let app = Router::new().fallback_service(tower::service_fn(move |req| {
        let future = fallback.handle(req);
        async move { Ok::<_, std::convert::Infallible>(future.await) }
    }));
    let app = PlainApp {
        router: app,
        direct: fixed_upstream.then_some(dispatch),
    };
    let initial_policies = live.borrow().policies.clone();
    let app = initial_policies.apply_inner(app, live.clone());
    let app = edge::redirect_from_env(app)?;
    let app = initial_policies.apply_access(app);
    let (app, logging) = access_log::configure(app)?;
    let requires_client_addr = logging || initial_policies.requires_client_addr();
    let compression = initial_policies.compression_enabled();
    let address = match env_utf8("LISTEN")? {
        Some(value) => value,
        None => {
            let port = env_utf8("PORT")?.unwrap_or_else(|| "8080".into());
            format!("127.0.0.1:{port}")
        }
    };
    let static_addr = env_utf8("JAPKA_STATIC_LISTEN")?;
    if check {
        let mut resolved_tcp = std::collections::HashSet::new();
        for (name, addr) in std::iter::once(("LISTEN", &address))
            .chain(tls_address.as_ref().map(|a| ("TLS_LISTEN", a)))
            .chain(static_addr.as_ref().map(|a| ("JAPKA_STATIC_LISTEN", a)))
        {
            let addrs: std::collections::HashSet<_> = tokio::net::lookup_host(addr)
                .await
                .map_err(|e| io::Error::other(format!("{name} ({addr}): {e}")))?
                .collect();
            if addrs.is_empty() {
                return Err(io::Error::other(format!(
                    "{name} ({addr}) did not resolve to any socket address"
                )));
            }
            for sock in addrs {
                if sock.port() != 0 && !resolved_tcp.insert(sock) {
                    return Err(io::Error::other(format!(
                        "conflicting listener address {sock} on {name}"
                    )));
                }
            }
        }
        for stream in &streams {
            if stream.tcp && stream.address.port() != 0 && !resolved_tcp.insert(stream.address) {
                return Err(io::Error::other(format!(
                    "conflicting TCP listener address {}",
                    stream.address
                )));
            }
        }
        return Ok(());
    }
    let listener = TcpListener::bind(&address)
        .await
        .map_err(|e| io::Error::new(e.kind(), format!("bind LISTEN ({address}): {e}")))?;
    eprintln!("Japka listening on http://{}", listener.local_addr()?);
    let mut jobs = tokio::task::JoinSet::new();
    for service in streams::bind(streams).await? {
        let task = service.spawn();
        jobs.spawn(async move { task.await.map_err(io::Error::other)? });
    }
    if let Some(address) = tls_address {
        let secure = app.router.clone();
        jobs.spawn(async move { edge::serve(&address, secure, live).await });
    }
    if let Some(addr) = static_addr {
        let static_listener = TcpListener::bind(&addr).await.map_err(|e| {
            io::Error::new(e.kind(), format!("bind JAPKA_STATIC_LISTEN ({addr}): {e}"))
        })?;
        eprintln!(
            "Japka static listener on http://{}",
            static_listener.local_addr()?
        );
        jobs.spawn_local(plain::serve(
            static_listener,
            app.clone(),
            plain::Transport {
                regions: enable_sendfile && !compression,
                static_only: true,
            },
        ));
    }
    if enable_sendfile && !compression {
        let transport = plain::Transport {
            regions: true,
            static_only: false,
        };
        if let Some(reactors) = reactors {
            let outcome = tokio::select! {
                result = reactors.serve(listener, app, transport) => result,
                _ = shutdown_signal() => Ok(()),
            };
            access_log::flush();
            return outcome;
        }
        jobs.spawn_local(plain::serve(listener, app, transport));
    } else {
        jobs.spawn(async move {
            let listener = listener.tap_io(|stream| {
                if let Err(error) = stream.set_nodelay(true) {
                    eprintln!("could not set TCP_NODELAY on client socket: {error}");
                }
            });
            if requires_client_addr {
                axum::serve(
                    listener,
                    app.router
                        .into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .await
            } else if app.direct.is_none() {
                axum::serve(listener, app.router).await
            } else {
                let service = tower::service_fn(move |req| {
                    let app = app.clone();
                    async move { Ok::<_, std::convert::Infallible>(app.respond(req).await) }
                });
                axum::serve(listener, tower::make::Shared::new(service)).await
            }
        });
    }
    let outcome = tokio::select! {
        result = jobs.join_next() => {
            result
                .ok_or_else(|| io::Error::other("no listeners"))?
                .map_err(io::Error::other)?
        }
        _ = shutdown_signal() => {
            jobs.abort_all();
            Ok(())
        }
    };
    access_log::flush();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_hop_headers_protects_framing_and_handles_non_utf8_connection() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONNECTION,
            header::HeaderValue::from_bytes(b"content-length, x-hop, \x80, authorization, x-hop-2")
                .unwrap(),
        );
        headers.insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from_static("5"),
        );
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_static("Bearer tok"),
        );
        headers.insert("x-hop", header::HeaderValue::from_static("1"));
        headers.insert("x-hop-2", header::HeaderValue::from_static("2"));
        headers.insert(
            header::TRANSFER_ENCODING,
            header::HeaderValue::from_static("chunked"),
        );

        strip_hop_headers(&mut headers);
        assert_eq!(headers.get(header::CONTENT_LENGTH).unwrap(), "5");
        assert_eq!(headers.get(header::AUTHORIZATION).unwrap(), "Bearer tok");
        assert!(!headers.contains_key(header::CONNECTION));
        assert!(!headers.contains_key(header::TRANSFER_ENCODING));
        assert!(!headers.contains_key("x-hop"));
        assert!(!headers.contains_key("x-hop-2"));

        let mut headers = HeaderMap::new();
        headers.insert("keep-alive", header::HeaderValue::from_static("timeout=5"));
        headers.insert(header::TE, header::HeaderValue::from_static("trailers"));
        headers.insert(header::ETAG, header::HeaderValue::from_static("\"v1\""));
        strip_hop_headers(&mut headers);
        assert!(!headers.contains_key("keep-alive"));
        assert!(!headers.contains_key(header::TE));
        assert_eq!(headers.get(header::ETAG).unwrap(), "\"v1\"");
    }
}
