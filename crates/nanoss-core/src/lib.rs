use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use minijinja::{context, Environment};
use pulldown_cmark::{html, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::Deserialize;
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

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub content_dir: PathBuf,
    pub output_dir: PathBuf,
    pub template_dir: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct BuildReport {
    pub rendered_pages: usize,
}

#[derive(Debug, Deserialize, Default)]
struct FrontMatter {
    title: Option<String>,
    slug: Option<String>,
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
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }

        let rendered = render_markdown_file(entry.path(), &env)?;
        let target = output_path_for(entry.path(), &config.content_dir, &config.output_dir, rendered.slug.as_deref())?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
        }
        fs::write(&target, rendered.html)
            .with_context(|| format!("failed to write rendered file {}", target.display()))?;
        report.rendered_pages += 1;
    }

    Ok(report)
}

struct RenderedPage {
    slug: Option<String>,
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

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some((level, Vec::new()));
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

fn output_path_for(source: &Path, content_root: &Path, output_root: &Path, slug: Option<&str>) -> Result<PathBuf> {
    if let Some(slug) = slug {
        return Ok(output_root.join(slug).join("index.html"));
    }

    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let mut target = output_root.join(rel);
    target.set_extension("html");
    Ok(target)
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
