use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use nanoss_core::{build_site, BuildConfig, JsBackend, TailwindBackend, TailwindConfig};
use notify::{RecursiveMode, Watcher};
use semver::Version;
use serde::{Deserialize, Serialize};
use tiny_http::{Response, Server, StatusCode};

const PROJECT_CONFIG_FILE: &str = "nanoss.toml";
const INTERNAL_ROOT_DIR: &str = ".nanoss";
const PLUGIN_REGISTRY_FILE: &str = ".nanoss/plugins/registry.json";
const THEMES_DIR: &str = ".nanoss/themes";
const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "nanoss", version, about = "A modern Rust static site generator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Build(BuildArgs),
    Dev(ServerArgs),
    Server(ServerArgs),
    Deploy(DeployArgs),
    GenerateCi(GenerateCiArgs),
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    Theme {
        #[command(subcommand)]
        command: ThemeCommand,
    },
}

#[derive(Clone, Args)]
struct BuildArgs {
    #[arg(long, default_value = "content")]
    content_dir: PathBuf,
    #[arg(long, default_value = "public")]
    output_dir: PathBuf,
    #[arg(long)]
    template_dir: Option<PathBuf>,
    #[arg(long)]
    theme: Option<String>,
    #[arg(long, default_value_t = false)]
    check_external_links: bool,
    #[arg(long, default_value_t = false)]
    fail_on_broken_links: bool,
    #[arg(long = "plugin")]
    plugin_paths: Vec<PathBuf>,
    #[arg(long, value_enum, default_value_t = JsBackendArg::Passthrough)]
    js_backend: JsBackendArg,
    #[arg(long)]
    tailwind_input: Option<PathBuf>,
    #[arg(long)]
    tailwind_output: Option<PathBuf>,
    #[arg(long, default_value = "tailwindcss")]
    tailwind_bin: String,
    #[arg(long, default_value_t = true)]
    tailwind_minify: bool,
    #[arg(long, value_enum, default_value_t = TailwindBackendArg::Standalone)]
    tailwind_backend: TailwindBackendArg,
    #[arg(long, default_value_t = false)]
    enable_ai_index: bool,
    #[arg(long)]
    base_path: Option<String>,
    #[arg(long)]
    site_domain: Option<String>,
}

#[derive(Clone, Args)]
struct ServerArgs {
    #[command(flatten)]
    build: BuildArgs,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 1111)]
    port: u16,
    #[arg(long, default_value_t = true)]
    watch: bool,
}

#[derive(Args)]
struct DeployArgs {
    #[arg(value_enum)]
    target: DeployTargetArg,
    #[arg(long, default_value = "public")]
    output_dir: PathBuf,
}

#[derive(Args)]
struct GenerateCiArgs {
    #[arg(value_enum)]
    provider: CiProviderArg,
    #[arg(long, default_value = "public")]
    output_dir: PathBuf,
}

#[derive(Subcommand)]
enum PluginCommand {
    List,
    Install {
        #[arg(long)]
        id: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        source: PathBuf,
        #[arg(long, default_value = HOST_VERSION)]
        min_host_version: String,
        #[arg(long, default_value_t = true)]
        official: bool,
    },
    Enable { id: String },
    Disable { id: String },
    Update {
        id: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        source: PathBuf,
    },
}

#[derive(Subcommand)]
enum ThemeCommand {
    List,
    New { name: String },
    Use { name: String },
    Validate { name: String },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum JsBackendArg {
    Passthrough,
    Esbuild,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TailwindBackendArg {
    Standalone,
    Rswind,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DeployTargetArg {
    Netlify,
    Vercel,
    CloudflarePages,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CiProviderArg {
    Github,
    Gitlab,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProjectConfig {
    #[serde(default)]
    plugins: ProjectPluginsConfig,
    #[serde(default)]
    theme: ProjectThemeConfig,
    #[serde(default)]
    build: ProjectBuildConfig,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProjectPluginsConfig {
    enabled: BTreeSet<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProjectThemeConfig {
    name: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProjectBuildConfig {
    base_path: Option<String>,
    site_domain: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PluginRegistry {
    plugins: BTreeMap<String, PluginRegistryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginRegistryEntry {
    id: String,
    version: String,
    min_host_version: String,
    wasm_path: PathBuf,
    official: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build(args) => run_build(&args),
        Command::Dev(mut args) => {
            args.watch = true;
            run_server(args)
        }
        Command::Server(args) => run_server(args),
        Command::Deploy(args) => run_deploy(args),
        Command::GenerateCi(args) => run_generate_ci(args),
        Command::Plugin { command } => run_plugin(command),
        Command::Theme { command } => run_theme(command),
    }
}

fn run_build(args: &BuildArgs) -> Result<()> {
    let config = load_project_config()?;
    let base_path = args
        .base_path
        .clone()
        .or_else(|| config.build.base_path.clone())
        .unwrap_or_else(|| "/".to_string());
    let site_domain = args
        .site_domain
        .clone()
        .or_else(|| config.build.site_domain.clone());
    let registry = load_plugin_registry()?;

    let mut plugin_paths = args.plugin_paths.clone();
    for id in &config.plugins.enabled {
        if let Some(entry) = registry.plugins.get(id) {
            ensure_plugin_compatible(entry)?;
            plugin_paths.push(entry.wasm_path.clone());
        }
    }
    plugin_paths.sort();
    plugin_paths.dedup();

    let selected_theme = args
        .theme
        .clone()
        .or_else(|| config.theme.name.clone())
        .map(|name| theme_dir_for(&name));
    if let Some(ref theme_dir) = selected_theme {
        validate_theme_dir(theme_dir)
            .with_context(|| format!("selected theme invalid: {}", theme_dir.display()))?;
    }

    let tailwind = match (&args.tailwind_input, &args.tailwind_output) {
        (Some(input_css), Some(output_css)) => Some(TailwindConfig {
            backend: match args.tailwind_backend {
                TailwindBackendArg::Standalone => TailwindBackend::Standalone,
                TailwindBackendArg::Rswind => TailwindBackend::Rswind,
            },
            input_css: input_css.clone(),
            output_css: output_css.clone(),
            binary: args.tailwind_bin.clone(),
            minify: args.tailwind_minify,
        }),
        _ => None,
    };

    let report = build_site(&BuildConfig {
        content_dir: args.content_dir.clone(),
        output_dir: args.output_dir.clone(),
        template_dir: args.template_dir.clone(),
        theme_dir: selected_theme,
        plugin_paths,
        plugin_timeout_ms: 2_000,
        plugin_memory_limit_mb: 128,
        check_external_links: args.check_external_links,
        fail_on_broken_links: args.fail_on_broken_links,
        js_backend: match args.js_backend {
            JsBackendArg::Passthrough => JsBackend::Passthrough,
            JsBackendArg::Esbuild => JsBackend::Esbuild,
        },
        tailwind,
        enable_ai_index: args.enable_ai_index,
        max_frontmatter_bytes: 64 * 1024,
        max_file_bytes: 10 * 1024 * 1024,
        max_total_files: 100_000,
        command_timeout_secs: 120,
        base_path,
        site_domain,
    })?;
    println!(
        "Built {} pages (skipped {}, {} with islands), compiled {} Sass files, copied {} assets, processed {} scripts, tailwind: {}, ai_indexed_pages: {}, checked {} external links ({} broken).",
        report.rendered_pages,
        report.skipped_pages,
        report.island_pages,
        report.compiled_sass,
        report.copied_assets,
        report.processed_scripts,
        report.compiled_tailwind,
        report.ai_indexed_pages,
        report.checked_external_links,
        report.broken_external_links
    );
    Ok(())
}

fn run_server(args: ServerArgs) -> Result<()> {
    run_build(&args.build)?;
    if args.watch {
        spawn_watch_thread(args.build.clone())?;
    }

    let bind_addr = format!("{}:{}", args.host, args.port);
    let server = Server::http(&bind_addr)
        .map_err(|err| anyhow::anyhow!("failed to bind server at {}: {}", bind_addr, err))?;
    println!("Serving {} at http://{}", args.build.output_dir.display(), bind_addr);
    loop {
        let request = server.recv().context("server failed to receive request")?;
        handle_static_request(request, &args.build.output_dir)?;
    }
}

fn run_deploy(args: DeployArgs) -> Result<()> {
    match args.target {
        DeployTargetArg::Netlify => {
            fs::write(
                "netlify.toml",
                format!(
                    "[build]\ncommand = \"cargo run -p nanoss-cli -- build --output-dir {}\"\npublish = \"{}\"\n",
                    args.output_dir.display(),
                    args.output_dir.display()
                ),
            )
            .context("failed to write netlify.toml")?;
            println!("Generated netlify.toml");
        }
        DeployTargetArg::Vercel => {
            fs::write(
                "vercel.json",
                format!(
                    "{{\n  \"buildCommand\": \"cargo run -p nanoss-cli -- build --output-dir {}\",\n  \"outputDirectory\": \"{}\"\n}}\n",
                    args.output_dir.display(),
                    args.output_dir.display()
                ),
            )
            .context("failed to write vercel.json")?;
            println!("Generated vercel.json");
        }
        DeployTargetArg::CloudflarePages => {
            fs::write(
                "wrangler.toml",
                format!(
                    "name = \"nanoss-site\"\ncompatibility_date = \"2026-01-01\"\n[pages]\nbuild_output_dir = \"{}\"\n",
                    args.output_dir.display()
                ),
            )
            .context("failed to write wrangler.toml")?;
            println!("Generated wrangler.toml");
        }
    }
    Ok(())
}

fn run_generate_ci(args: GenerateCiArgs) -> Result<()> {
    match args.provider {
        CiProviderArg::Github => {
            let workflow_path = PathBuf::from(".github/workflows/nanoss.yml");
            if let Some(parent) = workflow_path.parent() {
                fs::create_dir_all(parent).context("failed to create github workflow directory")?;
            }
            let yaml = format!(
                "name: nanoss\non: [push, pull_request]\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - uses: dtolnay/rust-toolchain@stable\n      - run: cargo run -p nanoss-cli -- build --output-dir {}\n      - run: bash scripts/bench_gate.sh bench/thresholds.toml\n      - uses: actions/upload-artifact@v4\n        with:\n          name: site\n          path: {}\n",
                args.output_dir.display(),
                args.output_dir.display()
            );
            fs::write(&workflow_path, yaml).context("failed to write github ci file")?;
            println!("Generated {}", workflow_path.display());
        }
        CiProviderArg::Gitlab => {
            let yaml = format!(
                "stages:\n  - build\nbuild:\n  stage: build\n  image: rust:latest\n  script:\n    - cargo run -p nanoss-cli -- build --output-dir {}\n    - bash scripts/bench_gate.sh bench/thresholds.toml\n  artifacts:\n    paths:\n      - {}\n",
                args.output_dir.display(),
                args.output_dir.display()
            );
            fs::write(".gitlab-ci.yml", yaml).context("failed to write .gitlab-ci.yml")?;
            println!("Generated .gitlab-ci.yml");
        }
    }
    Ok(())
}

fn run_plugin(command: PluginCommand) -> Result<()> {
    match command {
        PluginCommand::List => {
            let project = load_project_config()?;
            let registry = load_plugin_registry()?;
            if registry.plugins.is_empty() {
                println!("No plugins installed.");
            } else {
                for (id, entry) in registry.plugins {
                    let enabled = if project.plugins.enabled.contains(&id) {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    println!("{id} {} ({enabled}) -> {}", entry.version, entry.wasm_path.display());
                }
            }
        }
        PluginCommand::Install {
            id,
            version,
            source,
            min_host_version,
            official,
        } => {
            let entry = install_plugin(&id, &version, &source, &min_host_version, official)?;
            println!("Installed plugin {}@{} -> {}", entry.id, entry.version, entry.wasm_path.display());
        }
        PluginCommand::Enable { id } => {
            let registry = load_plugin_registry()?;
            if !registry.plugins.contains_key(&id) {
                bail!("plugin '{}' is not installed", id);
            }
            let mut config = load_project_config()?;
            config.plugins.enabled.insert(id.clone());
            save_project_config(&config)?;
            println!("Enabled plugin {id}");
        }
        PluginCommand::Disable { id } => {
            let mut config = load_project_config()?;
            config.plugins.enabled.remove(&id);
            save_project_config(&config)?;
            println!("Disabled plugin {id}");
        }
        PluginCommand::Update { id, version, source } => {
            let entry = install_plugin(&id, &version, &source, HOST_VERSION, true)?;
            println!("Updated plugin {}@{} -> {}", entry.id, entry.version, entry.wasm_path.display());
        }
    }
    Ok(())
}

fn run_theme(command: ThemeCommand) -> Result<()> {
    match command {
        ThemeCommand::List => {
            let config = load_project_config()?;
            let current = config.theme.name.unwrap_or_default();
            let themes_dir = PathBuf::from(THEMES_DIR);
            fs::create_dir_all(&themes_dir).context("failed to ensure themes directory")?;
            for entry in fs::read_dir(&themes_dir).context("failed to read themes directory")? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let marker = if name == current { "*" } else { " " };
                println!("{marker} {name}");
            }
        }
        ThemeCommand::New { name } => {
            let dir = theme_dir_for(&name);
            if dir.exists() {
                bail!("theme already exists: {}", dir.display());
            }
            fs::create_dir_all(dir.join("templates")).context("failed to create theme template dir")?;
            fs::create_dir_all(dir.join("static")).context("failed to create theme static dir")?;
            fs::write(
                dir.join("theme.toml"),
                format!("name = \"{}\"\nversion = \"0.1.0\"\n", name),
            )
            .context("failed to write theme.toml")?;
            fs::write(
                dir.join("templates/page.html"),
                "<!doctype html><html><head><meta charset=\"utf-8\" /><title>{{ title }}</title></head><body>{{ content | safe }}</body></html>\n",
            )
            .context("failed to write theme template")?;
            println!("Created theme at {}", dir.display());
        }
        ThemeCommand::Use { name } => {
            let dir = theme_dir_for(&name);
            validate_theme_dir(&dir)?;
            let mut config = load_project_config()?;
            config.theme.name = Some(name.clone());
            save_project_config(&config)?;
            println!("Theme '{}' is now active", name);
        }
        ThemeCommand::Validate { name } => {
            let dir = theme_dir_for(&name);
            validate_theme_dir(&dir)?;
            println!("Theme '{}' is valid", name);
        }
    }
    Ok(())
}

fn ensure_plugin_compatible(entry: &PluginRegistryEntry) -> Result<()> {
    let host = Version::parse(HOST_VERSION).context("invalid host semver")?;
    let min = Version::parse(&entry.min_host_version)
        .with_context(|| format!("invalid plugin min_host_version for {}", entry.id))?;
    if min > host {
        bail!(
            "plugin {} requires host >= {}, current {}",
            entry.id,
            entry.min_host_version,
            HOST_VERSION
        );
    }
    Ok(())
}

fn install_plugin(
    id: &str,
    version: &str,
    source: &Path,
    min_host_version: &str,
    official: bool,
) -> Result<PluginRegistryEntry> {
    if source.extension().and_then(|ext| ext.to_str()) != Some("wasm") {
        bail!("plugin source must be a .wasm file");
    }
    let target = PathBuf::from(INTERNAL_ROOT_DIR)
        .join("plugins")
        .join("store")
        .join(id)
        .join(version)
        .join(format!("{id}.wasm"));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).context("failed to create plugin install directory")?;
    }
    fs::copy(source, &target)
        .with_context(|| format!("failed to copy plugin {} -> {}", source.display(), target.display()))?;

    let entry = PluginRegistryEntry {
        id: id.to_string(),
        version: version.to_string(),
        min_host_version: min_host_version.to_string(),
        wasm_path: target,
        official,
    };
    ensure_plugin_compatible(&entry)?;

    let mut registry = load_plugin_registry()?;
    registry.plugins.insert(id.to_string(), entry.clone());
    save_plugin_registry(&registry)?;
    Ok(entry)
}

fn load_project_config() -> Result<ProjectConfig> {
    let path = PathBuf::from(PROJECT_CONFIG_FILE);
    if !path.exists() {
        return Ok(ProjectConfig::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("failed to read {}", PROJECT_CONFIG_FILE))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse {}", PROJECT_CONFIG_FILE))
}

fn save_project_config(config: &ProjectConfig) -> Result<()> {
    let raw = toml::to_string_pretty(config).context("failed to serialize project config")?;
    fs::write(PROJECT_CONFIG_FILE, raw).with_context(|| format!("failed to write {}", PROJECT_CONFIG_FILE))
}

fn load_plugin_registry() -> Result<PluginRegistry> {
    let path = PathBuf::from(PLUGIN_REGISTRY_FILE);
    if !path.exists() {
        return Ok(PluginRegistry::default());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read plugin registry {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse plugin registry {}", path.display()))
}

fn save_plugin_registry(registry: &PluginRegistry) -> Result<()> {
    let path = PathBuf::from(PLUGIN_REGISTRY_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(registry).context("failed to serialize plugin registry")?;
    fs::write(&path, raw).with_context(|| format!("failed to write {}", path.display()))
}

fn theme_dir_for(name: &str) -> PathBuf {
    PathBuf::from(THEMES_DIR).join(name)
}

fn validate_theme_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("theme directory not found: {}", path.display());
    }
    if !path.join("theme.toml").exists() {
        bail!("theme missing theme.toml: {}", path.display());
    }
    if !path.join("templates").join("page.html").exists() {
        bail!("theme missing templates/page.html: {}", path.display());
    }
    Ok(())
}

fn spawn_watch_thread(build: BuildArgs) -> Result<()> {
    let (event_tx, event_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = event_tx.send(res);
    })
    .context("failed to create file watcher")?;

    watcher
        .watch(&build.content_dir, RecursiveMode::Recursive)
        .context("failed to watch content directory")?;
    if let Some(ref template_dir) = build.template_dir {
        watcher
            .watch(template_dir, RecursiveMode::Recursive)
            .context("failed to watch template directory")?;
    }

    let selected_theme = load_project_config()?.theme.name;
    if let Some(theme_name) = selected_theme {
        let theme_dir = theme_dir_for(&theme_name);
        if theme_dir.exists() {
            watcher
                .watch(&theme_dir, RecursiveMode::Recursive)
                .context("failed to watch theme directory")?;
        }
    }

    thread::spawn(move || {
        let _watcher_guard = watcher;
        loop {
            match event_rx.recv() {
                Ok(Ok(_)) => {
                    thread::sleep(Duration::from_millis(100));
                    if let Err(err) = run_build(&build) {
                        eprintln!("rebuild failed: {err:#}");
                    } else {
                        println!("rebuild succeeded");
                    }
                }
                Ok(Err(err)) => eprintln!("watch error: {err:#}"),
                Err(_) => break,
            }
        }
    });
    Ok(())
}

fn handle_static_request(request: tiny_http::Request, output_dir: &Path) -> Result<()> {
    let raw = request.url().trim_start_matches('/');
    let rel = if raw.is_empty() { "index.html" } else { raw };
    let mut target = output_dir.join(rel);
    if target.is_dir() {
        target = target.join("index.html");
    }
    if !target.exists() {
        request
            .respond(Response::empty(StatusCode(404)))
            .context("failed to send 404 response")?;
        return Ok(());
    }
    let mut file = fs::File::open(&target).with_context(|| format!("failed to open {}", target.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", target.display()))?;
    request
        .respond(Response::from_data(bytes))
        .context("failed to send file response")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    fn cwd_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_temp_cwd<F>(f: F) -> Result<()>
    where
        F: FnOnce() -> Result<()>,
    {
        let _guard = cwd_lock().lock().expect("lock poisoned");
        let current = std::env::current_dir().context("failed to read current dir")?;
        let dir = tempdir().context("failed to create tempdir")?;
        std::env::set_current_dir(dir.path()).context("failed to enter tempdir")?;
        let run = f();
        std::env::set_current_dir(current).context("failed to restore cwd")?;
        run
    }

    #[test]
    fn plugin_install_and_enable_roundtrip() -> Result<()> {
        with_temp_cwd(|| {
            let wasm = PathBuf::from("demo.wasm");
            fs::write(&wasm, [0u8, 97, 115, 109]).context("failed to write wasm fixture")?;

            let entry = install_plugin("demo", "0.1.0", &wasm, HOST_VERSION, true)?;
            assert!(entry.wasm_path.exists());

            run_plugin(PluginCommand::Enable {
                id: "demo".to_string(),
            })?;
            let config = load_project_config()?;
            assert!(config.plugins.enabled.contains("demo"));
            Ok(())
        })
    }

    #[test]
    fn theme_validate_reports_missing_files() -> Result<()> {
        with_temp_cwd(|| {
            let dir = theme_dir_for("broken");
            fs::create_dir_all(&dir).context("failed to create broken theme dir")?;
            let err = validate_theme_dir(&dir).expect_err("expected validation failure");
            assert!(err.to_string().contains("theme.toml"));
            Ok(())
        })
    }

    #[test]
    fn generate_ci_writes_github_workflow() -> Result<()> {
        with_temp_cwd(|| {
            run_generate_ci(GenerateCiArgs {
                provider: CiProviderArg::Github,
                output_dir: PathBuf::from("public"),
            })?;
            let workflow = PathBuf::from(".github/workflows/nanoss.yml");
            assert!(workflow.exists());
            let raw = fs::read_to_string(&workflow).context("failed to read generated workflow")?;
            assert!(raw.contains("cargo run -p nanoss-cli -- build"));
            Ok(())
        })
    }
}
