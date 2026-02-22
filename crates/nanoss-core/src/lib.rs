use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use lightningcss::stylesheet::{ParserFlags, ParserOptions, PrinterOptions, StyleSheet};
use minijinja::{context, Environment};
use nanoss_plugin_host::{PluginHost, PluginHostConfig};
use nanoss_query::{combine_fingerprints, content_hash, QueryDb, SourceFile};
use once_cell::sync::Lazy;
use pulldown_cmark::{html, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
use rayon::prelude::*;
use rswind::create_processor;
use reqwest::blocking::Client;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;
use walkdir::WalkDir;

const DEFAULT_PAGE_TEMPLATE: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{{ title }}</title>
</head>
<body>
  <main>
    {% if toc %}
    <nav aria-label="Table of contents">
      {{ toc | safe }}
    </nav>
    {% endif %}
    {{ content | safe }}
  </main>
</body>
</html>
"#;

static SYNTAX_SET: Lazy<SyntaxSet> = Lazy::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: Lazy<ThemeSet> = Lazy::new(ThemeSet::load_defaults);
static HREF_HTTP_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new("href=[\"'](https?://[^\"']+)[\"']").expect("valid external link regex"));
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
const BUILD_CACHE_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone)]
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
}

#[derive(Debug, Serialize, Deserialize)]
struct PageIr {
    title: String,
    content_html: String,
    toc_html: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BuildCache {
    #[serde(default = "default_cache_schema_version")]
    schema_version: u32,
    #[serde(default)]
    pages: HashMap<String, CachePageRecord>,
    #[serde(default)]
    assets: HashMap<String, CacheAssetRecord>,
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

impl Default for BuildCache {
    fn default() -> Self {
        Self {
            schema_version: BUILD_CACHE_SCHEMA_VERSION,
            pages: HashMap::new(),
            assets: HashMap::new(),
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
    validate_build_config(config)?;
    let base_path = normalize_base_path(&config.base_path);
    let site_domain = normalize_site_domain(config.site_domain.as_deref())?;
    fs::create_dir_all(&config.output_dir)
        .with_context(|| format!("failed to create output directory {}", config.output_dir.display()))?;

    let mut env = Environment::new();
    let page_template = load_page_template(config.template_dir.as_deref(), config.theme_dir.as_deref())?;
    env.add_template("page.html", &page_template)
        .context("failed to register template")?;

    let mut plugin_host = PluginHost::new(PluginHostConfig {
        plugin_paths: config.plugin_paths.clone(),
        timeout_ms: config.plugin_timeout_ms,
        memory_limit_mb: config.plugin_memory_limit_mb,
    })?;
    plugin_host.init(&config.plugin_init_config_json)?;

    let mut report = BuildReport::default();
    let query_db = QueryDb::default();
    let data_context = load_data_context(&config.content_dir)?;
    let cache_path = config.output_dir.join(BUILD_CACHE_FILE);
    let mut build_cache = load_build_cache(&cache_path)?;
    let template_hash = compute_template_dependency_hash(
        &query_db,
        config.template_dir.as_deref(),
        config.theme_dir.as_deref(),
    )?;
    if let Some(tailwind) = &config.tailwind {
        run_tailwind(tailwind, &config.content_dir, config.command_timeout_secs)?;
        report.compiled_tailwind = true;
    }
    copy_site_static_assets(&config.static_dir, &config.output_dir)?;
    copy_theme_static_assets(config.theme_dir.as_deref(), &config.output_dir)?;

    let mut islands_runtime_written = false;
    let mut file_count = 0usize;
    for entry in WalkDir::new(&config.content_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
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
                let raw = fs::read_to_string(entry.path())
                    .with_context(|| format!("failed to read markdown file {}", entry.path().display()))?;
                validate_frontmatter_size(&raw, config.max_frontmatter_bytes)
                    .with_context(|| format!("frontmatter too large in {}", entry.path().display()))?;
                let current_hash = compute_page_build_hash(
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

                let rendered = render_markdown_file(
                    entry.path(),
                    &env,
                    &mut plugin_host,
                    &data_context,
                    &base_path,
                )?;
                let target =
                    output_path_for(
                        entry.path(),
                        &config.content_dir,
                        &config.output_dir,
                        rendered.slug.as_deref(),
                        rendered.lang.as_deref(),
                    )?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
                }
                let (island_html, has_islands) = compile_islands(&rendered.html);
                if has_islands && !islands_runtime_written {
                    write_islands_runtime(&config.output_dir)?;
                    islands_runtime_written = true;
                }
                if has_islands {
                    report.island_pages += 1;
                }
                fs::write(&target, island_html)
                    .with_context(|| format!("failed to write rendered file {}", target.display()))?;
                build_cache.pages.insert(
                    cache_key,
                    CachePageRecord {
                        hash: current_hash,
                        output: target.display().to_string(),
                    },
                );
                report.rendered_pages += 1;
            }
            Some("scss") | Some("sass") => {
                let hash = hashed_file_content(entry.path());
                let cache_key = entry.path().display().to_string();
                let target = asset_output_path(entry.path(), &config.content_dir, &config.output_dir, Some("css"))?;
                if let Some(record) = build_cache.assets.get(&cache_key) {
                    if record.hash == hash && PathBuf::from(&record.output).exists() {
                        continue;
                    }
                }
                compile_sass_file(
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
                let hash = hashed_file_content(entry.path());
                let cache_key = entry.path().display().to_string();
                let target = asset_output_path(entry.path(), &config.content_dir, &config.output_dir, None)?;
                if let Some(record) = build_cache.assets.get(&cache_key) {
                    if record.hash == hash && PathBuf::from(&record.output).exists() {
                        continue;
                    }
                }
                process_css_asset(entry.path(), &config.content_dir, &config.output_dir)?;
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
                let hash = hashed_file_content(entry.path());
                let cache_key = entry.path().display().to_string();
                let target = asset_output_path(
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
                process_script_asset(
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
                let hash = hashed_file_content(entry.path());
                let cache_key = entry.path().display().to_string();
                let target = asset_output_path(entry.path(), &config.content_dir, &config.output_dir, None)?;
                if let Some(record) = build_cache.assets.get(&cache_key) {
                    if record.hash == hash && PathBuf::from(&record.output).exists() {
                        continue;
                    }
                }
                copy_asset_file(entry.path(), &config.content_dir, &config.output_dir)?;
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
        let link_report = check_external_links(&config.output_dir)?;
        report.checked_external_links = link_report.checked;
        report.broken_external_links = link_report.broken;
        if config.fail_on_broken_links && report.broken_external_links > 0 {
            bail!("external link check failed: {} broken links", report.broken_external_links);
        }
    }

    if config.enable_ai_index {
        report.ai_indexed_pages = build_semantic_index(&config.content_dir, &config.output_dir)?;
    }

    let entries = collect_content_entries(&config.content_dir, &config.output_dir, &base_path)?;
    generate_content_organization_outputs(&entries, &config.output_dir, &env, &data_context, &base_path)?;
    generate_sitemap_and_feed(&entries, &config.output_dir, site_domain.as_deref())?;

    plugin_host.shutdown()?;
    save_build_cache(&cache_path, &build_cache)?;

    Ok(report)
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

fn render_markdown_file(
    path: &Path,
    env: &Environment<'_>,
    plugin_host: &mut PluginHost,
    data_context: &serde_json::Value,
    base_path: &str,
) -> Result<RenderedPage> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read markdown file {}", path.display()))?;
    let transformed_raw = plugin_host
        .transform_markdown(&path.display().to_string(), raw)
        .with_context(|| format!("plugin transform_markdown failed for {}", path.display()))?;
    let (frontmatter, markdown) = parse_frontmatter(&transformed_raw).with_context(|| {
        format!("failed to parse frontmatter for {}", path.display())
    })?;

    let (html_content, toc_items) = markdown_to_html(markdown);
    let title = frontmatter
        .title
        .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(ToOwned::to_owned))
        .unwrap_or_else(|| "Untitled".to_string());
    let ir = PageIr {
        title,
        content_html: html_content,
        toc_html: build_toc_html(&toc_items),
    };
    let ir_json = serde_json::to_string(&ir).context("failed to serialize page ir")?;
    let transformed_ir_json = plugin_host
        .on_page_ir(&path.display().to_string(), ir_json)
        .with_context(|| format!("plugin on_page_ir failed for {}", path.display()))?;
    let transformed_ir: PageIr =
        serde_json::from_str(&transformed_ir_json).context("plugin returned invalid page ir json")?;

    let tmpl = env.get_template("page.html").context("missing page.html template")?;
    let rendered_html = tmpl
        .render(context! {
            title => transformed_ir.title,
            content => transformed_ir.content_html,
            toc => transformed_ir.toc_html,
            data => data_context,
            base_path => base_path,
            base_href_prefix => base_href_prefix(base_path)
        })
        .context("failed to render page template")?;
    let html = plugin_host
        .on_post_render(&path.display().to_string(), rendered_html)
        .with_context(|| format!("plugin on_post_render failed for {}", path.display()))?;
    let html = rewrite_html_absolute_links_with_base_path(&html, base_path);

    Ok(RenderedPage {
        slug: frontmatter.slug,
        lang: frontmatter.lang,
        html,
    })
}

fn load_page_template(template_dir: Option<&Path>, theme_dir: Option<&Path>) -> Result<String> {
    if let Some(dir) = template_dir {
        let candidate = dir.join("page.html");
        if candidate.exists() {
            return fs::read_to_string(&candidate)
                .with_context(|| format!("failed to read template {}", candidate.display()));
        }
    }
    if let Some(theme) = theme_dir {
        let candidate = theme.join("templates").join("page.html");
        if candidate.exists() {
            return fs::read_to_string(&candidate)
                .with_context(|| format!("failed to read theme template {}", candidate.display()));
        }
    }
    Ok(DEFAULT_PAGE_TEMPLATE.to_string())
}

fn copy_theme_static_assets(theme_dir: Option<&Path>, output_dir: &Path) -> Result<()> {
    let Some(theme) = theme_dir else {
        return Ok(());
    };
    let static_root = theme.join("static");
    if !static_root.exists() {
        return Ok(());
    }

    for entry in WalkDir::new(&static_root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&static_root)
            .with_context(|| format!("failed to relativize theme static {}", entry.path().display()))?;
        let target = output_dir.join(rel);
        if target.exists() {
            // Site output takes precedence over theme static assets.
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create theme static parent {}", parent.display()))?;
        }
        fs::copy(entry.path(), &target)
            .with_context(|| format!("failed to copy theme static {} -> {}", entry.path().display(), target.display()))?;
    }
    Ok(())
}

fn copy_site_static_assets(static_dir: &Path, output_dir: &Path) -> Result<()> {
    if !static_dir.exists() {
        return Ok(());
    }

    for entry in WalkDir::new(static_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(static_dir)
            .with_context(|| format!("failed to relativize static asset {}", entry.path().display()))?;
        let target = output_dir.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create static asset parent {}", parent.display()))?;
        }
        fs::copy(entry.path(), &target)
            .with_context(|| format!("failed to copy static asset {} -> {}", entry.path().display(), target.display()))?;
    }

    Ok(())
}

fn parse_frontmatter(input: &str) -> Result<(FrontMatter, &str)> {
    if !input.starts_with("---\n") {
        return Ok((FrontMatter::default(), input));
    }

    let remainder = &input[4..];
    if let Some(end) = remainder.find("\n---\n") {
        let yaml = &remainder[..end];
        let body = &remainder[end + 5..];
        let frontmatter: FrontMatter = serde_yaml::from_str(yaml).context("invalid YAML frontmatter")?;
        return Ok((frontmatter, body));
    }

    Ok((FrontMatter::default(), input))
}

fn markdown_to_html(markdown: &str) -> (String, Vec<TocItem>) {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    let parser = Parser::new_ext(markdown, options);

    let mut events = Vec::new();
    let mut toc = Vec::new();
    let mut heading: Option<(HeadingLevel, Vec<Event<'_>>)> = None;
    let mut code_block: Option<CodeBlockState> = None;

    for event in parser {
        if let Some(block) = code_block.as_mut() {
            match event {
                Event::Text(text) | Event::Code(text) => block.content.push_str(&text),
                Event::SoftBreak | Event::HardBreak => block.content.push('\n'),
                Event::End(TagEnd::CodeBlock) => {
                    if let Some(finished) = code_block.take() {
                        let highlighted = highlight_code_block(&finished.language, &finished.content);
                        events.push(Event::Html(CowStr::Boxed(highlighted.into_boxed_str())));
                    }
                }
                _ => {}
            }
            continue;
        }

        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some((level, Vec::new()));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                code_block = Some(CodeBlockState {
                    language: language_from_code_block_kind(kind),
                    content: String::new(),
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, heading_events)) = heading.take() {
                    let text = heading_text(&heading_events);
                    let id = slugify(&text);
                    events.push(Event::Start(Tag::Heading {
                        level,
                        id: Some(id.clone().into()),
                        classes: Vec::new(),
                        attrs: Vec::new(),
                    }));
                    events.extend(heading_events);
                    events.push(Event::End(TagEnd::Heading(level)));

                    if !text.is_empty() {
                        toc.push(TocItem {
                            level: heading_level_to_u8(level),
                            id,
                            text,
                        });
                    }
                }
            }
            other => {
                if let Some((_, heading_events)) = heading.as_mut() {
                    heading_events.push(other);
                } else {
                    events.push(other);
                }
            }
        }
    }

    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    (out, toc)
}

fn output_path_for(
    source: &Path,
    content_root: &Path,
    output_root: &Path,
    slug: Option<&str>,
    lang: Option<&str>,
) -> Result<PathBuf> {
    if let Some(slug) = slug {
        validate_route_segment(slug, "slug")?;
        let mut base = output_root.to_path_buf();
        if let Some(lang) = lang {
            validate_route_segment(lang, "lang")?;
            base = base.join(lang);
        }
        let candidate = if slug == "index" {
            base.join("index.html")
        } else {
            base.join(slug).join("index.html")
        };
        ensure_inside_output(output_root, &candidate)?;
        return Ok(candidate);
    }

    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let mut target = output_root.to_path_buf();
    if let Some(lang) = lang {
        validate_route_segment(lang, "lang")?;
        target = target.join(lang);
    }
    target = target.join(rel);
    target.set_extension("html");
    ensure_inside_output(output_root, &target)?;
    Ok(target)
}

fn compile_sass_file(source: &Path, content_root: &Path, output_root: &Path, _timeout_secs: u64) -> Result<()> {
    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let mut target = output_root.join(rel);
    target.set_extension("css");

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }

    let css = grass::from_path(source, &grass::Options::default())
        .with_context(|| format!("failed to compile Sass file {}", source.display()))?;
    let optimized = optimize_css(source, &css)?;
    fs::write(&target, optimized).with_context(|| format!("failed to write Sass output {}", target.display()))?;
    Ok(())
}

fn process_css_asset(source: &Path, content_root: &Path, output_root: &Path) -> Result<()> {
    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let target = output_root.join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }
    let raw_css = fs::read_to_string(source)
        .with_context(|| format!("failed to read CSS asset {}", source.display()))?;
    let optimized = optimize_css(source, &raw_css)?;
    fs::write(&target, optimized)
        .with_context(|| format!("failed to write CSS asset {}", target.display()))?;
    Ok(())
}

fn copy_asset_file(source: &Path, content_root: &Path, output_root: &Path) -> Result<()> {
    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let target = output_root.join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }
    fs::copy(source, &target)
        .with_context(|| format!("failed to copy asset {} -> {}", source.display(), target.display()))?;
    Ok(())
}

fn process_script_asset(
    source: &Path,
    content_root: &Path,
    output_root: &Path,
    backend: JsBackend,
    timeout_secs: u64,
) -> Result<()> {
    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let mut target = output_root.join(rel);
    if source.extension().and_then(OsStr::to_str) == Some("ts") {
        target.set_extension("js");
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }

    match backend {
        JsBackend::Passthrough => {
            fs::copy(source, &target)
                .with_context(|| format!("failed to copy script {} -> {}", source.display(), target.display()))?;
        }
        JsBackend::Esbuild => {
            ensure_binary_name_safe("esbuild")?;
            let mut cmd = Command::new("esbuild");
            let mut child = cmd
                .arg(source)
                .arg("--bundle")
                .arg("--minify")
                .arg("--outfile")
                .arg(&target)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| format!("failed to execute esbuild for {}", source.display()))?;
            let status = wait_child_with_timeout(&mut child, timeout_secs)
                .with_context(|| format!("esbuild timeout for {}", source.display()))?;
            if !status.success() {
                bail!("esbuild failed for {}", source.display());
            }
        }
    }
    Ok(())
}

fn run_tailwind(config: &TailwindConfig, content_dir: &Path, timeout_secs: u64) -> Result<()> {
    match config.backend {
        TailwindBackend::Standalone => run_tailwind_standalone(config, timeout_secs),
        TailwindBackend::Rswind => run_tailwind_rswind(config, content_dir),
    }
}

fn run_tailwind_standalone(config: &TailwindConfig, timeout_secs: u64) -> Result<()> {
    ensure_binary_name_safe(&config.binary)?;
    if let Some(parent) = config.output_css.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create tailwind output parent {}", parent.display()))?;
    }
    let mut cmd = Command::new(&config.binary);
    cmd.arg("-i")
        .arg(&config.input_css)
        .arg("-o")
        .arg(&config.output_css);
    if config.minify {
        cmd.arg("--minify");
    }
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute {}", config.binary))?;
    let status = wait_child_with_timeout(&mut child, timeout_secs)
        .with_context(|| format!("{} timed out", config.binary))?;
    if !status.success() {
        bail!("tailwind standalone compile failed");
    }
    Ok(())
}

fn run_tailwind_rswind(config: &TailwindConfig, content_dir: &Path) -> Result<()> {
    if let Some(parent) = config.output_css.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create tailwind output parent {}", parent.display()))?;
    }

    let mut classes = Vec::new();
    for entry in WalkDir::new(content_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let raw = fs::read_to_string(entry.path()).unwrap_or_default();
        for cap in CLASS_ATTR_RE.captures_iter(&raw) {
            if let Some(group) = cap.get(1) {
                classes.extend(group.as_str().split_whitespace().map(ToOwned::to_owned));
            }
        }
    }

    let mut processor = create_processor();
    let mut css = processor.run_with(classes.iter());
    if config.minify {
        css = optimize_css(&config.output_css, &css)?;
    }
    fs::write(&config.output_css, css)
        .with_context(|| format!("failed to write rswind output {}", config.output_css.display()))?;
    Ok(())
}

fn heading_text(events: &[Event<'_>]) -> String {
    let mut text = String::new();
    for event in events {
        match event {
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            _ => {}
        }
    }
    text.trim().to_string()
}

fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_dash = false;
    for ch in value.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn build_toc_html(items: &[TocItem]) -> String {
    if items.is_empty() {
        return String::new();
    }

    let mut out = String::from("<ul>");
    for item in items {
        out.push_str(&format!(
            "<li class=\"toc-level-{}\"><a href=\"#{}\">{}</a></li>",
            item.level, item.id, item.text
        ));
    }
    out.push_str("</ul>");
    out
}

struct CodeBlockState {
    language: String,
    content: String,
}

fn language_from_code_block_kind(kind: CodeBlockKind<'_>) -> String {
    match kind {
        CodeBlockKind::Indented => String::new(),
        CodeBlockKind::Fenced(lang) => lang.to_string(),
    }
}

fn highlight_code_block(language: &str, code: &str) -> String {
    let syntax = if language.is_empty() {
        SYNTAX_SET.find_syntax_plain_text()
    } else {
        SYNTAX_SET
            .find_syntax_by_token(language)
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
    };

    if let Some(theme) = THEME_SET
        .themes
        .get("base16-ocean.dark")
        .or_else(|| THEME_SET.themes.values().next())
    {
        if let Ok(html) = highlighted_html_for_string(code, &SYNTAX_SET, syntax, theme) {
            return html;
        }
    }

    let safe_lang = sanitize_language_token(language);
    let safe_code = escape_html(code);
    if safe_lang.is_empty() {
        format!("<pre><code>{safe_code}</code></pre>")
    } else {
        format!("<pre><code class=\"language-{safe_lang}\">{safe_code}</code></pre>")
    }
}

fn sanitize_language_token(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn compile_islands(html: &str) -> (String, bool) {
    let mut had_islands = false;
    let replaced = ISLAND_TAG_RE
        .replace_all(html, |caps: &regex::Captures<'_>| {
            had_islands = true;
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let props = caps.get(2).map(|m| m.as_str()).unwrap_or("{}");
            format!(
                "<div data-island=\"{}\" data-props='{}'></div>",
                escape_html(name),
                escape_html(props)
            )
        })
        .into_owned();
    if !had_islands {
        return (replaced, false);
    }
    (
        format!(
            "{}\n<script type=\"module\" src=\"/_nanoss/islands-runtime.js\"></script>",
            replaced
        ),
        true,
    )
}

fn write_islands_runtime(output_root: &Path) -> Result<()> {
    let runtime_path = output_root.join("_nanoss").join("islands-runtime.js");
    if let Some(parent) = runtime_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create islands runtime parent {}", parent.display()))?;
    }
    let runtime = r#"const islands = document.querySelectorAll('[data-island]');
for (const node of islands) {
  const name = node.getAttribute('data-island');
  const raw = node.getAttribute('data-props') || '{}';
  let props = {};
  try {
    props = JSON.parse(raw);
  } catch {
    props = {};
  }
  node.textContent = `[island:${name}] hydrated with props ${JSON.stringify(props)}`;
}
"#;
    fs::write(&runtime_path, runtime)
        .with_context(|| format!("failed to write islands runtime {}", runtime_path.display()))?;
    Ok(())
}

fn optimize_css<'a>(source: &Path, css: &'a str) -> Result<String> {
    let options: ParserOptions<'a, 'a> = ParserOptions {
        filename: source.display().to_string(),
        css_modules: None,
        source_index: 0,
        error_recovery: false,
        warnings: None,
        flags: ParserFlags::empty(),
    };
    let output = StyleSheet::parse(css, options)
        .map_err(|err| anyhow!("failed to parse CSS {}: {err}", source.display()))?
        .to_css(PrinterOptions {
            minify: true,
            ..PrinterOptions::default()
        })
        .with_context(|| format!("failed to process CSS {}", source.display()))?;
    Ok(output.code)
}

#[derive(Default)]
struct LinkCheckReport {
    checked: usize,
    broken: usize,
}

fn check_external_links(output_root: &Path) -> Result<LinkCheckReport> {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("nanoss-link-checker/0.1.0")
        .build()
        .context("failed to create HTTP client for link checker")?;

    let mut links = HashSet::new();
    for entry in WalkDir::new(output_root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(OsStr::to_str) != Some("html") {
            continue;
        }
        let html = fs::read_to_string(entry.path())
            .with_context(|| format!("failed to read HTML file {}", entry.path().display()))?;
        for cap in HREF_HTTP_RE.captures_iter(&html) {
            if let Some(m) = cap.get(1) {
                links.insert(m.as_str().to_string());
            }
        }
    }

    let mut report = LinkCheckReport::default();
    for link in links {
        report.checked += 1;
        let status = check_url_status(&client, &link);
        if status.is_none() || status.unwrap() >= 400 {
            report.broken += 1;
            eprintln!("broken external link: {link}");
        }
    }

    Ok(report)
}

fn check_url_status(client: &Client, url: &str) -> Option<u16> {
    let head_status = request_status(client, Method::HEAD, url);
    match head_status {
        Some(code) if code == StatusCode::METHOD_NOT_ALLOWED.as_u16() => {
            request_status(client, Method::GET, url)
        }
        Some(code) => Some(code),
        None => request_status(client, Method::GET, url),
    }
}

fn load_build_cache(path: &Path) -> Result<BuildCache> {
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

fn save_build_cache(path: &Path, cache: &BuildCache) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create build cache parent {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(cache).context("failed to serialize build cache")?;
    fs::write(path, json).with_context(|| format!("failed to write build cache {}", path.display()))?;
    Ok(())
}

fn compute_template_dependency_hash(
    db: &QueryDb,
    template_dir: Option<&Path>,
    theme_dir: Option<&Path>,
) -> Result<String> {
    let mut hashes = Vec::new();

    if let Some(templates) = template_dir {
        let files: Vec<PathBuf> = WalkDir::new(templates)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(|e| e.path().to_path_buf())
            .collect();
        hashes.extend(files
            .par_iter()
            .map(|path| {
                let raw = read_file_for_query(path);
                let digest = blake3::hash(raw.as_bytes()).to_hex().to_string();
                format!("site:{}:{}", path.display(), digest)
            })
            .collect::<Vec<_>>());
    }

    if let Some(theme) = theme_dir {
        let theme_templates = theme.join("templates");
        if theme_templates.exists() {
            let files: Vec<PathBuf> = WalkDir::new(&theme_templates)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
                .map(|e| e.path().to_path_buf())
                .collect();
            hashes.extend(files
                .par_iter()
                .map(|path| {
                    let raw = read_file_for_query(path);
                    let digest = blake3::hash(raw.as_bytes()).to_hex().to_string();
                    format!("theme:{}:{}", path.display(), digest)
                })
                .collect::<Vec<_>>());
        }
    }

    hashes.sort_unstable();
    let mut merged = String::from("template-deps:v1");
    for hash in hashes {
        merged = combine_fingerprints(db, merged, hash);
    }
    Ok(merged)
}

fn compute_page_build_hash(
    db: &QueryDb,
    page_path: &Path,
    markdown_raw: &str,
    template_hash: &str,
    content_dir: &Path,
) -> Result<String> {
    let source = SourceFile::new(db, page_path.to_path_buf(), markdown_raw.to_string());
    let page_hash = content_hash(db, source);
    let asset_hash = compute_page_asset_dependency_hash(db, page_path, markdown_raw, content_dir)?;
    let with_assets = combine_fingerprints(db, page_hash, asset_hash);
    Ok(combine_fingerprints(
        db,
        with_assets,
        template_hash.to_string(),
    ))
}

fn compute_page_asset_dependency_hash(
    db: &QueryDb,
    page_path: &Path,
    markdown_raw: &str,
    content_dir: &Path,
) -> Result<String> {
    let mut deps = discover_page_asset_dependencies(page_path, markdown_raw);
    deps.retain(|path| path.starts_with(content_dir));

    let mut hashes = Vec::new();
    for dep in deps {
        if dep.is_file() {
            let raw = read_file_for_query(&dep);
            let source = SourceFile::new(db, dep.clone(), raw);
            let fingerprint = combine_fingerprints(
                db,
                dep.display().to_string(),
                content_hash(db, source),
            );
            hashes.push(fingerprint);
        }
    }

    hashes.sort_unstable();
    let mut merged = String::from("page-assets:v1");
    for hash in hashes {
        merged = combine_fingerprints(db, merged, hash);
    }
    Ok(merged)
}

fn discover_page_asset_dependencies(page_path: &Path, markdown_raw: &str) -> Vec<PathBuf> {
    let base_dir = page_path.parent().unwrap_or_else(|| Path::new("."));
    let mut deps = HashSet::new();
    for raw in extract_asset_like_refs(markdown_raw) {
        if is_external_ref(&raw) {
            continue;
        }
        if let Some(normalized) = normalize_ref(&raw) {
            deps.insert(base_dir.join(normalized));
        }
    }
    deps.into_iter().collect()
}

fn extract_asset_like_refs(markdown_raw: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for captures in MD_LINK_RE.captures_iter(markdown_raw) {
        if let Some(m) = captures.get(1) {
            refs.push(m.as_str().to_string());
        }
    }
    for captures in HTML_ASSET_ATTR_RE.captures_iter(markdown_raw) {
        if let Some(m) = captures.get(1) {
            refs.push(m.as_str().to_string());
        }
    }
    refs
}

fn is_external_ref(value: &str) -> bool {
    value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("mailto:")
        || value.starts_with('#')
        || value.starts_with("//")
        || value.starts_with("data:")
        || value.starts_with('/')
}

fn normalize_ref(value: &str) -> Option<&str> {
    let no_query = value.split('?').next().unwrap_or(value);
    let no_fragment = no_query.split('#').next().unwrap_or(no_query);
    if no_fragment.is_empty() {
        None
    } else {
        Some(no_fragment)
    }
}

fn read_file_for_query(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(err) => blake3::hash(&err.into_bytes()).to_hex().to_string(),
        },
        Err(_) => String::new(),
    }
}

fn build_semantic_index(content_dir: &Path, output_dir: &Path) -> Result<usize> {
    let files: Vec<PathBuf> = WalkDir::new(content_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.path().extension().and_then(OsStr::to_str) == Some("md"))
        .map(|entry| entry.path().to_path_buf())
        .collect();
    let docs: Vec<SemanticIndexDoc> = files
        .par_iter()
        .filter_map(|path| {
            let raw = fs::read_to_string(path).ok()?;
            let (frontmatter, body) = parse_frontmatter(&raw).ok()?;
            let title = frontmatter
                .title
                .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(ToOwned::to_owned))
                .unwrap_or_else(|| "Untitled".to_string());
            Some(SemanticIndexDoc {
                path: path.display().to_string(),
                title,
                embedding: embed_text_lightweight(body, 32),
            })
        })
        .collect();

    let semantic_path = output_dir.join("search").join("semantic-index.json");
    if let Some(parent) = semantic_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create semantic index parent {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&docs).context("failed to serialize semantic index")?;
    fs::write(&semantic_path, json)
        .with_context(|| format!("failed to write semantic index {}", semantic_path.display()))?;
    Ok(docs.len())
}

fn embed_text_lightweight(text: &str, dims: usize) -> Vec<f32> {
    let mut vec = vec![0.0f32; dims];
    for token in text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        let digest = blake3::hash(token.as_bytes());
        let bytes = digest.as_bytes();
        for i in 0..dims {
            let b = bytes[i % bytes.len()] as f32 / 255.0;
            vec[i] += (b - 0.5) * 2.0;
        }
    }
    let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    }
    vec
}

fn request_status(client: &Client, method: Method, url: &str) -> Option<u16> {
    client
        .request(method, url)
        .send()
        .ok()
        .map(|response| response.status().as_u16())
}

fn load_data_context(content_dir: &Path) -> Result<serde_json::Value> {
    let data_dir = content_dir.join("data");
    if !data_dir.exists() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    let mut root = serde_json::Map::new();
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
                let toml_value: toml::Value =
                    toml::from_str(&raw).with_context(|| format!("failed to parse toml data {}", entry.path().display()))?;
                serde_json::to_value(toml_value).context("failed to convert toml data to json value")?
            }
            _ => continue,
        };
        root.insert(key, value);
    }
    Ok(serde_json::Value::Object(root))
}

fn collect_content_entries(content_dir: &Path, output_dir: &Path, base_path: &str) -> Result<Vec<ContentEntry>> {
    let mut entries = Vec::new();
    for entry in WalkDir::new(content_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() || entry.path().extension().and_then(OsStr::to_str) != Some("md") {
            continue;
        }
        let raw = fs::read_to_string(entry.path())
            .with_context(|| format!("failed to read content entry {}", entry.path().display()))?;
        let (fm, _) = parse_frontmatter(&raw)
            .with_context(|| format!("failed to parse content entry {}", entry.path().display()))?;
        let output = output_path_for(
            entry.path(),
            content_dir,
            output_dir,
            fm.slug.as_deref(),
            fm.lang.as_deref(),
        )?;
        let rel = output
            .strip_prefix(output_dir)
            .unwrap_or(output.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        let url = to_site_url(rel.trim_end_matches("index.html"), base_path);
        entries.push(ContentEntry {
            title: fm
                .title
                .or_else(|| entry.path().file_stem().and_then(|s| s.to_str()).map(ToOwned::to_owned))
                .unwrap_or_else(|| "Untitled".to_string()),
            url,
            date: fm.date,
            tags: fm.tags.unwrap_or_default(),
            categories: fm.categories.unwrap_or_default(),
        });
    }
    entries.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(entries)
}

fn render_organization_page(
    env: &Environment<'_>,
    data_context: &serde_json::Value,
    base_path: &str,
    title: &str,
    body_html: &str,
) -> Result<String> {
    let tmpl = env.get_template("page.html").context("missing page.html template")?;
    let rendered = tmpl
        .render(context! {
            title => title,
            content => body_html,
            toc => "",
            data => data_context,
            base_path => base_path,
            base_href_prefix => base_href_prefix(base_path)
        })
        .context("failed to render organization page template")?;
    Ok(rewrite_html_absolute_links_with_base_path(&rendered, base_path))
}

fn generate_content_organization_outputs(
    entries: &[ContentEntry],
    output_dir: &Path,
    env: &Environment<'_>,
    data_context: &serde_json::Value,
    base_path: &str,
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let posts_dir = output_dir.join("posts");
    fs::create_dir_all(&posts_dir).with_context(|| format!("failed to create {}", posts_dir.display()))?;
    let per_page = 10usize;
    for (idx, chunk) in entries.chunks(per_page).enumerate() {
        let page_num = idx + 1;
        let page_dir = if page_num == 1 {
            posts_dir.clone()
        } else {
            posts_dir.join("page").join(page_num.to_string())
        };
        fs::create_dir_all(&page_dir).with_context(|| format!("failed to create {}", page_dir.display()))?;
        let mut body = String::from("<h1>Posts</h1><ul>");
        for item in chunk {
            body.push_str(&format!("<li><a href=\"{}\">{}</a></li>", item.url, item.title));
        }
        body.push_str("</ul>");
        let html = render_organization_page(env, data_context, base_path, "Posts", &body)?;
        fs::write(page_dir.join("index.html"), html).with_context(|| "failed to write posts page".to_string())?;
    }

    let mut tags: BTreeMap<String, Vec<&ContentEntry>> = BTreeMap::new();
    let mut categories: BTreeMap<String, Vec<&ContentEntry>> = BTreeMap::new();
    for item in entries {
        for tag in &item.tags {
            tags.entry(tag.clone()).or_default().push(item);
        }
        for category in &item.categories {
            categories.entry(category.clone()).or_default().push(item);
        }
    }
    write_taxonomy_pages(output_dir.join("tags"), "Tags", &tags, env, data_context, base_path)?;
    write_taxonomy_pages(
        output_dir.join("categories"),
        "Categories",
        &categories,
        env,
        data_context,
        base_path,
    )?;
    Ok(())
}

fn write_taxonomy_pages(
    root: PathBuf,
    heading: &str,
    groups: &BTreeMap<String, Vec<&ContentEntry>>,
    env: &Environment<'_>,
    data_context: &serde_json::Value,
    base_path: &str,
) -> Result<()> {
    fs::create_dir_all(&root).with_context(|| format!("failed to create {}", root.display()))?;
    for (name, entries) in groups {
        let key = slugify(name);
        let dir = root.join(&key);
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let mut body = format!("<h1>{}: {}</h1><ul>", heading, name);
        for item in entries {
            body.push_str(&format!("<li><a href=\"{}\">{}</a></li>", item.url, item.title));
        }
        body.push_str("</ul>");
        let html = render_organization_page(env, data_context, base_path, &format!("{}: {}", heading, name), &body)?;
        fs::write(dir.join("index.html"), html)
            .with_context(|| format!("failed to write taxonomy page {}", dir.display()))?;
    }
    Ok(())
}

fn generate_sitemap_and_feed(entries: &[ContentEntry], output_dir: &Path, site_domain: Option<&str>) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut sitemap =
        String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">");
    for item in entries {
        sitemap.push_str(&format!(
            "<url><loc>{}</loc></url>",
            canonicalize_site_url(&item.url, site_domain)
        ));
    }
    sitemap.push_str("</urlset>");
    fs::write(output_dir.join("sitemap.xml"), sitemap).context("failed to write sitemap.xml")?;

    let mut rss = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><rss version=\"2.0\"><channel><title>Nanoss Feed</title>",
    );
    for item in entries.iter().take(20) {
        rss.push_str(&format!(
            "<item><title>{}</title><link>{}</link>{}</item>",
            item.title,
            canonicalize_site_url(&item.url, site_domain),
            item.date
                .as_ref()
                .map(|date| format!("<pubDate>{}</pubDate>", date))
                .unwrap_or_default()
        ));
    }
    rss.push_str("</channel></rss>");
    fs::write(output_dir.join("rss.xml"), rss).context("failed to write rss.xml")?;
    Ok(())
}

fn validate_build_config(config: &BuildConfig) -> Result<()> {
    if config.max_frontmatter_bytes == 0 {
        bail!("max_frontmatter_bytes must be greater than zero");
    }
    if config.max_file_bytes == 0 {
        bail!("max_file_bytes must be greater than zero");
    }
    if config.max_total_files == 0 {
        bail!("max_total_files must be greater than zero");
    }
    if !config.base_path.starts_with('/') {
        bail!("base_path must start with '/'");
    }
    Ok(())
}

fn normalize_base_path(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    let mut normalized = trimmed.trim_end_matches('/').to_string();
    if !normalized.starts_with('/') {
        normalized = format!("/{}", normalized);
    }
    normalized
}

fn normalize_site_domain(input: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = input else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        bail!("site_domain must start with http:// or https://");
    }
    Ok(Some(trimmed.trim_end_matches('/').to_string()))
}

fn base_href_prefix(base_path: &str) -> &str {
    if base_path == "/" {
        ""
    } else {
        base_path
    }
}

fn with_base_path(url: &str, base_path: &str) -> String {
    if base_path == "/" || !url.starts_with('/') || url.starts_with("//") {
        return url.to_string();
    }
    if url == "/" {
        return format!("{}/", base_path);
    }
    if url == base_path || url.starts_with(&format!("{}/", base_path.trim_end_matches('/'))) {
        return url.to_string();
    }
    format!("{base_path}{url}")
}

fn to_site_url(path_without_root: &str, base_path: &str) -> String {
    let raw = format!("/{}", path_without_root).replace("//", "/");
    with_base_path(&raw, base_path)
}

fn canonicalize_site_url(path: &str, site_domain: Option<&str>) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    if let Some(domain) = site_domain {
        return format!("{domain}{path}");
    }
    path.to_string()
}

fn rewrite_html_absolute_links_with_base_path(html: &str, base_path: &str) -> String {
    if base_path == "/" {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len() + 32);
    let mut idx = 0usize;
    while let Some(rel) = html[idx..].find("href=\"/").or_else(|| html[idx..].find("src=\"/")) {
        let start = idx + rel;
        out.push_str(&html[idx..start]);
        let attr = if html[start..].starts_with("href=\"/") { "href" } else { "src" };
        let value_start = start + attr.len() + 2;
        if let Some(end_quote_rel) = html[value_start..].find('"') {
            let value_end = value_start + end_quote_rel;
            let original = &html[value_start..value_end];
            out.push_str(attr);
            out.push_str("=\"");
            out.push_str(&with_base_path(original, base_path));
            out.push('"');
            idx = value_end + 1;
        } else {
            out.push_str(&html[start..]);
            idx = html.len();
        }
    }
    out.push_str(&html[idx..]);
    out
}

fn validate_frontmatter_size(raw: &str, limit: usize) -> Result<()> {
    if !raw.starts_with("---\n") {
        return Ok(());
    }
    let remainder = &raw[4..];
    if let Some(end) = remainder.find("\n---\n") {
        if end > limit {
            bail!("frontmatter size {} exceeds limit {}", end, limit);
        }
    }
    Ok(())
}

fn hashed_file_content(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
        Err(_) => String::new(),
    }
}

fn asset_output_path(source: &Path, content_root: &Path, output_root: &Path, force_ext: Option<&str>) -> Result<PathBuf> {
    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let mut target = output_root.join(rel);
    if let Some(ext) = force_ext {
        target.set_extension(ext);
    }
    ensure_inside_output(output_root, &target)?;
    Ok(target)
}

fn validate_route_segment(value: &str, field: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{field} cannot be empty");
    }
    if value.contains("..") || value.contains('/') || value.contains('\\') {
        bail!("{field} contains invalid path traversal characters");
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("{field} may only contain [a-zA-Z0-9_-]");
    }
    Ok(())
}

fn ensure_inside_output(output_root: &Path, candidate: &Path) -> Result<()> {
    if !candidate.starts_with(output_root) {
        bail!(
            "target path escapes output directory: {}",
            candidate.display()
        );
    }
    Ok(())
}

fn ensure_binary_name_safe(binary: &str) -> Result<()> {
    if binary.trim().is_empty() {
        bail!("binary name cannot be empty");
    }
    if binary.contains('\n') || binary.contains('\r') {
        bail!("binary name contains invalid control characters");
    }
    Ok(())
}

fn wait_child_with_timeout(child: &mut Child, timeout_secs: u64) -> Result<ExitStatus> {
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("failed to poll child process")? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            bail!("child process timed out after {}s", timeout_secs.max(1));
        }
        thread::sleep(Duration::from_millis(50));
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn page_template_prefers_site_over_theme() -> Result<()> {
        let site = tempdir().context("failed to create site dir")?;
        let theme = tempdir().context("failed to create theme dir")?;
        fs::write(site.path().join("page.html"), "site").context("failed to write site template")?;
        fs::create_dir_all(theme.path().join("templates")).context("failed to make theme templates")?;
        fs::write(theme.path().join("templates/page.html"), "theme")
            .context("failed to write theme template")?;
        let chosen = load_page_template(Some(site.path()), Some(theme.path()))?;
        assert_eq!(chosen, "site");
        Ok(())
    }

    #[test]
    fn copy_theme_static_skips_existing_output() -> Result<()> {
        let theme = tempdir().context("failed to create theme dir")?;
        let out = tempdir().context("failed to create output dir")?;
        fs::create_dir_all(theme.path().join("static/assets")).context("failed to make static dir")?;
        fs::write(theme.path().join("static/assets/logo.txt"), "theme")
            .context("failed to write theme asset")?;
        fs::create_dir_all(out.path().join("assets")).context("failed to make out asset dir")?;
        fs::write(out.path().join("assets/logo.txt"), "site").context("failed to write site asset")?;

        copy_theme_static_assets(Some(theme.path()), out.path())?;
        let final_asset = fs::read_to_string(out.path().join("assets/logo.txt"))
            .context("failed to read merged asset")?;
        assert_eq!(final_asset, "site");
        Ok(())
    }

    #[test]
    fn copy_site_static_assets_overwrites_existing_output() -> Result<()> {
        let static_dir = tempdir().context("failed to create static dir")?;
        let out = tempdir().context("failed to create output dir")?;
        fs::create_dir_all(static_dir.path().join("assets")).context("failed to make static assets dir")?;
        fs::create_dir_all(out.path().join("assets")).context("failed to make output assets dir")?;
        fs::write(static_dir.path().join("assets/logo.txt"), "site").context("failed to write static asset")?;
        fs::write(out.path().join("assets/logo.txt"), "old").context("failed to write existing output asset")?;

        copy_site_static_assets(static_dir.path(), out.path())?;
        let final_asset = fs::read_to_string(out.path().join("assets/logo.txt"))
            .context("failed to read merged site static asset")?;
        assert_eq!(final_asset, "site");
        Ok(())
    }

    #[test]
    fn template_hash_includes_theme_templates() -> Result<()> {
        let query_db = QueryDb::default();
        let site_templates = tempdir().context("failed to create site templates dir")?;
        let theme = tempdir().context("failed to create theme dir")?;
        fs::create_dir_all(theme.path().join("templates")).context("failed to create theme templates")?;
        fs::write(site_templates.path().join("page.html"), "site-v1").context("failed to write site template")?;
        fs::write(theme.path().join("templates/page.html"), "theme-v1").context("failed to write theme template")?;

        let before = compute_template_dependency_hash(&query_db, Some(site_templates.path()), Some(theme.path()))?;
        fs::write(theme.path().join("templates/page.html"), "theme-v2").context("failed to update theme template")?;
        let after = compute_template_dependency_hash(&query_db, Some(site_templates.path()), Some(theme.path()))?;

        assert_ne!(before, after);
        Ok(())
    }

    #[test]
    fn route_segment_rejects_traversal() {
        assert!(validate_route_segment("../etc", "slug").is_err());
        assert!(validate_route_segment("ok-slug_1", "slug").is_ok());
    }

    #[test]
    fn build_cache_schema_mismatch_resets() -> Result<()> {
        let dir = tempdir().context("failed to create tempdir")?;
        let cache_file = dir.path().join(BUILD_CACHE_FILE);
        fs::write(
            &cache_file,
            r#"{"schema_version":1,"pages":{"k":{"hash":"h","output":"o"}}}"#,
        )
        .context("failed to write cache fixture")?;
        let cache = load_build_cache(&cache_file)?;
        assert_eq!(cache.schema_version, BUILD_CACHE_SCHEMA_VERSION);
        assert!(cache.pages.is_empty());
        Ok(())
    }

    #[test]
    fn data_context_supports_json_yaml_toml() -> Result<()> {
        let dir = tempdir().context("failed to create tempdir")?;
        fs::create_dir_all(dir.path().join("data")).context("failed to create data dir")?;
        fs::write(dir.path().join("data/site.json"), r#"{"name":"nanoss"}"#).context("write json")?;
        fs::write(dir.path().join("data/theme.yaml"), "kind: blog").context("write yaml")?;
        fs::write(dir.path().join("data/build.toml"), "mode = 'fast'").context("write toml")?;
        let data = load_data_context(dir.path())?;
        let obj = data.as_object().context("expected object data")?;
        assert!(obj.contains_key("site"));
        assert!(obj.contains_key("theme"));
        assert!(obj.contains_key("build"));
        Ok(())
    }
}
