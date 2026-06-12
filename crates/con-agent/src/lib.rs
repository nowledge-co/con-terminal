pub mod context;
pub mod control;
pub mod conversation;
pub mod hook;
pub mod playbooks;
pub mod provider;
pub mod shell_probe;
pub mod skills;
pub mod tmux;
pub mod tools;

pub use context::{
    PaneActionKind, PaneActionRecord, PaneRuntimeEvent, PaneRuntimeTracker, PaneShellContext,
    TerminalContext,
};
pub use control::{
    PaneAddressSpace, PaneAttachmentAuthority, PaneAttachmentKind, PaneAttachmentTransport,
    PaneControlCapability, PaneControlChannel, PaneControlState, PaneProtocolAttachment,
    PaneVisibleTarget, PaneVisibleTargetKind, TmuxControlMode, TmuxControlState,
};
pub use conversation::{Conversation, ConversationSummary, Message, MessageRole};
pub use hook::{ConHook, ToolApprovalDecision, is_dangerous};
pub use provider::{
    AgentConfig, AgentEvent, AgentProvider, OAuthDevicePrompt, ProviderConfig, ProviderKind,
    ProviderMap, SuggestionModelConfig, authorize_oauth_provider, oauth_token_dir,
    read_chatgpt_oauth_access_token,
};
pub use shell_probe::{ShellProbeResult, ShellProbeTmuxContext};
pub use skills::{Skill, SkillRegistry};
pub use tmux::{TmuxCapture, TmuxExecLocation, TmuxExecResult, TmuxPaneInfo, TmuxSnapshot};
pub use tools::{
    AgentCliTurnTool, BatchExecTool, CreatePaneTool, EditFileTool, EnsureLocalAgentTargetTool,
    EnsureLocalCodingWorkspaceTool, EnsureLocalShellTargetTool, EnsureRemoteShellTargetTool,
    EnsureRemoteTmuxShellTargetTool, EnsureRemoteTmuxWorkspaceTool, FileReadTool, FileWriteTool,
    ListFilesTool, ListPanesTool, ListTabWorkspacesTool, PaneCreateLocation, PaneInfo, PaneQuery,
    PaneRequest, PaneResponse, ProbeShellContextTool, ReadPaneTool, RemoteExecTool,
    ResolveWorkTargetTool, SearchPanesTool, SearchTool, SendKeysTool, ShellExecTool,
    TerminalExecRequest, TerminalExecResponse, TerminalExecTool, TmuxCaptureTool,
    TmuxEnsureShellTargetTool, TmuxFindTargetsTool, TmuxInspectTool, TmuxListTool,
    TmuxRunCommandTool, TmuxSendKeysTool, TmuxShellTurnTool, WaitForTool,
};
