use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use walkdir::WalkDir;

use crate::ports::{HttpPort, StdHttpPort};
use crate::RemoteDataSourceConfig;

pub(crate) fn load_data_context(
    content_dir: &Path,
    output_dir: &Path,
    remote_sources: &BTreeMap<String, RemoteDataSourceConfig>,
) -> Result<serde_json::Value> {
    let data_dir = content_dir.join("data");
    let mut root = serde_json::Map::new();
    if data_dir.exists() {
        for entry in WalkDir::new(&data_dir).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&data_dir)
                .with_context(|| format!("failed to relativize data file {}", entry.path().display()))?;
            let key = rel
                .with_extension("")
                .to_string_lossy()
                .replace(['/', '\\'], ".");
            let raw = fs::read_to_string(entry.path())
                .with_context(|| format!("failed to read data file {}", entry.path().display()))?;
            let value = match entry.path().extension().and_then(OsStr::to_str) {
                Some("json") => serde_json::from_str(&raw)
                    .with_context(|| format!("failed to parse json data {}", entry.path().display()))?,
                Some("yaml") | Some("yml") => serde_yaml::from_str::<serde_json::Value>(&raw)
                    .with_context(|| format!("failed to parse yaml data {}", entry.path().display()))?,
                Some("toml") => {
                    let toml_value: toml::Value = toml::from_str(&raw)
                        .with_context(|| format!("failed to parse toml data {}", entry.path().display()))?;
                    serde_json::to_value(toml_value).context("failed to convert toml data to json value")?
                }
                _ => continue,
            };
            root.insert(key, value);
        }
    }
    let remote = fetch_remote_data_sources(output_dir, remote_sources)?;
    for (key, value) in remote {
        root.insert(key, value);
    }
    Ok(serde_json::Value::Object(root))
}

pub(crate) fn fetch_remote_data_sources(
    output_dir: &Path,
    remote_sources: &BTreeMap<String, RemoteDataSourceConfig>,
) -> Result<BTreeMap<String, serde_json::Value>> {
    fetch_remote_data_sources_with_http(output_dir, remote_sources, &StdHttpPort)
}

pub(crate) fn fetch_remote_data_sources_with_http(
    output_dir: &Path,
    remote_sources: &BTreeMap<String, RemoteDataSourceConfig>,
    http: &dyn HttpPort,
) -> Result<BTreeMap<String, serde_json::Value>> {
    if remote_sources.is_empty() {
        return Ok(BTreeMap::new());
    }
    let cache_dir = output_dir.join(".nanoss-cache").join("remote-data");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("failed to create remote data cache dir {}", cache_dir.display()))?;

    let mut resolved = BTreeMap::new();
    for (key, source) in remote_sources {
        let cache_file = cache_dir.join(format!("{key}.json"));
        let method = source.method.to_uppercase();
        let mut loaded = None;
        if method == "GET" {
            if let Ok(value) = http.get_json(&source.url, 15, "nanoss-remote-data/0.1") {
                fs::write(&cache_file, serde_json::to_vec_pretty(&value)?)
                    .with_context(|| format!("failed to persist remote data cache {}", cache_file.display()))?;
                loaded = Some(value);
            }
        }
        if loaded.is_none() && cache_file.exists() {
            let cached = fs::read_to_string(&cache_file)
                .with_context(|| format!("failed to read cached remote data {}", cache_file.display()))?;
            loaded = Some(
                serde_json::from_str(&cached)
                    .with_context(|| format!("failed to parse cached remote data {}", cache_file.display()))?,
            );
        }
        if let Some(value) = loaded {
            resolved.insert(key.clone(), value);
        } else if source.fail_fast {
            bail!("remote data source '{}' failed and no cache is available", key);
        }
    }
    Ok(resolved)
}
