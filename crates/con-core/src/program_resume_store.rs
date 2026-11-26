//! Resume arguments may contain secrets. Layout snapshots carry only a
//! random reference. Restore records must be private to the original account.

use anyhow::{Context, Result, ensure};
use con_terminal::program_resume::ResumeCommand;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

const MAX_FILE_BYTES: u64 = 65_536;
const APPROVAL_LIFETIME: Duration = Duration::from_secs(60);
pub use con_terminal::program_resume::TERMINAL_CONTEXT_ENV;

#[derive(Serialize, Deserialize)]
struct Record {
    id: Uuid,
    command: ResumeCommand,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval: Option<Approval>,
}

#[derive(Serialize, Deserialize)]
struct Approval {
    candidate: Uuid,
    terminal: Uuid,
    issued_at: SystemTime,
}

pub struct ProgramResumeStore {
    directory: PathBuf,
}

impl Default for ProgramResumeStore {
    fn default() -> Self {
        Self {
            directory: con_paths::app_data_dir().join("program-resume"),
        }
    }
}

impl ProgramResumeStore {
    pub fn save(&self, id: Uuid, command: &ResumeCommand) -> Result<()> {
        self.write_record(&Record {
            id,
            command: command.clone(),
            approval: None,
        })
    }

    /// Bind the user's approval to the exact reviewed value and fresh PTY.
    /// Only the short-lived ticket identifier goes through the visible shell.
    pub fn approve(&self, id: Uuid, reviewed: &ResumeCommand, terminal: Uuid) -> Result<Uuid> {
        let stored = self
            .load(id)?
            .context("The saved command is no longer available")?;
        ensure!(
            &stored == reviewed,
            "The saved command changed before approval"
        );
        let ticket = Uuid::new_v4();
        self.write_record(&Record {
            id: ticket,
            command: stored,
            approval: Some(Approval {
                candidate: id,
                terminal,
                issued_at: SystemTime::now(),
            }),
        })?;
        Ok(ticket)
    }

    /// Recheck the terminal identity and atomically consume a launch ticket.
    /// Candidates cannot be launched through this path without user approval.
    pub fn consume_approval(&self, ticket: Uuid, terminal: Uuid) -> Result<ResumeCommand> {
        let record = self
            .read_record(ticket)?
            .context("Resume approval unavailable")?;
        let approval = record
            .approval
            .context("Resume command requires approval")?;
        ensure!(
            approval.terminal == terminal,
            "Resume approval belongs to another terminal"
        );
        let expired = approval
            .issued_at
            .elapsed()
            .map_or(true, |age| age > APPROVAL_LIFETIME);
        ensure!(
            self.remove(ticket)?,
            "Resume approval has already been used"
        );
        ensure!(!expired, "Resume approval expired");
        let current = self
            .load(approval.candidate)?
            .context("The saved command is no longer available")?;
        ensure!(
            current == record.command,
            "The saved command changed after approval"
        );
        ensure!(
            self.remove(approval.candidate)?,
            "The saved command has already been consumed"
        );
        Ok(current)
    }

    fn write_record(&self, record: &Record) -> Result<()> {
        record.command.validate()?;
        self.ensure_directory()?;
        let encoded = serde_json::to_vec(record)?;
        let bytes = protect(encoded)?;
        ensure!(
            bytes.len() as u64 <= MAX_FILE_BYTES,
            "Resume record is too large"
        );

        let temporary = self.directory.join(format!(".{}.tmp", Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut out = options.open(&temporary)?;
        let result = (|| {
            let write_result = out.write_all(&bytes).and_then(|_| out.sync_all());
            drop(out);
            write_result?;
            fs::rename(&temporary, self.path(record.id))?;
            self.sync_directory()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn load(&self, id: Uuid) -> Result<Option<ResumeCommand>> {
        Ok(self
            .read_record(id)?
            .filter(|record| record.approval.is_none())
            .map(|record| record.command))
    }

    fn read_record(&self, id: Uuid) -> Result<Option<Record>> {
        if let Err(error) = self.check_directory() {
            if error.downcast_ref::<std::io::Error>().is_some_and(absent) {
                return Ok(None);
            }
            return Err(error);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Never follow a link or block on a FIFO in the records directory.
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let path = self.path(id);
        #[cfg(windows)]
        {
            let metadata = fs::symlink_metadata(&path);
            match metadata {
                Err(error) if absent(&error) => return Ok(None),
                Err(error) => return Err(error.into()),
                Ok(metadata) => ensure!(
                    !metadata.file_type().is_symlink(),
                    "Resume record must not be a symbolic link"
                ),
            }
        }
        let file = match options.open(path) {
            Ok(f) => f,
            Err(error) if absent(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "Resume record must be a regular file");
        #[cfg(unix)]
        check_private(&metadata)?;
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_FILE_BYTES,
            "Resume record is too large"
        );
        let record: Record =
            serde_json::from_slice(&unprotect(bytes)?).context("Read private resume record")?;
        ensure!(record.id == id, "Resume record reference mismatch");
        record.command.validate()?;
        Ok(Some(record))
    }

    /// Delete before handing an approved record to a visible shell. A failed
    /// launch should never cause the command to be retried on another restart.
    pub fn remove(&self, id: Uuid) -> Result<bool> {
        if let Err(error) = self.check_directory() {
            if error.downcast_ref::<std::io::Error>().is_some_and(absent) {
                return Ok(false);
            }
            return Err(error);
        }
        match fs::remove_file(self.path(id)) {
            Ok(()) => {
                self.sync_directory()?;
                Ok(true)
            }
            Err(error) if absent(&error) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn path(&self, id: Uuid) -> PathBuf {
        self.directory.join(format!("{id}.record"))
    }

    fn check_directory(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.directory)?;
        ensure!(metadata.is_dir(), "Resume store must be a directory");
        #[cfg(unix)]
        check_private(&metadata)?;
        Ok(())
    }

    fn ensure_directory(&self) -> Result<()> {
        let parent = self
            .directory
            .parent()
            .context("Missing app data directory")?;
        fs::create_dir_all(parent)?;
        let mut directory = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        match directory.recursive(false).create(&self.directory) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        self.check_directory()
    }

    fn sync_directory(&self) -> Result<()> {
        #[cfg(unix)]
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

fn absent(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::NotFound
}

#[cfg(unix)]
fn check_private(metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid and geteuid take no pointers and do not change identity.
    let (uid, euid) = unsafe { (libc::getuid(), libc::geteuid()) };
    ensure!(
        uid == euid && uid != 0 && metadata.uid() == uid && metadata.mode() & 0o077 == 0,
        "Resume records require a private store owned by the current user"
    );
    Ok(())
}

#[cfg(unix)]
fn protect(bytes: Vec<u8>) -> Result<Vec<u8>> {
    Ok(bytes)
}

#[cfg(unix)]
fn unprotect(bytes: Vec<u8>) -> Result<Vec<u8>> {
    Ok(bytes)
}

#[cfg(windows)]
fn protect(bytes: Vec<u8>) -> Result<Vec<u8>> {
    dpapi(bytes, true)
}

#[cfg(windows)]
fn unprotect(bytes: Vec<u8>) -> Result<Vec<u8>> {
    dpapi(bytes, false)
}

#[cfg(windows)]
fn dpapi(mut bytes: Vec<u8>, protect: bool) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows::core::PCWSTR;

    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into()?,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the input remains allocated and the output is initialized.
    // DPAPI uses the current Windows user by default. Do not use machine scope.
    unsafe {
        if protect {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        }
        ensure!(
            !output.pbData.is_null(),
            "DPAPI returned an empty resume record"
        );
        let len = output.cbData as usize;
        let result = if len as u64 <= MAX_FILE_BYTES {
            Some(std::slice::from_raw_parts(output.pbData, len).to_vec())
        } else {
            None
        };
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        result.context("DPAPI returned an oversized resume record")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(ProgramResumeStore);

    impl Fixture {
        fn new() -> Self {
            Self(ProgramResumeStore {
                directory: std::env::temp_dir().join(format!("con-resume-test-{}", Uuid::new_v4())),
            })
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.directory);
        }
    }

    fn command() -> ResumeCommand {
        ResumeCommand {
            argv: vec!["example".into(), "resume-fixture-only".into()],
            cwd: Some(std::env::temp_dir()),
        }
    }

    #[test]
    fn record_round_trip_binds_id_and_can_be_consumed_once() {
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let fixture = Fixture::new();
        let store = &fixture.0;
        let id = Uuid::new_v4();
        assert_eq!(store.load(id).unwrap(), None);
        store.save(id, &command()).unwrap();
        assert_eq!(store.load(id).unwrap(), Some(command()));
        let other = Uuid::new_v4();
        fs::copy(store.path(id), store.path(other)).unwrap();
        assert!(store.load(other).is_err());

        #[cfg(windows)]
        assert!(
            !fs::read(store.path(id))
                .unwrap()
                .windows(b"resume-fixture-only".len())
                .any(|window| window == b"resume-fixture-only"),
            "saved Windows resume arguments must be protected by DPAPI"
        );

        assert!(store.remove(id).unwrap());
        assert!(!store.remove(id).unwrap());
        assert_eq!(store.load(id).unwrap(), None);
    }

    #[test]
    fn approval_binds_visible_command_and_terminal_and_is_consumed_once() {
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let fixture = Fixture::new();
        let store = &fixture.0;
        let terminal = Uuid::new_v4();
        let id = Uuid::new_v4();
        let saved = command();
        let mut other = saved.clone();
        other.argv.push("different-value".into());

        store.save(id, &saved).unwrap();
        assert!(store.consume_approval(id, terminal).is_err());
        assert!(store.approve(id, &other, terminal).is_err());
        assert_eq!(store.load(id).unwrap(), Some(saved.clone()));
        let ticket = store.approve(id, &saved, terminal).unwrap();
        assert_eq!(store.load(id).unwrap(), Some(saved.clone()));
        let second_ticket = store.approve(id, &saved, terminal).unwrap();
        assert_eq!(store.load(ticket).unwrap(), None);
        assert!(store.consume_approval(ticket, Uuid::new_v4()).is_err());
        assert_eq!(store.consume_approval(ticket, terminal).unwrap(), saved);
        assert!(store.consume_approval(ticket, terminal).is_err());
        assert!(store.consume_approval(second_ticket, terminal).is_err());
        assert_eq!(store.load(id).unwrap(), None);
    }

    #[test]
    fn expired_approval_cannot_launch() {
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let fixture = Fixture::new();
        let store = &fixture.0;
        let id = Uuid::new_v4();
        let terminal = Uuid::new_v4();
        store
            .write_record(&Record {
                id,
                command: command(),
                approval: Some(Approval {
                    candidate: Uuid::new_v4(),
                    terminal,
                    issued_at: SystemTime::now() - APPROVAL_LIFETIME * 2,
                }),
            })
            .unwrap();
        assert!(store.consume_approval(id, terminal).is_err());
        assert!(store.read_record(id).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_insecure_mode_and_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let fixture = Fixture::new();
        let store = &fixture.0;
        let id = Uuid::new_v4();
        store.save(id, &command()).unwrap();
        let path = store.path(id);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.load(id).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let alias = Uuid::new_v4();
        symlink(path, store.path(alias)).unwrap();
        assert!(store.load(alias).is_err());
    }
}
