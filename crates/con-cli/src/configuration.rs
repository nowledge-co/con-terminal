use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ConfigurationCommand {
    /// Import a self-contained Ghostty config snapshot without overwriting the destination.
    ImportGhostty(ImportGhosttyArgs),
    /// Export a self-contained native Ghostty snapshot, excluding Con settings and comments.
    ExportGhostty(ExportGhosttyArgs),
}

#[derive(Args)]
pub struct ImportGhosttyArgs {
    #[arg(long, value_name = "PATH")]
    from: PathBuf,
    #[arg(long, value_name = "PATH")]
    to: Option<PathBuf>,
}

#[derive(Args)]
pub struct ExportGhosttyArgs {
    #[arg(long, value_name = "PATH")]
    to: PathBuf,
    #[arg(long, value_name = "PATH")]
    from: Option<PathBuf>,
}

pub fn run(command: ConfigurationCommand) -> Result<()> {
    match command {
        ConfigurationCommand::ImportGhostty(args) => con_core::config::transfer::import_ghostty(
            args.from,
            args.to.unwrap_or_else(con_paths::config_file),
        ),
        ConfigurationCommand::ExportGhostty(args) => con_core::config::transfer::export_ghostty(
            args.from.unwrap_or_else(con_paths::config_file),
            args.to,
        ),
    }
}
