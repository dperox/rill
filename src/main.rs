mod access_log;
mod edge;
mod migration;
mod plain;
#[cfg(windows)]
mod transmitfile;

mod policy;
mod reload;
mod routing;

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
<head><meta charset="utf-8"><title>Rill | {number}</title></head>
<body><main><p>Rill</p><h1>{number} {reason}</h1></main></body>
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

fn strip_hop_headers(headers: &mut HeaderMap) {
    if let header::Entry::Occupied(connection) = headers.entry(header::CONNECTION) {
        let (_, mut values) = connection.remove_entry_mult();
        let first = values.next().unwrap();
        let remaining: Vec<_> = values.collect();
        for value in std::iter::once(first).chain(remaining) {
            if let Ok(value) = value.to_str() {
                for token in value.split(',') {
                    headers.remove(token.trim());
                }
            }
        }
    }
    for name in [
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}

struct Lease(Arc<Pool>, usize);
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(active) = &self.0.active {
            active.lock().unwrap()[self.1] -= 1;
        }
    }
}

async fn proxy(pool: Arc<Pool>, mut req: Request) -> Response {
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
    let cache_key = if downstream.is_some() {
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
        let mut parts = std::mem::take(req.uri_mut()).into_parts();
        parts.scheme = Some(peer.scheme.clone());
        parts.authority = Some(peer.authority.clone());
        let Ok(uri) = Uri::from_parts(parts) else {
            return error_page(StatusCode::BAD_REQUEST);
        };
        *req.uri_mut() = uri;
        req.headers_mut().insert(header::HOST, peer.host_header());
        let result = tokio::time::timeout_at(deadline, pool.client.request(req)).await;
        if matches!(&result, Ok(Err(error)) if error.is_connect())
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

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => runtime(false),
        ["config", "test"] => migration::validation(runtime(true)),
        ["migrate", "nginx"] => migration::run("-"),
        ["migrate", "nginx", path] => migration::run(path),
        ["--help"] | ["-h"] => {
            println!("rill | rill config test | rill migrate nginx [DUMP|-]");
            Ok(())
        }
        _ => migration::usage(),
    }
}

fn runtime(check: bool) -> io::Result<()> {
    let cpus = std::thread::available_parallelism().map_or(1, |count| count.get());
    let workers = match env::var("TOKIO_WORKER_THREADS") {
        Ok(value) => value
            .parse::<std::num::NonZeroUsize>()
            .map_err(|_| io::Error::other("TOKIO_WORKER_THREADS must be a positive integer"))?
            .get(),
        Err(env::VarError::NotPresent) => cpus,
        Err(error) => return Err(io::Error::other(error)),
    };
    let blocking_threads = match env::var("RILL_BLOCKING_THREADS") {
        Ok(value) => usize::from(
            value
                .parse::<std::num::NonZeroU16>()
                .map_err(|_| io::Error::other("RILL_BLOCKING_THREADS must be between 1 and 65535"))?
                .get(),
        ),
        Err(env::VarError::NotPresent) => 1,
        Err(error) => return Err(io::Error::other(error)),
    };
    let enable_sendfile = match env::var("RILL_SENDFILE").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        Err(env::VarError::NotPresent) => cfg!(any(windows, target_os = "linux")),
        _ => return Err(io::Error::other("RILL_SENDFILE must be 0 or 1")),
    };
    let streams = streams::configured()?;
    // Each independent reactor needs at least one blocking worker. Other
    // listeners also use Tokio's shared executor, so keep their existing pool.
    if cfg!(target_os = "linux")
        && workers > 1
        && blocking_threads >= workers
        && enable_sendfile
        && streams.is_empty()
        && env::var_os("TLS_LISTEN").is_none()
        && env::var_os("RILL_STATIC_LISTEN").is_none()
        && !policy::compression_from_env()?
    {
        let reactors = plain::WorkerReactors::new(workers, blocking_threads);
        return reactors
            .runtime(0)?
            .block_on(run(check, Some(reactors), enable_sendfile, streams));
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
        .block_on(tokio::task::LocalSet::new().run_until(run(
            check,
            None,
            enable_sendfile,
            streams,
        )))
}

enum Target {
    Proxy(Arc<Pool>),
    Static(sites::Site),
}

async fn run(
    check: bool,
    reactors: Option<plain::WorkerReactors>,
    enable_sendfile: bool,
    streams: Vec<streams::ListenerConfig>,
) -> io::Result<()> {
    let stream_chunk = static_io::stream_chunk_bytes()?;
    let root = PathBuf::from(env::var_os("DOCUMENT_ROOT").unwrap_or_else(|| "public".into()));
    let default_site = sites::Site::open(&root, stream_chunk)?;
    let (live, tls_address) = reload::start(stream_chunk, check)?;
    let routes = live.clone();
    let app = Router::new().fallback_service(tower::service_fn(move |req: Request| {
        let generation = routes.borrow();
        let pool = routing::pick(&generation.routes, &req);
        let target = if let Some(pool) = pool {
            Target::Proxy(pool)
        } else {
            let matched = if generation.sites.is_empty() {
                None
            } else {
                routing::authority(&req).and_then(|host| {
                    generation
                        .sites
                        .iter()
                        .find(|(name, _)| host.host().eq_ignore_ascii_case(name))
                })
            };
            Target::Static(matched.map_or(&default_site, |(_, site)| site).clone())
        };
        async move {
            let response = if static_io::has_repeated_condition(req.headers()) {
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
            };
            Ok::<_, std::convert::Infallible>(response)
        }
    }));
    let app = edge::redirect_from_env(app)?;
    let app = live.borrow().policies.apply(app, live.clone());
    let (app, logging) = access_log::configure(app)?;
    let requires_client_addr = logging || live.borrow().policies.requires_client_addr();
    let compression = live.borrow().policies.compression_enabled();
    let address = env::var("LISTEN").unwrap_or_else(|_| {
        format!(
            "127.0.0.1:{}",
            env::var("PORT").unwrap_or_else(|_| "8080".into())
        )
    });
    let static_addr = env::var("RILL_STATIC_LISTEN").ok();
    if check {
        for addr in std::iter::once(&address)
            .chain(tls_address.as_ref())
            .chain(static_addr.as_ref())
        {
            if tokio::net::lookup_host(addr).await?.next().is_none() {
                return Err(io::Error::other("listener address did not resolve"));
            }
        }
        return Ok(());
    }
    let listener = TcpListener::bind(address).await?;
    eprintln!("Rill listening on http://{}", listener.local_addr()?);
    let mut jobs = tokio::task::JoinSet::new();
    for service in streams::bind(streams).await? {
        let task = service.spawn();
        jobs.spawn(async move { task.await.map_err(io::Error::other)? });
    }
    if let Some(address) = tls_address {
        let secure = app.clone();
        jobs.spawn(async move { edge::serve(&address, secure, live).await });
    }
    if let Some(addr) = static_addr {
        let static_listener = TcpListener::bind(addr).await?;
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
            return reactors.serve(listener, app, transport).await;
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
                    app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .await
            } else {
                axum::serve(listener, app).await
            }
        });
    }
    jobs.join_next()
        .await
        .ok_or_else(|| io::Error::other("no listeners"))?
        .map_err(io::Error::other)?
}
