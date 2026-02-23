use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use minijinja::Environment;
use nanoss_metrics::MetricsCollector;
use nanoss_plugin_host::{PluginHost, PluginHostConfig};
use nanoss_query::{combine_fingerprints, content_hash, QueryDb, SourceFile};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use walkdir::WalkDir;

mod assets;
mod build;
mod cache;
mod data;
mod observability;
mod organization;
mod path;
mod ports;
mod render;
mod semantic;
mod seo;
mod utils;
mod validation;

const DEFAULT_PAGE_TEMPLATE: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{{ title }}</title>
  {% for alt in alternates %}
  <link rel="alternate" hreflang="{{ alt.locale }}" href="{{ alt.url }}" />
  {% endfor %}
</head>
<body>
  <main>
    {% if toc %}
    <nav aria-label="Table of contents">
      {{ toc | safe }}
    </nav>
    {% endif %}
    {{ content | safe }}
    {% if images.list|length > 0 %}
    <section>
      <h2>Image helpers</h2>
      <ul>
        {% for img in images.list %}
        <li><code>{{ img.source }}</code></li>
        {% endfor %}
      </ul>
    </section>
    {% endif %}
  </main>
</body>
</html>
"#;

#[allow(dead_code)]
static SYNTAX_SET: Lazy<SyntaxSet> = Lazy::new(SyntaxSet::load_defaults_newlines);
#[allow(dead_code)]
static THEME_SET: Lazy<ThemeSet> = Lazy::new(ThemeSet::load_defaults);
static HREF_HTTP_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new("href=[\"'](https?://[^\"']+)[\"']").expect("valid external link regex")
});
static CLASS_ATTR_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new("class=[\"']([^\"']+)[\"']").expect("valid html class attribute regex")
});
static ISLAND_TAG_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<island\s+name="([^"]+)"(?:\s+props='([^']*)')?\s*></island>"#)
        .expect("valid island regex")
});
static MD_LINK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)"#).expect("valid markdown link regex")
});
static HTML_ASSET_ATTR_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?:src|href)=["']([^"']+)["']"#).expect("valid html attr regex"));
const BUILD_CACHE_FILE: &str = ".nanoss-cache.json";
const BUILD_CACHE_SCHEMA_VERSION: u32 = 3;

#[derive(Clone)]
pub struct BuildConfig {
    pub content_dir: PathBuf,
    pub static_dir: PathBuf,
    pub output_dir: PathBuf,
    pub template_dir: Option<PathBuf>,
    pub theme_dir: Option<PathBuf>,
    pub plugin_paths: Vec<PathBuf>,
    pub plugin_init_config_json: String,
    pub plugin_timeout_ms: u64,
    pub plugin_memory_limit_mb: u64,
    pub check_external_links: bool,
    pub fail_on_broken_links: bool,
    pub js_backend: JsBackend,
    pub tailwind: Option<TailwindConfig>,
    pub enable_ai_index: bool,
    pub max_frontmatter_bytes: usize,
    pub max_file_bytes: u64,
    pub max_total_files: usize,
    pub command_timeout_secs: u64,
    pub base_path: String,
    pub site_domain: Option<String>,
    pub images: ImageBuildConfig,
    pub remote_data_sources: BTreeMap<String, RemoteDataSourceConfig>,
    pub i18n: I18nConfig,
    pub build_scope: BuildScope,
    pub metrics: Option<Arc<dyn MetricsCollector>>,
}

#[derive(Debug, Default)]
pub struct BuildReport {
    pub rendered_pages: usize,
    pub skipped_pages: usize,
    pub compiled_sass: usize,
    pub copied_assets: usize,
    pub checked_external_links: usize,
    pub broken_external_links: usize,
    pub processed_scripts: usize,
    pub compiled_tailwind: bool,
    pub island_pages: usize,
    pub ai_indexed_pages: usize,
    pub processed_images: usize,
}

#[derive(Debug, Clone, Default)]
pub enum BuildScope {
    #[default]
    Full,
    SinglePage {
        path: PathBuf,
    },
    AssetsOnly {
        paths: Vec<PathBuf>,
    },
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub plugins: ProjectPluginsConfig,
    #[serde(default)]
    pub theme: ProjectThemeConfig,
    #[serde(default)]
    pub build: ProjectBuildConfig,
    #[serde(default)]
    pub server: ProjectServerConfig,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProjectPluginsConfig {
    #[serde(default)]
    pub enabled: BTreeSet<String>,
    #[serde(default)]
    pub config: Option<toml::Value>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProjectThemeConfig {
    pub name: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProjectBuildConfig {
    pub base_path: Option<String>,
    pub site_domain: Option<String>,
    #[serde(default)]
    pub images: ProjectBuildImagesConfig,
    #[serde(default)]
    pub data_sources: BTreeMap<String, RemoteDataSourceConfig>,
    #[serde(default)]
    pub i18n: ProjectI18nConfig,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProjectBuildImagesConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub generate_webp: bool,
    #[serde(default)]
    pub generate_avif: bool,
    #[serde(default)]
    pub widths: Vec<u32>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProjectI18nConfig {
    #[serde(default)]
    pub locales: Vec<String>,
    pub default_locale: Option<String>,
    #[serde(default)]
    pub prefix_default_locale: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProjectServerConfig {
    pub mount_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImageBuildConfig {
    pub enabled: bool,
    pub generate_webp: bool,
    pub generate_avif: bool,
    pub widths: Vec<u32>,
}

impl Default for ImageBuildConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            generate_webp: false,
            generate_avif: false,
            widths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RemoteDataSourceConfig {
    pub url: String,
    #[serde(default = "default_remote_data_method")]
    pub method: String,
    #[serde(default)]
    pub fail_fast: bool,
}

fn default_remote_data_method() -> String {
    "GET".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default)]
pub struct I18nConfig {
    pub locales: Vec<String>,
    pub default_locale: Option<String>,
    pub prefix_default_locale: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum JsBackend {
    Passthrough,
    Esbuild,
}

#[derive(Debug, Clone)]
pub struct TailwindConfig {
    pub backend: TailwindBackend,
    pub input_css: PathBuf,
    pub output_css: PathBuf,
    pub binary: String,
    pub minify: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum TailwindBackend {
    Standalone,
    Rswind,
}

#[derive(Debug, Deserialize, Default)]
struct FrontMatter {
    title: Option<String>,
    slug: Option<String>,
    lang: Option<String>,
    date: Option<String>,
    tags: Option<Vec<String>>,
    categories: Option<Vec<String>>,
    template: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BuildCache {
    #[serde(default = "default_cache_schema_version")]
    schema_version: u32,
    #[serde(default)]
    pages: HashMap<String, CachePageRecord>,
    #[serde(default)]
    assets: HashMap<String, CacheAssetRecord>,
    #[serde(default)]
    images: HashMap<String, CacheImageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachePageRecord {
    hash: String,
    output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheAssetRecord {
    hash: String,
    output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheImageRecord {
    hash: String,
    output: String,
    width: Option<u32>,
    height: Option<u32>,
    variants: Vec<ImageVariantRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImageVariantRecord {
    format: String,
    width: Option<u32>,
    output: String,
}

impl Default for BuildCache {
    fn default() -> Self {
        Self {
            schema_version: BUILD_CACHE_SCHEMA_VERSION,
            pages: HashMap::new(),
            assets: HashMap::new(),
            images: HashMap::new(),
        }
    }
}

fn default_cache_schema_version() -> u32 {
    BUILD_CACHE_SCHEMA_VERSION
}

#[derive(Debug, Serialize)]
struct SemanticIndexDoc {
    path: String,
    title: String,
    embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
struct ContentEntry {
    title: String,
    url: String,
    date: Option<String>,
    tags: Vec<String>,
    categories: Vec<String>,
}

pub fn build_site(config: &BuildConfig) -> Result<BuildReport> {
    let build_started = Instant::now();
    let build_id = observability::next_build_id();
    observability::emit_event(
        "build.start",
        &build_id,
        serde_json::json!({
            "content_dir": config.content_dir.display().to_string(),
            "output_dir": config.output_dir.display().to_string(),
        }),
    );
    validation::validate_build_config(config)?;
    let base_path = path::normalize_base_path(&config.base_path);
    let site_domain = path::normalize_site_domain(config.site_domain.as_deref())?;
    fs::create_dir_all(&config.output_dir).with_context(|| {
        format!(
            "failed to create output directory {}",
            config.output_dir.display()
        )
    })?;

    let mut env = Environment::new();
    let templates =
        render::load_templates(config.template_dir.as_deref(), config.theme_dir.as_deref())?;
    for (name, source) in &templates {
        env.add_template(name, source)
            .with_context(|| format!("failed to register template {name}"))?;
    }

    let mut plugin_host = PluginHost::new(PluginHostConfig {
        plugin_paths: config.plugin_paths.clone(),
        timeout_ms: config.plugin_timeout_ms,
        memory_limit_mb: config.plugin_memory_limit_mb,
    })?;
    plugin_host.init(&config.plugin_init_config_json)?;

    let mut report = BuildReport::default();
    let query_db = QueryDb::default();
    let data_context = data::load_data_context(
        &config.content_dir,
        &config.output_dir,
        &config.remote_data_sources,
    )?;
    let cache_path = config.output_dir.join(BUILD_CACHE_FILE);
    let mut build_cache = cache::load_build_cache(&cache_path)?;
    let template_hash = utils::compute_template_dependency_hash(
        &query_db,
        config.template_dir.as_deref(),
        config.theme_dir.as_deref(),
    )?;
    if let Some(tailwind) = &config.tailwind {
        assets::run_tailwind(tailwind, &config.content_dir, config.command_timeout_secs)?;
        report.compiled_tailwind = true;
    }
    render::copy_site_static_assets(&config.static_dir, &config.output_dir)?;
    render::copy_theme_static_assets(config.theme_dir.as_deref(), &config.output_dir)?;

    let mut islands_runtime_written = false;
    let mut file_count = 0usize;
    let scope_paths = scope_paths_set(&config.build_scope);
    for entry in WalkDir::new(&config.content_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if !scope_includes_entry(&config.build_scope, &scope_paths, entry.path()) {
            continue;
        }
        file_count += 1;
        if file_count > config.max_total_files {
            bail!(
                "file count exceeds configured limit: {} > {}",
                file_count,
                config.max_total_files
            );
        }
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to read file metadata {}", entry.path().display()))?;
        if metadata.len() > config.max_file_bytes {
            bail!(
                "file exceeds configured size limit ({} bytes): {}",
                config.max_file_bytes,
                entry.path().display()
            );
        }
        match entry.path().extension().and_then(OsStr::to_str) {
            Some("md") => {
                let raw = fs::read_to_string(entry.path()).with_context(|| {
                    format!("failed to read markdown file {}", entry.path().display())
                })?;
                validation::validate_frontmatter_size(&raw, config.max_frontmatter_bytes)
                    .with_context(|| {
                        format!("frontmatter too large in {}", entry.path().display())
                    })?;
                let current_hash = utils::compute_page_build_hash(
                    &query_db,
                    entry.path(),
                    &raw,
                    &template_hash,
                    &config.content_dir,
                )?;
                let cache_key = entry.path().display().to_string();
                if let Some(record) = build_cache.pages.get(&cache_key) {
                    let cached_output = PathBuf::from(&record.output);
                    if record.hash == current_hash && cached_output.exists() {
                        report.skipped_pages += 1;
                        continue;
                    }
                }

                let rendered = render::render_markdown_file(
                    entry.path(),
                    &env,
                    &templates,
                    &mut plugin_host,
                    &data_context,
                    config,
                    &base_path,
                )?;
                let target = path::output_path_for(
                    entry.path(),
                    &config.content_dir,
                    &config.output_dir,
                    rendered.slug.as_deref(),
                    rendered.lang.as_deref(),
                    &config.i18n,
                )?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create parent directory {}", parent.display())
                    })?;
                }
                let (island_html, has_islands) = render::compile_islands(&rendered.html);
                if has_islands && !islands_runtime_written {
                    render::write_islands_runtime(&config.output_dir)?;
                    islands_runtime_written = true;
                }
                if has_islands {
                    report.island_pages += 1;
                }
                fs::write(&target, island_html).with_context(|| {
                    format!("failed to write rendered file {}", target.display())
                })?;
                build_cache.pages.insert(
                    cache_key,
                    CachePageRecord {
                        hash: current_hash,
                        output: target.display().to_string(),
                    },
                );
                report.rendered_pages += 1;
            }
            Some(ext) if assets::is_image_extension(ext) => {
                let hash = utils::hashed_file_content(entry.path());
                let cache_key = entry.path().display().to_string();
                let target = path::asset_output_path(
                    entry.path(),
                    &config.content_dir,
                    &config.output_dir,
                    None,
                )?;
                if let Some(record) = build_cache.assets.get(&cache_key) {
                    if record.hash == hash && PathBuf::from(&record.output).exists() {
                        report.copied_assets += 1;
                        continue;
                    }
                }
                let image_record = assets::process_image_asset(
                    entry.path(),
                    &config.content_dir,
                    &config.output_dir,
                    &config.images,
                )?;
                build_cache.assets.insert(
                    cache_key.clone(),
                    CacheAssetRecord {
                        hash: hash.clone(),
                        output: target.display().to_string(),
                    },
                );
                build_cache.images.insert(cache_key, image_record);
                report.copied_assets += 1;
                report.processed_images += 1;
            }
            Some("scss") | Some("sass") => {
                let hash = utils::hashed_file_content(entry.path());
                let cache_key = entry.path().display().to_string();
                let target = path::asset_output_path(
                    entry.path(),
                    &config.content_dir,
                    &config.output_dir,
                    Some("css"),
                )?;
                if let Some(record) = build_cache.assets.get(&cache_key) {
                    if record.hash == hash && PathBuf::from(&record.output).exists() {
                        continue;
                    }
                }
                assets::compile_sass_file(
                    entry.path(),
                    &config.content_dir,
                    &config.output_dir,
                    config.command_timeout_secs,
                )?;
                build_cache.assets.insert(
                    cache_key,
                    CacheAssetRecord {
                        hash,
                        output: target.display().to_string(),
                    },
                );
                report.compiled_sass += 1;
            }
            Some("css") => {
                let hash = utils::hashed_file_content(entry.path());
                let cache_key = entry.path().display().to_string();
                let target = path::asset_output_path(
                    entry.path(),
                    &config.content_dir,
                    &config.output_dir,
                    None,
                )?;
                if let Some(record) = build_cache.assets.get(&cache_key) {
                    if record.hash == hash && PathBuf::from(&record.output).exists() {
                        continue;
                    }
                }
                assets::process_css_asset(entry.path(), &config.content_dir, &config.output_dir)?;
                build_cache.assets.insert(
                    cache_key,
                    CacheAssetRecord {
                        hash,
                        output: target.display().to_string(),
                    },
                );
                report.copied_assets += 1;
            }
            Some("js") | Some("mjs") | Some("cjs") | Some("ts") => {
                let hash = utils::hashed_file_content(entry.path());
                let cache_key = entry.path().display().to_string();
                let target = path::asset_output_path(
                    entry.path(),
                    &config.content_dir,
                    &config.output_dir,
                    if entry.path().extension().and_then(OsStr::to_str) == Some("ts") {
                        Some("js")
                    } else {
                        None
                    },
                )?;
                if let Some(record) = build_cache.assets.get(&cache_key) {
                    if record.hash == hash && PathBuf::from(&record.output).exists() {
                        continue;
                    }
                }
                assets::process_script_asset(
                    entry.path(),
                    &config.content_dir,
                    &config.output_dir,
                    config.js_backend,
                    config.command_timeout_secs,
                )?;
                build_cache.assets.insert(
                    cache_key,
                    CacheAssetRecord {
                        hash,
                        output: target.display().to_string(),
                    },
                );
                report.processed_scripts += 1;
            }
            _ => {
                let hash = utils::hashed_file_content(entry.path());
                let cache_key = entry.path().display().to_string();
                let target = path::asset_output_path(
                    entry.path(),
                    &config.content_dir,
                    &config.output_dir,
                    None,
                )?;
                if let Some(record) = build_cache.assets.get(&cache_key) {
                    if record.hash == hash && PathBuf::from(&record.output).exists() {
                        continue;
                    }
                }
                assets::copy_asset_file(entry.path(), &config.content_dir, &config.output_dir)?;
                build_cache.assets.insert(
                    cache_key,
                    CacheAssetRecord {
                        hash,
                        output: target.display().to_string(),
                    },
                );
                report.copied_assets += 1;
            }
        }
    }

    if config.check_external_links {
        let link_report = utils::check_external_links(&config.output_dir)?;
        report.checked_external_links = link_report.checked;
        report.broken_external_links = link_report.broken;
        if config.fail_on_broken_links && report.broken_external_links > 0 {
            bail!(
                "external link check failed: {} broken links",
                report.broken_external_links
            );
        }
    }

    if config.enable_ai_index {
        report.ai_indexed_pages =
            semantic::build_semantic_index(&config.content_dir, &config.output_dir)?;
    }

    if !matches!(config.build_scope, BuildScope::AssetsOnly { .. }) {
        let entries = organization::collect_content_entries(
            &config.content_dir,
            &config.output_dir,
            &base_path,
            &config.i18n,
        )?;
        organization::generate_content_organization_outputs(
            &entries,
            &config.output_dir,
            &env,
            &data_context,
            &base_path,
        )?;
        seo::generate_sitemap_and_feed(&entries, &config.output_dir, site_domain.as_deref())?;
    }

    plugin_host.shutdown()?;
    cache::save_build_cache(&cache_path, &build_cache)?;
    if let Some(metrics) = &config.metrics {
        metrics.record_histogram(
            nanoss_metrics::metric_names::BUILD_DURATION_MS,
            build_started.elapsed().as_millis() as f64,
            &[("status", "ok")],
        );
        metrics.increment_counter(
            nanoss_metrics::metric_names::PAGES_RENDERED_TOTAL,
            &[("status", "ok")],
        );
    }
    observability::emit_event(
        "build.finish",
        &build_id,
        serde_json::json!({
            "duration_ms": build_started.elapsed().as_millis(),
            "rendered_pages": report.rendered_pages,
            "processed_images": report.processed_images,
            "copied_assets": report.copied_assets,
        }),
    );

    Ok(report)
}

fn scope_paths_set(scope: &BuildScope) -> HashSet<String> {
    build::scope_paths_set(scope)
}

fn scope_includes_entry(scope: &BuildScope, scope_paths: &HashSet<String>, path: &Path) -> bool {
    build::scope_includes_entry(scope, scope_paths, path)
}

struct RenderedPage {
    slug: Option<String>,
    lang: Option<String>,
    html: String,
}

#[derive(Debug)]
struct TocItem {
    level: u8,
    id: String,
    text: String,
}

#[derive(Default)]
struct LinkCheckReport {
    checked: usize,
    broken: usize,
}

#[cfg(test)]
mod tests;
