pub trait MetricsCollector: Send + Sync {
    fn increment_counter(&self, _name: &str, _labels: &[(&str, &str)]) {}
    fn record_histogram(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {}
}

#[derive(Default)]
pub struct NoOpMetricsCollector;

impl MetricsCollector for NoOpMetricsCollector {}

pub mod metric_names {
    pub const BUILD_DURATION_MS: &str = "nanoss_build_duration_ms";
    pub const PAGE_RENDER_DURATION_MS: &str = "nanoss_page_render_duration_ms";
    pub const PAGES_RENDERED_TOTAL: &str = "nanoss_pages_rendered_total";
    pub const PAGES_SKIPPED_TOTAL: &str = "nanoss_pages_skipped_total";
    pub const ASSETS_PROCESSED_TOTAL: &str = "nanoss_assets_processed_total";
}
