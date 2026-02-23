use std::path::Path;

use anyhow::{bail, Result};

use crate::BuildConfig;

pub(crate) fn validate_build_config(config: &BuildConfig) -> Result<()> {
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
    for width in &config.images.widths {
        if *width == 0 {
            bail!("image width variants must be > 0");
        }
    }
    if let Some(default_locale) = config.i18n.default_locale.as_deref() {
        validate_route_segment(default_locale, "default_locale")?;
    }
    for locale in &config.i18n.locales {
        validate_route_segment(locale, "locale")?;
    }
    Ok(())
}

pub(crate) fn validate_frontmatter_size(raw: &str, limit: usize) -> Result<()> {
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

pub(crate) fn validate_route_segment(value: &str, field: &str) -> Result<()> {
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

pub(crate) fn ensure_inside_output(output_root: &Path, candidate: &Path) -> Result<()> {
    if !candidate.starts_with(output_root) {
        bail!(
            "target path escapes output directory: {}",
            candidate.display()
        );
    }
    Ok(())
}

pub(crate) fn ensure_binary_name_safe(binary: &str) -> Result<()> {
    if binary.trim().is_empty() {
        bail!("binary name cannot be empty");
    }
    if binary.contains('\n') || binary.contains('\r') {
        bail!("binary name contains invalid control characters");
    }
    Ok(())
}
