use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::avif::AvifEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{ColorType, GenericImageView, ImageEncoder};
use rswind::create_processor;
use walkdir::WalkDir;

use crate::ports::{ProcessPort, StdProcessPort};
use crate::{optimize_css, CacheImageRecord, ImageBuildConfig, ImageVariantRecord, JsBackend, TailwindBackend, TailwindConfig, CLASS_ATTR_RE};

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

pub(crate) fn compile_sass_file(source: &Path, content_root: &Path, output_root: &Path, _timeout_secs: u64) -> Result<()> {
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
    let optimized = optimize_css(source, &css)?;
    fs::write(&target, optimized).with_context(|| format!("failed to write Sass output {}", target.display()))?;
    Ok(())
}

pub(crate) fn process_css_asset(source: &Path, content_root: &Path, output_root: &Path) -> Result<()> {
    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let target = output_root.join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }
    let raw_css = fs::read_to_string(source)
        .with_context(|| format!("failed to read CSS asset {}", source.display()))?;
    let optimized = optimize_css(source, &raw_css)?;
    fs::write(&target, optimized)
        .with_context(|| format!("failed to write CSS asset {}", target.display()))?;
    Ok(())
}

pub(crate) fn copy_asset_file(source: &Path, content_root: &Path, output_root: &Path) -> Result<()> {
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

pub(crate) fn process_image_asset(
    source: &Path,
    content_root: &Path,
    output_root: &Path,
    image_config: &ImageBuildConfig,
) -> Result<CacheImageRecord> {
    let rel = source
        .strip_prefix(content_root)
        .with_context(|| format!("{} is not inside {}", source.display(), content_root.display()))?;
    let target = output_root.join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }
    fs::copy(source, &target)
        .with_context(|| format!("failed to copy image {} -> {}", source.display(), target.display()))?;

    let mut record = CacheImageRecord {
        hash: crate::hashed_file_content(source),
        output: target.display().to_string(),
        width: None,
        height: None,
        variants: Vec::new(),
    };

    if !image_config.enabled {
        return Ok(record);
    }

    let img = image::open(source).with_context(|| format!("failed to open image {}", source.display()))?;
    let (orig_w, orig_h) = img.dimensions();
    record.width = Some(orig_w);
    record.height = Some(orig_h);

    let mut widths = image_config.widths.clone();
    widths.sort_unstable();
    widths.dedup();

    for width in widths.into_iter().filter(|w| *w > 0 && *w < orig_w) {
        let resized = img.resize(width, u32::MAX, FilterType::Lanczos3);
        if image_config.generate_webp {
            if let Some(path) = write_image_variant(&target, &resized, "webp", width)? {
                record.variants.push(ImageVariantRecord {
                    format: "webp".to_string(),
                    width: Some(width),
                    output: path.display().to_string(),
                });
            }
        }
        if image_config.generate_avif {
            if let Some(path) = write_image_variant(&target, &resized, "avif", width)? {
                record.variants.push(ImageVariantRecord {
                    format: "avif".to_string(),
                    width: Some(width),
                    output: path.display().to_string(),
                });
            }
        }
    }
    Ok(record)
}

pub(crate) fn write_image_variant(
    target: &Path,
    image: &image::DynamicImage,
    format: &str,
    width: u32,
) -> Result<Option<PathBuf>> {
    let mut path = target.to_path_buf();
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("image")
        .to_string();
    path.set_file_name(format!("{stem}-{width}.{format}"));

    let mut bytes = Vec::new();
    match format {
        "webp" => {
            let rgba = image.to_rgba8();
            let encoder = WebPEncoder::new_lossless(&mut bytes);
            encoder
                .write_image(&rgba, rgba.width(), rgba.height(), ColorType::Rgba8.into())
                .context("failed to encode webp variant")?;
        }
        "avif" => {
            let rgba = image.to_rgba8();
            let encoder = AvifEncoder::new(&mut bytes);
            encoder
                .write_image(&rgba, rgba.width(), rgba.height(), ColorType::Rgba8.into())
                .context("failed to encode avif variant")?;
        }
        _ => return Ok(None),
    }
    fs::write(&path, bytes).with_context(|| format!("failed to write variant {}", path.display()))?;
    Ok(Some(path))
}

pub(crate) fn is_image_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "tiff"
    )
}
