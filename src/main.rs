mod edge;
#[cfg(target_os = "linux")]
mod plain;
mod policy;
mod reload;
mod routing;
#[cfg(target_os = "linux")]
mod sendfile;
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
    time::{Duration, SystemTime},
};
use tokio::net::TcpListener;
use tower::{Service, ServiceExt};
use tower_http::services::ServeDir;

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

fn static_etag(seconds: u64, length: u64) -> header::HeaderValue {
    format!("\"{seconds:x}-{length:x}\"").parse().unwrap()
}

fn response_length(headers: &HeaderMap) -> Option<u64> {
    let length = match headers.get(header::CONTENT_RANGE) {
        Some(range) => range.to_str().ok()?.rsplit_once('/')?.1,
        None => headers.get(header::CONTENT_LENGTH)?.to_str().ok()?,
    };
    length.parse().ok()
}

fn has_repeated_condition(headers: &HeaderMap) -> bool {
    headers.len() != headers.keys_len()
        && [
            header::IF_MATCH,
            header::IF_NONE_MATCH,
            header::IF_MODIFIED_SINCE,
            header::IF_UNMODIFIED_SINCE,
            header::IF_RANGE,
        ]
        .iter()
        .any(|name| headers.get_all(name).iter().nth(1).is_some())
}

#[derive(Clone, PartialEq)]
struct StaticConditions {
    unmodified_since: Option<header::HeaderValue>,
    if_match: Option<header::HeaderValue>,
    modified_since: Option<header::HeaderValue>,
    none_match: Option<header::HeaderValue>,
}

impl StaticConditions {
    fn remove_from(headers: &mut HeaderMap) -> Self {
        Self {
            unmodified_since: headers.remove(header::IF_UNMODIFIED_SINCE),
            if_match: headers.remove(header::IF_MATCH),
            modified_since: headers.remove(header::IF_MODIFIED_SINCE),
            none_match: headers.remove(header::IF_NONE_MATCH),
        }
    }
}

fn static_condition_response(
    status: StatusCode,
    last_modified: Option<header::HeaderValue>,
    etag: Option<header::HeaderValue>,
) -> Response {
    if status == StatusCode::NOT_MODIFIED {
        let mut response = status.into_response();
        if let Some(value) = last_modified {
            response.headers_mut().insert(header::LAST_MODIFIED, value);
        }
        if let Some(value) = etag {
            response.headers_mut().insert(header::ETAG, value);
        }
        response
    } else {
        error_page(status)
    }
}

fn etag_matches(
    value: &header::HeaderValue,
    etag: Option<&header::HeaderValue>,
    weak: bool,
) -> bool {
    let value = value.as_bytes();
    if value == b"*" {
        return true;
    }
    let Some(etag) = etag else {
        return false;
    };
    value.split(|byte| *byte == b',').any(|candidate| {
        let candidate = candidate.trim_ascii();
        let candidate = if weak {
            candidate.strip_prefix(b"W/").unwrap_or(candidate)
        } else {
            candidate
        };
        candidate == etag.as_bytes()
    })
}

fn static_precondition(
    conditions: &StaticConditions,
    modified: Option<SystemTime>,
    etag: Option<&header::HeaderValue>,
) -> Option<StatusCode> {
    let parse_date =
        |value: &header::HeaderValue| httpdate::parse_http_date(value.to_str().ok()?).ok();
    if conditions.unmodified_since.as_ref().is_some_and(|value| {
        modified
            .zip(parse_date(value))
            .is_none_or(|(modified, date)| date < modified)
    }) || conditions
        .if_match
        .as_ref()
        .is_some_and(|value| !etag_matches(value, etag, false))
    {
        return Some(StatusCode::PRECONDITION_FAILED);
    }

    let since = conditions.modified_since.as_ref();
    let none_match = conditions.none_match.as_ref();
    if (since.is_some() || none_match.is_some())
        && since.is_none_or(|value| modified.is_some_and(|date| Some(date) == parse_date(value)))
        && none_match.is_none_or(|value| etag_matches(value, etag, true))
    {
        return Some(StatusCode::NOT_MODIFIED);
    }

    None
}

fn file_validators(headers: &HeaderMap) -> (Option<SystemTime>, Option<header::HeaderValue>) {
    let last_modified = headers.get(header::LAST_MODIFIED);
    let modified = last_modified
        .and_then(|value| value.to_str().ok())
        .and_then(|date| httpdate::parse_http_date(date).ok());
    let etag = modified
        .and_then(|date| date.duration_since(SystemTime::UNIX_EPOCH).ok())
        .zip(response_length(headers))
        .map(|(elapsed, length)| static_etag(elapsed.as_secs(), length));

    (modified, etag)
}

async fn serve_dir(mut files: ServeDir, mut req: Request) -> Response {
    if req.headers().contains_key(header::RANGE)
        && let Some(validator) = req.headers_mut().remove(header::IF_RANGE)
    {
        let (mut parts, body) = req.into_parts();
        let ranged = files
            .call(Request::from_parts(parts.clone(), body))
            .await
            .unwrap()
            .into_response();
        if !matches!(
            ranged.status(),
            StatusCode::PARTIAL_CONTENT | StatusCode::RANGE_NOT_SATISFIABLE
        ) {
            return ranged;
        }
        // Check the file actually opened for this range, including an unsatisfiable range.
        let (modified, etag) = file_validators(ranged.headers());
        let date = validator
            .to_str()
            .ok()
            .and_then(|s| httpdate::parse_http_date(s).ok());
        if etag.as_ref() == Some(&validator) || modified.is_some_and(|mtime| Some(mtime) == date) {
            return ranged;
        }
        parts.headers.remove(header::RANGE);
        req = Request::from_parts(parts, Body::empty());
    }
    files.oneshot(req).await.unwrap().into_response()
}

async fn serve_static(
    root: Arc<PathBuf>,
    files: ServeDir,
    stream_chunk: usize,
    cache: Option<Arc<static_io::AssetCache>>,
    mut req: Request,
) -> Response {
    let conditions = StaticConditions::remove_from(req.headers_mut());

    if let Some(task) =
        static_io::ordinary_file(root, stream_chunk, &req, &conditions, cache.as_ref())
        && let Ok(Ok(response)) = task.await
    {
        return response;
    }

    // ServeDir retains file metadata on 206 and 416, so preconditions can
    // take precedence over the range result from the same file lookup.
    let mut res = serve_dir(files, req).await;
    match res.status() {
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => return error_page(res.status()),
        StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::RANGE_NOT_SATISFIABLE => {}
        _ => return res,
    }

    let last_modified = res.headers().get(header::LAST_MODIFIED);
    let (modified, etag) = file_validators(res.headers());

    if let Some(status) = static_precondition(&conditions, modified, etag.as_ref()) {
        return static_condition_response(status, last_modified.cloned(), etag);
    }

    match res.status() {
        StatusCode::RANGE_NOT_SATISFIABLE => {
            let mut error = error_page(res.status());
            if let Some(range) = res.headers_mut().remove(header::CONTENT_RANGE) {
                error.headers_mut().insert(header::CONTENT_RANGE, range);
            }
            return error;
        }
        StatusCode::PARTIAL_CONTENT => {
            res.headers_mut().remove(header::ACCEPT_RANGES);
        }
        _ => {}
    }
    if let Some(value) = etag {
        res.headers_mut().insert(header::ETAG, value);
    }
    res
}

struct Pool {
    client: upstream::Transport,
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
        let Some(upgrade) = req.extensions_mut().remove::<hyper::upgrade::OnUpgrade>() else {
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
    let Some((index, lease)) = pool.select() else {
        return error_page(StatusCode::SERVICE_UNAVAILABLE);
    };
    let peer = &pool.servers[index];
    *req.version_mut() = axum::http::Version::HTTP_11;
    let mut parts = std::mem::take(req.uri_mut()).into_parts();
    parts.scheme = Some(peer.scheme.clone());
    parts.authority = Some(peer.authority.clone());
    let Ok(uri) = Uri::from_parts(parts) else {
        return error_page(StatusCode::BAD_REQUEST);
    };
    *req.uri_mut() = uri;
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
    req.headers_mut().insert(header::HOST, peer.host_header());
    let result = tokio::time::timeout(Duration::from_secs(60), pool.client.request(req)).await;
    match result {
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
    }
}

fn main() -> io::Result<()> {
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
        .block_on(run())
}

async fn run() -> io::Result<()> {
    let root = PathBuf::from(env::var_os("DOCUMENT_ROOT").unwrap_or_else(|| "public".into()))
        .canonicalize()?;
    if !root.is_dir() {
        return Err(io::Error::other("DOCUMENT_ROOT must be a directory"));
    }
    let stream_chunk = static_io::stream_chunk_bytes()?;
    let asset_cache = static_io::AssetCache::from_env()?;
    let files = ServeDir::new(&root).with_buf_chunk_size(stream_chunk);
    let root = Arc::new(root);
    let (live, tls_address) = reload::start()?;
    let routes = live.clone();
    let app = Router::new().fallback_service(tower::service_fn(move |req: Request| {
        let pool = routing::pick(&routes.borrow().routes, &req);
        let files = files.clone();
        let root = root.clone();
        let cache = asset_cache.clone();
        async move {
            let response = if has_repeated_condition(req.headers()) {
                error_page(StatusCode::BAD_REQUEST)
            } else if let Some(pool) = pool {
                proxy(pool, req).await
            } else {
                serve_static(root, files, stream_chunk, cache, req).await
            };
            Ok::<_, std::convert::Infallible>(response)
        }
    }));
    let app = live.borrow().policies.apply(app, live.clone());
    let requires_client_addr = live.borrow().policies.requires_client_addr();
    #[cfg(target_os = "linux")]
    let compression = live.borrow().policies.compression_enabled();
    let address = env::var("LISTEN").unwrap_or_else(|_| {
        format!(
            "127.0.0.1:{}",
            env::var("PORT").unwrap_or_else(|_| "8080".into())
        )
    });
    let listener = TcpListener::bind(address).await?;
    eprintln!("Rill listening on http://{}", listener.local_addr()?);
    let mut jobs = tokio::task::JoinSet::new();
    for service in streams::bind_from_env().await? {
        let task = service.spawn();
        jobs.spawn(async move { task.await.map_err(io::Error::other)? });
    }
    if let Some(address) = tls_address {
        let secure = app.clone();
        jobs.spawn(async move { edge::serve(&address, secure, live).await });
    }
    let enable_sendfile = match env::var("RILL_SENDFILE").as_deref() {
        Ok("0") => false,
        Ok("1") | Err(env::VarError::NotPresent) => true,
        _ => return Err(io::Error::other("RILL_SENDFILE must be 0 or 1")),
    };
    jobs.spawn(async move {
        #[cfg(target_os = "linux")]
        if enable_sendfile && !compression {
            return plain::serve(listener, app, requires_client_addr).await;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = enable_sendfile;
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
    jobs.join_next()
        .await
        .ok_or_else(|| io::Error::other("no listeners"))?
        .map_err(io::Error::other)?
}
