use super::*;
use std::collections::BTreeMap;

/// A single process owns routing and persistence for all of its workspaces.
pub(super) struct WorkspaceContinuity {
    pub workspaces: Vec<WeakEntity<ConWorkspace>>,
    pub socket_owner: Option<WeakEntity<ConWorkspace>>,
    pub save_tx: crossbeam_channel::Sender<SessionSaveRequest>,
}

impl Global for WorkspaceContinuity {}

impl WorkspaceContinuity {
    pub fn new() -> Self {
        Self {
            workspaces: Vec::new(),
            socket_owner: None,
            save_tx: spawn_session_save_worker(),
        }
    }
}

#[derive(Default)]
struct SessionSnapshots {
    windows: BTreeMap<gpui::EntityId, Session>,
    last_closed: Option<Session>,
}

impl SessionSnapshots {
    fn record(&mut self, window: gpui::EntityId, snapshot: Option<Session>, closed: bool) {
        if closed {
            self.windows.remove(&window);
            self.last_closed = if self.windows.is_empty() {
                snapshot
            } else {
                None
            };
        } else if let Some(snapshot) = snapshot {
            self.last_closed = None;
            self.windows.insert(window, snapshot);
        }
    }

    fn combined(&self) -> Option<Session> {
        let mut windows = self.windows.values();
        let Some(mut combined) = windows.next().cloned() else {
            return self.last_closed.clone();
        };
        // Retain the existing restore format. Tabs from secondary windows
        // reopen alongside the first window's tabs, with their layouts intact.
        for window in windows {
            combined.tabs.extend(window.tabs.iter().cloned());
        }
        Some(combined)
    }
}

fn spawn_session_save_worker() -> crossbeam_channel::Sender<SessionSaveRequest> {
    let (tx, rx) = crossbeam_channel::unbounded::<SessionSaveRequest>();
    std::thread::Builder::new()
        .name("con-session-save".into())
        .spawn(move || {
            let mut snapshots = SessionSnapshots::default();
            let resume_store = con_core::program_resume_store::ProgramResumeStore::default();
            while let Ok(request) = rx.recv() {
                let mut history = None;
                let mut waiters = Vec::new();
                let mut layout_changed = false;
                for request in std::iter::once(request).chain(rx.try_iter()) {
                    for write in request.program_resume_writes {
                        let result = match write {
                            ProgramResumeWrite::Save(id, command) => {
                                resume_store.save(id, &command)
                            }
                            ProgramResumeWrite::Remove(id) => resume_store.remove(id).map(|_| ()),
                        };
                        if let Err(error) = result {
                            log::warn!("Failed to update a private program resume record: {error}");
                        }
                    }
                    layout_changed |= request.snapshot.is_some() || request.closed;
                    snapshots.record(request.window, request.snapshot, request.closed);
                    if request.history.is_some() {
                        history = request.history;
                    }
                    waiters.extend(request.done);
                }

                if layout_changed
                    && let Some(session) = snapshots.combined()
                    && let Err(error) = serde_json::to_vec_pretty(&session)
                        .map_err(anyhow::Error::from)
                        .and_then(|bytes| {
                            con_update::write_atomic(&Session::session_path(), &bytes)
                        })
                {
                    log::warn!("Failed to save session: {error}");
                }
                if let Some(history) = history
                    && let Err(error) = history.save()
                {
                    log::warn!("Failed to save command history: {error}");
                }
                for waiter in waiters {
                    let _ = waiter.send(());
                }
            }
        })
        .expect("failed to spawn session save worker");
    tx
}

#[cfg(test)]
mod tests {
    use super::SessionSnapshots;
    use con_core::session::Session;
    use serde_json::json;

    fn snapshot(title: &str) -> Session {
        serde_json::from_value(json!({
            "tabs": [{"title": title, "cwd": null, "panes": []}],
            "active_tab": 0,
            "agent_panel_open": false
        }))
        .unwrap()
    }

    #[test]
    fn saves_from_different_windows_do_not_replace_each_other() {
        let first = gpui::EntityId::from(1);
        let second = gpui::EntityId::from(2);
        let mut snapshots = SessionSnapshots::default();
        snapshots.record(first, Some(snapshot("first")), false);
        snapshots.record(second, Some(snapshot("second")), false);
        snapshots.record(first, Some(snapshot("first updated")), false);

        let session = snapshots.combined().unwrap();
        assert_eq!(session.tabs.len(), 2);
        assert_eq!(session.tabs[0].title, "first updated");
        assert_eq!(session.tabs[1].title, "second");

        snapshots.record(first, Some(snapshot("first updated")), true);
        assert_eq!(snapshots.combined().unwrap().tabs[0].title, "second");
        snapshots.record(second, Some(snapshot("second")), true);
        assert_eq!(snapshots.combined().unwrap().tabs[0].title, "second");
    }

    #[test]
    fn removing_a_tearoff_source_preserves_the_destination() {
        let source = gpui::EntityId::from(1);
        let destination = gpui::EntityId::from(2);
        let mut snapshots = SessionSnapshots::default();
        snapshots.record(source, Some(snapshot("before drag")), false);
        snapshots.record(destination, Some(snapshot("transferred")), false);
        snapshots.record(source, None, true);

        let session = snapshots.combined().unwrap();
        assert_eq!(session.tabs.len(), 1);
        assert_eq!(session.tabs[0].title, "transferred");
    }
}
