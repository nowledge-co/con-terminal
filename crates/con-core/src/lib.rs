pub mod config;
pub mod control;
pub mod harness;
pub mod release_channel;
pub mod session;
pub mod suggestions;
pub mod tab_summary;
pub mod terminal_status;
pub mod terminal_title;
pub mod workspace_layout;

pub use config::Config;
pub use control::{
    AgentAskResult, ControlCommand, ControlError, ControlMethodInfo, ControlRequestEnvelope,
    ControlResult, ControlSocketHandle, DEFAULT_SOCKET_PATH, JSON_RPC_VERSION, JsonRpcRequest,
    JsonRpcResponse, PaneTarget, SurfaceTarget, SystemIdentifyResult, TabInfo, control_methods,
    control_socket_path, spawn_control_socket_server,
};
pub use harness::{AgentHarness, AgentSession};
pub use suggestions::{SuggestionContext, SuggestionEngine};
pub use tab_summary::{TabIconKind, TabSummary, TabSummaryEngine, TabSummaryRequest};
