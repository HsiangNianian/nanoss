use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginApiVersion {
    V1Json,
    V2TypedDraft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginPageIrV1 {
    pub title: String,
    pub content_html: String,
    pub toc_html: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginPageIrV2 {
    pub path: String,
    pub title: String,
    pub content_html: String,
    pub toc_html: String,
}

#[derive(Debug, Clone)]
pub struct PluginBoundary {
    api_version: PluginApiVersion,
}

impl PluginBoundary {
    pub fn new(api_version: PluginApiVersion) -> Self {
        Self { api_version }
    }

    pub fn api_version(&self) -> PluginApiVersion {
        self.api_version
    }

    pub fn serialize_v1(ir: &PluginPageIrV1) -> Result<String> {
        serde_json::to_string(ir).context("failed to serialize plugin page ir v1")
    }

    pub fn deserialize_v1(raw: &str) -> Result<PluginPageIrV1> {
        serde_json::from_str(raw).context("failed to deserialize plugin page ir v1")
    }

    pub fn v1_to_v2(path: &str, ir: &PluginPageIrV1) -> PluginPageIrV2 {
        PluginPageIrV2 {
            path: path.to_string(),
            title: ir.title.clone(),
            content_html: ir.content_html.clone(),
            toc_html: ir.toc_html.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginBoundaryError {
    #[error("plugin execution failed: {0}")]
    Execution(String),
    #[error("plugin payload format mismatch: {0}")]
    Payload(String),
    #[error("plugin API version mismatch: {0}")]
    Version(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_roundtrip_works() {
        let ir = PluginPageIrV1 {
            title: "T".to_string(),
            content_html: "<p>x</p>".to_string(),
            toc_html: "<ul></ul>".to_string(),
        };
        let json = PluginBoundary::serialize_v1(&ir).expect("serialize");
        let restored = PluginBoundary::deserialize_v1(&json).expect("deserialize");
        assert_eq!(restored.title, "T");
        assert_eq!(restored.content_html, "<p>x</p>");
    }

    #[test]
    fn v1_to_v2_keeps_content() {
        let ir = PluginPageIrV1 {
            title: "Title".to_string(),
            content_html: "<main/>".to_string(),
            toc_html: "<nav/>".to_string(),
        };
        let v2 = PluginBoundary::v1_to_v2("/index", &ir);
        assert_eq!(v2.path, "/index");
        assert_eq!(v2.title, ir.title);
    }
}
