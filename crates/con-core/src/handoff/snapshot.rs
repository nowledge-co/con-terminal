use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use con_agent::handoff::{digest, sanitize};

use super::WorkspaceSnapshot;

pub(super) fn git(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .env_clear()
        .envs(con_agent::handoff::probe_environment())
        .arg("--no-optional-locks")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg("core.untrackedCache=false")
        .args(args)
        .current_dir(cwd)
        .output()
        .context("Read Git workspace")?;
    ensure!(
        output.status.success(),
        "Cannot read Git workspace ({})",
        args.join(" ")
    );
    ensure!(
        output.stdout.len() <= 16 * 1024 * 1024,
        "Git metadata exceeds handoff limit"
    );
    Ok(output.stdout)
}

fn path(cwd: &Path, args: &[&str]) -> Result<PathBuf> {
    let bytes = git(cwd, args)?;
    let text = String::from_utf8(bytes)?;
    let p = PathBuf::from(text.trim_end_matches('\n'));
    Ok(if p.is_absolute() { p } else { cwd.join(p) }.canonicalize()?)
}

pub(super) fn capture(cwd: &Path) -> Result<WorkspaceSnapshot> {
    let cwd = cwd.canonicalize()?;
    let root = path(&cwd, &["rev-parse", "--show-toplevel"])?;
    let git_dir = path(&cwd, &["rev-parse", "--absolute-git-dir"])?;
    let git_common_dir = path(&cwd, &["rev-parse", "--git-common-dir"])?;
    let index = git(&root, &["ls-files", "--stage", "-z"])?;
    // Gitlinks have an independent working tree. Refuse rather than fingerprint it incompletely.
    ensure!(
        !index.split(|b| *b == 0).any(|r| r.starts_with(b"160000 ")),
        "Submodule workspaces require manual handoff in this version"
    );
    let head = git(&cwd, &["rev-parse", "--verify", "HEAD"])
        .map(|v| String::from_utf8_lossy(&v).trim().to_owned())
        .unwrap_or_else(|_| "unborn".into());
    let tracked = git(&root, &["ls-files", "-z"])?;
    let untracked = git(&root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let staging = cwd.join(".con/handoffs");
    let mut paths = BTreeSet::new();
    let mut untracked_names = Vec::new();
    for (bytes, is_untracked) in [(&tracked, false), (&untracked, true)] {
        for name in bytes.split(|b| *b == 0).filter(|n| !n.is_empty()) {
            let name =
                std::str::from_utf8(name).context("Non-UTF-8 file names require manual handoff")?;
            let relative = Path::new(name);
            ensure!(
                !relative.is_absolute()
                    && !relative
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir)),
                "Invalid Git path"
            );
            if root.join(relative).starts_with(&staging) {
                ensure!(
                    is_untracked,
                    "Tracked .con/handoffs files conflict with private staging"
                );
                continue;
            }
            paths.insert(name.to_owned());
            if is_untracked {
                untracked_names.push(name.to_owned());
            }
        }
    }
    ensure!(
        paths.len() <= 100_000,
        "Workspace has too many files for automatic handoff"
    );
    let mut hashes = Vec::new();
    let mut total = 0u64;
    for name in paths {
        let p = root.join(&name);
        hashes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        hashes.extend_from_slice(name.as_bytes());
        let metadata = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                hashes.extend_from_slice(b"missing");
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if metadata.file_type().is_symlink() {
            hashes.extend_from_slice(b"symlink");
            hashes.extend_from_slice(
                digest(fs::read_link(&p)?.as_os_str().as_encoded_bytes()).as_bytes(),
            );
        } else {
            ensure!(metadata.is_file(), "Unsupported workspace entry: {name}");
            // Reject parent symlink traversal as well as leaf special files.
            ensure!(
                p.canonicalize()?.starts_with(&root),
                "File escapes the workspace: {name}"
            );
            ensure!(
                metadata.len() <= 64 * 1024 * 1024,
                "File exceeds snapshot limit: {name}"
            );
            total += metadata.len();
            ensure!(
                total <= 1024 * 1024 * 1024,
                "Workspace exceeds 1 GiB snapshot limit"
            );
            let mut content = Vec::new();
            fs::File::open(&p)?
                .take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut content)?;
            ensure!(
                content.len() <= 64 * 1024 * 1024,
                "File grew beyond snapshot limit"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                hashes.extend_from_slice(&metadata.permissions().mode().to_le_bytes());
            }
            hashes.extend_from_slice(digest(&content).as_bytes());
        }
    }
    let status = String::from_utf8(git(&root, &["status", "--short", "--untracked-files=all"])?)?;
    Ok(WorkspaceSnapshot {
        cwd,
        root,
        git_dir,
        git_common_dir,
        head,
        index_digest: digest(&index),
        worktree_digest: digest(&hashes),
        status: sanitize(&status),
        untracked: untracked_names,
    })
}

pub(super) fn unchanged(expected: &WorkspaceSnapshot) -> Result<()> {
    let actual = capture(&expected.cwd)?;
    ensure!(actual == *expected, "Workspace changed; send a new handoff");
    Ok(())
}
