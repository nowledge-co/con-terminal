use std::path::Path;

use anyhow::{Result, ensure};

/// Thread evidence gathered from a process's open files: the rollout it is
/// writing (strong) and the writer locks it holds (fallback).
#[derive(Default)]
pub(super) struct ThreadEvidence {
    rollouts: Vec<String>,
    locks: Vec<String>,
}

impl ThreadEvidence {
    pub(super) fn is_empty(&self) -> bool {
        self.rollouts.is_empty() && self.locks.is_empty()
    }

    pub(super) fn extend(&mut self, other: Self) {
        self.rollouts.extend(other.rollouts);
        self.locks.extend(other.locks);
    }

    pub(super) fn add(&mut self, path: &Path, codex_home: &Path) {
        if let Some(id) = rollout_thread_id(path, &codex_home.join("sessions")) {
            self.rollouts.push(id);
        } else if let Some(id) = thread_lock_id(path, &codex_home.join("thread-writer-locks")) {
            self.locks.push(id);
        }
    }

    pub(super) fn current(self) -> Result<Option<String>> {
        if let Some(id) = unique_id(self.rollouts, "rollout files")? {
            return Ok(Some(id));
        }
        unique_id(self.locks, "thread locks")
    }
}

fn unique_id(mut ids: Vec<String>, kind: &str) -> Result<Option<String>> {
    ids.sort();
    ids.dedup();
    ensure!(ids.len() <= 1, "Codex process holds multiple {kind}");
    Ok(ids.into_iter().next())
}

fn thread_lock_id(path: &Path, lock_dir: &Path) -> Option<String> {
    if path.parent() != Some(lock_dir) {
        return None;
    }
    let id = path.file_name()?.to_str()?.strip_suffix(".lock")?;
    uuid::Uuid::parse_str(id)
        .is_ok_and(|parsed| parsed.to_string() == id)
        .then(|| id.to_owned())
}

fn rollout_thread_id(path: &Path, sessions_dir: &Path) -> Option<String> {
    if !path.starts_with(sessions_dir) {
        return None;
    }
    let stem = path
        .file_name()?
        .to_str()?
        .strip_prefix("rollout-")?
        .strip_suffix(".jsonl")?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    uuid::Uuid::parse_str(id)
        .is_ok_and(|parsed| parsed.to_string() == id)
        .then(|| id.to_owned())
}

#[cfg(any(test, target_os = "macos"))]
pub(super) fn parse_open_thread_evidence(output: &[u8], codex_home: &Path) -> ThreadEvidence {
    let mut found = ThreadEvidence::default();
    for line in String::from_utf8_lossy(output).lines() {
        if let Some(path) = line.strip_prefix('n').map(Path::new) {
            found.add(path, codex_home);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unique_id_dedups() {
        assert!(
            unique_id(vec!["a".into(), "a".into()], "locks")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn rollout_evidence_wins_over_ambiguous_locks() {
        let home = Path::new("/home/test/.codex");
        let first = "01a0cd0f-5fcb-7200-88c2-b09fc036c5a3";
        let second = "01a0cd10-22ff-7444-a11a-43185ca6eb21";
        // The resume transition holds two locks but writes one rollout.
        let output = format!(
            "n{}/thread-writer-locks/{first}.lock\n\
             n{}/thread-writer-locks/{second}.lock\n\
             n{}/sessions/2026/09/23/rollout-2026-09-23T13-54-38-{second}.jsonl\n",
            home.display(),
            home.display(),
            home.display()
        );
        assert_eq!(
            parse_open_thread_evidence(output.as_bytes(), home)
                .current()
                .unwrap(),
            Some(second.into())
        );
    }

    #[test]
    fn process_lock_identifies_only_a_unique_codex_thread() {
        let home = Path::new("/home/test/.codex");
        let root = home.join("thread-writer-locks");
        let first = "01a0cd0f-5fcb-7200-88c2-b09fc036c5a3";
        let second = "01a0cd10-22ff-7444-a11a-43185ca6eb21";
        let output = format!(
            "p123\nf3\nn/home/test/.codex/state_5.sqlite\nf4\nn{}/{}.lock\n",
            root.display(),
            first
        );
        assert_eq!(
            parse_open_thread_evidence(output.as_bytes(), home)
                .current()
                .unwrap(),
            Some(first.into())
        );
        let spoofed = format!(
            "n/home/test/other/thread-writer-locks/{first}.lock\nn{}/not-a-uuid.lock\n",
            root.display()
        );
        assert_eq!(
            parse_open_thread_evidence(spoofed.as_bytes(), home)
                .current()
                .unwrap(),
            None
        );
        let multiple = format!("{output}n{}/{second}.lock\n", root.display());
        assert!(
            parse_open_thread_evidence(multiple.as_bytes(), home)
                .current()
                .is_err()
        );
        // A rollout path outside the store or with a mangled id is ignored.
        let stray = format!(
            "n/tmp/rollout-2026-09-23T13-54-38-{first}.jsonl\nn{}/sessions/2026/09/23/rollout-short.jsonl\n",
            home.display()
        );
        assert_eq!(
            parse_open_thread_evidence(stray.as_bytes(), home)
                .current()
                .unwrap(),
            None
        );
    }
}
