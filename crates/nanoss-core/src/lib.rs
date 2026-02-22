use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use lightningcss::stylesheet::{ParserOptions, PrinterOptions, StyleSheet};
use minijinja::{context, Environment};
use nanoss_plugin_host::{PluginHost, PluginHostConfig};
use nanoss_query::{combine_fingerprints, content_hash, QueryDb, SourceFile};
use once_cell::sync::Lazy;
use pulldown_cmark::{html, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
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
const BUILD_CACHE_FILE: &str = ".nanoss-cache.json";

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub content_dir: PathBuf,
    pub output_dir: PathBuf,
    pub template_dir: Option<PathBuf>,
    pub plugin_paths: Vec<PathBuf>,
    pub plugin_timeout_ms: u64,
    pub plugin_memory_limit_mb: u64,
    pub check_external_links: bool,
    pub fail_on_broken_links: bool,
    pub js_backend: JsBackend,
    pub tailwind: Option<TailwindConfig>,
    pub enable_ai_index: bool,
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
}

#[derive(Debug, Serialize, Deserialize)]
struct PageIr {
    title: String,
    content_html: String,
    toc_html: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BuildCache {
    pages: HashMap<String, CachePageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachePageRecord {
    hash: String,
    output: String,
}

#[derive(Debug, Serialize)]
struct SemanticIndexDoc {
    path: String,
    title: String,
    embedding: Vec<f32>,
}

pub fn build_site(config: &BuildConfig) -> Result<BuildReport> {
    fs::create_dir_all(&config.output_dir)
        .with_context(|| format!("failed to create output directory {}", config.output_dir.display()))?;

    let mut env = Environment::new();
    let page_template = load_page_template(config.template_dir.as_deref())?;
    env.add_template("page.html", &page_template)
        .context("failed to register template")?;

    let plugin_host = PluginHost::new(PluginHostConfig {
        plugin_paths: config.plugin_paths.clone(),
        timeout_ms: config.plugin_timeout_ms,
        memory_limit_mb: config.plugin_memory_limit_mb,
    })?;
    plugin_host.init("{}")?;

    let mut report = BuildReport::default();
    let query_db = QueryDb::default();
    let cache_path = config.output_dir.join(BUILD_CACHE_FILE);
    let mut build_cache = load_build_cache(&cache_path)?;
    let dependency_hash = compute_global_dependency_hash(&query_db, config.content_dir.as_path(), config.template_dir.as_deref())?;
    if let Some(tailwind) = &config.tailwind {
        run_tailwind(tailwind, &config.content_dir)?;
        report.compiled_tailwind = true;
    }

    let mut islands_runtime_written = false;
    for entry in WalkDir::new(&config.content_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        match entry.path().extension().and_then(OsStr::to_str) {
            Some("md") => {
                let raw = fs::read_to_string(entry.path())
                    .with_context(|| format!("failed to read markdown file {}", entry.path().display()))?;
                let source = SourceFile::new(&query_db, entry.path().to_path_buf(), raw);
                let page_hash = content_hash(&query_db, source);
                let current_hash = combine_fingerprints(&query_db, page_hash, dependency_hash.clone());
                let cache_key = entry.path().display().to_string();
                if let Some(record) = build_cache.pages.get(&cache_key) {
                    let cached_output = PathBuf::from(&record.output);
                    if record.hash == current_hash && cached_output.exists() {
                        report.skipped_pages += 1;
                        continue;
                    }
                }

                let rendered = render_markdown_file(entry.path(), &env, &plugin_host)?;
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
                compile_sass_file(entry.path(), &config.content_dir, &config.output_dir)?;
                report.compiled_sass += 1;
            }
            Some("css") => {
                process_css_asset(entry.path(), &config.content_dir, &config.output_dir)?;
                report.copied_assets += 1;
            }
            Some("js") | Some("mjs") | Some("cjs") | Some("ts") => {
                process_script_asset(entry.path(), &config.content_dir, &config.output_dir, config.js_backend)?;
                report.processed_scripts += 1;
            }
            _ => {
                copy_asset_file(entry.path(), &config.content_dir, &config.output_dir)?;
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

fn render_markdown_file(path: &Path, env: &Environment<'_>, plugin_host: &PluginHost) -> Result<RenderedPage> {
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
            toc => transformed_ir.toc_html
        })
        .context("failed to render page template")?;
    let html = plugin_host
        .on_post_render(&path.display().to_string(), rendered_html)
        .with_context(|| format!("plugin on_post_render failed for {}", path.display()))?;

    Ok(RenderedPage {
        slug: frontmatter.slug,
        lang: frontmatter.lang,
        html,
    })
}

fn load_page_template(template_dir: Option<&Path>) -> Result<String> {
    if let Some(dir) = template_dir {
        let candidate = dir.join("page.html");
        if candidate.exists() {
            return fs::read_to_string(&candidate)
                .with_context(|| format!("failed to read template {}", candidate.display()));
        }
    }
    Ok(DEFAULT_PAGE_TEMPLATE.to_string())
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
        let mut base = output_root.to_path_buf();
        if let Some(lang) = lang {
            base = base.join(lang);
        }
        return Ok(base.join(slug).join("index.html"));
    }

    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let mut target = output_root.to_path_buf();
    if let Some(lang) = lang {
        target = target.join(lang);
    }
    target = target.join(rel);
    target.set_extension("html");
    Ok(target)
}

fn compile_sass_file(source: &Path, content_root: &Path, output_root: &Path) -> Result<()> {
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

fn process_script_asset(source: &Path, content_root: &Path, output_root: &Path, backend: JsBackend) -> Result<()> {
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
            let status = Command::new("esbuild")
                .arg(source)
                .arg("--bundle")
                .arg("--minify")
                .arg("--outfile")
                .arg(&target)
                .status()
                .with_context(|| format!("failed to execute esbuild for {}", source.display()))?;
            if !status.success() {
                bail!("esbuild failed for {}", source.display());
            }
        }
    }
    Ok(())
}

fn run_tailwind(config: &TailwindConfig, content_dir: &Path) -> Result<()> {
    match config.backend {
        TailwindBackend::Standalone => run_tailwind_standalone(config),
        TailwindBackend::Rswind => run_tailwind_rswind(config, content_dir),
    }
}

fn run_tailwind_standalone(config: &TailwindConfig) -> Result<()> {
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
    let status = cmd
        .status()
        .with_context(|| format!("failed to execute {}", config.binary))?;
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

fn optimize_css(source: &Path, css: &str) -> Result<String> {
    let options: ParserOptions<'static, 'static> = ParserOptions {
        filename: source.display().to_string(),
        ..ParserOptions::default()
    };
    // LightningCSS parser keeps references tied to input lifetime; for this prototype we
    // promote the buffer to 'static per build step to keep integration straightforward.
    let leaked_css: &'static str = Box::leak(css.to_string().into_boxed_str());
    let output = StyleSheet::parse(
        leaked_css,
        options,
    )
    .with_context(|| format!("failed to parse CSS {}", source.display()))?
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
    let cache = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse build cache {}", path.display()))?;
    Ok(cache)
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

fn compute_global_dependency_hash(db: &QueryDb, content_dir: &Path, template_dir: Option<&Path>) -> Result<String> {
    let mut hashes = Vec::new();
    for entry in WalkDir::new(content_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(OsStr::to_str) == Some("md") {
            continue;
        }
        let raw = read_file_for_query(entry.path());
        let source = SourceFile::new(db, entry.path().to_path_buf(), raw);
        let fingerprint = combine_fingerprints(
            db,
            entry.path().display().to_string(),
            content_hash(db, source),
        );
        hashes.push(fingerprint);
    }

    if let Some(templates) = template_dir {
        for entry in WalkDir::new(templates).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let raw = read_file_for_query(entry.path());
            let source = SourceFile::new(db, entry.path().to_path_buf(), raw);
            let fingerprint = combine_fingerprints(
                db,
                entry.path().display().to_string(),
                content_hash(db, source),
            );
            hashes.push(fingerprint);
        }
    }

    hashes.sort_unstable();
    let mut merged = String::from("deps:v1");
    for hash in hashes {
        merged = combine_fingerprints(db, merged, hash);
    }
    Ok(merged)
}

fn read_file_for_query(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes.clone()) {
            Ok(text) => text,
            Err(_) => blake3::hash(&bytes).to_hex().to_string(),
        },
        Err(_) => String::new(),
    }
}

fn build_semantic_index(content_dir: &Path, output_dir: &Path) -> Result<usize> {
    let mut docs = Vec::new();
    for entry in WalkDir::new(content_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(OsStr::to_str) != Some("md") {
            continue;
        }
        let raw = fs::read_to_string(entry.path())
            .with_context(|| format!("failed to read markdown for semantic index {}", entry.path().display()))?;
        let (frontmatter, body) = parse_frontmatter(&raw)
            .with_context(|| format!("failed to parse markdown for semantic index {}", entry.path().display()))?;
        let title = frontmatter
            .title
            .or_else(|| entry.path().file_stem().and_then(|s| s.to_str()).map(ToOwned::to_owned))
            .unwrap_or_else(|| "Untitled".to_string());
        let embedding = embed_text_lightweight(body, 32);
        docs.push(SemanticIndexDoc {
            path: entry.path().display().to_string(),
            title,
            embedding,
        });
    }

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
