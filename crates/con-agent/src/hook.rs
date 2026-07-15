use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Sender;
use rig::agent::{AgentHook, Flow, HookContext, StepEvent, StepEventKind};
use rig::completion::CompletionModel;

use crate::provider::AgentEvent;

/// Tool approval decision sent from the UI back to the hook.
#[derive(Debug, Clone)]
pub struct ToolApprovalDecision {
    pub call_id: String,
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Hook that bridges Rig's agent loop into con's event system.
///
/// Emits `AgentEvent`s for tool calls and tool results so the UI
/// can show what the agent is doing in real time.
///
/// For dangerous tools (terminal_exec, shell_exec, file_write, edit_file),
/// blocks on an approval channel waiting for the user's decision before proceeding.
///
/// ## Channel design
///
/// Each `ConHook` instance gets its own dedicated `approval_rx`.
/// The harness creates a fresh channel pair per `send_message()` call
/// and passes the sender to the UI via a `ToolApprovalNeeded` harness
/// event. This avoids race conditions: only one hook instance reads
/// from each channel, and tool calls within a single agent request
/// are sequential (Rig's default concurrency is 1).
///
/// ## Streaming
///
/// `on_text_delta` fires during streaming via `stream_prompt()`,
/// emitting `AgentEvent::Token` for each text chunk. The UI
/// receives these incrementally for real-time text rendering.
#[derive(Clone)]
pub struct ConHook {
    event_tx: Sender<AgentEvent>,
    approval_rx: crossbeam_channel::Receiver<ToolApprovalDecision>,
    auto_approve: bool,
    cancel_flag: Arc<AtomicBool>,
}

impl ConHook {
    pub fn new(
        event_tx: Sender<AgentEvent>,
        approval_rx: crossbeam_channel::Receiver<ToolApprovalDecision>,
        auto_approve: bool,
        cancel_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            event_tx,
            approval_rx,
            auto_approve,
            cancel_flag,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(
        auto_approve: bool,
        cancelled: bool,
    ) -> (
        ConHook,
        crossbeam_channel::Receiver<AgentEvent>,
        crossbeam_channel::Sender<ToolApprovalDecision>,
    ) {
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let (approval_tx, approval_rx) = crossbeam_channel::unbounded();
        (
            ConHook::new(
                event_tx,
                approval_rx,
                auto_approve,
                Arc::new(AtomicBool::new(cancelled)),
            ),
            event_rx,
            approval_tx,
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn safe_tool_continues_without_approval() {
        let (hook, event_rx, _approval_tx) = hook(false, false);

        let flow = hook.on_tool_call("read_pane", "call-1", "{}").await;

        assert_eq!(flow, Flow::Continue);
        assert!(matches!(
            event_rx.try_recv(),
            Ok(AgentEvent::ToolCallStart { call_id, tool_name, .. })
                if call_id == "call-1" && tool_name == "read_pane"
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dangerous_tool_honors_denial_reason() {
        let (hook, event_rx, approval_tx) = hook(false, false);
        approval_tx
            .send(ToolApprovalDecision {
                call_id: "call-2".into(),
                allowed: false,
                reason: Some("Not approved".into()),
            })
            .unwrap();

        let flow = hook.on_tool_call("terminal_exec", "call-2", "{}").await;

        assert_eq!(
            flow,
            Flow::Skip {
                reason: "Not approved".into()
            }
        );
        assert!(matches!(
            event_rx.try_recv(),
            Ok(AgentEvent::ToolCallStart { call_id, .. }) if call_id == "call-2"
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_releases_pending_approval() {
        let (hook, _event_rx, _approval_tx) = hook(false, true);

        let flow = hook.on_tool_call("file_write", "call-3", "{}").await;

        assert_eq!(
            flow,
            Flow::Skip {
                reason: "Tool approval timed out or cancelled".into()
            }
        );
    }
}

pub fn is_dangerous(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "shell_exec"
            | "terminal_exec"
            | "batch_exec"
            | "file_write"
            | "edit_file"
            | "send_keys"
            | "ensure_remote_shell_target"
            | "remote_exec"
            | "ensure_local_coding_workspace"
            | "tmux_run_command"
            | "tmux_ensure_shell_target"
            | "tmux_ensure_agent_target"
    )
}

/// Poll interval when waiting for approval. Short enough to respond
/// quickly to cancellation, long enough to avoid busy-spinning.
const APPROVAL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// Total approval timeout: 5 minutes.
const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

impl<M: CompletionModel> AgentHook<M> for ConHook {
    async fn on_event(&self, _ctx: &HookContext, event: StepEvent<'_, M>) -> Flow {
        match event {
            StepEvent::ToolCall {
                tool_name,
                internal_call_id,
                args,
                ..
            } => self.on_tool_call(tool_name, internal_call_id, args).await,
            StepEvent::ToolResult {
                tool_name,
                internal_call_id,
                result,
                ..
            } => {
                let _ = self.event_tx.send(AgentEvent::ToolCallComplete {
                    call_id: internal_call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    result: result.to_string(),
                });
                Flow::cont()
            }
            StepEvent::TextDelta { delta, .. } => {
                let _ = self.event_tx.send(AgentEvent::Token(delta.to_string()));
                Flow::cont()
            }
            _ => Flow::cont(),
        }
    }

    fn observes(&self, kind: StepEventKind) -> bool {
        matches!(kind, StepEventKind::TextDelta)
    }
}

impl ConHook {
    async fn on_tool_call(&self, tool_name: &str, internal_call_id: &str, args: &str) -> Flow {
        let call_id = internal_call_id.to_string();
        let tool_name = tool_name.to_string();
        let args = args.to_string();
        let event_tx = self.event_tx.clone();
        let approval_rx = self.approval_rx.clone();
        let auto_approve = self.auto_approve;
        let cancel_flag = self.cancel_flag.clone();

        async move {
            let _ = event_tx.send(AgentEvent::ToolCallStart {
                call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
            });

            if !is_dangerous(&tool_name) || auto_approve {
                return Flow::cont();
            }

            // Poll for approval with short intervals so we can respond
            // to cancellation (e.g. app quit) without blocking shutdown.
            let decision = tokio::task::block_in_place(|| {
                let deadline = std::time::Instant::now() + APPROVAL_TIMEOUT;
                loop {
                    // Check cancellation first — enables clean shutdown
                    if cancel_flag.load(Ordering::Relaxed) {
                        return None;
                    }

                    match approval_rx.recv_timeout(APPROVAL_POLL_INTERVAL) {
                        Ok(decision) => return Some(decision),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            if std::time::Instant::now() >= deadline {
                                return None;
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            return None;
                        }
                    }
                }
            });

            match decision {
                Some(d) if d.allowed => Flow::cont(),
                Some(d) => Flow::skip(
                    d.reason
                        .unwrap_or_else(|| "User denied tool execution".to_string()),
                ),
                None => Flow::skip("Tool approval timed out or cancelled"),
            }
        }
        .await
    }
}
