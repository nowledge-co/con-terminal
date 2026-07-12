use crossbeam_channel::Sender;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::clipboard::Clipboard;
use gpui_component::divider::Divider;
use gpui_component::input::{Input, InputEvent, InputState, Position};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::scroll::ScrollableElement;
use gpui_component::spinner::Spinner;
use gpui_component::{ActiveTheme, Disableable, Icon, Sizable as _, Theme};

/// Max lines to show for tool result previews in collapsed steps
const TOOL_RESULT_PREVIEW_LINES: usize = 6;
const LONG_MARKDOWN_PROGRESSIVE_THRESHOLD_BLOCKS: usize = 14;
const LONG_MARKDOWN_INITIAL_BLOCKS: usize = 8;
const LONG_MARKDOWN_BATCH_BLOCKS: usize = 6;
const LONG_MARKDOWN_BATCH_DELAY_MS: u64 = 8;
use con_agent::{
    AgentConfig, ConversationSummary, ProviderKind, ToolApprovalDecision, conversation::AgentStep,
    oauth_token_dir,
};
use con_core::harness::HarnessEvent;

use chrono::Utc;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use crate::chat_markdown::{
    ChatMarkdownBlockView, ChatMarkdownTone, ParsedChatMarkdown, chat_markdown_block_gap,
    render_parsed_chat_markdown, render_parsed_chat_markdown_prefix_with_copy_namespace,
};
use crate::input_bar::SkillEntry;
use crate::motion::{MotionValue, vertical_reveal_offset};
use crate::settings_panel::provider_label;
use crate::ui_scale::{mono_px, mono_space_px, ui_px, ui_space_px};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgentStatus {
    Idle,
    Thinking,
    Responding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FollowOutputState {
    Auto,
    Detached,
    PendingJump,
}

struct ToolCallEntry {
    call_id: String,
    tool_name: String,
    args: String,
    result: Option<String>,
    result_expanded: bool,
    started_at: std::time::Instant,
    duration: Option<std::time::Duration>,
}

struct PendingApproval {
    call_id: String,
    tool_name: String,
    args: String,
    approval_tx: Sender<ToolApprovalDecision>,
}

pub struct NewConversation;
impl EventEmitter<NewConversation> for AgentPanel {}

pub struct LoadConversation {
    pub id: String,
}
impl EventEmitter<LoadConversation> for AgentPanel {}

pub struct DeleteConversation {
    pub id: String,
}
impl EventEmitter<DeleteConversation> for AgentPanel {}

pub struct InlineInputSubmit {
    pub text: String,
}
impl EventEmitter<InlineInputSubmit> for AgentPanel {}

pub struct InlineSkillAutocompleteChanged;
impl EventEmitter<InlineSkillAutocompleteChanged> for AgentPanel {}

pub struct CancelRequest;
impl EventEmitter<CancelRequest> for AgentPanel {}

pub struct SetAutoApprove {
    pub enabled: bool,
}
impl EventEmitter<SetAutoApprove> for AgentPanel {}

pub struct SelectSessionModel {
    pub model: String,
}
impl EventEmitter<SelectSessionModel> for AgentPanel {}

pub struct SelectSessionProvider {
    pub provider: ProviderKind,
}
impl EventEmitter<SelectSessionProvider> for AgentPanel {}

/// Emitted when user edits and resubmits a previous message.
/// The workspace truncates the conversation and re-sends.
pub struct RerunFromMessage {
    pub content: String,
}
impl EventEmitter<RerunFromMessage> for AgentPanel {}

// ---------------------------------------------------------------------------
// PanelState — per-tab conversation UI state (no GPUI dependency)
// ---------------------------------------------------------------------------

const SYSTEM_GREETING: &str = "Ask anything about your terminal, code, or system. The agent can read files, run commands, and search your workspace.";

/// Per-tab panel state. Holds messages, tool calls, and approvals.
/// Methods on PanelState are pure data operations — no GPUI notifications.
/// AgentPanel delegates to PanelState and calls cx.notify() itself.
pub struct PanelState {
    messages: Vec<PanelMessage>,
    tool_calls: Vec<ToolCallEntry>,
    pending_approvals: Vec<PendingApproval>,
    pub(crate) streaming: bool,
    pub(crate) status: AgentStatus,
}

impl PanelState {
    pub fn new() -> Self {
        Self {
            messages: vec![PanelMessage::new("system", SYSTEM_GREETING)],
            tool_calls: Vec::new(),
            pending_approvals: Vec::new(),
            streaming: false,
            status: AgentStatus::Idle,
        }
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Populate from a loaded conversation, including persisted thinking and steps.
    pub fn from_conversation(conv: &con_agent::Conversation) -> Self {
        let mut state = Self::new();
        for msg in &conv.messages {
            let role = match msg.role {
                con_agent::MessageRole::User => "user",
                con_agent::MessageRole::Assistant => "assistant",
                con_agent::MessageRole::System => "system",
                con_agent::MessageRole::Tool => "system",
            };
            state.restore_message(role, &msg.content, msg.model.as_deref(), msg.duration_ms);
            state.restore_last_assistant_trace(msg.thinking.as_deref(), &msg.steps);
        }
        state
    }

    pub fn clear(&mut self) {
        self.messages = vec![PanelMessage::new("system", SYSTEM_GREETING)];
        self.tool_calls.clear();
        self.pending_approvals.clear();
        self.streaming = false;
        self.status = AgentStatus::Idle;
    }

    /// Truncate conversation back to (but not including) the message at `msg_idx`.
    /// Used for edit-and-rerun: removes the old user message and everything after it.
    pub fn truncate_to(&mut self, msg_idx: usize) {
        if msg_idx < self.messages.len() {
            self.messages.truncate(msg_idx);
        }
        self.tool_calls.clear();
        self.pending_approvals.clear();
        self.streaming = false;
        self.status = AgentStatus::Idle;
    }

    pub fn add_message(&mut self, role: &str, content: &str) {
        self.streaming = false;
        self.status = AgentStatus::Idle;
        self.messages.push(PanelMessage::new(role, content));
    }

    pub fn restore_message(
        &mut self,
        role: &str,
        content: &str,
        model: Option<&str>,
        duration_ms: Option<u64>,
    ) {
        self.streaming = false;
        self.status = AgentStatus::Idle;
        let mut message = PanelMessage::new(role, content);
        message.model = model.map(ToOwned::to_owned);
        message.duration_ms = duration_ms;
        self.messages.push(message);
    }

    pub fn restore_last_assistant_trace(&mut self, thinking: Option<&str>, steps: &[AgentStep]) {
        if let Some(last) = self.messages.last_mut() {
            if last.role == "assistant" {
                last.set_thinking(thinking.map(ToOwned::to_owned));
                last.steps = restored_steps_from_agent_steps(steps);
                last.thinking_collapsed = true;
                if !last.steps.is_empty() {
                    last.steps_collapsed = true;
                }
                last.touch();
            }
        }
    }

    pub fn add_step(&mut self, step: &str) {
        self.status = AgentStatus::Thinking;
        if let Some(last) = self.messages.last_mut() {
            last.steps.push(StepEntry {
                icon: "phosphor/brain-duotone.svg",
                label: step.to_string(),
                detail: None,
                status: StepStatus::Complete,
                detail_collapsed: true,
                detail_expanded: false,
                duration: None,
            });
            last.touch();
        }
    }

    pub fn add_tool_call(&mut self, call_id: &str, tool_name: &str, args: &str) {
        self.status = AgentStatus::Thinking;
        self.tool_calls.push(ToolCallEntry {
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            args: args.to_string(),
            result: None,
            result_expanded: false,
            started_at: std::time::Instant::now(),
            duration: None,
        });
    }

    pub fn complete_tool_call(&mut self, call_id: &str, result: &str) {
        if let Some(entry) = self.tool_calls.iter_mut().find(|e| e.call_id == call_id) {
            entry.duration = Some(entry.started_at.elapsed());
            entry.result = Some(result.to_string());
        }
    }

    pub fn add_pending_approval(
        &mut self,
        call_id: &str,
        tool_name: &str,
        args: &str,
        approval_tx: Sender<ToolApprovalDecision>,
    ) {
        self.pending_approvals.push(PendingApproval {
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            args: args.to_string(),
            approval_tx,
        });
    }

    pub fn update_thinking(&mut self, text: &str) {
        self.status = AgentStatus::Thinking;
        if !self.streaming {
            let mut msg = PanelMessage::assistant();
            msg.set_thinking(Some(String::new()));
            self.messages.push(msg);
            self.streaming = true;
        }
        if let Some(last) = self.messages.last_mut() {
            last.append_thinking(text);
        }
    }

    pub fn update_streaming(&mut self, token: &str) {
        self.status = AgentStatus::Responding;
        if !self.streaming {
            self.messages.push(PanelMessage::assistant());
            self.streaming = true;
        }
        if let Some(last) = self.messages.last_mut() {
            last.append_content(token);
        }
    }

    pub fn complete_response(
        &mut self,
        final_content: &str,
        model: Option<&str>,
        duration_ms: Option<u64>,
    ) {
        self.status = AgentStatus::Idle;
        if self.streaming {
            // Only overwrite streamed content if final_content is non-empty.
            // Some providers don't emit text items, leaving final_content empty —
            // in that case, keep whatever was accumulated during streaming.
            if !final_content.is_empty() {
                if let Some(last) = self.messages.last_mut() {
                    last.replace_content(final_content);
                }
            }
            // Attach metadata to the last assistant message
            if let Some(last) = self.messages.last_mut() {
                if last.role == "assistant" {
                    last.model = model.map(|s| s.to_string());
                    last.duration_ms = duration_ms;
                    last.touch();
                }
            }
            self.streaming = false;
        } else if !final_content.is_empty() {
            let mut msg = PanelMessage::new("assistant", final_content);
            msg.model = model.map(|s| s.to_string());
            msg.duration_ms = duration_ms;
            self.messages.push(msg);
        }
        // Ensure a message exists for tool call steps even when no text was produced.
        // This happens when the agent only used tools without generating text output.
        if !self.tool_calls.is_empty()
            && self.messages.last().map_or(true, |m| m.role != "assistant")
        {
            let mut msg = PanelMessage::new("assistant", "");
            msg.model = model.map(|s| s.to_string());
            msg.duration_ms = duration_ms;
            self.messages.push(msg);
        }
        // Move active tool calls into the message's step timeline
        for tc in self.tool_calls.drain(..) {
            let args_display = format_tool_args(&tc.tool_name, &tc.args);
            let human_name = humanize_tool_name(&tc.tool_name);
            let (status, detail) = if let Some(result) = &tc.result {
                let formatted = format_tool_result(&tc.tool_name, &result);
                (StepStatus::Complete, Some(formatted))
            } else {
                (StepStatus::Complete, None)
            };
            if let Some(last) = self.messages.last_mut() {
                last.steps.push(StepEntry {
                    icon: tool_icon(&tc.tool_name),
                    label: format!("{}: {}", human_name, truncate_str(&args_display, 60)),
                    detail,
                    status,
                    detail_collapsed: true,
                    detail_expanded: false,
                    duration: tc.duration,
                });
                last.touch();
            }
        }
        // Auto-collapse steps on completed messages to reduce scroll noise. This
        // must happen after live tool calls are moved into the step timeline.
        if let Some(last) = self.messages.last_mut() {
            if !last.steps.is_empty() {
                last.steps_collapsed = true;
                last.touch();
            }
        }
    }

    /// Apply a harness event to this state (for background tabs, no GPUI).
    pub fn apply_event(&mut self, event: HarnessEvent) {
        match event {
            HarnessEvent::Thinking => {
                self.add_step("Thinking...");
            }
            HarnessEvent::ThinkingDelta(text) => {
                self.update_thinking(&text);
            }
            HarnessEvent::Step(step) => {
                let label = describe_agent_step(&step);
                self.add_step(&label);
            }
            HarnessEvent::Token(token) => {
                self.update_streaming(&token);
            }
            HarnessEvent::ToolCallStart {
                call_id,
                tool_name,
                args,
            } => {
                self.add_tool_call(&call_id, &tool_name, &args);
            }
            HarnessEvent::ToolApprovalNeeded {
                call_id,
                tool_name,
                args,
                approval_tx,
            } => {
                self.add_pending_approval(&call_id, &tool_name, &args, approval_tx);
            }
            HarnessEvent::ToolCallComplete {
                call_id,
                tool_name: _,
                result,
            } => {
                self.complete_tool_call(&call_id, &result);
            }
            HarnessEvent::ResponseComplete(msg) => {
                self.complete_response(&msg.content, msg.model.as_deref(), msg.duration_ms);
            }
            HarnessEvent::Error(err) => {
                self.add_message("system", &format!("Error: {}", err));
            }
            HarnessEvent::SkillsUpdated(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// AgentPanel — GPUI entity wrapping PanelState
// ---------------------------------------------------------------------------

pub struct AgentPanel {
    state: PanelState,
    assistant_message_views: Vec<Option<Entity<AssistantMessageView>>>,
    message_list_state: ListState,
    message_list_handler_installed: bool,
    history_scroll_handle: ScrollHandle,
    transcript_footer_scroll_handle: ScrollHandle,
    showing_history: bool,
    conversation_list: Vec<ConversationSummary>,
    auto_approve: bool,
    current_provider: Option<ProviderKind>,
    session_provider_options: Vec<ProviderSelectItem>,
    model_name: String,
    session_model_options: Vec<String>,
    content_reveal: MotionValue,
    /// Index of user message currently being edited inline (None = not editing)
    editing_msg_idx: Option<usize>,
    /// Input state for the inline edit field
    edit_input_state: Option<Entity<InputState>>,
    /// When true, show a compact inline input at the bottom of the panel
    /// (used when the main input bar is hidden but agent panel is open)
    show_inline_input: bool,
    /// Input state for the inline input at bottom of agent panel
    inline_input_state: Option<Entity<InputState>>,
    /// Skills available for /slash-command completion in inline input
    skills: Vec<SkillEntry>,
    /// Global submitted-input history for the inline composer.
    recent_inputs: Vec<String>,
    inline_history_nav_index: Option<usize>,
    inline_history_nav_draft: Option<String>,
    inline_input_has_text: bool,
    inline_input_has_skills: bool,
    follow_output: FollowOutputState,
    /// Currently highlighted skill in inline autocomplete
    inline_skill_selection: usize,
    /// Tracks whether shift was held on the last enter keystroke (for inline input)
    inline_shift_enter: bool,
    ui_opacity: f32,
}

#[derive(Clone)]
struct ProviderSelectItem {
    provider: ProviderKind,
    short_label: &'static str,
    label: &'static str,
    icon_path: &'static str,
}

#[derive(Clone)]
struct PanelMessage {
    id: u64,
    revision: u64,
    role: String,
    content: String,
    content_markdown: Option<Arc<ParsedChatMarkdown>>,
    content_markdown_len: usize,
    content_markdown_pending: bool,
    /// Extended thinking/reasoning text from the model (collapsible)
    thinking: Option<String>,
    thinking_markdown: Option<Arc<ParsedChatMarkdown>>,
    thinking_collapsed: bool,
    steps: Vec<StepEntry>,
    steps_collapsed: bool,
    /// Model name for assistant messages (e.g. "claude-sonnet-4-6")
    model: Option<String>,
    /// Response duration in milliseconds for assistant messages
    duration_ms: Option<u64>,
}

/// A structured step entry (replaces raw Debug strings).
#[derive(Clone)]
struct StepEntry {
    icon: &'static str,
    label: String,
    /// Full formatted result (may be multi-line JSON, command output, etc.)
    detail: Option<String>,
    status: StepStatus,
    /// Whether the detail section is collapsed (default: true)
    detail_collapsed: bool,
    /// Whether the visible result content is fully expanded.
    detail_expanded: bool,
    /// How long this step took to execute.
    duration: Option<std::time::Duration>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum StepStatus {
    Running,
    Complete,
    Denied,
}

impl PanelMessage {
    fn new(role: &str, content: &str) -> Self {
        Self {
            id: next_panel_message_id(),
            revision: 0,
            role: role.to_string(),
            content: content.to_string(),
            content_markdown: None,
            content_markdown_len: 0,
            content_markdown_pending: false,
            thinking: None,
            thinking_markdown: None,
            thinking_collapsed: true,
            steps: Vec::new(),
            steps_collapsed: false,
            model: None,
            duration_ms: None,
        }
    }

    fn assistant() -> Self {
        Self::new("assistant", "")
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn append_content(&mut self, token: &str) {
        self.content.push_str(token);
        self.touch();
    }

    fn replace_content(&mut self, content: &str) {
        self.content = content.to_string();
        self.content_markdown = None;
        self.content_markdown_len = 0;
        self.content_markdown_pending = false;
        self.touch();
    }

    fn set_thinking(&mut self, thinking: Option<String>) {
        self.thinking_markdown = None;
        self.thinking = thinking;
        self.touch();
    }

    fn append_thinking(&mut self, text: &str) {
        let thinking = self.thinking.get_or_insert_with(String::new);
        thinking.push_str(text);
        self.thinking_markdown = None;
        self.touch();
    }

    fn ensure_thinking_markdown(&mut self) {
        if self.thinking_markdown.is_some() {
            return;
        }

        let Some(thinking) = self.thinking.as_deref().filter(|text| !text.is_empty()) else {
            return;
        };

        self.thinking_markdown = Some(Arc::new(ParsedChatMarkdown::parse(thinking)));
    }
}

static NEXT_PANEL_MESSAGE_ID: AtomicU64 = AtomicU64::new(1);

fn next_panel_message_id() -> u64 {
    NEXT_PANEL_MESSAGE_ID.fetch_add(1, Ordering::Relaxed)
}

struct AssistantMessageView {
    panel: WeakEntity<AgentPanel>,
    msg_idx: usize,
    message: PanelMessage,
    state_key: AssistantMessageStateKey,
    is_streaming_assistant: bool,
    rendered_markdown_blocks: usize,
    markdown_hydration_pending: bool,
    markdown_block_views: Vec<Option<Entity<ChatMarkdownBlockView>>>,
    markdown_block_view_len: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct AssistantMessageStateKey {
    revision: u64,
    content_len: usize,
    content_head: String,
    content_markdown_len: usize,
    content_markdown_blocks: usize,
}

impl AssistantMessageView {
    fn new(
        panel: WeakEntity<AgentPanel>,
        msg_idx: usize,
        message: PanelMessage,
        state_key: AssistantMessageStateKey,
        is_streaming_assistant: bool,
    ) -> Self {
        let rendered_markdown_blocks =
            Self::initial_markdown_block_limit(state_key.content_markdown_blocks);
        Self {
            panel,
            msg_idx,
            message,
            state_key,
            is_streaming_assistant,
            rendered_markdown_blocks,
            markdown_hydration_pending: false,
            markdown_block_views: Vec::new(),
            markdown_block_view_len: 0,
        }
    }

    fn update_state(
        &mut self,
        msg_idx: usize,
        message: PanelMessage,
        state_key: AssistantMessageStateKey,
        is_streaming_assistant: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = self.msg_idx != msg_idx
            || self.is_streaming_assistant != is_streaming_assistant
            || self.state_key != state_key;
        if changed {
            let same_message_family = self.msg_idx == msg_idx
                && state_key.content_len >= self.state_key.content_len
                && state_key.content_head == self.state_key.content_head
                && state_key.content_markdown_len >= self.state_key.content_markdown_len;

            self.msg_idx = msg_idx;
            self.message = message;
            self.state_key = state_key;
            self.is_streaming_assistant = is_streaming_assistant;
            if self.state_key.content_markdown_blocks == 0 {
                self.rendered_markdown_blocks = 0;
                self.markdown_hydration_pending = false;
                self.markdown_block_views.clear();
                self.markdown_block_view_len = 0;
            } else if !same_message_family
                || self.rendered_markdown_blocks == 0
                || self.rendered_markdown_blocks > self.state_key.content_markdown_blocks
            {
                self.rendered_markdown_blocks =
                    Self::initial_markdown_block_limit(self.state_key.content_markdown_blocks);
                self.markdown_hydration_pending = false;
                self.markdown_block_views.clear();
                self.markdown_block_view_len = self.state_key.content_markdown_len;
            } else {
                self.rendered_markdown_blocks = self
                    .rendered_markdown_blocks
                    .min(self.state_key.content_markdown_blocks);
                if self.markdown_block_view_len != self.state_key.content_markdown_len {
                    self.markdown_block_views.clear();
                    self.markdown_block_view_len = self.state_key.content_markdown_len;
                }
            }
            cx.notify();
        }
    }

    fn initial_markdown_block_limit(total_blocks: usize) -> usize {
        if total_blocks <= LONG_MARKDOWN_PROGRESSIVE_THRESHOLD_BLOCKS {
            total_blocks
        } else {
            total_blocks.min(LONG_MARKDOWN_INITIAL_BLOCKS)
        }
    }

    fn schedule_markdown_hydration(&mut self, cx: &mut Context<Self>) {
        if self.markdown_hydration_pending
            || self.state_key.content_markdown_blocks <= self.rendered_markdown_blocks
            || self.state_key.content_markdown_blocks <= LONG_MARKDOWN_PROGRESSIVE_THRESHOLD_BLOCKS
        {
            return;
        }

        self.markdown_hydration_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(LONG_MARKDOWN_BATCH_DELAY_MS))
                .await;
            let _ = this.update(cx, |view, cx| {
                view.markdown_hydration_pending = false;
                let total_blocks = view.state_key.content_markdown_blocks;
                if total_blocks <= view.rendered_markdown_blocks {
                    return;
                }
                view.rendered_markdown_blocks =
                    (view.rendered_markdown_blocks + LONG_MARKDOWN_BATCH_BLOCKS).min(total_blocks);
                if view.rendered_markdown_blocks < total_blocks {
                    view.schedule_markdown_hydration(cx);
                }
                let msg_idx = view.msg_idx;
                let panel = view.panel.clone();
                let _ = panel.update(cx, |panel, cx| {
                    panel.remeasure_message(msg_idx);
                    cx.notify();
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn render_markdown_blocks(
        &mut self,
        markdown: &Arc<ParsedChatMarkdown>,
        visible_blocks: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.markdown_block_view_len != self.state_key.content_markdown_len {
            self.markdown_block_views.clear();
            self.markdown_block_view_len = self.state_key.content_markdown_len;
        }

        if self.markdown_block_views.len() < visible_blocks {
            self.markdown_block_views
                .resize_with(visible_blocks, || None);
        }

        let gap = chat_markdown_block_gap(ChatMarkdownTone::Message, theme);
        let copy_namespace = format!("asst-{}", self.msg_idx);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(gap)
            .children((0..visible_blocks).map(|block_idx| {
                let document = markdown.clone();
                let view = if let Some(view) = self.markdown_block_views[block_idx].as_ref() {
                    let view = view.clone();
                    view.update(cx, |view, cx| {
                        view.update(
                            document,
                            block_idx,
                            ChatMarkdownTone::Message,
                            copy_namespace.clone(),
                            cx,
                        );
                    });
                    view
                } else {
                    let view = cx.new(|_| {
                        ChatMarkdownBlockView::new(
                            document,
                            block_idx,
                            ChatMarkdownTone::Message,
                            copy_namespace.clone(),
                        )
                    });
                    self.markdown_block_views[block_idx] = Some(view.clone());
                    view
                };
                view.into_any_element()
            }))
            .into_any_element()
    }
}

impl AgentPanel {
    fn session_sidebar_provider_kind(provider: &ProviderKind) -> ProviderKind {
        match provider {
            ProviderKind::MiniMaxAnthropic => ProviderKind::MiniMax,
            ProviderKind::MoonshotAnthropic => ProviderKind::Moonshot,
            ProviderKind::ZAIAnthropic => ProviderKind::ZAI,
            _ => provider.clone(),
        }
    }

    fn provider_short_label(provider: &ProviderKind) -> &'static str {
        match provider {
            ProviderKind::Anthropic => "Claude",
            ProviderKind::OpenAI => "OpenAI",
            ProviderKind::ChatGPT => "ChatGPT",
            ProviderKind::GitHubCopilot => "Copilot",
            ProviderKind::OpenAICompatible => "Compat",
            ProviderKind::MiniMax => "MiniMax",
            ProviderKind::Moonshot => "Moonshot",
            ProviderKind::ZAI => "Z.AI",
            ProviderKind::DeepSeek => "DeepSeek",
            ProviderKind::Groq => "Groq",
            ProviderKind::Gemini => "Gemini",
            ProviderKind::Ollama => "Ollama",
            ProviderKind::OpenRouter => "OpenRouter",
            ProviderKind::Mistral => "Mistral",
            _ => "Provider",
        }
    }

    fn provider_icon_path(provider: &ProviderKind) -> &'static str {
        match provider {
            ProviderKind::Anthropic => "providers/anthropic.svg",
            ProviderKind::OpenAI => "providers/openai.svg",
            ProviderKind::ChatGPT => "providers/openai.svg",
            ProviderKind::GitHubCopilot => "providers/githubcopilot.svg",
            ProviderKind::OpenAICompatible => "phosphor/plugs-connected.svg",
            ProviderKind::MiniMax => "providers/minimax.svg",
            ProviderKind::Moonshot => "providers/moonshot.svg",
            ProviderKind::ZAI => "providers/zai.svg",
            ProviderKind::DeepSeek => "providers/deepseek.svg",
            ProviderKind::Groq => "providers/groq.svg",
            ProviderKind::Gemini => "providers/gemini.svg",
            ProviderKind::Ollama => "providers/ollama.svg",
            ProviderKind::OpenRouter => "providers/openrouter.svg",
            ProviderKind::Mistral => "providers/mistral.svg",
            _ => "phosphor/plugs-connected.svg",
        }
    }

    fn provider_option(provider: ProviderKind) -> ProviderSelectItem {
        let sidebar_provider = Self::session_sidebar_provider_kind(&provider);
        ProviderSelectItem {
            short_label: Self::provider_short_label(&sidebar_provider),
            label: provider_label(&sidebar_provider),
            icon_path: Self::provider_icon_path(&sidebar_provider),
            provider: sidebar_provider,
        }
    }

    fn default_provider_options() -> Vec<ProviderSelectItem> {
        vec![
            ProviderKind::Anthropic,
            ProviderKind::OpenAI,
            ProviderKind::ChatGPT,
            ProviderKind::GitHubCopilot,
            ProviderKind::OpenAICompatible,
            ProviderKind::MiniMax,
            ProviderKind::Moonshot,
            ProviderKind::ZAI,
            ProviderKind::DeepSeek,
            ProviderKind::Groq,
            ProviderKind::Gemini,
            ProviderKind::Ollama,
            ProviderKind::OpenRouter,
            ProviderKind::Mistral,
        ]
        .into_iter()
        .map(Self::provider_option)
        .collect::<Vec<_>>()
    }

    fn provider_is_configured(config: &AgentConfig, provider: &ProviderKind) -> bool {
        let sidebar_provider = Self::session_sidebar_provider_kind(provider);
        if oauth_token_dir(&sidebar_provider).is_some_and(|dir| dir.exists()) {
            return true;
        }

        let has_auth_config = |kind: &ProviderKind| {
            config.providers.get(kind).is_some_and(|provider| {
                let has_api_key = provider
                    .api_key
                    .as_ref()
                    .is_some_and(|v| !v.trim().is_empty())
                    || provider
                        .api_key_env
                        .as_ref()
                        .is_some_and(|v| !v.trim().is_empty());
                if has_api_key {
                    return true;
                }

                *kind == ProviderKind::OpenAICompatible
                    && provider
                        .base_url
                        .as_ref()
                        .is_some_and(|v| !v.trim().is_empty())
            })
        };

        has_auth_config(&sidebar_provider)
            || match sidebar_provider {
                ProviderKind::MiniMax => has_auth_config(&ProviderKind::MiniMaxAnthropic),
                ProviderKind::Moonshot => has_auth_config(&ProviderKind::MoonshotAnthropic),
                ProviderKind::ZAI => has_auth_config(&ProviderKind::ZAIAnthropic),
                _ => false,
            }
    }

    pub fn configured_session_providers(config: &AgentConfig) -> Vec<ProviderKind> {
        let current_provider = Self::session_sidebar_provider_kind(&config.provider);
        let mut providers = Vec::new();
        if Self::provider_is_configured(config, &current_provider) {
            providers.push(current_provider.clone());
        }
        for provider in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAI,
            ProviderKind::ChatGPT,
            ProviderKind::GitHubCopilot,
            ProviderKind::OpenAICompatible,
            ProviderKind::MiniMax,
            ProviderKind::Moonshot,
            ProviderKind::ZAI,
            ProviderKind::DeepSeek,
            ProviderKind::Groq,
            ProviderKind::Gemini,
            ProviderKind::Ollama,
            ProviderKind::OpenRouter,
            ProviderKind::Mistral,
        ] {
            if Self::provider_is_configured(config, &provider) && !providers.contains(&provider) {
                providers.push(provider);
            }
        }
        if providers.is_empty() {
            providers.push(current_provider);
        }
        providers
    }

    fn current_provider_option(&self) -> Option<ProviderSelectItem> {
        let current = self.current_provider.as_ref()?;
        self.session_provider_options
            .iter()
            .find(|item| &item.provider == current)
            .cloned()
            .or_else(|| Some(Self::provider_option(current.clone())))
    }

    #[allow(dead_code)]
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            state: PanelState::new(),
            assistant_message_views: Vec::new(),
            message_list_state: ListState::new(0, ListAlignment::Top, px(2048.0)),
            message_list_handler_installed: false,
            history_scroll_handle: ScrollHandle::new(),
            transcript_footer_scroll_handle: ScrollHandle::new(),
            showing_history: false,
            conversation_list: Vec::new(),
            auto_approve: false,
            current_provider: None,
            session_provider_options: Self::default_provider_options(),
            model_name: String::new(),
            session_model_options: Vec::new(),
            content_reveal: MotionValue::new(1.0),
            editing_msg_idx: None,
            edit_input_state: None,
            show_inline_input: false,
            inline_input_state: None,
            skills: Vec::new(),
            recent_inputs: Vec::new(),
            inline_history_nav_index: None,
            inline_history_nav_draft: None,
            inline_input_has_text: false,
            inline_input_has_skills: false,
            follow_output: FollowOutputState::Auto,
            inline_skill_selection: 0,
            inline_shift_enter: false,
            ui_opacity: 0.90,
        };
        panel.message_list_state.set_follow_mode(FollowMode::Tail);
        panel.ensure_inline_input_state(window, cx);
        panel
    }

    pub fn state(&self) -> &PanelState {
        &self.state
    }

    pub fn set_model_name(&mut self, name: String) {
        self.model_name = name;
    }

    pub fn set_provider_name(
        &mut self,
        provider: ProviderKind,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.current_provider = Some(Self::session_sidebar_provider_kind(&provider));
    }

    pub fn set_session_provider_options(
        &mut self,
        providers: Vec<ProviderKind>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.session_provider_options = providers
            .into_iter()
            .map(Self::provider_option)
            .collect::<Vec<_>>();
    }

    pub fn set_session_model_options(
        &mut self,
        models: Vec<String>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.session_model_options = models;
    }

    pub fn set_auto_approve(&mut self, enabled: bool) {
        self.auto_approve = enabled;
    }

    pub fn set_ui_opacity(&mut self, opacity: f32) {
        self.ui_opacity = opacity.clamp(0.35, 1.0);
    }

    /// Create with a pre-populated panel state (e.g. restored from session).
    pub fn with_state(state: PanelState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            state,
            assistant_message_views: Vec::new(),
            message_list_state: ListState::new(0, ListAlignment::Top, px(2048.0)),
            message_list_handler_installed: false,
            history_scroll_handle: ScrollHandle::new(),
            transcript_footer_scroll_handle: ScrollHandle::new(),
            showing_history: false,
            conversation_list: Vec::new(),
            auto_approve: false,
            current_provider: None,
            session_provider_options: Self::default_provider_options(),
            model_name: String::new(),
            session_model_options: Vec::new(),
            content_reveal: MotionValue::new(1.0),
            editing_msg_idx: None,
            edit_input_state: None,
            show_inline_input: false,
            inline_input_state: None,
            skills: Vec::new(),
            recent_inputs: Vec::new(),
            inline_history_nav_index: None,
            inline_history_nav_draft: None,
            inline_input_has_text: false,
            inline_input_has_skills: false,
            follow_output: FollowOutputState::Auto,
            inline_skill_selection: 0,
            inline_shift_enter: false,
            ui_opacity: 0.90,
        };
        panel.message_list_state.set_follow_mode(FollowMode::Tail);
        panel.ensure_inline_input_state(window, cx);
        panel
    }

    /// Swap the displayed panel state. Returns the old state (to stash back in the tab).
    pub fn swap_state(&mut self, new_state: PanelState, cx: &mut Context<Self>) -> PanelState {
        let old = std::mem::replace(&mut self.state, new_state);
        self.message_list_state = ListState::new(0, ListAlignment::Top, px(2048.0));
        self.message_list_state.set_follow_mode(FollowMode::Tail);
        self.message_list_handler_installed = false;
        self.transcript_footer_scroll_handle = ScrollHandle::new();
        self.assistant_message_views.clear();
        self.follow_output = FollowOutputState::Auto;
        self.showing_history = false;
        cx.notify();
        old
    }

    pub fn clear_messages(&mut self, cx: &mut Context<Self>) {
        self.state.clear();
        self.message_list_state
            .splice(0..self.message_list_state.item_count(), 0);
        self.assistant_message_views.clear();
        self.showing_history = false;
        cx.notify();
    }

    /// Edit-and-rerun: truncate conversation to before `msg_idx`, add the
    /// edited content as a new user message, and emit RerunFromMessage so
    /// the workspace re-sends to the agent.
    pub fn rerun_from(&mut self, msg_idx: usize, new_content: String, cx: &mut Context<Self>) {
        self.state.truncate_to(msg_idx);
        self.state.add_message("user", &new_content);
        self.assistant_message_views.truncate(msg_idx);
        cx.emit(RerunFromMessage {
            content: new_content,
        });
        cx.notify();
    }

    /// Start inline editing of a user message.
    fn start_editing(
        &mut self,
        msg_idx: usize,
        content: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = content.to_string();
        let input_state = cx.new(|cx| {
            let mut s = InputState::new(window, cx).auto_grow(1, 6);
            s.set_value(&content, window, cx);
            s
        });
        // Subscribe to Enter key to submit the edit
        cx.subscribe_in(&input_state, window, {
            move |this, _, ev: &InputEvent, _window, cx| {
                if let InputEvent::PressEnter { secondary: false } = ev {
                    this.submit_edit(cx);
                }
            }
        })
        .detach();
        self.editing_msg_idx = Some(msg_idx);
        self.edit_input_state = Some(input_state);
        cx.notify();
    }

    /// Submit the inline edit: truncate and rerun.
    fn submit_edit(&mut self, cx: &mut Context<Self>) {
        if let (Some(msg_idx), Some(input_state)) =
            (self.editing_msg_idx, self.edit_input_state.as_ref())
        {
            let new_content = input_state.read(cx).value().to_string();
            self.editing_msg_idx = None;
            self.edit_input_state = None;
            if !new_content.trim().is_empty() {
                self.rerun_from(msg_idx, new_content, cx);
            } else {
                cx.notify();
            }
        }
    }

    /// Cancel inline editing.
    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.editing_msg_idx = None;
        self.edit_input_state = None;
        cx.notify();
    }

    pub fn toggle_history(&mut self, cx: &mut Context<Self>) {
        self.showing_history = !self.showing_history;
        if self.showing_history {
            self.conversation_list = con_agent::Conversation::list_all();
        }
        self.content_reveal
            .restart(0.0, 1.0, std::time::Duration::from_millis(180));
        cx.notify();
    }

    pub fn refresh_conversation_list(&mut self, cx: &mut Context<Self>) {
        self.conversation_list = con_agent::Conversation::list_all();
        cx.notify();
    }

    pub fn set_show_inline_input(&mut self, show: bool) {
        self.show_inline_input = show;
    }

    fn ensure_inline_input_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inline_input_state.is_some() {
            return;
        }

        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Ask anything…")
                .auto_grow(1, 4)
        });
        cx.observe_keystrokes(|this, event, _window, _cx| {
            if event.keystroke.key == "enter" {
                this.inline_shift_enter = event.keystroke.modifiers.shift;
            }
        })
        .detach();
        cx.observe_keystrokes(|this, event, window, cx| {
            if !Self::should_fallback_handle_inline_history_key(event)
                || !this.show_inline_input
                || !this.inline_input_is_focused(window, cx)
            {
                return;
            }

            let key = event.keystroke.key.clone();
            let _ = this.handle_inline_navigation_key(key.as_str(), window, cx);
        })
        .detach();
        cx.subscribe_in(
            &state,
            window,
            |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    let mut needs_notify = this.sync_inline_input_visual_state(cx);
                    if let Some(history_ix) = this.inline_history_nav_index {
                        let current_value = this
                            .inline_input_state
                            .as_ref()
                            .map(|input| input.read(cx).value().to_string())
                            .unwrap_or_default();
                        let matches_current = this
                            .recent_inputs
                            .get(history_ix)
                            .is_some_and(|entry| entry == &current_value);
                        if !matches_current {
                            this.inline_history_nav_index = None;
                            this.inline_history_nav_draft = None;
                            needs_notify = true;
                        }
                    }
                    let skills = this.filtered_inline_skills(cx);
                    let previous_skill_selection = this.inline_skill_selection;
                    if skills.is_empty() {
                        this.inline_skill_selection = 0;
                    } else {
                        this.inline_skill_selection = this
                            .inline_skill_selection
                            .min(skills.len().saturating_sub(1));
                    }
                    if this.inline_skill_selection != previous_skill_selection {
                        needs_notify = true;
                    }
                    cx.emit(InlineSkillAutocompleteChanged);
                    if needs_notify {
                        cx.notify();
                    }
                }
                InputEvent::PressEnter { .. } => {
                    if this.inline_shift_enter {
                        this.inline_shift_enter = false;
                        return;
                    }

                    if let Some(ref input) = this.inline_input_state {
                        input.update(cx, |s, cx| {
                            let cursor = s.cursor();
                            let val = s.value().to_string();
                            if cursor > 0 && val.as_bytes().get(cursor - 1) == Some(&b'\n') {
                                let mut cleaned = val[..cursor - 1].to_string();
                                cleaned.push_str(&val[cursor..]);
                                s.set_value(&cleaned, window, cx);
                            }
                        });
                    }

                    let has_completions = !this.filtered_inline_skills(cx).is_empty();
                    if has_completions {
                        let skills = this.filtered_inline_skills(cx);
                        let sel = this
                            .inline_skill_selection
                            .min(skills.len().saturating_sub(1));
                        if let Some(skill) = skills.get(sel) {
                            let name = skill.name.clone();
                            this.complete_inline_skill(&name, window, cx);
                        }
                    } else if let Some(ref input) = this.inline_input_state {
                        let text = input.read(cx).value().to_string();
                        if !text.trim().is_empty() {
                            input.update(cx, |s, cx| s.set_value("", window, cx));
                            cx.emit(InlineInputSubmit { text });
                        }
                    }
                }
                _ => {}
            },
        )
        .detach();
        self.inline_input_state = Some(state);
        self.sync_inline_input_visual_state(cx);
    }

    pub fn set_skills(&mut self, skills: Vec<SkillEntry>, cx: &mut Context<Self>) {
        if self.skills.len() == skills.len()
            && self.skills.iter().zip(skills.iter()).all(|(left, right)| {
                left.name == right.name && left.description == right.description
            })
        {
            return;
        }

        self.skills = skills;
        let visual_changed = self.sync_inline_input_visual_state(cx);
        let previous_selection = self.inline_skill_selection;
        let completions_len = self.filtered_inline_skills(cx).len();
        if completions_len == 0 {
            self.inline_skill_selection = 0;
        } else {
            self.inline_skill_selection = self
                .inline_skill_selection
                .min(completions_len.saturating_sub(1));
        }

        if visual_changed
            || self.inline_input_has_skills
            || self.inline_skill_selection != previous_selection
        {
            cx.emit(InlineSkillAutocompleteChanged);
            cx.notify();
        }
    }

    pub fn set_recent_inputs(&mut self, inputs: Vec<String>) {
        if self.recent_inputs == inputs {
            return;
        }
        self.recent_inputs = inputs;
        self.inline_history_nav_index = None;
        self.inline_history_nav_draft = None;
    }

    fn sync_inline_input_visual_state(&mut self, cx: &App) -> bool {
        let Some(ref input) = self.inline_input_state else {
            let changed = self.inline_input_has_text || self.inline_input_has_skills;
            self.inline_input_has_text = false;
            self.inline_input_has_skills = false;
            return changed;
        };

        let text = input.read(cx).value().to_string();
        let has_text = !text.trim().is_empty();
        let has_skills = {
            let trimmed = text.trim();
            if !trimmed.starts_with('/') {
                false
            } else {
                let query = &trimmed[1..].to_lowercase();
                !query.contains(' ')
                    && self.skills.iter().any(|skill| {
                        query.is_empty() || skill.name.to_lowercase().starts_with(query)
                    })
            }
        };

        let changed =
            self.inline_input_has_text != has_text || self.inline_input_has_skills != has_skills;
        self.inline_input_has_text = has_text;
        self.inline_input_has_skills = has_skills;
        changed
    }

    pub fn focus_inline_input(&self, window: &mut Window, cx: &mut App) -> bool {
        let Some(ref input) = self.inline_input_state else {
            return false;
        };
        input.read(cx).focus_handle(cx).focus(window, cx);
        true
    }

    /// Return matching skills if the inline input text starts with `/`.
    pub fn filtered_inline_skills(&self, cx: &App) -> Vec<&SkillEntry> {
        let Some(ref input) = self.inline_input_state else {
            return Vec::new();
        };
        let text = input.read(cx).value().to_string();
        let trimmed = text.trim();
        if !trimmed.starts_with('/') {
            return Vec::new();
        }
        let query = &trimmed[1..].to_lowercase();
        if query.contains(' ') {
            return Vec::new();
        }
        self.skills
            .iter()
            .filter(|s| query.is_empty() || s.name.to_lowercase().starts_with(query))
            .collect()
    }

    pub fn inline_skill_selection(&self) -> usize {
        self.inline_skill_selection
    }

    pub fn inline_skill_popup_offset(&self, cx: &App) -> Pixels {
        let Some(ref input) = self.inline_input_state else {
            return px(48.0);
        };
        let rows = input.read(cx).value().lines().count().clamp(1, 4);
        px(48.0 + (rows.saturating_sub(1) as f32 * 20.0))
    }

    pub fn complete_inline_skill(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ref input) = self.inline_input_state {
            input.update(cx, |s, cx| {
                s.set_value(&format!("/{name} "), window, cx);
            });
        }
        self.inline_skill_selection = 0;
        cx.emit(InlineSkillAutocompleteChanged);
        cx.notify();
    }

    fn navigate_inline_history(
        &mut self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.recent_inputs.is_empty() {
            return false;
        }

        let Some(ref input) = self.inline_input_state else {
            return false;
        };

        let next_index = match (previous, self.inline_history_nav_index) {
            (true, None) => {
                self.inline_history_nav_draft = Some(input.read(cx).value().to_string());
                Some(0)
            }
            (true, Some(ix)) if ix + 1 < self.recent_inputs.len() => Some(ix + 1),
            (true, Some(ix)) => Some(ix),
            (false, Some(0)) => None,
            (false, Some(ix)) => Some(ix - 1),
            (false, None) => None,
        };

        self.inline_history_nav_index = next_index;
        let replacement = next_index
            .and_then(|ix| self.recent_inputs.get(ix).cloned())
            .or_else(|| self.inline_history_nav_draft.clone())
            .unwrap_or_default();
        let cursor = replacement.chars().count() as u32;

        input.update(cx, |state, cx| {
            state.set_value(&replacement, window, cx);
            state.set_cursor_position(Position::new(0, cursor), window, cx);
        });
        self.inline_skill_selection = 0;
        cx.emit(InlineSkillAutocompleteChanged);
        cx.notify();
        true
    }

    pub fn inline_input_is_focused(&self, window: &Window, cx: &App) -> bool {
        self.inline_input_state
            .as_ref()
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    fn should_fallback_handle_inline_history_key(event: &KeystrokeEvent) -> bool {
        let key = event.keystroke.key.as_str();
        if key != "up" && key != "down" {
            return false;
        }

        let mods = event.keystroke.modifiers;
        if mods.control || mods.platform || mods.alt || mods.shift {
            return false;
        }

        event.action.as_ref().is_none_or(|action| {
            action.partial_eq(&gpui_component::input::MoveUp)
                || action.partial_eq(&gpui_component::input::MoveDown)
        })
    }

    fn inline_history_boundary_reached(&self, previous: bool, cx: &App) -> bool {
        let Some(ref input) = self.inline_input_state else {
            return false;
        };

        let state = input.read(cx);
        Self::inline_history_boundary(&state.value(), state.cursor() as u32, previous)
    }

    fn inline_history_boundary(value: &str, cursor: u32, previous: bool) -> bool {
        let _ = cursor;
        let _ = previous;
        !value.contains('\n')
    }

    fn handle_inline_navigation_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let has_completions = !self.filtered_inline_skills(cx).is_empty();
        match key {
            "up" if has_completions => {
                self.inline_skill_selection = self.inline_skill_selection.saturating_sub(1);
                cx.emit(InlineSkillAutocompleteChanged);
                cx.notify();
                true
            }
            "down" if has_completions => {
                let max = self.filtered_inline_skills(cx).len().saturating_sub(1);
                self.inline_skill_selection = (self.inline_skill_selection + 1).min(max);
                cx.emit(InlineSkillAutocompleteChanged);
                cx.notify();
                true
            }
            "up" if self.inline_history_boundary_reached(true, cx) => {
                self.navigate_inline_history(true, window, cx)
            }
            "down" if self.inline_history_boundary_reached(false, cx) => {
                self.navigate_inline_history(false, window, cx)
            }
            _ => false,
        }
    }

    fn scroll_to_bottom(&self) {
        self.message_list_state.scroll_to_end();
    }

    fn is_scrolled_near_bottom(&self) -> bool {
        self.message_list_state.is_following_tail()
    }

    fn should_follow_output_after_change(&self) -> bool {
        self.follow_output != FollowOutputState::Detached || self.is_scrolled_near_bottom()
    }

    fn follow_output_after_change(&mut self) {
        if self.should_follow_output_after_change() {
            self.follow_output = FollowOutputState::PendingJump;
            self.scroll_to_bottom();
        } else {
            self.follow_output = FollowOutputState::Detached;
        }
    }

    fn install_message_list_handler(&mut self, cx: &mut Context<Self>) {
        if self.message_list_handler_installed {
            return;
        }
        self.message_list_handler_installed = true;
        let panel = cx.weak_entity();
        self.message_list_state
            .set_scroll_handler(move |event, _window, cx| {
                let next = if event.is_following_tail {
                    FollowOutputState::Auto
                } else {
                    FollowOutputState::Detached
                };
                let _ = panel.update(cx, |this, cx| {
                    if this.follow_output != next {
                        this.follow_output = next;
                        cx.notify();
                    }
                });
            });
    }

    fn sync_message_list_count(&mut self) {
        let old_count = self.message_list_state.item_count();
        let new_count = self.state.messages.len();
        if new_count > old_count {
            self.message_list_state
                .splice(old_count..old_count, new_count - old_count);
        } else if new_count < old_count {
            self.message_list_state.splice(new_count..old_count, 0);
        }
    }

    fn remeasure_message(&self, msg_idx: usize) {
        if msg_idx < self.message_list_state.item_count() {
            self.message_list_state
                .remeasure_items(msg_idx..msg_idx + 1);
        }
    }

    pub fn add_message(&mut self, role: &str, content: &str, cx: &mut Context<Self>) {
        let old_count = self.state.messages.len();
        self.state.add_message(role, content);
        self.sync_message_list_count();
        if self.state.messages.len() == old_count {
            self.remeasure_message(old_count.saturating_sub(1));
        }
        if role == "user" {
            self.follow_output = FollowOutputState::PendingJump;
        }
        self.follow_output_after_change();
        cx.notify();
    }

    pub fn add_step(&mut self, step: &str, cx: &mut Context<Self>) {
        self.state.add_step(step);
        if let Some(last_idx) = self.state.messages.len().checked_sub(1) {
            self.remeasure_message(last_idx);
        }
        self.follow_output_after_change();
        cx.notify();
    }

    pub fn add_tool_call(
        &mut self,
        call_id: &str,
        tool_name: &str,
        args: &str,
        cx: &mut Context<Self>,
    ) {
        self.state.add_tool_call(call_id, tool_name, args);
        self.follow_output_after_change();
        cx.notify();
    }

    pub fn complete_tool_call(
        &mut self,
        call_id: &str,
        _tool_name: &str,
        result: &str,
        cx: &mut Context<Self>,
    ) {
        self.state.complete_tool_call(call_id, result);
        self.follow_output_after_change();
        cx.notify();
    }

    pub fn add_pending_approval(
        &mut self,
        call_id: &str,
        tool_name: &str,
        args: &str,
        approval_tx: Sender<ToolApprovalDecision>,
        cx: &mut Context<Self>,
    ) {
        self.state
            .add_pending_approval(call_id, tool_name, args, approval_tx);
        self.follow_output_after_change();
        cx.notify();
    }

    fn resolve_approval(&mut self, index: usize, allowed: bool, cx: &mut Context<Self>) {
        if index >= self.state.pending_approvals.len() {
            return;
        }
        let approval = self.state.pending_approvals.remove(index);
        let _ = approval.approval_tx.send(ToolApprovalDecision {
            call_id: approval.call_id,
            allowed,
            reason: if allowed {
                None
            } else {
                Some("User denied tool execution".to_string())
            },
        });
        if let Some(last_idx) = self.state.messages.len().checked_sub(1) {
            let last = &mut self.state.messages[last_idx];
            let human_name = humanize_tool_name(&approval.tool_name);
            let (status, label) = if allowed {
                (StepStatus::Complete, format!("Allowed {}", human_name))
            } else {
                (StepStatus::Denied, format!("Denied {}", human_name))
            };
            last.steps.push(StepEntry {
                icon: if allowed {
                    "phosphor/check-circle-fill.svg"
                } else {
                    "phosphor/x.svg"
                },
                label,
                detail: None,
                status,
                detail_collapsed: true,
                detail_expanded: false,
                duration: None,
            });
            last.touch();
            self.remeasure_message(last_idx);
        }
        cx.notify();
    }

    fn resolve_all_approvals(&mut self, cx: &mut Context<Self>) {
        while !self.state.pending_approvals.is_empty() {
            self.resolve_approval(0, true, cx);
        }
    }

    pub fn update_thinking(&mut self, text: &str, cx: &mut Context<Self>) {
        let old_count = self.state.messages.len();
        self.state.update_thinking(text);
        self.sync_message_list_count();
        let msg_idx = self.state.messages.len().saturating_sub(1);
        if self.state.messages.len() > old_count || !self.state.messages.is_empty() {
            self.remeasure_message(msg_idx);
        }
        self.follow_output_after_change();
        cx.notify();
    }

    pub fn update_streaming(&mut self, token: &str, cx: &mut Context<Self>) {
        let old_count = self.state.messages.len();
        self.state.update_streaming(token);
        self.sync_message_list_count();
        let msg_idx = self.state.messages.len().saturating_sub(1);
        if self.state.messages.len() > old_count || !self.state.messages.is_empty() {
            self.remeasure_message(msg_idx);
        }
        self.ensure_content_markdown_async(msg_idx, cx);
        self.follow_output_after_change();
        cx.notify();
    }

    pub fn complete_response(&mut self, msg: &con_agent::Message, cx: &mut Context<Self>) {
        let old_count = self.state.messages.len();
        self.state
            .complete_response(&msg.content, msg.model.as_deref(), msg.duration_ms);
        self.sync_message_list_count();
        if let Some(last_idx) = self.state.messages.len().checked_sub(1) {
            self.ensure_content_markdown_async(last_idx, cx);
            if self.state.messages.len() > old_count || !self.state.messages.is_empty() {
                self.remeasure_message(last_idx);
            }
        }
        self.follow_output_after_change();
        cx.notify();
    }

    fn ensure_content_markdown_async(&mut self, msg_idx: usize, cx: &mut Context<Self>) {
        let Some(message) = self.state.messages.get_mut(msg_idx) else {
            return;
        };
        if message.content.is_empty()
            || message.content_markdown_pending
            || message.content_markdown_len == message.content.len()
        {
            return;
        }

        let content = message.content.clone();
        let message_id = message.id;
        let parse_content = content.clone();
        let parse_len = content.len();
        message.content_markdown_pending = true;
        message.touch();

        cx.spawn(async move |this, cx| {
            let parsed = Arc::new(
                cx.background_executor()
                    .spawn(async move { ParsedChatMarkdown::parse(&parse_content) })
                    .await,
            );
            let _ = this.update(cx, |panel, cx| {
                let mut needs_follow_up = false;
                if let Some(message) = panel.state.messages.get_mut(msg_idx) {
                    if message.id != message_id {
                        return;
                    }
                    if message.content == content {
                        message.content_markdown = Some(parsed.clone());
                        message.content_markdown_len = parse_len;
                    } else if message.content.starts_with(&content) {
                        message.content_markdown = Some(parsed.clone());
                        message.content_markdown_len = parse_len;
                        needs_follow_up = message.content_markdown_len < message.content.len();
                    }
                    message.content_markdown_pending = false;
                    message.touch();
                }
                panel.remeasure_message(msg_idx);
                if needs_follow_up {
                    panel.ensure_content_markdown_async(msg_idx, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_assistant_message_view(
        &mut self,
        msg_idx: usize,
        is_streaming_assistant: bool,
        cx: &mut Context<Self>,
    ) -> Option<Entity<AssistantMessageView>> {
        let Some(message) = self.state.messages.get(msg_idx) else {
            return None;
        };
        if message.role != "assistant" {
            return None;
        }

        if self.assistant_message_views.len() <= msg_idx {
            self.assistant_message_views
                .resize_with(msg_idx + 1, || None);
        }

        let state_key = AssistantMessageStateKey {
            revision: message.revision,
            content_len: message.content.len(),
            content_head: message.content.chars().take(96).collect(),
            content_markdown_len: message.content_markdown_len,
            content_markdown_blocks: message
                .content_markdown
                .as_ref()
                .map(|markdown| markdown.block_count())
                .unwrap_or(0),
        };
        if let Some(view) = self.assistant_message_views[msg_idx].as_ref() {
            let view = view.clone();
            let unchanged = {
                let existing = view.read(cx);
                existing.msg_idx == msg_idx
                    && existing.is_streaming_assistant == is_streaming_assistant
                    && existing.state_key == state_key
            };
            if unchanged {
                return Some(view);
            }

            let snapshot = message.clone();
            view.update(cx, |view, cx| {
                view.update_state(msg_idx, snapshot, state_key, is_streaming_assistant, cx);
            });
            Some(view)
        } else {
            let snapshot = message.clone();
            let panel = cx.weak_entity();
            let view = cx.new(|_| {
                AssistantMessageView::new(
                    panel,
                    msg_idx,
                    snapshot,
                    state_key,
                    is_streaming_assistant,
                )
            });
            self.assistant_message_views[msg_idx] = Some(view.clone());
            Some(view)
        }
    }

    /// Derive a human-readable status line from current panel state.
    fn status_text(&self) -> Option<(&'static str, &'static str)> {
        // Priority: approval > running tool > thinking > responding
        if !self.state.pending_approvals.is_empty() {
            return Some(("phosphor/shield-warning-fill.svg", "Awaiting approval…"));
        }
        if let Some(tc) = self.state.tool_calls.iter().find(|tc| tc.result.is_none()) {
            let label = match tc.tool_name.as_str() {
                "terminal_exec" | "batch_exec" => "Running command…",
                "shell_exec" => "Running in background…",
                "file_read" => "Reading file…",
                "file_write" | "edit_file" => "Writing file…",
                "search" | "search_panes" => "Searching…",
                "list_panes" | "read_pane" => "Reading pane…",
                "list_files" => "Listing files…",
                "send_keys" => "Sending keys…",
                "wait_for" => "Waiting…",
                "create_pane" => "Creating pane…",
                _ => "Running tool…",
            };
            return Some((tool_icon(&tc.tool_name), label));
        }
        match self.state.status {
            AgentStatus::Thinking => Some(("phosphor/brain-duotone.svg", "Thinking…")),
            AgentStatus::Responding => Some(("phosphor/chat-circle-fill.svg", "Writing…")),
            AgentStatus::Idle => None,
        }
    }

    fn running_tool_call(&self) -> Option<&ToolCallEntry> {
        self.state.tool_calls.iter().find(|tc| tc.result.is_none())
    }
}

fn tool_icon(tool_name: &str) -> &'static str {
    match tool_name {
        "terminal_exec" | "batch_exec" => "phosphor/play-circle-fill.svg",
        "shell_exec" => "phosphor/terminal-duotone.svg",
        "probe_shell_context" => "phosphor/magnifying-glass.svg",
        "file_write" => "phosphor/pencil-simple.svg",
        "file_read" => "phosphor/file-code.svg",
        "edit_file" => "phosphor/pencil-simple.svg",
        "list_files" => "phosphor/folder.svg",
        "search" | "search_panes" => "phosphor/magnifying-glass.svg",
        "list_panes" | "list_tab_workspaces" | "tmux_list_targets" | "tmux_find_targets" => {
            "phosphor/rows-fill.svg"
        }
        "read_pane" => "phosphor/eye-fill.svg",
        "tmux_capture_pane" => "phosphor/eye-fill.svg",
        "send_keys" | "tmux_send_keys" => "phosphor/keyboard-fill.svg",
        "wait_for" => "phosphor/hourglass-fill.svg",
        "create_pane"
        | "ensure_remote_shell_target"
        | "tmux_ensure_shell_target"
        | "tmux_ensure_agent_target" => "phosphor/stack-fill.svg",
        "resolve_work_target" => "phosphor/compass-fill.svg",
        "remote_exec" | "tmux_run_command" => "phosphor/target-fill.svg",
        _ => "phosphor/gear.svg",
    }
}

fn format_pane_target(v: &serde_json::Value) -> Option<String> {
    let pane_index = v.get("pane_index").and_then(|i| i.as_u64());
    let pane_id = v.get("pane_id").and_then(|i| i.as_u64());

    match (pane_index, pane_id) {
        (Some(index), Some(id)) => Some(format!("pane {} · id {}", index, id)),
        (Some(index), None) => Some(format!("pane {}", index)),
        (None, Some(id)) => Some(format!("id {}", id)),
        (None, None) => None,
    }
}

fn result_is_expandable(content: &str) -> bool {
    content.lines().count() > TOOL_RESULT_PREVIEW_LINES
}

fn format_tool_args(tool_name: &str, args: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(args) {
        match tool_name {
            "terminal_exec" | "shell_exec" => {
                if let Some(cmd) = v.get("command").and_then(|c| c.as_str()) {
                    if let Some(target) = format_pane_target(&v) {
                        return format!("{target} · {cmd}");
                    }
                    return cmd.to_string();
                }
            }
            "file_read" | "file_write" | "edit_file" => {
                if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                    return path.to_string();
                }
            }
            "list_files" => {
                return v
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or(".")
                    .to_string();
            }
            "search" => {
                if let Some(pattern) = v.get("pattern").and_then(|q| q.as_str()) {
                    return format!("\"{}\"", pattern);
                }
            }
            "search_panes" => {
                if let Some(pattern) = v.get("pattern").and_then(|q| q.as_str()) {
                    return format!("\"{}\"", pattern);
                }
            }
            "batch_exec" => {
                if let Some(cmds) = v.get("commands").and_then(|c| c.as_array()) {
                    return format!("{} commands", cmds.len());
                }
            }
            "send_keys" => {
                if let Some(keys) = v.get("keys").and_then(|k| k.as_str()) {
                    if let Some(target) = format_pane_target(&v) {
                        return format!("{target} · {keys}");
                    }
                    return keys.to_string();
                }
            }
            "wait_for" => {
                let pattern = v.get("pattern").and_then(|p| p.as_str()).unwrap_or("");
                let timeout = v.get("timeout_secs").and_then(|t| t.as_u64());
                let mut parts = Vec::new();
                if let Some(target) = format_pane_target(&v) {
                    parts.push(target);
                }
                if !pattern.is_empty() {
                    parts.push(format!("\"{}\"", pattern));
                }
                if let Some(t) = timeout {
                    parts.push(format!("{}s", t));
                }
                return if parts.is_empty() {
                    "waiting…".to_string()
                } else {
                    parts.join(" · ")
                };
            }
            "create_pane" => {
                let cmd = v.get("command").and_then(|c| c.as_str());
                let dir = v.get("directory").and_then(|d| d.as_str());
                let location = v.get("location").and_then(|loc| loc.as_str());
                let body = match (cmd, dir) {
                    (Some(c), _) => c.to_string(),
                    (None, Some(d)) => d.to_string(),
                    _ => "new pane".to_string(),
                };
                return match location {
                    Some(location) => format!("{body} · {location}"),
                    None => body,
                };
            }
            "read_pane" | "list_panes" => {
                if let Some(target) = format_pane_target(&v) {
                    return target;
                }
                return "all panes".to_string();
            }
            "list_tab_workspaces" => {
                return "tab workspaces".to_string();
            }
            "resolve_work_target" => {
                let intent = v.get("intent").and_then(|i| i.as_str()).unwrap_or("work");
                let host = v.get("host_contains").and_then(|h| h.as_str());
                let agent = v.get("agent_name").and_then(|a| a.as_str());
                let mut parts = vec![intent.replace('_', " ")];
                if let Some(host) = host {
                    parts.push(format!("host={host}"));
                }
                if let Some(agent) = agent {
                    parts.push(format!("agent={agent}"));
                }
                if let Some(target) = format_pane_target(&v) {
                    parts.push(target);
                }
                return parts.join(" · ");
            }
            "ensure_remote_shell_target" => {
                if let Some(host) = v.get("host").and_then(|h| h.as_str()) {
                    return host.to_string();
                }
            }
            "remote_exec" => {
                let hosts = v
                    .get("hosts")
                    .and_then(|hosts| hosts.as_array())
                    .map(|hosts| {
                        hosts
                            .iter()
                            .filter_map(|host| host.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|hosts| !hosts.is_empty());
                let command = v.get("command").and_then(|cmd| cmd.as_str());
                return match (hosts, command) {
                    (Some(hosts), Some(command)) => format!("{hosts} · {command}"),
                    (Some(hosts), None) => hosts,
                    (None, Some(command)) => command.to_string(),
                    (None, None) => "remote exec".to_string(),
                };
            }
            "tmux_list_targets" | "tmux_find_targets" | "tmux_inspect" => {
                if let Some(target) = format_pane_target(&v) {
                    return target;
                }
                return "tmux".to_string();
            }
            "tmux_capture_pane" => {
                let outer = format_pane_target(&v);
                let target = v.get("target").and_then(|t| t.as_str());
                return match (outer, target) {
                    (Some(outer), Some(target)) => format!("{outer} · {target}"),
                    (Some(outer), None) => outer,
                    (None, Some(target)) => target.to_string(),
                    (None, None) => "tmux capture".to_string(),
                };
            }
            "tmux_send_keys" => {
                let outer = format_pane_target(&v);
                let target = v.get("target").and_then(|t| t.as_str());
                let text = v
                    .get("text")
                    .and_then(|t| t.as_str())
                    .or_else(|| v.get("keys").and_then(|t| t.as_str()));
                return match (outer, target, text) {
                    (Some(outer), Some(target), Some(text)) => {
                        format!("{outer} · {target} · {text}")
                    }
                    (Some(outer), Some(target), None) => format!("{outer} · {target}"),
                    (Some(outer), None, Some(text)) => format!("{outer} · {text}"),
                    (Some(outer), None, None) => outer,
                    (None, Some(target), Some(text)) => format!("{target} · {text}"),
                    (None, Some(target), None) => target.to_string(),
                    (None, None, Some(text)) => text.to_string(),
                    (None, None, None) => "tmux keys".to_string(),
                };
            }
            "tmux_run_command" | "tmux_ensure_shell_target" | "tmux_ensure_agent_target" => {
                let outer = format_pane_target(&v);
                let command = v
                    .get("command")
                    .and_then(|c| c.as_str())
                    .or_else(|| v.get("agent").and_then(|a| a.as_str()));
                return match (outer, command) {
                    (Some(outer), Some(command)) => format!("{outer} · {command}"),
                    (Some(outer), None) => outer,
                    (None, Some(command)) => command.to_string(),
                    (None, None) => "tmux command".to_string(),
                };
            }
            _ => {}
        }
    }
    truncate_str(args, 120)
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..s.floor_char_boundary(max_len)])
    }
}

/// Human-friendly tool name labels.
fn humanize_tool_name(name: &str) -> String {
    match name {
        "terminal_exec" | "shell_exec" => "Run".to_string(),
        "batch_exec" => "Batch Run".to_string(),
        "probe_shell_context" => "Probe Shell".to_string(),
        "file_read" => "Read".to_string(),
        "file_write" => "Write".to_string(),
        "edit_file" => "Edit".to_string(),
        "list_files" => "List Files".to_string(),
        "search" => "Search".to_string(),
        "search_panes" => "Search Panes".to_string(),
        "list_panes" => "List Panes".to_string(),
        "list_tab_workspaces" => "List Workspaces".to_string(),
        "read_pane" => "Read Pane".to_string(),
        "send_keys" => "Send Keys".to_string(),
        "create_pane" => "New Pane".to_string(),
        "wait_for" => "Wait For".to_string(),
        "resolve_work_target" => "Resolve Target".to_string(),
        "ensure_remote_shell_target" => "Prepare Remote Shell".to_string(),
        "remote_exec" => "Run on Hosts".to_string(),
        "tmux_inspect" => "Inspect tmux".to_string(),
        "tmux_list_targets" => "List tmux".to_string(),
        "tmux_find_targets" => "Find tmux".to_string(),
        "tmux_capture_pane" => "Capture tmux Pane".to_string(),
        "tmux_ensure_shell_target" => "Prepare tmux Shell".to_string(),
        "tmux_ensure_agent_target" => "Prepare Agent Target".to_string(),
        "tmux_run_command" => "Run in tmux".to_string(),
        "tmux_send_keys" => "Send tmux Keys".to_string(),
        _ => {
            // Fallback: title-case with _ → space
            name.split('_')
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        Some(c) => {
                            let mut s = c.to_uppercase().to_string();
                            s.extend(chars);
                            s
                        }
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// Unwrap Rig's double-encoding: Rig calls `serde_json::to_string()` on tool Output,
/// so `Output = String` gets wrapped in JSON string escaping. This unwraps it.
fn unwrap_tool_result(raw: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::String(inner)) => {
            // Double-encoded: Rig wrapped our string in JSON quotes.
            // Try to parse the inner string as JSON (e.g., batch_exec's JSON array).
            match serde_json::from_str::<serde_json::Value>(&inner) {
                Ok(v) => v,
                Err(_) => serde_json::Value::String(inner),
            }
        }
        Ok(v) => v,
        Err(_) => serde_json::Value::String(raw.to_string()),
    }
}

/// Format a tool result for display. Content-aware: understands the shape of
/// each tool's output and renders it as human-readable text, not raw JSON.
fn format_tool_result(tool_name: &str, raw: &str) -> String {
    let value = unwrap_tool_result(raw);

    match tool_name {
        "batch_exec" => format_batch_result(&value),
        "terminal_exec" | "shell_exec" => format_exec_result(&value),
        "list_panes" => format_list_panes_result(&value),
        "search" | "search_panes" => format_search_result(&value),
        "wait_for" => format_wait_for_result(&value),
        _ => format_generic_result(&value),
    }
}

/// batch_exec: show each pane's output as a labeled block.
fn format_batch_result(value: &serde_json::Value) -> String {
    let arr = match value.as_array() {
        Some(a) => a,
        None => return format_generic_result(value),
    };
    let mut out = String::new();
    for item in arr {
        let idx = item.get("pane_index").and_then(|v| v.as_u64()).unwrap_or(0);
        let output = item.get("output").and_then(|v| v.as_str()).unwrap_or("");
        let exit_code = item.get("exit_code").and_then(|v| v.as_i64());
        let error = item
            .get("error")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("── Pane {} ", idx));
        if let Some(code) = exit_code {
            out.push_str(&format!("(exit {}) ", code));
        }
        out.push_str("──\n");
        if let Some(err) = error {
            out.push_str(&format!("Error: {}\n", err));
        }
        let cleaned = output.trim();
        if !cleaned.is_empty() {
            out.push_str(cleaned);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// terminal_exec / shell_exec: show stdout directly.
fn format_exec_result(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(obj) => {
            let stdout = obj.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
            let stderr = obj.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
            let exit_code = obj.get("exit_code").and_then(|v| v.as_i64());

            let mut out = String::new();
            let trimmed = stdout.trim();
            if !trimmed.is_empty() {
                out.push_str(trimmed);
            }
            if !stderr.trim().is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("stderr: {}", stderr.trim()));
            }
            if let Some(code) = exit_code {
                if code != 0 {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&format!("exit code: {}", code));
                }
            }
            if out.is_empty() {
                "(no output)".to_string()
            } else {
                out
            }
        }
        serde_json::Value::String(s) => s.clone(),
        _ => format_generic_result(value),
    }
}

/// list_panes: show a compact pane summary.
fn format_list_panes_result(value: &serde_json::Value) -> String {
    let arr = match value.as_array() {
        Some(a) => a,
        None => return format_generic_result(value),
    };
    let mut out = String::new();
    for item in arr {
        let idx = item.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        let pane_id = item.get("pane_id").and_then(|v| v.as_u64());
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("?");
        let focused = item
            .get("is_focused")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let busy = item
            .get("is_busy")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let alive = item
            .get("is_alive")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let hostname = item.get("hostname").and_then(|v| v.as_str());
        let cwd = item.get("cwd").and_then(|v| v.as_str());

        let mut flags = Vec::new();
        if focused {
            flags.push("focused");
        }
        if busy {
            flags.push("busy");
        }
        if !alive {
            flags.push("dead");
        }
        if let Some(h) = hostname {
            flags.push(h);
        }

        let location = cwd.unwrap_or("");
        let flag_str = if flags.is_empty() {
            String::new()
        } else {
            format!(" ({})", flags.join(", "))
        };
        let id_str = pane_id
            .map(|pane_id| format!(" [id {}]", pane_id))
            .unwrap_or_default();
        out.push_str(&format!(
            "{}{}. {}{} {}\n",
            idx, id_str, title, flag_str, location
        ));
    }
    out.trim_end().to_string()
}

/// search / search_panes: show matching lines.
fn format_search_result(value: &serde_json::Value) -> String {
    // search_panes returns SearchResults: Vec<(pane_idx, line_num, text)>
    // search returns SearchOutput with matches
    match value {
        serde_json::Value::Array(arr) => {
            let mut out = String::new();
            for item in arr {
                if let Some(arr_item) = item.as_array() {
                    // Tuple format: [pane_idx, line_num, text]
                    let pane = arr_item.first().and_then(|v| v.as_u64()).unwrap_or(0);
                    let line = arr_item.get(1).and_then(|v| v.as_u64()).unwrap_or(0);
                    let text = arr_item.get(2).and_then(|v| v.as_str()).unwrap_or("");
                    out.push_str(&format!("pane {}:{}: {}\n", pane, line, text));
                }
            }
            if out.is_empty() {
                "(no matches)".to_string()
            } else {
                out.trim_end().to_string()
            }
        }
        _ => format_generic_result(value),
    }
}

/// wait_for: show status + relevant output snippet.
fn format_wait_for_result(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(obj) => {
            let status = obj
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let output = obj.get("output").and_then(|v| v.as_str()).unwrap_or("");
            let mut out = format!("status: {}", status);
            let trimmed = output.trim();
            if !trimmed.is_empty() {
                out.push_str(&format!("\n{}", trimmed));
            }
            out
        }
        serde_json::Value::String(s) => s.clone(),
        _ => format_generic_result(value),
    }
}

/// Fallback: render JSON values as human-readable text, not raw JSON dumps.
fn format_generic_result(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "(empty)".to_string(),
        serde_json::Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Object(obj) => {
            // Render as clean key: value lines
            let mut out = String::new();
            for (key, val) in obj {
                if !out.is_empty() {
                    out.push('\n');
                }
                let val_str = match val {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => "—".to_string(),
                    serde_json::Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Array(arr) => {
                        if arr.len() <= 3 {
                            arr.iter()
                                .map(|v| match v {
                                    serde_json::Value::String(s) => s.clone(),
                                    _ => v.to_string(),
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        } else {
                            format!("{} items", arr.len())
                        }
                    }
                    serde_json::Value::Object(_) => {
                        format!("({} fields)", val.as_object().map_or(0, |o| o.len()))
                    }
                };
                out.push_str(&format!("{}: {}", key, val_str));
            }
            out
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                "(empty list)".to_string()
            } else {
                let mut out = String::new();
                for (i, item) in arr.iter().enumerate() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    match item {
                        serde_json::Value::String(s) => out.push_str(s),
                        serde_json::Value::Object(obj) => {
                            // Render object items as inline key=value
                            let parts: Vec<String> = obj
                                .iter()
                                .map(|(k, v)| {
                                    let v_str = match v {
                                        serde_json::Value::String(s) => s.clone(),
                                        _ => v.to_string(),
                                    };
                                    format!("{}={}", k, v_str)
                                })
                                .collect();
                            out.push_str(&format!("{}. {}", i, parts.join("  ")));
                        }
                        _ => out.push_str(&format!("{}. {}", i, item)),
                    }
                }
                out
            }
        }
    }
}

/// Render a result block — subtle bg, monospace for multi-line, inline for short text.
fn render_result_block(
    content: &str,
    _id_prefix: &str,
    theme: &gpui_component::Theme,
    connected: bool,
) -> AnyElement {
    let is_short = content.lines().count() <= 1 && content.len() < 80;
    let nested_surface = if connected {
        trace_inner_surface(theme)
    } else {
        trace_step_surface(theme)
    };
    let result_shell_surface = if connected {
        theme.background.opacity(0.88)
    } else {
        trace_inner_surface(theme)
    };
    let structured_rows = parse_key_value_rows(content);

    if is_short && content != "(no output)" {
        if connected {
            div()
                .pl(px(18.0))
                .pr(px(12.0))
                .py(px(8.0))
                .bg(nested_surface)
                .child(
                    div()
                        .px(px(9.0))
                        .py(px(6.0))
                        .rounded(px(9.0))
                        .bg(result_shell_surface)
                        .text_size(px(10.5))
                        .font_family(theme.mono_font_family.clone())
                        .line_height(px(15.0))
                        .text_color(theme.foreground.opacity(0.72))
                        .overflow_x_hidden()
                        .whitespace_normal()
                        .child(content.to_string()),
                )
                .into_any_element()
        } else {
            // Short result — inline, no code block
            div()
                .ml(px(20.0))
                .px(px(9.0))
                .py(px(6.0))
                .rounded(px(9.0))
                .bg(result_shell_surface)
                .text_size(px(10.5))
                .font_family(theme.mono_font_family.clone())
                .line_height(px(15.0))
                .text_color(theme.foreground.opacity(0.68))
                .overflow_x_hidden()
                .whitespace_normal()
                .child(content.to_string())
                .into_any_element()
        }
    } else {
        let body = if let Some(rows) = structured_rows {
            render_key_value_text(rows.as_slice(), theme).into_any_element()
        } else {
            render_result_text(content, theme.foreground.opacity(0.74), theme).into_any_element()
        };
        if connected {
            div()
                .pl(px(18.0))
                .pr(px(12.0))
                .py(px(9.0))
                .bg(nested_surface)
                .child(
                    div()
                        .px(px(10.0))
                        .py(px(9.0))
                        .rounded(px(10.0))
                        .bg(result_shell_surface)
                        .overflow_x_hidden()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(10.75))
                        .line_height(px(16.0))
                        .text_color(theme.muted_foreground.opacity(0.72))
                        .child(body),
                )
                .into_any_element()
        } else {
            div()
                .px(px(10.0))
                .py(px(9.0))
                .rounded(px(10.0))
                .bg(result_shell_surface)
                .overflow_x_hidden()
                .font_family(theme.mono_font_family.clone())
                .text_size(px(10.75))
                .line_height(px(16.0))
                .text_color(theme.muted_foreground.opacity(0.69))
                .child(body)
                .into_any_element()
        }
    }
}

/// Create a preview of a tool result — first N lines + count of remaining.
fn result_preview(formatted: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = formatted.lines().collect();
    if lines.len() <= max_lines {
        formatted.to_string()
    } else {
        let preview: String = lines[..max_lines].join("\n");
        format!("{}\n… {} more lines", preview, lines.len() - max_lines)
    }
}

fn hidden_result_line_count(content: &str, max_lines: usize) -> usize {
    content.lines().count().saturating_sub(max_lines)
}

fn parse_key_value_rows(content: &str) -> Option<Vec<(String, String)>> {
    let mut rows = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("…") {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(": ") else {
            return None;
        };
        if key.len() > 32 || value.is_empty() {
            return None;
        }
        rows.push((key.to_string(), value.to_string()));
    }

    (rows.len() >= 2).then_some(rows)
}

fn render_key_value_text(rows: &[(String, String)], theme: &gpui_component::Theme) -> Div {
    let key_width = rows
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0)
        .min(24);
    let mut text = String::new();
    for (idx, (key, value)) in rows.iter().enumerate() {
        if idx > 0 {
            text.push('\n');
        }
        let pad = key_width.saturating_sub(key.chars().count());
        text.push_str(key);
        text.push_str(&" ".repeat(pad));
        text.push_str("  ");
        text.push_str(value);
    }
    render_result_text(&text, theme.foreground.opacity(0.80), theme)
}

fn render_result_text(content: &str, color: Hsla, theme: &gpui_component::Theme) -> Div {
    let display = if content.is_empty() {
        "\u{200B}".to_string()
    } else {
        content.to_string()
    };
    let text_style = TextStyle {
        color,
        font_family: theme.mono_font_family.clone(),
        font_size: px(10.75).into(),
        line_height: px(16.0).into(),
        white_space: WhiteSpace::Normal,
        ..Default::default()
    };

    div().child(StyledText::new(display.clone()).with_runs(vec![text_style.to_run(display.len())]))
}

fn trace_group_surface(theme: &gpui_component::Theme) -> Hsla {
    theme.secondary.opacity(0.42)
}

fn trace_step_surface(theme: &gpui_component::Theme) -> Hsla {
    theme.background.opacity(0.94)
}

fn trace_step_header_surface(theme: &gpui_component::Theme) -> Hsla {
    theme.muted.opacity(0.09)
}

fn trace_step_header_hover_surface(theme: &gpui_component::Theme) -> Hsla {
    theme.muted.opacity(0.13)
}

fn trace_detail_surface(theme: &gpui_component::Theme) -> Hsla {
    theme.secondary.opacity(0.18)
}

fn trace_inner_surface(theme: &gpui_component::Theme) -> Hsla {
    theme.background.opacity(0.90)
}

fn result_toggle_label(content: &str, expanded: bool) -> String {
    if expanded {
        "Show less".to_string()
    } else {
        let hidden = hidden_result_line_count(content, TOOL_RESULT_PREVIEW_LINES);
        if hidden > 0 {
            format!(
                "Show {} more line{}",
                hidden,
                if hidden == 1 { "" } else { "s" }
            )
        } else {
            "Show full output".to_string()
        }
    }
}

fn restored_steps_from_agent_steps(agent_steps: &[AgentStep]) -> Vec<StepEntry> {
    use std::collections::HashMap;

    let mut restored = Vec::new();
    let mut tool_call_positions: HashMap<String, usize> = HashMap::new();

    for step in agent_steps {
        match step {
            AgentStep::Thinking(text) => {
                restored.push(StepEntry {
                    icon: "phosphor/brain-duotone.svg",
                    label: text.clone(),
                    detail: None,
                    status: StepStatus::Complete,
                    detail_collapsed: true,
                    detail_expanded: false,
                    duration: None,
                });
            }
            AgentStep::ToolCall {
                call_id,
                tool,
                input,
            } => {
                let args = if input.is_string() {
                    input
                        .as_str()
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| input.to_string())
                } else {
                    input.to_string()
                };
                let label = format!(
                    "{}: {}",
                    humanize_tool_name(tool),
                    truncate_str(&format_tool_args(tool, &args), 60)
                );
                let idx = restored.len();
                restored.push(StepEntry {
                    icon: tool_icon(tool),
                    label,
                    detail: None,
                    status: StepStatus::Running,
                    detail_collapsed: true,
                    detail_expanded: false,
                    duration: None,
                });
                if let Some(call_id) = call_id.as_deref() {
                    tool_call_positions.insert(call_id.to_string(), idx);
                }
            }
            AgentStep::ToolResult {
                call_id,
                tool,
                output,
                success,
            } => {
                let detail = Some(format_tool_result(tool, output));
                let status = if *success {
                    StepStatus::Complete
                } else {
                    StepStatus::Denied
                };

                let mut updated = false;
                if let Some(call_id) = call_id.as_deref() {
                    if let Some(idx) = tool_call_positions.remove(call_id) {
                        if let Some(existing) = restored.get_mut(idx) {
                            existing.detail = detail.clone();
                            existing.status = status;
                            updated = true;
                        }
                    }
                }

                if !updated {
                    restored.push(StepEntry {
                        icon: tool_icon(tool),
                        label: humanize_tool_name(tool),
                        detail,
                        status,
                        detail_collapsed: true,
                        detail_expanded: false,
                        duration: None,
                    });
                }
            }
            AgentStep::Text(_) => {}
        }
    }

    for idx in tool_call_positions.into_values() {
        if let Some(step) = restored.get_mut(idx) {
            step.status = StepStatus::Complete;
        }
    }

    restored
}

pub(crate) fn describe_agent_step(step: &AgentStep) -> String {
    match step {
        AgentStep::Thinking(text) => text.clone(),
        AgentStep::Text(text) => text.clone(),
        AgentStep::ToolCall { tool, .. } => format!("Calling {}", humanize_tool_name(tool)),
        AgentStep::ToolResult { tool, success, .. } => {
            if *success {
                format!("Finished {}", humanize_tool_name(tool))
            } else {
                format!("{} needs review", humanize_tool_name(tool))
            }
        }
    }
}

fn render_inline_state(
    label: SharedString,
    color: Hsla,
    theme: &gpui_component::Theme,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(ui_space_px(theme, 5.0))
        .px(ui_space_px(theme, 7.0))
        .py(ui_space_px(theme, 3.0))
        .rounded_full()
        .bg(color.opacity(0.10))
        .child(
            div()
                .size(ui_space_px(theme, 5.0))
                .rounded_full()
                .bg(color.opacity(0.85)),
        )
        .child(
            div()
                .text_size(ui_px(theme, 10.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground.opacity(0.62))
                .child(label),
        )
        .into_any_element()
}

fn render_meta_chip(
    label: impl Into<SharedString>,
    surface: Hsla,
    text: Hsla,
    theme: &gpui_component::Theme,
    mono: bool,
) -> AnyElement {
    let chip_px = |value| {
        if mono {
            mono_space_px(theme, value)
        } else {
            ui_space_px(theme, value)
        }
    };
    let mut label_el = div()
        .text_size(if mono {
            mono_px(theme, 10.25)
        } else {
            ui_px(theme, 10.25)
        })
        .font_weight(FontWeight::MEDIUM)
        .text_color(text)
        .child(label.into());

    if mono {
        label_el = label_el.font_family(theme.mono_font_family.clone());
    }

    div()
        .flex()
        .items_center()
        .px(chip_px(8.0))
        .py(chip_px(3.0))
        .rounded(chip_px(7.0))
        .bg(surface)
        .child(label_el)
        .into_any_element()
}

fn humanize_provider_name(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "chatgpt" => "ChatGPT".to_string(),
        "openai" => "OpenAI".to_string(),
        "anthropic" => "Anthropic".to_string(),
        "github-copilot" => "GitHub Copilot".to_string(),
        "moonshot" => "Moonshot".to_string(),
        "deepseek" => "DeepSeek".to_string(),
        other if !other.is_empty() => {
            let mut chars = other.chars();
            match chars.next() {
                Some(c) => {
                    let mut s = c.to_uppercase().to_string();
                    s.extend(chars);
                    s
                }
                None => "Con".to_string(),
            }
        }
        _ => "Con".to_string(),
    }
}

fn split_model_identity(model: &str) -> (Option<String>, String) {
    if let Some((provider, inner_model)) = model.split_once(':') {
        let provider = provider.trim();
        let inner_model = inner_model.trim();
        if !provider.is_empty() && !inner_model.is_empty() {
            return (
                Some(humanize_provider_name(provider)),
                humanize_model_name(inner_model),
            );
        }
    }
    if model.is_empty() {
        (None, "Con".to_string())
    } else {
        (None, humanize_model_name(model))
    }
}

fn render_model_chips(
    model: &str,
    duration_ms: Option<u64>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    let (provider, label) = split_model_identity(model);
    let mut row = div().flex().items_center().gap(ui_space_px(theme, 6.0));

    if let Some(provider) = provider {
        row = row
            .child(
                div()
                    .text_size(ui_px(theme, 10.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground.opacity(0.48))
                    .child(provider),
            )
            .child(
                div()
                    .size(ui_space_px(theme, 3.0))
                    .rounded_full()
                    .bg(theme.muted_foreground.opacity(0.18)),
            );
    }

    row = row.child(render_meta_chip(
        label,
        theme.secondary_hover,
        theme.foreground.opacity(0.60),
        theme,
        true,
    ));

    if let Some(dur) = duration_ms {
        row = row.child(
            div()
                .text_size(mono_px(theme, 10.0))
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground.opacity(0.38))
                .child(format_duration_ms(dur)),
        );
    }

    row.into_any_element()
}

fn format_model_identity_text(model: &str) -> String {
    let (provider, label) = split_model_identity(model);
    match provider {
        Some(provider) => format!("{provider} · {label}"),
        None => label,
    }
}

fn render_result_toggle_chrome(
    expanded: bool,
    label: String,
    theme: &gpui_component::Theme,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(mono_space_px(theme, 5.0))
        .py(mono_space_px(theme, 2.0))
        .child(
            svg()
                .path(if expanded {
                    "phosphor/caret-up.svg"
                } else {
                    "phosphor/caret-down.svg"
                })
                .size(mono_space_px(theme, 9.5))
                .text_color(theme.muted_foreground.opacity(0.30)),
        )
        .child(
            div()
                .text_size(mono_px(theme, 10.0))
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground.opacity(0.40))
                .child(label),
        )
        .into_any_element()
}

fn render_section_kicker(label: &str, theme: &gpui_component::Theme) -> AnyElement {
    div()
        .text_size(ui_px(theme, 8.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted_foreground.opacity(0.32))
        .child(label.to_ascii_uppercase())
        .into_any_element()
}

fn render_step_status_tag(status: StepStatus, theme: &gpui_component::Theme) -> Option<AnyElement> {
    match status {
        StepStatus::Running => Some(render_inline_state("Live".into(), theme.warning, theme)),
        StepStatus::Complete => None,
        StepStatus::Denied => Some(render_inline_state("Denied".into(), theme.danger, theme)),
    }
}

fn run_status_summary(step_count: usize, running_count: usize, denied_count: usize) -> String {
    if running_count > 0 {
        format!("{} live · {} total", running_count, step_count)
    } else if denied_count > 0 {
        format!("{} denied · {} total", denied_count, step_count)
    } else {
        format!(
            "{} step{}",
            step_count,
            if step_count == 1 { "" } else { "s" }
        )
    }
}

fn render_user_message_text(
    content: &str,
    msg_idx: usize,
    theme: &gpui_component::Theme,
) -> AnyElement {
    if !content.contains('\n') {
        return div()
            .text_size(ui_px(theme, 13.5))
            .line_height(ui_px(theme, 21.0))
            .text_color(theme.foreground)
            .child(content.to_string())
            .into_any_element();
    }

    let mut block = div()
        .flex()
        .flex_col()
        .gap(ui_space_px(theme, 4.0))
        .text_size(ui_px(theme, 13.5))
        .line_height(ui_px(theme, 21.0))
        .text_color(theme.foreground);

    for (line_idx, line) in content.lines().enumerate() {
        block = if line.is_empty() {
            block.child(
                div()
                    .id(format!("user-msg-{msg_idx}-blank-{line_idx}"))
                    .h(ui_space_px(theme, 10.0)),
            )
        } else {
            block.child(
                div()
                    .id(format!("user-msg-{msg_idx}-line-{line_idx}"))
                    .child(line.to_string()),
            )
        };
    }

    block.into_any_element()
}

fn render_plain_multiline_text(
    content: &str,
    font_family: SharedString,
    font_size: Pixels,
    line_height: Pixels,
    color: Hsla,
) -> AnyElement {
    let display = if content.is_empty() {
        "\u{200B}".to_string()
    } else {
        content.to_string()
    };
    let text_style = TextStyle {
        color,
        font_family,
        font_size: font_size.into(),
        line_height: line_height.into(),
        white_space: WhiteSpace::Normal,
        ..Default::default()
    };
    div()
        .w_full()
        .child(StyledText::new(display.clone()).with_runs(vec![text_style.to_run(display.len())]))
        .into_any_element()
}

fn visible_markdown_block_count(rendered_markdown_blocks: usize, block_count: usize) -> usize {
    rendered_markdown_blocks.max(1).min(block_count)
}

fn unparsed_markdown_suffix(message: &PanelMessage) -> Option<&str> {
    let parsed_len = message.content_markdown_len;
    if parsed_len == 0
        || parsed_len >= message.content.len()
        || !message.content.is_char_boundary(parsed_len)
    {
        return None;
    }

    let suffix = &message.content[parsed_len..];
    (!suffix.is_empty()).then_some(suffix)
}

fn render_assistant_message(
    msg: &PanelMessage,
    panel: WeakEntity<AgentPanel>,
    view: WeakEntity<AssistantMessageView>,
    msg_idx: usize,
    _is_streaming_assistant: bool,
    rendered_markdown_blocks: usize,
    rendered_markdown: Option<AnyElement>,
    theme: &Theme,
) -> AnyElement {
    let assistant_content_for_copy = msg.content.clone();
    let msg_model = msg.model.as_deref().unwrap_or("");
    let msg_duration_ms = msg.duration_ms;
    let mut msg_el = div().flex().flex_col().gap(px(4.0));
    let mut header_row = div().flex().items_center().gap(px(6.0)).pb(px(3.0)).child(
        svg()
            .path("phosphor/oven-duotone.svg")
            .size(px(13.0))
            .text_color(theme.primary.opacity(0.65)),
    );
    header_row = header_row.child(render_model_chips(msg_model, msg_duration_ms, theme));
    msg_el = msg_el.child(header_row);

    if let Some(thinking) = &msg.thinking
        && !thinking.is_empty()
    {
        let thinking_collapsed = msg.thinking_collapsed;
        let chevron = if thinking_collapsed {
            "phosphor/caret-right.svg"
        } else {
            "phosphor/caret-down.svg"
        };
        let word_count = thinking.split_whitespace().count();
        let thinking_summary = if word_count > 0 {
            format!("Thought · {} words", word_count)
        } else {
            "Thinking…".to_string()
        };

        let thinking_panel = panel.clone();
        msg_el = msg_el.child(
            div()
                .id(SharedString::from(format!("thinking-toggle-{msg_idx}")))
                .flex()
                .items_center()
                .gap(px(5.0))
                .ml(px(19.0))
                .py(px(2.0))
                .px(px(4.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme.muted.opacity(0.05)))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    let _ = thinking_panel.update(cx, |this, cx| {
                        if let Some(m) = this.state.messages.get_mut(msg_idx) {
                            m.thinking_collapsed = !m.thinking_collapsed;
                            m.touch();
                        }
                        this.remeasure_message(msg_idx);
                        cx.notify();
                    });
                })
                .child(
                    svg()
                        .path(chevron)
                        .size(px(10.0))
                        .text_color(theme.muted_foreground.opacity(0.3)),
                )
                .child(
                    svg()
                        .path("phosphor/brain-duotone.svg")
                        .size(px(11.0))
                        .text_color(theme.primary.opacity(0.42)),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted_foreground.opacity(0.4))
                        .child(thinking_summary),
                ),
        );

        if !thinking_collapsed {
            let mut thinking_el = div()
                .ml(px(23.0))
                .mr(px(4.0))
                .mt(px(1.0))
                .mb(px(2.0))
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(8.0))
                .bg(theme.muted.opacity(0.04))
                .max_h(px(200.0))
                .overflow_y_hidden()
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(theme.muted_foreground.opacity(0.54));
            if let Some(markdown) = msg.thinking_markdown.as_ref() {
                thinking_el = thinking_el.child(render_parsed_chat_markdown(
                    markdown,
                    ChatMarkdownTone::Thinking,
                    theme,
                ));
            } else {
                thinking_el = thinking_el.child(div().whitespace_normal().child(thinking.clone()));
            }
            msg_el = msg_el.child(thinking_el);
        }
    }

    if !msg.content.is_empty() {
        let mut content_el = div()
            .ml(px(19.0))
            .pr(px(4.0))
            .text_size(px(14.0))
            .line_height(px(23.0))
            .text_color(theme.foreground.opacity(0.88));

        if let Some(markdown) = msg.content_markdown.as_ref() {
            let visible_blocks =
                visible_markdown_block_count(rendered_markdown_blocks, markdown.block_count());
            if let Some(rendered_markdown) = rendered_markdown {
                content_el = content_el.child(rendered_markdown);
            } else {
                content_el =
                    content_el.child(render_parsed_chat_markdown_prefix_with_copy_namespace(
                        markdown,
                        ChatMarkdownTone::Message,
                        theme,
                        visible_blocks,
                        format!("asst-{msg_idx}"),
                    ));
            }
            if let Some(suffix) = unparsed_markdown_suffix(msg) {
                content_el = content_el.child(div().mt(ui_space_px(theme, 6.0)).child(
                    render_plain_multiline_text(
                        suffix,
                        theme.mono_font_family.clone(),
                        mono_px(theme, 14.0),
                        mono_px(theme, 23.0),
                        theme.foreground.opacity(0.78),
                    ),
                ));
            }
            if visible_blocks < markdown.block_count() {
                let remaining = markdown.block_count().saturating_sub(visible_blocks);
                let more_view = view.clone();
                content_el = content_el.child(
                    div().mt(ui_space_px(theme, 8.0)).child(
                        div()
                            .id(SharedString::from(format!("assistant-more-{msg_idx}")))
                            .flex()
                            .items_center()
                            .gap(mono_space_px(theme, 6.0))
                            .px(mono_space_px(theme, 8.0))
                            .py(mono_space_px(theme, 4.0))
                            .rounded(mono_space_px(theme, 7.0))
                            .cursor_pointer()
                            .bg(theme.muted.opacity(0.05))
                            .hover(|s| s.bg(theme.muted.opacity(0.09)))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                let _ = more_view.update(cx, |view, cx| {
                                    view.rendered_markdown_blocks = (view.rendered_markdown_blocks
                                        + LONG_MARKDOWN_BATCH_BLOCKS * 2)
                                        .min(view.state_key.content_markdown_blocks);
                                    let msg_idx = view.msg_idx;
                                    let panel = view.panel.clone();
                                    let _ = panel.update(cx, |panel, cx| {
                                        panel.remeasure_message(msg_idx);
                                        cx.notify();
                                    });
                                    cx.notify();
                                });
                            })
                            .child(
                                svg()
                                    .path("phosphor/caret-down.svg")
                                    .size(mono_space_px(theme, 10.0))
                                    .text_color(theme.muted_foreground.opacity(0.55)),
                            )
                            .child(
                                div()
                                    .text_size(mono_px(theme, 10.5))
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground.opacity(0.58))
                                    .child(format!("Show more · {remaining} blocks left")),
                            ),
                    ),
                );
            }
        } else {
            content_el = content_el.child(render_plain_multiline_text(
                &msg.content,
                theme.mono_font_family.clone(),
                mono_px(theme, 14.0),
                mono_px(theme, 23.0),
                theme.foreground.opacity(0.88),
            ));
        }

        msg_el = msg_el.child(content_el);

        msg_el = msg_el.child(
            div().ml(px(19.0)).mt(px(2.0)).child(
                Clipboard::new(format!("copy-asst-{msg_idx}"))
                    .value(SharedString::from(assistant_content_for_copy)),
            ),
        );
    }

    if !msg.steps.is_empty() {
        let step_count = msg.steps.len();
        let collapsed = msg.steps_collapsed;
        let chevron = if collapsed {
            "phosphor/caret-right.svg"
        } else {
            "phosphor/caret-down.svg"
        };
        let running_count = msg
            .steps
            .iter()
            .filter(|step| step.status == StepStatus::Running)
            .count();
        let denied_count = msg
            .steps
            .iter()
            .filter(|step| step.status == StepStatus::Denied)
            .count();
        let run_title = if running_count > 0 {
            "Working now"
        } else if denied_count > 0 {
            "Run needs review"
        } else {
            "Run trace"
        };

        let run_panel = panel.clone();
        let mut run_card = div()
            .ml(ui_space_px(theme, 19.0))
            .mr(ui_space_px(theme, 4.0))
            .mt(ui_space_px(theme, 6.0))
            .px(ui_space_px(theme, 10.0))
            .py(ui_space_px(theme, 10.0))
            .rounded(ui_space_px(theme, 12.0))
            .bg(trace_group_surface(theme))
            .flex()
            .flex_col()
            .gap(ui_space_px(theme, 10.0));

        run_card = run_card.child(
            div()
                .id(SharedString::from(format!("steps-toggle-{msg_idx}")))
                .flex()
                .items_center()
                .min_w_0()
                .gap(ui_space_px(theme, 8.0))
                .px(ui_space_px(theme, 4.0))
                .py(ui_space_px(theme, 3.0))
                .cursor_pointer()
                .rounded(ui_space_px(theme, 10.0))
                .hover(|s| s.bg(theme.muted.opacity(0.035)))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    let _ = run_panel.update(cx, |this, cx| {
                        if let Some(m) = this.state.messages.get_mut(msg_idx) {
                            m.steps_collapsed = !m.steps_collapsed;
                            m.touch();
                        }
                        this.remeasure_message(msg_idx);
                        cx.notify();
                    });
                })
                .child(
                    svg()
                        .path(chevron)
                        .size(ui_space_px(theme, 11.0))
                        .text_color(theme.muted_foreground.opacity(0.34)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(ui_space_px(theme, 2.0))
                        .child(render_section_kicker(run_title, theme))
                        .child(
                            div()
                                .text_size(ui_px(theme, 12.75))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.foreground.opacity(0.74))
                                .child("Actions, probes, and output"),
                        )
                        .child(
                            div()
                                .text_size(ui_px(theme, 10.75))
                                .text_color(theme.muted_foreground.opacity(0.46))
                                .min_w_0()
                                .child("A compact trace of what the agent actually did"),
                        ),
                )
                .child({
                    let mut summary = div()
                        .flex()
                        .items_center()
                        .gap(ui_space_px(theme, 8.0))
                        .flex_shrink_0()
                        .child(
                            div()
                                .text_size(mono_px(theme, 10.5))
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground.opacity(0.40))
                                .child(run_status_summary(step_count, running_count, denied_count)),
                        );
                    if running_count > 0 {
                        summary = summary.child(render_inline_state(
                            format!("{} live", running_count).into(),
                            theme.warning,
                            theme,
                        ));
                    }
                    if denied_count > 0 {
                        summary = summary.child(render_inline_state(
                            format!("{} denied", denied_count).into(),
                            theme.danger,
                            theme,
                        ));
                    }
                    summary
                }),
        );

        if !collapsed {
            let mut steps_el = div().flex().flex_col().gap(ui_space_px(theme, 8.0));

            for (step_idx, step) in msg.steps.iter().enumerate() {
                let icon_color = match step.status {
                    StepStatus::Running => theme.warning,
                    StepStatus::Complete => theme.muted_foreground.opacity(0.4),
                    StepStatus::Denied => theme.danger.opacity(0.7),
                };

                let has_detail = step.detail.is_some();
                let detail_collapsed = step.detail_collapsed;

                let (step_name, step_detail) = if let Some(colon_pos) = step.label.find(": ") {
                    (&step.label[..colon_pos], Some(&step.label[colon_pos + 2..]))
                } else {
                    (step.label.as_str(), None)
                };

                let mut top_line = div()
                    .flex()
                    .items_center()
                    .gap(ui_space_px(theme, 6.0))
                    .child(
                        svg()
                            .path(step.icon)
                            .size(ui_space_px(theme, 12.0))
                            .flex_shrink_0()
                            .text_color(icon_color),
                    )
                    .child(
                        div()
                            .text_size(ui_px(theme, 12.5))
                            .text_color(theme.foreground.opacity(0.66))
                            .font_weight(FontWeight::MEDIUM)
                            .child(step_name.to_string()),
                    )
                    .child(div().flex_1());

                if let Some(status_tag) = render_step_status_tag(step.status, theme) {
                    top_line = top_line.child(status_tag);
                }

                if let Some(dur) = step.duration {
                    top_line = top_line.child(
                        div()
                            .flex_shrink_0()
                            .text_size(mono_px(theme, 10.0))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground.opacity(0.36))
                            .child(format_step_duration(dur)),
                    );
                }

                if has_detail {
                    top_line = top_line.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(mono_space_px(theme, 4.0))
                            .child(
                                div()
                                    .text_size(mono_px(theme, 10.0))
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground.opacity(0.34))
                                    .child(if detail_collapsed { "Details" } else { "Hide" }),
                            )
                            .child(
                                svg()
                                    .path(if detail_collapsed {
                                        "phosphor/caret-right.svg"
                                    } else {
                                        "phosphor/caret-down.svg"
                                    })
                                    .size(mono_space_px(theme, 10.0))
                                    .flex_shrink_0()
                                    .text_color(theme.muted_foreground.opacity(0.30)),
                            ),
                    );
                }

                let mut step_header = div()
                    .flex()
                    .flex_col()
                    .gap(ui_space_px(theme, 3.0))
                    .px(ui_space_px(theme, 12.0))
                    .py(ui_space_px(theme, 10.0))
                    .child(top_line);

                if let Some(detail_text) = step_detail {
                    step_header = step_header.child(
                        div()
                            .ml(mono_space_px(theme, 18.0))
                            .text_size(mono_px(theme, 10.5))
                            .line_height(mono_px(theme, 15.0))
                            .text_color(theme.muted_foreground.opacity(0.56))
                            .font_family(theme.mono_font_family.clone())
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .child(truncate_str(detail_text, 84)),
                    );
                }

                let mut step_shell = div()
                    .flex()
                    .flex_col()
                    .rounded(ui_space_px(theme, 12.0))
                    .overflow_hidden()
                    .bg(trace_step_surface(theme))
                    .gap(px(0.0));

                let step_header_shell = step_header
                    .bg(trace_step_header_surface(theme))
                    .min_h(ui_space_px(theme, 44.0));

                if has_detail {
                    let step_panel = panel.clone();
                    step_shell = step_shell.child(
                        step_header_shell
                            .id(SharedString::from(format!(
                                "step-detail-{msg_idx}-{step_idx}"
                            )))
                            .cursor_pointer()
                            .hover(|s| s.bg(trace_step_header_hover_surface(theme)))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                let _ = step_panel.update(cx, |this, cx| {
                                    if let Some(m) = this.state.messages.get_mut(msg_idx)
                                        && let Some(s) = m.steps.get_mut(step_idx)
                                    {
                                        s.detail_collapsed = !s.detail_collapsed;
                                        m.touch();
                                    }
                                    this.remeasure_message(msg_idx);
                                    cx.notify();
                                });
                            }),
                    );
                } else {
                    step_shell = step_shell.child(step_header_shell);
                }

                if let Some(detail) = &step.detail
                    && !detail_collapsed
                {
                    let is_expandable = result_is_expandable(detail);
                    let visible_detail = if step.detail_expanded || !is_expandable {
                        detail.to_string()
                    } else {
                        result_preview(detail, TOOL_RESULT_PREVIEW_LINES)
                    };
                    let detail_block = div()
                        .px(ui_space_px(theme, 12.0))
                        .pt(ui_space_px(theme, 10.0))
                        .bg(trace_detail_surface(theme))
                        .child(render_result_block(
                            &visible_detail,
                            &format!("step-result-{msg_idx}-{step_idx}"),
                            theme,
                            true,
                        ));
                    step_shell = step_shell.child(
                        detail_block.with_animation(
                            SharedString::from(format!("step-detail-reveal-{msg_idx}-{step_idx}")),
                            Animation::new(std::time::Duration::from_millis(170))
                                .with_easing(ease_out_quint()),
                            |this, delta| this.opacity(delta).pt(px((1.0 - delta) * 6.0)),
                        ),
                    );
                    if is_expandable {
                        let expanded = step.detail_expanded;
                        let button_label = result_toggle_label(detail, expanded);
                        let detail_toggle_panel = panel.clone();
                        let detail_toggle = div()
                            .px(ui_space_px(theme, 12.0))
                            .pt(ui_space_px(theme, 4.0))
                            .pb(ui_space_px(theme, 10.0))
                            .bg(trace_detail_surface(theme))
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "step-detail-expand-{msg_idx}-{step_idx}"
                                    )))
                                    .cursor_pointer()
                                    .hover(|s| {
                                        s.bg(theme.muted.opacity(0.03))
                                            .rounded(ui_space_px(theme, 6.0))
                                    })
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        let _ = detail_toggle_panel.update(cx, |this, cx| {
                                            if let Some(message) =
                                                this.state.messages.get_mut(msg_idx)
                                                && let Some(step) = message.steps.get_mut(step_idx)
                                            {
                                                step.detail_expanded = !step.detail_expanded;
                                                message.touch();
                                            }
                                            this.remeasure_message(msg_idx);
                                            cx.notify();
                                        });
                                    })
                                    .child(
                                        div()
                                            .px(mono_space_px(theme, 2.0))
                                            .py(mono_space_px(theme, 2.0))
                                            .child(render_result_toggle_chrome(
                                                expanded,
                                                button_label,
                                                theme,
                                            )),
                                    ),
                            );
                        step_shell = step_shell.child(
                            detail_toggle.with_animation(
                                SharedString::from(format!(
                                    "step-detail-toggle-{msg_idx}-{step_idx}"
                                )),
                                Animation::new(std::time::Duration::from_millis(185))
                                    .with_easing(ease_out_quint()),
                                |this, delta| this.opacity(delta).pt(px((1.0 - delta) * 4.0)),
                            ),
                        );
                    } else {
                        step_shell = step_shell.child(
                            div()
                                .px(ui_space_px(theme, 10.0))
                                .pb(ui_space_px(theme, 10.0))
                                .bg(trace_detail_surface(theme)),
                        );
                    }
                }

                steps_el = steps_el.child(step_shell);
            }

            run_card = run_card.child(steps_el.with_animation(
                SharedString::from(format!("steps-reveal-{msg_idx}")),
                Animation::new(std::time::Duration::from_millis(190)).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta).pt(px((1.0 - delta) * 8.0)),
            ));
        }

        msg_el = msg_el.child(run_card);
    }

    msg_el.into_any_element()
}

impl Render for AssistantMessageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let theme = &theme;
        let msg_idx = self.msg_idx;
        let panel = self.panel.clone();
        let view = cx.weak_entity();
        let rendered_markdown_blocks = self.rendered_markdown_blocks;

        if !self.is_streaming_assistant && !self.message.thinking_collapsed {
            self.message.ensure_thinking_markdown();
        }

        if self.is_streaming_assistant
            && self.state_key.content_markdown_blocks > self.rendered_markdown_blocks
        {
            self.schedule_markdown_hydration(cx);
        }

        let content_markdown = self.message.content_markdown.clone();
        let rendered_markdown = content_markdown
            .as_ref()
            .filter(|markdown| markdown.block_count() > 0)
            .map(|markdown| {
                let visible_blocks =
                    visible_markdown_block_count(rendered_markdown_blocks, markdown.block_count());
                self.render_markdown_blocks(markdown, visible_blocks, theme, cx)
            });

        render_assistant_message(
            &self.message,
            panel,
            view,
            msg_idx,
            self.is_streaming_assistant,
            rendered_markdown_blocks,
            rendered_markdown,
            theme,
        )
    }
}

impl AgentPanel {
    fn render_message_item(
        &mut self,
        msg_idx: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let theme = &theme;
        let msg = &self.state.messages[msg_idx];
        let is_user = msg.role == "user";
        let is_system = msg.role == "system";

        let mut msg_el = div().flex().flex_col().gap(px(4.0));

        if is_system {
            msg_el = msg_el.child(
                div()
                    .px(ui_space_px(theme, 6.0))
                    .py(ui_space_px(theme, 8.0))
                    .text_size(ui_px(theme, 12.5))
                    .text_color(theme.muted_foreground.opacity(0.40))
                    .line_height(ui_px(theme, 19.0))
                    .child(msg.content.clone()),
            );
        } else if is_user {
            let is_editing = self.editing_msg_idx == Some(msg_idx);
            if is_editing {
                if let Some(edit_input) = &self.edit_input_state {
                    msg_el = msg_el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .items_end()
                            .child(
                                div()
                                    .w_full()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(12.0))
                                    .bg(theme.primary.opacity(0.07))
                                    .font_family(theme.mono_font_family.clone())
                                    .on_key_down(cx.listener(
                                        |this, event: &KeyDownEvent, _window, cx| {
                                            if event.keystroke.key == "escape" {
                                                this.cancel_edit(cx);
                                            }
                                        },
                                    ))
                                    .child(
                                        Input::new(edit_input)
                                            .appearance(false)
                                            .cleanable(false)
                                            .font_family(theme.mono_font_family.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .id(ElementId::Name(
                                                format!("edit-cancel-{msg_idx}").into(),
                                            ))
                                            .h(px(24.0))
                                            .px(px(10.0))
                                            .flex()
                                            .items_center()
                                            .rounded(px(5.0))
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .cursor_pointer()
                                            .text_color(theme.muted_foreground)
                                            .hover(|s| s.bg(theme.muted.opacity(0.08)))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.cancel_edit(cx);
                                                }),
                                            )
                                            .child("Cancel"),
                                    )
                                    .child(
                                        div()
                                            .id(ElementId::Name(
                                                format!("edit-submit-{msg_idx}").into(),
                                            ))
                                            .h(px(24.0))
                                            .px(px(10.0))
                                            .flex()
                                            .items_center()
                                            .rounded(px(5.0))
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .cursor_pointer()
                                            .bg(theme.primary)
                                            .text_color(theme.primary_foreground)
                                            .hover(|s| s.bg(theme.primary_hover))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.submit_edit(cx);
                                                }),
                                            )
                                            .child("Send"),
                                    ),
                            ),
                    );
                }
            } else {
                let user_content: String = msg.content.clone();
                msg_el = msg_el.child(
                    div()
                        .id(ElementId::Name(format!("user-msg-{msg_idx}").into()))
                        .group("user-msg")
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(3.0))
                        .child(
                            div()
                                .max_w(rems(22.0))
                                .px(px(12.0))
                                .py(px(8.0))
                                .rounded(px(14.0))
                                .rounded_tr(px(4.0))
                                .bg(theme.primary.opacity(0.06))
                                .child(render_user_message_text(&user_content, msg_idx, theme)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(1.0))
                                .invisible()
                                .group_hover("user-msg", |s| s.visible())
                                .child(
                                    Clipboard::new(format!("copy-user-{msg_idx}"))
                                        .value(SharedString::from(user_content.clone())),
                                )
                                .child({
                                    let content_for_edit = user_content.clone();
                                    div()
                                        .id(ElementId::Name(format!("edit-{msg_idx}").into()))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(24.0))
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.muted.opacity(0.10)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                this.start_editing(
                                                    msg_idx,
                                                    &content_for_edit,
                                                    window,
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(
                                            svg()
                                                .path("phosphor/pencil-simple.svg")
                                                .size(px(13.0))
                                                .text_color(theme.muted_foreground.opacity(0.4)),
                                        )
                                })
                                .child({
                                    let content_for_rerun = user_content.clone();
                                    div()
                                        .id(ElementId::Name(format!("rerun-{msg_idx}").into()))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(24.0))
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.muted.opacity(0.10)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.rerun_from(
                                                    msg_idx,
                                                    content_for_rerun.clone(),
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(
                                            svg()
                                                .path("phosphor/arrow-clockwise.svg")
                                                .size(px(13.0))
                                                .text_color(theme.muted_foreground.opacity(0.4)),
                                        )
                                }),
                        ),
                );
            }
        } else {
            let is_streaming_assistant =
                self.state.streaming && msg_idx + 1 == self.state.messages.len();
            if !msg.content.is_empty()
                && !msg.content_markdown_pending
                && msg.content_markdown_len < msg.content.len()
            {
                self.ensure_content_markdown_async(msg_idx, cx);
            }

            if let Some(view) =
                self.sync_assistant_message_view(msg_idx, is_streaming_assistant, cx)
            {
                msg_el = msg_el.child(view);
            }
        }

        msg_el.into_any_element()
    }
}

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inline_input_state(window, cx);
        self.install_message_list_handler(cx);
        self.sync_message_list_count();

        let theme = cx.theme().clone();
        let theme = &theme;
        let content_reveal = self.content_reveal.value(window);

        // ── Messages ──────────────────────────────────────────────
        let total_messages = self.state.messages.len();
        if self.assistant_message_views.len() > total_messages {
            self.assistant_message_views.truncate(total_messages);
        }
        let messages_content = list(
            self.message_list_state.clone(),
            cx.processor(|this, msg_idx: usize, window, cx| {
                div()
                    .w_full()
                    .pb(px(16.0))
                    .child(this.render_message_item(msg_idx, window, cx))
                    .into_any_element()
            }),
        )
        .with_sizing_behavior(ListSizingBehavior::Auto)
        .h_full()
        .min_h_0()
        .w_full()
        .pt(px(12.0))
        .pb(px(64.0))
        .flex_grow();

        let mut transcript_footer = div().flex().flex_col().gap(px(16.0));

        // ── Active tool calls (skip if awaiting approval — the card shows it) ──
        // Collect call_ids that have pending approvals to avoid duplicate rendering.
        let pending_call_ids: std::collections::HashSet<&str> = self
            .state
            .pending_approvals
            .iter()
            .map(|a| a.call_id.as_str())
            .collect();

        let visible_tool_calls: Vec<_> = self
            .state
            .tool_calls
            .iter()
            .enumerate()
            .filter(|(_, tc)| !pending_call_ids.contains(tc.call_id.as_str()))
            .collect();

        if !visible_tool_calls.is_empty() {
            let mut tc_container = div()
                .flex()
                .flex_col()
                .ml(ui_space_px(theme, 19.0))
                .mr(ui_space_px(theme, 4.0))
                .gap(ui_space_px(theme, 10.0))
                .px(ui_space_px(theme, 10.0))
                .py(ui_space_px(theme, 10.0))
                .rounded(ui_space_px(theme, 12.0))
                .bg(trace_group_surface(theme))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .gap(ui_space_px(theme, 8.0))
                        .px(ui_space_px(theme, 4.0))
                        .py(ui_space_px(theme, 3.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(ui_space_px(theme, 2.0))
                                .child(render_section_kicker("Working now", theme))
                                .child(
                                    div()
                                        .text_size(ui_px(theme, 12.75))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.foreground.opacity(0.74))
                                        .child("Live tools for this reply"),
                                )
                                .child(
                                    div()
                                        .text_size(ui_px(theme, 10.75))
                                        .text_color(theme.muted_foreground.opacity(0.46))
                                        .min_w_0()
                                        .child("Commands and reads still in progress"),
                                ),
                        )
                        .child(render_inline_state(
                            format!(
                                "{} live",
                                visible_tool_calls
                                    .iter()
                                    .filter(|(_, tc)| tc.result.is_none())
                                    .count()
                            )
                            .into(),
                            theme.warning,
                            theme,
                        )),
                );

            for (tc_idx, tc) in visible_tool_calls {
                let is_done = tc.result.is_some();
                let icon = tool_icon(&tc.tool_name);
                let args_display = format_tool_args(&tc.tool_name, &tc.args);
                let human_name = humanize_tool_name(&tc.tool_name);

                let icon_color = if is_done {
                    theme.muted_foreground.opacity(0.4)
                } else {
                    theme.warning
                };

                // Icon or spinner
                let icon_el: AnyElement = if is_done {
                    svg()
                        .path(icon)
                        .size(ui_space_px(theme, 12.0))
                        .flex_shrink_0()
                        .text_color(icon_color)
                        .into_any_element()
                } else {
                    div()
                        .flex_shrink_0()
                        .child(Spinner::new().small().color(theme.warning))
                        .into_any_element()
                };

                let dur = tc.duration.unwrap_or_else(|| tc.started_at.elapsed());

                // Two-line layout matching completed steps
                let top_line = div()
                    .flex()
                    .items_center()
                    .gap(ui_space_px(theme, 6.0))
                    .child(icon_el)
                    .child(
                        div()
                            .text_size(ui_px(theme, 12.5))
                            .text_color(theme.foreground.opacity(0.66))
                            .font_weight(FontWeight::MEDIUM)
                            .child(human_name),
                    )
                    .child(div().flex_1()) // spacer
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(mono_px(theme, 10.0))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground.opacity(0.36))
                            .child(format_step_duration(dur)),
                    );
                let top_line = if is_done {
                    top_line
                } else {
                    top_line.child(render_inline_state("Live".into(), theme.warning, theme))
                };

                let mut tc_row = div()
                    .flex()
                    .flex_col()
                    .gap(ui_space_px(theme, 3.0))
                    .px(ui_space_px(theme, 12.0))
                    .py(ui_space_px(theme, 10.0))
                    .child(top_line);

                // Args on second line
                if !args_display.is_empty() {
                    tc_row = tc_row.child(
                        div()
                            .ml(mono_space_px(theme, 18.0))
                            .text_size(mono_px(theme, 10.75))
                            .text_color(theme.muted_foreground.opacity(0.50))
                            .font_family(theme.mono_font_family.clone())
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .child(truncate_str(&args_display, 60)),
                    );
                }

                let mut tc_el = div()
                    .flex()
                    .flex_col()
                    .rounded(ui_space_px(theme, 12.0))
                    .overflow_hidden()
                    .bg(trace_step_surface(theme))
                    .gap(px(0.0))
                    .child(
                        tc_row
                            .bg(trace_step_header_surface(theme))
                            .min_h(ui_space_px(theme, 44.0)),
                    );

                // Result preview
                if let Some(result) = &tc.result {
                    let formatted = format_tool_result(&tc.tool_name, result);
                    let is_expandable = result_is_expandable(&formatted);
                    let visible = if tc.result_expanded || !is_expandable {
                        formatted.clone()
                    } else {
                        result_preview(&formatted, TOOL_RESULT_PREVIEW_LINES)
                    };
                    tc_el = tc_el.child(
                        div()
                            .px(px(12.0))
                            .pt(px(10.0))
                            .bg(trace_detail_surface(theme))
                            .child(render_result_block(
                                &visible,
                                &format!("tc-result-{tc_idx}"),
                                theme,
                                true,
                            )),
                    );
                    if is_expandable {
                        let expanded = tc.result_expanded;
                        let button_label = result_toggle_label(&formatted, expanded);
                        tc_el = tc_el.child(
                            div().px(px(12.0)).pt(px(4.0)).pb(px(10.0)).child(
                                div()
                                    .id(SharedString::from(format!("tc-result-expand-{tc_idx}")))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.muted.opacity(0.03)).rounded(px(6.0)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            if let Some(tool_call) =
                                                this.state.tool_calls.get_mut(tc_idx)
                                            {
                                                tool_call.result_expanded =
                                                    !tool_call.result_expanded;
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(div().px(px(2.0)).py(px(2.0)).child(
                                        render_result_toggle_chrome(expanded, button_label, theme),
                                    )),
                            ),
                        );
                    } else {
                        tc_el = tc_el.child(
                            div()
                                .px(px(12.0))
                                .pb(px(10.0))
                                .bg(trace_detail_surface(theme)),
                        );
                    }
                }
                tc_container = tc_container.child(tc_el);
            }
            transcript_footer = transcript_footer.child(tc_container);
        }

        // ── Pending approvals ───────────────────────────────────
        for (i, approval) in self.state.pending_approvals.iter().enumerate() {
            let icon = tool_icon(&approval.tool_name);
            let args_display = format_tool_args(&approval.tool_name, &approval.args);
            let allow_idx = i;
            let deny_idx = i;
            let human_tool = humanize_tool_name(&approval.tool_name);

            // Approval card — clean, confident layout
            let approval_el =
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .ml(px(19.0))
                    .mr(px(4.0))
                    .px(px(12.0))
                    .py(px(11.0))
                    .rounded(px(14.0))
                    .bg(theme.warning.opacity(0.035))
                    // Header — tool info
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(render_section_kicker("Approval needed", theme))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                svg()
                                                    .path(icon)
                                                    .size(px(13.0))
                                                    .flex_shrink_0()
                                                    .text_color(theme.warning.opacity(0.70)),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.5))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(theme.foreground.opacity(0.70))
                                                    .flex_shrink_0()
                                                    .child(human_tool),
                                            ),
                                    ),
                            )
                            .child(div().flex_1())
                            .child(div().text_size(px(12.5)).flex_shrink_0().child(
                                render_inline_state("Approval".into(), theme.warning, theme),
                            )),
                    )
                    // Args — monospace, full width
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground.opacity(0.55))
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .child(truncate_str(&args_display, 80)),
                    )
                    .child(Divider::horizontal().color(theme.warning.opacity(0.12)))
                    // Action row — clear hierarchy
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                Button::new(format!("allow-{i}"))
                                    .label("Allow")
                                    .small()
                                    .primary()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.resolve_approval(allow_idx, true, cx);
                                    })),
                            )
                            .child(
                                Button::new(format!("allow-all-{i}"))
                                    .label("Allow All")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.auto_approve = true;
                                        cx.emit(SetAutoApprove { enabled: true });
                                        this.resolve_all_approvals(cx);
                                    })),
                            )
                            .child(
                                Button::new(format!("deny-{i}"))
                                    .label("Deny")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.resolve_approval(deny_idx, false, cx);
                                    })),
                            ),
                    );

            transcript_footer = transcript_footer.child(approval_el);
        }

        // ── Status indicator (hidden when approval cards are visible — they ARE the status) ──
        if self.state.pending_approvals.is_empty() {
            if let Some((_icon, label)) = self.status_text() {
                let status_color = match self.state.status {
                    AgentStatus::Thinking => theme.warning,
                    AgentStatus::Responding => theme.success,
                    AgentStatus::Idle => theme.muted_foreground,
                };
                transcript_footer = transcript_footer.child(
                    div()
                        .ml(px(19.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .py(px(3.0))
                        .px(px(4.0))
                        .child(Spinner::new().small().color(status_color))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme.muted_foreground.opacity(0.50))
                                .child(label),
                        ),
                );
            }
        } // end pending_approvals.is_empty() guard

        // ── Header ──────────────────────────────────────────────
        let status_indicator: AnyElement = match self.state.status {
            AgentStatus::Idle => div()
                .size(px(7.0))
                .rounded_full()
                .bg(theme.muted_foreground.opacity(0.25))
                .into_any_element(),
            AgentStatus::Thinking => Spinner::new()
                .small()
                .color(theme.warning)
                .into_any_element(),
            AgentStatus::Responding => Spinner::new()
                .small()
                .color(theme.success)
                .into_any_element(),
        };

        let header_left = div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .flex_shrink_0()
            .child(
                svg()
                    .path("phosphor/oven-duotone.svg")
                    .size(px(14.0))
                    .text_color(theme.primary),
            )
            .child(status_indicator);

        let control_icons = {
            let mut actions = div().flex().items_center().gap(px(2.0)).flex_shrink_0();

            if self.state.status != AgentStatus::Idle {
                actions = actions.child(
                    Button::new("agent-stop")
                        .icon(Icon::default().path("phosphor/stop.svg"))
                        .ghost()
                        .xsmall()
                        .tooltip("Stop")
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.emit(CancelRequest);
                            this.state.status = AgentStatus::Idle;
                            this.state.streaming = false;
                            cx.notify();
                        })),
                );
            }

            actions
                .child(
                    Button::new("agent-auto-approve")
                        .icon(Icon::default().path(if self.auto_approve {
                            "phosphor/fast-forward-duotone.svg"
                        } else {
                            "phosphor/fast-forward.svg"
                        }))
                        .ghost()
                        .xsmall()
                        .text_color(if self.auto_approve {
                            theme.primary
                        } else {
                            theme.muted_foreground.opacity(0.65)
                        })
                        .tooltip(if self.auto_approve {
                            "Auto-approve on"
                        } else {
                            "Auto-approve off"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            let enabled = !this.auto_approve;
                            this.auto_approve = enabled;
                            cx.emit(SetAutoApprove { enabled });
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("agent-history-toggle")
                        .icon(Icon::default().path("phosphor/clock-counter-clockwise.svg"))
                        .ghost()
                        .xsmall()
                        .tooltip("History")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_history(cx);
                        })),
                )
                .child(
                    Button::new("agent-new-chat")
                        .icon(Icon::default().path("phosphor/plus.svg"))
                        .ghost()
                        .xsmall()
                        .tooltip("New Chat")
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.emit(NewConversation);
                            this.clear_messages(cx);
                        })),
                )
        };

        let current_provider = self.current_provider_option();
        let provider_icon_path = current_provider
            .as_ref()
            .map(|provider| provider.icon_path)
            .unwrap_or("phosphor/plugs-connected.svg");
        let provider_short_label = current_provider
            .as_ref()
            .map(|provider| provider.short_label)
            .unwrap_or("AI");
        let panel = cx.entity().clone();
        let provider_picker = Button::new("agent-provider-picker")
            .ghost()
            .xsmall()
            .compact()
            .dropdown_caret(true)
            .tooltip("Provider")
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .bg(theme.title_bar.opacity(self.ui_opacity * 0.92))
                            .child(
                                svg()
                                    .path(provider_icon_path)
                                    .size(px(12.0))
                                    .text_color(theme.foreground.opacity(0.82)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .line_height(px(12.0))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.foreground.opacity(0.88))
                            .child(provider_short_label),
                    ),
            )
            .dropdown_menu_with_anchor(Corner::BottomLeft, {
                let options = self.session_provider_options.clone();
                let current_provider = self.current_provider.clone();
                let panel = panel.clone();
                move |menu: PopupMenu, _, _| {
                    options.iter().cloned().fold(
                        menu.min_w(px(180.0)).max_h(px(240.0)).scrollable(true),
                        |menu, option| {
                            let checked = current_provider.as_ref() == Some(&option.provider);
                            menu.item(
                                PopupMenuItem::new(option.label)
                                    .icon(Icon::default().path(option.icon_path))
                                    .checked(checked)
                                    .on_click({
                                        let provider = option.provider.clone();
                                        let panel = panel.clone();
                                        move |_, _, cx| {
                                            let _ = panel.update(cx, |_, cx| {
                                                cx.emit(SelectSessionProvider {
                                                    provider: provider.clone(),
                                                });
                                            });
                                        }
                                    }),
                            )
                        },
                    )
                }
            });

        let model_label = if self.model_name.is_empty() {
            SharedString::from("Model")
        } else {
            SharedString::from(humanize_model_name(&self.model_name))
        };
        let model_picker = div().w(px(156.0)).flex_shrink_0().overflow_hidden().child(
            Button::new("agent-model-picker")
                .ghost()
                .xsmall()
                .compact()
                .dropdown_caret(true)
                .tooltip("Model")
                .disabled(self.session_model_options.is_empty())
                .label(model_label)
                .dropdown_menu_with_anchor(Corner::BottomLeft, {
                    let current_model = self.model_name.clone();
                    let options = self.session_model_options.clone();
                    let panel = panel.clone();
                    move |menu: PopupMenu, _, _| {
                        options.iter().cloned().fold(
                            menu.min_w(px(240.0)).max_h(px(240.0)).scrollable(true),
                            |menu, model: String| {
                                let checked = model == current_model;
                                menu.item(
                                    PopupMenuItem::new(model.clone())
                                        .checked(checked)
                                        .on_click({
                                            let selected_model = model.clone();
                                            let panel = panel.clone();
                                            move |_, _, cx| {
                                                let _ = panel.update(cx, |_, cx| {
                                                    cx.emit(SelectSessionModel {
                                                        model: selected_model.clone(),
                                                    });
                                                });
                                            }
                                        }),
                                )
                            },
                        )
                    }
                }),
        );

        let header = div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .w_full()
            .min_w_0()
            .px(px(14.0))
            .py(px(10.0))
            .flex_shrink_0()
            .child(header_left)
            .child(provider_picker)
            .child(model_picker);
        let header = header.child(control_icons);

        let live_activity_strip = if let Some(approval) = self.state.pending_approvals.first() {
            let args_display = format_tool_args(&approval.tool_name, &approval.args);
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(ui_space_px(theme, 7.0))
                    .mx(ui_space_px(theme, 14.0))
                    .mb(ui_space_px(theme, 8.0))
                    .child(render_section_kicker("Needs approval", theme))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(ui_space_px(theme, 8.0))
                            .child(render_inline_state(
                                "Approval waiting".into(),
                                theme.warning,
                                theme,
                            ))
                            .child(
                                div()
                                    .text_size(mono_px(theme, 10.5))
                                    .line_height(mono_px(theme, 15.0))
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground.opacity(0.60))
                                    .min_w_0()
                                    .overflow_x_hidden()
                                    .whitespace_nowrap()
                                    .child(truncate_str(&args_display, 96)),
                            ),
                    )
                    .child(Divider::horizontal().color(theme.muted.opacity(0.10)))
                    .into_any_element(),
            )
        } else if let Some(tc) = self.running_tool_call() {
            let args_display = format_tool_args(&tc.tool_name, &tc.args);
            let human_name = humanize_tool_name(&tc.tool_name);
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(ui_space_px(theme, 7.0))
                    .mx(ui_space_px(theme, 14.0))
                    .mb(ui_space_px(theme, 8.0))
                    .child(render_section_kicker("Current run", theme))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(ui_space_px(theme, 8.0))
                            .child(Spinner::new().small().color(theme.warning))
                            .child(
                                div()
                                    .text_size(ui_px(theme, 12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.foreground.opacity(0.74))
                                    .child(human_name),
                            )
                            .child(
                                div()
                                    .text_size(mono_px(theme, 10.5))
                                    .line_height(mono_px(theme, 15.0))
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground.opacity(0.60))
                                    .min_w_0()
                                    .overflow_x_hidden()
                                    .whitespace_nowrap()
                                    .child(truncate_str(&args_display, 96)),
                            ),
                    )
                    .child(Divider::horizontal().color(theme.muted.opacity(0.10)))
                    .into_any_element(),
            )
        } else if let Some((_icon, label)) = self.status_text() {
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(ui_space_px(theme, 7.0))
                    .mx(ui_space_px(theme, 14.0))
                    .mb(ui_space_px(theme, 8.0))
                    .child(render_section_kicker("Live status", theme))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(ui_space_px(theme, 8.0))
                            .child(
                                Spinner::new()
                                    .small()
                                    .color(theme.muted_foreground.opacity(0.6)),
                            )
                            .child(
                                div()
                                    .text_size(ui_px(theme, 12.0))
                                    .text_color(theme.foreground.opacity(0.66))
                                    .child(label),
                            ),
                    )
                    .child(Divider::horizontal().color(theme.muted.opacity(0.10)))
                    .into_any_element(),
            )
        } else {
            None
        };

        // ── Panel ───────────────────────────────────────────────
        // Use system proportional font for readable prose — the workspace root
        // sets Ioskeley Mono which would cascade here without this override.
        let mut panel = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .items_stretch()
            .bg(theme.title_bar.opacity(self.ui_opacity))
            .font_family(theme.font_family.clone())
            .text_size(theme.font_size)
            .child(header)
            .child(Divider::horizontal().color(theme.muted.opacity(0.08)));

        if let Some(live_activity_strip) = live_activity_strip {
            panel = panel.child(live_activity_strip);
        }

        if self.showing_history {
            let mut history_content = div()
                .id("agent-history-list")
                .flex()
                .flex_col()
                .h_full()
                .overflow_y_scroll()
                .track_scroll(&self.history_scroll_handle)
                .pl(px(14.0))
                .pr(px(22.0))
                .pt(px(8.0))
                .gap(px(1.0));

            if self.conversation_list.is_empty() {
                history_content = history_content.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .p(px(16.0))
                        .child("No saved conversations"),
                );
            } else {
                for (i, summary) in self.conversation_list.iter().enumerate() {
                    let conv_id = summary.id.clone();
                    let delete_id = summary.id.clone();
                    let date = format_conversation_date(summary);
                    let msg_count = summary.message_count;
                    let model_part = summary
                        .model
                        .as_deref()
                        .filter(|m| !m.is_empty())
                        .map(|m| format!(" · {}", format_model_identity_text(m)))
                        .unwrap_or_default();
                    history_content = history_content.child(
                        div()
                            .id(SharedString::from(format!("conv-{i}")))
                            .group("conv-row")
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(12.0))
                            .h(px(44.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.muted.opacity(0.06)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.emit(LoadConversation {
                                        id: conv_id.clone(),
                                    });
                                    this.showing_history = false;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .overflow_x_hidden()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child(truncate_str(&summary.title, 36)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground.opacity(0.6))
                                            .child(format!(
                                                "{date}{model_part}  ·  {msg_count} messages"
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("del-wrap-{i}")))
                                    .flex()
                                    .items_center()
                                    .flex_shrink_0()
                                    .invisible()
                                    .group_hover("conv-row", |s| s.visible())
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .child(
                                        Button::new(SharedString::from(format!("del-conv-{i}")))
                                            .icon(Icon::default().path("phosphor/trash.svg"))
                                            .ghost()
                                            .xsmall()
                                            .on_click(cx.listener(move |_this, _, _, cx| {
                                                cx.emit(DeleteConversation {
                                                    id: delete_id.clone(),
                                                });
                                            })),
                                    ),
                            ),
                    );
                }
            }
            // Container: relative + scrollbar on container, content scrolls inside
            panel = panel.child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        history_content
                            .opacity(content_reveal)
                            .pt(vertical_reveal_offset(content_reveal, 10.0)),
                    )
                    .vertical_scrollbar(&self.history_scroll_handle),
            );
        } else {
            let show_jump_to_latest = self.follow_output == FollowOutputState::Detached
                && !self.is_scrolled_near_bottom();
            let transcript_surface = div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    div().size_full().min_h_0().pl(px(14.0)).pr(px(34.0)).child(
                        messages_content
                            .opacity(content_reveal)
                            .pt(vertical_reveal_offset(content_reveal, 10.0)),
                    ),
                )
                .vertical_scrollbar(&self.message_list_state);

            let mut messages_container = div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(transcript_surface);

            messages_container = messages_container.child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .max_h(px(320.0))
                    .min_h_0()
                    .pl(px(16.0))
                    .pr(px(22.0))
                    .pb(px(12.0))
                    .child(
                        div()
                            .id("agent-transcript-footer")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.transcript_footer_scroll_handle)
                            .child(
                                transcript_footer
                                    .opacity(content_reveal)
                                    .pt(vertical_reveal_offset(content_reveal, 10.0)),
                            ),
                    )
                    .vertical_scrollbar(&self.transcript_footer_scroll_handle),
            );

            if show_jump_to_latest {
                messages_container = messages_container.child(
                    div()
                        .absolute()
                        .right(px(14.0))
                        .bottom(if self.show_inline_input {
                            px(66.0)
                        } else {
                            px(14.0)
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded_full()
                                .cursor_pointer()
                                .bg(theme.title_bar.opacity(0.84))
                                .hover(|s| {
                                    s.bg(theme.title_bar.opacity(0.94))
                                        .text_color(theme.foreground.opacity(0.96))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.follow_output = FollowOutputState::PendingJump;
                                        this.scroll_to_bottom();
                                        cx.notify();
                                    }),
                                )
                                .child(div().w(px(10.0)).h(px(10.0)).flex_shrink_0())
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .line_height(px(11.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.foreground.opacity(0.86))
                                        .child("Latest"),
                                )
                                .child(
                                    svg()
                                        .path("phosphor/arrow-line-down.svg")
                                        .size(px(10.0))
                                        .flex_shrink_0()
                                        .text_color(theme.muted_foreground.opacity(0.8)),
                                ),
                        ),
                );
            }

            panel = panel.child(messages_container);
        }

        // Inline input — shown when main input bar is hidden
        if self.show_inline_input {
            // State was pre-created above before the theme borrow
            let inline_input = self.inline_input_state.clone().unwrap();

            let has_text = self.inline_input_has_text;
            let has_skills = self.inline_input_has_skills;
            let inline_control_size = mono_space_px(theme, 24.0);
            let inline_input_text_size = mono_px(theme, 13.0);

            // Send button — circular, matches main input bar
            let send_button = div()
                .id("inline-send-btn")
                .flex()
                .items_center()
                .justify_center()
                .size(inline_control_size)
                .rounded(mono_space_px(theme, 12.0))
                .cursor_pointer()
                .flex_shrink_0()
                .bg(if has_text && !has_skills {
                    theme.primary
                } else {
                    theme.muted.opacity(0.12)
                })
                .hover(|s| {
                    if has_text {
                        s.bg(theme.primary_hover)
                    } else {
                        s.bg(theme.muted.opacity(0.18))
                    }
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        if let Some(ref input) = this.inline_input_state {
                            let text = input.read(cx).text().to_string();
                            if !text.trim().is_empty() {
                                input.update(cx, |s, cx| s.set_value("", window, cx));
                                cx.emit(InlineInputSubmit { text });
                            }
                        }
                    }),
                )
                .child(
                    svg()
                        .path("phosphor/arrow-up.svg")
                        .size(mono_space_px(theme, 12.0))
                        .text_color(if has_text && !has_skills {
                            theme.primary_foreground
                        } else {
                            theme.muted_foreground.opacity(0.4)
                        }),
                );

            panel = panel.child(
                div()
                    .absolute()
                    .left(mono_space_px(theme, 8.0))
                    .right(mono_space_px(theme, 9.0))
                    .bottom(mono_space_px(theme, 8.0))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        let key = event.keystroke.key.as_str();
                        let has_completions = !this.filtered_inline_skills(cx).is_empty();

                        if key == "tab" && has_completions {
                            let skills = this.filtered_inline_skills(cx);
                            let sel = this
                                .inline_skill_selection
                                .min(skills.len().saturating_sub(1));
                            if let Some(skill) = skills.get(sel) {
                                let name = skill.name.clone();
                                this.complete_inline_skill(&name, window, cx);
                            }
                            cx.stop_propagation();
                            return;
                        }

                        if key == "up" || key == "down" {
                            if this.handle_inline_navigation_key(key, window, cx) {
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                            return;
                        }

                        if key == "escape" && has_completions {
                            if let Some(ref input) = this.inline_input_state {
                                input.update(cx, |s, cx| s.set_value("", window, cx));
                            }
                            this.inline_skill_selection = 0;
                            cx.emit(InlineSkillAutocompleteChanged);
                            cx.notify();
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }))
                    // Input container — matches main input bar visual style
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .min_h(mono_space_px(theme, 46.0))
                            .gap(mono_space_px(theme, 8.0))
                            .pl(mono_space_px(theme, 7.0))
                            .pr(mono_space_px(theme, 8.0))
                            .py(mono_space_px(theme, 8.0))
                            .rounded(mono_space_px(theme, 14.0))
                            .bg(theme.background)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    if let Some(ref input) = this.inline_input_state {
                                        input.read(cx).focus_handle(cx).focus(window, cx);
                                    }
                                }),
                            )
                            // Text field
                            .child(
                                div()
                                    .flex_1()
                                    .w_full()
                                    .min_w(mono_space_px(theme, 120.0))
                                    .font_family(theme.mono_font_family.clone())
                                    .text_size(inline_input_text_size)
                                    .child(
                                        div().w_full().child(
                                            Input::new(&inline_input)
                                                .appearance(false)
                                                .cleanable(false)
                                                .font_family(theme.mono_font_family.clone())
                                                .text_size(inline_input_text_size),
                                        ),
                                    ),
                            )
                            .child(send_button),
                    ),
            );
        }

        panel
    }
}

/// Convert model ID to a human-readable short name.
fn humanize_model_name(model: &str) -> String {
    // Common patterns: "claude-sonnet-4-6" → "Sonnet 4.6",
    // "claude-sonnet-4-5-20250929" → "Sonnet 4.5"
    if model.contains("claude") {
        if let Some(rest) = model.strip_prefix("claude-") {
            let mut segments: Vec<&str> = rest.split('-').collect();
            // Strip trailing date stamp (8+ contiguous digits, e.g. 20250929).
            if let Some(last) = segments.last() {
                if last.len() >= 8 && last.chars().all(|c| c.is_ascii_digit()) {
                    segments.pop();
                }
            }
            if segments.len() >= 2 {
                let family = segments[0];
                let version = segments[1..].join(".");
                let family_cap = if let Some(first) = family.chars().next() {
                    format!("{}{}", first.to_uppercase(), &family[first.len_utf8()..])
                } else {
                    String::new()
                };
                return format!("{} {}", family_cap, version);
            }
        }
    }
    if model.contains("gpt-4") {
        return model.replace("gpt-", "GPT-");
    }
    if model.is_empty() {
        return "No model".to_string();
    }
    // Fallback: show as-is but truncated
    truncate_str(model, 18)
}

#[cfg(test)]
mod tests {
    use super::{AgentPanel, PanelState, StepStatus, humanize_model_name};
    use con_agent::{AgentConfig, ProviderConfig, ProviderKind};

    #[test]
    fn humanize_strips_date_suffix() {
        assert_eq!(
            humanize_model_name("claude-sonnet-4-5-20250929"),
            "Sonnet 4.5"
        );
        assert_eq!(humanize_model_name("claude-opus-4-1-20250805"), "Opus 4.1");
        assert_eq!(humanize_model_name("claude-sonnet-4-6"), "Sonnet 4.6");
        assert_eq!(humanize_model_name(""), "No model");
    }

    #[test]
    fn humanize_unknown_model_truncates_at_eighteen_chars() {
        // Unknown provider/family hits the fallback `truncate_str(model, 18)` branch.
        let short = "kimi-k2-0905";
        assert_eq!(humanize_model_name(short), short);

        let long = "some-unknown-model-name-2026";
        let result = humanize_model_name(long);
        assert!(result.ends_with('…'), "expected ellipsis, got {result:?}");
        // 18-char prefix + the ellipsis character.
        assert_eq!(result.chars().count(), 19);
        assert!(result.starts_with(&long[..18]));
    }

    #[test]
    fn multiline_drafts_disable_inline_history_navigation() {
        let text = "first line\nsecond line\nthird line";

        assert!(!AgentPanel::inline_history_boundary(text, 0, true));
        assert!(!AgentPanel::inline_history_boundary(text, 4, true));
        assert!(!AgentPanel::inline_history_boundary(text, 12, true));
        assert!(!AgentPanel::inline_history_boundary(text, 15, false));
        assert!(!AgentPanel::inline_history_boundary(
            text,
            text.chars().count() as u32,
            false,
        ));
    }

    #[test]
    fn single_line_drafts_keep_inline_history_navigation() {
        let text = "single line";

        assert!(AgentPanel::inline_history_boundary(text, 0, true));
        assert!(AgentPanel::inline_history_boundary(
            text,
            text.chars().count() as u32,
            false,
        ));
    }

    #[test]
    fn provider_model_only_is_not_treated_as_configured() {
        let mut config = AgentConfig::default();
        config.providers.set(
            &ProviderKind::OpenAI,
            ProviderConfig {
                model: Some("gpt-4o".to_string()),
                ..ProviderConfig::default()
            },
        );

        assert!(!AgentPanel::provider_is_configured(
            &config,
            &ProviderKind::OpenAI
        ));
    }

    #[test]
    fn provider_api_key_is_treated_as_configured() {
        let mut config = AgentConfig::default();
        config.providers.set(
            &ProviderKind::OpenAI,
            ProviderConfig {
                api_key: Some("sk-test".to_string()),
                ..ProviderConfig::default()
            },
        );

        assert!(AgentPanel::provider_is_configured(
            &config,
            &ProviderKind::OpenAI
        ));
    }

    #[test]
    fn provider_api_key_env_is_treated_as_configured() {
        let mut config = AgentConfig::default();
        config.providers.set(
            &ProviderKind::Anthropic,
            ProviderConfig {
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                ..ProviderConfig::default()
            },
        );

        assert!(AgentPanel::provider_is_configured(
            &config,
            &ProviderKind::Anthropic
        ));
    }

    #[test]
    fn provider_whitespace_api_key_is_not_configured() {
        let mut config = AgentConfig::default();
        config.providers.set(
            &ProviderKind::OpenAI,
            ProviderConfig {
                api_key: Some("   ".to_string()),
                ..ProviderConfig::default()
            },
        );

        assert!(!AgentPanel::provider_is_configured(
            &config,
            &ProviderKind::OpenAI
        ));
    }

    #[test]
    fn openai_compatible_base_url_only_is_treated_as_configured() {
        let mut config = AgentConfig::default();
        config.providers.set(
            &ProviderKind::OpenAICompatible,
            ProviderConfig {
                base_url: Some("http://127.0.0.1:11434/v1".to_string()),
                ..ProviderConfig::default()
            },
        );

        assert!(AgentPanel::provider_is_configured(
            &config,
            &ProviderKind::OpenAICompatible
        ));
    }

    #[test]
    fn configured_provider_list_excludes_model_only_entries() {
        let mut config = AgentConfig::default();
        config.provider = ProviderKind::Anthropic;
        config.providers.set(
            &ProviderKind::OpenAI,
            ProviderConfig {
                model: Some("gpt-4.1".to_string()),
                ..ProviderConfig::default()
            },
        );
        config.providers.set(
            &ProviderKind::Anthropic,
            ProviderConfig {
                api_key: Some("sk-ant-test".to_string()),
                ..ProviderConfig::default()
            },
        );

        let providers = AgentPanel::configured_session_providers(&config);
        assert!(providers.contains(&ProviderKind::Anthropic));
        assert!(!providers.contains(&ProviderKind::OpenAI));
    }

    #[test]
    fn completed_response_collapses_drained_tool_steps() {
        let mut state = PanelState::new();
        state.add_message("user", "search the repo");
        state.add_tool_call("call-1", "search", r#"{"query":"scrollbar"}"#);
        state.complete_tool_call("call-1", "line one\nline two");

        state.complete_response("Done.", Some("test-model"), Some(42));

        let message = state.messages.last().expect("assistant message");
        assert_eq!(message.role, "assistant");
        assert_eq!(message.content, "Done.");
        assert!(message.steps_collapsed);
        assert_eq!(message.steps.len(), 1);
        assert_eq!(message.steps[0].status, StepStatus::Complete);
        assert!(message.steps[0].detail_collapsed);
        assert!(state.tool_calls.is_empty());
    }

    #[test]
    fn response_complete_does_not_leave_unresolved_tool_steps_live() {
        let mut state = PanelState::new();
        state.add_message("user", "read a file");
        state.add_tool_call("call-1", "file_read", r#"{"path":"Cargo.toml"}"#);

        state.complete_response("Done.", Some("test-model"), Some(42));

        let message = state.messages.last().expect("assistant message");
        assert!(message.steps_collapsed);
        assert_eq!(message.steps.len(), 1);
        assert_eq!(message.steps[0].status, StepStatus::Complete);
        assert!(state.tool_calls.is_empty());
    }
}

/// Format a step/tool call duration.
fn format_step_duration(dur: std::time::Duration) -> String {
    if dur.as_secs() >= 60 {
        format!("{}m {}s", dur.as_secs() / 60, dur.as_secs() % 60)
    } else if dur.as_millis() >= 1000 {
        format!("{:.1}s", dur.as_secs_f64())
    } else {
        format!("{}ms", dur.as_millis())
    }
}

/// Format milliseconds into a human-readable duration string.
fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{}m {}s", mins, secs)
    }
}

/// Format a conversation date — "Today", "Yesterday", or "Apr 3" + time.
fn format_conversation_date(summary: &ConversationSummary) -> String {
    let now = Utc::now();
    let today = now.date_naive();
    let date = summary.created_at.date_naive();
    if date == today {
        format!("Today, {}", summary.created_at.format("%H:%M"))
    } else if date == today - chrono::Duration::days(1) {
        format!("Yesterday, {}", summary.created_at.format("%H:%M"))
    } else {
        summary.created_at.format("%b %d, %H:%M").to_string()
    }
}
