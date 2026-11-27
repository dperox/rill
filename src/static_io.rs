//! Serve ordinary files in small groups on Tokio's blocking pool.
mod cache;
mod response;
pub use cache::AssetCache;
pub(crate) use response::{has_repeated_condition, serve_static};

use axum::{
    body::{Body, Bytes},
    extract::Request,
    http::{Method, header},
    response::Response,
};
use response::{StaticConditions, static_condition_response, static_etag, static_precondition};
use std::{
    cell::RefCell,
    env,
    fs::File,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, sync::oneshot};
use tokio_util::io::ReaderStream;

const INLINE_LIMIT: u64 = 64 * 1024;
const MAX_BATCH: usize = 8;

pub fn stream_chunk_bytes() -> io::Result<usize> {
    match env::var("RILL_STATIC_CHUNK_KIB") {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|kib| (4..=256).contains(kib))
            .map(|kib| kib * 1024)
            .ok_or_else(|| {
                io::Error::other("RILL_STATIC_CHUNK_KIB must be an integer from 4 to 256")
            }),
        Err(env::VarError::NotPresent) => Ok(64 * 1024),
        Err(error) => Err(io::Error::other(error)),
    }
}

struct FileJob {
    path: PathBuf,
    head: bool,
    #[cfg(target_os = "linux")]
    socket: Option<Arc<tokio::net::tcp::OwnedWriteHalf>>,
    stream_chunk: usize,
    #[cfg(target_os = "linux")]
    cache: Option<Arc<AssetCache>>,
    conditions: StaticConditions,
    reply: oneshot::Sender<io::Result<Response>>,
}

type Batch = Mutex<Vec<FileJob>>;

thread_local! {
    static PENDING: RefCell<Weak<Batch>> = const { RefCell::new(Weak::new()) };
}

fn ordinary_file(
    root: Arc<PathBuf>,
    stream_chunk: usize,
    req: &Request,
    conditions: &StaticConditions,
    cache: Option<&Arc<AssetCache>>,
) -> Option<oneshot::Receiver<io::Result<Response>>> {
    let head = *req.method() == Method::HEAD;
    let uri_path = req.uri().path();
    if !(head || *req.method() == Method::GET)
        || req.headers().contains_key(header::RANGE)
        || uri_path.contains(['%', '\\'])
    {
        return None;
    }
    let relative = Path::new(uri_path.strip_prefix('/')?);
    if !relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let mut path = root.join(relative);
    if uri_path.ends_with('/') {
        path.push("index.html");
    }

    let (reply, receive) = oneshot::channel();
    #[cfg(target_os = "linux")]
    let socket = crate::plain::socket(req.version());
    #[cfg(target_os = "linux")]
    let cache = if let (Some(cache), Some(socket)) = (cache, socket.as_ref()) {
        if AssetCache::eligible(uri_path) {
            if let Some(asset) = cache.get(path.as_os_str()) {
                let _ = reply.send(Ok(asset.response(head, conditions, socket.clone())));
                return Some(receive);
            }
            (!head).then(|| cache.clone())
        } else {
            None
        }
    } else {
        None
    };
    #[cfg(not(target_os = "linux"))]
    let _ = cache;
    let job = FileJob {
        path,
        head,
        #[cfg(target_os = "linux")]
        socket,
        #[cfg(target_os = "linux")]
        cache,
        stream_chunk,
        conditions: conditions.clone(),
        reply,
    };
    PENDING.with_borrow_mut(|slot| {
        if let Some(pending) = slot.upgrade() {
            let mut jobs = pending.lock().unwrap();
            if !jobs.is_empty() && jobs.len() < MAX_BATCH {
                jobs.push(job);
                return;
            }
        }
        let pending = Arc::new(Mutex::new(vec![job]));
        *slot = Arc::downgrade(&pending);
        tokio::task::spawn_blocking(move || {
            // Seal the group before the first open. New arrivals must observe a fresh lookup.
            let jobs = std::mem::take(&mut *pending.lock().unwrap());
            serve_batch(jobs);
        });
    });
    Some(receive)
}

enum FileResponse {
    Inline(Response<Bytes>),
    Separate(Response),
}

fn serve_batch(jobs: Vec<FileJob>) {
    let mut jobs = jobs.into_iter().peekable();
    while let Some(job) = jobs.next() {
        if job.reply.is_closed() {
            continue;
        }
        let result = match read_file(&job) {
            Ok(FileResponse::Inline(response)) => {
                // Path equality normalizes some spellings (e.g. a trailing "/.").
                // Only share when the kernel would see the identical path bytes.
                while let Some(next) = jobs.next_if(|next| {
                    #[cfg(target_os = "linux")]
                    if next.cache.is_some() {
                        return false;
                    }
                    next.path.as_os_str() == job.path.as_os_str()
                        && next.head == job.head
                        && next.conditions == job.conditions
                }) {
                    if !next.reply.is_closed() {
                        let _ = next.reply.send(Ok(response.clone().map(Body::from)));
                    }
                }
                Ok(response.map(Body::from))
            }
            Ok(FileResponse::Separate(response)) => Ok(response),
            Err(error) => Err(error),
        };
        let _ = job.reply.send(result);
    }
}

fn read_file(job: &FileJob) -> io::Result<FileResponse> {
    // An earlier job in this sealed batch may have filled the cache.
    #[cfg(target_os = "linux")]
    if let (Some(cache), Some(socket)) = (&job.cache, &job.socket)
        && let Some(asset) = cache.get(job.path.as_os_str())
    {
        return Ok(FileResponse::Separate(asset.response(
            false,
            &job.conditions,
            socket.clone(),
        )));
    }
    let mut opts = File::options();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = opts.open(&job.path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other("requires standard static handler"));
    }

    let len = meta.len();
    // Validators use whole seconds, matching the precision of the HTTP date.
    let seconds = meta
        .modified()
        .ok()
        .and_then(|date| date.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs())
        // httpdate supports HTTP years 1970 through 9999.
        .filter(|&seconds| seconds < 253_402_300_800);
    let modified = seconds.map(|s| UNIX_EPOCH + Duration::from_secs(s));
    let etag = seconds.map(|s| static_etag(s, len));
    let last_modified =
        modified.map(|date| header::HeaderValue::from_str(&httpdate::fmt_http_date(date)).unwrap());
    if let Some(status) = static_precondition(&job.conditions, modified, etag.as_ref()) {
        return Ok(FileResponse::Separate(static_condition_response(
            status,
            last_modified,
            etag,
        )));
    }

    let mut response = Response::builder()
        .header(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static(
                mime_guess::from_path(&job.path)
                    .first_raw()
                    .unwrap_or("application/octet-stream"),
            ),
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len);
    if let Some(value) = last_modified {
        response = response.header(header::LAST_MODIFIED, value);
    }
    if let Some(value) = etag {
        response = response.header(header::ETAG, value);
    }
    #[cfg(target_os = "linux")]
    if let (Some(cache), Some(socket)) = (&job.cache, &job.socket) {
        let asset = cache::Asset::load(file, len, response, modified)?;
        let asset = cache.insert(job.path.clone().into_os_string(), asset);
        return Ok(FileResponse::Separate(asset.response(
            false,
            &job.conditions,
            socket.clone(),
        )));
    }
    if job.head || len <= INLINE_LIMIT {
        let bytes = if job.head || len == 0 {
            Bytes::new()
        } else {
            read_inline(&mut file, len)?
        };
        return response
            .body(bytes)
            .map(FileResponse::Inline)
            .map_err(io::Error::other);
    }
    #[cfg(target_os = "linux")]
    if let Some(socket) = &job.socket {
        return response
            .extension(direct_body(socket.clone(), file, len))
            .body(Body::empty())
            .map(FileResponse::Separate)
            .map_err(io::Error::other);
    }
    let reader = tokio::fs::File::from_std(file).take(len);
    let body = Body::from_stream(ReaderStream::with_capacity(reader, job.stream_chunk));
    response
        .body(body)
        .map(FileResponse::Separate)
        .map_err(io::Error::other)
}

fn read_inline(file: &mut File, len: u64) -> io::Result<Bytes> {
    let mut data = vec![0; len as usize];
    file.read_exact(&mut data)?;
    Ok(Bytes::from(data))
}

#[cfg(target_os = "linux")]
fn direct_body(
    socket: Arc<tokio::net::tcp::OwnedWriteHalf>,
    file: impl std::borrow::Borrow<File> + Send + 'static,
    len: u64,
) -> hyper::ext::Http1DirectBody {
    hyper::ext::Http1DirectBody::new(len, async move {
        crate::sendfile::transfer(socket, file, len, 1024 * 1024).await?;
        Ok(len)
    })
}
