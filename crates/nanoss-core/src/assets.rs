use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use rswind::create_processor;
use walkdir::WalkDir;

use crate::ports::{ProcessPort, StdProcessPort};
use crate::{optimize_css, JsBackend, TailwindBackend, TailwindConfig, CLASS_ATTR_RE};

pub(crate) fn process_script_asset(
    source: &Path,
    content_root: &Path,
    output_root: &Path,
    backend: JsBackend,
    timeout_secs: u64,
) -> Result<()> {
    process_script_asset_with_executor(source, content_root, output_root, backend, timeout_secs, &StdProcessPort)
}

pub(crate) fn process_script_asset_with_executor(
    source: &Path,
    content_root: &Path,
    output_root: &Path,
    backend: JsBackend,
    timeout_secs: u64,
    executor: &dyn ProcessPort,
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
            let args = vec![
                source.to_string_lossy().to_string(),
                "--bundle".to_string(),
                "--minify".to_string(),
                "--outfile".to_string(),
                target.to_string_lossy().to_string(),
            ];
            executor
                .run("esbuild", &args, timeout_secs)
                .with_context(|| format!("esbuild failed for {}", source.display()))?;
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
    run_tailwind_standalone_with_executor(config, timeout_secs, &StdProcessPort)
}

pub(crate) fn run_tailwind_standalone_with_executor(
    config: &TailwindConfig,
    timeout_secs: u64,
    executor: &dyn ProcessPort,
) -> Result<()> {
    if let Some(parent) = config.output_css.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create tailwind output parent {}", parent.display()))?;
    }
    let mut args = vec![
        "-i".to_string(),
        config.input_css.to_string_lossy().to_string(),
        "-o".to_string(),
        config.output_css.to_string_lossy().to_string(),
    ];
    if config.minify {
        args.push("--minify".to_string());
    }
    executor
        .run(&config.binary, &args, timeout_secs)
        .with_context(|| format!("{} compile failed", config.binary))?;
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
