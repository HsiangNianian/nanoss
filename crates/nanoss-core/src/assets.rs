use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use rswind::create_processor;
use walkdir::WalkDir;

use crate::{optimize_css, wait_child_with_timeout, JsBackend, TailwindBackend, TailwindConfig, CLASS_ATTR_RE};

pub(crate) fn process_script_asset(
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
            crate::ensure_binary_name_safe("esbuild")?;
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

pub(crate) fn run_tailwind(config: &TailwindConfig, content_dir: &Path, timeout_secs: u64) -> Result<()> {
    match config.backend {
        TailwindBackend::Standalone => run_tailwind_standalone(config, timeout_secs),
        TailwindBackend::Rswind => run_tailwind_rswind(config, content_dir),
    }
}

pub(crate) fn run_tailwind_standalone(config: &TailwindConfig, timeout_secs: u64) -> Result<()> {
    crate::ensure_binary_name_safe(&config.binary)?;
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

pub(crate) fn run_tailwind_rswind(config: &TailwindConfig, content_dir: &Path) -> Result<()> {
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
