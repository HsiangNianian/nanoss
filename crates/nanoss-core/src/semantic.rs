use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::ffi::OsStr;
use walkdir::WalkDir;

use crate::SemanticIndexDoc;

pub(crate) fn build_semantic_index(content_dir: &Path, output_dir: &Path) -> Result<usize> {
    let files: Vec<PathBuf> = WalkDir::new(content_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.path().extension().and_then(OsStr::to_str) == Some("md"))
        .map(|entry| entry.path().to_path_buf())
        .collect();
    let docs: Vec<SemanticIndexDoc> = files
        .par_iter()
        .filter_map(|path| {
            let raw = fs::read_to_string(path).ok()?;
            let (frontmatter, body) = crate::render::parse_frontmatter(&raw).ok()?;
            let title = frontmatter
                .title
                .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(ToOwned::to_owned))
                .unwrap_or_else(|| "Untitled".to_string());
            Some(SemanticIndexDoc {
                path: path.display().to_string(),
                title,
                embedding: embed_text_lightweight(body, 32),
            })
        })
        .collect();

    let semantic_path = output_dir.join("search").join("semantic-index.json");
    if let Some(parent) = semantic_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create semantic index parent {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&docs).context("failed to serialize semantic index")?;
    fs::write(&semantic_path, json)
        .with_context(|| format!("failed to write semantic index {}", semantic_path.display()))?;
    Ok(docs.len())
}

pub(crate) fn embed_text_lightweight(text: &str, dims: usize) -> Vec<f32> {
    let mut vec = vec![0.0f32; dims];
    for token in text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        let digest = blake3::hash(token.as_bytes());
        let bytes = digest.as_bytes();
        for i in 0..dims {
            let b = bytes[i % bytes.len()] as f32 / 255.0;
            vec[i] += (b - 0.5) * 2.0;
        }
    }
    let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    }
    vec
}
