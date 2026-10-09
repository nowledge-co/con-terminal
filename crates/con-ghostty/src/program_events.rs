//! Owned OSC 7501 facts copied out of a Ghostty callback.
//!
//! The library borrows report text only for the duration of the callback.
//! Each backend keeps these events until the workspace applies them.

use std::collections::VecDeque;

/// How many reports to retain if the UI has not drained them yet.
const CAP: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramStatusEvent {
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

pub(crate) fn push_capped(queue: &mut VecDeque<ProgramStatusEvent>, event: ProgramStatusEvent) {
    if queue.len() >= CAP {
        queue.pop_front();
    }
    queue.push_back(event);
}
