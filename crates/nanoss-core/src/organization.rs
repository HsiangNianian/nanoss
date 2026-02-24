use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use minijinja::{context, Environment};
use std::ffi::OsStr;
use walkdir::WalkDir;

use crate::{ContentEntry, I18nConfig};

pub(crate) fn collect_content_entries(
    content_dir: &Path,
    output_dir: &Path,
    base_path: &str,
    i18n: &I18nConfig,
    include_drafts: bool,
) -> Result<Vec<ContentEntry>> {
    let mut entries = Vec::new();
    for entry in WalkDir::new(content_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(OsStr::to_str) != Some("md")
        {
            continue;
        }
        let raw = fs::read_to_string(entry.path())
            .with_context(|| format!("failed to read content entry {}", entry.path().display()))?;
        let (fm, _) = crate::render::parse_frontmatter(&raw)
            .with_context(|| format!("failed to parse content entry {}", entry.path().display()))?;
        if !crate::render::should_render_content(&fm, include_drafts)? {
            continue;
        }
        let output = crate::path::output_path_for(
            entry.path(),
            content_dir,
            output_dir,
            fm.slug.as_deref(),
            fm.lang.as_deref(),
            i18n,
        )?;
        let rel = output
            .strip_prefix(output_dir)
            .unwrap_or(output.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        let url = crate::path::to_site_url(rel.trim_end_matches("index.html"), base_path);
        entries.push(ContentEntry {
            title: fm
                .title
                .or_else(|| {
                    entry
                        .path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(ToOwned::to_owned)
                })
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

pub(crate) fn render_organization_page(
    env: &Environment<'_>,
    data_context: &serde_json::Value,
    base_path: &str,
    title: &str,
    body_html: &str,
) -> Result<String> {
    let tmpl = env
        .get_template("page.html")
        .context("missing page.html template")?;
    let rendered = tmpl
        .render(context! {
            title => title,
            content => body_html,
            toc => "",
            data => data_context,
            images => serde_json::json!({"list": []}),
            alternates => Vec::<serde_json::Value>::new(),
            locale => "und",
            seo => serde_json::json!({
                "title": title,
                "description": title,
                "canonical": crate::path::with_base_path("/", base_path),
                "og_image": serde_json::Value::Null,
                "twitter_card": "summary",
                "json_ld": serde_json::Value::Null,
                "noindex": false
            }),
            base_path => base_path,
            base_href_prefix => crate::path::base_href_prefix(base_path)
        })
        .context("failed to render organization page template")?;
    Ok(crate::path::rewrite_html_absolute_links_with_base_path(
        &rendered, base_path,
    ))
}

pub(crate) fn generate_content_organization_outputs(
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
    fs::create_dir_all(&posts_dir)
        .with_context(|| format!("failed to create {}", posts_dir.display()))?;
    let per_page = 10usize;
    for (idx, chunk) in entries.chunks(per_page).enumerate() {
        let page_num = idx + 1;
        let page_dir = if page_num == 1 {
            posts_dir.clone()
        } else {
            posts_dir.join("page").join(page_num.to_string())
        };
        fs::create_dir_all(&page_dir)
            .with_context(|| format!("failed to create {}", page_dir.display()))?;
        let mut body = String::from("<h1>Posts</h1><ul>");
        for item in chunk {
            body.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                item.url, item.title
            ));
        }
        body.push_str("</ul>");
        let html = render_organization_page(env, data_context, base_path, "Posts", &body)?;
        fs::write(page_dir.join("index.html"), html)
            .with_context(|| "failed to write posts page".to_string())?;
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
    write_taxonomy_pages(
        output_dir.join("tags"),
        "Tags",
        &tags,
        env,
        data_context,
        base_path,
    )?;
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

pub(crate) fn write_taxonomy_pages(
    root: PathBuf,
    heading: &str,
    groups: &BTreeMap<String, Vec<&ContentEntry>>,
    env: &Environment<'_>,
    data_context: &serde_json::Value,
    base_path: &str,
) -> Result<()> {
    fs::create_dir_all(&root).with_context(|| format!("failed to create {}", root.display()))?;
    for (name, entries) in groups {
        let key = crate::render::slugify(name);
        let dir = root.join(&key);
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let mut body = format!("<h1>{}: {}</h1><ul>", heading, name);
        for item in entries {
            body.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                item.url, item.title
            ));
        }
        body.push_str("</ul>");
        let html = render_organization_page(
            env,
            data_context,
            base_path,
            &format!("{}: {}", heading, name),
            &body,
        )?;
        fs::write(dir.join("index.html"), html)
            .with_context(|| format!("failed to write taxonomy page {}", dir.display()))?;
    }
    Ok(())
}
