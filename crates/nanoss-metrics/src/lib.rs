pub trait MetricsCollector: Send + Sync {
    fn increment_counter(&self, _name: &str, _labels: &[(&str, &str)]) {}
    fn record_histogram(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {}
}

#[derive(Default)]
pub struct NoOpMetricsCollector;

impl MetricsCollector for NoOpMetricsCollector {}

#[derive(Default)]
pub struct StdoutMetricsCollector;

impl MetricsCollector for StdoutMetricsCollector {
    fn increment_counter(&self, name: &str, labels: &[(&str, &str)]) {
        println!(
            "{{\"metric_type\":\"counter\",\"name\":\"{}\",\"labels\":{}}}",
            name,
            labels_to_json(labels)
        );
    }

    fn record_histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        println!(
            "{{\"metric_type\":\"histogram\",\"name\":\"{}\",\"value\":{},\"labels\":{}}}",
            name,
            value,
            labels_to_json(labels)
        );
    }
}

fn labels_to_json(labels: &[(&str, &str)]) -> String {
    let mut out = String::from("{");
    for (idx, (key, value)) in labels.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&escape_json(key));
        out.push_str("\":\"");
        out.push_str(&escape_json(value));
        out.push('"');
    }
    out.push('}');
    out
}

fn escape_json(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

pub mod metric_names {
    pub const BUILD_DURATION_MS: &str = "nanoss_build_duration_ms";
    pub const PAGE_RENDER_DURATION_MS: &str = "nanoss_page_render_duration_ms";
    pub const PAGES_RENDERED_TOTAL: &str = "nanoss_pages_rendered_total";
    pub const PAGES_SKIPPED_TOTAL: &str = "nanoss_pages_skipped_total";
    pub const ASSETS_PROCESSED_TOTAL: &str = "nanoss_assets_processed_total";
}
