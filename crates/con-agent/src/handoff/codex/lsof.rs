//! Bounded, read-only lsof with explicit no-match semantics.
use std::{process::Stdio, time::Duration};

use anyhow::{Context, Result, ensure};
use tokio::{io::AsyncReadExt, process::Command};

pub(super) async fn inspect(args: &[&str]) -> Result<Vec<u8>> {
    let mut child = Command::new("/usr/sbin/lsof")
        .args(args)
        .env_clear()
        .envs(crate::handoff::probe_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Cannot start Codex process inspection")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Missing lsof stdout")?
        .take(1024 * 1024 + 1);
    let mut stderr = child
        .stderr
        .take()
        .context("Missing lsof stderr")?
        .take(8193);
    let (mut data, mut errors) = (Vec::new(), Vec::new());
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::try_join!(
            stdout.read_to_end(&mut data),
            stderr.read_to_end(&mut errors)
        )?;
        ensure!(
            data.len() <= 1024 * 1024 && errors.len() <= 8192,
            "Codex inspection output exceeds limit"
        );
        check_status(child.wait().await?.code(), &data, &errors)?;
        Ok::<_, anyhow::Error>(data)
    })
    .await
    .context("Codex process inspection timed out")?
}

fn check_status(code: Option<i32>, data: &[u8], errors: &[u8]) -> Result<()> {
    // lsof exits 1 for an empty selection. A diagnostic or partial output is
    // not an empty selection and must not unlock the recent-session fallback.
    ensure!(
        errors.is_empty(),
        "Codex process inspection reported an error"
    );
    ensure!(
        code == Some(0) || (code == Some(1) && data.is_empty()),
        "Codex process inspection failed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_selection_is_distinct_from_failed_or_partial_inspection() {
        assert!(check_status(Some(1), b"", b"").is_ok());
        assert!(check_status(Some(0), b"p123\n", b"").is_ok());
        assert!(check_status(Some(1), b"p123\n", b"").is_err());
        assert!(check_status(Some(1), b"", b"permission denied").is_err());
        assert!(check_status(Some(2), b"", b"").is_err());
        assert!(check_status(None, b"", b"").is_err());
    }
}
