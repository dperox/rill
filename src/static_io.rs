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
    root: Arc<PathBuf>,
    path: PathBuf,
    head: bool,
    regions: bool,
    stream_chunk: usize,
    #[cfg(target_os = "linux")]
    cache: Option<Arc<AssetCache>>,
    conditions: StaticConditions,
    reply: oneshot::Sender<io::Result<OrdinaryOutcome>>,
}

pub(super) enum OrdinaryOutcome {
    Response(Response),
    Reject(axum::http::StatusCode),
    Fallback,
}

pub(super) fn has_unsafe_windows_segment(segment: &str) -> bool {
    if segment.contains(':') {
        return true;
    }
    let stem = segment
        .trim_end_matches([' ', '.'])
        .split('.')
        .next()
        .unwrap_or("");
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "CONIN$"
            | "CONOUT$"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn percent_decode_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            let decoded = ((hi << 4) | lo) as u8;
            if decoded == 0 {
                return None;
            }
            out.push(decoded);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Validate a path before delegating to `ServeDir` so symlinks outside `root`,
/// Windows ADS/device paths, and non-regular special files (e.g. FIFOs) never
/// escape `DOCUMENT_ROOT` or block Tokio's blocking pool.
pub(super) fn check_fallback_path(root: &Path, uri_path: &str) -> bool {
    let Some(decoded) = percent_decode_path(uri_path) else {
        return false;
    };
    if decoded.contains('\\') {
        return false;
    }
    let Some(stripped) = decoded.strip_prefix('/') else {
        return false;
    };
    let mut candidate = root.to_path_buf();
    for segment in stripped.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if !candidate.pop() || !candidate.starts_with(root) {
                return false;
            }
            continue;
        }
        if cfg!(windows) && has_unsafe_windows_segment(segment) {
            return false;
        }
        candidate.push(segment);
    }
    let mut check_target = candidate;
    if uri_path.ends_with('/') {
        let index = check_target.join("index.html");
        if index.symlink_metadata().is_ok() {
            check_target = index;
        }
    }
    match std::fs::canonicalize(&check_target) {
        Ok(canonical) => {
            if !canonical.starts_with(root) {
                return false;
            }
            match std::fs::metadata(&canonical) {
                Ok(meta) => {
                    if meta.is_dir() {
                        let index = canonical.join("index.html");
                        if let Ok(index_canon) = std::fs::canonicalize(&index) {
                            if !index_canon.starts_with(root) {
                                return false;
                            }
                            return std::fs::metadata(&index_canon)
                                .is_ok_and(|m| m.is_file() || m.is_dir());
                        }
                        true
                    } else {
                        meta.is_file()
                    }
                }
                Err(_) => true,
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
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
) -> Option<oneshot::Receiver<io::Result<OrdinaryOutcome>>> {
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
        .all(|c| matches!(c, Component::Normal(s) if !cfg!(windows) || !s.to_str().is_none_or(has_unsafe_windows_segment)))
    {
        return None;
    }
    let mut path = root.join(relative);
    if uri_path.ends_with('/') {
        path.push("index.html");
    }

    let (reply, receive) = oneshot::channel();
    let regions = req
        .extensions()
        .get::<crate::plain::Transport>()
        .is_some_and(|r| r.regions);
    #[cfg(target_os = "linux")]
    let cache = if let Some(cache) = cache.filter(|_| regions) {
        if AssetCache::eligible(uri_path) {
            if let Some(asset) = cache.get(path.as_os_str()) {
                let _ = reply.send(Ok(OrdinaryOutcome::Response(
                    asset.response(head, conditions),
                )));
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
        root,
        path,
        head,
        regions,
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
        let submit = move || {
            tokio::task::spawn_blocking(move || {
                // Seal before the first lookup. Later arrivals must see a fresh open.
                let jobs = std::mem::take(&mut *pending.lock().unwrap());
                serve_batch(jobs);
            });
        };
        if regions {
            // Let other ready connections join the batch before the blocking
            // worker seals it. There is no timer or minimum batch size.
            tokio::task::spawn_local(async move {
                submit();
            });
        } else {
            submit();
        }
    });
    Some(receive)
}

enum FileResponse {
    Inline(Response<Bytes>),
    Separate(Response),
    Reject(axum::http::StatusCode),
    Fallback,
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
                        let _ = next.reply.send(Ok(OrdinaryOutcome::Response(
                            response.clone().map(Body::from),
                        )));
                    }
                }
                Ok(OrdinaryOutcome::Response(response.map(Body::from)))
            }
            Ok(FileResponse::Separate(response)) => Ok(OrdinaryOutcome::Response(response)),
            Ok(FileResponse::Reject(status)) => Ok(OrdinaryOutcome::Reject(status)),
            Ok(FileResponse::Fallback) => Ok(OrdinaryOutcome::Fallback),
            Err(error) => Err(error),
        };
        let _ = job.reply.send(result);
    }
}

fn read_file(job: &FileJob) -> io::Result<FileResponse> {
    // An earlier job in this sealed batch may have filled the cache.
    #[cfg(target_os = "linux")]
    if let Some(cache) = &job.cache
        && let Some(asset) = cache.get(job.path.as_os_str())
    {
        return Ok(FileResponse::Separate(
            asset.response(false, &job.conditions),
        ));
    }
    let canonical = match std::fs::canonicalize(&job.path) {
        Ok(canonical) => canonical,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(FileResponse::Reject(axum::http::StatusCode::NOT_FOUND));
        }
        Err(err) => return Err(err),
    };
    if !canonical.starts_with(&*job.root) {
        return Ok(FileResponse::Reject(axum::http::StatusCode::NOT_FOUND));
    }
    let mut opts = File::options();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = opts.open(&canonical)?;
    let meta = file.metadata()?;
    if meta.is_dir() {
        return Ok(FileResponse::Fallback);
    }
    if !meta.is_file() {
        // Never delegate FIFOs, sockets, or device nodes to ServeDir, which
        // opens files without O_NONBLOCK and can deadlock Tokio's blocking pool.
        return Ok(FileResponse::Reject(axum::http::StatusCode::NOT_FOUND));
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
    if let Some(cache) = &job.cache {
        let asset = cache::Asset::load(file, len, response, modified)?;
        let asset = cache.insert(job.path.clone().into_os_string(), asset);
        return Ok(FileResponse::Separate(
            asset.response(false, &job.conditions),
        ));
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
    if job.regions {
        return response
            .extension(http_kit::FileRegion::new(file, 0, len))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_ads_and_dos_device_segments_rejected() {
        for bad in [
            "index.html::$DATA",
            "file:Zone.Identifier",
            "CON",
            "nul.txt",
            "COM1.log",
            "lpt9 ",
        ] {
            assert!(has_unsafe_windows_segment(bad), "{bad}");
        }
        for good in ["index.html", "console.js", "null.txt", "com10.bin"] {
            assert!(!has_unsafe_windows_segment(good), "{good}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn fallback_path_blocks_symlink_escape_and_fifo() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("rill-static-sec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("root");
        let outside = base.join("secret.txt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, b"secret").unwrap();
        std::fs::write(root.join("ok.txt"), b"ok").unwrap();
        symlink(&outside, root.join("escape.txt")).unwrap();
        symlink(root.join("ok.txt"), root.join("inside.txt")).unwrap();

        let canon_root = root.canonicalize().unwrap();
        assert!(check_fallback_path(&canon_root, "/ok.txt"));
        assert!(check_fallback_path(&canon_root, "/inside.txt"));
        assert!(!check_fallback_path(&canon_root, "/escape.txt"));
        assert!(!check_fallback_path(&canon_root, "/%65scape.txt"));
        assert!(!check_fallback_path(&canon_root, "/../secret.txt"));

        let fifo = root.join("pipe");
        let c_fifo = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);
        assert!(!check_fallback_path(&canon_root, "/pipe"));

        let _ = std::fs::remove_dir_all(&base);
    }
}
