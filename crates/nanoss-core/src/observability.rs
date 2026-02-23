use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
