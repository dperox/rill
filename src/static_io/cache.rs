use std::{env, io, sync::Arc};

#[cfg(target_os = "linux")]
use {
    super::{Body, Bytes, File, INLINE_LIMIT, Response, header, read_inline},
    crate::{StaticConditions, static_condition_response, static_precondition},
    std::{
        collections::{HashMap, VecDeque},
        ffi::{OsStr, OsString},
        sync::Mutex,
        time::SystemTime,
    },
    tokio::net::tcp::OwnedWriteHalf,
};

pub struct AssetCache {
    #[cfg(target_os = "linux")]
    capacity: usize,
    #[cfg(target_os = "linux")]
    entries: Mutex<Entries>,
}

impl AssetCache {
    pub fn from_env() -> io::Result<Option<Arc<Self>>> {
        let value = match env::var("RILL_ASSET_CACHE_ENTRIES") {
            Ok(value) => value,
            Err(env::VarError::NotPresent) => return Ok(None),
            Err(error) => return Err(io::Error::other(error)),
        };
        Self::from_entries(&value)
    }

    fn from_entries(value: &str) -> io::Result<Option<Arc<Self>>> {
        let capacity = value
            .parse::<usize>()
            .ok()
            .filter(|&n| n <= 4096)
            .ok_or_else(|| {
                io::Error::other("RILL_ASSET_CACHE_ENTRIES must be an integer from 0 to 4096")
            })?;
        if capacity == 0 {
            return Ok(None);
        }
        #[cfg(target_os = "linux")]
        return Ok(Some(Arc::new(Self {
            capacity,
            entries: Mutex::new(Entries::default()),
        })));
        #[cfg(not(target_os = "linux"))]
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct Entries {
    by_path: HashMap<Arc<OsStr>, Arc<Asset>>,
    fifo: VecDeque<Arc<OsStr>>,
}

#[cfg(target_os = "linux")]
impl AssetCache {
    pub(super) fn eligible(uri_path: &str) -> bool {
        let Some(path) = uri_path.strip_prefix('/') else {
            return false;
        };
        if path.split('/').any(|part| matches!(part, "" | "." | "..")) {
            return false;
        }
        let name = path.rsplit('/').next().unwrap_or("");
        let mut parts = name.rsplitn(3, '.');
        let (Some(extension), Some(hash), Some(base)) = (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        !base.is_empty()
            && !extension.is_empty()
            && (4..=64).contains(&hash.len())
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
    }

    pub(super) fn get(&self, path: &OsStr) -> Option<Arc<Asset>> {
        self.entries.lock().unwrap().by_path.get(path).cloned()
    }

    pub(super) fn insert(&self, path: OsString, asset: Asset) -> Arc<Asset> {
        let mut entries = self.entries.lock().unwrap();
        if let Some(existing) = entries.by_path.get(path.as_os_str()) {
            return existing.clone();
        }
        let evicted = if entries.fifo.len() == self.capacity {
            let oldest = entries.fifo.pop_front().unwrap();
            entries.by_path.remove(&oldest)
        } else {
            None
        };
        let path = Arc::<OsStr>::from(path);
        let asset = Arc::new(asset);
        entries.fifo.push_back(path.clone());
        entries.by_path.insert(path, asset.clone());
        drop(entries);
        // Closing an evicted file descriptor may involve filesystem work.
        drop(evicted);
        asset
    }
}

#[cfg(target_os = "linux")]
enum Content {
    Inline(Bytes),
    File(Arc<File>, u64),
}

#[cfg(target_os = "linux")]
pub(super) struct Asset {
    headers: axum::http::HeaderMap,
    modified: Option<SystemTime>,
    content: Content,
}

#[cfg(target_os = "linux")]
impl Asset {
    pub(super) fn load(
        mut file: File,
        len: u64,
        response: axum::http::response::Builder,
        modified: Option<SystemTime>,
    ) -> io::Result<Self> {
        let headers = response
            .body(())
            .map_err(io::Error::other)?
            .into_parts()
            .0
            .headers;
        let content = if len <= INLINE_LIMIT {
            Content::Inline(read_inline(&mut file, len)?)
        } else {
            Content::File(Arc::new(file), len)
        };
        Ok(Self {
            headers,
            modified,
            content,
        })
    }

    pub(super) fn response(
        &self,
        head: bool,
        conditions: &StaticConditions,
        socket: Arc<OwnedWriteHalf>,
    ) -> Response {
        let etag = self.headers.get(header::ETAG);
        if let Some(status) = static_precondition(conditions, self.modified, etag) {
            return static_condition_response(
                status,
                self.headers.get(header::LAST_MODIFIED).cloned(),
                etag.cloned(),
            );
        }
        let mut response = Response::new(Body::empty());
        *response.headers_mut() = self.headers.clone();
        if !head {
            match &self.content {
                Content::Inline(bytes) => *response.body_mut() = Body::from(bytes.clone()),
                Content::File(file, len) => {
                    response.extensions_mut().insert(super::direct_body(
                        socket,
                        file.clone(),
                        *len,
                    ));
                }
            }
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_capacity() {
        assert!(AssetCache::from_entries("0").unwrap().is_none());
        for bad in ["", "-1", "4097", "unknown"] {
            assert!(AssetCache::from_entries(bad).is_err());
        }
        assert!(AssetCache::from_entries("4096").is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn requires_a_fingerprint_and_clean_path() {
        for path in ["/app.8f31.js", "/a/b/main.min.DeadBEEF.css"] {
            assert!(AssetCache::eligible(path));
        }
        for path in [
            "/app.js",
            "/app.123.js",
            "/app.123z.js",
            "/app.1234.",
            "/.1234.js",
            "/dir//app.1234.js",
            "/./app.1234.js",
            "/dir/../app.1234.js",
            "/app.1234.js/",
        ] {
            assert!(!AssetCache::eligible(path), "{path}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hits_and_duplicate_fills_preserve_fifo_order() {
        let cache = AssetCache::from_entries("2").unwrap().unwrap();
        let asset = || Asset {
            headers: Default::default(),
            modified: None,
            content: Content::Inline(Bytes::new()),
        };
        let first = cache.insert("first".into(), asset());
        cache.insert("second".into(), asset());
        assert!(Arc::ptr_eq(
            &first,
            &cache.get(OsStr::new("first")).unwrap()
        ));
        assert!(Arc::ptr_eq(&first, &cache.insert("first".into(), asset())));
        cache.insert("third".into(), asset());
        assert!(cache.get(OsStr::new("first")).is_none());
        assert!(cache.get(OsStr::new("second")).is_some());
    }
}
