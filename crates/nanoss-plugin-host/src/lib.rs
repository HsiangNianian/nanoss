use std::path::PathBuf;

use anyhow::{bail, Result};

pub struct PluginHostConfig {
    pub plugin_paths: Vec<PathBuf>,
    pub timeout_ms: u64,
    pub memory_limit_mb: u64,
}

impl Default for PluginHostConfig {
    fn default() -> Self {
        Self {
            plugin_paths: Vec::new(),
            timeout_ms: 2_000,
            memory_limit_mb: 128,
        }
    }
}

pub struct PluginHost {
    config: PluginHostConfig,
}

impl PluginHost {
    pub fn new(config: PluginHostConfig) -> Self {
        Self { config }
    }

    pub fn init(&self, _config_json: &str) -> Result<()> {
        self.validate_paths()?;
        Ok(())
    }

    pub fn transform_markdown(&self, _path: &str, content: String) -> Result<String> {
        Ok(content)
    }

    pub fn on_page_ir(&self, _path: &str, ir_json: String) -> Result<String> {
        Ok(ir_json)
    }

    pub fn on_post_render(&self, _path: &str, html: String) -> Result<String> {
        Ok(html)
    }

    pub fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    pub fn wit_interface(&self) -> &'static str {
        nanoss_plugin_api::PLUGIN_WIT
    }

    fn validate_paths(&self) -> Result<()> {
        for plugin in &self.config.plugin_paths {
            if !plugin.exists() {
                bail!("plugin not found: {}", plugin.display());
            }
        }
        Ok(())
    }
}
