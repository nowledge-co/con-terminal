//! Portable check preference. macOS uses Sparkle's own persisted setting.
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Mutex, OnceLock},
};

#[derive(Clone, Copy, Serialize, Deserialize)]
struct Preferences {
    automatic_checks: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            automatic_checks: true,
        }
    }
}
static PREFERENCES: OnceLock<Mutex<Preferences>> = OnceLock::new();
fn path() -> std::path::PathBuf {
    con_paths::app_data_dir().join("update-preferences.json")
}
fn read(path: &Path) -> Preferences {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            log::warn!("Cannot read update preference: {error}; automatic checks disabled");
            Preferences {
                automatic_checks: false,
            }
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Preferences::default(),
        Err(error) => {
            log::warn!("Cannot read update preference: {error}; automatic checks disabled");
            Preferences {
                automatic_checks: false,
            }
        }
    }
}
fn slot() -> &'static Mutex<Preferences> {
    PREFERENCES.get_or_init(|| Mutex::new(read(&path())))
}
pub(super) fn enabled() -> bool {
    slot().lock().unwrap().automatic_checks
}
fn write(path: &Path, enabled: bool) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    con_update::write_atomic(
        path,
        &serde_json::to_vec(&Preferences {
            automatic_checks: enabled,
        })?,
    )
}
pub(super) fn set(enabled: bool) -> anyhow::Result<()> {
    let mut preferences = slot().lock().unwrap();
    write(&path(), enabled)?;
    preferences.automatic_checks = enabled;
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn preference_survives_restart_and_corruption_does_not_enable_network_checks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("updates.json");
        assert!(super::read(&path).automatic_checks);
        super::write(&path, false).unwrap();
        assert!(!super::read(&path).automatic_checks);
        super::write(&path, true).unwrap();
        assert!(super::read(&path).automatic_checks);
        std::fs::write(&path, b"broken").unwrap();
        assert!(!super::read(&path).automatic_checks);
        assert!(super::write(dir.path(), false).is_err());
    }
}
