use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(target_os = "macos")]
use std::time::Instant;

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme, ElementExt, Sizable, Theme,
    input::{Escape as InputEscape, Input, InputEvent, InputState},
    tooltip::Tooltip,
};
use serde_json::json;
use tokio::sync::oneshot;

const AGENT_PANEL_DEFAULT_WIDTH: f32 = 400.0;
const AGENT_PANEL_MIN_WIDTH: f32 = 200.0;
const TERMINAL_MIN_CONTENT_WIDTH: f32 = 360.0;
const TOP_BAR_COMPACT_HEIGHT: f32 = 28.0;
const TOP_BAR_TABS_HEIGHT: f32 = 36.0;
const CHROME_TRANSITION_SEAM_COVER: f32 = 4.0;
const CHROME_MOTION_SEAM_OVERDRAW: f32 = 6.0;
#[cfg(target_os = "macos")]
const CHROME_SNAP_GUARD_MS: u64 = 160;
#[cfg(target_os = "macos")]
const CHROME_RELEASE_COVER_MS: u64 = 48;
const MAX_SHELL_HISTORY_PER_PANE: usize = 80;
const MAX_GLOBAL_SHELL_HISTORY: usize = 240;
const MAX_GLOBAL_INPUT_HISTORY: usize = 240;
const SPLIT_PREVIEW_SEAM_THICKNESS: f32 = 6.0;
const TAB_DRAG_PREVIEW_WIDTH: f32 = 180.0;
const TAB_DRAG_PREVIEW_HEIGHT: f32 = 28.0;

use crate::activity_bar::{ActivityBar, ActivitySlot, ActivitySlotChanged, ActivityTogglePanel};
use crate::agent_panel::{
    AgentPanel, CancelRequest, DeleteConversation, InlineInputSubmit,
    InlineSkillAutocompleteChanged, LoadConversation, NewConversation, PanelState,
    RerunFromMessage, SelectSessionModel, SelectSessionProvider, SetAutoApprove,
};
use crate::command_palette::{
    CommandPalette, PaletteDismissed, PaletteSelect, ToggleCommandPalette,
};
use crate::editor_view::{ActiveFileChanged, EditorEmptied, EditorView};
use crate::file_tree_view::{FileTreeView, OpenFile};
use crate::input_bar::{
    EscapeInput, InputBar, InputEdited, InputMode, InputScopeChanged, PaneInfo,
    SkillAutocompleteChanged, SubmitInput, TogglePaneScopePicker as TogglePaneScopePickerRequested,
};
use crate::model_registry::ModelRegistry;
use crate::motion::MotionValue;
use crate::pane_tree::{
    PaneTree, SplitDirection, SplitPlacement, SurfaceCreateOptions, SurfaceRenameEditor,
};
use crate::settings_panel::{self, AppearancePreview, SaveSettings, SettingsPanel, ThemePreview};
use crate::sidebar::{
    DraggedTab, DraggedTabOrigin, NewSession, PANEL_MAX_WIDTH, PANEL_MIN_WIDTH, SessionEntry,
    SessionSidebar, SidebarCloseOthers, SidebarCloseTab, SidebarDuplicate, SidebarOpenToolSlot,
    SidebarPaneToTab, SidebarRename, SidebarReorder, SidebarSelect, SidebarSetColor,
    SidebarShowSessions,
};
use crate::sidebar_search_view::SidebarSearchView;
use crate::terminal_pane::{TerminalPane, subscribe_terminal_pane};
use con_terminal::TerminalTheme;

use crate::ghostty_view::{
    GhosttyCwdChanged, GhosttyFocusChanged, GhosttyProcessExited, GhosttySplitRequested,
    GhosttyTitleChanged, GhosttyView,
};
use crate::{
    AddWorkspaceLayoutTabs, AskAi, ClearRestoredTerminalHistory, ClearTerminal, ClosePane,
    CloseSurface, CloseTab, CollapseSidebar, Copy, Cut, CycleInputMode, EditorDeleteBackward,
    EditorDeleteForward, EditorInsertNewline, EditorMoveDown, EditorMoveEnd, EditorMoveHome,
    EditorMoveLeft, EditorMoveLineEnd, EditorMoveLineStart, EditorMoveRight, EditorMoveUp,
    EditorSave, EditorSelectDown, EditorSelectEnd, EditorSelectHome, EditorSelectLeft,
    EditorSelectRight, EditorSelectUp, ExportWorkspaceLayout, FocusFiles, FocusInput, Minimize,
    NewSurface, NewSurfaceSplitDown, NewSurfaceSplitRight, NewTab, NextSurface, NextTab,
    OpenWorkspaceLayoutWindow, Paste, PreviousSurface, PreviousTab, Quit, RenameSurface,
    SearchFiles, SelectAll, SelectTab1, SelectTab2, SelectTab3, SelectTab4, SelectTab5, SelectTab6,
    SelectTab7, SelectTab8, SelectTab9, SplitDown, SplitLeft, SplitRight, SplitUp,
    ToggleAgentPanel, ToggleLeftPanel, TogglePaneScopePicker, TogglePaneZoom, Undo,
};
use con_agent::{
    AgentConfig, Conversation, ProviderKind, TerminalExecRequest, TerminalExecResponse,
};
use con_core::config::{
    AppearanceConfig, Config, TabsOrientation, TerminalConfig, sanitize_terminal_font_family,
};
use con_core::control::{
    AgentAskResult, ControlCommand, ControlError, ControlRequestEnvelope, ControlResult,
    SystemIdentifyResult, TabInfo,
};
use con_core::harness::{
    AgentHarness, AgentSession, HarnessEvent, InputKind, SkillScanJob, SkillScanResult,
};
use con_core::session::{
    AgentModelOverrideState, AgentRoutingState, GlobalHistoryState, PaneLayoutState,
    PaneSplitDirection, Session, TabState,
};
use con_core::workspace_layout::WorkspaceLayout;
use con_core::{
    SuggestionContext, SuggestionEngine, TabIconKind, TabSummary, TabSummaryEngine,
    TabSummaryRequest,
};

mod agent_panel_events;
mod caption;
mod chrome;
mod chrome_actions;
mod control_agent_tools;
mod control_requests;
mod control_surfaces;
mod editor_actions;
mod helpers;
mod input_events;
mod lifecycle;
mod pane_actions;
mod render;
mod session_state;
mod session_worker;
mod sidebar_settings;
mod suggestions;
mod tab_actions;
mod tab_presentation;
mod terminal_factory;
#[cfg(test)]
mod tests;
mod types;
mod window_actions;

use caption::*;
use helpers::*;
use session_worker::*;
use tab_presentation::*;
use terminal_factory::*;
use types::*;

/// The main workspace: tabs + agent panel + input bar + settings overlay
pub struct ConWorkspace {
    config: Config,
    sidebar: Entity<SessionSidebar>,
    tabs: Vec<Tab>,
    active_tab: usize,
    /// True when this workspace is the singleton quick terminal,
    /// which must never be fully closed — closing the last tab
    /// should reinitialize a fresh tab and hide the window instead.
    is_quick_terminal: bool,
    terminal_font_family: String,
    ui_font_family: String,
    ui_font_size: f32,
    font_size: f32,
    terminal_cursor_style: String,
    terminal_opacity: f32,
    terminal_blur: bool,
    ui_opacity: f32,
    tab_accent_inactive_alpha: f32,
    tab_accent_inactive_hover_alpha: f32,
    background_image: Option<String>,
    background_image_opacity: f32,
    background_image_position: String,
    background_image_fit: String,
    background_image_repeat: bool,
    agent_panel: Entity<AgentPanel>,
    input_bar: Entity<InputBar>,
    settings_panel: Entity<SettingsPanel>,
    settings_window: Option<AnyWindowHandle>,
    settings_window_panel: Option<Entity<SettingsPanel>>,
    command_palette: Entity<CommandPalette>,
    model_registry: ModelRegistry,
    harness: AgentHarness,
    shell_suggestion_engine: SuggestionEngine,
    global_shell_history: VecDeque<CommandSuggestionEntry>,
    global_input_history: VecDeque<String>,
    pane_scope_picker_open: bool,
    agent_panel_open: bool,
    agent_panel_motion: MotionValue,
    agent_panel_width: f32,
    tab_strip_motion: MotionValue,
    input_bar_visible: bool,
    input_bar_motion: MotionValue,
    /// Tracks whether a modal was open on the last render, so we can
    /// restore terminal focus when a modal dismisses itself internally.
    modal_was_open: bool,
    ghostty_hidden: bool,
    /// Agent panel drag state: start X position and start width when drag began.
    agent_panel_drag: Option<(f32, f32)>,
    /// Vertical tabs panel drag state: start X position and start width when drag began.
    sidebar_drag: Option<(f32, f32)>,
    /// Current terminal color theme
    terminal_theme: TerminalTheme,
    /// Shared Ghostty app instance for all panes in this window.
    ghostty_app: std::sync::Arc<con_ghostty::GhosttyApp>,
    /// Last wake generation observed from Ghostty's embedded runtime.
    last_ghostty_wake_generation: u64,
    #[cfg(target_os = "macos")]
    chrome_transition_underlay_until: Option<Instant>,
    #[cfg(target_os = "macos")]
    agent_panel_snap_guard_until: Option<Instant>,
    #[cfg(target_os = "macos")]
    input_bar_snap_guard_until: Option<Instant>,
    #[cfg(target_os = "macos")]
    top_chrome_snap_guard_until: Option<Instant>,
    #[cfg(target_os = "macos")]
    agent_panel_release_cover_until: Option<Instant>,
    #[cfg(target_os = "macos")]
    input_bar_release_cover_until: Option<Instant>,
    #[cfg(target_os = "macos")]
    top_chrome_release_cover_until: Option<Instant>,
    #[cfg(target_os = "linux")]
    linux_window_shape_signature: Option<(u32, u32, crate::LinuxWindowShapeRadii)>,
    /// Pending create-pane requests that need a window context to process.
    pending_create_pane_requests: Vec<PendingCreatePane>,
    /// Pending window-aware control requests such as tab lifecycle mutations.
    pending_window_control_requests: Vec<PendingWindowControlRequest>,
    /// Pending surface-control requests that need a window context to allocate a terminal view.
    pending_surface_control_requests: Vec<PendingSurfaceControlRequest>,
    /// Inline editor for the pane-local surface rail.
    surface_rename: Option<SurfaceRenameEditor>,
    /// Control-plane requests from the external `con-cli` socket bridge.
    control_request_rx: crossbeam_channel::Receiver<ControlRequestEnvelope>,
    /// Keeps the Unix socket alive for this workspace instance.
    control_socket: Option<con_core::ControlSocketHandle>,
    /// Pending external agent requests keyed by 0-based tab index.
    pending_control_agent_requests: HashMap<usize, PendingControlAgentRequest>,
    shell_suggestion_rx: crossbeam_channel::Receiver<ShellSuggestionResult>,
    shell_suggestion_tx: crossbeam_channel::Sender<ShellSuggestionResult>,
    /// Background AI engine that produces optional tab labels and icons.
    /// Shares the harness's tokio runtime and the user's
    /// `agent.suggestion_model` settings.
    tab_summary_engine: TabSummaryEngine,
    tab_summary_rx: crossbeam_channel::Receiver<(u64, u64, TabSummary)>,
    tab_summary_tx: crossbeam_channel::Sender<(u64, u64, TabSummary)>,
    skill_scan_rx: crossbeam_channel::Receiver<SkillScanResult>,
    skill_scan_tx: crossbeam_channel::Sender<SkillScanResult>,
    /// Bumped whenever summary-model settings change so late async
    /// responses from the old configuration are ignored.
    tab_summary_generation: u64,
    /// Monotonic counter for [`Tab::summary_id`] — stable across the
    /// window's lifetime so the summary engine's per-tab cache
    /// survives reorders and tab close/reopen.
    next_tab_summary_id: u64,
    /// Monotonic request id for control-plane agent asks so stale timeout tasks cannot
    /// cancel a newer request on the same tab.
    next_control_agent_request_id: u64,
    /// Window handle used to re-enter a window-aware context from deferred control work.
    window_handle: AnyWindowHandle,
    /// Weak self handle for deferred window callbacks.
    workspace_handle: WeakEntity<ConWorkspace>,
    /// Ensures native window-close cleanup only runs once.
    window_close_prepared: bool,
    /// Ordered, coalescing session persistence worker.
    session_save_tx: crossbeam_channel::Sender<SessionSaveRequest>,
    /// Inline rename state for the horizontal tab strip.
    tab_rename: Option<TabRenameEditor>,
    /// Escape-cancel marker so the subsequent input blur does not
    /// auto-save the value we meant to discard.
    tab_rename_cancelled_generation: Option<u64>,
    /// Monotonic generation for horizontal tab rename editors so stale
    /// blur events from an older input cannot commit after a reopen.
    tab_rename_generation: u64,
    /// Drop slot (0..=N) tracked while a DraggedTab is in flight over
    /// the horizontal tab strip. Slot K = "insert before tab K".
    tab_strip_drop_slot: Option<usize>,
    /// Active horizontal tab drag target. Split targets drive live layout
    /// preview and suppress the reorder slot indicator.
    tab_drag_target: Option<TabDragTarget>,
    /// Drag source tab id captured from GPUI's drag preview callback so
    /// drag-move handlers can resolve source/target indices.
    active_dragged_tab_session_id: std::sync::Arc<std::sync::Mutex<Option<u64>>>,
    /// Workspace-owned visible preview for horizontal tab drags. We hide the
    /// GPUI active-drag preview and render this overlay so movement can be
    /// locked to the tab row.
    tab_drag_preview: std::sync::Arc<std::sync::Mutex<Option<TabDragPreviewState>>>,
    /// macOS titlebar drag is initiated explicitly after actual mouse
    /// movement so double-click still reaches the titlebar handler.
    #[cfg(target_os = "macos")]
    top_bar_should_move: bool,
    /// Active pane title drag state — used only for split-preview overlay
    /// rendering while a DraggedTab with origin=Pane is over the pane content.
    pane_title_drag: Option<PaneTitleDragState>,
    /// Last painted bounds for the pane tree content, used to resolve
    /// pane-title drag drop targets in window coordinates.
    pane_content_bounds: std::sync::Arc<std::sync::Mutex<Option<Bounds<Pixels>>>>,
    /// Last painted bounds for each tab in the horizontal strip, used to
    /// resolve pane-title drag slot when cursor is in the tab bar.
    /// Vec index == tab index; each entry is the tab's window-coordinate bounds.
    tab_strip_tab_bounds: std::sync::Arc<std::sync::Mutex<Vec<Bounds<Pixels>>>>,
    /// Bounds of real tabs only (no ghost placeholder), in render order.
    /// Used exclusively by pane-title-drag slot calculation so it never
    /// needs to know where the ghost tab sits in the visual layout.
    pane_title_drag_tab_bounds: std::sync::Arc<std::sync::Mutex<Vec<Bounds<Pixels>>>>,
    /// When true, the per-pane title bar is hidden even in split layouts.
    hide_pane_title_bar: bool,

    /// Workspace-level focus handle — focused when an editor pane is active
    /// so keyboard actions (Cmd+T, Cmd+W, etc.) still reach the workspace.
    workspace_focus: gpui::FocusHandle,
    /// File/search section switcher in the left sidebar.
    activity_bar: Entity<ActivityBar>,
    /// Which file/search section is active.
    activity_slot: ActivitySlot,
    /// Whether the whole left sidebar is visible.
    left_panel_open: bool,
    /// Whether the file/search drawer is visible when vertical tabs are enabled.
    sidebar_tools_open: bool,
    /// File tree view — global singleton, root follows active tab cwd.
    file_tree_view: Entity<FileTreeView>,
    /// Search view — global singleton, root follows the same sidebar root.
    search_view: Entity<SidebarSearchView>,
}
