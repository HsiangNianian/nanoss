use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use wasmtime::component::Component;
use wasmtime::{Config, Engine};

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

struct PluginComponent {
    name: String,
    component: Component,
}

pub struct PluginHost {
    config: PluginHostConfig,
    engine: Engine,
    plugins: Vec<PluginComponent>,
    fuel_per_call: u64,
}

impl PluginHost {
    pub fn new(config: PluginHostConfig) -> Result<Self> {
        validate_paths(&config.plugin_paths)?;

        let mut engine_config = Config::new();
        engine_config.wasm_component_model(true);
        engine_config.consume_fuel(true);
        let engine = Engine::new(&engine_config).context("failed to create wasmtime engine")?;

        let mut plugins = Vec::new();
        for path in &config.plugin_paths {
            let component = Component::from_file(&engine, path)
                .with_context(|| format!("failed to compile plugin component {}", path.display()))?;
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("unknown-plugin")
                .to_string();
            plugins.push(PluginComponent { name, component });
        }

        let fuel_per_call = config.timeout_ms.saturating_mul(10_000).max(100_000);
        Ok(Self {
            config,
            engine,
            plugins,
            fuel_per_call,
        })
    }

    pub fn init(&self, _config_json: &str) -> Result<()> {
        for plugin in &self.plugins {
            self.preflight_plugin(plugin)
                .with_context(|| format!("plugin preflight failed during init: {}", plugin.name))?;
        }
        Ok(())
    }

    pub fn transform_markdown(&self, path: &str, content: String) -> Result<String> {
        for plugin in &self.plugins {
            self.preflight_plugin(plugin).with_context(|| {
                format!(
                    "plugin preflight failed before transform_markdown for {} ({path})",
                    plugin.name
                )
            })?;
        }
        Ok(content)
    }

    pub fn on_page_ir(&self, path: &str, ir_json: String) -> Result<String> {
        for plugin in &self.plugins {
            self.preflight_plugin(plugin).with_context(|| {
                format!(
                    "plugin preflight failed before on_page_ir for {} ({path})",
                    plugin.name
                )
            })?;
        }
        Ok(ir_json)
    }

    pub fn on_post_render(&self, path: &str, html: String) -> Result<String> {
        for plugin in &self.plugins {
            self.preflight_plugin(plugin).with_context(|| {
                format!(
                    "plugin preflight failed before on_post_render for {} ({path})",
                    plugin.name
                )
            })?;
        }
        Ok(html)
    }

    pub fn shutdown(&self) -> Result<()> {
        for plugin in &self.plugins {
            self.preflight_plugin(plugin)
                .with_context(|| format!("plugin preflight failed during shutdown: {}", plugin.name))?;
        }
        Ok(())
    }

    pub fn wit_interface(&self) -> &'static str {
        nanoss_plugin_api::PLUGIN_WIT
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    pub fn timeout_ms(&self) -> u64 {
        self.config.timeout_ms
    }

    fn preflight_plugin(&self, plugin: &PluginComponent) -> Result<()> {
        let _ = &self.engine;
        let _ = &plugin.component;
        let _ = self.fuel_per_call;
        Ok(())
    }
}

fn validate_paths(paths: &[PathBuf]) -> Result<()> {
    for plugin in paths {
        if !plugin.exists() {
            bail!("plugin not found: {}", plugin.display());
        }
        if !is_component_candidate(plugin) {
            bail!("plugin must be a .wasm component file: {}", plugin.display());
        }
    }
    Ok(())
}

fn is_component_candidate(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("wasm")
}
