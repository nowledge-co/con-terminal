use super::*;
use std::{
    process::{Command, Stdio},
    time::Instant,
};

fn validate(plan: &Prepared) -> Result<()> {
    ensure!(
        plan.root.is_absolute() && fs::canonicalize(&plan.root)? == plan.root,
        "Invalid installation root"
    );
    ensure!(
        plan.stage.parent() == Some(plan.root.as_path())
            && plan
                .stage
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s
                    .strip_prefix(".con-update-")
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())),
        "Invalid update staging path"
    );
    ensure!(
        !plan.stage.is_symlink() && fs::canonicalize(&plan.stage)? == plan.stage,
        "Invalid update staging directory"
    );
    for name in binary_names() {
        ensure!(
            !plan.root.join(name).is_symlink()
                && plan.root.join(name).is_file()
                && !fs::metadata(plan.root.join(name))?.permissions().readonly(),
            "Installation changed; refusing update"
        );
    }
    Ok(())
}

/// Launch a copy of the existing trusted companion, never code from an
/// unverified download. The caller may quit only after the helper acknowledges.
pub fn spawn_helper(plan: &Prepared) -> Result<()> {
    validate(plan)?;
    let guard = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(plan.stage.join("helper.lock"))?;
    guard
        .try_lock()
        .context("This update is already waiting for con to close")?;
    for token in ["ready", "commit", "helper-error"] {
        let _ = fs::remove_file(plan.stage.join(token));
    }
    drop(guard);
    let helper = plan.stage.join(if cfg!(windows) {
        "helper.exe"
    } else {
        "helper"
    });
    fs::copy(plan.root.join(binary_names()[1]), &helper)?;
    let mut command = Command::new(helper);
    command
        .arg("+update-helper")
        .arg(plan.stage.join("plan.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().context("Could not start update helper")?;
    let started = Instant::now();
    loop {
        if plan.stage.join("ready").exists() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            if let Ok(message) = fs::read_to_string(plan.stage.join("helper-error")) {
                bail!("{message}");
            }
            bail!("Update helper stopped before handoff: {status}");
        }
        ensure!(
            started.elapsed() < Duration::from_secs(15),
            "Update helper did not acknowledge; con remains open"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn run_helper(path: &Path) -> Result<()> {
    let plan: Prepared = serde_json::from_slice(&fs::read(path)?)?;
    validate(&plan)?;
    ensure!(
        path == plan.stage.join("plan.json"),
        "Unexpected update plan"
    );
    let guard = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(plan.stage.join("helper.lock"))?;
    guard
        .try_lock()
        .context("Another helper owns this update")?;
    ensure!(
        newer(&plan.release.version, BUILD_VERSION)
            || (plan.stage.join("journal").exists() && !plan.stage.join("complete").exists()),
        "Refusing an update that is not newer than the installed helper"
    );
    let result = apply_after_exit(&plan, PUBLIC_KEY, Duration::from_secs(120));
    let message = match &result {
        Ok(()) => "Update installed".to_owned(),
        Err(error) => format!(
            "Update could not finish: {error:#}. Recovery files were retained; retry from Settings."
        ),
    };
    if result.is_err() {
        let _ = durable_write(&plan.stage.join("helper-error"), message.as_bytes());
    }
    let _ = durable_write(&plan.root.join(".con-update-result"), message.as_bytes());
    // Relaunch after success or a completed rollback. No shell interpretation.
    // A helper failure before acknowledgment must not launch a second app.
    if plan.stage.join("ready").exists()
        && plan.stage.join("commit").exists()
        && plan.stage.join("journal").exists()
        && !plan.stage.join("rollback-failed").exists()
    {
        if let Err(error) = Command::new(plan.root.join(binary_names()[0]))
            .current_dir(dirs::home_dir().unwrap_or_else(|| plan.root.clone()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .open(plan.root.join(".con-update.lock"))?;
            lock.try_lock()
                .context("Could not lock installation for launch-failure recovery")?;
            rollback(&plan.root, &plan.stage)?;
            let _ = fs::remove_file(plan.stage.join("complete"));
            durable_write(
                &plan.root.join(".con-update-result"),
                format!(
                    "The new application could not start ({error}); previous binaries restored."
                )
                .as_bytes(),
            )?;
            let _ = Command::new(plan.root.join(binary_names()[0])).spawn();
            return Err(error.into());
        }
    }
    result
}

fn apply_after_exit(plan: &Prepared, key: &str, wait: Duration) -> Result<()> {
    // Re-read and verify signed bytes after handoff; stage files are not trusted
    // just because a prior process verified them.
    let bytes = read_archive(plan)?;
    verify(&bytes, &plan.release, key)?;
    let verified = plan.stage.join("verified");
    if verified.exists() {
        fs::remove_dir_all(&verified)?;
    }
    fs::create_dir(&verified)?;
    extract(&bytes, &verified)?;
    validate_identity(&verified, &plan.release)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(plan.root.join(".con-update.lock"))?;
    durable_write(&plan.stage.join("ready"), b"ready")?;
    let started = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(error) => return Err(error.into()),
        }
        ensure!(
            started.elapsed() < wait,
            "Other con instances are still open; no files were replaced"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // Only the authenticated app exit authorizes replacement. A caller that
    // failed/cancelled handoff leaves no commit token.
    ensure!(
        plan.stage.join("commit").is_file(),
        "Update handoff was cancelled"
    );
    recover(&plan.root, &plan.stage)?;
    let _ = fs::remove_file(plan.stage.join("rollback-failed"));
    replace_pair(&plan.root, &plan.stage, &verified)?;
    Ok(())
}

/// Recoverable two-file transaction. The write-ahead marker and backups survive
/// helper interruption. No unrelated installation files are removed.
fn replace_pair(root: &Path, stage: &Path, verified: &Path) -> Result<()> {
    durable_write(&stage.join("journal"), b"replacing")?;
    let result = (|| {
        for name in binary_names() {
            let current = root.join(name);
            let backup = stage.join(format!("{name}.old"));
            let pending = stage.join(format!("{name}.pending"));
            fs::copy(&current, &pending).with_context(|| format!("Could not back up {name}"))?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&pending)?
                .sync_all()?;
            fs::rename(pending, &backup)?;
            fs::rename(verified.join(name), &current)?;
        }
        #[cfg(unix)]
        File::open(root)?.sync_all()?;
        durable_write(&stage.join("complete"), b"complete")?;
        Ok(())
    })();
    if result.is_err()
        && let Err(error) = rollback(root, stage)
    {
        fs::write(stage.join("rollback-failed"), format!("{error:#}"))?;
        return Err(error.context(
            "Could not restore installation; backups retained in update staging directory",
        ));
    }
    result
}

pub fn rollback(root: &Path, stage: &Path) -> Result<()> {
    for name in binary_names().into_iter().rev() {
        let backup = stage.join(format!("{name}.old"));
        if backup.exists() {
            let current = root.join(name);
            fs::rename(backup, current)?;
        }
    }
    Ok(())
}

/// Called before new downloads. A partial previous transaction must be recovered
/// under the same exclusive lock before any replacement is attempted.
fn recover(root: &Path, stage: &Path) -> Result<()> {
    ensure!(
        stage.parent() == Some(root) && !stage.is_symlink(),
        "Invalid recovery staging"
    );
    // Only the current, re-verified plan can authorize recovery. Never scan
    // similarly named directories and treat their backups as executable code.
    if stage.join("journal").exists() && !stage.join("complete").exists() {
        rollback(root, stage)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_file_failure_restores_both_originals() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stage = root.join("stage");
        let verified = stage.join("verified");
        fs::create_dir_all(&verified).unwrap();
        for name in binary_names() {
            fs::write(root.join(name), format!("old {name}")).unwrap();
        }
        fs::write(verified.join(binary_names()[0]), "new").unwrap();
        assert!(replace_pair(root, &stage, &verified).is_err());
        for name in binary_names() {
            assert_eq!(
                fs::read_to_string(root.join(name)).unwrap(),
                format!("old {name}")
            );
        }
    }
    #[test]
    fn partial_journal_recovers_and_success_preserves_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stage = root.join(".con-update-test");
        let verified = stage.join("verified");
        fs::create_dir_all(&verified).unwrap();
        for name in binary_names() {
            fs::write(root.join(name), "old").unwrap();
            fs::write(verified.join(name), "new").unwrap();
        }
        fs::write(root.join("preferences"), "keep").unwrap();
        fs::rename(
            root.join(binary_names()[0]),
            stage.join(format!("{}.old", binary_names()[0])),
        )
        .unwrap();
        fs::write(stage.join("journal"), "replacing").unwrap();
        recover(root, &stage).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(binary_names()[0])).unwrap(),
            "old"
        );
        replace_pair(root, &stage, &verified).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("preferences")).unwrap(),
            "keep"
        );
        recover(root, &stage).unwrap();
        for name in binary_names() {
            assert_eq!(fs::read_to_string(root.join(name)).unwrap(), "new");
        }
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    #[test]
    fn signed_handoff_waits_for_running_apps_then_installs_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let stage = root.join(".con-update-fixture");
        fs::create_dir(&stage).unwrap();
        for name in binary_names() {
            fs::write(root.join(name), format!("old {name}")).unwrap();
        }
        let bytes = crate::tests::archive(false);
        let key = SigningKey::from_bytes(&[31; 32]);
        let public = STANDARD.encode(key.verifying_key().to_bytes());
        let release = Release {
            version: "0.1.0-beta.174".into(),
            url: "https://example.com/archive".into(),
            signature: STANDARD.encode(key.sign(&bytes).to_bytes()),
            length: bytes.len() as u64,
            notes: String::new(),
        };
        fs::write(stage.join("archive"), bytes).unwrap();
        let plan = Prepared {
            root: root.clone(),
            stage: stage.clone(),
            release,
        };
        let running = hold_installation(&root).unwrap();
        let worker =
            std::thread::spawn(move || apply_after_exit(&plan, &public, Duration::from_secs(5)));
        let start = Instant::now();
        while !stage.join("ready").exists() {
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
        for name in binary_names() {
            assert_eq!(
                fs::read_to_string(root.join(name)).unwrap(),
                format!("old {name}")
            );
        }
        durable_write(&stage.join("commit"), b"apply").unwrap();
        drop(running);
        worker.join().unwrap().unwrap();
        for name in binary_names() {
            assert_eq!(
                fs::read_to_string(root.join(name)).unwrap(),
                format!("new {name}")
            );
        }
        assert!(stage.join("complete").exists());
    }
    #[test]
    fn invalid_signature_cannot_acknowledge_or_modify_installation() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join(".con-update-fixture");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("archive"), b"changed").unwrap();
        let plan = Prepared {
            root: dir.path().to_owned(),
            stage: stage.clone(),
            release: Release {
                version: "0.1.0-beta.174".into(),
                url: String::new(),
                signature: STANDARD.encode([0; 64]),
                length: 7,
                notes: String::new(),
            },
        };
        assert!(
            apply_after_exit(&plan, &STANDARD.encode([0; 32]), Duration::from_millis(1)).is_err()
        );
        assert!(!stage.join("ready").exists());
        assert!(!stage.join("journal").exists());
    }
}
