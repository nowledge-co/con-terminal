//! Recover beta.167's con-owned history without evaluating commands or replacing
//! the user's history. Keep private backups and a checkpoint for later starts.
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};

// Restoring a workspace starts many shells together. Serialize briefly, but
// never turn a busy recovery or native history writer into a startup error.
const LOCK_WAIT: Duration = Duration::from_millis(250);
const LOCK_POLL: Duration = Duration::from_millis(10);

fn lock_recovery(file: &std::fs::File) -> Result<bool> {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(true),
            Err(std::fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Ok(false);
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error).context("Could not lock con history recovery");
            }
        }
    }
}

// zsh stores a multiline command as backslash-newline continuations. Work on
// bytes: history can contain zsh metafication bytes that are not UTF-8.
fn records(bytes: &[u8]) -> Vec<&[u8]> {
    let mut result = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' && (index == 0 || bytes[index - 1] != b'\\') {
            result.push(&bytes[start..=index]);
            start = index + 1;
        }
    }
    result
}

fn private_file(path: &Path) -> Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

// zsh defaults to HISTFILE.LOCK, or uses fcntl with HIST_FCNTL_LOCK.
// Honor both protocols so live native shells cannot replace our append.
#[cfg(unix)]
struct ZshLock {
    path: std::path::PathBuf,
    file: std::fs::File,
}
#[cfg(unix)]
impl ZshLock {
    fn acquire(history: &Path) -> Result<Option<Self>> {
        let mut name = history.as_os_str().to_os_string();
        name.push(".LOCK");
        let path = std::path::PathBuf::from(name);
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match private_file(&path) {
                Ok(file) => return Ok(Some(Self { path, file })),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) =>
                {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    std::thread::sleep(LOCK_POLL);
                }
                Err(error) => {
                    return Err(error
                        .context("Shell history is busy; recovery will retry in a new terminal"));
                }
            }
        }
    }
}
#[cfg(unix)]
impl Drop for ZshLock {
    fn drop(&mut self) {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(own), Ok(current)) = (self.file.metadata(), fs::symlink_metadata(&self.path)) {
            if own.dev() == current.dev() && own.ino() == current.ino() {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}
#[cfg(unix)]
fn lock_native_file(file: &std::fs::File) -> Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: flock contains only C integers and fcntl reads it synchronously.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(libc::EAGAIN) | Some(libc::EACCES)
        ) {
            return Ok(false);
        }
        return Err(error).context("Could not lock native shell history");
    }
    Ok(true)
}

pub(super) fn recover(source: &Path, target: &Path) -> Result<usize> {
    if !source.exists() || source == target {
        return Ok(0);
    }
    ensure!(
        !source.is_symlink() && source.metadata()?.is_file(),
        "Con history is not a regular file"
    );
    let directory = source
        .parent()
        .context("Missing history directory")?
        .join("history-recovery");
    fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("lock"))?;
    if !lock_recovery(&lock)? {
        return Ok(0);
    }
    #[cfg(unix)]
    let Some(_source_lock) = ZshLock::acquire(source)? else {
        return Ok(0);
    };
    let mut source_file = OpenOptions::new().read(true).write(true).open(source)?;
    #[cfg(unix)]
    if !lock_native_file(&source_file)? {
        return Ok(0);
    }
    let mut source_bytes = Vec::new();
    source_file.read_to_end(&mut source_bytes)?;
    let source_records = records(&source_bytes);
    if source_records.is_empty() {
        return Ok(0);
    }
    let checkpoint_path = directory.join("recovered");
    let checkpoint = match fs::read(&checkpoint_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let checkpoint_records: HashSet<&[u8]> = records(&checkpoint).into_iter().collect();
    if source_records
        .iter()
        .all(|record| checkpoint_records.contains(record))
    {
        // The old source is deliberately retained. It does not require touching
        // the user's history on every later shell startup once checkpointed.
        return Ok(0);
    }
    // Follow a user-chosen history symlink, but never replace the link or file.
    let target = if target.exists() {
        target.canonicalize()?
    } else {
        target.to_owned()
    };
    if target == source.canonicalize()? {
        return Ok(0);
    }
    if target.exists() {
        ensure!(
            target.metadata()?.is_file(),
            "Shell history target is not a regular file"
        );
    }
    #[cfg(unix)]
    let Some(_target_lock) = ZshLock::acquire(&target)? else {
        return Ok(0);
    };
    let mut options = OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&target)?;
    #[cfg(not(unix))]
    if !lock_recovery(&file)? {
        return Ok(0);
    }
    #[cfg(unix)]
    if !lock_native_file(&file)? {
        return Ok(0);
    }
    ensure!(
        file.metadata()?.is_file(),
        "Shell history target is not a regular file"
    );
    let mut original = Vec::new();
    file.read_to_end(&mut original)?;
    let mut known: HashSet<&[u8]> = records(&original)
        .into_iter()
        .chain(records(&checkpoint))
        .collect();
    let additions: Vec<_> = source_records
        .iter()
        .copied()
        .filter(|record| known.insert(record))
        .collect();
    if !additions.is_empty() {
        let prefix = uuid::Uuid::new_v4().to_string();
        for (suffix, bytes) in [("source", &source_bytes), ("target", &original)] {
            let mut backup = private_file(&directory.join(format!("{prefix}.{suffix}")))?;
            backup.write_all(bytes)?;
            backup.sync_all()?;
        }
        // O_APPEND preserves all existing bytes and concurrent appends. Never
        // rewrite/truncate the target, even when SAVEHIST is small.
        let mut append = Vec::new();
        if !original.is_empty() && !original.ends_with(b"\n") {
            append.push(b'\n');
        }
        for record in &additions {
            append.extend_from_slice(record);
        }
        file.write_all(&append)?;
        file.sync_all()?;
    }
    // Keep the checkpoint cumulative so native history trimming or a still-open
    // beta.167 shell rewriting its history cannot re-import older commands.
    let mut recovered = checkpoint;
    let checkpoint_records: HashSet<Vec<u8>> = records(&recovered)
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect();
    for record in source_records {
        if !checkpoint_records.contains(record) {
            recovered.extend_from_slice(record);
        }
    }
    if fs::read(&checkpoint_path).ok().as_deref() != Some(&recovered) {
        super::install::atomic_replace(&checkpoint_path, &recovered)?;
    }
    Ok(additions.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_preserves_bytes_multiline_history_and_retries_without_duplicates() {
        let root = std::env::temp_dir().join(format!("con-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join(".zsh_history");
        let target = root.join("user-history");
        let original = b": 1:0;old\n: 2:0;shared\n";
        let recent = b": 2:0;shared\n: 3:0;echo first\\\nsecond\n: 4:0;echo \x83\xff\nunfinished";
        fs::write(&target, original).unwrap();
        fs::write(&source, recent).unwrap();
        assert_eq!(recover(&source, &target).unwrap(), 2);
        let merged = fs::read(&target).unwrap();
        assert_eq!(
            merged,
            [
                original.as_slice(),
                b": 3:0;echo first\\\nsecond\n: 4:0;echo \x83\xff\n"
            ]
            .concat()
        );
        assert_eq!(fs::read(&source).unwrap(), recent);
        assert_eq!(recover(&source, &target).unwrap(), 0);
        assert_eq!(fs::read(&target).unwrap(), merged);
        // A native shell can trim its own history. Do not import old entries again.
        fs::write(&target, b": 5:0;newest\n").unwrap();
        assert_eq!(recover(&source, &target).unwrap(), 0);
        fs::write(&source, b": 6:0;later-from-old-shell\n").unwrap();
        assert_eq!(recover(&source, &target).unwrap(), 1);
        assert_eq!(recover(&source, &target).unwrap(), 0);
        let backups = fs::read_dir(root.join("history-recovery"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "source" || ext == "target")
            })
            .count();
        assert_eq!(backups, 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_refuses_special_files_and_preserves_source() {
        let root = std::env::temp_dir().join(format!("con-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join(".zsh_history");
        fs::write(&source, b": 1:0;preserved\n").unwrap();
        assert!(recover(&source, &root).is_err());
        assert_eq!(recover(&source, &source).unwrap(), 0);
        assert_eq!(fs::read(&source).unwrap(), b": 1:0;preserved\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_does_not_write_when_a_native_shell_holds_the_history_lock() {
        let root = std::env::temp_dir().join(format!("con-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join(".zsh_history");
        let target = root.join("user-history");
        fs::write(&source, b": 2:0;recent\n").unwrap();
        fs::write(&target, b": 1:0;original\n").unwrap();
        let busy = ZshLock::acquire(&target).unwrap().unwrap();
        assert_eq!(recover(&source, &target).unwrap(), 0);
        assert_eq!(fs::read(&target).unwrap(), b": 1:0;original\n");
        assert_eq!(fs::read(&source).unwrap(), b": 2:0;recent\n");
        drop(busy);
        assert_eq!(recover(&source, &target).unwrap(), 1);
        assert_eq!(
            fs::read(&target).unwrap(),
            b": 1:0;original\n: 2:0;recent\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn competing_recovery_defers_without_writes_and_retries_after_unlock() {
        let root = std::env::temp_dir().join(format!("con-history-{}", uuid::Uuid::new_v4()));
        let directory = root.join("history-recovery");
        fs::create_dir_all(&directory).unwrap();
        let source = root.join(".zsh_history");
        let target = root.join("user-history");
        fs::write(&source, b": 2:0;recent\n").unwrap();
        fs::write(&target, b": 1:0;original\n").unwrap();
        let owner = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("lock"))
            .unwrap();
        owner.try_lock().unwrap();
        assert_eq!(recover(&source, &target).unwrap(), 0);
        assert_eq!(fs::read(&target).unwrap(), b": 1:0;original\n");
        assert!(!directory.join("recovered").exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        drop(owner);
        assert_eq!(recover(&source, &target).unwrap(), 1);
        assert_eq!(recover(&source, &target).unwrap(), 0);
        assert_eq!(fs::read(&source).unwrap(), b": 2:0;recent\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn checkpointed_source_does_not_lock_or_recreate_the_users_history() {
        let root = std::env::temp_dir().join(format!("con-history-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join(".zsh_history");
        let target = root.join("user-history");
        fs::write(&source, b": 2:0;recent\n").unwrap();
        fs::write(&target, b": 1:0;original\n").unwrap();
        assert_eq!(recover(&source, &target).unwrap(), 1);
        let owner = ZshLock::acquire(&target).unwrap().unwrap();
        assert_eq!(recover(&source, &target).unwrap(), 0);
        // A native shell may trim or remove its history. The old source must
        // not recreate those commands merely because another pane starts.
        fs::remove_file(&target).unwrap();
        assert_eq!(recover(&source, &target).unwrap(), 0);
        assert!(!target.exists());
        fs::write(&source, b": 3:0;later\n").unwrap();
        assert_eq!(recover(&source, &target).unwrap(), 0);
        assert!(!target.exists());
        drop(owner);
        assert_eq!(recover(&source, &target).unwrap(), 1);
        assert_eq!(fs::read(&target).unwrap(), b": 3:0;later\n");
        fs::remove_dir_all(root).unwrap();
    }
}
