use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../nanoss-plugin-api/wit",
        world: "nanoss-plugin",
    });
}

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
    store: Store<HostState>,
    instance: bindings::NanossPlugin,
}

struct HostState {
    limits: StoreLimits,
}

impl bindings::nanoss::plugin::host::Host for HostState {
    fn log(&mut self, level: String, message: String) {
        eprintln!(
            "{{\"component\":\"plugin\",\"hook\":\"host.log\",\"status\":\"{}\",\"message\":{}}}",
            level,
            json_string(&message)
        );
    }
}

pub struct PluginHost {
    config: PluginHostConfig,
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
        let mut linker = Linker::new(&engine);
        bindings::NanossPlugin::add_to_linker::<HostState, wasmtime::component::HasSelf<HostState>>(
            &mut linker,
            |state| state,
        )
        .context("failed to add plugin host imports to linker")?;

        let fuel_per_call = config.timeout_ms.saturating_mul(10_000).max(100_000);
        let mut plugins = Vec::new();
        for path in &config.plugin_paths {
            let component = Component::from_file(&engine, path)
                .with_context(|| format!("failed to compile plugin component {}", path.display()))?;
            let pre = linker
                .instantiate_pre(&component)
                .with_context(|| format!("failed to pre-instantiate plugin component {}", path.display()))?;
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("unknown-plugin")
                .to_string();
            let mut store = create_store(&engine, &config, fuel_per_call)?;
            let raw_instance = pre
                .instantiate(&mut store)
                .with_context(|| format!("failed to instantiate plugin {}", path.display()))?;
            let instance = bindings::NanossPlugin::new(&mut store, &raw_instance)
                .with_context(|| format!("failed to bind plugin exports {}", path.display()))?;
            plugins.push(PluginComponent {
                name,
                store,
                instance,
            });
        }

        Ok(Self {
            config,
            plugins,
            fuel_per_call,
        })
    }

    pub fn init(&mut self, config_json: &str) -> Result<()> {
        for plugin in &mut self.plugins {
            let started = Instant::now();
            plugin
                .store
                .set_fuel(self.fuel_per_call)
                .with_context(|| format!("failed to set plugin fuel: {}", plugin.name))?;
            plugin
                .instance
                .nanoss_plugin_hooks()
                .call_init(&mut plugin.store, config_json)
                .with_context(|| format!("plugin init failed: {}", plugin.name))?;
            log_plugin_event(&plugin.name, "init", started.elapsed().as_millis(), "ok");
        }
        Ok(())
    }

    pub fn transform_markdown(&mut self, path: &str, content: String) -> Result<String> {
        let mut next = content;
        for plugin in &mut self.plugins {
            let started = Instant::now();
            plugin
                .store
                .set_fuel(self.fuel_per_call)
                .with_context(|| format!("failed to set plugin fuel: {}", plugin.name))?;
            next = plugin
                .instance
                .nanoss_plugin_hooks()
                .call_transform_markdown(&mut plugin.store, path, &next)
                .with_context(|| format!("plugin transform_markdown failed: {}", plugin.name))?;
            log_plugin_event(
                &plugin.name,
                "transform_markdown",
                started.elapsed().as_millis(),
                "ok",
            );
        }
        Ok(next)
    }

    pub fn on_page_ir(&mut self, path: &str, ir_json: String) -> Result<String> {
        let mut next = ir_json;
        for plugin in &mut self.plugins {
            let started = Instant::now();
            plugin
                .store
                .set_fuel(self.fuel_per_call)
                .with_context(|| format!("failed to set plugin fuel: {}", plugin.name))?;
            next = plugin
                .instance
                .nanoss_plugin_hooks()
                .call_on_page_ir(&mut plugin.store, path, &next)
                .with_context(|| format!("plugin on_page_ir failed: {}", plugin.name))?;
            log_plugin_event(&plugin.name, "on_page_ir", started.elapsed().as_millis(), "ok");
        }
        Ok(next)
    }

    pub fn on_post_render(&mut self, path: &str, html: String) -> Result<String> {
        let mut next = html;
        for plugin in &mut self.plugins {
            let started = Instant::now();
            plugin
                .store
                .set_fuel(self.fuel_per_call)
                .with_context(|| format!("failed to set plugin fuel: {}", plugin.name))?;
            next = plugin
                .instance
                .nanoss_plugin_hooks()
                .call_on_post_render(&mut plugin.store, path, &next)
                .with_context(|| format!("plugin on_post_render failed: {}", plugin.name))?;
            log_plugin_event(
                &plugin.name,
                "on_post_render",
                started.elapsed().as_millis(),
                "ok",
            );
        }
        Ok(next)
    }

    pub fn shutdown(&mut self) -> Result<()> {
        for plugin in &mut self.plugins {
            let started = Instant::now();
            plugin
                .store
                .set_fuel(self.fuel_per_call)
                .with_context(|| format!("failed to set plugin fuel: {}", plugin.name))?;
            plugin
                .instance
                .nanoss_plugin_hooks()
                .call_shutdown(&mut plugin.store)
                .with_context(|| format!("plugin shutdown failed: {}", plugin.name))?;
            log_plugin_event(&plugin.name, "shutdown", started.elapsed().as_millis(), "ok");
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

}

fn create_store(engine: &Engine, config: &PluginHostConfig, fuel_per_call: u64) -> Result<Store<HostState>> {
    let mut store = Store::new(
            engine,
            HostState {
                limits: StoreLimitsBuilder::new()
                    .memory_size(config.memory_limit_mb.saturating_mul(1024 * 1024) as usize)
                    .build(),
            },
        );
    store.limiter(|state| &mut state.limits);
    store
        .set_fuel(fuel_per_call)
        .context("failed to set plugin execution fuel")?;
    Ok(store)
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

fn log_plugin_event(plugin: &str, hook: &str, duration_ms: u128, status: &str) {
    eprintln!(
        "{{\"component\":\"plugin\",\"plugin\":\"{}\",\"hook\":\"{}\",\"duration_ms\":{},\"status\":\"{}\"}}",
        plugin, hook, duration_ms, status
    );
}

fn json_string(input: &str) -> String {
    format!("\"{}\"", input.replace('\\', "\\\\").replace('"', "\\\""))
}
