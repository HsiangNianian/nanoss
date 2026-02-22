use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use minijinja::{context, Environment};
use once_cell::sync::Lazy;
use pulldown_cmark::{html, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
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

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub content_dir: PathBuf,
    pub output_dir: PathBuf,
    pub template_dir: Option<PathBuf>,
    pub check_external_links: bool,
    pub fail_on_broken_links: bool,
}

#[derive(Debug, Default)]
pub struct BuildReport {
    pub rendered_pages: usize,
    pub compiled_sass: usize,
    pub copied_assets: usize,
    pub checked_external_links: usize,
    pub broken_external_links: usize,
}

#[derive(Debug, Deserialize, Default)]
struct FrontMatter {
    title: Option<String>,
    slug: Option<String>,
    lang: Option<String>,
}

pub fn build_site(config: &BuildConfig) -> Result<BuildReport> {
    fs::create_dir_all(&config.output_dir)
        .with_context(|| format!("failed to create output directory {}", config.output_dir.display()))?;

    let mut env = Environment::new();
    let page_template = load_page_template(config.template_dir.as_deref())?;
    env.add_template("page.html", &page_template)
        .context("failed to register template")?;

    let mut report = BuildReport::default();
    for entry in WalkDir::new(&config.content_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        match entry.path().extension().and_then(OsStr::to_str) {
            Some("md") => {
                let rendered = render_markdown_file(entry.path(), &env)?;
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
                fs::write(&target, rendered.html)
                    .with_context(|| format!("failed to write rendered file {}", target.display()))?;
                report.rendered_pages += 1;
            }
            Some("scss") | Some("sass") => {
                compile_sass_file(entry.path(), &config.content_dir, &config.output_dir)?;
                report.compiled_sass += 1;
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

fn render_markdown_file(path: &Path, env: &Environment<'_>) -> Result<RenderedPage> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read markdown file {}", path.display()))?;
    let (frontmatter, markdown) = parse_frontmatter(&raw).with_context(|| {
        format!("failed to parse frontmatter for {}", path.display())
    })?;

    let (html_content, toc_items) = markdown_to_html(markdown);
    let toc = build_toc_html(&toc_items);
    let title = frontmatter
        .title
        .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(ToOwned::to_owned))
        .unwrap_or_else(|| "Untitled".to_string());

    let tmpl = env.get_template("page.html").context("missing page.html template")?;
    let html = tmpl
        .render(context! { title => title, content => html_content, toc => toc })
        .context("failed to render page template")?;

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
    fs::write(&target, css).with_context(|| format!("failed to write Sass output {}", target.display()))?;
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

fn request_status(client: &Client, method: Method, url: &str) -> Option<u16> {
    client
        .request(method, url)
        .send()
        .ok()
        .map(|response| response.status().as_u16())
}
