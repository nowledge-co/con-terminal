//! Terminal-local launcher for a command already reviewed in the app.

use anyhow::{Context, Result, ensure};
use con_core::program_resume_store::{ProgramResumeStore, TERMINAL_CONTEXT_ENV};
use std::ffi::OsString;
use uuid::Uuid;

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(args.len() == 1, "Expected one program resume approval");
    let ticket = args[0]
        .to_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .context("Invalid program resume approval")?;
    let terminal = std::env::var(TERMINAL_CONTEXT_ENV)
        .ok()
        .and_then(|value| Uuid::parse_str(&value).ok())
        .context("Open the resume action in its con terminal")?;
    let command = ProgramResumeStore::default().consume_approval(ticket, terminal)?;
    let mut launch = command
        .literal_command()
        .context("Prepare resumed program")?;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Stay in the job the shell created for con-cli.
        Err(launch.exec()).context("Start resumed program")
    }
    #[cfg(windows)]
    {
        let status = launch.status().context("Start resumed program")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
