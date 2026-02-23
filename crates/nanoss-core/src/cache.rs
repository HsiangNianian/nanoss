use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::{BuildCache, BUILD_CACHE_SCHEMA_VERSION};

pub(crate) fn load_build_cache(path: &Path) -> Result<BuildCache> {
    if !path.exists() {
        return Ok(BuildCache::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read build cache {}", path.display()))?;
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create build cache parent {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(cache).context("failed to serialize build cache")?;
    fs::write(path, json).with_context(|| format!("failed to write build cache {}", path.display()))?;
    Ok(())
}
