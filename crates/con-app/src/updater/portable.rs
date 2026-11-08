//! One state machine for signed portable updates; Sparkle remains the macOS backend.
#![cfg_attr(target_os = "macos", allow(dead_code))]
use super::{CheckState, latest_slot};
use con_update::{Installation, Prepared, Release};
use std::{
    fs::{self, File},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
static REVISION: AtomicU64 = AtomicU64::new(0);
static RELEASE: Mutex<Option<Release>> = Mutex::new(None);
static PREPARED: Mutex<Option<Prepared>> = Mutex::new(None);
static CANCEL: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);
static EXIT: AtomicBool = AtomicBool::new(false);
static LOCK: OnceLock<File> = OnceLock::new();
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("updater runtime"))
}
fn state(value: CheckState) {
    if let Ok(mut slot) = latest_slot().lock() {
        *slot = value;
        REVISION.fetch_add(1, Ordering::Relaxed);
    }
}
pub fn notes() -> Vec<con_update::NoteBlock> {
    RELEASE
        .lock()
        .ok()
        .and_then(|s| s.as_ref().map(|r| con_update::notes_blocks(&r.notes)))
        .unwrap_or_default()
}
pub fn managed() -> Option<&'static str> {
    if review_enabled() {
        return None;
    }
    if con_update::PUBLIC_KEY.is_empty() {
        return Some(
            "This build cannot verify update signatures. Download a signed release to update.",
        );
    }
    match con_update::host_installation() {
        Installation::Managed(message) => Some(message),
        Installation::Direct(_) if LOCK.get().is_none() => {
            Some("This installation cannot apply updates safely. Download the release manually.")
        }
        _ => None,
    }
}
pub fn init() {
    if let Installation::Direct(root) = con_update::host_installation() {
        match con_update::hold_installation(&root) {
            Ok(lock) => {
                let _ = LOCK.set(lock);
            }
            Err(error) => {
                state(CheckState::Error(format!(
                    "Installation is not writable: {error}"
                )));
                return;
            }
        }
        if let Ok(Some(prepared)) = con_update::pending(&root)
            && (con_update::newer(&prepared.release.version, &crate::app_display_version())
                || prepared.stage.join("journal").exists())
        {
            *RELEASE.lock().unwrap() = Some(prepared.release.clone());
            state(CheckState::Ready {
                version: prepared.release.version.clone(),
            });
            *PREPARED.lock().unwrap() = Some(prepared);
        }
        if let Ok(message) = fs::read_to_string(root.join(".con-update-result")) {
            if !message.starts_with("Update installed") {
                state(CheckState::Error(message));
            }
            let _ = fs::remove_file(root.join(".con-update-result"));
        }
    }
}
pub fn check(manual: bool) {
    if let Some(prepared) = PREPARED.lock().ok().and_then(|p| p.clone()) {
        state(CheckState::Ready {
            version: prepared.release.version,
        });
        return;
    }
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    state(CheckState::Checking);
    runtime().spawn(async move {
        let result = async {
            let channel = con_core::release_channel::current();
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()?;
            let url = channel.feed_url(
                con_core::release_channel::host_platform(),
                con_core::release_channel::host_arch(),
            );
            let mut response = client.get(url).send().await?.error_for_status()?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                anyhow::ensure!(
                    bytes.len() + chunk.len() <= 2 * 1024 * 1024,
                    "Update feed is too large"
                );
                bytes.extend_from_slice(&chunk);
            }
            let release = con_update::parse_feed(
                std::str::from_utf8(&bytes)?,
                con_core::release_channel::host_platform(),
                con_core::release_channel::host_arch(),
            )?;
            let skipped = !manual
                && fs::read_to_string(con_paths::app_data_dir().join("skipped-update"))
                    .ok()
                    .as_deref()
                    == Some(release.version.as_str());
            let next = candidate_state(&release, &crate::app_display_version(), skipped);
            *RELEASE.lock().unwrap() = Some(release);
            Ok::<_, anyhow::Error>(next)
        }
        .await;
        state(result.unwrap_or_else(|error| CheckState::Error(error.to_string())));
        BUSY.store(false, Ordering::SeqCst);
    });
}
fn candidate_state(release: &Release, running: &str, skipped: bool) -> CheckState {
    if !con_update::newer(&release.version, running) {
        CheckState::UpToDate
    } else if skipped {
        CheckState::Skipped {
            version: release.version.clone(),
        }
    } else {
        CheckState::UpdateAvailable {
            version: release.version.clone(),
            url: release.url.clone(),
        }
    }
}
pub fn download() {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(release) = RELEASE.lock().ok().and_then(|r| r.clone()) else {
        BUSY.store(false, Ordering::SeqCst);
        return;
    };
    if LOCK.get().is_none() {
        state(CheckState::Error(
            "This installation is not ready for in-app updates.".into(),
        ));
        BUSY.store(false, Ordering::SeqCst);
        return;
    }
    let Installation::Direct(root) = con_update::host_installation() else {
        BUSY.store(false, Ordering::SeqCst);
        return;
    };
    CANCEL.store(false, Ordering::SeqCst);
    let version = release.version.clone();
    state(CheckState::Downloading {
        version: version.clone(),
        received: 0,
        total: release.length,
    });
    runtime().spawn(async move {
        let result = con_update::prepare(&root, release, &CANCEL, |received, total| {
            state(CheckState::Downloading {
                version: version.clone(),
                received,
                total,
            })
        })
        .await;
        match result {
            Ok(prepared) => {
                *PREPARED.lock().unwrap() = Some(prepared);
                state(CheckState::Ready { version });
            }
            Err(error) => {
                state(CheckState::Error(error.to_string()));
            }
        }
        BUSY.store(false, Ordering::SeqCst);
    });
}
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}
pub fn skip() {
    if !matches!(super::latest_check(), CheckState::UpdateAvailable { .. }) {
        return;
    }
    if let Some(release) = RELEASE.lock().ok().and_then(|r| r.clone()) {
        let path = con_paths::app_data_dir().join("skipped-update");
        let _ = fs::create_dir_all(con_paths::app_data_dir());
        match fs::write(path, &release.version) {
            Ok(()) => state(CheckState::Skipped {
                version: release.version,
            }),
            Err(error) => state(CheckState::Error(format!(
                "Could not save skipped version: {error}"
            ))),
        }
    }
}
pub fn restart() {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(prepared) = PREPARED.lock().ok().and_then(|p| p.clone()) else {
        BUSY.store(false, Ordering::SeqCst);
        return;
    };
    state(CheckState::Installing);
    runtime().spawn_blocking(move || {
        match con_update::spawn_helper(&prepared) {
            Ok(()) => {
                EXIT.store(true, Ordering::SeqCst);
            }
            Err(error) => {
                state(CheckState::Error(error.to_string()));
            }
        }
        BUSY.store(false, Ordering::SeqCst);
    });
}
pub fn take_exit() -> bool {
    EXIT.swap(false, Ordering::SeqCst)
}

thread_local! {
    static WORKSPACES: std::cell::RefCell<Vec<gpui::WeakEntity<crate::workspace::ConWorkspace>>> = const { std::cell::RefCell::new(Vec::new()) };
}
pub fn register_workspace(workspace: gpui::WeakEntity<crate::workspace::ConWorkspace>) {
    WORKSPACES.with(|all| all.borrow_mut().push(workspace));
}
pub fn observe(cx: &mut gpui::App) {
    if !enabled() {
        return;
    }
    cx.spawn(async move |cx| {
        let mut seen = REVISION.load(Ordering::Relaxed);
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(200))
                .await;
            let revision = REVISION.load(Ordering::Relaxed);
            if revision != seen {
                let _ = cx.update(|cx| cx.refresh_windows());
                seen = revision;
            }
            if take_exit() {
                let _ = cx.update(|cx| {
                    if let Err(error) = save_restart_snapshot(cx) {
                        state(CheckState::Error(format!(
                            "Could not save sessions; con remains open: {error}"
                        )));
                        return;
                    }
                    let Some(prepared) = PREPARED.lock().ok().and_then(|p| p.clone()) else {
                        return;
                    };
                    if let Err(error) = con_update::authorize_exit(&prepared) {
                        state(CheckState::Error(error.to_string()));
                        return;
                    }
                    WORKSPACES.with(|all| {
                        for workspace in all.borrow().iter() {
                            let _ = workspace
                                .update(cx, |workspace, cx| workspace.prepare_app_exit(cx));
                        }
                    });
                    cx.quit();
                });
                // Keep observing if saving or authorizing exit failed so the
                // user can retry without restarting con. Successful quit ends
                // this task with the application.
            }
        }
    })
    .detach();
}

fn save_restart_snapshot(cx: &mut gpui::App) -> anyhow::Result<()> {
    let sessions = WORKSPACES.with(|all| {
        all.borrow()
            .iter()
            .filter_map(|workspace| {
                workspace
                    .update(cx, |workspace, cx| workspace.snapshot_for_update(cx))
                    .ok()
                    .flatten()
            })
            .collect::<Vec<_>>()
    });
    let combined = merge_restart_sessions(sessions)?;
    con_update::write_atomic(
        &con_paths::app_data_dir().join("update-session.json"),
        &serde_json::to_vec(&combined)?,
    )
}
fn merge_history(
    into: &mut con_core::session::GlobalHistoryState,
    from: con_core::session::GlobalHistoryState,
) {
    for entry in from.global_shell_history {
        if !into
            .global_shell_history
            .iter()
            .any(|old| old.command == entry.command && old.cwd == entry.cwd)
        {
            into.global_shell_history.push(entry);
        }
    }
    for entry in from.input_history {
        if !into.input_history.contains(&entry) {
            into.input_history.push(entry);
        }
    }
}
fn merge_restart_sessions(
    mut sessions: Vec<con_core::session::Session>,
) -> anyhow::Result<con_core::session::Session> {
    anyhow::ensure!(!sessions.is_empty(), "No workspace snapshot available");
    let mut combined = sessions.remove(0);
    let mut history = con_core::session::GlobalHistoryState {
        global_shell_history: std::mem::take(&mut combined.global_shell_history),
        input_history: std::mem::take(&mut combined.input_history),
    };
    for session in sessions {
        combined.tabs.extend(session.tabs);
        merge_history(
            &mut history,
            con_core::session::GlobalHistoryState {
                global_shell_history: session.global_shell_history,
                input_history: session.input_history,
            },
        );
    }
    combined.global_shell_history = history.global_shell_history;
    combined.input_history = history.input_history;
    Ok(combined)
}

pub fn restore_session() {
    let path = con_paths::app_data_dir().join("update-session.json");
    if let Ok(bytes) = fs::read(&path) {
        match (|| -> anyhow::Result<()> {
            let session = serde_json::from_slice::<con_core::session::Session>(&bytes)?;
            // The separate live history journal remains authoritative. Restore
            // layout/native identities without rewriting history or its order.
            con_update::write_atomic(
                &con_core::session::Session::session_path(),
                &serde_json::to_vec_pretty(&session)?,
            )
        })() {
            Ok(()) => {
                let _ = fs::remove_file(path);
            }
            Err(error) => {
                log::error!(
                    "Could not restore pre-update session snapshot; backup retained: {error}"
                );
                state(CheckState::Error(format!(
                    "Could not restore the pre-update session snapshot. Its backup was retained: {error}"
                )));
            }
        }
    }
}

pub fn enabled() -> bool {
    !cfg!(target_os = "macos") || review_enabled()
}
fn review_enabled() -> bool {
    #[cfg(debug_assertions)]
    {
        return std::env::var_os("CON_PORTABLE_UPDATE_REVIEW").is_some();
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}
pub fn seed_review() -> bool {
    if !review_enabled() {
        return false;
    }
    #[cfg(debug_assertions)]
    {
        let mode = std::env::var("CON_PORTABLE_UPDATE_REVIEW").unwrap_or_default();
        let release=Release{version:"0.1.0-beta.174".into(),url:"https://con.nowledge.co/changelog/".into(),signature:String::new(),length:96_000_000,
            notes:r#"<article><h2>Fixed</h2><h3>Terminal continuity</h3><ul><li>Keep native agent sessions linked when restarting con.<span class="credit">con community</span></li></ul><h2>Changed</h2><h3>Updates</h3><ul><li>Download verified updates while you work. Restart when you’re ready.</li></ul></article>"#.into()};
        *RELEASE.lock().unwrap() = Some(release);
        let next = match mode.as_str() {
            "downloading" => CheckState::Downloading {
                version: "0.1.0-beta.174".into(),
                received: 54_000_000,
                total: 96_000_000,
            },
            "ready" => CheckState::Ready {
                version: "0.1.0-beta.174".into(),
            },
            "error" => CheckState::Error(
                "Download interrupted. Your current installation is unchanged.".into(),
            ),
            _ => CheckState::UpdateAvailable {
                version: "0.1.0-beta.174".into(),
                url: "https://con.nowledge.co/changelog/".into(),
            },
        };
        state(next);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skipped_version_does_not_hide_that_it_is_already_installed() {
        let release = Release {
            version: "0.1.0-beta.174".into(),
            url: "https://example.com/update".into(),
            signature: String::new(),
            length: 1,
            notes: String::new(),
        };
        assert!(matches!(
            candidate_state(&release, "0.1.0-beta.173", true),
            CheckState::Skipped { .. }
        ));
        assert_eq!(
            candidate_state(&release, "0.1.0-beta.174", true),
            CheckState::UpToDate
        );
        assert_eq!(
            candidate_state(&release, "0.1.0-beta.175", true),
            CheckState::UpToDate
        );
        assert!(matches!(
            candidate_state(&release, "0.1.0-beta.173", false),
            CheckState::UpdateAvailable { .. }
        ));
    }

    #[test]
    fn restart_snapshot_preserves_all_tabs_native_ids_and_unique_history() {
        let mut first = con_core::session::Session::default();
        first.tabs[0].title = "Codex".into();
        first.tabs[0].conversation_id = Some("native-first".into());
        first.input_history = vec!["shared".into()];
        let mut second = con_core::session::Session::default();
        second.tabs[0].title = "Claude".into();
        second.tabs[0].conversation_id = Some("native-second".into());
        second.input_history = vec!["shared".into(), "second window".into()];
        let merged = merge_restart_sessions(vec![first, second]).unwrap();
        assert_eq!(
            merged
                .tabs
                .iter()
                .map(|t| t.conversation_id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("native-first"), Some("native-second")]
        );
        assert_eq!(merged.input_history, vec!["shared", "second window"]);
        assert!(merge_restart_sessions(vec![]).is_err());
    }
    #[test]
    fn snapshot_history_keeps_window_specific_entries() {
        use con_core::session::{CommandHistoryEntryState, GlobalHistoryState};
        let entry = |command: &str, cwd: &str| CommandHistoryEntryState {
            command: command.into(),
            cwd: Some(cwd.into()),
        };
        let mut live = GlobalHistoryState {
            input_history: vec!["newer".into()],
            global_shell_history: vec![entry("git status", "/a")],
        };
        merge_history(
            &mut live,
            GlobalHistoryState {
                input_history: vec!["older".into(), "newer".into()],
                global_shell_history: vec![entry("git status", "/a"), entry("git status", "/b")],
            },
        );
        assert_eq!(live.input_history, vec!["newer", "older"]);
        assert_eq!(live.global_shell_history.len(), 2);
    }
}

pub fn discard() {
    if !matches!(super::latest_check(), CheckState::Ready { .. })
        || BUSY.swap(true, Ordering::SeqCst)
    {
        return;
    }
    let result = (|| -> anyhow::Result<()> {
        let Some(prepared) = PREPARED.lock().ok().and_then(|p| p.clone()) else {
            return Ok(());
        };
        anyhow::ensure!(
            !prepared.stage.join("journal").exists(),
            "Recover this interrupted update before discarding its backups"
        );
        let guard = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(prepared.stage.join("helper.lock"))?;
        guard.try_lock()?;
        drop(guard);
        fs::remove_dir_all(&prepared.stage)?;
        *PREPARED.lock().unwrap() = None;
        state(CheckState::UpdateAvailable {
            version: prepared.release.version,
            url: prepared.release.url,
        });
        Ok(())
    })();
    if let Err(error) = result {
        state(CheckState::Error(error.to_string()));
    }
    BUSY.store(false, Ordering::SeqCst);
}
