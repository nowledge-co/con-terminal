//! Read-only source adapters and native target launch capabilities.
mod binding;
mod codex;
mod environment;
mod inventory;
mod kind;
mod launch;
mod model;
#[cfg(target_os = "macos")]
mod procinfo;
mod protocol;
mod sanitize;
mod sources;
mod target;
mod types;

pub use binding::{
    SessionBinding, active_session_binding, agent_from_process_argv, agent_from_process_group,
    kimi_session_not_started,
};
#[cfg(target_os = "macos")]
pub use procinfo::process_start_secs;
#[cfg(not(target_os = "macos"))]
pub fn process_start_secs(_pid: i32) -> Option<u64> {
    None
}
pub use codex::{active_codex_thread_id, discover_codex, export_codex};
pub use environment::{launch_environment, probe_environment};
pub use inventory::{installed_agents, refresh_installed_agents};
pub use kind::AgentKind;
pub use launch::{create_target, kimi_delivery_payload, target_args, target_args_with_model};
pub use model::candidate_models;
pub use protocol::process_path as native_agent_path;
pub use sanitize::{digest, sanitize};
pub use sources::{discover_sessions, export_session};
pub use target::probe_target;
pub use types::*;
