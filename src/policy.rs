use axum::{
    Router,
    body::{Body, Bytes, HttpBody},
    extract::{ConnectInfo, Request, State},
    http::{Extensions, HeaderMap, HeaderValue, Method, StatusCode, Version, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use ipnet::IpNet;
use std::{
    collections::HashMap,
    env, io,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::{Semaphore, watch};
use tower_http::{
    compression::CompressionLayer,
    compression::predicate::{DefaultPredicate, Predicate},
};

const MAX_BODY: usize = 64 * 1024;
const MAX_METADATA: usize = 8 * 1024;
const MAX_FILLS: usize = 16;
const MAX_ENTRIES: usize = 256;
const MAX_IPS: usize = 16_384;
const WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct Policies {
    access: Arc<Access>,
    cache: Option<Arc<Cache>>,
    compress: bool,
}

struct Access {
    allow: Vec<IpNet>,
    deny: Vec<IpNet>,
    limiter: Option<RateLimiter>,
}

struct RateLimiter {
    limit: u64,
    window: Mutex<RateWindow>,
}

struct RateWindow {
    start: Instant,
    counts: HashMap<IpAddr, u64>,
}

struct Cache {
    ttl: u64,
    coalesce: bool,
    state: Mutex<CacheState>,
    fills: Arc<Semaphore>,
}

#[derive(Default)]
struct CacheState {
    generation: u64,
    entries: HashMap<Arc<CacheKey>, Entry>,
    flights: HashMap<Arc<CacheKey>, watch::Sender<()>>,
}

pub enum Lookup {
    Hit(Response),
    Wait(Waiter),
    Miss(Option<PendingCacheKey>),
}

pub struct Waiter {
    cache: Arc<Cache>,
    key: Arc<CacheKey>,
    done: watch::Receiver<()>,
}

impl Waiter {
    pub async fn cached_response(mut self) -> Option<Response> {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.done.changed()).await;
        self.cache.state.lock().unwrap().hit(&self.key)
    }
}

struct Flight {
    cache: Arc<Cache>,
    key: Arc<CacheKey>,
}

// Closing the channel releases every follower, including on cancellation.
impl Drop for Flight {
    fn drop(&mut self) {
        self.cache.state.lock().unwrap().flights.remove(&self.key);
    }
}

pub struct PendingCacheKey {
    _flight: Option<Flight>,
    key: Arc<CacheKey>,
    generation: u64,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct CacheKey {
    secure: bool,
    authority: Option<String>,
    target: String,
    headers: [Vec<HeaderValue>; 5],
}

struct Entry {
    inserted: Instant,
    ttl: Duration,
    initial_age: u64,
    headers: HeaderMap,
    body: Bytes,
}

fn invalid(var: &str, reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{var} {reason}"))
}

fn variable(name: &str) -> io::Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(invalid(name, "must be valid UTF-8")),
    }
}

fn number(name: &str) -> io::Result<u64> {
    variable(name)?
        .map(|value| {
            value
                .parse()
                .map_err(|_| invalid(name, "must be an unsigned integer"))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn networks(name: &str) -> io::Result<Vec<IpNet>> {
    variable(name)?
        .unwrap_or_default()
        .split(',')
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            let value = value.trim();
            value
                .parse::<IpNet>()
                .or_else(|_| value.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| invalid(name, "must contain only IP addresses or CIDR networks"))
        })
        .collect()
}

impl Policies {
    pub fn from_env() -> io::Result<Self> {
        let ttl = number("CACHE_TTL")?;
        let limit = number("RATE_LIMIT")?;
        let coalesce = match variable("CACHE_COALESCE")?.as_deref() {
            None | Some("0") => false,
            Some("1") if ttl > 0 => true,
            _ => {
                return Err(invalid(
                    "CACHE_COALESCE",
                    "use 0 or use 1 with CACHE_TTL enabled",
                ));
            }
        };
        let compress = match variable("COMPRESS")?.as_deref() {
            None | Some("0") => false,
            Some("1") => true,
            _ => return Err(invalid("COMPRESS", "must be 0 or 1")),
        };
        Ok(Self {
            access: Arc::new(Access {
                allow: networks("ALLOW_IPS")?,
                deny: networks("DENY_IPS")?,
                limiter: (limit != 0).then(|| RateLimiter {
                    limit,
                    window: Mutex::new(RateWindow {
                        start: Instant::now(),
                        counts: HashMap::new(),
                    }),
                }),
            }),
            cache: (ttl != 0).then(|| {
                Arc::new(Cache {
                    ttl,
                    coalesce,
                    state: Mutex::new(CacheState::default()),
                    fills: Arc::new(Semaphore::new(MAX_FILLS)),
                })
            }),
            compress,
        })
    }

    #[cfg(any(windows, target_os = "linux"))]
    pub fn compression_enabled(&self) -> bool {
        self.compress
    }

    pub fn requires_client_addr(&self) -> bool {
        !self.access.allow.is_empty()
            || !self.access.deny.is_empty()
            || self.access.limiter.is_some()
    }

    pub fn apply(&self, mut app: Router, live: crate::reload::Live) -> Router {
        if self.compress {
            app = app
                .layer(middleware::from_fn(mark_for_compression))
                .layer(
                    CompressionLayer::new()
                        .compress_when(DefaultPredicate::new().and(compressible)),
                )
                .layer(middleware::from_fn(weaken_compressed_etag));
        }
        if self.requires_client_addr() {
            app = app.layer(middleware::from_fn_with_state(
                self.access.clone(),
                check_access,
            ));
        }
        if self.cache.is_some() {
            app = app.layer(middleware::from_fn_with_state(live, invalidate_on_write));
        }
        app
    }

    pub fn cache_lookup(&self, request: &Request) -> Lookup {
        let Some(cache) = &self.cache else {
            return Lookup::Miss(None);
        };
        let Some(key) = CacheKey::from_request(request) else {
            return Lookup::Miss(None);
        };
        let mut state = cache.state.lock().unwrap();
        if let Some(response) = state.hit(&key) {
            return Lookup::Hit(response);
        }
        if let Some((stored, sender)) = state.flights.get_key_value(&key) {
            return Lookup::Wait(Waiter {
                cache: cache.clone(),
                key: stored.clone(),
                done: sender.subscribe(),
            });
        }
        if cache.coalesce && state.flights.len() >= MAX_FILLS {
            return Lookup::Miss(None);
        }
        let key = Arc::new(key);
        let flight = cache.coalesce.then(|| {
            state.flights.insert(key.clone(), watch::channel(()).0);
            Flight {
                cache: cache.clone(),
                key: key.clone(),
            }
        });
        Lookup::Miss(Some(PendingCacheKey {
            key,
            generation: state.generation,
            _flight: flight,
        }))
    }

    pub async fn cache_response(
        &self,
        pending: Option<PendingCacheKey>,
        response: Response,
    ) -> Response {
        let (Some(cache), Some(pending)) = (&self.cache, pending) else {
            return response;
        };
        let Some((ttl, initial_age)) = cache_freshness(&response, cache.ttl) else {
            return response;
        };
        let Ok(permit) = cache.fills.clone().try_acquire_owned() else {
            return response;
        };
        let cache = cache.clone();
        let inserted = Instant::now();
        let (parts, mut body) = response.into_parts();
        let length = body.size_hint().exact().unwrap() as usize;
        let headers = owned_headers(&parts.headers);
        let stream = async_stream::stream! {
            let _permit = permit;
            let mut fill = Some((pending, headers, Vec::with_capacity(length)));
            while let Some(frame) = body.frame().await {
                let data = frame.as_ref().ok().and_then(|frame| frame.data_ref());
                if let (Some((_, _, bytes)), Some(data)) = (&mut fill, data)
                    && data.len() <= length - bytes.len()
                {
                    bytes.extend_from_slice(data);
                    // The upstream HTTP/1 decoder knows there can be no more
                    // frames when its fixed content-length is exhausted.
                    if bytes.len() == length && body.is_end_stream()
                        && inserted.elapsed() < ttl
                        && let Some((pending, headers, bytes)) = fill.take()
                    {
                        cache.state.lock().unwrap().insert(pending.key, pending.generation, Entry {
                            inserted, ttl, initial_age, headers,
                            body: Bytes::from(bytes),
                        });
                    }
                } else {
                    fill = None;
                }
                yield frame;
            }
        };
        Response::from_parts(parts, Body::new(http_body_util::StreamBody::new(stream)))
    }
}

impl CacheState {
    fn hit(&mut self, key: &CacheKey) -> Option<Response> {
        let entry = self.entries.get(key)?;
        let elapsed = entry.inserted.elapsed();
        if elapsed >= entry.ttl {
            self.entries.remove(key);
            return None;
        }
        let mut response = Response::new(Body::from(entry.body.clone()));
        *response.headers_mut() = entry.headers.clone();
        response.headers_mut().insert(
            header::AGE,
            HeaderValue::from(entry.initial_age.saturating_add(elapsed.as_secs())),
        );
        Some(response)
    }

    fn insert(&mut self, key: Arc<CacheKey>, generation: u64, entry: Entry) {
        if generation != self.generation {
            return;
        }
        if self.entries.len() == MAX_ENTRIES {
            self.entries
                .retain(|_, entry| entry.inserted.elapsed() < entry.ttl);
        }
        if self.entries.len() == MAX_ENTRIES && !self.entries.contains_key(&key) {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted)
                .map(|(key, _)| key.clone());
            if let Some(key) = oldest {
                self.entries.remove(&key);
            }
        }
        self.entries.insert(key, entry);
    }
}

// Isolates cleartext and encrypted requests even when a listener uses
// origin-form URIs that don't carry the transport scheme.
#[derive(Clone)]
pub struct SecureTransport;

fn owned_headers(headers: &HeaderMap) -> HeaderMap {
    headers
        .iter()
        .map(|(name, value)| (name.clone(), owned_header(value)))
        .collect()
}

fn owned_header(value: &HeaderValue) -> HeaderValue {
    let mut copy = HeaderValue::from_bytes(value.as_bytes()).unwrap();
    copy.set_sensitive(value.is_sensitive());
    copy
}

impl CacheKey {
    fn from_request(request: &Request) -> Option<Self> {
        let headers = request.headers();
        let allowed = [
            header::HOST,
            header::ACCEPT,
            header::ACCEPT_ENCODING,
            header::USER_AGENT,
            header::ACCEPT_LANGUAGE,
        ];
        let target = request.uri().path_and_query()?.as_str();
        if request.method() != Method::GET
            || request.body().size_hint().exact() != Some(0)
            || target.len()
                + metadata_size(headers)
                + request
                    .uri()
                    .authority()
                    .map_or(0, |value| value.as_str().len())
                > MAX_METADATA
            || headers.keys().any(|name| !allowed.contains(name))
        {
            return None;
        }
        Some(Self {
            target: target.to_owned(),
            secure: request.extensions().get::<SecureTransport>().is_some(),
            authority: request.uri().authority().map(ToString::to_string),
            headers: allowed.map(|name| headers.get_all(name).iter().map(owned_header).collect()),
        })
    }
}

fn metadata_size(headers: &HeaderMap) -> usize {
    headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 32)
        .sum()
}

fn cache_freshness(response: &Response, cap: u64) -> Option<(Duration, u64)> {
    let headers = response.headers();
    if response.status() != StatusCode::OK
        || !(1..=MAX_BODY as u64).contains(&response.body().size_hint().exact()?)
        || metadata_size(headers) > MAX_METADATA
        || [
            header::SET_COOKIE,
            header::VARY,
            header::CONTENT_RANGE,
            header::CONTENT_ENCODING,
            header::WWW_AUTHENTICATE,
            header::TRAILER,
        ]
        .iter()
        .any(|name| headers.contains_key(name))
    {
        return None;
    }

    // Accept the simple opt-in directives only. Unrecognized directives or
    // ambiguous syntax make this response ineligible for the shared cache.
    let mut public = false;
    let mut max_age = None;
    let mut shared_max_age = None;
    for value in headers.get_all(header::CACHE_CONTROL) {
        for directive in value.to_str().ok()?.split(',').map(str::trim) {
            if directive.eq_ignore_ascii_case("public") {
                public = true;
                continue;
            }
            let (name, seconds) = directive.split_once('=')?;
            let age = if name.eq_ignore_ascii_case("max-age") {
                &mut max_age
            } else if name.eq_ignore_ascii_case("s-maxage") {
                &mut shared_max_age
            } else {
                return None;
            };
            let seconds = if let Some(quoted) = seconds.strip_prefix('"') {
                quoted.strip_suffix('"')?
            } else {
                seconds
            };
            if seconds.is_empty()
                || !seconds.bytes().all(|byte| byte.is_ascii_digit())
                || age.replace(seconds.parse::<u64>().ok()?).is_some()
            {
                return None;
            }
        }
    }
    if !public {
        return None;
    }

    let mut initial_age = match headers.get(header::AGE) {
        Some(value) => value.to_str().ok()?.parse::<u64>().ok()?,
        None => 0,
    };
    if let Some(date) = headers.get(header::DATE) {
        let date = httpdate::parse_http_date(date.to_str().ok()?).ok()?;
        let apparent = SystemTime::now()
            .duration_since(date)
            .unwrap_or_default()
            .as_secs();
        initial_age = initial_age.max(apparent);
    }
    let remaining = shared_max_age
        .or(max_age)?
        .saturating_sub(initial_age)
        .min(cap);
    (remaining > 0).then(|| (Duration::from_secs(remaining), initial_age))
}

async fn invalidate_on_write(
    State(live): State<crate::reload::Live>,
    request: Request,
    next: Next,
) -> Response {
    let target = (!request.method().is_safe())
        .then(|| request.uri().path_and_query().map(ToString::to_string))
        .flatten();
    let response = next.run(request).await;
    if let Some(target) = target
        && (response.status().is_success() || response.status().is_redirection())
    {
        // Even an earlier generation's write can change data served by the current routes.
        let generation = live.borrow();
        let Some(cache) = &generation.policies.cache else {
            return response;
        };
        let mut state = cache.state.lock().unwrap();
        state.entries.retain(|key, _| key.target != target);
        state.generation = state.generation.wrapping_add(1);
    }
    response
}

async fn check_access(State(access): State<Arc<Access>>, request: Request, next: Next) -> Response {
    let Some(peer) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return crate::error_page(StatusCode::FORBIDDEN);
    };
    let ip = peer.ip().to_canonical();
    if access.deny.iter().any(|net| net.contains(&ip))
        || (!access.allow.is_empty() && !access.allow.iter().any(|net| net.contains(&ip)))
    {
        return crate::error_page(StatusCode::FORBIDDEN);
    }

    if let Some(limiter) = &access.limiter {
        let mut window = limiter.window.lock().unwrap();
        if window.start.elapsed() >= WINDOW {
            window.start = Instant::now();
            window.counts.clear();
        }
        let at_capacity = window.counts.len() >= MAX_IPS;
        let accepted = match window.counts.get_mut(&ip) {
            Some(count) if *count < limiter.limit => {
                *count += 1;
                true
            }
            None if !at_capacity => {
                window.counts.insert(ip, 1);
                true
            }
            _ => false,
        };
        if !accepted {
            let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            return response;
        }
    }
    next.run(request).await
}

#[derive(Clone)]
struct CompressionCandidate;

async fn mark_for_compression(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let mut response = next.run(request).await;
    if method != Method::HEAD && !response.headers().contains_key(header::CONTENT_ENCODING) {
        response.extensions_mut().insert(CompressionCandidate);
    }
    response
}

fn compressible(
    status: StatusCode,
    _: Version,
    headers: &HeaderMap,
    extensions: &Extensions,
) -> bool {
    extensions.get::<CompressionCandidate>().is_some()
        && !matches!(
            status,
            StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED | StatusCode::PARTIAL_CONTENT
        )
        && !["digest", "content-digest", "content-md5", "repr-digest"]
            .iter()
            .any(|name| headers.contains_key(*name))
        && headers.get_all(header::CACHE_CONTROL).iter().all(|value| {
            value.to_str().is_ok_and(|value| {
                !value
                    .split(',')
                    .any(|directive| directive.trim().eq_ignore_ascii_case("no-transform"))
            })
        })
}

async fn weaken_compressed_etag(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    if response
        .extensions_mut()
        .remove::<CompressionCandidate>()
        .is_some()
        && response.headers().contains_key(header::CONTENT_ENCODING)
        && let Some(etag) = response.headers_mut().get_mut(header::ETAG)
        && etag.as_bytes().starts_with(b"\"")
    {
        let mut weak = Vec::with_capacity(etag.as_bytes().len() + 2);
        weak.extend_from_slice(b"W/");
        weak.extend_from_slice(etag.as_bytes());
        *etag = HeaderValue::from_bytes(&weak).unwrap();
    }
    response
}
