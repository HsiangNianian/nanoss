use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use lightningcss::stylesheet::{ParserFlags, ParserOptions, PrinterOptions, StyleSheet};
use reqwest::{Method, StatusCode};
use walkdir::WalkDir;

use crate::{
    combine_fingerprints, content_hash, LinkCheckReport, QueryDb, SourceFile, HREF_HTTP_RE,
    HTML_ASSET_ATTR_RE, MD_LINK_RE,
};

pub(crate) fn optimize_css<'a>(source: &Path, css: &'a str) -> Result<String> {
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

pub(crate) fn check_external_links(output_root: &Path) -> Result<LinkCheckReport> {
    let http = crate::ports::StdHttpPort;
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
        let status = check_url_status(&http, &link);
        if status.is_none() || status.unwrap() >= 400 {
            report.broken += 1;
            eprintln!("broken external link: {link}");
        }
    }
    Ok(report)
}

pub(crate) fn check_url_status(http: &dyn crate::ports::HttpPort, url: &str) -> Option<u16> {
    let head_status = request_status(http, Method::HEAD, url);
    match head_status {
        Some(code) if code == StatusCode::METHOD_NOT_ALLOWED.as_u16() => {
            request_status(http, Method::GET, url)
        }
        Some(code) => Some(code),
        None => request_status(http, Method::GET, url),
    }
}

pub(crate) fn request_status(
    http: &dyn crate::ports::HttpPort,
    method: Method,
    url: &str,
) -> Option<u16> {
    http.request_status(method, url, 10, "nanoss-link-checker/0.1.0")
}

pub(crate) fn compute_template_dependency_hash(
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
        hashes.extend(
            files
                .iter()
                .map(|path| {
                    let raw = read_file_for_query(path);
                    let digest = blake3::hash(raw.as_bytes()).to_hex().to_string();
                    format!("site:{}:{}", path.display(), digest)
                })
                .collect::<Vec<_>>(),
        );
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
            hashes.extend(
                files
                    .iter()
                    .map(|path| {
                        let raw = read_file_for_query(path);
                        let digest = blake3::hash(raw.as_bytes()).to_hex().to_string();
                        format!("theme:{}:{}", path.display(), digest)
                    })
                    .collect::<Vec<_>>(),
            );
        }
    }

    hashes.sort_unstable();
    let mut merged = String::from("template-deps:v1");
    for hash in hashes {
        merged = combine_fingerprints(db, merged, hash);
    }
    Ok(merged)
}

pub(crate) fn compute_page_build_hash(
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

pub(crate) fn compute_page_asset_dependency_hash(
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
            let fingerprint =
                combine_fingerprints(db, dep.display().to_string(), content_hash(db, source));
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

pub(crate) fn discover_page_asset_dependencies(
    page_path: &Path,
    markdown_raw: &str,
) -> Vec<PathBuf> {
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

pub(crate) fn extract_asset_like_refs(markdown_raw: &str) -> Vec<String> {
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

pub(crate) fn is_external_ref(value: &str) -> bool {
    value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("mailto:")
        || value.starts_with('#')
        || value.starts_with("//")
        || value.starts_with("data:")
        || value.starts_with('/')
}

pub(crate) fn normalize_ref(value: &str) -> Option<&str> {
    let no_query = value.split('?').next().unwrap_or(value);
    let no_fragment = no_query.split('#').next().unwrap_or(no_query);
    if no_fragment.is_empty() {
        None
    } else {
        Some(no_fragment)
    }
}

pub(crate) fn read_file_for_query(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(err) => blake3::hash(&err.into_bytes()).to_hex().to_string(),
        },
        Err(_) => String::new(),
    }
}

pub(crate) fn hashed_file_content(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
        Err(_) => String::new(),
    }
}

pub(crate) fn wait_child_with_timeout(child: &mut Child, timeout_secs: u64) -> Result<ExitStatus> {
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
