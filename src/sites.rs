//! Exact-host document roots used after proxy route selection.
use crate::static_io::AssetCache;
use axum::http::uri::Authority;
use serde::Deserialize;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tower_http::services::ServeDir;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    host: String,
    root: PathBuf,
}

#[derive(Clone)]
pub struct Site {
    pub root: Arc<PathBuf>,
    pub files: ServeDir,
    pub cache: Option<Arc<AssetCache>>,
}

impl Site {
    pub fn open(path: &Path, chunk: usize) -> io::Result<Self> {
        let root = path.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::other(format!(
                "{} must be a directory",
                path.display()
            )));
        }
        Ok(Self {
            files: ServeDir::new(&root).with_buf_chunk_size(chunk),
            root: Arc::new(root),
            cache: AssetCache::from_env()?,
        })
    }
}

pub fn load(defs: Vec<Definition>, base: &Path, chunk: usize) -> io::Result<Vec<(String, Site)>> {
    let mut sites = Vec::new();
    for def in defs {
        let host = def.host.parse::<Authority>().map_err(io::Error::other)?;
        if host.host().is_empty() || host.host() != def.host || def.host.contains(['*', '@']) {
            return Err(io::Error::other(
                "site needs an exact hostname without a port",
            ));
        }
        let name = def.host.to_ascii_lowercase();
        if sites.iter().any(|(existing, _)| existing == &name) {
            return Err(io::Error::other("duplicate site host"));
        }
        sites.push((name, Site::open(&base.join(def.root), chunk)?));
    }
    Ok(sites)
}
