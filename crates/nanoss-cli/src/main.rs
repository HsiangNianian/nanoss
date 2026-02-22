use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use nanoss_core::{build_site, BuildConfig};
use nanoss_plugin_host::{PluginHost, PluginHostConfig};

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
    },
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
        } => {
            let plugin_host = PluginHost::new(PluginHostConfig {
                plugin_paths,
                timeout_ms: 2_000,
                memory_limit_mb: 128,
            })?;
            plugin_host.init("{}")?;

            let report = build_site(&BuildConfig {
                content_dir,
                output_dir,
                template_dir,
                check_external_links,
                fail_on_broken_links,
            })?;
            plugin_host.shutdown()?;
            println!(
                "Built {} pages, compiled {} Sass files, copied {} assets, checked {} external links ({} broken).",
                report.rendered_pages,
                report.compiled_sass,
                report.copied_assets,
                report.checked_external_links,
                report.broken_external_links
            );
        }
    }

    Ok(())
}
