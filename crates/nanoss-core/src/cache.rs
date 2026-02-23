use std::path::Path;

use anyhow::{Context, Result};

use crate::ports::{FileSystemPort, StdFileSystemPort};
use crate::{BuildCache, BUILD_CACHE_SCHEMA_VERSION};

pub(crate) fn load_build_cache(path: &Path) -> Result<BuildCache> {
    load_build_cache_with_fs(path, &StdFileSystemPort)
}

pub(crate) fn load_build_cache_with_fs(path: &Path, fs_port: &dyn FileSystemPort) -> Result<BuildCache> {
    if !fs_port.exists(path) {
        return Ok(BuildCache::default());
    }
    let raw = fs_port.read_to_string(path)?;
    match serde_json::from_str(&raw) {
        Ok(cache) => {
            let cache: BuildCache = cache;
            if cache.schema_version != BUILD_CACHE_SCHEMA_VERSION {
                eprintln!(
                    "warning: cache schema mismatch {} != {}, resetting cache",
                    cache.schema_version, BUILD_CACHE_SCHEMA_VERSION
                );
                Ok(BuildCache::default())
            } else {
                Ok(cache)
            }
        }
        Err(err) => {
            eprintln!(
                "warning: invalid build cache {}, resetting cache: {}",
                path.display(),
                err
            );
            Ok(BuildCache::default())
        }
    }
}

pub(crate) fn save_build_cache(path: &Path, cache: &BuildCache) -> Result<()> {
    save_build_cache_with_fs(path, cache, &StdFileSystemPort)
}

pub(crate) fn save_build_cache_with_fs(
    path: &Path,
    cache: &BuildCache,
    fs_port: &dyn FileSystemPort,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_port.create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cache).context("failed to serialize build cache")?;
    fs_port.write_string(path, &json)?;
    Ok(())
}
