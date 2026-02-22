use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use nanoss_core::{build_site, BuildConfig, JsBackend, TailwindBackend, TailwindConfig};

#[derive(Parser)]
#[command(name = "nanoss", version, about = "A modern Rust static site generator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Build {
        #[arg(long, default_value = "content")]
        content_dir: PathBuf,
        #[arg(long, default_value = "public")]
        output_dir: PathBuf,
        #[arg(long)]
        template_dir: Option<PathBuf>,
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
    },
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Build {
            content_dir,
            output_dir,
            template_dir,
            check_external_links,
            fail_on_broken_links,
            plugin_paths,
            js_backend,
            tailwind_input,
            tailwind_output,
            tailwind_bin,
            tailwind_minify,
            tailwind_backend,
        } => {
            let tailwind = match (tailwind_input, tailwind_output) {
                (Some(input_css), Some(output_css)) => Some(TailwindConfig {
                    backend: match tailwind_backend {
                        TailwindBackendArg::Standalone => TailwindBackend::Standalone,
                        TailwindBackendArg::Rswind => TailwindBackend::Rswind,
                    },
                    input_css,
                    output_css,
                    binary: tailwind_bin,
                    minify: tailwind_minify,
                }),
                _ => None,
            };
            let report = build_site(&BuildConfig {
                content_dir,
                output_dir,
                template_dir,
                plugin_paths,
                plugin_timeout_ms: 2_000,
                plugin_memory_limit_mb: 128,
                check_external_links,
                fail_on_broken_links,
                js_backend: match js_backend {
                    JsBackendArg::Passthrough => JsBackend::Passthrough,
                    JsBackendArg::Esbuild => JsBackend::Esbuild,
                },
                tailwind,
            })?;
            println!(
                "Built {} pages ({} with islands), compiled {} Sass files, copied {} assets, processed {} scripts, tailwind: {}, checked {} external links ({} broken).",
                report.rendered_pages,
                report.island_pages,
                report.compiled_sass,
                report.copied_assets,
                report.processed_scripts,
                report.compiled_tailwind,
                report.checked_external_links,
                report.broken_external_links
            );
        }
    }

    Ok(())
}
