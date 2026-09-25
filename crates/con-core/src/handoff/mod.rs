//! Durable, local handoff jobs with independent snapshots and delivery state.
//! Multiple jobs may share a worktree; only in-flight delivery to one existing Tab is exclusive.
mod cleanup;
mod context;
mod fallback;
pub use fallback::*;
mod coordinator;
mod rpc;
mod snapshot;
mod store;
mod types;
mod validate;

pub use context::instruction;
pub use coordinator::HandoffService;
pub use rpc::HandoffRpc;
pub use store::HandoffLock;
pub use types::*;

pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod delivery_regression;
