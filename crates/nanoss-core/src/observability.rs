use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, path::Path};

use anyhow::{Context, Result};

static BUILD_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_build_id() -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = BUILD_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("build-{now_ms}-{seq}")
}

pub(crate) fn emit_event(stage: &str, build_id: &str, payload: serde_json::Value) {
    tracing::info!(
        event_name = stage,
        build_id = build_id,
        payload = %payload,
        "nanoss event"
    );
    let mut obj = serde_json::Map::new();
    obj.insert(
        "stage".to_string(),
        serde_json::Value::String(stage.to_string()),
    );
    obj.insert(
        "build_id".to_string(),
        serde_json::Value::String(build_id.to_string()),
    );
    obj.insert("payload".to_string(), payload);
    println!("{}", serde_json::Value::Object(obj));
}

pub(crate) fn write_build_report(
    output_dir: &Path,
    build_id: &str,
    duration_ms: u128,
    report: &crate::BuildReport,
) -> Result<()> {
    let report_dir = output_dir.join("_nanoss");
    fs::create_dir_all(&report_dir)
        .with_context(|| format!("failed to create {}", report_dir.display()))?;
    let report_path = report_dir.join("build-report.json");
    let payload = serde_json::json!({
        "build_id": build_id,
        "duration_ms": duration_ms,
        "rendered_pages": report.rendered_pages,
        "skipped_pages": report.skipped_pages,
        "compiled_sass": report.compiled_sass,
        "copied_assets": report.copied_assets,
        "processed_scripts": report.processed_scripts,
        "processed_images": report.processed_images,
        "island_pages": report.island_pages,
        "ai_indexed_pages": report.ai_indexed_pages,
        "checked_external_links": report.checked_external_links,
        "broken_external_links": report.broken_external_links,
        "compiled_tailwind": report.compiled_tailwind
    });
    fs::write(&report_path, serde_json::to_vec_pretty(&payload)?)
        .with_context(|| format!("failed to write {}", report_path.display()))?;
    Ok(())
}
