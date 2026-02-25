use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use nanoss_core::{
    build_site, BuildConfig, BuildScope, I18nConfig, ImageBuildConfig, JsBackend, ProjectConfig,
    TailwindBackend, TailwindConfig,
};
use nanoss_metrics::StdoutMetricsCollector;
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
#[command(
    name = "nanoss",
    version,
    about = "A modern Rust static site generator"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Build(BuildArgs),
    Init(InitArgs),
    New(NewArgs),
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
    #[arg(long, default_value = "static")]
    static_dir: PathBuf,
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
    #[arg(long, default_value_t = false)]
    include_drafts: bool,
    #[arg(long)]
    config: Option<PathBuf>,
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
    #[arg(long)]
    mount_path: Option<String>,
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

#[derive(Args)]
struct InitArgs {
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    #[arg(long, short = 'f', default_value_t = false)]
    force: bool,
}

#[derive(Args)]
struct NewArgs {
    #[command(subcommand)]
    kind: Option<NewCommand>,
    name: Option<String>,
    #[arg(long, short = 'f', default_value_t = false)]
    force: bool,
}

#[derive(Subcommand)]
enum NewCommand {
    Site {
        name: String,
        #[arg(long, short = 'f', default_value_t = false)]
        force: bool,
    },
    Theme {
        name: String,
        #[arg(long, short = 'f', default_value_t = false)]
        force: bool,
    },
    Page {
        path: String,
        #[arg(long, short = 'f', default_value_t = false)]
        force: bool,
    },
    Plugin {
        name: String,
        #[arg(long, short = 'f', default_value_t = false)]
        force: bool,
    },
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
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
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
        Command::Init(args) => run_init(args),
        Command::New(args) => run_new(args),
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

fn run_init(args: InitArgs) -> Result<()> {
    create_site_scaffold(&args.dir, args.force)?;
    println!("Initialized Nanoss starter at {}", args.dir.display());
    Ok(())
}

fn run_new(args: NewArgs) -> Result<()> {
    let force = args.force;
    match (args.kind, args.name) {
        (
            Some(NewCommand::Site {
                name,
                force: sub_force,
            }),
            None,
        ) => create_new_site_scaffold(Path::new(&name), force || sub_force),
        (
            Some(NewCommand::Theme {
                name,
                force: sub_force,
            }),
            None,
        ) => create_theme_scaffold(&name, force || sub_force),
        (
            Some(NewCommand::Page {
                path,
                force: sub_force,
            }),
            None,
        ) => create_page_scaffold(&path, force || sub_force),
        (
            Some(NewCommand::Plugin {
                name,
                force: sub_force,
            }),
            None,
        ) => create_plugin_scaffold(Path::new("plugins"), &name, force || sub_force),
        (None, Some(name)) => {
            let selected = prompt_new_kind(&name, io::stdin().lock(), io::stdout())?;
            match selected {
                NewKind::Site => create_new_site_scaffold(Path::new(&name), force),
                NewKind::Theme => create_theme_scaffold(&name, force),
                NewKind::Page => create_page_scaffold(&name, force),
                NewKind::Plugin => create_plugin_scaffold(Path::new("plugins"), &name, force),
            }
        }
        (None, None) => {
            bail!("usage: nanoss new <name> or nanoss new <site|theme|page|plugin> <value>")
        }
        _ => bail!("when using explicit new type, do not pass extra unnamed args"),
    }?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum NewKind {
    Site,
    Theme,
    Page,
    Plugin,
}

fn prompt_new_kind<R: BufRead, W: Write>(
    name: &str,
    mut input: R,
    mut output: W,
) -> Result<NewKind> {
    writeln!(
        output,
        "Select type for '{}':\n  1) site\n  2) theme\n  3) page\n  4) plugin",
        name
    )
    .context("failed to write prompt")?;
    write!(output, "Enter choice [1-4]: ").context("failed to write prompt")?;
    output.flush().context("failed to flush prompt")?;

    for _ in 0..3 {
        let mut line = String::new();
        input
            .read_line(&mut line)
            .context("failed to read selection")?;
        let choice = line.trim().to_ascii_lowercase();
        let selected = match choice.as_str() {
            "1" | "site" => Some(NewKind::Site),
            "2" | "theme" => Some(NewKind::Theme),
            "3" | "page" => Some(NewKind::Page),
            "4" | "plugin" => Some(NewKind::Plugin),
            _ => None,
        };
        if let Some(kind) = selected {
            return Ok(kind);
        }
        writeln!(output, "invalid choice, please enter 1/2/3/4")
            .context("failed to write prompt")?;
        write!(output, "Enter choice [1-4]: ").context("failed to write prompt")?;
        output.flush().context("failed to flush prompt")?;
    }
    bail!("too many invalid selections")
}

fn run_build(args: &BuildArgs) -> Result<()> {
    run_build_with_scope(args, BuildScope::Full)
}

fn run_build_with_scope(args: &BuildArgs, build_scope: BuildScope) -> Result<()> {
    tracing::info!(event_name = "cli.build.start", scope = ?build_scope);
    let config = load_project_config_for_build(args)?;
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

    let plugin_init_config_json = config
        .plugins
        .config
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("failed to serialize [plugins].config as JSON")?
        .unwrap_or_else(|| "{}".to_string());

    let report = build_site(&BuildConfig {
        content_dir: args.content_dir.clone(),
        static_dir: args.static_dir.clone(),
        output_dir: args.output_dir.clone(),
        template_dir: args.template_dir.clone(),
        theme_dir: selected_theme,
        plugin_paths,
        plugin_init_config_json,
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
        images: ImageBuildConfig {
            enabled: config.build.images.enabled,
            generate_webp: config.build.images.generate_webp,
            generate_avif: config.build.images.generate_avif,
            widths: config.build.images.widths.clone(),
        },
        remote_data_sources: config.build.data_sources.clone(),
        i18n: I18nConfig {
            locales: config.build.i18n.locales.clone(),
            default_locale: config.build.i18n.default_locale.clone(),
            prefix_default_locale: config.build.i18n.prefix_default_locale,
        },
        build_scope,
        include_drafts: args.include_drafts,
        metrics: Some(std::sync::Arc::new(StdoutMetricsCollector)),
    })?;
    println!(
        "Built {} pages (skipped {}, {} with islands), compiled {} Sass files, copied {} assets, processed {} scripts, processed {} images, tailwind: {}, ai_indexed_pages: {}, checked {} external links ({} broken).",
        report.rendered_pages,
        report.skipped_pages,
        report.island_pages,
        report.compiled_sass,
        report.copied_assets,
        report.processed_scripts,
        report.processed_images,
        report.compiled_tailwind,
        report.ai_indexed_pages,
        report.checked_external_links,
        report.broken_external_links
    );
    tracing::info!(
        event_name = "cli.build.finish",
        rendered_pages = report.rendered_pages,
        skipped_pages = report.skipped_pages
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
    let project = load_project_config()?;
    let mount_path = normalize_mount_path(
        args.mount_path
            .clone()
            .or(project.server.mount_path)
            .as_deref()
            .unwrap_or("/"),
    )?;
    println!(
        "Serving {} at http://{}{}",
        args.build.output_dir.display(),
        bind_addr,
        if mount_path == "/" { "" } else { &mount_path }
    );
    loop {
        let request = server.recv().context("server failed to receive request")?;
        handle_static_request(request, &args.build.output_dir, &mount_path)?;
    }
}

#[derive(Debug, Clone, Copy)]
enum RebuildScope {
    Full,
    SinglePage,
    AssetsOnly,
    Template,
    Mixed,
}

fn run_build_scoped(args: &BuildArgs, scope: RebuildScope, changed: &[PathBuf]) -> Result<()> {
    println!("{{\"stage\":\"rebuild_scope\",\"scope\":\"{:?}\"}}", scope);
    let build_scope = match scope {
        RebuildScope::SinglePage => changed
            .iter()
            .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
            .cloned()
            .map(|path| BuildScope::SinglePage { path })
            .unwrap_or(BuildScope::Full),
        RebuildScope::AssetsOnly => BuildScope::AssetsOnly {
            paths: changed.to_vec(),
        },
        RebuildScope::Template | RebuildScope::Mixed | RebuildScope::Full => BuildScope::Full,
    };
    run_build_with_scope(args, build_scope)
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
                    println!(
                        "{id} {} ({enabled}) -> {}",
                        entry.version,
                        entry.wasm_path.display()
                    );
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
            println!(
                "Installed plugin {}@{} -> {}",
                entry.id,
                entry.version,
                entry.wasm_path.display()
            );
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
        PluginCommand::Update {
            id,
            version,
            source,
        } => {
            let entry = install_plugin(&id, &version, &source, HOST_VERSION, true)?;
            println!(
                "Updated plugin {}@{} -> {}",
                entry.id,
                entry.version,
                entry.wasm_path.display()
            );
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
            create_theme_scaffold(&name, false)?;
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

fn create_new_site_scaffold(root: &Path, force: bool) -> Result<()> {
    if root.exists() && !force {
        bail!(
            "target directory already exists: {} (use -f/--force to continue)",
            root.display()
        );
    }
    create_site_scaffold(root, force)
}

fn create_site_scaffold(root: &Path, force: bool) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("failed to create {}", root.display()))?;
    create_if_missing(
        &root.join(PROJECT_CONFIG_FILE),
        "[build]\nbase_path = \"/\"\n# site_domain = \"https://example.com\"\n\n[build.images]\nenabled = true\ngenerate_webp = true\ngenerate_avif = false\nwidths = [480, 768, 1200]\n\n[build.i18n]\nlocales = [\"en\", \"zh\"]\ndefault_locale = \"en\"\nprefix_default_locale = false\n\n[server]\n# mount_path = \"/nanoss\"\n",
        force,
    )?;
    create_if_missing(
        &root.join("content/index.md"),
        "---\ntitle: Home\n---\n\n# Welcome to Nanoss\n\nThis site is scaffolded by `nanoss init/new`.\n",
        force,
    )?;
    create_if_missing(
        &root.join("content/styles/site.css"),
        starter_site_css(),
        force,
    )?;
    create_if_missing(
        &root.join("content/scripts/site.js"),
        "console.log(\"nanoss starter loaded\");\n",
        force,
    )?;
    create_if_missing(&root.join("static/.gitkeep"), "", force)?;
    create_if_missing(
        &root.join("templates/page.html"),
        starter_page_template(),
        force,
    )?;
    Ok(())
}

fn create_theme_scaffold(name: &str, force: bool) -> Result<()> {
    let dir = theme_dir_for(name);
    if dir.exists() && !force {
        bail!(
            "theme already exists: {} (use -f/--force to continue)",
            dir.display()
        );
    }
    fs::create_dir_all(dir.join("templates")).context("failed to create theme template dir")?;
    fs::create_dir_all(dir.join("static")).context("failed to create theme static dir")?;
    create_if_missing(
        &dir.join("theme.toml"),
        &format!("name = \"{}\"\nversion = \"0.1.0\"\n", name),
        force,
    )?;
    create_if_missing(
        &dir.join("templates/page.html"),
        starter_theme_template(),
        force,
    )?;
    create_if_missing(&dir.join("static/theme.css"), starter_theme_css(), force)?;
    create_if_missing(&dir.join("static/.gitkeep"), "", force)?;
    println!("Created theme at {}", dir.display());
    Ok(())
}

fn create_page_scaffold(path: &str, force: bool) -> Result<()> {
    let mut rel = PathBuf::from(path);
    if rel.extension().is_none() {
        rel.set_extension("md");
    }
    let target = PathBuf::from("content").join(&rel);
    let stem = rel
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("new-page")
        .to_string();
    let title = stem.replace('-', " ");
    let body = format!(
        "---\ntitle: {}\nslug: {}\n---\n\n# {}\n\nWrite your content here.\n",
        title, stem, title
    );
    create_if_missing(&target, &body, force)?;
    println!("Created page {}", target.display());
    Ok(())
}

fn create_plugin_scaffold(root: &Path, name: &str, force: bool) -> Result<()> {
    let dir = root.join(name);
    if dir.exists() && !force {
        bail!(
            "plugin scaffold already exists: {} (use -f/--force to continue)",
            dir.display()
        );
    }
    fs::create_dir_all(dir.join("src"))
        .with_context(|| format!("failed to create {}", dir.display()))?;
    let crate_name = format!("nanoss-plugin-{}", name.replace('-', "_"));
    create_if_missing(
        &dir.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
            crate_name
        ),
        force,
    )?;
    create_if_missing(
        &dir.join("src/lib.rs"),
        "//! Minimal Nanoss plugin scaffold.\n// TODO: implement exports based on nanoss plugin WIT bindings.\n",
        force,
    )?;
    create_if_missing(
        &dir.join("README.md"),
        "# Nanoss Plugin Scaffold\n\nBuild this crate for wasm32 and install with `nanoss plugin install`.\n",
        force,
    )?;
    println!("Created plugin scaffold {}", dir.display());
    Ok(())
}

fn create_if_missing(path: &Path, content: &str, force: bool) -> Result<()> {
    if path.exists() {
        if force {
            fs::write(path, content)
                .with_context(|| format!("failed to overwrite {}", path.display()))?;
            println!("overwrite {}", path.display());
            return Ok(());
        }
        println!("skip existing {}", path.display());
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

fn starter_page_template() -> &'static str {
    r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta name="description" content="{{ seo.description }}" />
  <link rel="canonical" href="{{ seo.canonical }}" />
  <meta property="og:title" content="{{ seo.title }}" />
  <meta property="og:description" content="{{ seo.description }}" />
  <meta property="og:type" content="article" />
  <meta property="og:url" content="{{ seo.canonical }}" />
  {% if seo.og_image %}
  <meta property="og:image" content="{{ seo.og_image }}" />
  {% endif %}
  <meta name="twitter:card" content="{{ seo.twitter_card }}" />
  <meta name="twitter:title" content="{{ seo.title }}" />
  <meta name="twitter:description" content="{{ seo.description }}" />
  {% if seo.og_image %}
  <meta name="twitter:image" content="{{ seo.og_image }}" />
  {% endif %}
  {% if seo.noindex %}
  <meta name="robots" content="noindex,nofollow" />
  {% endif %}
  <title>{{ title }}</title>
  {% if seo.json_ld %}
  <script type="application/ld+json">{{ seo.json_ld | safe }}</script>
  {% endif %}
  <link rel="stylesheet" href="{{ base_href_prefix }}/styles/site.css" />
</head>
<body>
  <header class="topbar">
    <div class="container brand">Nanoss</div>
  </header>
  <main class="container layout">
    {% if toc %}
    <aside class="toc-card" aria-label="Table of contents">
      <h2>On this page</h2>
      {{ toc | safe }}
    </aside>
    {% endif %}
    <article class="content-card">
      {{ content | safe }}
    </article>
  </main>
  <script src="{{ base_href_prefix }}/scripts/site.js"></script>
</body>
</html>
"#
}

fn starter_site_css() -> &'static str {
    r#":root {
  --bg: #0b1020;
  --panel: #111936;
  --panel-2: #162345;
  --text: #e8ecf8;
  --muted: #98a2b3;
  --accent: #7dd3fc;
  --line: #26314f;
}

* { box-sizing: border-box; }
body {
  margin: 0;
  font-family: Inter, system-ui, -apple-system, Segoe UI, Roboto, Helvetica, Arial, sans-serif;
  background: radial-gradient(circle at 20% 0%, #101a38 0%, var(--bg) 45%);
  color: var(--text);
}

.container { max-width: 1080px; margin: 0 auto; padding: 0 20px; }
.topbar {
  border-bottom: 1px solid var(--line);
  background: rgba(11, 16, 32, 0.7);
  backdrop-filter: blur(6px);
}
.brand { font-weight: 700; padding: 16px 20px; letter-spacing: 0.2px; }

.layout {
  display: grid;
  gap: 18px;
  grid-template-columns: minmax(220px, 280px) 1fr;
  padding-top: 22px;
  padding-bottom: 36px;
}
.toc-card, .content-card {
  border: 1px solid var(--line);
  border-radius: 14px;
  background: linear-gradient(180deg, var(--panel-2), var(--panel));
}
.toc-card { padding: 14px 16px; position: sticky; top: 16px; height: fit-content; }
.toc-card h2 { margin: 0 0 10px; font-size: 14px; color: var(--muted); text-transform: uppercase; letter-spacing: 0.6px; }
.content-card { padding: 20px 24px; min-height: 70vh; }

a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
pre {
  overflow-x: auto;
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 12px;
  background: #0b1329;
}
code { font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace; }

@media (max-width: 900px) {
  .layout { grid-template-columns: 1fr; }
  .toc-card { position: static; }
}

@media (prefers-color-scheme: light) {
  :root {
    --bg: #f4f7fb;
    --panel: #ffffff;
    --panel-2: #ffffff;
    --text: #0f172a;
    --muted: #475569;
    --accent: #0369a1;
    --line: #dbe4f0;
  }
  body {
    background: linear-gradient(180deg, #f7fbff 0%, var(--bg) 38%);
  }
  .topbar {
    background: rgba(255, 255, 255, 0.75);
  }
  pre {
    background: #f8fafc;
  }
}
"#
}

fn starter_theme_template() -> &'static str {
    r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{{ title }}</title>
  <link rel="stylesheet" href="{{ base_href_prefix }}/theme.css" />
</head>
<body>
  <main class="container">
    {% if toc %}
    <nav class="toc">{{ toc | safe }}</nav>
    {% endif %}
    <article class="content">{{ content | safe }}</article>
  </main>
</body>
</html>
"#
}

fn starter_theme_css() -> &'static str {
    r#":root { color-scheme: dark light; }
body {
  margin: 0;
  background: #0f172a;
  color: #e5e7eb;
  font-family: Inter, system-ui, sans-serif;
}
.container { max-width: 900px; margin: 0 auto; padding: 28px 20px 48px; }
.toc, .content {
  border: 1px solid #334155;
  border-radius: 12px;
  background: #111827;
}
.toc { padding: 12px 14px; margin-bottom: 14px; }
.content { padding: 20px; }
a { color: #7dd3fc; }
pre { overflow-x: auto; background: #0b1222; border-radius: 10px; padding: 12px; }

@media (prefers-color-scheme: light) {
  body {
    background: #f8fafc;
    color: #0f172a;
  }
  .toc, .content {
    border-color: #dbe4f0;
    background: #ffffff;
  }
  a { color: #0369a1; }
  pre { background: #f1f5f9; }
}
"#
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
    if min.major < host.major {
        eprintln!(
            "warning: plugin {} targets host major {} but current major is {} (legacy compatibility mode)",
            entry.id, min.major, host.major
        );
    }
    if min.minor + 2 < host.minor {
        eprintln!(
            "hint: plugin {} is older than two host minor versions; consider upgrading plugin for latest v2 payload support",
            entry.id
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
    fs::copy(source, &target).with_context(|| {
        format!(
            "failed to copy plugin {} -> {}",
            source.display(),
            target.display()
        )
    })?;

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
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", PROJECT_CONFIG_FILE))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse {}", PROJECT_CONFIG_FILE))
}

fn load_project_config_for_build(args: &BuildArgs) -> Result<ProjectConfig> {
    if let Some(path) = &args.config {
        return load_project_config_from_path(path);
    }
    let content_candidate = args
        .content_dir
        .parent()
        .map(|parent| parent.join(PROJECT_CONFIG_FILE));
    if let Some(path) = content_candidate.as_ref().filter(|p| p.exists()) {
        return load_project_config_from_path(path);
    }
    load_project_config()
}

fn load_project_config_from_path(path: &Path) -> Result<ProjectConfig> {
    if !path.exists() {
        return Ok(ProjectConfig::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read project config {}", path.display()))?;
    toml::from_str(&raw)
        .with_context(|| format!("failed to parse project config {}", path.display()))
}

fn save_project_config(config: &ProjectConfig) -> Result<()> {
    let raw = toml::to_string_pretty(config).context("failed to serialize project config")?;
    fs::write(PROJECT_CONFIG_FILE, raw)
        .with_context(|| format!("failed to write {}", PROJECT_CONFIG_FILE))
}

fn load_plugin_registry() -> Result<PluginRegistry> {
    let path = PathBuf::from(PLUGIN_REGISTRY_FILE);
    if !path.exists() {
        return Ok(PluginRegistry::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read plugin registry {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse plugin registry {}", path.display()))
}

fn save_plugin_registry(registry: &PluginRegistry) -> Result<()> {
    let path = PathBuf::from(PLUGIN_REGISTRY_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let raw =
        serde_json::to_string_pretty(registry).context("failed to serialize plugin registry")?;
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
        if template_dir.exists() {
            watcher
                .watch(template_dir, RecursiveMode::Recursive)
                .context("failed to watch template directory")?;
        } else {
            eprintln!(
                "warning: template directory not found, skipping watch: {}",
                template_dir.display()
            );
        }
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
                Ok(Ok(event)) => {
                    thread::sleep(Duration::from_millis(100));
                    let changed = event.paths;
                    let change_kind = classify_watch_change(&build, &changed);
                    println!(
                        "{{\"stage\":\"watch_rebuild_start\",\"change_kind\":\"{}\",\"changed\":{}}}",
                        change_kind,
                        serde_json::to_string(&changed).unwrap_or_else(|_| "[]".to_string())
                    );
                    let scope = match change_kind {
                        "single_page" => RebuildScope::SinglePage,
                        "assets_only" => RebuildScope::AssetsOnly,
                        "template" => RebuildScope::Template,
                        "mixed" => RebuildScope::Mixed,
                        _ => RebuildScope::Full,
                    };
                    if let Err(err) = run_build_scoped(&build, scope, &changed) {
                        eprintln!("rebuild failed: {err:#}");
                    } else {
                        println!(
                            "{{\"stage\":\"watch_rebuild_finish\",\"change_kind\":\"{}\",\"status\":\"ok\"}}",
                            change_kind
                        );
                    }
                }
                Ok(Err(err)) => eprintln!("watch error: {err:#}"),
                Err(_) => break,
            }
        }
    });
    Ok(())
}

fn classify_watch_change(build: &BuildArgs, changed: &[PathBuf]) -> &'static str {
    if changed.is_empty() {
        return "unknown";
    }
    let is_template = changed.iter().any(|path| {
        path.extension().and_then(|ext| ext.to_str()) == Some("html")
            && (build
                .template_dir
                .as_ref()
                .map(|dir| path.starts_with(dir))
                .unwrap_or(false)
                || path.to_string_lossy().contains(".nanoss/themes/"))
    });
    if is_template {
        return "template";
    }
    let md_paths: Vec<&PathBuf> = changed
        .iter()
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    if md_paths.len() == 1 && changed.len() == 1 {
        return "single_page";
    }
    if md_paths.is_empty() {
        return "assets_only";
    }
    "mixed"
}

fn handle_static_request(
    request: tiny_http::Request,
    output_dir: &Path,
    mount_path: &str,
) -> Result<()> {
    let url_path = request
        .url()
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_start_matches('/');
    let request_path = format!("/{}", url_path);
    let routed = route_for_mount(&request_path, mount_path);
    let Some(route_path) = routed else {
        request
            .respond(Response::empty(StatusCode(404)))
            .context("failed to send 404 response")?;
        return Ok(());
    };
    let raw = route_path.trim_start_matches('/');
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
    let mut file =
        fs::File::open(&target).with_context(|| format!("failed to open {}", target.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", target.display()))?;
    request
        .respond(Response::from_data(bytes))
        .context("failed to send file response")?;
    Ok(())
}

fn normalize_mount_path(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Ok("/".to_string());
    }
    if !trimmed.starts_with('/') {
        bail!("mount_path must start with '/'");
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

fn route_for_mount(request_path: &str, mount_path: &str) -> Option<String> {
    if mount_path == "/" {
        return Some(request_path.to_string());
    }
    if request_path == mount_path {
        return Some("/".to_string());
    }
    let prefix = format!("{}/", mount_path);
    request_path
        .strip_prefix(&prefix)
        .map(|rest| format!("/{}", rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
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

    fn default_build_args() -> BuildArgs {
        BuildArgs {
            content_dir: PathBuf::from("content"),
            static_dir: PathBuf::from("static"),
            output_dir: PathBuf::from("public"),
            template_dir: Some(PathBuf::from("templates")),
            theme: None,
            check_external_links: false,
            fail_on_broken_links: false,
            plugin_paths: Vec::new(),
            js_backend: JsBackendArg::Passthrough,
            tailwind_input: None,
            tailwind_output: None,
            tailwind_bin: "tailwindcss".to_string(),
            tailwind_minify: true,
            tailwind_backend: TailwindBackendArg::Standalone,
            enable_ai_index: false,
            base_path: Some("/".to_string()),
            site_domain: None,
            include_drafts: false,
            config: None,
        }
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

    #[test]
    fn init_creates_starter_and_is_idempotent() -> Result<()> {
        with_temp_cwd(|| {
            run_init(InitArgs {
                dir: PathBuf::from("."),
                force: false,
            })?;
            run_init(InitArgs {
                dir: PathBuf::from("."),
                force: false,
            })?;
            assert!(PathBuf::from("nanoss.toml").exists());
            assert!(PathBuf::from("content/index.md").exists());
            assert!(PathBuf::from("templates/page.html").exists());
            Ok(())
        })
    }

    #[test]
    fn new_subcommands_create_expected_scaffold() -> Result<()> {
        with_temp_cwd(|| {
            run_new(NewArgs {
                kind: Some(NewCommand::Site {
                    name: "demo-site".to_string(),
                    force: false,
                }),
                name: None,
                force: false,
            })?;
            assert!(PathBuf::from("demo-site/nanoss.toml").exists());

            run_new(NewArgs {
                kind: Some(NewCommand::Theme {
                    name: "demo-theme".to_string(),
                    force: false,
                }),
                name: None,
                force: false,
            })?;
            assert!(PathBuf::from(".nanoss/themes/demo-theme/theme.toml").exists());

            run_new(NewArgs {
                kind: Some(NewCommand::Page {
                    path: "guide/getting-started".to_string(),
                    force: false,
                }),
                name: None,
                force: false,
            })?;
            assert!(PathBuf::from("content/guide/getting-started.md").exists());

            run_new(NewArgs {
                kind: Some(NewCommand::Plugin {
                    name: "demo-plugin".to_string(),
                    force: false,
                }),
                name: None,
                force: false,
            })?;
            assert!(PathBuf::from("plugins/demo-plugin/Cargo.toml").exists());
            Ok(())
        })
    }

    #[test]
    fn prompt_new_kind_accepts_numeric_and_text() -> Result<()> {
        let mut out = Vec::new();
        let kind = prompt_new_kind("demo", Cursor::new("2\n"), &mut out)?;
        assert!(matches!(kind, NewKind::Theme));

        let mut out = Vec::new();
        let kind = prompt_new_kind("demo", Cursor::new("plugin\n"), &mut out)?;
        assert!(matches!(kind, NewKind::Plugin));
        Ok(())
    }

    #[test]
    fn init_then_build_succeeds() -> Result<()> {
        with_temp_cwd(|| {
            run_init(InitArgs {
                dir: PathBuf::from("."),
                force: false,
            })?;
            run_build(&default_build_args())?;
            assert!(PathBuf::from("public/index.html").exists());
            Ok(())
        })
    }

    #[test]
    fn new_site_requires_force_when_dir_exists() -> Result<()> {
        with_temp_cwd(|| {
            fs::create_dir_all("demo").context("failed to create existing dir")?;
            let err = run_new(NewArgs {
                kind: Some(NewCommand::Site {
                    name: "demo".to_string(),
                    force: false,
                }),
                name: None,
                force: false,
            })
            .expect_err("expected exists error");
            assert!(err.to_string().contains("already exists"));

            run_new(NewArgs {
                kind: Some(NewCommand::Site {
                    name: "demo".to_string(),
                    force: false,
                }),
                name: None,
                force: true,
            })?;
            assert!(PathBuf::from("demo/nanoss.toml").exists());
            Ok(())
        })
    }

    #[test]
    fn mount_path_routing_and_normalization_work() -> Result<()> {
        assert_eq!(normalize_mount_path("/")?, "/");
        assert_eq!(normalize_mount_path("/nanoss/")?, "/nanoss");
        assert!(normalize_mount_path("nanoss").is_err());

        assert_eq!(route_for_mount("/nanoss", "/nanoss"), Some("/".to_string()));
        assert_eq!(
            route_for_mount("/nanoss/cli.html", "/nanoss"),
            Some("/cli.html".to_string())
        );
        assert_eq!(route_for_mount("/cli.html", "/nanoss"), None);
        Ok(())
    }

    #[test]
    fn build_prefers_content_parent_project_config() -> Result<()> {
        with_temp_cwd(|| {
            fs::write(
                "nanoss.toml",
                "[build]\nbase_path = \"/root-base\"\n\n[build.images]\nenabled = true\ngenerate_webp = false\ngenerate_avif = false\nwidths = []\n",
            )
            .context("failed to write root config")?;
            create_new_site_scaffold(Path::new("examples/e2e"), false)?;
            fs::write(
                "examples/e2e/nanoss.toml",
                "[build]\nbase_path = \"/\"\nsite_domain = \"https://example.com\"\n\n[build.images]\nenabled = true\ngenerate_webp = false\ngenerate_avif = false\nwidths = []\n",
            )
            .context("failed to write e2e config")?;

            let mut args = default_build_args();
            args.content_dir = PathBuf::from("examples/e2e/content");
            args.template_dir = Some(PathBuf::from("examples/e2e/templates"));
            args.output_dir = PathBuf::from("examples/e2e/public");
            args.static_dir = PathBuf::from("examples/e2e/static");
            args.site_domain = None;
            run_build(&args)?;

            let index = fs::read_to_string("examples/e2e/public/index.html")
                .context("failed to read generated index")?;
            assert!(!index.contains("root-base"));
            let robots = fs::read_to_string("examples/e2e/public/robots.txt")
                .context("failed to read generated robots")?;
            assert!(robots.contains("Sitemap: https://example.com/sitemap.xml"));
            Ok(())
        })
    }
}
