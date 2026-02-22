use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use nanoss_core::{build_site, BuildConfig};

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
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Build {
            content_dir,
            output_dir,
            template_dir,
        } => {
            let report = build_site(&BuildConfig {
                content_dir,
                output_dir,
                template_dir,
            })?;
            println!("Built {} pages.", report.rendered_pages);
        }
    }

    Ok(())
}
