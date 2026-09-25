use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};

use super::{HandoffBundle, HandoffJob};

/// Explicitly unlock before close: forked processes can briefly inherit the
/// open file description even with close-on-exec enabled.
pub struct HandoffLock(File);

impl Drop for HandoffLock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            log::warn!("Could not unlock handoff guard: {error}");
        }
    }
}

pub(super) fn validate_id(id: &str) -> Result<()> {
    let uuid = uuid::Uuid::parse_str(id).context("Invalid handoff ID")?;
    ensure!(
        uuid.to_string() == id,
        "Handoff ID must be a canonical UUID"
    );
    Ok(())
}

pub(super) fn private_dir(path: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Unsafe handoff directory: {}",
            path.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                metadata.permissions().mode() & 0o077 == 0,
                "Handoff directory must be private (0700): {}",
                path.display()
            );
        }
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .with_context(|| format!("Create {}", path.display()))?;
    Ok(())
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn ordinary_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Unsafe handoff file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.nlink() == 1 && metadata.permissions().mode() & 0o077 == 0,
            "Handoff files must be private and not hard-linked"
        );
    }
    Ok(())
}

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = options().create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    ordinary_file(path)?;
    let mut data = Vec::new();
    File::open(path)?
        .take(12 * 1024 * 1024 + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() <= 12 * 1024 * 1024,
        "Handoff record exceeds limit"
    );
    Ok(serde_json::from_slice(&data)?)
}

/// Tags a record file that is missing while its job directory still exists:
/// a storage-integrity fault, not the "unknown job" lookup failure the same
/// `NotFound` reports when the whole directory is gone. The control-plane
/// error classifier matches this exact text to keep the two apart — keep it
/// in sync with `MISSING_RECORD_MARKER` in the handoff route.
const MISSING_RECORD: &str = "Handoff record is missing";

fn flag_missing_record(error: anyhow::Error, dir: &Path) -> anyhow::Error {
    let missing = error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    });
    if missing && dir.exists() {
        error.context(MISSING_RECORD)
    } else {
        error
    }
}

pub(super) fn atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if path.exists() {
        ordinary_file(path)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    write_new(&temporary, &serde_json::to_vec_pretty(value)?)?;
    fs::rename(&temporary, path)?;
    // The rename already replaced the record, so a fsync failure must not
    // contradict the now-visible state.
    if let Some(parent) = path.parent() {
        fsync_dir(parent);
    }
    Ok(())
}

/// Best-effort directory fsync after a rename published or replaced a record.
/// Only guards power-loss durability; the visible state already changed.
pub(super) fn fsync_dir(path: &Path) {
    #[cfg(unix)]
    if let Err(error) = File::open(path).and_then(|dir| dir.sync_all()) {
        log::warn!("Could not fsync {}: {error}", path.display());
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Staging directories hold a prepare in flight; the final job directory is
/// published with a single atomic rename once every file is written.
pub(super) const TEMP_PREFIX: &str = ".tmp-";

#[derive(Clone)]
pub(super) struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        private_dir(&root)?;
        Ok(Self {
            root: root.canonicalize()?,
        })
    }

    pub fn lock(&self) -> Result<HandoffLock> {
        private_dir(&self.root)?;
        let path = self.root.join("store.lock");
        let file = match options().create_new(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                ordinary_file(&path)?;
                options().open(path)?
            }
            Err(e) => return Err(e.into()),
        };
        file.lock()?;
        Ok(HandoffLock(file))
    }

    pub fn directory(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        let dir = self.root.join(id);
        if dir.exists() {
            private_dir(&dir)?;
        }
        Ok(dir)
    }

    pub fn job(&self, id: &str) -> Result<HandoffJob> {
        let job: HandoffJob = self.read_record(id, "job.json")?;
        ensure!(
            job.id == id && job.request.request_id == id,
            "Handoff identity mismatch"
        );
        Ok(job)
    }

    pub fn bundle(&self, id: &str) -> Result<HandoffBundle> {
        let bundle: HandoffBundle = self.read_record(id, "bundle.json")?;
        ensure!(
            bundle.schema_version == 1 && bundle.handoff_id == id,
            "Unsupported handoff bundle"
        );
        Ok(bundle)
    }

    pub fn jobs(&self) -> Result<Vec<HandoffJob>> {
        Ok(self.scan_jobs()?.0)
    }

    fn read_record<T: DeserializeOwned>(&self, id: &str, file: &str) -> Result<T> {
        let dir = self.directory(id)?;
        read(&dir.join(file)).map_err(|error| flag_missing_record(error, &dir))
    }

    /// IDs whose `job.json` exists but cannot be parsed or fails identity
    /// checks, plus canonical UUID directories missing `job.json` entirely.
    /// Retain their directories for diagnosis; they do not block other jobs.
    pub fn corrupt_jobs(&self) -> Result<Vec<String>> {
        Ok(self.scan_jobs()?.1)
    }

    fn scan_jobs(&self) -> Result<(Vec<HandoffJob>, Vec<String>)> {
        let mut jobs = Vec::new();
        let mut corrupt = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if validate_id(&name).is_err() {
                continue;
            }
            if !entry.path().join("job.json").exists() {
                log::warn!("Handoff record {name} lost its job file; retaining it");
                corrupt.push(name);
                continue;
            }
            match self.job(&name) {
                Ok(job) => jobs.push(job),
                Err(error) => {
                    log::warn!("Skipping unreadable handoff record {name}: {error}");
                    corrupt.push(name);
                }
            }
        }
        jobs.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        Ok((jobs, corrupt))
    }

    /// Fresh private staging directory for one prepare attempt.
    pub fn staging_dir(&self) -> Result<PathBuf> {
        let dir = self
            .root
            .join(format!("{TEMP_PREFIX}{}", uuid::Uuid::new_v4()));
        private_dir(&dir)?;
        Ok(dir)
    }

    /// Remove crash leftovers: staging directories from prepares that
    /// crashed before the atomic publish rename. The caller must hold the
    /// store lock — a live prepare holds it across staging and the rename,
    /// so anything matching here can only come from a crashed process.
    /// Canonical UUID directories are never swept, even without `job.json`:
    /// on disk that is indistinguishable from a published record that lost
    /// its job file, so it is retained and reported by `corrupt_jobs`.
    pub fn sweep_incomplete(&self) -> Result<usize> {
        let mut removed = 0;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(TEMP_PREFIX) {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                fs::remove_dir_all(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn save(&self, job: &HandoffJob) -> Result<()> {
        atomic(&self.directory(&job.id)?.join("job.json"), job)
    }

    pub fn launch_guard(&self, id: &str) -> Result<HandoffLock> {
        let path = self.directory(id)?.join("launch.lock");
        let file = match options().create_new(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                ordinary_file(&path)?;
                options().open(path)?
            }
            Err(e) => return Err(e.into()),
        };
        file.try_lock().map_err(|_| anyhow::anyhow!("The startup helper or target agent is still running; exit it before releasing this handoff"))?;
        Ok(HandoffLock(file))
    }
}

pub(super) fn stage(bundle: &HandoffBundle) -> Result<()> {
    let parent = bundle.workspace.cwd.join(".con");
    match fs::symlink_metadata(&parent) {
        Ok(m) => ensure!(
            m.is_dir() && !m.file_type().is_symlink(),
            "Unsafe .con directory"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&parent)?;
        }
        Err(e) => return Err(e.into()),
    }
    let root = parent.join("handoffs");
    private_dir(&root)?;
    let dir = root.join(&bundle.handoff_id);
    if dir.try_exists()? {
        // A previous attempt may have staged the export but failed to persist
        // the job record afterwards (e.g. disk full on save). Accept a
        // byte-identical export as an already-completed stage so the retry
        // can proceed; missing, extra, edited, or unsafe entries still refuse.
        ensure!(
            staged_matches(&dir, bundle)?,
            "Handoff staging already exists; prepare a new handoff"
        );
        return Ok(());
    }
    private_dir(&dir)?;
    // The job directory now exists; a later failure must not leave a partial
    // export behind, or retention cleanup would choke on the missing files.
    if let Err(error) = stage_files(bundle, &dir) {
        if let Err(cleanup) = fs::remove_dir_all(&dir) {
            log::warn!(
                "Could not remove partial handoff export {}: {cleanup}",
                dir.display()
            );
        }
        return Err(error);
    }
    Ok(())
}

/// A previously staged export counts as complete only when it holds exactly
/// the bundle's two files with matching bytes — no missing, extra, edited, or
/// unsafe entries.
fn staged_matches(dir: &Path, bundle: &HandoffBundle) -> Result<bool> {
    private_dir(dir)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        ordinary_file(&entry.path())?;
        entries.push(entry.file_name());
    }
    entries.sort();
    if entries.len() != 2 || entries[0] != "context.md" || entries[1] != "evidence.json" {
        return Ok(false);
    }
    Ok(
        fs::read(dir.join("context.md"))? == bundle.context.as_bytes()
            && fs::read(dir.join("evidence.json"))? == serde_json::to_vec_pretty(&bundle.history)?,
    )
}

fn stage_files(bundle: &HandoffBundle, dir: &Path) -> Result<()> {
    // Exclude only this registered job, preserving other untracked files and user rules.
    let relative = dir.strip_prefix(&bundle.workspace.root)?;
    let relative = relative.to_str().context("Non-UTF-8 project path")?;
    ensure!(
        !relative.contains(['\n', '\r']),
        "Newlines in project paths are unsupported"
    );
    let escaped = relative.chars().fold(String::new(), |mut result, c| {
        if "\\*?[]!# ".contains(c) {
            result.push('\\');
        }
        result.push(c);
        result
    });
    let rule = format!("/{escaped}/");
    let exclude_bytes = super::snapshot::git(
        &bundle.workspace.cwd,
        &["rev-parse", "--git-path", "info/exclude"],
    )?;
    let exclude_path = PathBuf::from(String::from_utf8(exclude_bytes)?.trim_end_matches('\n'));
    let exclude = if exclude_path.is_absolute() {
        exclude_path
    } else {
        bundle.workspace.cwd.join(exclude_path)
    };
    let info = exclude.parent().context("Missing Git info directory")?;
    if !info.exists() {
        fs::create_dir(info)?;
    }
    ensure!(
        !fs::symlink_metadata(info)?.file_type().is_symlink(),
        "Unsafe Git info directory"
    );
    let mut opts = OpenOptions::new();
    opts.read(true).append(true).create(true);
    if let Ok(m) = fs::symlink_metadata(&exclude) {
        ensure!(
            m.is_file() && !m.file_type().is_symlink(),
            "Unsafe Git exclude file"
        );
    }
    let mut file = opts.open(&exclude)?;
    file.lock()?;
    let mut existing = String::new();
    (&mut file)
        .take(1024 * 1024)
        .read_to_string(&mut existing)?;
    ensure!(
        file.metadata()?.len() <= 1024 * 1024,
        "Git exclude file exceeds limit"
    );
    if !existing.lines().any(|line| line == rule) {
        writeln!(file, "\n# Con handoff {}\n{rule}", bundle.handoff_id)?;
        file.sync_all()?;
    }
    write_new(&dir.join("context.md"), bundle.context.as_bytes())?;
    write_new(
        &dir.join("evidence.json"),
        &serde_json::to_vec_pretty(&bundle.history)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_unlock_releases_guard_with_an_inherited_description() {
        let root = std::env::temp_dir().join(format!("handoff-lock-{}", uuid::Uuid::new_v4()));
        let store = Store::new(root.clone()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        private_dir(&store.directory(&id).unwrap()).unwrap();
        let guard = store.launch_guard(&id).unwrap();
        // A duplicate models a descriptor inherited between fork and exec.
        let inherited = guard.0.try_clone().unwrap();
        assert!(store.launch_guard(&id).is_err());
        drop(guard);
        let next = store.launch_guard(&id).unwrap();
        drop(next);
        drop(inherited);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_failed_stage_removes_its_partial_export_and_can_be_retried() {
        use con_agent::handoff::{AgentKind, HistoryExport, SourceSession};
        let workspace =
            std::env::temp_dir().join(format!("handoff-stage-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let bundle = HandoffBundle {
            schema_version: 1,
            handoff_id: id.clone(),
            created_at: 1,
            history: HistoryExport {
                source: SourceSession {
                    export_warning: None,
                    agent: AgentKind::Codex,
                    id: "source".into(),
                    store_identity: "test".into(),
                    title: "Title".into(),
                    cwd: workspace.clone(),
                    updated_at: 1,
                },
                agent_version: "test".into(),
                last_turn_id: "turn".into(),
                records: vec![],
                omissions: vec![],
                digest: "digest".into(),
            },
            workspace: super::super::WorkspaceSnapshot {
                cwd: workspace.clone(),
                root: workspace.clone(),
                git_dir: workspace.join(".git"),
                git_common_dir: workspace.join(".git"),
                head: String::new(),
                index_digest: String::new(),
                worktree_digest: String::new(),
                status: String::new(),
                untracked: vec![],
            },
            goal: "goal".into(),
            context: "context".into(),
        };
        // The workspace is not a Git repository yet, so staging fails after
        // the export directory has already been created.
        assert!(stage(&bundle).is_err());
        let export = workspace.join(".con/handoffs").join(&id);
        assert!(!export.exists(), "partial export must be removed");
        // The side effects are safely rebuildable: staging succeeds once the
        // workspace becomes a Git repository.
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&workspace)
                .status()
                .unwrap()
                .success()
        );
        stage(&bundle).unwrap();
        assert_eq!(fs::read(export.join("context.md")).unwrap(), b"context");
        assert!(export.join("evidence.json").exists());
        fs::remove_dir_all(&workspace).unwrap();
    }

    #[test]
    fn restaging_a_byte_identical_export_is_idempotent_but_edits_refuse() {
        use con_agent::handoff::{AgentKind, HistoryExport, SourceSession};
        let workspace =
            std::env::temp_dir().join(format!("handoff-restage-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&workspace)
                .status()
                .unwrap()
                .success()
        );
        let id = uuid::Uuid::new_v4().to_string();
        let bundle = HandoffBundle {
            schema_version: 1,
            handoff_id: id.clone(),
            created_at: 1,
            history: HistoryExport {
                source: SourceSession {
                    export_warning: None,
                    agent: AgentKind::Codex,
                    id: "source".into(),
                    store_identity: "test".into(),
                    title: "Title".into(),
                    cwd: workspace.clone(),
                    updated_at: 1,
                },
                agent_version: "test".into(),
                last_turn_id: "turn".into(),
                records: vec![],
                omissions: vec![],
                digest: "digest".into(),
            },
            workspace: super::super::WorkspaceSnapshot {
                cwd: workspace.clone(),
                root: workspace.clone(),
                git_dir: workspace.join(".git"),
                git_common_dir: workspace.join(".git"),
                head: String::new(),
                index_digest: String::new(),
                worktree_digest: String::new(),
                status: String::new(),
                untracked: vec![],
            },
            goal: "goal".into(),
            context: "context".into(),
        };
        stage(&bundle).unwrap();
        // The export was staged but the job record was never persisted (e.g.
        // the save failed): the retry must accept the identical export.
        stage(&bundle).unwrap();
        let export = workspace.join(".con/handoffs").join(&id);
        // An edited, partial, or extended export is not the staged bundle and
        // still refuses.
        fs::write(export.join("context.md"), b"user edited").unwrap();
        assert!(stage(&bundle).is_err());
        fs::write(export.join("context.md"), b"context").unwrap();
        fs::write(export.join("notes.txt"), b"extra").unwrap();
        assert!(stage(&bundle).is_err());
        fs::remove_dir_all(&workspace).unwrap();
    }
}
