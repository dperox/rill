//! Exact-host document roots used after proxy route selection.
use crate::static_io::AssetCache;
use serde::Deserialize;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tower_http::services::ServeDir;

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(rename = "Site")]
#[serde(deny_unknown_fields)]
pub struct Definition {
    /// Exact hostname for this static site.
    host: String,
    /// Directory, relative to the configuration file or absolute.
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
        let root = path
            .canonicalize()
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
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
        let name = crate::routing::validate_hostname(&def.host, "site")?;
        if sites.iter().any(|(existing, _)| existing == &name) {
            return Err(io::Error::other(format!("duplicate site host: {name}")));
        }
        sites.push((name, Site::open(&base.join(def.root), chunk)?));
    }
    Ok(sites)
}
