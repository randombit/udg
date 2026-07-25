use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use udg::pipeline::{build_cmd, check_cmd, extract_cmd, serve_cmd};

/// Documentation generator and linter for C++/C/Python libraries.
#[derive(Parser)]
#[command(name = "udg", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Extract and render the documentation site.
    Build {
        /// Path to udg.toml.
        #[arg(short, long, default_value = "udg.toml")]
        config: PathBuf,
        /// Output directory (overrides [output] dir).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Open the built site in a browser.
        #[arg(long)]
        open: bool,
    },
    /// Run documentation lints and coverage checks (CI gate).
    Check {
        /// Path to udg.toml.
        #[arg(short, long, default_value = "udg.toml")]
        config: PathBuf,
        /// Record current coverage as the new ratchet baseline.
        #[arg(long)]
        update_ratchet: bool,
        /// List every undocumented public item as a finding (implied by
        /// [check] require_docs in the config).
        #[arg(long)]
        undocumented: bool,
        /// List every documented function whose parameters lack @param /
        /// :param: docs (implied by [check] require_param_docs).
        #[arg(long)]
        undocumented_params: bool,
    },
    /// Build the site and serve it locally.
    Serve {
        /// Path to udg.toml.
        #[arg(short, long, default_value = "udg.toml")]
        config: PathBuf,
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },
    /// Extract API items to JSON IR.
    Extract {
        /// Path to udg.toml.
        #[arg(short, long, default_value = "udg.toml")]
        config: PathBuf,
        /// Output file (stdout if omitted).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Extract { config, output } => extract_cmd(&config, output.as_deref()),
        Command::Build {
            config,
            output,
            open,
        } => build_cmd(&config, output.as_deref(), open),
        Command::Check {
            config,
            update_ratchet,
            undocumented,
            undocumented_params,
        } => check_cmd(&config, update_ratchet, undocumented, undocumented_params),
        Command::Serve { config, port } => serve_cmd(&config, port),
    }
}
