//! Owned OSC 7501 facts copied out of a Ghostty callback.
//!
//! The library borrows report text only for the duration of the callback.
//! Backends reduce events in arrival order and hand off one bounded snapshot.
//! Lifecycle barriers cannot be evicted by a burst of progress reports.

use con_terminal::program_status::{BlockedKind, Incoming, State, SurfaceProgramStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramStatusEvent {
    Snapshot(SurfaceProgramStatus),
    Report {
        /// `GhosttyProgramStatusState`. `5` removes records instead of storing one.
        state: i32,
        kind: i32,
        /// `-1` when the program did not send a percentage.
        progress: i8,
        id: String,
        app: String,
        title: String,
        message: String,
    },
    /// OSC 133 prompt start.
    Prompt,
    /// The process attached to the terminal exited.
    ProcessExit,
    /// RIS (`ESC c`).
    FullReset,
    /// A key was delivered to this surface.
    Key,
}

#[derive(Default)]
pub(crate) struct ProgramStatusBuffer {
    status: SurfaceProgramStatus,
    pending: bool,
}

impl ProgramStatusBuffer {
    #[cfg(target_os = "macos")]
    pub(crate) fn is_pending(&self) -> bool {
        self.pending
    }

    pub(crate) fn take(&mut self) -> Vec<ProgramStatusEvent> {
        if !std::mem::take(&mut self.pending) {
            return Vec::new();
        }
        vec![ProgramStatusEvent::Snapshot(self.status.clone())]
    }
}

pub(crate) fn reduce(buffer: &mut ProgramStatusBuffer, event: ProgramStatusEvent) {
    if !buffer.status.protocol_seen()
        && matches!(
            event,
            ProgramStatusEvent::Key
                | ProgramStatusEvent::Prompt
                | ProgramStatusEvent::ProcessExit
                | ProgramStatusEvent::FullReset
        )
    {
        return;
    }
    match event {
        ProgramStatusEvent::Snapshot(_) => return,
        ProgramStatusEvent::Report {
            state,
            kind,
            progress,
            id,
            app,
            title,
            message,
        } => {
            let result = if state == 5 {
                buffer.status.clear(&id)
            } else {
                let state = match state {
                    0 => State::Idle,
                    1 => State::Working,
                    2 => State::Done,
                    3 => State::Blocked,
                    4 => State::Error,
                    _ => return,
                };
                buffer.status.apply(Incoming {
                    state,
                    kind: match kind {
                        1 => Some(BlockedKind::Permission),
                        2 => Some(BlockedKind::Question),
                        3 => Some(BlockedKind::Auth),
                        _ => None,
                    },
                    progress: (progress >= 0).then_some(progress as u8),
                    id: &id,
                    app: (!app.is_empty()).then_some(app.as_str()),
                    title: (!title.is_empty()).then_some(title.as_str()),
                    message: (!message.is_empty()).then_some(message.as_str()),
                })
            };
            if result.is_err() {
                return;
            }
        }
        ProgramStatusEvent::Prompt => {
            buffer.status.on_shell_prompt();
        }
        ProgramStatusEvent::ProcessExit => {
            buffer.status.on_process_exit();
        }
        ProgramStatusEvent::FullReset => {
            buffer.status.on_full_reset();
        }
        ProgramStatusEvent::Key => {
            buffer.status.acknowledge();
        }
    }
    buffer.pending = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: i32, id: &str) -> ProgramStatusEvent {
        ProgramStatusEvent::Report {
            state,
            kind: 0,
            progress: -1,
            id: id.into(),
            app: String::new(),
            title: String::new(),
            message: String::new(),
        }
    }

    fn snapshot(buffer: &mut ProgramStatusBuffer) -> SurfaceProgramStatus {
        let events = buffer.take();
        assert_eq!(events.len(), 1);
        let ProgramStatusEvent::Snapshot(status) = events.into_iter().next().unwrap() else {
            panic!("expected reduced snapshot");
        };
        assert!(buffer.take().is_empty());
        status
    }

    #[test]
    fn a_burst_never_drops_subtree_clear_or_reset() {
        let mut buffer = ProgramStatusBuffer::default();
        reduce(&mut buffer, report(3, "old/child"));
        snapshot(&mut buffer);
        reduce(&mut buffer, report(5, "old"));
        for _ in 0..4096 {
            reduce(&mut buffer, report(1, "live"));
        }
        let status = snapshot(&mut buffer);
        assert_eq!(status.records().len(), 1);
        assert_eq!(status.records()[0].id.path(), "live");
        reduce(&mut buffer, ProgramStatusEvent::FullReset);
        for _ in 0..4096 {
            reduce(&mut buffer, report(1, "next"));
        }
        let status = snapshot(&mut buffer);
        assert_eq!(status.records().len(), 1);
        assert_eq!(status.records()[0].id.path(), "next");
    }

    #[test]
    fn input_and_prompt_order_survive_coalescing() {
        let mut buffer = ProgramStatusBuffer::default();
        reduce(&mut buffer, report(2, "done"));
        reduce(&mut buffer, ProgramStatusEvent::Key);
        reduce(&mut buffer, report(2, "done"));
        reduce(&mut buffer, report(1, "running"));
        reduce(&mut buffer, report(3, "blocked"));
        reduce(&mut buffer, ProgramStatusEvent::Prompt);
        let status = snapshot(&mut buffer);
        assert_eq!(status.records().len(), 1);
        assert!(!status.records()[0].unseen);
        assert!(status.protocol_seen());
        reduce(&mut buffer, ProgramStatusEvent::FullReset);
        assert!(!snapshot(&mut buffer).protocol_seen());
    }

    #[test]
    fn ordinary_shell_input_never_allocates_pending_status_snapshots() {
        let mut buffer = ProgramStatusBuffer::default();
        for _ in 0..4096 {
            reduce(&mut buffer, ProgramStatusEvent::Key);
            reduce(&mut buffer, ProgramStatusEvent::Prompt);
        }
        assert!(buffer.take().is_empty());
    }
}
