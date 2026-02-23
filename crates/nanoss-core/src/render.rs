use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use minijinja::{context, Environment};
use nanoss_plugin_host::PluginHost;

use crate::{
    base_href_prefix, build_i18n_alternates, build_page_image_helpers, build_toc_html, expand_component_shortcodes,
    markdown_to_html, parse_frontmatter, resolve_template_name, rewrite_html_absolute_links_with_base_path, BuildConfig,
    PageIr, RenderedPage,
};

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
    let (frontmatter, markdown) =
        parse_frontmatter(&transformed_raw).with_context(|| format!("failed to parse frontmatter for {}", path.display()))?;

    let expanded_markdown = expand_component_shortcodes(markdown);
    let (html_content, toc_items) = markdown_to_html(&expanded_markdown);
    let title = frontmatter
        .title
        .clone()
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
