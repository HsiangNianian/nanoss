use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::I18nConfig;

pub(crate) fn output_path_for(
    source: &Path,
    content_root: &Path,
    output_root: &Path,
    slug: Option<&str>,
    lang: Option<&str>,
    i18n: &I18nConfig,
) -> Result<PathBuf> {
    let locale_segment = locale_prefix_for_output(lang, i18n)?;
    if let Some(slug) = slug {
        crate::validation::validate_route_segment(slug, "slug")?;
        let mut base = output_root.to_path_buf();
        if let Some(locale) = locale_segment.as_deref() {
            base = base.join(locale);
        }
        let candidate = if slug == "index" {
            base.join("index.html")
        } else {
            base.join(slug).join("index.html")
        };
        crate::validation::ensure_inside_output(output_root, &candidate)?;
        return Ok(candidate);
    }

    let rel = source.strip_prefix(content_root).with_context(|| {
        format!(
            "{} is not inside {}",
            source.display(),
            content_root.display()
        )
    })?;
    let mut target = output_root.to_path_buf();
    if let Some(locale) = locale_segment.as_deref() {
        target = target.join(locale);
    }
    target = target.join(rel);
    target.set_extension("html");
    crate::validation::ensure_inside_output(output_root, &target)?;
    Ok(target)
}

pub(crate) fn locale_prefix_for_output(
    lang: Option<&str>,
    i18n: &I18nConfig,
) -> Result<Option<String>> {
    let Some(locale) = lang.or(i18n.default_locale.as_deref()) else {
        return Ok(None);
    };
    crate::validation::validate_route_segment(locale, "locale")?;
    if !i18n.locales.is_empty() && !i18n.locales.iter().any(|candidate| candidate == locale) {
        bail!("locale '{}' is not in configured locales", locale);
    }
    if i18n.default_locale.as_deref() == Some(locale) && !i18n.prefix_default_locale {
        return Ok(None);
    }
    Ok(Some(locale.to_string()))
}

pub(crate) fn normalize_base_path(input: &str) -> String {
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

pub(crate) fn normalize_site_domain(input: Option<&str>) -> Result<Option<String>> {
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

pub(crate) fn base_href_prefix(base_path: &str) -> &str {
    if base_path == "/" {
        ""
    } else {
        base_path
    }
}

pub(crate) fn with_base_path(url: &str, base_path: &str) -> String {
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

pub(crate) fn to_site_url(path_without_root: &str, base_path: &str) -> String {
    let raw = format!("/{}", path_without_root).replace("//", "/");
    with_base_path(&raw, base_path)
}

pub(crate) fn canonicalize_site_url(path: &str, site_domain: Option<&str>) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    if let Some(domain) = site_domain {
        return format!("{domain}{path}");
    }
    path.to_string()
}

pub(crate) fn rewrite_html_absolute_links_with_base_path(html: &str, base_path: &str) -> String {
    if base_path == "/" {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len() + 32);
    let mut idx = 0usize;
    while let Some(rel) = html[idx..]
        .find("href=\"/")
        .or_else(|| html[idx..].find("src=\"/"))
    {
        let start = idx + rel;
        out.push_str(&html[idx..start]);
        let attr = if html[start..].starts_with("href=\"/") {
            "href"
        } else {
            "src"
        };
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

pub(crate) fn asset_output_path(
    source: &Path,
    content_root: &Path,
    output_root: &Path,
    force_ext: Option<&str>,
) -> Result<PathBuf> {
    let rel = source.strip_prefix(content_root).with_context(|| {
        format!(
            "{} is not inside {}",
            source.display(),
            content_root.display()
        )
    })?;
    let mut target = output_root.join(rel);
    if let Some(ext) = force_ext {
        target.set_extension(ext);
    }
    crate::validation::ensure_inside_output(output_root, &target)?;
    Ok(target)
}
