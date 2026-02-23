use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use minijinja::{context, Environment};
use nanoss_plugin_boundary::{PluginApiVersion, PluginBoundary, PluginPageIrV1};
use nanoss_plugin_host::PluginHost;
use pulldown_cmark::{
    html, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use syntect::html::highlighted_html_for_string;
use walkdir::WalkDir;

use crate::{BuildConfig, FrontMatter, RenderedPage};

pub(crate) fn render_markdown_file(
    path: &Path,
    env: &Environment<'_>,
    templates: &HashMap<String, String>,
    plugin_host: &mut PluginHost,
    data_context: &serde_json::Value,
    config: &BuildConfig,
    base_path: &str,
) -> Result<RenderedPage> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read markdown file {}", path.display()))?;
    let transformed_raw = plugin_host
        .transform_markdown(&path.display().to_string(), raw)
        .with_context(|| format!("plugin transform_markdown failed for {}", path.display()))?;
    let (frontmatter, markdown) = parse_frontmatter(&transformed_raw)
        .with_context(|| format!("failed to parse frontmatter for {}", path.display()))?;

    let expanded_markdown = expand_component_shortcodes(markdown);
    let (html_content, toc_items) = markdown_to_html(&expanded_markdown);
    let title = frontmatter
        .title
        .clone()
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "Untitled".to_string());
    let boundary = PluginBoundary::new(PluginApiVersion::V1Json);
    let ir = PluginPageIrV1 {
        title,
        content_html: html_content,
        toc_html: build_toc_html(&toc_items),
    };
    let ir_json = PluginBoundary::serialize_v1(&ir).context("failed to serialize page ir")?;
    let transformed_ir_json = plugin_host
        .on_page_ir(&path.display().to_string(), ir_json)
        .with_context(|| format!("plugin on_page_ir failed for {}", path.display()))?;
    let transformed_ir = PluginBoundary::deserialize_v1(&transformed_ir_json)
        .context("plugin returned invalid page ir json")?;
    if boundary.api_version() == PluginApiVersion::V2TypedDraft {
        let _ = PluginBoundary::v1_to_v2(&path.display().to_string(), &transformed_ir);
    }

    let template_name = resolve_template_name(path, &frontmatter, templates);
    let tmpl = env
        .get_template(&template_name)
        .with_context(|| format!("missing template {}", template_name))?;
    let image_helpers = build_page_image_helpers(path, markdown, config, base_path)?;
    let alternates = build_i18n_alternates(path, &frontmatter, config, base_path)?;
    let rendered_html = tmpl
        .render(context! {
            title => transformed_ir.title,
            content => transformed_ir.content_html,
            toc => transformed_ir.toc_html,
            data => data_context,
            images => image_helpers,
            alternates => alternates,
            locale => frontmatter.lang.clone().or_else(|| config.i18n.default_locale.clone()).unwrap_or_else(|| "und".to_string()),
            base_path => base_path,
            base_href_prefix => crate::path::base_href_prefix(base_path)
        })
        .context("failed to render page template")?;
    let html = plugin_host
        .on_post_render(&path.display().to_string(), rendered_html)
        .with_context(|| format!("plugin on_post_render failed for {}", path.display()))?;
    let html = crate::path::rewrite_html_absolute_links_with_base_path(&html, base_path);

    Ok(RenderedPage {
        slug: frontmatter.slug,
        lang: frontmatter.lang,
        html,
    })
}

pub(crate) fn load_templates(
    template_dir: Option<&Path>,
    theme_dir: Option<&Path>,
) -> Result<HashMap<String, String>> {
    let mut templates = HashMap::new();
    templates.insert(
        "page.html".to_string(),
        crate::DEFAULT_PAGE_TEMPLATE.to_string(),
    );

    if let Some(theme) = theme_dir {
        let root = theme.join("templates");
        if root.exists() {
            collect_templates_from_dir(&root, &mut templates)?;
        }
    }
    if let Some(site) = template_dir {
        if site.exists() {
            collect_templates_from_dir(site, &mut templates)?;
        }
    }
    Ok(templates)
}

pub(crate) fn collect_templates_from_dir(
    root: &Path,
    templates: &mut HashMap<String, String>,
) -> Result<()> {
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(OsStr::to_str) != Some("html") {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .with_context(|| format!("failed to relativize template {}", entry.path().display()))?;
        let key = rel.to_string_lossy().replace('\\', "/");
        let value = fs::read_to_string(entry.path())
            .with_context(|| format!("failed to read template {}", entry.path().display()))?;
        templates.insert(key, value);
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn load_page_template(
    template_dir: Option<&Path>,
    theme_dir: Option<&Path>,
) -> Result<String> {
    if let Some(site) = template_dir {
        let site_page = site.join("page.html");
        if site_page.exists() {
            return fs::read_to_string(&site_page)
                .with_context(|| format!("failed to read site template {}", site_page.display()));
        }
    }
    if let Some(theme) = theme_dir {
        let theme_page = theme.join("templates").join("page.html");
        if theme_page.exists() {
            return fs::read_to_string(&theme_page).with_context(|| {
                format!("failed to read theme template {}", theme_page.display())
            });
        }
    }
    Ok(crate::DEFAULT_PAGE_TEMPLATE.to_string())
}

pub(crate) fn copy_theme_static_assets(theme_dir: Option<&Path>, output_dir: &Path) -> Result<()> {
    let Some(theme) = theme_dir else {
        return Ok(());
    };
    let static_root = theme.join("static");
    if !static_root.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(&static_root)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&static_root)
            .with_context(|| format!("failed to relativize {}", entry.path().display()))?;
        let target = output_dir.join(rel);
        if target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(entry.path(), &target).with_context(|| {
            format!(
                "failed to copy theme static asset {} -> {}",
                entry.path().display(),
                target.display()
            )
        })?;
    }
    Ok(())
}

pub(crate) fn copy_site_static_assets(static_dir: &Path, output_dir: &Path) -> Result<()> {
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
            .with_context(|| format!("failed to relativize {}", entry.path().display()))?;
        let target = output_dir.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(entry.path(), &target).with_context(|| {
            format!(
                "failed to copy static asset {} -> {}",
                entry.path().display(),
                target.display()
            )
        })?;
    }
    Ok(())
}

pub(crate) fn parse_frontmatter(input: &str) -> Result<(FrontMatter, &str)> {
    if let Some(stripped) = input.strip_prefix("---\n") {
        if let Some(end) = stripped.find("\n---\n") {
            let yaml = &stripped[..end];
            let rest = &stripped[end + 5..];
            let fm: FrontMatter =
                serde_yaml::from_str(yaml).context("failed to parse frontmatter yaml")?;
            return Ok((fm, rest));
        }
    }
    Ok((FrontMatter::default(), input))
}

pub(crate) fn markdown_to_html(markdown: &str) -> (String, Vec<crate::TocItem>) {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(markdown, options);
    let mut out_events = Vec::new();
    let mut toc_items = Vec::new();
    let mut heading_level = None::<HeadingLevel>;
    let mut heading_events: Vec<Event<'_>> = Vec::new();

    for event in parser {
        match &event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading_level = Some(*level);
                heading_events.clear();
            }
            Event::End(TagEnd::Heading(..)) => {
                if let Some(level) = heading_level.take() {
                    let text = heading_text(&heading_events);
                    let id = slugify(&text);
                    toc_items.push(crate::TocItem {
                        level: heading_level_to_u8(level),
                        id: id.clone(),
                        text: text.clone(),
                    });
                    out_events.push(Event::Html(CowStr::from(format!(
                        "<h{} id=\"{}\"><a href=\"#{}\">{}</a></h{}>",
                        heading_level_to_u8(level),
                        id,
                        id,
                        escape_html(&text),
                        heading_level_to_u8(level)
                    ))));
                    heading_events.clear();
                    continue;
                }
            }
            _ => {}
        }
        if heading_level.is_some() {
            heading_events.push(event.clone());
            continue;
        }
        out_events.push(event);
    }

    let mut html_out = String::new();
    html::push_html(&mut html_out, out_events.into_iter());
    (html_out, toc_items)
}

pub(crate) fn expand_component_shortcodes(markdown: &str) -> String {
    let mut output = String::new();
    let mut rest = markdown;
    loop {
        let Some(start) = rest.find("{{<") else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..start]);
        let tail = &rest[start + 3..];
        let Some(end) = tail.find(">}}") else {
            output.push_str(&rest[start..]);
            break;
        };
        let expr = tail[..end].trim();
        let mut parts = expr.split_whitespace();
        let name = parts.next().unwrap_or("component");
        let attrs = parts.collect::<Vec<_>>().join(" ");
        output.push_str(&format!(
            "<div data-shortcode=\"{}\" data-attrs=\"{}\"></div>",
            escape_html(name),
            escape_html(&attrs)
        ));
        rest = &tail[end + 3..];
    }
    output
}

pub(crate) fn resolve_template_name(
    path: &Path,
    frontmatter: &FrontMatter,
    templates: &HashMap<String, String>,
) -> String {
    if let Some(name) = &frontmatter.template {
        if templates.contains_key(name) {
            return name.clone();
        }
    }
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        let file_template = format!("{stem}.html");
        if templates.contains_key(&file_template) {
            return file_template;
        }
    }
    "page.html".to_string()
}

pub(crate) fn build_page_image_helpers(
    path: &Path,
    markdown: &str,
    config: &BuildConfig,
    base_path: &str,
) -> Result<serde_json::Value> {
    let refs = crate::utils::discover_page_asset_dependencies(path, markdown);
    let mut items = Vec::new();
    for source in refs {
        let cache_key = source.display().to_string();
        let rel_url = source
            .strip_prefix(&config.content_dir)
            .unwrap_or(source.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        let mut variants = Vec::new();
        if let Ok(cache_raw) = fs::read_to_string(config.output_dir.join(crate::BUILD_CACHE_FILE)) {
            if let Ok(cache) = serde_json::from_str::<serde_json::Value>(&cache_raw) {
                if let Some(entries) = cache
                    .get("images")
                    .and_then(|v| v.get(&cache_key))
                    .and_then(|v| v.get("variants"))
                    .and_then(|v| v.as_array())
                {
                    for variant in entries {
                        if let (Some(format), Some(output)) =
                            (variant.get("format"), variant.get("output"))
                        {
                            variants.push(serde_json::json!({
                                "format": format,
                                "output": crate::path::with_base_path(
                                    &format!("/{}", output.as_str().unwrap_or_default().replace('\\', "/")),
                                    base_path
                                )
                            }));
                        }
                    }
                }
            }
        }
        items.push(serde_json::json!({
            "source": crate::path::with_base_path(&format!("/{}", rel_url), base_path),
            "variants": variants
        }));
    }
    Ok(serde_json::json!({ "list": items }))
}

pub(crate) fn build_i18n_alternates(
    path: &Path,
    frontmatter: &FrontMatter,
    config: &BuildConfig,
    base_path: &str,
) -> Result<Vec<serde_json::Value>> {
    if config.i18n.locales.is_empty() {
        return Ok(Vec::new());
    }
    let slug = frontmatter
        .slug
        .clone()
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "index".to_string());
    let mut alternates = Vec::new();
    for locale in &config.i18n.locales {
        let locale_prefix = crate::path::locale_prefix_for_output(Some(locale), &config.i18n)?;
        let route = if slug == "index" {
            match locale_prefix {
                Some(prefix) => format!("/{prefix}/"),
                None => "/".to_string(),
            }
        } else {
            match locale_prefix {
                Some(prefix) => format!("/{prefix}/{slug}/"),
                None => format!("/{slug}/"),
            }
        };
        alternates.push(serde_json::json!({
            "locale": locale,
            "url": crate::path::with_base_path(&route, base_path),
        }));
    }
    Ok(alternates)
}

pub(crate) fn heading_text(events: &[Event<'_>]) -> String {
    let mut text = String::new();
    for event in events {
        match event {
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            _ => {}
        }
    }
    text.trim().to_string()
}

pub(crate) fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

pub(crate) fn slugify(value: &str) -> String {
    let mut slug = String::new();
    for ch in value.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

pub(crate) fn build_toc_html(items: &[crate::TocItem]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut html = String::from("<ul>");
    for item in items {
        html.push_str(&format!(
            "<li class=\"toc-level-{}\"><a href=\"#{}\">{}</a></li>",
            item.level,
            item.id,
            escape_html(&item.text)
        ));
    }
    html.push_str("</ul>");
    html
}

#[allow(dead_code)]
pub(crate) fn language_from_code_block_kind(kind: CodeBlockKind<'_>) -> String {
    match kind {
        CodeBlockKind::Indented => "txt".to_string(),
        CodeBlockKind::Fenced(name) => sanitize_language_token(name.as_ref()),
    }
}

#[allow(dead_code)]
pub(crate) fn highlight_code_block(language: &str, code: &str) -> String {
    let syntax = crate::SYNTAX_SET
        .find_syntax_by_token(language)
        .unwrap_or_else(|| crate::SYNTAX_SET.find_syntax_plain_text());
    let theme = crate::THEME_SET
        .themes
        .get("base16-ocean.dark")
        .or_else(|| crate::THEME_SET.themes.values().next());
    match theme {
        Some(theme) => highlighted_html_for_string(code, &crate::SYNTAX_SET, syntax, theme)
            .unwrap_or_else(|_| format!("<pre><code>{}</code></pre>", escape_html(code))),
        None => format!("<pre><code>{}</code></pre>", escape_html(code)),
    }
}

#[allow(dead_code)]
pub(crate) fn sanitize_language_token(value: &str) -> String {
    let lowered = value.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        "txt".to_string()
    } else {
        lowered
    }
}

pub(crate) fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn compile_islands(html: &str) -> (String, bool) {
    let mut has_islands = false;
    let replaced = crate::ISLAND_TAG_RE.replace_all(html, |caps: &regex::Captures<'_>| {
        has_islands = true;
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or("unknown");
        let props = caps.get(2).map(|m| m.as_str()).unwrap_or("{}");
        format!(
            "<div data-island=\"{}\" data-props='{}'></div>",
            escape_html(name),
            escape_html(props)
        )
    });
    let mut output = replaced.to_string();
    if has_islands {
        output = inject_islands_runtime_script(&output);
    }
    (output, has_islands)
}

pub(crate) fn inject_islands_runtime_script(html: &str) -> String {
    let runtime_tag = r#"<script type=\"module\" src=\"/_nanoss/islands-runtime.js\"></script>"#;
    if let Some(pos) = html.rfind("</body>") {
        let mut out = String::with_capacity(html.len() + runtime_tag.len() + 1);
        out.push_str(&html[..pos]);
        out.push_str(runtime_tag);
        out.push_str(&html[pos..]);
        out
    } else {
        format!("{html}{runtime_tag}")
    }
}

pub(crate) fn write_islands_runtime(output_root: &Path) -> Result<()> {
    let runtime_path = output_root.join("_nanoss").join("islands-runtime.js");
    if let Some(parent) = runtime_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(
        &runtime_path,
        r#"window.NanossIslands = (() => {
  const handlers = new Map();
  const hydrate = (targetName) => {
    const selector = targetName ? `[data-island="${targetName}"]` : "[data-island]";
    document.querySelectorAll(selector).forEach((node) => {
      const name = node.getAttribute("data-island");
      const raw = node.getAttribute("data-props") || "{}";
      const handler = handlers.get(name);
      if (!handler) return;
      let props = {};
      try { props = JSON.parse(raw); } catch (_) {}
      handler(node, props);
    });
  };
  return {
    register(name, handler) {
      handlers.set(name, handler);
      hydrate(name);
    },
    hydrate(targetName) {
      hydrate(targetName);
    }
  };
})();
"#,
    )
    .with_context(|| format!("failed to write islands runtime {}", runtime_path.display()))?;
    Ok(())
}
