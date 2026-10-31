//! Left-sidebar support state.
//!
//! This module owns the vertical tab rail/panel inside the product sidebar:
//! shared width, opacity, tab metadata, hover cards, and drag helpers.
//!
//! Visual rules
//! ---
//! - Active row: low-opacity selection fill + foreground text. **No accent
//!   bar.** A single, unambiguous selection cue is enough; tab accent
//!   colors render as the row surface itself at active/inactive alpha.
//! - The close affordance is hover-only on
//!   every row, including the active one. Quiet by default; reveal
//!   on intent.
//! - Surface separation comes from quiet foreground-derived fills, not borders.

use crate::activity_bar::ActivitySlot;
use crate::motion::{MotionValue, SidebarMotion};
use crate::ui_scale::{ui_icon_px, ui_px};
use con_core::terminal_status::Status;
use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point,
    Render, SharedString, Size, Stateful, StatefulInteractiveElement, Styled, WeakEntity, Window,
    anchored, div, point, prelude::*, px, svg,
};
use gpui_component::{
    ActiveTheme, ElementExt, InteractiveElementExt, Sizable,
    button::{Button, ButtonCustomVariant, ButtonVariants},
    input::{Escape as InputEscape, Input, InputEvent, InputState},
    menu::{ContextMenuExt, PopupMenu},
    tooltip::Tooltip,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Width of the always-visible icon rail in collapsed mode.
pub const RAIL_WIDTH: f32 = 44.0;
/// Width of the full panel in pinned mode.
pub const PANEL_WIDTH: f32 = 240.0;
pub const PANEL_MIN_WIDTH: f32 = 184.0;
pub const PANEL_MAX_WIDTH: f32 = 360.0;
/// Compact details next to a rail tile; long paths wrap within this width.
#[allow(dead_code)]
const HOVER_CARD_WIDTH: f32 = 304.0;
/// Single-line tabs keep the expanded navigator as light as the compact rail.
const ROW_HEIGHT: f32 = 32.0;
const ROW_SELECTION_INSET: f32 = 3.0;
const CLOSE_TARGET_SIZE: f32 = 20.0;
const CLOSE_CIRCLE_SIZE: f32 = 14.0;
/// Square session tile size, leaving 2px on each side of the rail.
const RAIL_ICON_SIZE: f32 = RAIL_WIDTH - 4.0;
/// Selection is inset from the hit target; dragging still uses the full tile.
const RAIL_SELECTION_SIZE: f32 = 32.0;
/// Vertical gap between rail icons. Used to compute the icon's
/// y-center for hover-card anchoring.
const RAIL_ICON_GAP: f32 = 2.0;
const NAV_CONTROL_HOVER_ALPHA: f32 = 0.035;
const NAV_CONTROL_PRESS_ALPHA: f32 = 0.055;
const RAIL_BUTTON_SIZE: f32 = if cfg!(target_os = "macos") {
    24.0
} else {
    28.0
};
const RAIL_DIVIDER_HEIGHT: f32 = if cfg!(target_os = "macos") { 3.0 } else { 5.0 };
const RAIL_SESSIONS_GAP: f32 = if cfg!(target_os = "macos") { 8.0 } else { 12.0 };
const RAIL_TOP_CONTROL_COUNT: f32 = 4.0;
const RAIL_TOP_DIVIDER_COUNT: f32 = 2.0;
const RAIL_TOP_GAP_COUNT_BEFORE_SESSIONS: f32 = 7.0;
const RAIL_TOP_CONTROLS_HEIGHT: f32 = RAIL_BUTTON_SIZE * RAIL_TOP_CONTROL_COUNT
    + RAIL_DIVIDER_HEIGHT * RAIL_TOP_DIVIDER_COUNT
    + RAIL_ICON_GAP * RAIL_TOP_GAP_COUNT_BEFORE_SESSIONS
    + RAIL_SESSIONS_GAP;

/// One spring sample owns both the reserved width and the moving chrome.
#[derive(Clone, Copy)]
struct NavigationLayout {
    width: f32,
    row_height: f32,
    row_inset: f32,
    selection_inset: f32,
    selection_radius: f32,
    icon_size: f32,
    icon_center_x: f32,
    list_top: f32,
    label_opacity: f32,
    compact: bool,
    preview: bool,
    controls: [(f32, f32); 4],
}

impl NavigationLayout {
    fn new(panel_width: f32, leading: f32, row_height: f32, icon_size: f32, progress: f32) -> Self {
        let t = progress.clamp(0.0, 1.0);
        let mix = |from: f32, to: f32| from + (to - from) * t;
        let files_y = RAIL_BUTTON_SIZE + RAIL_DIVIDER_HEIGHT + 2.0 * RAIL_ICON_GAP;
        let search_y = files_y + RAIL_BUTTON_SIZE + RAIL_ICON_GAP;
        let new_y = search_y + RAIL_BUTTON_SIZE + RAIL_DIVIDER_HEIGHT + 2.0 * RAIL_ICON_GAP;
        let controls = [
            (8.0, 0.0),
            (40.0, files_y),
            (72.0, search_y),
            (
                if cfg!(target_os = "macos") {
                    104.0
                } else {
                    panel_width - RAIL_BUTTON_SIZE - 8.0
                },
                new_y,
            ),
        ]
        .map(|(expanded_x, compact_y)| {
            let compact_x = if cfg!(target_os = "macos") {
                (RAIL_WIDTH - RAIL_BUTTON_SIZE) * 0.5
            } else {
                8.0
            };
            (mix(compact_x, expanded_x), leading + mix(compact_y, 0.0))
        });
        Self {
            width: mix(RAIL_WIDTH, panel_width),
            row_height: mix(RAIL_ICON_SIZE, row_height),
            row_inset: mix(2.0, 8.0),
            selection_inset: mix(4.0, ROW_SELECTION_INSET),
            selection_radius: mix(
                10.0,
                if cfg!(target_os = "macos") {
                    7.0
                } else {
                    (row_height - 2.0 * ROW_SELECTION_INSET) * 0.5
                },
            ),
            icon_size: mix(18.0, icon_size),
            icon_center_x: mix(RAIL_ICON_SIZE * 0.5, row_height * 0.5),
            list_top: leading
                + mix(
                    RAIL_TOP_CONTROLS_HEIGHT,
                    RAIL_BUTTON_SIZE + RAIL_SESSIONS_GAP,
                ),
            label_opacity: ((t - 0.15) / 0.7).clamp(0.0, 1.0),
            compact: t < 0.5,
            preview: false,
            controls,
        }
    }

    fn preview(panel_width: f32, leading: f32, progress: f32) -> Self {
        // Hover reveals labels horizontally. Every icon, row and control keeps
        // the compact geometry under the pointer, including during reversals.
        let mut layout = Self::new(panel_width, leading, ROW_HEIGHT, 18.0, 0.0);
        layout.width = RAIL_WIDTH + (panel_width - RAIL_WIDTH) * progress.clamp(0.0, 1.0);
        layout.label_opacity = ((progress - 0.15) / 0.7).clamp(0.0, 1.0);
        layout.preview = true;
        layout
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramStatusDetail {
    pub content: con_core::program_status::ProgramDetail,
    pub source_path: Option<String>,
}

/// A retained, user-facing summary of one physical pane, never a raw command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneHoverDetail {
    pub id: u64,
    pub icon: &'static str,
    pub title: String,
    pub directory: Option<String>,
    pub status: Option<Status>,
    pub status_label: String,
}

/// One row in the tab sidebar panel.
#[derive(Clone)]
pub struct SessionEntry {
    pub id: u64,
    pub name: String,
    pub subtitle: Option<String>,
    pub is_ssh: bool,
    pub needs_attention: bool,
    pub status: Option<Status>,
    /// Structured presentation for the program record that owns `status`.
    /// Empty when the indicator comes from a heuristic.
    pub status_detail: Option<ProgramStatusDetail>,
    pub icon: &'static str,
    /// Retained, deduplicated identities of the active content in each pane.
    pub pane_icons: Vec<&'static str>,
    pub pane_details: Vec<PaneHoverDetail>,
    pub native_session_hint: Option<String>,
    pub has_user_label: bool,
    /// How many panes the tab contains (split count). Surfaced in
    /// the rail's hover card so the user can see "this tab has 3
    /// panes" without expanding.
    pub pane_count: usize,
    /// Optional accent color set via the tab context menu.
    pub color: Option<con_core::session::TabAccentColor>,
}

impl SessionEntry {
    pub(crate) fn identity_icons(&self) -> &[&'static str] {
        if self.pane_icons.is_empty() {
            std::slice::from_ref(&self.icon)
        } else {
            &self.pane_icons
        }
    }
}

/// Visual state of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelMode {
    Collapsed,
    Pinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailModeButtonAction {
    ShowSessionsFromToolPanel,
    RestorePinnedPanelFromRail,
    ExpandCollapsedPanelFromOverride,
    TogglePinned,
}

/// Per-tab transient state — the inline-rename input.
struct RenameState {
    index: usize,
    session_id: u64,
    generation: u64,
    input: Entity<InputState>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenameLifecycleStateSnapshot {
    active_generation: Option<u64>,
    cancelled_generation: Option<u64>,
}

/// What the user is currently dragging.
#[derive(Clone)]
pub struct DraggedTab {
    pub session_id: u64,
    pub label: SharedString,
    pub icon: &'static str,
    pub origin: DraggedTabOrigin,
    pub preview_constraint: Option<DraggedTabPreviewConstraint>,
    /// Set when origin is `Pane` — the pane to promote to a new tab on drop.
    pub pane_id: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DraggedTabPreviewConstraint {
    pub cursor_offset_y: Pixels,
    pub top: Pixels,
    pub height: Pixels,
    pub preview_height: Pixels,
    /// X offset of the cursor inside the source tab element at drag start.
    /// GPUI places the drag preview root at `current_mouse - cursor_offset`,
    /// so this is enough to compute the preview's natural left edge.
    pub cursor_offset_x: Pixels,
    /// Left edge of the tab bar in window coordinates.
    pub left: Pixels,
    /// Width of the tab bar (used to clamp the right edge).
    pub bar_width: Pixels,
    /// Width of the drag preview element.
    pub preview_width: Pixels,
}

#[cfg(test)]
pub fn constrained_drag_preview_y_shift(
    mouse_y: Pixels,
    constraint: DraggedTabPreviewConstraint,
) -> Pixels {
    let root_top = mouse_y - constraint.cursor_offset_y;
    let max_top = constraint.top + (constraint.height - constraint.preview_height).max(px(0.0));
    let desired_top = root_top.clamp(constraint.top, max_top);
    desired_top - root_top
}

/// Compute the X shift to apply to the drag preview so it stays inside
/// the horizontal tab bar.
///
/// GPUI places the preview root at `current_mouse - cursor_offset`,
/// where `cursor_offset` is the cursor's position inside the source element
/// when drag starts. Therefore the preview's natural left edge is exactly
/// `mouse_x - cursor_offset_x`; clamping that value and subtracting the
/// natural left gives the relative `.left()` shift to apply.
#[cfg(test)]
pub fn constrained_drag_preview_x_shift(
    mouse_x: Pixels,
    constraint: DraggedTabPreviewConstraint,
) -> Pixels {
    let natural_left = mouse_x - constraint.cursor_offset_x;
    let max_left = constraint.left + (constraint.bar_width - constraint.preview_width).max(px(0.0));
    let desired_left = natural_left.clamp(constraint.left, max_left);
    desired_left - natural_left
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraggedTabOrigin {
    HorizontalTabStrip,
    Sidebar,
    /// Dragged from a pane title bar — pane_id is in DraggedTab.pane_id.
    Pane,
}

impl Render for DraggedTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Pane-origin and horizontal-tab-strip drags render their visible
        // previews from Workspace-owned overlays. GPUI's built-in active-drag
        // preview cannot be reliably axis-constrained with relative style
        // offsets, so hide it for these origins.
        if matches!(
            self.origin,
            DraggedTabOrigin::Pane
                | DraggedTabOrigin::HorizontalTabStrip
                | DraggedTabOrigin::Sidebar
        ) {
            return div().size(px(0.0));
        }

        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .w(px(120.0))
            .h(px(28.0))
            .px(px(8.0))
            .rounded(px(4.0))
            .bg(theme.title_bar.opacity(0.92))
            .text_color(theme.foreground)
            .text_size(px(12.0))
            .font_family(theme.font_family.clone())
            .child(
                svg()
                    .path(self.icon)
                    .size(ui_icon_px(theme, 12.0))
                    .flex_shrink_0()
                    .text_color(theme.foreground),
            )
            .child(div().truncate().child(self.label.clone()))
    }
}

/// Tab sidebar side panel.
pub struct SessionSidebar {
    mode: PanelMode,
    sessions: Vec<SessionEntry>,
    activity_layer: Option<Entity<crate::tab_activity::TabActivity>>,
    pub(crate) notifications: crate::tab_activity::TabNotifications,
    active_session: usize,
    leading_top_pad: f32,
    /// Adjacent terminal gutter, supplied by the workspace. Compact navigation
    /// is centered between the window edge and the terminal card in both modes.
    terminal_gutter: f32,
    peek_open: bool,
    peek_hovered: bool,
    peek_motion: SidebarMotion,
    peek_task: Option<gpui::Task<()>>,
    peek_menu_open: bool,
    peek_menu_subscription: Option<gpui::Subscription>,
    rail_scroll: gpui::ScrollHandle,
    panel_scroll: gpui::ScrollHandle,
    tools_motion: SidebarMotion,
    tool_slot_motion: MotionValue,
    tool_slot_initialized: bool,
    /// Smooth width animation between rail (0.0) and pinned (1.0).
    width_motion: SidebarMotion,
    /// Pinning a wide hover preview still starts with compact icon geometry.
    /// Its layout must move independently of the already revealed width.
    peek_pin_layout_motion: Option<SidebarMotion>,
    /// User-resized width of the pinned panel. Collapsed mode ignores
    /// this and stays at the fixed rail width.
    panel_width: f32,
    /// Render-time maximum supplied by the workspace after reserving
    /// terminal and agent-panel width. This keeps the sidebar's actual
    /// painted edge aligned with workspace seam and resize-handle math.
    effective_panel_max_width: f32,
    /// Inline rename state, `Some` while the user is editing a label.
    rename: Option<RenameState>,
    /// Escape-cancel marker so a subsequent blur from the old input
    /// does not commit the just-cancelled rename.
    rename_cancelled_generation: Option<u64>,
    /// Monotonic generation for sidebar rename editors so stale blur
    /// events from an older input cannot commit after a reopen.
    rename_generation: u64,
    /// The rail-icon index currently under the cursor. Drives the
    /// floating hover card. Cleared on rail mouse-leave.
    hovered_rail: Option<usize>,
    /// Drop slot (0..=N) the user is currently hovering while a
    /// `DraggedTab` is in flight. `0` = before the first row,
    /// `N` = after the last row. The indicator line appears above
    /// row `slot` for slot < N, and below row N-1 for slot == N.
    /// Without that "after-last" affordance there's no way to drag
    /// a tab to the bottom of the list.
    drop_slot: Option<usize>,
    /// Last rail bounds origin observed during a drag. GPUI's rail
    /// `on_drop` gives us the window mouse position but not the
    /// element bounds, so we cache this from `on_drag_move`.
    rail_drag_origin_y: Option<f32>,
    /// Last painted bounds for visible tab sidebar targets (rail icons or panel rows).
    tab_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    /// Workspace-independent visible preview for tab sidebar drags.
    drag_preview: Rc<RefCell<Option<SidebarDragPreviewState>>>,
    /// Independent opacity for selection surfaces and floating cards.
    ui_opacity: f32,
    /// True while the workspace is showing Files/Search beside the
    /// vertical tabs. The sidebar then becomes the single icon rail so
    /// tools and sessions share one left-navigation surface.
    tools_panel_open: bool,
    /// Render-only compact rail override used when the workspace reveals
    /// hidden vertical tabs after explicit tab creation. This must not mutate
    /// `mode`: pinned/collapsed is a user preference persisted in sessions.
    rail_only_override: bool,
    active_tool_slot: ActivitySlot,
    tab_accent_inactive_alpha: f32,
    tab_accent_inactive_hover_alpha: f32,
}

pub struct SidebarSelect {
    pub index: usize,
}
pub struct NewSession;
pub struct SidebarCloseTab {
    pub session_id: u64,
}
pub struct SidebarRename {
    pub session_id: u64,
    /// `None` clears the user override and falls back to smart naming.
    pub label: Option<String>,
    /// False when rename ended without a user edit. This lets the
    /// workspace restore terminal focus without freezing a smart label
    /// into an explicit user override.
    pub changed_by_user: bool,
}
pub struct SidebarDuplicate {
    pub session_id: u64,
}
pub struct SidebarReorder {
    pub session_id: u64,
    pub to: usize,
    /// Context-menu move actions are relative to the current tab
    /// position. Drag/drop emits an absolute destination slot.
    pub move_delta: Option<isize>,
}

pub struct SidebarPaneToTab {
    pub pane_id: usize,
    pub to: usize,
}
pub struct SidebarCloseOthers {
    pub session_id: u64,
}

pub struct SidebarSetColor {
    pub session_id: u64,
    pub color: Option<con_core::session::TabAccentColor>,
}

pub struct SidebarOpenToolSlot {
    pub slot: ActivitySlot,
}

pub struct SidebarShowSessions;

impl EventEmitter<SidebarSelect> for SessionSidebar {}
impl EventEmitter<NewSession> for SessionSidebar {}
impl EventEmitter<SidebarCloseTab> for SessionSidebar {}
impl EventEmitter<SidebarRename> for SessionSidebar {}
impl EventEmitter<SidebarDuplicate> for SessionSidebar {}
impl EventEmitter<SidebarReorder> for SessionSidebar {}
impl EventEmitter<SidebarPaneToTab> for SessionSidebar {}
impl EventEmitter<SidebarCloseOthers> for SessionSidebar {}
impl EventEmitter<SidebarSetColor> for SessionSidebar {}
impl EventEmitter<SidebarOpenToolSlot> for SessionSidebar {}
impl EventEmitter<SidebarShowSessions> for SessionSidebar {}

impl SessionSidebar {
    pub(crate) fn first_control_center_y(&self) -> f32 {
        self.leading_top_pad + RAIL_BUTTON_SIZE * 0.5
    }

    pub(crate) fn set_terminal_gutter(&mut self, gutter: f32) -> bool {
        let gutter = gutter.max(0.0);
        let changed = (self.terminal_gutter - gutter).abs() > f32::EPSILON;
        self.terminal_gutter = gutter;
        changed
    }

    fn navigation_content_offset(&self) -> f32 {
        self.terminal_gutter * 0.5 * (1.0 - self.tools_progress())
    }

    fn can_peek(&self) -> bool {
        self.renders_rail_only() && !self.tools_visible() && self.rename.is_none()
    }

    fn peek_active(&self) -> bool {
        self.peek_open || self.peek_motion.is_animating() || self.peek_motion.current() > 0.0
    }

    fn set_navigation_hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.peek_hovered = hovered;
        self.peek_task = None;
        if !self.can_peek()
            || self.peek_menu_open
            || cx.has_active_drag()
            || hovered == self.peek_open
        {
            return;
        }
        self.peek_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(if hovered { 180 } else { 160 }))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.can_peek()
                    && !this.peek_menu_open
                    && this.peek_hovered == hovered
                    && !cx.has_active_drag()
                {
                    this.peek_open = hovered;
                    this.peek_motion.set_target(if hovered { 1.0 } else { 0.0 });
                    this.hovered_rail = None;
                    cx.notify();
                }
            });
        }));
    }

    fn dismiss_navigation_peek(&mut self) {
        self.peek_task = None;
        self.peek_open = false;
        self.peek_hovered = false;
        self.peek_motion = SidebarMotion::new(0.0);
        self.peek_menu_open = false;
        self.peek_menu_subscription = None;
    }

    fn hold_navigation_peek_for_menu(&mut self, menu: Entity<PopupMenu>, cx: &mut Context<Self>) {
        if !self.peek_active() {
            return;
        }
        self.peek_task = None;
        self.peek_menu_open = true;
        self.peek_menu_subscription =
            Some(cx.subscribe(&menu, |this, _, _: &gpui::DismissEvent, cx| {
                this.peek_menu_open = false;
                this.peek_menu_subscription = None;
                this.set_navigation_hover(this.peek_hovered, cx);
            }));
    }

    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self::default_state()
    }

    fn default_state() -> Self {
        Self {
            mode: PanelMode::Collapsed,
            sessions: Vec::new(),
            activity_layer: None,
            notifications: Default::default(),
            active_session: 0,
            // The workspace top bar already reserves the macOS
            // traffic-light/titlebar area before the sidebar is
            // rendered. Keep only a small visual inset here; a
            // platform titlebar inset creates a dead band above the
            // tab-sidebar controls.
            leading_top_pad: 6.0,
            terminal_gutter: 0.0,
            peek_open: false,
            peek_hovered: false,
            peek_motion: SidebarMotion::new(0.0),
            peek_task: None,
            peek_menu_open: false,
            peek_menu_subscription: None,
            rail_scroll: gpui::ScrollHandle::new(),
            panel_scroll: gpui::ScrollHandle::new(),
            tools_motion: SidebarMotion::new(0.0),
            tool_slot_motion: MotionValue::new(0.0),
            tool_slot_initialized: false,
            width_motion: SidebarMotion::new(0.0),
            peek_pin_layout_motion: None,
            panel_width: PANEL_WIDTH,
            effective_panel_max_width: PANEL_MAX_WIDTH,
            rename: None,
            rename_cancelled_generation: None,
            rename_generation: 0,
            hovered_rail: None,
            drop_slot: None,
            rail_drag_origin_y: None,
            tab_bounds: Rc::new(RefCell::new(Vec::new())),
            drag_preview: Rc::new(RefCell::new(None)),
            ui_opacity: 0.92,
            tools_panel_open: false,
            rail_only_override: false,
            active_tool_slot: ActivitySlot::Files,
            tab_accent_inactive_alpha: crate::tab_colors::TAB_ACCENT_INACTIVE_ALPHA,
            tab_accent_inactive_hover_alpha: crate::tab_colors::TAB_ACCENT_INACTIVE_HOVER_ALPHA,
        }
    }

    #[cfg(test)]
    fn new_for_test() -> Self {
        Self::default_state()
    }

    pub fn set_ui_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let opacity = opacity.clamp(0.55, 0.98);
        if (self.ui_opacity - opacity).abs() > f32::EPSILON {
            self.ui_opacity = opacity;
            cx.notify();
        }
    }

    pub fn set_tab_accent_alphas(
        &mut self,
        inactive_alpha: f32,
        inactive_hover_alpha: f32,
        cx: &mut Context<Self>,
    ) {
        let inactive_alpha = sanitize_tab_accent_alpha(inactive_alpha);
        let inactive_hover_alpha =
            sanitize_tab_accent_alpha(inactive_hover_alpha).max(inactive_alpha);
        if (self.tab_accent_inactive_alpha - inactive_alpha).abs() > f32::EPSILON
            || (self.tab_accent_inactive_hover_alpha - inactive_hover_alpha).abs() > f32::EPSILON
        {
            self.tab_accent_inactive_alpha = inactive_alpha;
            self.tab_accent_inactive_hover_alpha = inactive_hover_alpha;
            cx.notify();
        }
    }

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        let preview_progress = (pinned && self.peek_active()).then(|| self.peek_motion.current());
        self.dismiss_navigation_peek();
        let had_rail_only_override = self.rail_only_override;
        self.rail_only_override = false;
        let new_mode = if pinned {
            PanelMode::Pinned
        } else {
            PanelMode::Collapsed
        };
        if self.mode != new_mode || had_rail_only_override {
            self.mode = new_mode;
            self.cancel_rename(cx);
            self.hovered_rail = None;
            let target = if pinned && !self.tools_panel_open {
                1.0
            } else {
                0.0
            };
            if let Some(progress) = preview_progress {
                self.width_motion = SidebarMotion::new(progress);
            } else if pinned && had_rail_only_override {
                self.width_motion = SidebarMotion::new(0.0);
            }
            if pinned && (preview_progress.is_some() || had_rail_only_override) {
                self.peek_pin_layout_motion = Some(SidebarMotion::new(0.0));
            }
            if let Some(motion) = self.peek_pin_layout_motion.as_mut() {
                motion.set_target(target);
            }
            self.width_motion.set_target(target);
            cx.notify();
        }
    }

    pub fn is_pinned(&self) -> bool {
        matches!(self.mode, PanelMode::Pinned)
    }

    #[cfg(target_os = "macos")]
    pub fn caption_dock_progress(&self, visibility: f32, minimum_width: f32) -> f32 {
        // Files/Search render a rail plus a tool column whose combined width
        // is panel_width, even though the tab rail itself is compact.
        let target_width = self.effective_panel_width();
        if (!self.tools_visible() && self.rail_only_override) || target_width <= minimum_width {
            return 0.0;
        }
        let rendered_width = self.rendered_panel_width();
        ((rendered_width * visibility - minimum_width) / (target_width - minimum_width))
            .clamp(0.0, 1.0)
    }

    pub fn has_resizable_content(&self) -> bool {
        self.tools_panel_open || (self.is_pinned() && !self.rail_only_override)
    }

    pub fn panel_width(&self) -> f32 {
        self.panel_width
    }

    pub fn rendered_width(&self) -> f32 {
        if self.rail_only_override {
            return RAIL_WIDTH;
        }
        RAIL_WIDTH + (self.effective_panel_width() - RAIL_WIDTH) * self.width_motion.current()
    }

    pub(crate) fn tools_progress(&self) -> f32 {
        self.tools_motion.current()
    }

    pub(crate) fn tools_visible(&self) -> bool {
        self.tools_panel_open || self.tools_progress() > 0.0
    }

    pub(crate) fn rendered_tools_width(&self) -> f32 {
        (self.effective_panel_width() - RAIL_WIDTH) * self.tools_progress()
    }

    pub(crate) fn rendered_panel_width(&self) -> f32 {
        self.rendered_width() + self.rendered_tools_width()
    }

    pub(crate) fn tool_slot_blend(
        &mut self,
        slot: ActivitySlot,
        animate: bool,
        window: &mut Window,
        cx: &App,
    ) -> f32 {
        self.target_tool_slot_at(slot, animate, std::time::Instant::now());
        self.tool_slot_motion.value(window, cx)
    }

    fn target_tool_slot_at(&mut self, slot: ActivitySlot, animate: bool, now: std::time::Instant) {
        let target = if slot == ActivitySlot::Search {
            1.0
        } else {
            0.0
        };
        if animate && self.tool_slot_initialized {
            self.tool_slot_motion
                .set_target_at(target, Duration::from_millis(140), now);
        } else {
            self.tool_slot_motion = MotionValue::new(target);
        }
        self.tool_slot_initialized = true;
    }

    pub fn advance_width_motion(&mut self, window: &mut Window, cx: &App) -> bool {
        self.width_motion.value(window, cx);
        self.tools_motion.value(window, cx);
        let layout_animating = self.peek_pin_layout_motion.as_mut().is_some_and(|motion| {
            motion.value(window, cx);
            motion.is_animating()
        });
        self.width_motion.is_animating() || self.tools_motion.is_animating() || layout_animating
    }

    pub fn set_tools_panel_open(&mut self, open: bool, active_slot: ActivitySlot) -> bool {
        self.set_tools_panel_open_at(open, active_slot, std::time::Instant::now())
    }

    fn set_tools_panel_open_at(
        &mut self,
        open: bool,
        active_slot: ActivitySlot,
        now: std::time::Instant,
    ) -> bool {
        let changed = self.tools_panel_open != open || self.active_tool_slot != active_slot;
        if self.tools_panel_open != open {
            self.tools_panel_open = open;
            self.hovered_rail = None;
            if open {
                self.dismiss_navigation_peek();
                if self.rail_only_override {
                    self.width_motion = SidebarMotion::new(0.0);
                    self.peek_pin_layout_motion = None;
                }
                self.rail_only_override = false;
            }
        }
        // Complementary channels share one timestamp so a pinned navigator
        // swaps with a tool column without pulsing the combined width.
        let navigation = if self.is_pinned() && !open { 1.0 } else { 0.0 };
        self.tools_motion
            .retarget_at(if open { 1.0 } else { 0.0 }, now);
        self.width_motion.retarget_at(navigation, now);
        if let Some(motion) = self.peek_pin_layout_motion.as_mut() {
            motion.retarget_at(navigation, now);
        }
        self.active_tool_slot = active_slot;
        changed
    }

    /// Clear lingering rail hover state. Called by the workspace when the
    /// rail leaves the layout (panel hidden / vertical tabs off) — an
    /// unmounted rail cannot deliver the hover-leave event, so without this
    /// the hover card could persist or reappear once the rail is restored.
    pub fn clear_hovered_rail(&mut self, cx: &mut Context<Self>) {
        let changed = self.hovered_rail.is_some() || self.peek_active() || self.peek_task.is_some();
        self.hovered_rail = None;
        self.dismiss_navigation_peek();
        if changed {
            cx.notify();
        }
    }

    pub fn show_rail_only_preserving_pinned(&mut self, cx: &mut Context<Self>) {
        if matches!(self.mode, PanelMode::Collapsed) {
            self.hovered_rail = None;
            return;
        }
        if self.rail_only_override {
            return;
        }
        self.rail_only_override = true;
        self.hovered_rail = None;
        cx.notify();
    }

    pub fn clear_rail_only_override(&mut self, cx: &mut Context<Self>) {
        if !self.rail_only_override {
            return;
        }
        self.rail_only_override = false;
        self.hovered_rail = None;
        cx.notify();
    }

    pub fn set_panel_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = width.max(PANEL_MIN_WIDTH);
        if (self.panel_width - width).abs() > f32::EPSILON {
            self.panel_width = width;
            cx.notify();
        }
    }

    pub fn set_effective_panel_max_width(&mut self, width: f32) {
        self.effective_panel_max_width = width.max(PANEL_MIN_WIDTH);
    }

    #[allow(dead_code)]
    pub fn update_drag_preview_from_mouse(
        &mut self,
        mouse: Point<Pixels>,
        viewport_size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = self.drag_preview.borrow().clone() else {
            return;
        };
        let preview_size = tab_drag_preview_size(self.is_pinned());
        let max_top = (viewport_size.height - preview_size.height).max(px(0.0));
        let probe =
            vertical_drag_overlay_probe_position(mouse, &preview, preview_size, px(0.0), max_top);
        let bounds = self.tab_bounds.borrow().clone();
        if let Some(slot) = vertical_slot_from_bounds(probe.y, &bounds, self.sessions.len())
            && self.drop_slot != Some(slot)
        {
            self.drop_slot = Some(slot);
            self.hovered_rail = None;
            cx.notify();
        }
    }

    fn ensure_drag_preview(
        &mut self,
        dragged: &DraggedTab,
        source_left: Pixels,
        cursor_offset_y: Pixels,
    ) {
        if self.drag_preview.borrow().is_some() {
            return;
        }
        *self.drag_preview.borrow_mut() = Some(SidebarDragPreviewState {
            label: dragged.label.clone(),
            icon: dragged.icon,
            source_left,
            cursor_offset_y,
            is_pane_origin: dragged.origin == DraggedTabOrigin::Pane,
        });
    }

    fn clear_drag_preview(&mut self) {
        *self.drag_preview.borrow_mut() = None;
    }

    #[allow(dead_code)]
    pub fn drag_preview_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let preview = self.drag_preview.borrow().clone()?;
        // Only render the sidebar overlay when the cursor is actually inside
        // the sidebar (drop_slot is Some). If the pane is being dragged but
        // the cursor is outside the sidebar, the workspace pane floating
        // preview should be the only visible preview.
        self.drop_slot?;
        // For pane-origin drags, the workspace floating preview already follows
        // the cursor. The sidebar only needs to show the drop indicator line —
        // no extra overlay here.
        if preview.is_pane_origin {
            return None;
        }
        let theme = cx.theme();
        let preview_size = tab_drag_preview_size(self.is_pinned());
        let max_top = (window.viewport_size().height - preview_size.height).max(px(0.0));
        let origin = vertical_drag_overlay_origin(
            window.mouse_position(),
            &preview,
            preview_size,
            px(0.0),
            max_top,
        );
        Some(
            div()
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(preview_size.width)
                .h(preview_size.height)
                .px(px(10.0))
                .rounded(px(preview_size.height.as_f32() * 0.5))
                .flex()
                .items_center()
                .gap(px(6.0))
                .cursor_grab()
                .bg(theme.title_bar.opacity(0.92))
                .font_family(theme.font_family.clone())
                .text_size(px(12.0))
                .text_color(theme.foreground)
                .child(crate::assets::tab_identity_icon(
                    sidebar_tab_icon(preview.icon),
                    ui_icon_px(theme, 18.0),
                    theme.foreground,
                ))
                .child(div().flex_1().min_w_0().truncate().child(preview.label))
                .into_any_element(),
        )
    }

    pub fn clamped_panel_width(width: f32, effective_max_width: f32) -> f32 {
        width.clamp(PANEL_MIN_WIDTH, effective_max_width.max(PANEL_MIN_WIDTH))
    }

    pub(crate) fn effective_panel_width(&self) -> f32 {
        Self::clamped_panel_width(self.panel_width, self.effective_panel_max_width)
    }

    fn renders_rail_only(&self) -> bool {
        self.tools_panel_open
            || self.rail_only_override
            || matches!(self.mode, PanelMode::Collapsed)
    }

    fn rail_mode_button_action(&self) -> RailModeButtonAction {
        if self.tools_panel_open {
            return RailModeButtonAction::ShowSessionsFromToolPanel;
        }
        if self.rail_only_override {
            return if self.is_pinned() {
                RailModeButtonAction::RestorePinnedPanelFromRail
            } else {
                RailModeButtonAction::ExpandCollapsedPanelFromOverride
            };
        }
        RailModeButtonAction::TogglePinned
    }

    pub fn toggle_pinned(&mut self, cx: &mut Context<Self>) {
        let now_pinned = !self.is_pinned();
        self.set_pinned(now_pinned, cx);
    }

    /// Retained metadata also used by the horizontal tab strip.
    pub(crate) fn session(&self, index: usize) -> Option<&SessionEntry> {
        self.sessions.get(index)
    }

    /// Update the session list from workspace state.
    pub fn sync_sessions(
        &mut self,
        sessions: Vec<SessionEntry>,
        active: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = &self.rename
            && (state.index >= sessions.len() || sessions[state.index].id != state.session_id)
        {
            self.rename = None;
        }
        self.notifications
            .retain(|id| sessions.iter().any(|session| session.id == *id));
        self.sessions = sessions;
        self.active_session = active;
        cx.notify();
    }

    fn begin_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.sessions.len() {
            return;
        }
        let session_id = self.sessions[index].id;
        let generation = self.rename_generation;
        self.rename_generation += 1;
        let initial = self.sessions[index].name.clone();
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(&initial, window, cx);
            s.set_placeholder("Tab name", window, cx);
            s
        });

        let select_all_on_focus = Rc::new(Cell::new(true));
        let changed_by_user = Rc::new(Cell::new(false));
        cx.subscribe_in(&input, window, {
            let select_all_on_focus = select_all_on_focus.clone();
            let changed_by_user = changed_by_user.clone();
            move |this, input_entity, event: &InputEvent, window, cx| match event {
                InputEvent::Focus if select_all_on_focus.replace(false) => {
                    window.dispatch_action(Box::new(gpui_component::input::SelectAll), cx);
                }
                InputEvent::Change => {
                    changed_by_user.set(true);
                }
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    if this.rename_cancelled_generation == Some(generation)
                        || this.rename.as_ref().map(|rename| rename.generation) != Some(generation)
                    {
                        if this.rename_cancelled_generation == Some(generation) {
                            this.rename_cancelled_generation = None;
                        }
                        return;
                    }
                    let value = input_entity.read(cx).value().to_string();
                    let label = normalize_sidebar_rename_label(&value);
                    cx.emit(SidebarRename {
                        session_id,
                        label,
                        changed_by_user: changed_by_user.get(),
                    });
                    this.rename = None;
                    this.rename_cancelled_generation = None;
                    cx.notify();
                }
                _ => {}
            }
        })
        .detach();

        input.update(cx, |state, cx| state.focus(window, cx));
        self.rename = Some(RenameState {
            index,
            session_id,
            generation,
            input,
        });
        cx.notify();
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(rename) = self.rename.take() {
            self.rename_cancelled_generation = Some(rename.generation);
            cx.notify();
        }
    }

    pub fn start_rename_by_id(
        &mut self,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.mode, PanelMode::Collapsed) {
            self.set_pinned(true, cx);
        }
        cx.defer_in(window, move |this, window, cx| {
            let Some(index) = this
                .sessions
                .iter()
                .position(|session| session.id == session_id)
            else {
                return;
            };
            this.begin_rename(index, window, cx);
        });
    }

    #[allow(dead_code)]
    fn render_rail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let session_count = self.sessions.len();
        let mode_icon = "phosphor/list.svg";
        let mode_color = if self.tools_panel_open || (self.is_pinned() && !self.rail_only_override)
        {
            theme.foreground
        } else {
            theme.muted_foreground
        };
        let files_color = if self.tools_panel_open && self.active_tool_slot == ActivitySlot::Files {
            theme.primary
        } else {
            theme.muted_foreground
        };
        let search_color = if self.tools_panel_open && self.active_tool_slot == ActivitySlot::Search
        {
            theme.primary
        } else {
            theme.muted_foreground
        };
        // The expanded list owns tab navigation and drag bounds. Keep the
        // rail's session tiles only when that list is not visible.
        let show_session_tiles = self.renders_rail_only();
        if show_session_tiles {
            self.tab_bounds.borrow_mut().clear();
        }
        let rail = div()
            .id("tab-sidebar-rail")
            .relative()
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(self.leading_top_pad))
            .pb(px(8.0))
            .gap(px(RAIL_ICON_GAP))
            // The workspace paints the terminal-matched sidebar base once.
            // Mouse-leave on the rail container clears the hover card
            // even when the cursor exits via a fast diagonal motion
            // that may skip the per-icon hover transitions.
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.set_navigation_hover(*hovered, cx);
                if !*hovered && this.hovered_rail.take().is_some() {
                    cx.notify();
                }
            }))
            // Container-level drag-move so the drop-target indicator
            // tracks the cursor even when it drifts off the 32px
            // pill into the 2-px gap between pills (or just brushes
            // the pill's rounded corner). The per-pill on_drag_move
            // handlers below give the same feedback while the cursor
            // is squarely on a pill; this fallback covers the gaps.
            .on_drag_move::<DraggedTab>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                    if !show_session_tiles {
                        return;
                    }
                    if !matches!(
                        event.drag(cx).origin,
                        DraggedTabOrigin::Sidebar | DraggedTabOrigin::Pane
                    ) {
                        return;
                    }
                    this.rail_drag_origin_y = Some(f32::from(event.bounds.origin.y));
                    let is_pane = event.drag(cx).origin == DraggedTabOrigin::Pane;
                    // For pane-origin drags, only proceed if the cursor is
                    // actually inside the container bounds. GPUI bubbles
                    // drag_move to parent containers even when the cursor is
                    // outside them, so without this check the sidebar would
                    // show indicators while the pane is still on the right.
                    if !point_in_bounds(&event.event.position, &event.bounds) {
                        if this.drop_slot.is_some() || this.drag_preview.borrow().is_some() {
                            this.drop_slot = None;
                            this.clear_drag_preview();
                            cx.notify();
                        }
                        return;
                    }
                    let slot = if !is_pane {
                        let bounds = this.tab_bounds.borrow().clone();
                        vertical_slot_from_bounds(event.event.position.y, &bounds, session_count)
                            .or_else(|| {
                                rail_slot_for_cursor(event, session_count, this.leading_top_pad)
                            })
                    } else {
                        let bounds = this.tab_bounds.borrow().clone();
                        vertical_slot_from_bounds(event.event.position.y, &bounds, session_count)
                    };
                    if let Some(slot) = slot {
                        // Cursor is over a tab item — create/keep preview.
                        let preview_size = tab_drag_preview_size(this.is_pinned());
                        this.ensure_drag_preview(
                            event.drag(cx),
                            event.bounds.origin.x,
                            preview_size.height / 2.0,
                        );
                        if this.drop_slot != Some(slot) {
                            this.drop_slot = Some(slot);
                            this.hovered_rail = None;
                            cx.notify();
                        }
                    } else if is_pane {
                        // Pane drag but cursor not over any tab item — clear
                        // preview and drop slot so the sidebar overlay hides.
                        if this.drop_slot.is_some() || this.drag_preview.borrow().is_some() {
                            this.drop_slot = None;
                            this.clear_drag_preview();
                            cx.notify();
                        }
                    }
                },
            ))
            // Container-level on_drop fires when the cursor is
            // anywhere inside the rail's bounds at mouseup —
            // including the gaps between pills. Without this the
            // user has to land the cursor precisely on a 32×32 pill
            // to reorder, which is unforgiving on a 44-px rail.
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                if !show_session_tiles {
                    return;
                }
                if !matches!(
                    dragged.origin,
                    DraggedTabOrigin::Sidebar | DraggedTabOrigin::Pane
                ) {
                    return;
                }
                // For pane-origin drags, only handle the drop if the cursor
                // was actually inside the sidebar (drop_slot is Some). If the
                // user dropped on the pane content area, the event bubbles here
                // but we must not steal it.
                if dragged.origin == DraggedTabOrigin::Pane && this.drop_slot.is_none() {
                    this.clear_drag_preview();
                    return;
                }
                let to = this.drop_slot.or_else(|| {
                    let bounds = this.tab_bounds.borrow().clone();
                    let probe_y = if dragged.origin == DraggedTabOrigin::Sidebar {
                        window.mouse_position().y
                    } else {
                        event_probe_y_from_vertical_preview(
                            window.mouse_position(),
                            dragged,
                            &bounds,
                        )
                    };
                    vertical_slot_from_bounds(probe_y, &bounds, session_count).or_else(|| {
                        rail_slot_for_cursor_position(
                            window.mouse_position(),
                            this.rail_drag_origin_y,
                            session_count,
                            this.leading_top_pad,
                        )
                    })
                });
                this.drop_slot = None;
                this.rail_drag_origin_y = None;
                this.clear_drag_preview();
                if let Some(to) = to {
                    match dragged.origin {
                        DraggedTabOrigin::Sidebar => cx.emit(SidebarReorder {
                            session_id: dragged.session_id,
                            to,
                            move_delta: None,
                        }),
                        DraggedTabOrigin::Pane => {
                            if let Some(pane_id) = dragged.pane_id {
                                cx.emit(SidebarPaneToTab { pane_id, to });
                            }
                        }
                        DraggedTabOrigin::HorizontalTabStrip => {}
                    }
                }
                cx.notify();
            }))
            .child(rail_icon_button(
                "tab-sidebar-rail-expand",
                mode_icon,
                mode_color,
                theme,
                self.navigation_content_offset(),
                cx.listener(|this, _, _, cx| match this.rail_mode_button_action() {
                    RailModeButtonAction::ShowSessionsFromToolPanel => {
                        this.set_pinned(false, cx);
                        cx.emit(SidebarShowSessions);
                    }
                    RailModeButtonAction::RestorePinnedPanelFromRail => {
                        this.clear_rail_only_override(cx);
                    }
                    RailModeButtonAction::ExpandCollapsedPanelFromOverride => {
                        this.clear_rail_only_override(cx);
                        this.set_pinned(true, cx);
                    }
                    RailModeButtonAction::TogglePinned => {
                        this.toggle_pinned(cx);
                    }
                }),
            ))
            .child(div().h(px(RAIL_DIVIDER_HEIGHT)).w(px(20.0)))
            .child(rail_icon_button(
                "tab-sidebar-rail-files",
                "phosphor/tree-view.svg",
                files_color,
                theme,
                self.navigation_content_offset(),
                cx.listener(|_, _, _, cx| {
                    cx.emit(SidebarOpenToolSlot {
                        slot: ActivitySlot::Files,
                    })
                }),
            ))
            .child(rail_icon_button(
                "tab-sidebar-rail-search",
                "phosphor/list-magnifying-glass.svg",
                search_color,
                theme,
                self.navigation_content_offset(),
                cx.listener(|_, _, _, cx| {
                    cx.emit(SidebarOpenToolSlot {
                        slot: ActivitySlot::Search,
                    })
                }),
            ))
            .child(div().h(px(RAIL_DIVIDER_HEIGHT)).w(px(20.0)))
            .child(rail_icon_button(
                "tab-sidebar-rail-new",
                "phosphor/plus.svg",
                theme.muted_foreground,
                theme,
                self.navigation_content_offset(),
                cx.listener(|_, _, _, cx| cx.emit(NewSession)),
            ))
            .child(div().h(px(RAIL_SESSIONS_GAP)).flex_shrink_0());

        if !show_session_tiles {
            return rail;
        }

        // Session pill list — the only vertically scrollable region of
        // the rail. The control buttons and dividers above stay fixed:
        // when the window is short or there are many sessions, this area
        // absorbs the vertical deficit (flex_1 + min_h_0) and scrolls
        // instead of compressing the square session tiles.
        let mut pill_list = div()
            .id("tab-sidebar-rail-scroll")
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(RAIL_ICON_GAP))
            .overflow_y_scroll()
            .track_scroll(&self.rail_scroll);

        for (i, session) in self.sessions.iter().enumerate() {
            let is_active = i == self.active_session;
            // Selection must read without the accent stripe. Overlay the shared
            // base rather than making the same background more opaque.
            let active_bg = sidebar_selection_fill(theme);
            let hover_bg = theme.foreground.opacity(0.04);
            // Drop indicator — a 2px primary-color line above this
            // pill if drop_slot == i, or below the last pill if
            // drop_slot == N. Both states share the same indicator
            // primitive; positioning is the only difference.
            let drag_in_sidebar = cx.has_active_drag() && self.drag_preview.borrow().is_some();
            let show_indicator_above = self.drop_slot == Some(i) && drag_in_sidebar;
            let show_indicator_below =
                i + 1 == session_count && self.drop_slot == Some(session_count) && drag_in_sidebar;
            let session_id = session.id;
            let dragged = DraggedTab {
                session_id,
                label: session.name.clone().into(),
                icon: session.icon,
                origin: DraggedTabOrigin::Sidebar,
                preview_constraint: None,
                pane_id: None,
            };

            let tab_bounds = self.tab_bounds.clone();
            // One selection cue, matching the expanded list. Custom tab
            // colors stay visible as fills rather than external stripes.
            let pill_bg = if let Some(color) = session.color {
                crate::tab_colors::tab_accent_surface_hsla(
                    color,
                    if is_active {
                        crate::tab_colors::TAB_ACCENT_ACTIVE_ALPHA
                    } else {
                        self.tab_accent_inactive_alpha
                    },
                    cx,
                )
            } else if is_active {
                active_bg
            } else {
                gpui::transparent_black()
            };
            let inactive_hover_bg = session.color.map_or(hover_bg, |color| {
                crate::tab_colors::tab_accent_surface_hsla(
                    color,
                    self.tab_accent_inactive_hover_alpha,
                    cx,
                )
            });
            let mut visual = div()
                .absolute()
                .left(px(self.navigation_content_offset()))
                .top_0()
                .size(px(RAIL_ICON_SIZE))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .debug_selector(|| "sidebar-session-selection".into())
                        .absolute()
                        .size(px(RAIL_SELECTION_SIZE))
                        .rounded(px(10.0))
                        .bg(pill_bg)
                        .group_hover(
                            SharedString::from(format!("rail-tab-group-{i}")),
                            move |s| {
                                if is_active {
                                    s
                                } else {
                                    s.bg(inactive_hover_bg)
                                }
                            },
                        ),
                )
                .child(self.activity_layer.as_ref().unwrap().read(cx).icon_group(
                    session.identity_icons(),
                    ui_icon_px(theme, 18.0),
                    if is_active {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    },
                    session.status,
                    theme,
                    true,
                ));
            let mut pill = div()
                .id(SharedString::from(format!("rail-tab-{i}")))
                .debug_selector(|| "sidebar-session-tile".into())
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .size(px(RAIL_ICON_SIZE))
                .flex_shrink_0()
                .cursor_pointer()
                .group(SharedString::from(format!("rail-tab-group-{i}")))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _, _, cx| cx.emit(SidebarSelect { index: i })),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |_this, _, _, cx| {
                        cx.stop_propagation();
                        cx.emit(SidebarCloseTab { session_id });
                    }),
                )
                .on_double_click(cx.listener(move |this, _, window, cx| {
                    this.start_rename_by_id(session_id, window, cx);
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    let want = if *hovered { Some(i) } else { None };
                    if this.hovered_rail != want {
                        this.hovered_rail = want;
                        cx.notify();
                    }
                }))
                // Drag origin only — drag_move + drop live on the
                // rail container so they cover gaps between pills
                // and the slot above the first / below the last row.
                .on_drag(dragged, |dragged, _offset, _window, cx| {
                    cx.new(|_| dragged.clone())
                })
                .on_prepaint(move |bounds, _, _| {
                    tab_bounds.borrow_mut().push(bounds);
                });

            if let Some(dot) = self.notifications.dot(
                session.id,
                crate::tab_activity::notification_label(
                    session.status,
                    session.needs_attention,
                    is_active,
                ),
                theme.primary,
                gpui::size(px(RAIL_ICON_SIZE), px(RAIL_ICON_SIZE)),
                gpui::size(
                    crate::assets::tab_identity_group_width(
                        session.identity_icons().len(),
                        ui_icon_px(theme, 18.0).min(px(18.0)),
                        true,
                    ),
                    crate::assets::tab_identity_group_height(
                        session.identity_icons().len(),
                        ui_icon_px(theme, 18.0).min(px(18.0)),
                    ),
                ),
                window,
                cx,
            ) {
                visual = visual.child(dot);
            }
            pill = pill.child(visual);
            if show_indicator_above {
                pill = pill.child(rail_drop_indicator(theme, true));
            }
            if show_indicator_below {
                pill = pill.child(rail_drop_indicator(theme, false));
            }

            pill_list = pill_list.child(pill);
        }

        rail.child(pill_list)
    }

    /// Temporary navigation uses the existing panel rows and their interactions,
    /// but is painted by the workspace over the terminal, never reserved in layout.
    pub(crate) fn render_navigation_peek_overlay(
        &mut self,
        top: f32,
        background: Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.can_peek() || !self.peek_active() {
            return None;
        }
        let progress = self.peek_motion.value(window, cx);
        if !self.peek_open && progress == 0.0 {
            return None;
        }
        let activity = self
            .activity_layer
            .get_or_insert_with(|| cx.new(|cx| crate::tab_activity::TabActivity::new(window, cx)))
            .clone();
        activity.read(cx).clear();
        let layout =
            NavigationLayout::preview(self.effective_panel_width(), self.leading_top_pad, progress);
        let width = layout.width;
        let body = self.render_panel_body(layout, window, cx);
        Some(
            div()
                .id("tab-sidebar-navigation-peek")
                .debug_selector(|| "sidebar-navigation-peek".into())
                .absolute()
                .left_0()
                .top(px(top))
                .w(px(width + self.terminal_gutter))
                .h((window.viewport_size().height - px(top)).max(px(0.0)))
                // This is attached window chrome, continuing from the caption
                // to the bottom edge. A local corner cuts a notch into that
                // continuous surface and exposes the terminal beneath it.
                .overflow_hidden()
                .occlude()
                .bg(Hsla {
                    a: 1.0,
                    ..background
                })
                .font_family(cx.theme().font_family.clone())
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.set_navigation_hover(*hovered, cx);
                }))
                .child(
                    div()
                        .relative()
                        .left(px(self.navigation_content_offset()))
                        .w(px(width))
                        .h_full()
                        .child(body),
                )
                .child(activity)
                .into_any_element(),
        )
    }

    /// Floating hover card shown next to the hovered rail icon.
    /// Composed in workspace coordinates so it renders OVER the
    /// translucent terminal pane (instead of behind it).
    ///
    /// Anchor to the measured tile, not cursor movement within it. This keeps
    /// the details stable while the user reads them or moves to another tab.
    ///
    /// Composed by the workspace as a root overlay (window coordinates)
    /// so it paints above the terminal and is not clipped by the
    /// sidebar's overflow-hidden container.
    pub fn render_hover_card_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.renders_rail_only() || self.peek_active() || self.peek_task.is_some() {
            return None;
        }
        let i = self.hovered_rail?;
        if self.drop_slot.is_some() {
            return None;
        }
        let session = self.sessions.get(i)?;
        let theme = cx.theme();
        let bg = theme.popover.opacity(1.0);
        let cursor = window.mouse_position();
        let anchor_y = self
            .tab_bounds
            .borrow()
            .get(i)
            .map(|bounds| bounds.top())
            .unwrap_or(cursor.y - ui_px(theme, 21.0));
        let min_top = self.leading_top_pad + RAIL_TOP_CONTROLS_HEIGHT + 8.0;
        let max_height = (window.viewport_size().height.as_f32() - min_top - 8.0).clamp(1.0, 240.0);

        let card_inner = sidebar_hover_content(session, window, cx);
        #[cfg(target_os = "macos")]
        let theme = cx.theme();

        let card = div()
            .id("tab-sidebar-hover-card")
            .debug_selector(|| "tab-hover-card".into())
            .w(px(HOVER_CARD_WIDTH.min(
                (window.viewport_size().width.as_f32() - RAIL_WIDTH - 14.0).max(1.0),
            )))
            .relative()
            .rounded(px(10.0))
            .bg(bg)
            .occlude()
            .child(
                card_inner
                    .id("tab-title-details")
                    .max_h(px(max_height))
                    .overflow_y_scroll(),
            )
            .map(|card| {
                #[cfg(target_os = "macos")]
                {
                    card.child(
                        crate::workspace::frame::surface_edge(
                            10.0,
                            theme
                                .foreground
                                .opacity(if theme.is_dark() { 0.12 } else { 0.075 }),
                            true,
                        )
                        .inset_0(),
                    )
                }
                #[cfg(not(target_os = "macos"))]
                {
                    card
                }
            });

        // Let GPUI fit the measured card, including wrapped paths, instead
        // of estimating its height from character counts or line counts.
        Some(
            anchored()
                .position(point(px(RAIL_WIDTH + 6.0), anchor_y.max(px(min_top))))
                .snap_to_window_with_margin(px(8.0))
                .child(card)
                .into_any_element(),
        )
    }

    fn render_panel_body(
        &mut self,
        layout: NavigationLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mode_label = format!(
            "{} tabs ({})",
            if self.tools_panel_open {
                "Show"
            } else if layout.preview {
                "Keep expanded"
            } else if self.is_pinned() {
                "Compact"
            } else {
                "Expand"
            },
            self.sessions.len()
        );
        let preview = layout
            .preview
            .then_some((layout.width, layout.label_opacity));
        let mode_color = if self.tools_panel_open {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        };
        let files_color = if self.tools_panel_open && self.active_tool_slot == ActivitySlot::Files {
            cx.theme().primary
        } else {
            cx.theme().muted_foreground
        };
        let search_color = if self.tools_panel_open && self.active_tool_slot == ActivitySlot::Search
        {
            cx.theme().primary
        } else {
            cx.theme().muted_foreground
        };
        let header = div()
            .relative()
            .w_full()
            .h(px(layout.list_top))
            .flex_shrink_0()
            .child(navigation_button(
                "tab-sidebar-rail-expand",
                "phosphor/list.svg",
                mode_label.into(),
                layout.controls[0],
                preview,
                mode_color,
                cx,
                cx.listener(move |this, _, _, cx| {
                    if this.tools_panel_open {
                        this.set_pinned(false, cx);
                        cx.emit(SidebarShowSessions);
                    } else if layout.preview {
                        this.set_pinned(true, cx);
                    } else {
                        this.toggle_pinned(cx);
                    }
                }),
            ))
            .child(navigation_button(
                "tab-sidebar-rail-files",
                "phosphor/tree-view.svg",
                "Files".into(),
                layout.controls[1],
                preview,
                files_color,
                cx,
                cx.listener(|_, _, _, cx| {
                    cx.emit(SidebarOpenToolSlot {
                        slot: ActivitySlot::Files,
                    })
                }),
            ))
            .child(navigation_button(
                "tab-sidebar-rail-search",
                "phosphor/list-magnifying-glass.svg",
                "Search across files".into(),
                layout.controls[2],
                preview,
                search_color,
                cx,
                cx.listener(|_, _, _, cx| {
                    cx.emit(SidebarOpenToolSlot {
                        slot: ActivitySlot::Search,
                    })
                }),
            ))
            .child(navigation_button(
                "tab-sidebar-rail-new",
                "phosphor/plus.svg",
                "New tab".into(),
                layout.controls[3],
                preview,
                cx.theme().muted_foreground,
                cx,
                cx.listener(|_, _, _, cx| cx.emit(NewSession)),
            ));

        let total = self.sessions.len();
        self.tab_bounds.borrow_mut().clear();
        // Body-level drag fallback: per-row drag_move + on_drop only
        // fire when the cursor is squarely inside a row's bounds. If
        // the user releases the mouse just below the last row (a
        // common gesture when targeting the "after the last row"
        // slot) the row's on_drop never fires — and without this
        // fallback the reorder is silently dropped. This handler
        // catches every drop inside the list area, including the
        // empty space below the last row, and maps it to a slot
        // using the same top-half / bottom-half rule as the per-row
        // handlers.
        let mut list = div()
            .id("tab-sidebar-panel-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px(px(layout.row_inset))
            .gap(px(2.0))
            .overflow_y_scroll()
            .track_scroll(if layout.preview {
                &self.rail_scroll
            } else {
                &self.panel_scroll
            })
            // Body-level drag_move ONLY runs the "below last row"
            // detection — the per-row handlers handle every cursor
            // position that's actually over a row. We fire the
            // fallback only when the cursor is below the last row
            // (i.e. the last row's bounds end above the cursor),
            // setting drop_slot = total so the indicator below the
            // last row lights up.
            .on_drag_move::<DraggedTab>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                    if !matches!(
                        event.drag(cx).origin,
                        DraggedTabOrigin::Sidebar | DraggedTabOrigin::Pane
                    ) {
                        return;
                    }
                    let is_pane = event.drag(cx).origin == DraggedTabOrigin::Pane;
                    // For all drag origins, only proceed if cursor is inside container.
                    if !point_in_bounds(&event.event.position, &event.bounds) {
                        if this.drop_slot.is_some() || this.drag_preview.borrow().is_some() {
                            this.drop_slot = None;
                            this.clear_drag_preview();
                            cx.notify();
                        }
                        return;
                    }
                    let bounds = this.tab_bounds.borrow().clone();
                    let slot = vertical_slot_from_bounds(event.event.position.y, &bounds, total);
                    if let Some(slot) = slot {
                        // Cursor is over a tab item — create/keep preview.
                        let preview_size = tab_drag_preview_size(this.is_pinned());
                        this.ensure_drag_preview(
                            event.drag(cx),
                            event.bounds.origin.x,
                            preview_size.height / 2.0,
                        );
                        if this.drop_slot != Some(slot) {
                            this.drop_slot = Some(slot);
                            cx.notify();
                        }
                        return;
                    }
                    if is_pane {
                        // Pane drag but cursor not over any tab item — clear.
                        if this.drop_slot.is_some() || this.drag_preview.borrow().is_some() {
                            this.drop_slot = None;
                            this.clear_drag_preview();
                            cx.notify();
                        }
                        return;
                    }
                    if total == 0 {
                        return;
                    }
                    // Sidebar drag: "below last row" fallback.
                    if bounds
                        .last()
                        .is_some_and(|last| event.event.position.y >= last.bottom())
                    {
                        let preview_missing = this.drag_preview.borrow().is_none();
                        let preview_size = tab_drag_preview_size(this.is_pinned());
                        this.ensure_drag_preview(
                            event.drag(cx),
                            event.bounds.origin.x,
                            preview_size.height / 2.0,
                        );
                        if this.drop_slot != Some(total) || preview_missing {
                            this.drop_slot = Some(total);
                            cx.notify();
                        }
                    }
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                if !matches!(
                    dragged.origin,
                    DraggedTabOrigin::Sidebar | DraggedTabOrigin::Pane
                ) {
                    return;
                }
                // For pane-origin drags, only handle if cursor was in sidebar.
                if dragged.origin == DraggedTabOrigin::Pane && this.drop_slot.is_none() {
                    this.clear_drag_preview();
                    return;
                }
                // Body-level drop fallback. The per-row on_drop only
                // fires when mouseup is squarely inside a row's
                // bounds. If the user released just below the last
                // row (the "after the last row" gesture) no per-row
                // handler fires and the drag is silently dropped —
                // hence this fallback.
                let to = this
                    .drop_slot
                    .or_else(|| {
                        let bounds = this.tab_bounds.borrow().clone();
                        vertical_slot_from_bounds(window.mouse_position().y, &bounds, total)
                    })
                    .unwrap_or(total);
                this.drop_slot = None;
                this.clear_drag_preview();
                match dragged.origin {
                    DraggedTabOrigin::Sidebar => cx.emit(SidebarReorder {
                        session_id: dragged.session_id,
                        to,
                        move_delta: None,
                    }),
                    DraggedTabOrigin::Pane => {
                        if let Some(pane_id) = dragged.pane_id {
                            cx.emit(SidebarPaneToTab { pane_id, to });
                        }
                    }
                    DraggedTabOrigin::HorizontalTabStrip => {}
                }
                cx.notify();
            }));

        let renaming_index = self.rename.as_ref().map(|r| r.index);
        let rename_input = self.rename.as_ref().map(|r| r.input.clone());
        let drop_slot = self.drop_slot;
        let drag_in_sidebar = cx.has_active_drag() && self.drag_preview.borrow().is_some();

        for i in 0..total {
            let session_clone = self.sessions[i].clone();
            let row = self.render_panel_row(
                i,
                &session_clone,
                renaming_index,
                rename_input.clone(),
                drop_slot,
                total,
                drag_in_sidebar,
                window,
                cx,
                layout,
            );
            list = list.child(row);
        }

        div()
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .flex_shrink_0()
            .child(header)
            .child(list)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_panel_row(
        &self,
        i: usize,
        session: &SessionEntry,
        renaming_index: Option<usize>,
        rename_input: Option<Entity<InputState>>,
        drop_slot: Option<usize>,
        total: usize,
        drag_in_sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        layout: NavigationLayout,
    ) -> AnyElement {
        let theme_clone = cx.theme().clone();
        let theme = &theme_clone;

        let is_active = i == self.active_session;
        let is_renaming = renaming_index == Some(i);
        // Indicator above this row when drop_slot == i, and below
        // the last row (i == total-1) when drop_slot == total. Slot
        // semantics: K means "insert at index K"; slot N means "at
        // the bottom".
        let drop_above = drop_slot == Some(i) && drag_in_sidebar;
        let drop_below = i + 1 == total && drop_slot == Some(total) && drag_in_sidebar;

        let row_h = layout.row_height;

        let label_block: AnyElement = if is_renaming {
            if let Some(input) = rename_input {
                div()
                    .flex_1()
                    .min_w_0()
                    .on_action(cx.listener(|this, _: &InputEscape, _, cx| {
                        this.cancel_rename(cx);
                    }))
                    .child(Input::new(&input).small().appearance(false))
                    .into_any_element()
            } else {
                div().flex_1().min_w_0().into_any_element()
            }
        } else {
            let name = session.name.clone();
            let sys = theme.font_family.clone();
            let fg = theme.foreground;
            div()
                .flex()
                .items_center()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .w_full()
                        .overflow_hidden()
                        .truncate()
                        .debug_selector(|| "sidebar-session-name".into())
                        .text_size(ui_px(theme, 12.5))
                        .line_height(ui_px(theme, 18.0))
                        .font_family(sys)
                        .text_color(fg)
                        .font_weight(if is_active {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .child(name),
                )
                .into_any_element()
        };

        let row_group = SharedString::from(format!("panel-tab-row-{i}"));
        let session_id = session.id;

        // Reserve one stable trailing slot. Rename stays on double-click and
        // in the context menu instead of stealing title width on every row.
        let close_btn =
            sidebar_close_button(SharedString::from(format!("panel-tab-close-{i}")), cx)
                .on_click(
                    cx.listener(move |_this, _, _, cx| cx.emit(SidebarCloseTab { session_id })),
                )
                .invisible()
                .group_hover(row_group.clone(), |s| s.visible());

        let drop_line_above = div()
            .absolute()
            .left(px(8.0))
            .right(px(8.0))
            .top(px(-1.0))
            .h(px(2.0))
            .rounded(px(1.0))
            .bg(if drop_above {
                theme.primary
            } else {
                gpui::transparent_black()
            });
        let drop_line_below = div()
            .absolute()
            .left(px(8.0))
            .right(px(8.0))
            .bottom(px(-1.0))
            .h(px(2.0))
            .rounded(px(1.0))
            .bg(if drop_below {
                theme.primary
            } else {
                gpui::transparent_black()
            });

        let accent_color = session.color;
        let row_bg = if is_active {
            accent_color
                .map(|color| {
                    crate::tab_colors::tab_accent_surface_hsla(
                        color,
                        crate::tab_colors::TAB_ACCENT_ACTIVE_ALPHA,
                        cx,
                    )
                })
                .unwrap_or_else(|| sidebar_selection_fill(theme))
        } else {
            accent_color
                .map(|color| {
                    crate::tab_colors::tab_accent_surface_hsla(
                        color,
                        self.tab_accent_inactive_alpha,
                        cx,
                    )
                })
                .unwrap_or(gpui::transparent_black())
        };
        let hover_bg = theme.foreground.opacity(0.04);
        let inactive_hover_bg = accent_color
            .map(|color| {
                crate::tab_colors::tab_accent_surface_hsla(
                    color,
                    self.tab_accent_inactive_hover_alpha,
                    cx,
                )
            })
            .unwrap_or(hover_bg);

        let dragged = DraggedTab {
            session_id,
            label: session.name.clone().into(),
            icon: session.icon,
            origin: DraggedTabOrigin::Sidebar,
            preview_constraint: None,
            pane_id: None,
        };
        let tab_bounds = self.tab_bounds.clone();

        let icon_size = px(layout.icon_size);
        let identity_width = if layout.compact {
            px(32.0)
        } else {
            crate::assets::tab_identity_group_width(3, icon_size, false).max(px(32.0))
        };
        // A shared leading well keeps every title on one column, regardless of
        // how many distinct applications the row happens to contain.
        let label_left = layout.icon_center_x + identity_width.as_f32() * 0.5 + 8.0;
        let mut icon_stack = div()
            .absolute()
            .left(px(layout.icon_center_x) - identity_width * 0.5)
            .top(px((row_h - 32.0) * 0.5))
            .w(identity_width)
            .h(px(32.0))
            .flex()
            .items_center()
            .justify_center()
            .child(self.activity_layer.as_ref().unwrap().read(cx).icon_group(
                session.identity_icons(),
                icon_size,
                if is_active {
                    theme.foreground
                } else {
                    theme.muted_foreground
                },
                session.status,
                theme,
                layout.compact,
            ));
        if let Some(dot) = self.notifications.dot(
            session.id,
            crate::tab_activity::notification_label(
                session.status,
                session.needs_attention,
                is_active,
            ),
            theme.primary,
            gpui::size(identity_width, px(32.0)),
            gpui::size(
                crate::assets::tab_identity_group_width(
                    session.identity_icons().len(),
                    icon_size,
                    layout.compact,
                ),
                crate::assets::tab_identity_group_height(session.identity_icons().len(), icon_size),
            ),
            window,
            cx,
        ) {
            icon_stack = icon_stack.child(dot);
        }

        let row = div()
            .id(SharedString::from(format!("panel-tab-{i}")))
            .debug_selector(|| "sidebar-session-row".into())
            .group(row_group.clone())
            .relative()
            .flex()
            .items_center()
            .h(px(row_h))
            .flex_shrink_0()
            .rounded(px(row_h * 0.5))
            .cursor_pointer()
            .child(
                div()
                    .debug_selector(|| "sidebar-session-surface".into())
                    .absolute()
                    .inset(px(layout.selection_inset))
                    .rounded(px(layout.selection_radius))
                    .bg(row_bg)
                    .group_hover(row_group.clone(), move |s| {
                        if is_active {
                            s
                        } else {
                            s.bg(inactive_hover_bg)
                        }
                    }),
            )
            .text_color(if is_active {
                theme.foreground
            } else {
                theme.muted_foreground
            })
            .tooltip({
                let sidebar = cx.entity().downgrade();
                move |window, cx| {
                    let width = (window.viewport_size().width.as_f32() - 48.0).clamp(1.0, 304.0);
                    let details = cx
                        .new(|cx| SidebarHoverDetails::new(sidebar.clone(), session_id, width, cx));
                    Tooltip::element(move |_, _| details.clone())
                        .p_0()
                        .border_0()
                        .shadow(Vec::new())
                        .rounded(px(10.0))
                        .build(window, cx)
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _, _, cx| cx.emit(SidebarSelect { index: i })),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |_this, _, _, cx| cx.emit(SidebarCloseTab { session_id })),
            )
            .on_double_click(cx.listener(move |this, _, window, cx| {
                this.begin_rename(i, window, cx);
            }))
            .on_drag(dragged, |dragged, _offset, _window, cx| {
                cx.new(|_| dragged.clone())
            })
            .on_prepaint(move |bounds, _, _| {
                tab_bounds.borrow_mut().push(bounds);
            })
            .on_drag_move::<DraggedTab>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                    if !matches!(
                        event.drag(cx).origin,
                        DraggedTabOrigin::Sidebar | DraggedTabOrigin::Pane
                    ) {
                        return;
                    }
                    if !point_in_bounds(&event.event.position, &event.bounds) {
                        return;
                    }
                    // Cursor is inside this row — create/keep preview.
                    let preview_size = tab_drag_preview_size(this.is_pinned());
                    this.ensure_drag_preview(
                        event.drag(cx),
                        event.bounds.origin.x,
                        preview_size.height / 2.0,
                    );
                    // Top half → insert above (slot = i), bottom half → insert below (slot = i+1).
                    let local_y = event.event.position.y - event.bounds.origin.y;
                    let half = event.bounds.size.height / 2.0;
                    let slot = if local_y < half { i } else { i + 1 };
                    if this.drop_slot != Some(slot) {
                        this.drop_slot = Some(slot);
                        cx.notify();
                    }
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                if !matches!(
                    dragged.origin,
                    DraggedTabOrigin::Sidebar | DraggedTabOrigin::Pane
                ) {
                    return;
                }
                // For pane-origin drags, only handle if cursor was in sidebar.
                if dragged.origin == DraggedTabOrigin::Pane && this.drop_slot.is_none() {
                    this.clear_drag_preview();
                    return;
                }
                let to = this.drop_slot.unwrap_or(i);
                this.drop_slot = None;
                this.clear_drag_preview();
                match dragged.origin {
                    DraggedTabOrigin::Sidebar => cx.emit(SidebarReorder {
                        session_id: dragged.session_id,
                        to,
                        move_delta: None,
                    }),
                    DraggedTabOrigin::Pane => {
                        if let Some(pane_id) = dragged.pane_id {
                            cx.emit(SidebarPaneToTab { pane_id, to });
                        }
                    }
                    DraggedTabOrigin::HorizontalTabStrip => {}
                }
                cx.notify();
            }))
            .context_menu({
                let session_id = session.id;
                let has_user_label = session.has_user_label;
                let current_color = session.color;
                let weak = cx.weak_entity();
                move |menu, _window, cx| {
                    if let Some(sidebar) = weak.upgrade() {
                        let menu_entity = cx.entity();
                        sidebar.update(cx, |sidebar, cx| {
                            sidebar.hold_navigation_peek_for_menu(menu_entity, cx);
                        });
                    }
                    build_row_context_menu(
                        menu,
                        weak.clone(),
                        i,
                        session_id,
                        total,
                        has_user_label,
                        current_color,
                    )
                }
            })
            .child(drop_line_above)
            .child(drop_line_below)
            .child(icon_stack)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .left(px(label_left))
                    .w(px((layout.width
                        - 2.0 * layout.row_inset
                        - label_left
                        - (row_h - CLOSE_TARGET_SIZE) * 0.5
                        - CLOSE_TARGET_SIZE
                        - 8.0)
                        .max(0.0)))
                    .overflow_hidden()
                    .opacity(layout.label_opacity)
                    .child(label_block),
            )
            .when(layout.label_opacity >= 0.99, |row| {
                row.child(
                    div()
                        .absolute()
                        .right(px((row_h - CLOSE_TARGET_SIZE) * 0.5))
                        .top(px((row_h - CLOSE_TARGET_SIZE) * 0.5))
                        .child(close_btn),
                )
            });
        row.into_any_element()
    }
}

impl Render for SessionSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Deferred context menus inherit the trigger's text style, including its font.
        let ui_font = cx.theme().font_family.clone();
        let activity_layer = self
            .activity_layer
            .get_or_insert_with(|| cx.new(|cx| crate::tab_activity::TabActivity::new(window, cx)))
            .clone();
        if self.peek_active() && self.can_peek() {
            return div().w(px(RAIL_WIDTH)).h_full().into_any_element();
        }
        activity_layer.read(cx).clear();
        // Clear stale drop indicator after the drag completes — GPUI
        // doesn't expose an on_drag_end hook for the source element.
        let has_drag = cx.has_active_drag();
        if !has_drag {
            let mut changed = false;
            if self.drop_slot.is_some() {
                self.drop_slot = None;
                changed = true;
            }
            if self.drag_preview.borrow().is_some() {
                self.clear_drag_preview();
                changed = true;
            }
            if changed {
                cx.notify();
            }
        }
        // The workspace samples this too, so terminal reservation follows
        // the same animated compact/expanded extent.
        let tools = self.tools_motion.value(window, cx);
        let progress = self.width_motion.value(window, cx);
        let layout_progress = self
            .peek_pin_layout_motion
            .as_mut()
            .map_or(progress, |motion| motion.value(window, cx));
        if self
            .peek_pin_layout_motion
            .as_ref()
            .is_some_and(|motion| !motion.is_animating())
            && !self.width_motion.is_animating()
        {
            self.peek_pin_layout_motion = None;
        }

        if tools >= 1.0 || self.rail_only_override {
            return div()
                .font_family(ui_font.clone())
                .relative()
                .h_full()
                .w(px(RAIL_WIDTH))
                .flex_shrink_0()
                .child(self.render_rail(window, cx))
                .child(activity_layer)
                .into_any_element();
        }

        if self.is_pinned() || progress > 0.0 {
            let theme = cx.theme();
            let mut layout = NavigationLayout::new(
                self.effective_panel_width(),
                self.leading_top_pad.max(0.0),
                ui_px(theme, ROW_HEIGHT).max(px(ROW_HEIGHT)).as_f32(),
                ui_icon_px(theme, 16.0).as_f32(),
                layout_progress,
            );
            // Keep the preview's revealed width and labels while its compact
            // icons travel into the pinned toolbar/list arrangement.
            layout.width = RAIL_WIDTH + (self.effective_panel_width() - RAIL_WIDTH) * progress;
            layout.label_opacity = ((progress - 0.15) / 0.7).clamp(0.0, 1.0);
            let panel = self
                .render_panel_body(layout, window, cx)
                .relative()
                .left(px(self.navigation_content_offset()));
            div()
                .font_family(ui_font)
                .relative()
                .h_full()
                .w(px(layout.width))
                .flex_shrink_0()
                .overflow_hidden()
                .child(panel)
                .child(activity_layer)
                .into_any_element()
        } else {
            div()
                .font_family(ui_font)
                .relative()
                .h_full()
                .w(px(RAIL_WIDTH))
                .flex_shrink_0()
                .child(self.render_rail(window, cx))
                .child(activity_layer)
                .into_any_element()
        }
    }
}

fn sidebar_selection_fill(theme: &gpui_component::Theme) -> Hsla {
    let opacity = if cfg!(target_os = "macos") {
        if theme.is_dark() { 0.10 } else { 0.045 }
    } else if theme.is_dark() {
        0.12
    } else {
        0.065
    };
    theme.foreground.opacity(opacity)
}

fn sidebar_tab_icon(icon: &'static str) -> &'static str {
    crate::assets::tab_identity_asset(icon)
}

/// An open component tooltip follows live tab facts without subscribing the
/// sidebar to animation frames or holding a stale copy from mouse entry.
struct SidebarHoverDetails {
    sidebar: WeakEntity<SessionSidebar>,
    session_id: u64,
    width: f32,
}

impl SidebarHoverDetails {
    fn new(
        sidebar: WeakEntity<SessionSidebar>,
        session_id: u64,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(sidebar) = sidebar.upgrade() {
            cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        }
        Self {
            sidebar,
            session_id,
            width,
        }
    }
}

impl Render for SidebarHoverDetails {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.sidebar.upgrade().and_then(|sidebar| {
            sidebar
                .read(cx)
                .sessions
                .iter()
                .find(|session| session.id == self.session_id)
                .cloned()
        });
        session
            .map(|session| {
                sidebar_hover_content(&session, window, cx)
                    .w(px(self.width))
                    .id("live-tab-details")
                    .max_h(px((window.viewport_size().height.as_f32() - 48.0).max(1.0)))
                    .overflow_y_scroll()
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element())
    }
}

/// Shared by the expanded navigator tooltip and compact rail hover card.
/// Keep metadata out of the permanent single-line rows.
fn multi_pane_hover_content(session: &SessionEntry, window: &mut Window, cx: &mut App) -> Div {
    let theme = cx.theme().clone();
    let layer = window.use_keyed_state(
        gpui::ElementId::NamedInteger("pane-hover-activity".into(), session.id),
        cx,
        |window, cx| cx.new(|cx| crate::tab_activity::TabActivity::new(window, cx)),
    );
    let layer = layer.read(cx).clone();
    layer.read(cx).clear();
    let size = ui_icon_px(&theme, 16.0);
    let secondary = theme.popover.blend(theme.foreground.opacity(0.72));
    let mut rows = div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .px(px(12.0))
        .pb(px(12.0));
    for pane in &session.pane_details {
        let activity = pane.status.filter(|status| {
            !matches!(
                status.activity,
                con_core::terminal_status::Activity::Idle
                    | con_core::terminal_status::Activity::Unknown
            )
        });
        let marker = activity.map(|status| layer.read(cx).status_marker(status, size, &theme));
        let mut text = div().flex_1().min_w_0().flex().flex_col().child(
            div()
                .debug_selector({
                    let id = pane.id;
                    move || format!("pane-hover-title-{id}")
                })
                .text_size(ui_px(&theme, 12.0))
                .line_height(ui_px(&theme, 18.0))
                .text_color(theme.foreground)
                .child(pane.title.clone()),
        );
        if let Some(directory) = &pane.directory
            && directory != &pane.title
        {
            text = text.child(
                div()
                    .debug_selector({
                        let id = pane.id;
                        move || format!("pane-hover-path-{id}")
                    })
                    .mt(px(4.0))
                    .font_family(theme.mono_font_family.clone())
                    .text_size(ui_px(&theme, 11.0))
                    .line_height(ui_px(&theme, 16.0))
                    .text_color(secondary)
                    .child(directory.clone()),
            );
        }
        rows = rows.child(
            div()
                .id(gpui::ElementId::NamedInteger("pane-detail".into(), pane.id))
                .debug_selector({
                    let id = pane.id;
                    move || format!("pane-hover-row-{id}")
                })
                .flex_shrink_0()
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(
                    div()
                        .mt(px(1.0))
                        .flex_none()
                        .child(crate::assets::tab_identity_icon(
                            pane.icon,
                            size,
                            theme.foreground,
                        )),
                )
                .child(text)
                .child(
                    div()
                        .id("pane-state")
                        .relative()
                        .flex_none()
                        .size(size)
                        .role(gpui::Role::Label)
                        .aria_label(pane.status_label.clone())
                        .children(marker),
                )
                .when_some(
                    activity.and_then(|status| status.percent),
                    |row, percent| row.child(hover_progress(percent, &theme)),
                ),
        );
    }
    div()
        .relative()
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .bg(gpui::linear_gradient(
            180.0,
            gpui::linear_color_stop(
                theme
                    .popover
                    .blend(
                        theme
                            .foreground
                            .opacity(if theme.is_dark() { 0.055 } else { 0.032 }),
                    ),
                0.0,
            ),
            gpui::linear_color_stop(theme.popover.blend(theme.foreground.opacity(0.008)), 1.0),
        ))
        .font_family(theme.font_family.clone())
        .child(
            div()
                .debug_selector(|| "tab-hover-heading".into())
                .px(px(12.0))
                .py(px(12.0))
                .text_size(ui_px(&theme, 13.0))
                .line_height(ui_px(&theme, 20.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(session.name.clone()),
        )
        .child(rows)
        .children(session.native_session_hint.as_ref().map(|hint| {
            div()
                .px(px(12.0))
                .pb(px(12.0))
                .text_size(ui_px(&theme, 12.0))
                .line_height(ui_px(&theme, 18.0))
                .text_color(secondary)
                .child(hint.clone())
        }))
        .child(layer)
}

fn hover_progress(percent: u8, theme: &gpui_component::Theme) -> Div {
    div()
        .debug_selector(|| "tab-hover-progress".into())
        .flex_none()
        .w(ui_px(theme, 28.0))
        .text_size(ui_px(theme, 11.0))
        .text_color(theme.popover.blend(theme.foreground.opacity(0.72)))
        .font_family(theme.mono_font_family.clone())
        .child(format!("{percent}%"))
}

fn sidebar_hover_content(session: &SessionEntry, window: &mut Window, cx: &mut App) -> Div {
    if session.pane_count > 1 {
        return multi_pane_hover_content(session, window, cx);
    }
    let theme = cx.theme().clone();
    let theme = &theme;
    let application = session
        .status_detail
        .as_ref()
        .and_then(|detail| detail.content.application.as_deref());
    let app_icon = crate::assets::agent_cli_icon(application);
    let icons = app_icon
        .as_ref()
        .map(std::slice::from_ref)
        .unwrap_or_else(|| session.identity_icons());
    let status_marker = session.status.map(|status| {
        let layer = window.use_keyed_state(
            gpui::ElementId::NamedInteger("tab-hover-activity".into(), session.id),
            cx,
            // Keep the keyed owner inert: animation notifications belong to
            // the mounted layer, never the containing sidebar or tooltip.
            |window, cx| cx.new(|cx| crate::tab_activity::TabActivity::new(window, cx)),
        );
        let layer = layer.read(cx).clone();
        let size = ui_icon_px(theme, 16.0);
        let label = session
            .status_detail
            .as_ref()
            .map(|detail| detail.content.status.clone())
            .unwrap_or_else(|| format!("{:?}", status.activity));
        let activity = layer.read(cx);
        activity.clear();
        let marker = activity.status_marker(status, size, theme);
        div()
            .id("tab-hover-status")
            .debug_selector(|| "tab-hover-status".into())
            .role(gpui::Role::Label)
            .aria_label(label)
            .relative()
            .flex_none()
            .size(size)
            .child(marker)
            .child(layer)
            .into_any_element()
    });
    let secondary = theme.popover.blend(theme.foreground.opacity(0.72));
    let mut header = div()
        .debug_selector(|| "tab-hover-summary".into())
        .flex_shrink_0()
        .flex()
        .flex_col()
        .px(px(12.0))
        .py(px(12.0))
        .font_family(theme.font_family.clone())
        .child(
            div()
                .debug_selector(|| "tab-hover-heading".into())
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .debug_selector(|| "tab-hover-name".into())
                        .flex_1()
                        .min_w_0()
                        .text_size(ui_px(theme, 13.0))
                        .line_height(ui_px(theme, 20.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(session.name.clone()),
                )
                .child(crate::assets::tab_identity_group(
                    icons,
                    ui_icon_px(theme, 16.0),
                    theme.foreground,
                    false,
                    theme,
                ))
                .children(status_marker)
                .when_some(
                    session.status.and_then(|status| status.percent),
                    |row, percent| row.child(hover_progress(percent, theme)),
                ),
        );
    if let Some(app) = application
        && app_icon.is_none()
        && !app.eq_ignore_ascii_case(&session.name)
    {
        header = header.child(
            div()
                .mt(px(4.0))
                .text_size(ui_px(theme, 11.0))
                .line_height(ui_px(theme, 16.0))
                .text_color(secondary)
                .child(app.to_string()),
        );
    }
    let mut content = div()
        .flex()
        .flex_col()
        .px(px(12.0))
        .pb(px(12.0))
        .font_family(theme.font_family.clone());
    let mut has_body = false;
    if let Some(context) = session
        .status_detail
        .as_ref()
        .and_then(|detail| detail.content.context.as_ref())
    {
        content = content.child(
            div()
                .debug_selector(|| "tab-hover-context".into())
                .flex_shrink_0()
                .text_size(ui_px(theme, 12.0))
                .line_height(ui_px(theme, 18.0))
                .text_color(theme.foreground)
                .child(context.clone()),
        );
        has_body = true;
    }
    if let Some(hint) = &session.native_session_hint {
        content = content.child(
            div()
                .mt(px(if has_body { 8.0 } else { 0.0 }))
                .text_size(ui_px(theme, 12.0))
                .line_height(ui_px(theme, 18.0))
                .text_color(secondary)
                .child(hint.clone()),
        );
        has_body = true;
    }
    if let Some(path) = session
        .status_detail
        .as_ref()
        .and_then(|detail| detail.source_path.as_ref())
        .or(session.subtitle.as_ref())
    {
        content = content.child(
            div()
                .debug_selector(|| "tab-hover-path".into())
                .flex_shrink_0()
                .mt(px(if has_body { 8.0 } else { 0.0 }))
                .w_full()
                .font_family(theme.mono_font_family.clone())
                .text_size(ui_px(theme, 11.0))
                .line_height(ui_px(theme, 16.0))
                .text_color(secondary)
                .child(path.clone()),
        );
        has_body = true;
    }
    let mut metadata: Vec<&str> = Vec::new();
    if session.is_ssh {
        metadata.push("SSH");
    }
    if session.needs_attention {
        metadata.push("Unread activity");
    }
    if !metadata.is_empty() {
        content = content.child(
            div()
                .debug_selector(|| "tab-hover-meta".into())
                .flex_shrink_0()
                .mt(px(if has_body { 8.0 } else { 0.0 }))
                .text_size(ui_px(theme, 10.5))
                .line_height(ui_px(theme, 14.0))
                .text_color(secondary)
                .child(metadata.join(" · ")),
        );
        has_body = true;
    }
    div()
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .bg(gpui::linear_gradient(
            180.0,
            gpui::linear_color_stop(
                theme
                    .popover
                    .blend(
                        theme
                            .foreground
                            .opacity(if theme.is_dark() { 0.055 } else { 0.032 }),
                    ),
                0.0,
            ),
            gpui::linear_color_stop(theme.popover.blend(theme.foreground.opacity(0.008)), 1.0),
        ))
        .font_family(theme.font_family.clone())
        .child(header)
        .when(has_body, |view| view.child(content))
}

fn sanitize_tab_accent_alpha(alpha: f32) -> f32 {
    if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        crate::tab_colors::TAB_ACCENT_INACTIVE_ALPHA
    }
}

/// 2-px horizontal drop indicator drawn above (`above=true`) or
/// below the rail pill it's attached to. Used during a drag to mark
/// the slot the dragged tab will land in if the user releases now.
#[allow(dead_code)]
fn rail_drop_indicator(theme: &gpui_component::Theme, above: bool) -> Div {
    let bar = div()
        .absolute()
        .left(px(4.0))
        .right(px(4.0))
        .h(px(2.0))
        .rounded(px(1.0))
        .bg(theme.primary);
    if above {
        bar.top(px(-2.0))
    } else {
        bar.bottom(px(-2.0))
    }
}

fn point_in_bounds(p: &gpui::Point<gpui::Pixels>, b: &gpui::Bounds<gpui::Pixels>) -> bool {
    p.x >= b.origin.x
        && p.x < b.origin.x + b.size.width
        && p.y >= b.origin.y
        && p.y < b.origin.y + b.size.height
}

/// Map the cursor's y-position during a rail drag to a drop slot
/// `0..=N` where N == session_count. Slot K means "insert before
/// row K" (so slot N means "after the last row"). The user can
/// hover anywhere in the rail's vertical column — including the
/// 2-px gaps between pills, the slot above the first pill, and the
/// slot below the last pill — and get a meaningful drop target.
///
/// Splits each row's y-range in half: top half → slot K (above K),
/// bottom half → slot K+1 (below K). That's what makes the
/// "drag-to-last-position" gesture work — without the bottom-half
/// rule, the deepest reachable slot was N-1 and there was no way
/// to land below the last row.
#[allow(dead_code)]
fn rail_slot_for_cursor(
    event: &gpui::DragMoveEvent<DraggedTab>,
    session_count: usize,
    leading_top_pad: f32,
) -> Option<usize> {
    if !point_in_bounds(&event.event.position, &event.bounds) {
        return None;
    }
    let local_y = f32::from(event.event.position.y - event.bounds.origin.y);
    rail_slot_from_local_y(local_y, session_count, leading_top_pad)
}

/// On-drop fallback — GPUI gives us the cursor's window-coords
/// position here, so subtract the rail origin cached during
/// `on_drag_move` before applying the rail-local layout math.
#[allow(dead_code)]
fn rail_slot_for_cursor_position(
    cursor: gpui::Point<gpui::Pixels>,
    rail_origin_y: Option<f32>,
    session_count: usize,
    leading_top_pad: f32,
) -> Option<usize> {
    let local_y = f32::from(cursor.y) - rail_origin_y?;
    rail_slot_from_local_y(local_y, session_count, leading_top_pad)
}

#[allow(dead_code)]
fn rail_slot_from_local_y(
    local_y: f32,
    session_count: usize,
    leading_top_pad: f32,
) -> Option<usize> {
    if session_count == 0 {
        return None;
    }
    // Mirror the rail layout in render_rail:
    //   pt(leading_top_pad)
    //   + top controls (expand, files, search, new, two dividers, gaps)
    //   + scrollable session list rows
    let header = leading_top_pad + RAIL_TOP_CONTROLS_HEIGHT;
    let stride = RAIL_ICON_SIZE + RAIL_ICON_GAP;
    if local_y < header {
        return Some(0);
    }
    let offset = local_y - header;
    let row_f = offset / stride;
    let row = (row_f.floor() as i64).max(0) as usize;
    if row >= session_count {
        return Some(session_count);
    }
    // Within-row position: 0.0..1.0. < 0.5 → above this row,
    // ≥ 0.5 → below this row (= above row+1).
    let frac = row_f - row_f.floor();
    let slot = if frac < 0.5 { row } else { row + 1 };
    Some(slot.min(session_count))
}

#[allow(dead_code)]
fn rail_icon_button<F>(
    id: &'static str,
    icon: &'static str,
    icon_color: Hsla,
    theme: &gpui_component::Theme,
    offset: f32,
    handler: F,
) -> Stateful<Div>
where
    F: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    let hover_bg = theme.foreground.opacity(NAV_CONTROL_HOVER_ALPHA);
    div()
        .id(id)
        .debug_selector(move || id.into())
        .relative()
        .left(px(offset))
        .size(px(RAIL_BUTTON_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(
            div()
                .debug_selector(move || format!("{id}-icon"))
                .size(ui_icon_px(theme, 16.0))
                .flex_shrink_0()
                .child(
                    svg()
                        .path(icon)
                        .size(ui_icon_px(theme, 16.0))
                        .text_color(icon_color),
                ),
        )
        .on_mouse_down(MouseButton::Left, handler)
}

fn build_row_context_menu(
    menu: PopupMenu,
    weak: WeakEntity<SessionSidebar>,
    index: usize,
    session_id: u64,
    total: usize,
    has_user_label: bool,
    current_color: Option<con_core::session::TabAccentColor>,
) -> PopupMenu {
    use crate::tab_context_menu::{TabMenuOptions, build_tab_context_menu};
    build_tab_context_menu(
        menu,
        TabMenuOptions {
            rename: {
                let weak = weak.clone();
                Box::new(move |window, cx| {
                    if let Some(e) = weak.upgrade() {
                        e.update(cx, |this, cx| {
                            this.start_rename_by_id(session_id, window, cx)
                        });
                    }
                })
            },
            duplicate: {
                let weak = weak.clone();
                Box::new(move |_window, cx| {
                    if let Some(e) = weak.upgrade() {
                        e.update(cx, |_, cx| cx.emit(SidebarDuplicate { session_id }));
                    }
                })
            },
            reset_name: if has_user_label {
                let weak = weak.clone();
                Some(Box::new(move |_window, cx| {
                    if let Some(e) = weak.upgrade() {
                        e.update(cx, |_, cx| {
                            cx.emit(SidebarRename {
                                session_id,
                                label: None,
                                changed_by_user: true,
                            })
                        });
                    }
                }))
            } else {
                None
            },
            move_up: if index > 0 {
                let weak = weak.clone();
                Some(Box::new(move |_window, cx| {
                    if let Some(e) = weak.upgrade() {
                        e.update(cx, |_, cx| {
                            cx.emit(SidebarReorder {
                                session_id,
                                to: index - 1,
                                move_delta: Some(-1),
                            })
                        });
                    }
                }))
            } else {
                None
            },
            move_down: if index + 1 < total {
                let weak = weak.clone();
                Some(Box::new(move |_window, cx| {
                    if let Some(e) = weak.upgrade() {
                        e.update(cx, |_, cx| {
                            cx.emit(SidebarReorder {
                                session_id,
                                to: index + 2,
                                move_delta: Some(1),
                            })
                        });
                    }
                }))
            } else {
                None
            },
            close_to_right: None,
            close_tab: {
                let weak = weak.clone();
                Box::new(move |_window, cx| {
                    if let Some(e) = weak.upgrade() {
                        e.update(cx, |_, cx| cx.emit(SidebarCloseTab { session_id }));
                    }
                })
            },
            close_others: if total > 1 {
                let weak = weak.clone();
                Some(Box::new(move |_window, cx| {
                    if let Some(e) = weak.upgrade() {
                        e.update(cx, |_, cx| cx.emit(SidebarCloseOthers { session_id }));
                    }
                }))
            } else {
                None
            },
            set_color: {
                let weak = weak.clone();
                Box::new(move |color, _window, cx| {
                    if let Some(e) = weak.upgrade() {
                        e.update(cx, |_, cx| cx.emit(SidebarSetColor { session_id, color }));
                    }
                })
            },
            current_color,
        },
    )
}

fn sidebar_button_tooltip(mut button: Button, label: SharedString) -> Button {
    button.interactivity().tooltip(move |window, cx| {
        let theme = cx.theme();
        Tooltip::new(label.clone())
            .font_family(theme.font_family.clone())
            .text_size(ui_px(theme, 11.5))
            .text_color(theme.foreground)
            .bg(Hsla {
                a: 1.0,
                ..theme.popover
            })
            .border_0()
            .shadow(Vec::new())
            .rounded(px(6.0))
            .px(px(8.0))
            .py(px(4.0))
            .build(window, cx)
    });
    button
}

#[allow(clippy::too_many_arguments)]
fn navigation_button<F>(
    id: &'static str,
    icon: &'static str,
    label: SharedString,
    origin: (f32, f32),
    preview: Option<(f32, f32)>,
    color: Hsla,
    cx: &App,
    handler: F,
) -> Button
where
    F: Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
{
    let icon_size = ui_icon_px(cx.theme(), 16.0);
    let visible_label = if preview.is_some() && id == "tab-sidebar-rail-expand" {
        "Keep expanded".into()
    } else {
        label.clone()
    };
    let button = Button::new(id)
        .debug_selector(move || id.into())
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(color)
                .hover(cx.theme().foreground.opacity(NAV_CONTROL_HOVER_ALPHA))
                .active(cx.theme().foreground.opacity(NAV_CONTROL_PRESS_ALPHA)),
        )
        .small()
        .absolute()
        .left(px(origin.0))
        .top(px(origin.1))
        .size(px(RAIL_BUTTON_SIZE))
        .w(px(preview.map_or(RAIL_BUTTON_SIZE, |(width, _)| {
            (width - origin.0 - 8.0).max(RAIL_BUTTON_SIZE)
        })))
        .h(px(RAIL_BUTTON_SIZE))
        .p_0()
        .rounded(px(7.0))
        .tab_stop(false)
        .accessibility_label(label.clone())
        .child(
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .debug_selector(move || format!("{id}-icon"))
                        .absolute()
                        .left((px(RAIL_BUTTON_SIZE) - icon_size) * 0.5)
                        .top((px(RAIL_BUTTON_SIZE) - icon_size) * 0.5)
                        .size(icon_size)
                        .child(svg().path(icon).size(icon_size).text_color(color)),
                )
                .when_some(preview, |content, (_, opacity)| {
                    content.child(
                        div()
                            .absolute()
                            .left(px(RAIL_BUTTON_SIZE + 12.0))
                            .right(px(8.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .opacity(opacity)
                            .text_size(ui_px(cx.theme(), 12.5))
                            .text_color(cx.theme().muted_foreground)
                            .child(div().truncate().child(visible_label)),
                    )
                }),
        )
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(handler);
    if preview.is_some() {
        // The preview already displays the action text. A second floating label
        // obscures neighboring controls and adds an unrelated elevation layer.
        button
    } else {
        sidebar_button_tooltip(button, label)
    }
}

fn sidebar_close_button(id: SharedString, cx: &App) -> Button {
    let theme = cx.theme();
    let circle = theme
        .foreground
        .opacity(if theme.is_dark() { 0.065 } else { 0.04 });
    let hover = theme
        .foreground
        .opacity(if theme.is_dark() { 0.14 } else { 0.10 });
    let pressed = theme.foreground.opacity(0.18);
    let button = Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(theme.muted_foreground)
                .hover(theme.transparent)
                .active(theme.transparent),
        )
        .small()
        .size(px(CLOSE_TARGET_SIZE))
        .w(px(CLOSE_TARGET_SIZE))
        .h(px(CLOSE_TARGET_SIZE))
        .p_0()
        .flex_shrink_0()
        .rounded_full()
        .tab_stop(false)
        .accessibility_label("Close tab")
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id("sidebar-close-circle")
                .debug_selector(|| "sidebar-close-circle".into())
                .flex()
                .items_center()
                .justify_center()
                .size(px(CLOSE_CIRCLE_SIZE))
                .rounded_full()
                .bg(circle)
                .group_hover("tab-close-control", move |s| s.bg(hover))
                .group_active("tab-close-control", move |s| s.bg(pressed))
                .child(
                    svg()
                        .path("phosphor/x-thin.svg")
                        .size(px(8.0))
                        .text_color(theme.foreground),
                ),
        )
        .group("tab-close-control");
    sidebar_button_tooltip(button, "Close tab".into())
}

#[cfg(test)]
fn begin_rename_lifecycle(generation: &mut u64, _session_id: u64) -> RenameLifecycleStateSnapshot {
    let current_generation = *generation;
    *generation += 1;
    RenameLifecycleStateSnapshot {
        active_generation: Some(current_generation),
        cancelled_generation: None,
    }
}

#[cfg(test)]
fn cancel_rename_lifecycle(state: RenameLifecycleStateSnapshot) -> RenameLifecycleStateSnapshot {
    RenameLifecycleStateSnapshot {
        active_generation: None,
        cancelled_generation: state.active_generation,
    }
}

#[cfg(test)]
fn begin_rename_after_cancel_lifecycle(
    generation: &mut u64,
    state: RenameLifecycleStateSnapshot,
    session_id: u64,
) -> RenameLifecycleStateSnapshot {
    let next = begin_rename_lifecycle(generation, session_id);
    RenameLifecycleStateSnapshot {
        active_generation: next.active_generation,
        cancelled_generation: state.cancelled_generation,
    }
}

#[cfg(test)]
fn blur_should_commit_for_lifecycle(
    state: RenameLifecycleStateSnapshot,
    blur_generation: u64,
) -> bool {
    state.cancelled_generation != Some(blur_generation)
        && state.active_generation == Some(blur_generation)
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct SidebarDragPreviewState {
    pub label: SharedString,
    pub icon: &'static str,
    pub source_left: Pixels,
    pub cursor_offset_y: Pixels,
    /// True when the drag originated from a pane title bar. In that case the
    /// workspace floating preview already follows the cursor, so the sidebar
    /// should only show the drop indicator — not an additional overlay.
    pub is_pane_origin: bool,
}

pub fn tab_drag_preview_size(pinned: bool) -> Size<Pixels> {
    Size {
        width: if pinned { px(180.0) } else { px(RAIL_WIDTH) },
        height: if pinned { px(28.0) } else { px(RAIL_ICON_SIZE) },
    }
}

#[allow(dead_code)]
pub fn vertical_drag_overlay_origin(
    mouse: Point<Pixels>,
    preview: &SidebarDragPreviewState,
    _preview_size: Size<Pixels>,
    min_top: Pixels,
    max_top: Pixels,
) -> Point<Pixels> {
    point(
        preview.source_left,
        (mouse.y - preview.cursor_offset_y).clamp(min_top, max_top),
    )
}

#[allow(dead_code)]
pub fn vertical_drag_overlay_probe_position(
    mouse: Point<Pixels>,
    preview: &SidebarDragPreviewState,
    preview_size: Size<Pixels>,
    min_top: Pixels,
    max_top: Pixels,
) -> Point<Pixels> {
    let origin = vertical_drag_overlay_origin(mouse, preview, preview_size, min_top, max_top);
    point(
        origin.x + preview_size.width / 2.0,
        origin.y + preview_size.height / 2.0,
    )
}

#[allow(dead_code)]
fn event_probe_y_from_vertical_preview(
    mouse: Point<Pixels>,
    dragged: &DraggedTab,
    _bounds: &[Bounds<Pixels>],
) -> Pixels {
    let preview_size = tab_drag_preview_size(false);
    let cursor_offset_y = dragged
        .preview_constraint
        .map(|constraint| constraint.cursor_offset_y)
        .unwrap_or(preview_size.height / 2.0);
    mouse.y - cursor_offset_y + preview_size.height / 2.0
}

pub fn vertical_slot_from_bounds(
    probe_y: Pixels,
    bounds: &[Bounds<Pixels>],
    item_count: usize,
) -> Option<usize> {
    if item_count == 0 || bounds.is_empty() {
        return None;
    }
    let mut slot = item_count;
    for (i, bounds) in bounds.iter().enumerate() {
        let mid_y = bounds.origin.y + bounds.size.height / 2.0;
        if probe_y < mid_y {
            slot = i;
            break;
        }
    }
    Some(slot.min(item_count))
}

fn normalize_sidebar_rename_label(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tools_swap_with_pinned_navigation_at_one_extent_and_reverse_without_jumping() {
        use std::time::{Duration, Instant};
        let mut sidebar = super::SessionSidebar::default_state();
        sidebar.mode = super::PanelMode::Pinned;
        sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
        let start = Instant::now();
        sidebar.set_tools_panel_open_at(true, crate::activity_bar::ActivitySlot::Files, start);
        for ms in [0, 16, 48, 96] {
            let now = start + Duration::from_millis(ms);
            let nav = sidebar.width_motion.update_at(now, false, || {});
            let tool = sidebar.tools_motion.update_at(now, false, || {});
            assert!((nav + tool - 1.0).abs() < 0.00001);
        }
        let turn = start + Duration::from_millis(70);
        let nav = sidebar.width_motion.update_at(turn, false, || {});
        let tool = sidebar.tools_motion.update_at(turn, false, || {});
        sidebar.set_tools_panel_open_at(false, crate::activity_bar::ActivitySlot::Files, turn);
        assert!((sidebar.width_motion.update_at(turn, false, || {}) - nav).abs() < 0.00001);
        assert!((sidebar.tools_motion.update_at(turn, false, || {}) - tool).abs() < 0.00001);
        for ms in [0, 16, 48, 160, 800] {
            let now = turn + Duration::from_millis(ms);
            let nav = sidebar.width_motion.update_at(now, false, || {});
            let tool = sidebar.tools_motion.update_at(now, false, || {});
            assert!((nav + tool - 1.0).abs() < 0.00001);
        }
        assert_eq!(
            sidebar.rendered_panel_width(),
            sidebar.effective_panel_width()
        );
    }

    #[test]
    fn tools_close_to_a_stable_rail_and_rail_only_opening_does_not_jump() {
        use std::time::{Duration, Instant};
        let mut sidebar = super::SessionSidebar::default_state();
        sidebar.tools_panel_open = true;
        sidebar.tools_motion = crate::motion::SidebarMotion::new(1.0);
        let start = Instant::now();
        sidebar.set_tools_panel_open_at(false, crate::activity_bar::ActivitySlot::Files, start);
        let mut previous = 1.0;
        for ms in [0, 16, 48, 160, 800] {
            let now = start + Duration::from_millis(ms);
            assert_eq!(sidebar.width_motion.update_at(now, false, || {}), 0.0);
            let tool = sidebar.tools_motion.update_at(now, false, || {});
            assert!(tool <= previous);
            previous = tool;
        }
        assert_eq!(sidebar.rendered_panel_width(), super::RAIL_WIDTH);
        sidebar.mode = super::PanelMode::Pinned;
        sidebar.rail_only_override = true;
        sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
        let start = Instant::now() + Duration::from_secs(1);
        sidebar.set_tools_panel_open_at(true, crate::activity_bar::ActivitySlot::Files, start);
        assert_eq!(sidebar.rendered_panel_width(), super::RAIL_WIDTH);
        assert!(!sidebar.can_peek());
        sidebar.width_motion.update_at(start, true, || {});
        sidebar.tools_motion.update_at(start, true, || {});
        assert_eq!(
            sidebar.rendered_panel_width(),
            sidebar.effective_panel_width()
        );
    }

    #[test]
    fn tool_content_fades_reverse_continuously_and_skip_hidden_or_reduced_motion() {
        use crate::activity_bar::ActivitySlot;
        use std::time::{Duration, Instant};
        let mut sidebar = super::SessionSidebar::default_state();
        let start = Instant::now();
        sidebar.target_tool_slot_at(ActivitySlot::Search, true, start);
        assert_eq!(sidebar.tool_slot_motion.current_at(start), 1.0);
        let start = start + Duration::from_secs(1);
        sidebar.target_tool_slot_at(ActivitySlot::Files, true, start);
        let turn = start + Duration::from_millis(40);
        let value = sidebar.tool_slot_motion.current_at(turn);
        assert!(value > 0.0 && value < 1.0);
        sidebar.target_tool_slot_at(ActivitySlot::Search, true, turn);
        assert_eq!(sidebar.tool_slot_motion.current_at(turn), value);
        assert!(
            sidebar
                .tool_slot_motion
                .current_at(turn + Duration::from_millis(10))
                > value
        );
        assert_eq!(sidebar.tool_slot_motion.update_at(turn, true, || {}), 1.0);
        sidebar.target_tool_slot_at(ActivitySlot::Files, false, turn);
        assert_eq!(sidebar.tool_slot_motion.current_at(turn), 0.0);
        assert!(!sidebar.tool_slot_motion.is_animating());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn caption_reaches_its_endpoint_with_navigation_and_never_overhangs_a_rail() {
        let mut sidebar = super::SessionSidebar::default_state();
        sidebar.mode = super::PanelMode::Pinned;
        for width in [184.0, 230.0, 320.0] {
            sidebar.panel_width = width;
            for progress in [0.0, 0.5, 0.9, 1.0, 0.3, 1.0] {
                sidebar.width_motion = crate::motion::SidebarMotion::new(progress);
                for visibility in [0.0, 0.5, 0.9, 1.0] {
                    let dock = sidebar.caption_dock_progress(visibility, 168.0);
                    let visible_width = sidebar.rendered_width() * visibility;
                    assert!((0.0..=1.0).contains(&dock));
                    if dock > 0.0 {
                        assert!(visible_width > 168.0);
                    }
                    if progress == 1.0 && visibility == 1.0 {
                        assert_eq!(dock, 1.0, "caption waited for an independent animation");
                    }
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn caption_uses_the_effective_panel_not_the_saved_expanded_preference(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        let sidebar = cx.new(super::SessionSidebar::new);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.set_pinned(true, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
            assert_eq!(sidebar.caption_dock_progress(1.0, 168.0), 1.0);
            for visibility in [0.0, 0.5, 0.7] {
                assert_eq!(sidebar.caption_dock_progress(visibility, 168.0), 0.0);
            }
            assert!(sidebar.caption_dock_progress(0.9, 168.0) > 0.0);
            sidebar.show_rail_only_preserving_pinned(cx);
            assert!(sidebar.is_pinned());
            assert_eq!(sidebar.caption_dock_progress(1.0, 168.0), 0.0);
            sidebar.clear_rail_only_override(cx);
            assert_eq!(sidebar.caption_dock_progress(1.0, 168.0), 1.0);
            sidebar.set_tools_panel_open(true, crate::activity_bar::ActivitySlot::Files);
            // This test checks endpoint policy. Clocked complementary-channel
            // continuity is covered by tools_swap_with_pinned_navigation.
            sidebar.width_motion = crate::motion::SidebarMotion::new(0.0);
            sidebar.tools_motion = crate::motion::SidebarMotion::new(1.0);
            assert_eq!(sidebar.caption_dock_progress(1.0, 168.0), 1.0);
            sidebar.set_tools_panel_open(false, crate::activity_bar::ActivitySlot::Files);
            sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
            sidebar.tools_motion = crate::motion::SidebarMotion::new(0.0);
            sidebar.set_pinned(false, cx);
            assert!(
                sidebar.caption_dock_progress(1.0, 168.0) > 0.99,
                "the outgoing expanded layout still has room for the caption"
            );
            sidebar.width_motion = crate::motion::SidebarMotion::new(0.0);
            assert_eq!(sidebar.caption_dock_progress(1.0, 168.0), 0.0);
            sidebar.set_pinned(true, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(0.0);
            assert_eq!(sidebar.caption_dock_progress(1.0, 168.0), 0.0);
        });
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn tool_columns_dock_the_caption_by_combined_width_not_rail_width(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        let sidebar = cx.new(super::SessionSidebar::new);
        sidebar.update(cx, |sidebar, _| {
            for slot in [
                crate::activity_bar::ActivitySlot::Files,
                crate::activity_bar::ActivitySlot::Search,
            ] {
                sidebar.set_tools_panel_open(true, slot);
                sidebar.tools_motion = crate::motion::SidebarMotion::new(1.0);
                assert_eq!(sidebar.rendered_width(), super::RAIL_WIDTH);
                assert_eq!(sidebar.caption_dock_progress(1.0, 168.0), 1.0);
                assert_eq!(sidebar.caption_dock_progress(0.5, 168.0), 0.0);
            }
            sidebar.set_tools_panel_open(false, crate::activity_bar::ActivitySlot::Search);
            sidebar.tools_motion = crate::motion::SidebarMotion::new(0.0);
            assert_eq!(
                sidebar.caption_dock_progress(1.0, 168.0),
                0.0,
                "a rail alone still needs the global caption"
            );
        });
    }

    #[gpui::test]
    fn collapsing_keeps_outgoing_panel_extent_until_motion_settles(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        let sidebar = cx.new(super::SessionSidebar::new);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.set_pinned(true, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
            let expanded = sidebar.rendered_width();
            sidebar.set_pinned(false, cx);
            assert!(sidebar.rendered_width() > super::RAIL_WIDTH);
            assert!(sidebar.rendered_width() <= expanded);
            assert!(!sidebar.is_pinned());
            sidebar.width_motion = crate::motion::SidebarMotion::new(0.0);
            assert_eq!(sidebar.rendered_width(), super::RAIL_WIDTH);
        });
    }

    use super::{
        PanelMode, RAIL_ICON_SIZE, RAIL_TOP_CONTROLS_HEIGHT, RailModeButtonAction, SessionSidebar,
        SidebarDragPreviewState, begin_rename_after_cancel_lifecycle, begin_rename_lifecycle,
        blur_should_commit_for_lifecycle, cancel_rename_lifecycle, normalize_sidebar_rename_label,
        rail_slot_from_local_y, vertical_drag_overlay_probe_position, vertical_slot_from_bounds,
    };
    use gpui::{Bounds, InteractiveElement, ParentElement, Point, Size, Styled, px};
    use gpui_component::ActiveTheme;
    use std::time::Duration;

    struct HoverCardTestView(gpui::Entity<SessionSidebar>);

    struct NavigationPeekTestView(gpui::Entity<SessionSidebar>);

    impl gpui::Render for NavigationPeekTestView {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let width = self.0.read(cx).rendered_width();
            let bg = cx.theme().background;
            let mut root = gpui::div()
                .relative()
                .flex()
                .w(px(700.0))
                .h(window.viewport_size().height)
                .child(gpui::div().w(px(width)).h_full().child(self.0.clone()))
                .child(
                    gpui::div()
                        .debug_selector(|| "peek-test-terminal".into())
                        .flex_1()
                        .h_full(),
                );
            if let Some(overlay) = self.0.update(cx, |sidebar, cx| {
                sidebar.render_navigation_peek_overlay(0.0, bg, window, cx)
            }) {
                root = root.child(overlay);
            }
            root
        }
    }

    #[gpui::test]
    fn pinning_hover_animates_icons_even_when_preview_width_is_complete(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        use std::time::Instant;
        cx.update(gpui_component::init);
        let sidebar = cx.new(|cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = vec![session(None, None)];
            sidebar.peek_open = true;
            sidebar.peek_motion = crate::motion::SidebarMotion::new(1.0);
            sidebar
        });
        let (_, cx) = cx.add_window_view(|_, cx| {
            cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
            NavigationPeekTestView(sidebar.clone())
        });
        let compact_icon = cx.debug_bounds("tab-sidebar-rail-files-icon").unwrap();
        let preview_width = cx
            .debug_bounds("sidebar-navigation-peek")
            .unwrap()
            .size
            .width;
        sidebar.update(cx, |sidebar, cx| sidebar.set_pinned(true, cx));
        let start_icon = cx.debug_bounds("tab-sidebar-rail-files-icon").unwrap();
        assert!((start_icon.top() - compact_icon.top()).abs() < px(2.0));
        assert_eq!(
            sidebar.read_with(cx, |s, _| px(s.rendered_width())),
            preview_width
        );
        sidebar.update(cx, |sidebar, cx| {
            let mut motion = crate::motion::SidebarMotion::new(0.0);
            motion.retarget_at(1.0, Instant::now() - Duration::from_millis(48));
            sidebar.peek_pin_layout_motion = Some(motion);
            cx.notify();
        });
        let intermediate = cx.debug_bounds("tab-sidebar-rail-files-icon").unwrap();
        assert!(intermediate.top() < start_icon.top() - px(2.0));
        assert!(intermediate.top() > px(16.0));
        sidebar.update(cx, |sidebar, cx| sidebar.set_pinned(false, cx));
        let reversed = cx.debug_bounds("tab-sidebar-rail-files-icon").unwrap();
        assert!((reversed.top() - intermediate.top()).abs() < px(2.0));
        cx.update(|_, cx| cx.set_reduce_motion(true));
        sidebar.update(cx, |_, cx| cx.notify());
        assert_eq!(
            sidebar.read_with(cx, |s, _| s.rendered_width()),
            super::RAIL_WIDTH
        );
        sidebar.update(cx, |sidebar, cx| {
            sidebar.peek_open = true;
            sidebar.peek_motion = crate::motion::SidebarMotion::new(1.0);
            sidebar.set_pinned(true, cx);
        });
        let reduced = cx.debug_bounds("tab-sidebar-rail-files-icon").unwrap();
        let mode = cx.debug_bounds("tab-sidebar-rail-expand-icon").unwrap();
        assert_eq!(reduced.top(), mode.top());
        assert!(sidebar.read_with(cx, |s, _| s.peek_pin_layout_motion.is_none()));
    }

    #[gpui::test]
    fn navigation_hover_previews_all_tabs_without_reserving_terminal_width(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        cx.update(gpui_component::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let sidebar = cx.new(|cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = (0..60)
                .map(|id| {
                    let mut entry = session(None, None);
                    entry.id = id;
                    entry.name = format!("Task {id}");
                    entry
                })
                .collect();
            sidebar
        });
        let (view, cx) = cx.add_window_view(|_, cx| {
            cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
            NavigationPeekTestView(sidebar.clone())
        });
        sidebar.update(cx, |sidebar, cx| {
            sidebar
                .rail_scroll
                .set_offset(gpui::point(px(0.0), px(-84.0)));
            cx.notify();
        });
        let terminal = cx.debug_bounds("peek-test-terminal").unwrap();
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.rail_scroll.offset().y) < px(0.0));
        let row_origins = sidebar.read_with(cx, |sidebar, _| {
            sidebar
                .tab_bounds
                .borrow()
                .iter()
                .map(|bounds| bounds.top())
                .collect::<Vec<_>>()
        });
        let files_icon = cx.debug_bounds("tab-sidebar-rail-files-icon").unwrap();
        sidebar.update(cx, |sidebar, cx| sidebar.set_navigation_hover(true, cx));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(179));
        cx.run_until_parked();
        assert!(cx.debug_bounds("sidebar-navigation-peek").is_none());
        cx.executor().advance_clock(Duration::from_millis(2));
        cx.run_until_parked();
        assert!(cx.debug_bounds("sidebar-navigation-peek").unwrap().right() > terminal.left());
        assert_eq!(cx.debug_bounds("peek-test-terminal").unwrap(), terminal);
        sidebar.read_with(cx, |sidebar, _| {
            assert!(!sidebar.is_pinned());
            assert_eq!(sidebar.rendered_width(), super::RAIL_WIDTH);
            assert_eq!(sidebar.tab_bounds.borrow().len(), row_origins.len());
            assert_eq!(
                sidebar
                    .tab_bounds
                    .borrow()
                    .iter()
                    .map(|bounds| bounds.top())
                    .collect::<Vec<_>>(),
                row_origins
            );
        });
        assert_eq!(
            cx.debug_bounds("tab-sidebar-rail-files-icon").unwrap(),
            files_icon
        );
        // Crossing from rail into preview cancels the delayed close.
        sidebar.update(cx, |sidebar, cx| sidebar.set_navigation_hover(false, cx));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        sidebar.update(cx, |sidebar, cx| sidebar.set_navigation_hover(true, cx));
        cx.executor().advance_clock(Duration::from_millis(240));
        cx.run_until_parked();
        assert!(cx.debug_bounds("sidebar-navigation-peek").is_some());
        sidebar.update(cx, |sidebar, cx| sidebar.set_navigation_hover(false, cx));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(221));
        cx.run_until_parked();
        assert!(cx.debug_bounds("sidebar-navigation-peek").is_none());
        assert_eq!(cx.debug_bounds("peek-test-terminal").unwrap(), terminal);
        view.update(cx, |_, cx| cx.notify());
    }

    impl gpui::Render for HoverCardTestView {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{ParentElement, Styled};
            gpui::div()
                .size_full()
                .children(self.0.update(cx, |sidebar, cx| {
                    sidebar.render_hover_card_overlay(window, cx)
                }))
        }
    }

    struct TabDetailsTestView {
        session: super::SessionEntry,
        width: f32,
    }

    impl gpui::Render for TabDetailsTestView {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{InteractiveElement, Styled};
            super::sidebar_hover_content(&self.session, window, cx)
                .debug_selector(|| "details-content".into())
                .w(px(self.width))
        }
    }

    #[gpui::test]
    fn shared_hover_details_wrap_long_context_without_losing_metadata(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        for mode in [
            gpui_component::ThemeMode::Light,
            gpui_component::ThemeMode::Dark,
        ] {
            for (width, font_size) in [(232.0, 16.0), (304.0, 16.0), (304.0, 24.0)] {
                let (_, view) = cx.add_window_view(|_, cx| {
                    gpui_component::Theme::change(mode, None, cx);
                    gpui_component::Theme::update(cx, |theme| theme.font_size = px(font_size));
                    let mut entry = session(
                        Some(con_core::terminal_status::Status {
                            surface_id: 1,
                            activity: con_core::terminal_status::Activity::Busy,
                            evidence: con_core::terminal_status::Evidence::ProgramStatus,
                            percent: None,
                        }),
                        None,
                    );
                    entry.status_detail = Some(super::ProgramStatusDetail {
                        content: con_core::program_status::ProgramDetail {
                            application: Some("opencode".into()),
                            scope: Some("ses_e8706effeCn9UcgQrtNYJXJ".into()),
                            status: "Working".into(),
                            context: Some(
                                "Inspect completion configuration and shell paths".into(),
                            ),
                        },

                        source_path: None,
                    });
                    entry.name = "Terminal configuration and shell completion verification".into();
                    entry.subtitle = Some("~/dev/项目/nowledge-labs-website/nowledge-mem".into());
                    TabDetailsTestView {
                        session: entry,
                        width,
                    }
                });
                let content = view.debug_bounds("details-content").unwrap();
                let surface = view.debug_bounds("tab-hover-summary").unwrap();
                assert_eq!(surface.left(), content.left());
                assert_eq!(surface.right(), content.right());
                let context = view.debug_bounds("tab-hover-context").unwrap();
                assert!(surface.bottom() <= context.top());
                let heading = view.debug_bounds("tab-hover-heading").unwrap();
                let name = view.debug_bounds("tab-hover-name").unwrap();
                let path = view.debug_bounds("tab-hover-path").unwrap();
                let status = view.debug_bounds("tab-hover-status").unwrap();
                assert!(name.size.height > px(18.0), "full long title must wrap");
                assert!(status.top() >= heading.top());
                assert!(status.bottom() <= heading.bottom());
                assert!(status.bottom() < path.top());
                for bounds in [heading, path, status] {
                    assert!(bounds.left() >= content.left() + px(12.0));
                    assert!(bounds.right() <= content.right() - px(12.0));
                    assert!(bounds.bottom() <= content.bottom() - px(12.0));
                }
            }
        }
    }

    #[gpui::test]
    fn hover_details_anchor_to_the_tile_and_omit_redundant_single_pane_metadata(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            use gpui::AppContext;
            HoverCardTestView(cx.new(|cx| {
                let mut sidebar = SessionSidebar::new(cx);
                sidebar.hovered_rail = Some(0);
                sidebar.sessions = vec![session(None, None)];
                sidebar.tab_bounds.borrow_mut().push(Bounds::new(
                    gpui::point(px(8.0), px(300.0)),
                    gpui::size(px(40.0), px(40.0)),
                ));
                sidebar
            }))
        });
        let before = cx.debug_bounds("tab-hover-card").unwrap();
        assert_eq!(before.top(), px(300.0));
        assert!(cx.debug_bounds("tab-hover-meta").is_none());
        cx.simulate_mouse_move(
            gpui::point(px(22.0), px(332.0)),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        view.update(cx, |_, cx| cx.notify());
        assert_eq!(cx.debug_bounds("tab-hover-card").unwrap(), before);
    }

    #[gpui::test]
    fn hover_card_keeps_title_path_and_status_inside_balanced_padding(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let (_, cx) = cx.add_window_view(|_, cx| {
            use gpui::AppContext;
            HoverCardTestView(cx.new(|cx| {
                let mut sidebar = SessionSidebar::new(cx);
                sidebar.hovered_rail = Some(0);
                sidebar.sessions = vec![super::SessionEntry {
                    id: 0,
                    name: "Remote build".into(),
                    subtitle: Some(
                        "/project/a-long-parent-directory/another-long-directory/server".into(),
                    ),
                    is_ssh: true,
                    needs_attention: true,
                    status: Some(con_core::terminal_status::Status {
                        surface_id: 1,
                        activity: con_core::terminal_status::Activity::NeedsInput,
                        evidence: con_core::terminal_status::Evidence::ProgramStatus,
                        percent: None,
                    }),
                    status_detail: Some(super::ProgramStatusDetail {
                        source_path: Some(
                            "/project/a-long-parent-directory/another-long-directory/server".into(),
                        ),
                        ..test_status_detail("Waiting for permission · 40%".into())
                    }),

                    icon: "phosphor/terminal.svg",
                    pane_icons: Vec::new(),
                    pane_details: Vec::new(),
                    native_session_hint: None,
                    has_user_label: true,
                    pane_count: 1,
                    color: None,
                }];
                sidebar
            }))
        });
        let card = cx.debug_bounds("tab-hover-card").expect("hover card");
        let heading = cx.debug_bounds("tab-hover-heading").expect("heading");
        let name = cx.debug_bounds("tab-hover-name").expect("name");
        let path = cx.debug_bounds("tab-hover-path").expect("path");
        let status = cx.debug_bounds("tab-hover-status").expect("status");
        let meta = cx.debug_bounds("tab-hover-meta").expect("metadata");
        assert_eq!(heading.left() - card.left(), px(12.0));
        assert_eq!(card.right() - heading.right(), px(12.0));
        assert_eq!(heading.top() - card.top(), px(12.0));
        assert_eq!(card.bottom() - meta.bottom(), px(12.0));
        assert!(name.right() <= status.left());
        assert!(status.top() >= heading.top());
        assert!(status.bottom() < path.top());
        assert!(path.bottom() <= meta.top());
        assert!(status.right() <= heading.right());
        assert_eq!(path.left(), name.left());
        assert!(
            path.size.height > px(16.0),
            "long paths wrap instead of losing their tail"
        );
    }

    #[gpui::test]
    fn hover_status_tracks_ui_font_in_light_and_dark(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            use gpui::AppContext;
            HoverCardTestView(cx.new(|cx| {
                let mut sidebar = SessionSidebar::new(cx);
                sidebar.hovered_rail = Some(0);
                sidebar.sessions = vec![session(
                    Some(con_core::terminal_status::Status {
                        surface_id: 1,
                        activity: con_core::terminal_status::Activity::Busy,
                        evidence: con_core::terminal_status::Evidence::ProgramStatus,
                        percent: None,
                    }),
                    Some("Working".into()),
                )];
                sidebar
            }))
        });
        for mode in [
            gpui_component::ThemeMode::Light,
            gpui_component::ThemeMode::Dark,
        ] {
            cx.update(|_, cx| {
                gpui_component::Theme::change(mode, None, cx);
            });
            let mut heights = Vec::new();
            for (font_size, expected_height) in [(12.0, 14.4), (16.0, 16.0), (24.0, 21.6)] {
                cx.update(|_, cx| {
                    gpui_component::Theme::update(cx, |theme| theme.font_size = px(font_size));
                });
                view.update(cx, |_, cx| cx.notify());
                let status = cx.debug_bounds("tab-hover-status").expect("status marker");
                assert!((status.size.height.as_f32() - expected_height).abs() < 0.5);
                let path = cx.debug_bounds("tab-hover-path").expect("path");
                assert!(status.bottom() < path.top());
                assert!(cx.debug_bounds("tab-hover-meta").is_none());
                heights.push(status.size.height.as_f32());
            }
            assert!(heights[0] < heights[1] && heights[1] < heights[2]);
        }
    }

    #[gpui::test]
    fn multi_pane_hover_exposes_every_panes_own_task_and_directory(cx: &mut gpui::TestAppContext) {
        use con_core::terminal_status::{Activity, Evidence, Status};
        cx.update(gpui_component::init);
        for mode in [
            gpui_component::ThemeMode::Light,
            gpui_component::ThemeMode::Dark,
        ] {
            for count in [2, 6] {
                let (_, cx) = cx.add_window_view(|_, cx| {
                    gpui_component::Theme::change(mode, None, cx);
                    let mut session = session(None, None);
                    session.pane_count = count;
                    session.pane_details = (0..count)
                        .map(|id| super::PaneHoverDetail {
                            id: id as u64,
                            icon: if id == 0 {
                                "agents/codex.svg"
                            } else {
                                "phosphor/terminal.svg"
                            },
                            title: if id == 0 {
                                "Reviewing terminal chrome".into()
                            } else {
                                format!("service-{id}")
                            },
                            directory: Some(format!("/project/service-{id}")),
                            status: Some(Status {
                                surface_id: id as u64,
                                activity: if id == 0 {
                                    Activity::NeedsInput
                                } else {
                                    Activity::Busy
                                },
                                evidence: Evidence::ProgramStatus,
                                percent: None,
                            }),
                            status_label: if id == 0 {
                                "Waiting for permission".into()
                            } else {
                                "Working".into()
                            },
                        })
                        .collect();
                    TabDetailsTestView {
                        session,
                        width: 304.0,
                    }
                });
                let mut previous = cx.debug_bounds("tab-hover-heading").unwrap();
                for id in 0..count {
                    let row = cx
                        .debug_bounds(
                            [
                                "pane-hover-row-0",
                                "pane-hover-row-1",
                                "pane-hover-row-2",
                                "pane-hover-row-3",
                                "pane-hover-row-4",
                                "pane-hover-row-5",
                            ][id],
                        )
                        .expect("every physical pane has a readable row");
                    let title = cx
                        .debug_bounds(
                            [
                                "pane-hover-title-0",
                                "pane-hover-title-1",
                                "pane-hover-title-2",
                                "pane-hover-title-3",
                                "pane-hover-title-4",
                                "pane-hover-title-5",
                            ][id],
                        )
                        .unwrap();
                    let path = cx
                        .debug_bounds(
                            [
                                "pane-hover-path-0",
                                "pane-hover-path-1",
                                "pane-hover-path-2",
                                "pane-hover-path-3",
                                "pane-hover-path-4",
                                "pane-hover-path-5",
                            ][id],
                        )
                        .unwrap();
                    assert!(previous.bottom() <= row.top());
                    assert!(title.bottom() <= path.top());
                    assert!(path.right() <= row.right());
                    previous = row;
                }
                assert!(cx.debug_bounds("tab-hover-pane-count").is_none());
                assert!(cx.debug_bounds("tab-hover-meta").is_none());
                cx.update(|window, _| window.activate_window());
                assert!(
                    cx.update(|window, cx| window.simulate_next_frame(cx)) > 0,
                    "multiple working panes keep their shared activity layer alive"
                );
            }
        }
    }

    #[gpui::test]
    fn open_hover_details_follow_live_status_updates(cx: &mut gpui::TestAppContext) {
        use con_core::terminal_status::{Activity, Evidence, Status};
        use gpui::AppContext;
        cx.update(gpui_component::init);
        let sidebar = cx.new(|cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = vec![session(
                Some(Status {
                    surface_id: 1,
                    activity: Activity::Busy,
                    evidence: Evidence::ProgramStatus,
                    percent: None,
                }),
                Some("Working".into()),
            )];
            sidebar
        });
        let id = sidebar.read_with(cx, |sidebar, _| sidebar.sessions[0].id);
        let (_, cx) = cx.add_window_view(|_, cx| {
            super::SidebarHoverDetails::new(sidebar.downgrade(), id, 304.0, cx)
        });
        assert!(cx.debug_bounds("tab-hover-status").is_some());
        assert!(cx.debug_bounds("tab-status-glyph").is_none());
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sessions[0].status.as_mut().unwrap().activity = Activity::NeedsInput;
            cx.notify();
        });
        assert!(
            cx.debug_bounds("tab-status-glyph").is_some(),
            "open tooltip must follow attention updates"
        );
        sidebar.update(cx, |sidebar, cx| {
            sidebar.sessions[0].status.as_mut().unwrap().activity = Activity::Done;
            cx.notify();
        });
        assert!(cx.debug_bounds("tab-status-glyph").is_none());
        assert!(
            cx.debug_bounds("tab-notification-dot").is_some(),
            "completion must replace the previous graphic"
        );
    }

    #[gpui::test]
    fn hover_status_transitions_keep_the_header_stable(cx: &mut gpui::TestAppContext) {
        use con_core::terminal_status::{Activity, Evidence, Status};
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, _| TabDetailsTestView {
            session: session(
                Some(Status {
                    surface_id: 1,
                    activity: Activity::Busy,
                    evidence: Evidence::ProgramStatus,
                    percent: None,
                }),
                Some("Working".into()),
            ),
            width: 304.0,
        });
        let initial = cx.debug_bounds("tab-hover-status").expect("busy marker");
        let name = cx.debug_bounds("tab-hover-name").expect("name");
        for reduced in [false, true] {
            cx.update(|_, cx| cx.set_reduce_motion(reduced));
            for activity in [
                Activity::Busy,
                Activity::NeedsInput,
                Activity::Paused,
                Activity::Error,
                Activity::Done,
                Activity::Idle,
            ] {
                view.update(cx, |view, cx| {
                    view.session.status.as_mut().unwrap().activity = activity;
                    cx.notify();
                });
                assert_eq!(cx.debug_bounds("tab-hover-status").unwrap(), initial);
                assert_eq!(cx.debug_bounds("tab-hover-name").unwrap(), name);
                assert!(cx.debug_bounds("tab-hover-progress").is_none());
            }
        }
        view.update(cx, |view, cx| {
            let status = view.session.status.as_mut().unwrap();
            status.activity = Activity::Busy;
            status.percent = Some(37);
            cx.notify();
        });
        let marker = cx.debug_bounds("tab-hover-status").unwrap();
        let progress = cx
            .debug_bounds("tab-hover-progress")
            .expect("determinate progress");
        assert_eq!(marker.size, initial.size);
        assert!(marker.right() <= progress.left());
    }

    #[gpui::test]
    fn sidebar_status_icon_follows_density_and_font(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = vec![session(
                Some(con_core::terminal_status::Status {
                    surface_id: 1,
                    activity: con_core::terminal_status::Activity::NeedsInput,
                    evidence: con_core::terminal_status::Evidence::ProgramStatus,
                    percent: None,
                }),
                Some("Waiting for permission".into()),
            )];
            sidebar
        });

        let slot = cx.debug_bounds("tab-status-slot").expect("compact slot");
        assert_eq!(slot.size, Size::new(px(32.0), px(32.0)));
        assert!(cx.debug_bounds("tab-status-glyph").is_some());
        assert!(cx.debug_bounds("tab-status-brand").is_none());
        assert!(rendered_rings(&view, cx).is_empty());

        view.update(cx, |sidebar, cx| {
            sidebar.set_pinned(true, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
        });
        for (font_size, expected) in [(12.0, 14.4), (16.0, 16.0), (24.0, 21.6)] {
            cx.update(|_, cx| {
                gpui_component::Theme::update(cx, |theme| theme.font_size = px(font_size));
            });
            view.update(cx, |_, cx| cx.notify());
            let slot = cx.debug_bounds("tab-status-slot").expect("expanded slot");
            assert!(
                (slot.size.width.as_f32() - expected).abs() < 0.5,
                "font {font_size}: icon slot {} expected {}",
                slot.size.width.as_f32(),
                expected
            );
            assert!(cx.debug_bounds("tab-status-glyph").is_some());
            assert!(rendered_rings(&view, cx).is_empty());
        }

        view.update(cx, |sidebar, cx| {
            sidebar.sessions[0].status = Some(con_core::terminal_status::Status {
                surface_id: 1,
                activity: con_core::terminal_status::Activity::Busy,
                evidence: con_core::terminal_status::Evidence::ProgramStatus,
                percent: Some(40),
            });
            cx.notify();
        });
        let rings = rendered_rings(&view, cx);
        assert_eq!(rings.len(), 1);
        let slot = cx.debug_bounds("tab-status-slot").unwrap();
        assert_eq!(rings[0].size, Size::new(px(18.0), px(18.0)));
        assert!((slot.center().x - rings[0].center().x).abs() < px(0.5));
        assert!((slot.center().y - rings[0].center().y).abs() < px(0.5));
        assert!(cx.debug_bounds("tab-status-glyph").is_none());
        assert!(cx.debug_bounds("tab-status-brand").is_none());

        view.update(cx, |sidebar, cx| {
            sidebar.set_pinned(false, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(0.0);
        });
        assert_eq!(
            cx.debug_bounds("tab-status-slot")
                .expect("compact busy slot")
                .size,
            Size::new(px(32.0), px(32.0))
        );
        let rings = rendered_rings(&view, cx);
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].size, Size::new(px(24.0), px(24.0)));
        let slot = cx.debug_bounds("tab-status-slot").unwrap();
        assert!((slot.center().x - rings[0].center().x).abs() < px(0.5));
        assert!((slot.center().y - rings[0].center().y).abs() < px(0.5));
        assert!(cx.debug_bounds("tab-status-brand").is_some());
        assert!(cx.debug_bounds("tab-status-glyph").is_none());
    }

    fn rendered_rings(
        view: &gpui::Entity<SessionSidebar>,
        cx: &mut gpui::VisualTestContext,
    ) -> Vec<Bounds<gpui::Pixels>> {
        view.update(cx, |sidebar, cx| {
            sidebar
                .activity_layer
                .as_ref()
                .unwrap()
                .read(cx)
                .rendered_ring_bounds()
        })
    }

    fn test_status_detail(status: String) -> super::ProgramStatusDetail {
        super::ProgramStatusDetail {
            content: con_core::program_status::ProgramDetail {
                application: None,
                scope: None,
                status,
                context: None,
            },

            source_path: None,
        }
    }

    fn session(
        status: Option<con_core::terminal_status::Status>,
        status_detail: Option<String>,
    ) -> super::SessionEntry {
        super::SessionEntry {
            id: 0,
            name: "Build".into(),
            subtitle: Some("/project".into()),
            is_ssh: false,
            needs_attention: false,
            status,
            status_detail: status_detail.map(test_status_detail),

            icon: "phosphor/terminal.svg",
            pane_icons: Vec::new(),
            pane_details: Vec::new(),
            native_session_hint: None,
            has_user_label: true,
            pane_count: 1,
            color: None,
        }
    }

    #[gpui::test]
    fn navigation_balances_the_terminal_gutter_in_both_modes(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut sidebar = SessionSidebar::new(cx);
            let mut entry = session(None, None);
            entry.pane_icons = vec!["tab-terminal-refined.png", "tab-code.png", "tab-file.png"];
            sidebar.sessions = vec![entry];
            sidebar.set_terminal_gutter(5.0);
            sidebar
        });
        let selection = cx.debug_bounds("sidebar-session-selection").unwrap();
        let tile = cx.debug_bounds("sidebar-session-tile").unwrap();
        let control = cx.debug_bounds("tab-sidebar-rail-files").unwrap();
        let icon = cx.debug_bounds("tab-status-slot").unwrap();
        let pane_edge = px(super::RAIL_WIDTH + 5.0);
        assert_eq!(selection.left(), pane_edge - selection.right());
        assert_eq!(control.center().x, selection.center().x);
        assert_eq!(icon.center().x, selection.center().x);
        assert_eq!(tile.size, gpui::size(px(40.0), px(40.0)));
        assert!(tile.left() >= px(0.0) && tile.right() <= px(super::RAIL_WIDTH));
        assert!(selection.right() < px(super::RAIL_WIDTH));

        // The interpolated panel must meet the corrected rail at its compact
        // endpoint, while the expanded rows share the same balanced margins.
        view.update(cx, |sidebar, cx| {
            sidebar.set_pinned(true, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(0.0);
        });
        assert_eq!(
            cx.debug_bounds("sidebar-session-surface")
                .unwrap()
                .center()
                .x,
            selection.center().x
        );
        assert_eq!(
            cx.debug_bounds("tab-sidebar-rail-files")
                .unwrap()
                .center()
                .x,
            control.center().x
        );
        for expansion in [0.25, 0.5, 0.75, 1.0] {
            view.update(cx, |sidebar, cx| {
                sidebar.width_motion = crate::motion::SidebarMotion::new(expansion);
                sidebar.set_terminal_gutter(0.0);
                cx.notify();
            });
            let original = cx.debug_bounds("tab-sidebar-rail-files").unwrap();
            let width = view.read_with(cx, |sidebar, _| sidebar.rendered_width());
            view.update(cx, |sidebar, cx| {
                sidebar.set_terminal_gutter(5.0);
                cx.notify();
            });
            let adjusted = cx.debug_bounds("tab-sidebar-rail-files").unwrap();
            assert!((adjusted.left() - original.left() - px(2.5)).abs() <= px(0.5));
            assert_eq!(adjusted.size, original.size);
            assert_eq!(
                view.read_with(cx, |sidebar, _| sidebar.rendered_width()),
                width
            );
        }
        let expanded_name = cx.debug_bounds("sidebar-session-name").unwrap();
        view.update(cx, |sidebar, cx| {
            sidebar.set_terminal_gutter(0.0);
            cx.notify();
        });
        assert!(
            (expanded_name.left()
                - cx.debug_bounds("sidebar-session-name").unwrap().left()
                - px(2.5))
            .abs()
                <= px(0.5)
        );

        view.update(cx, |sidebar, cx| {
            sidebar.set_terminal_gutter(5.0);
            sidebar.set_tools_panel_open(true, crate::activity_bar::ActivitySlot::Files);
            cx.notify();
        });
        assert_eq!(
            cx.debug_bounds("tab-sidebar-rail-files")
                .unwrap()
                .center()
                .x,
            px(super::RAIL_WIDTH * 0.5)
        );
    }

    #[gpui::test]
    fn selection_is_inset_without_shrinking_drag_targets(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = vec![session(None, None)];
            sidebar
        });
        let tile = cx.debug_bounds("sidebar-session-tile").unwrap();
        let selection = cx.debug_bounds("sidebar-session-selection").unwrap();
        let icon = cx.debug_bounds("tab-status-brand").unwrap();
        assert_eq!(tile.size, Size::new(px(40.0), px(40.0)));
        assert_eq!(selection.size, Size::new(px(32.0), px(32.0)));
        assert_eq!(tile.center(), selection.center());
        assert_eq!(tile.center(), icon.center());
        assert_eq!(icon.size, Size::new(px(18.0), px(18.0)));
        view.read_with(cx, |sidebar, _| {
            assert_eq!(sidebar.tab_bounds.borrow()[0], tile);
        });

        view.update(cx, |sidebar, cx| {
            sidebar.set_pinned(true, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
        });
        for mode in [
            gpui_component::ThemeMode::Light,
            gpui_component::ThemeMode::Dark,
        ] {
            cx.update(|_, cx| gpui_component::Theme::change(mode, None, cx));
            for font_size in [12.0, 16.0, 24.0] {
                cx.update(|_, cx| {
                    gpui_component::Theme::update(cx, |theme| theme.font_size = px(font_size));
                });
                view.update(cx, |_, cx| cx.notify());
                let row = cx.debug_bounds("sidebar-session-row").unwrap();
                let name = cx.debug_bounds("sidebar-session-name").unwrap();
                assert!(row.top() < name.top());
                assert!((row.center().y - name.center().y).abs() <= px(0.5));
                assert!(name.bottom() < row.bottom());
                let surface = cx.debug_bounds("sidebar-session-surface").unwrap();
                assert_eq!(row.center(), surface.center());
                assert_eq!(
                    surface.size.height,
                    row.size.height - px(2.0 * super::ROW_SELECTION_INSET)
                );
                cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
                let close = cx.debug_bounds("sidebar-close-circle").unwrap();
                assert_eq!(close.center().y, row.center().y);
                assert!(
                    (row.right() - close.right() - (close.top() - row.top())).abs() < px(0.5),
                    "the trailing circle must be concentric with the capsule end at every UI scale"
                );
            }
        }
    }

    #[gpui::test]
    fn expanded_close_keeps_text_stable_and_does_not_select_the_tab(cx: &mut gpui::TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut sidebar = SessionSidebar::new(cx);
            let mut entry = session(None, None);
            entry.name = "Long title with CJK 中文 and more text".into();
            sidebar.sessions = vec![entry];
            sidebar.mode = super::PanelMode::Pinned;
            sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
            sidebar
        });
        let closed = Rc::new(Cell::new(None));
        let selected = Rc::new(Cell::new(0));
        cx.update(|_, cx| {
            let closed = closed.clone();
            cx.subscribe(&view, move |_, event: &super::SidebarCloseTab, _| {
                closed.set(Some(event.session_id));
            })
            .detach();
            let selected = selected.clone();
            cx.subscribe(&view, move |_, _: &super::SidebarSelect, _| {
                selected.set(selected.get() + 1);
            })
            .detach();
        });
        for width in [
            super::PANEL_MIN_WIDTH,
            super::PANEL_WIDTH,
            super::PANEL_MAX_WIDTH,
        ] {
            view.update(cx, |sidebar, cx| sidebar.set_panel_width(width, cx));
            for mode in [
                gpui_component::ThemeMode::Light,
                gpui_component::ThemeMode::Dark,
            ] {
                cx.update(|_, cx| gpui_component::Theme::change(mode, None, cx));
                view.update(cx, |_, cx| cx.notify());
                let row = cx.debug_bounds("sidebar-session-row").unwrap();
                let name = cx.debug_bounds("sidebar-session-name").unwrap();
                cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
                let close = cx.debug_bounds("sidebar-close-circle").unwrap();
                assert_eq!(close.size, gpui::size(px(14.0), px(14.0)));
                assert_eq!(row.center().y, close.center().y);
                assert!(name.right() < close.left());
                assert!(close.right() < row.right());
                assert_eq!(name, cx.debug_bounds("sidebar-session-name").unwrap());
                cx.simulate_click(close.center(), gpui::Modifiers::default());
                assert_eq!(closed.get(), Some(0));
                assert_eq!(
                    selected.get(),
                    0,
                    "closing must not activate a background tab"
                );
                closed.set(None);
            }
        }
    }

    #[gpui::test]
    fn completion_uses_one_notification_and_keeps_identity_in_rail_and_panel(
        cx: &mut gpui::TestAppContext,
    ) {
        use con_core::terminal_status::{Activity, Evidence, Status};
        cx.update(gpui_component::init);
        let done = Status {
            surface_id: 1,
            activity: Activity::Done,
            evidence: Evidence::ProgramStatus,
            percent: None,
        };
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = vec![session(Some(done), None)];
            sidebar
        });
        for expanded in [false, true] {
            view.update(cx, |sidebar, cx| {
                sidebar.set_pinned(expanded, cx);
                sidebar.width_motion =
                    crate::motion::SidebarMotion::new(if expanded { 1.0 } else { 0.0 });
                sidebar.sessions[0].status = Some(done);
                sidebar.sessions[0].needs_attention = true;
                cx.notify();
            });
            let identity = cx.debug_bounds("tab-status-brand").unwrap();
            let notification = cx.debug_bounds("tab-notification-dot").unwrap();
            assert!((notification.center().x - (identity.right() - px(1.0))).abs() <= px(0.5));
            assert!((notification.center().y - (identity.top() + px(1.0))).abs() <= px(0.5));
            assert!(cx.debug_bounds("tab-status-glyph").is_none());
            view.update(cx, |sidebar, cx| {
                sidebar.sessions[0].status = Some(Status {
                    activity: Activity::Idle,
                    ..done
                });
                sidebar.sessions[0].needs_attention = false;
                cx.notify();
            });
            assert!(cx.debug_bounds("tab-notification-dot").is_none());
            assert_eq!(cx.debug_bounds("tab-status-brand").unwrap(), identity);
            view.update(cx, |sidebar, cx| {
                sidebar.sessions[0].status = Some(Status {
                    activity: Activity::Error,
                    ..done
                });
                sidebar.sessions[0].needs_attention = true;
                cx.notify();
            });
            assert!(cx.debug_bounds("tab-notification-dot").is_none());
            assert!(cx.debug_bounds("tab-status-glyph").is_some());
        }
    }

    #[gpui::test]
    fn busy_switches_from_a_compact_identity_ring_to_expanded_bars_without_moving_the_title(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = vec![session(
                Some(con_core::terminal_status::Status {
                    surface_id: 1,
                    activity: con_core::terminal_status::Activity::Busy,
                    evidence: con_core::terminal_status::Evidence::ProgramStatus,
                    percent: None,
                }),
                None,
            )];
            sidebar
        });
        let brand = cx.debug_bounds("tab-status-brand").unwrap();
        assert!(cx.debug_bounds("tab-status-glyph").is_none());
        let slot = cx.debug_bounds("tab-status-slot").unwrap();
        let markers = rendered_rings(&view, cx);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].center(), slot.center());
        assert_eq!(brand.center(), slot.center());
        assert!(markers[0].size.width > brand.size.width);
        assert!(markers[0].size.height > brand.size.height);

        view.update(cx, |sidebar, cx| {
            sidebar.sessions[0].pane_icons =
                vec!["tab-terminal-refined.png", "tab-code.png", "tab-file.png"];
            cx.notify();
        });
        let stack = cx.debug_bounds("tab-status-brand").unwrap();
        let slot = cx.debug_bounds("tab-status-slot").unwrap();
        let markers = rendered_rings(&view, cx);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].center(), slot.center());
        assert_eq!(stack.center(), slot.center());
        assert!(markers[0].size.width > stack.size.width);
        assert!(markers[0].size.height > stack.size.height);

        view.update(cx, |sidebar, cx| {
            sidebar.set_pinned(true, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
        });
        assert!(cx.debug_bounds("tab-status-brand").is_none());
        assert_eq!(rendered_rings(&view, cx).len(), 1);
        let busy_title = cx.debug_bounds("sidebar-session-name").unwrap();
        view.update(cx, |sidebar, cx| {
            sidebar.sessions[0].status = None;
            cx.notify();
        });
        assert!(cx.debug_bounds("tab-status-brand").is_some());
        assert!(rendered_rings(&view, cx).is_empty());
        assert_eq!(busy_title, cx.debug_bounds("sidebar-session-name").unwrap());
        for icons in [
            vec!["tab-terminal-refined.png"],
            vec!["tab-terminal-refined.png", "tab-code.png"],
            vec!["tab-terminal-refined.png", "tab-code.png", "tab-file.png"],
        ] {
            view.update(cx, |sidebar, cx| {
                sidebar.sessions[0].pane_icons = icons;
                cx.notify();
            });
            assert_eq!(
                busy_title,
                cx.debug_bounds("sidebar-session-name").unwrap(),
                "single-app and multi-app titles must share one column"
            );
        }
    }

    #[gpui::test]
    fn expanded_toolbar_reclaims_the_rail_and_keeps_tools_clickable(cx: &mut gpui::TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = vec![session(None, None)];
            sidebar.mode = PanelMode::Pinned;
            sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
            sidebar
        });
        let opened = Rc::new(Cell::new(None));
        cx.update(|_, cx| {
            let opened = opened.clone();
            cx.subscribe(&view, move |_, event: &super::SidebarOpenToolSlot, _| {
                opened.set(Some(event.slot));
            })
            .detach();
        });
        for width in [
            super::PANEL_MIN_WIDTH,
            super::PANEL_WIDTH,
            super::PANEL_MAX_WIDTH,
        ] {
            view.update(cx, |sidebar, cx| sidebar.set_panel_width(width, cx));
            let expand = cx.debug_bounds("tab-sidebar-rail-expand").unwrap();
            let files = cx.debug_bounds("tab-sidebar-rail-files").unwrap();
            let search = cx.debug_bounds("tab-sidebar-rail-search").unwrap();
            let new = cx.debug_bounds("tab-sidebar-rail-new").unwrap();
            assert_eq!(files.center().y, expand.center().y);
            assert_eq!(search.center().y, expand.center().y);
            assert_eq!(new.center().y, expand.center().y);
            assert!(expand.right() < files.left() && files.right() < search.left());
            assert!(search.right() < new.left());
            let row = cx.debug_bounds("sidebar-session-row").unwrap();
            assert_eq!(
                row.left(),
                px(8.0),
                "no permanent tool column in expanded mode"
            );
            assert_eq!(row.size.width, px(width - 16.0));
            assert!(row.top() > new.bottom());
            cx.simulate_click(files.center(), gpui::Modifiers::default());
            assert_eq!(opened.get(), Some(crate::activity_bar::ActivitySlot::Files));
            cx.simulate_click(search.center(), gpui::Modifiers::default());
            assert_eq!(
                opened.get(),
                Some(crate::activity_bar::ActivitySlot::Search)
            );
        }
    }

    #[gpui::test]
    fn spring_samples_move_one_icon_and_keep_hitboxes_inside_the_reserved_width(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = vec![session(None, None)];
            sidebar.mode = PanelMode::Pinned;
            sidebar
        });
        let mut previous_y = None;
        for t in [0.0, 0.1, 0.25, 0.5, 0.75, 1.0] {
            view.update(cx, |sidebar, cx| {
                sidebar.width_motion = crate::motion::SidebarMotion::new(t);
                cx.notify();
            });
            let row = cx.debug_bounds("sidebar-session-row").unwrap();
            let icon = cx.debug_bounds("tab-status-brand").unwrap();
            #[cfg(target_os = "macos")]
            if t == 0.0 {
                for selector in [
                    "tab-sidebar-rail-expand",
                    "tab-sidebar-rail-files",
                    "tab-sidebar-rail-search",
                    "tab-sidebar-rail-new",
                ] {
                    assert_eq!(
                        cx.debug_bounds(selector).unwrap().center().x,
                        icon.center().x
                    );
                }
            }
            let width = view.read_with(cx, |sidebar, _| sidebar.rendered_width());
            assert!(row.left() >= px(0.0) && row.right() <= px(width));
            assert!((icon.center().y - row.center().y).abs() < px(0.5));
            assert!(icon.left() >= row.left() && icon.right() <= row.right());
            if let Some(previous) = previous_y {
                assert!(icon.center().y < previous);
            }
            previous_y = Some(icon.center().y);
            view.read_with(cx, |sidebar, _| {
                assert_eq!(sidebar.tab_bounds.borrow().len(), 1);
            });
        }
        let expanded_icon = cx.debug_bounds("tab-status-brand").unwrap();
        for t in [0.75, 0.5, 0.25, 0.0] {
            view.update(cx, |sidebar, cx| {
                sidebar.width_motion = crate::motion::SidebarMotion::new(t);
                cx.notify();
            });
        }
        let moving_compact = cx.debug_bounds("tab-status-brand").unwrap();
        assert!(expanded_icon.center().y < moving_compact.center().y);
        view.update(cx, |sidebar, cx| {
            sidebar.mode = PanelMode::Collapsed;
            cx.notify();
        });
        assert_eq!(
            moving_compact,
            cx.debug_bounds("tab-status-brand").unwrap(),
            "settling onto the compact rail must not jump the icon"
        );
    }

    #[gpui::test]
    fn sessions_have_one_navigation_list_and_keep_drag_bounds(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut sidebar = SessionSidebar::new(cx);
            sidebar.sessions = (0..2)
                .map(|id| super::SessionEntry {
                    id,
                    name: format!("Tab {id}"),
                    subtitle: Some("/project".into()),
                    is_ssh: false,
                    needs_attention: false,
                    status: None,
                    status_detail: None,

                    icon: "phosphor/terminal.svg",
                    pane_icons: Vec::new(),
                    pane_details: Vec::new(),
                    native_session_hint: None,
                    has_user_label: false,
                    pane_count: 1,
                    color: None,
                })
                .collect();
            sidebar
        });
        assert!(cx.debug_bounds("sidebar-session-tile").is_some());
        assert!(cx.debug_bounds("sidebar-session-row").is_none());
        let new_tab = cx
            .debug_bounds("tab-sidebar-rail-new")
            .expect("new tab control");
        view.read_with(cx, |sidebar, _| {
            let first = sidebar.tab_bounds.borrow()[0];
            assert!(first.top() - new_tab.bottom() >= gpui::px(super::RAIL_SESSIONS_GAP));
            assert_eq!(
                first.top(),
                gpui::px(sidebar.leading_top_pad + super::RAIL_TOP_CONTROLS_HEIGHT),
                "drag fallback must match the actual session list origin",
            );
        });
        view.update(cx, |sidebar, cx| {
            sidebar.set_pinned(true, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(1.0);
        });
        assert!(cx.debug_bounds("sidebar-session-tile").is_none());
        assert!(cx.debug_bounds("sidebar-session-row").is_some());
        view.read_with(cx, |sidebar, _| {
            let bounds = sidebar.tab_bounds.borrow();
            assert_eq!(bounds.len(), 2);
            assert_eq!(
                vertical_slot_from_bounds(bounds[1].center().y, &bounds, 2),
                Some(2),
                "expanded rows remain the drag reorder targets",
            );
        });
        for slot in [
            crate::activity_bar::ActivitySlot::Files,
            crate::activity_bar::ActivitySlot::Search,
        ] {
            view.update(cx, |sidebar, cx| {
                sidebar.set_tools_panel_open(true, slot);
                cx.notify();
            });
            assert!(cx.debug_bounds("sidebar-session-tile").is_some());
            assert!(cx.debug_bounds("sidebar-session-row").is_none());
            view.read_with(cx, |sidebar, _| {
                assert_eq!(sidebar.tab_bounds.borrow().len(), 2);
            });
        }
        view.update(cx, |sidebar, cx| {
            sidebar.set_tools_panel_open(false, crate::activity_bar::ActivitySlot::Files);
            sidebar.show_rail_only_preserving_pinned(cx);
        });
        assert!(cx.debug_bounds("sidebar-session-tile").is_some());
        assert!(cx.debug_bounds("sidebar-session-row").is_none());
        view.update(cx, |sidebar, cx| sidebar.clear_rail_only_override(cx));
        assert!(cx.debug_bounds("sidebar-session-tile").is_none());
        assert!(cx.debug_bounds("sidebar-session-row").is_some());
        view.update(cx, |sidebar, cx| {
            sidebar.set_pinned(false, cx);
            sidebar.width_motion = crate::motion::SidebarMotion::new(0.0);
        });
        assert!(cx.debug_bounds("sidebar-session-tile").is_some());
        assert!(cx.debug_bounds("sidebar-session-row").is_none());
    }

    #[test]
    fn bundled_font_has_visible_title_status_glyphs() {
        let face = ttf_parser::Face::parse(
            include_bytes!("../../../assets/fonts/IoskeleyMono-Regular.ttf"),
            0,
        )
        .unwrap();
        for frame in ('\u{2801}'..='\u{28ff}').chain("·✢✳✶✻✽!.".chars()) {
            let glyph = face.glyph_index(frame).expect("status glyph must exist");
            let bounds = face
                .glyph_bounding_box(glyph)
                .expect("status glyph must have an outline");
            assert!(
                bounds.width() > 0 && bounds.height() > 0,
                "empty glyph: {frame}"
            );
        }
    }

    #[test]
    fn vertical_slot_from_bounds_uses_row_midpoints() {
        let bounds = vec![
            Bounds::new(
                Point {
                    x: px(10.0),
                    y: px(100.0),
                },
                Size {
                    width: px(200.0),
                    height: px(40.0),
                },
            ),
            Bounds::new(
                Point {
                    x: px(10.0),
                    y: px(144.0),
                },
                Size {
                    width: px(200.0),
                    height: px(40.0),
                },
            ),
        ];

        assert_eq!(vertical_slot_from_bounds(px(90.0), &bounds, 2), Some(0));
        assert_eq!(vertical_slot_from_bounds(px(121.0), &bounds, 2), Some(1));
        assert_eq!(vertical_slot_from_bounds(px(180.0), &bounds, 2), Some(2));
    }

    #[test]
    fn rail_slot_from_local_y_starts_after_fixed_controls() {
        assert_eq!(
            rail_slot_from_local_y(RAIL_TOP_CONTROLS_HEIGHT - 1.0, 3, 0.0),
            Some(0)
        );
        assert_eq!(
            rail_slot_from_local_y(RAIL_TOP_CONTROLS_HEIGHT, 3, 0.0),
            Some(0)
        );
        assert_eq!(
            rail_slot_from_local_y(RAIL_TOP_CONTROLS_HEIGHT + RAIL_ICON_SIZE * 0.75, 3, 0.0),
            Some(1)
        );
        // 40px tiles + 2px gaps: the second slot's midpoint is 63px.
        // A stale 32px stride would incorrectly insert after the second tile.
        for (offset, expected) in [(62.0, 1), (64.0, 2), (126.0, 3)] {
            assert_eq!(
                rail_slot_from_local_y(RAIL_TOP_CONTROLS_HEIGHT + 7.0 + offset, 3, 7.0),
                Some(expected)
            );
        }
    }

    #[test]
    fn vertical_drag_overlay_probe_locks_x_and_uses_overlay_center() {
        let preview = SidebarDragPreviewState {
            label: "tab".into(),
            icon: "phosphor/terminal.svg",
            source_left: px(12.0),
            cursor_offset_y: px(8.0),
            is_pane_origin: false,
        };

        let probe = vertical_drag_overlay_probe_position(
            Point {
                x: px(480.0),
                y: px(220.0),
            },
            &preview,
            Size {
                width: px(180.0),
                height: px(28.0),
            },
            px(50.0),
            px(400.0),
        );

        assert_eq!(
            probe,
            Point {
                x: px(102.0),
                y: px(226.0)
            }
        );
    }

    #[test]
    fn normalize_sidebar_rename_label_trims_and_clears_blank_values() {
        assert_eq!(normalize_sidebar_rename_label(""), None);
        assert_eq!(normalize_sidebar_rename_label("   \t  \n"), None);
        assert_eq!(
            normalize_sidebar_rename_label("  hello  "),
            Some("hello".to_string())
        );
    }

    #[test]
    fn rail_mode_button_expands_on_first_click_from_collapsed_override() {
        let mut sidebar = SessionSidebar {
            mode: PanelMode::Collapsed,
            rail_only_override: true,
            ..SessionSidebar::new_for_test()
        };

        assert_eq!(
            sidebar.rail_mode_button_action(),
            RailModeButtonAction::ExpandCollapsedPanelFromOverride
        );

        sidebar.mode = PanelMode::Pinned;
        assert_eq!(
            sidebar.rail_mode_button_action(),
            RailModeButtonAction::RestorePinnedPanelFromRail
        );

        sidebar.rail_only_override = false;
        assert_eq!(
            sidebar.rail_mode_button_action(),
            RailModeButtonAction::TogglePinned
        );
        sidebar.tools_panel_open = true;
        assert_eq!(
            sidebar.rail_mode_button_action(),
            RailModeButtonAction::ShowSessionsFromToolPanel
        );
    }

    #[test]
    fn stale_blur_after_escape_and_same_tab_reopen_should_not_commit() {
        let mut generation = 0;
        let state = begin_rename_lifecycle(&mut generation, 42);
        let cancelled_generation = state.active_generation.unwrap();
        let state = cancel_rename_lifecycle(state);
        let reopened = begin_rename_after_cancel_lifecycle(&mut generation, state, 42);

        assert!(!blur_should_commit_for_lifecycle(
            reopened,
            cancelled_generation
        ));

        let active_generation = reopened.active_generation.unwrap();
        assert!(blur_should_commit_for_lifecycle(
            reopened,
            active_generation
        ));
    }
}
