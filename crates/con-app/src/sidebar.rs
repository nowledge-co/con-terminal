//! Left-sidebar support state.
//!
//! This module owns the vertical tab rail/panel inside the product sidebar:
//! shared width, opacity, tab metadata, hover cards, and drag helpers.
//!
//! Visual rules
//! ---
//! - Active row: elevated pill bg + foreground text. **No accent
//!   bar.** A single, unambiguous selection cue is enough; tab accent
//!   colors render as the row surface itself at active/inactive alpha.
//! - Action affordances (rename pencil, close X) are hover-only on
//!   every row, including the active one. Quiet by default; reveal
//!   on intent.
//! - Surface separation comes from `surface_tone()` (foreground
//!   blended into background at small intensities), not borders.

use crate::activity_bar::ActivitySlot;
use crate::motion::MotionValue;
use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point,
    Render, SharedString, Size, Stateful, StatefulInteractiveElement, Styled, WeakEntity, Window,
    div, point, prelude::*, px, svg,
};
use gpui_component::{
    ActiveTheme, ElementExt, InteractiveElementExt, Sizable,
    input::{Escape as InputEscape, Input, InputEvent, InputState},
    menu::{ContextMenuExt, PopupMenu},
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
/// Width of the floating hover card shown when the cursor is over a
/// rail icon. Slightly wider than the panel so two-line rows are
/// comfortable at this magnification.
#[allow(dead_code)]
const HOVER_CARD_WIDTH: f32 = 240.0;
/// Per-row height in pinned mode (two-line layout — name + subtitle).
const ROW_HEIGHT: f32 = 44.0;
/// Per-icon size in the rail.
const RAIL_ICON_SIZE: f32 = 32.0;
/// Vertical gap between rail icons. Used to compute the icon's
/// y-center for hover-card anchoring.
const RAIL_ICON_GAP: f32 = 2.0;
const RAIL_TOP_CONTROLS_HEIGHT: f32 =
    28.0 + RAIL_ICON_GAP + 28.0 + RAIL_ICON_GAP + 5.0 + RAIL_ICON_GAP;

/// Cubic ease-out feel for the panel width animation.
#[cfg(not(target_os = "macos"))]
const PANEL_TWEEN: Duration = Duration::from_millis(220);
#[cfg(target_os = "macos")]
const PANEL_TWEEN: Duration = Duration::ZERO;

/// One row in the tab sidebar panel.
pub struct SessionEntry {
    pub id: u64,
    pub name: String,
    pub subtitle: Option<String>,
    pub is_ssh: bool,
    pub needs_attention: bool,
    pub icon: &'static str,
    pub has_user_label: bool,
    /// How many panes the tab contains (split count). Surfaced in
    /// the rail's hover card so the user can see "this tab has 3
    /// panes" without expanding.
    pub pane_count: usize,
    /// Optional accent color set via the tab context menu.
    pub color: Option<con_core::session::TabAccentColor>,
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
            .text_color(theme.foreground.opacity(0.82))
            .text_size(px(12.0))
            .font_family(theme.font_family.clone())
            .child(
                svg()
                    .path(self.icon)
                    .size(px(12.0))
                    .text_color(theme.foreground),
            )
            .child(div().truncate().child(self.label.clone()))
    }
}

/// Tab sidebar side panel.
pub struct SessionSidebar {
    mode: PanelMode,
    sessions: Vec<SessionEntry>,
    active_session: usize,
    leading_top_pad: f32,
    /// Smooth width animation between rail (0.0) and pinned (1.0).
    width_motion: MotionValue,
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
    /// Effective UI opacity from the workspace appearance settings.
    /// Lower values let the window/terminal backdrop treatment show
    /// through, matching the rest of con's chrome.
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
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self::default_state()
    }

    fn default_state() -> Self {
        Self {
            mode: PanelMode::Collapsed,
            sessions: Vec::new(),
            active_session: 0,
            // The workspace top bar already reserves the macOS
            // traffic-light/titlebar area before the sidebar is
            // rendered. Keep only a small visual inset here; a
            // platform titlebar inset creates a dead band above the
            // tab-sidebar controls.
            leading_top_pad: 6.0,
            width_motion: MotionValue::new(0.0),
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
        let had_rail_only_override = self.rail_only_override;
        self.rail_only_override = false;
        let new_mode = if pinned {
            PanelMode::Pinned
        } else {
            PanelMode::Collapsed
        };
        if self.mode != new_mode {
            self.mode = new_mode;
            self.cancel_rename(cx);
            self.hovered_rail = None;
            let target = if pinned { 1.0 } else { 0.0 };
            self.width_motion.set_target(target, PANEL_TWEEN);
            cx.notify();
        } else if had_rail_only_override {
            cx.notify();
        }
    }

    pub fn is_pinned(&self) -> bool {
        matches!(self.mode, PanelMode::Pinned)
    }

    pub fn panel_width(&self) -> f32 {
        self.panel_width
    }

    pub fn rendered_width(&self) -> f32 {
        if self.renders_rail_only() {
            return RAIL_WIDTH;
        }
        match self.mode {
            PanelMode::Collapsed => RAIL_WIDTH,
            PanelMode::Pinned => self.effective_panel_width(),
        }
    }

    pub fn set_tools_panel_open(&mut self, open: bool, active_slot: ActivitySlot) {
        if self.tools_panel_open != open {
            self.tools_panel_open = open;
            self.hovered_rail = None;
            if open {
                self.rail_only_override = false;
            }
        }
        self.active_tool_slot = active_slot;
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

    #[allow(dead_code)]
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
        if self.drop_slot.is_none() {
            return None;
        }
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
                .rounded(px(4.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .cursor_grab()
                .bg(theme.title_bar.opacity(0.92))
                .font_family(theme.font_family.clone())
                .text_size(px(12.0))
                .text_color(theme.foreground.opacity(0.82))
                .child(
                    svg()
                        .path(preview.icon)
                        .size(px(12.0))
                        .flex_shrink_0()
                        .text_color(theme.foreground),
                )
                .child(div().flex_1().min_w_0().truncate().child(preview.label))
                .into_any_element(),
        )
    }

    pub fn clamped_panel_width(width: f32, effective_max_width: f32) -> f32 {
        width.clamp(PANEL_MIN_WIDTH, effective_max_width.max(PANEL_MIN_WIDTH))
    }

    fn effective_panel_width(&self) -> f32 {
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

    /// Update the session list from workspace state.
    pub fn sync_sessions(
        &mut self,
        sessions: Vec<SessionEntry>,
        active: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = &self.rename {
            if state.index >= sessions.len() || sessions[state.index].id != state.session_id {
                self.rename = None;
            }
        }
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
    fn render_rail(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let rail_bg = sidebar_surface(theme, self.ui_opacity, 0.035);
        let session_count = self.sessions.len();
        let mode_icon = if self.tools_panel_open || (self.is_pinned() && !self.rail_only_override) {
            "phosphor/caret-line-left.svg"
        } else {
            "phosphor/caret-line-right.svg"
        };
        let mode_color = if self.tools_panel_open || (self.is_pinned() && !self.rail_only_override)
        {
            theme.foreground.opacity(0.74)
        } else {
            theme.muted_foreground.opacity(0.78)
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
        self.tab_bounds.borrow_mut().clear();
        let mut rail = div()
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
            .bg(rail_bg)
            // Mouse-leave on the rail container clears the hover card
            // even when the cursor exits via a fast diagonal motion
            // that may skip the per-icon hover transitions.
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
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
            .child(
                div()
                    .h(px(1.0))
                    .w(px(20.0))
                    .my(px(2.0))
                    .bg(theme.muted_foreground.opacity(0.18)),
            )
            .child(rail_icon_button(
                "tab-sidebar-rail-files",
                "phosphor/folders.svg",
                files_color,
                theme,
                cx.listener(|_, _, _, cx| {
                    cx.emit(SidebarOpenToolSlot {
                        slot: ActivitySlot::Files,
                    })
                }),
            ))
            .child(rail_icon_button(
                "tab-sidebar-rail-search",
                "phosphor/file-magnifying-glass.svg",
                search_color,
                theme,
                cx.listener(|_, _, _, cx| {
                    cx.emit(SidebarOpenToolSlot {
                        slot: ActivitySlot::Search,
                    })
                }),
            ))
            .child(
                div()
                    .h(px(1.0))
                    .w(px(20.0))
                    .my(px(2.0))
                    .bg(theme.muted_foreground.opacity(0.18)),
            )
            .child(rail_icon_button(
                "tab-sidebar-rail-new",
                "phosphor/plus.svg",
                theme.muted_foreground,
                theme,
                cx.listener(|_, _, _, cx| cx.emit(NewSession)),
            ));

        for (i, session) in self.sessions.iter().enumerate() {
            let is_active = i == self.active_session;
            let active_bg = elevated_surface(theme, self.ui_opacity);
            let hover_bg = sidebar_surface(theme, self.ui_opacity, 0.075);
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
            // No accent-color background fill — keep pills monochrome.
            // Accent color is expressed only via the active dot below.
            let pill_bg = if is_active {
                active_bg
            } else {
                gpui::transparent_black()
            };
            let inactive_hover_bg = hover_bg;
            let mut pill = div()
                .id(SharedString::from(format!("rail-tab-{i}")))
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .size(px(RAIL_ICON_SIZE))
                .rounded(px(8.0))
                .cursor_pointer()
                .bg(pill_bg)
                .hover(move |s| {
                    if is_active {
                        s
                    } else {
                        s.bg(inactive_hover_bg)
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
                })
                .child(
                    svg()
                        .path(session.icon)
                        .size(px(16.0))
                        .text_color(if is_active {
                            theme.foreground
                        } else {
                            theme.muted_foreground.opacity(0.78)
                        }),
                );

            if session.needs_attention && !is_active {
                pill = pill.child(
                    div()
                        .absolute()
                        .top(px(3.0))
                        .right(px(3.0))
                        .size(px(6.0))
                        .rounded_full()
                        .bg(theme.primary),
                );
            }
            if is_active {
                let dot_color = if let Some(color) = session.color {
                    crate::tab_colors::tab_accent_color_hsla(color, cx)
                } else {
                    crate::tab_colors::active_tab_indicator_color()
                };
                pill = pill.child(
                    div()
                        .absolute()
                        .bottom(px(3.0))
                        .right(px(3.0))
                        .size(px(6.0))
                        .rounded_full()
                        .bg(dot_color),
                );
            }
            if show_indicator_above {
                pill = pill.child(rail_drop_indicator(theme, true));
            }
            if show_indicator_below {
                pill = pill.child(rail_drop_indicator(theme, false));
            }

            rail = rail.child(pill);
        }

        rail.child(div().flex_1())
    }

    /// Floating hover card shown next to the hovered rail icon.
    /// Composed in workspace coordinates so it renders OVER the
    /// translucent terminal pane (instead of behind it).
    ///
    /// The card vertically tracks the cursor's current y so the user
    /// always sees the card beside their finger. Anchoring it to the
    /// icon's geometric center sounded cleaner but actually requires
    /// computing the rail layout from this side, which is brittle —
    /// the cursor IS the icon, and "follow the cursor" is the
    /// established Apple tooltip pattern (Finder, Mail, Safari).
    #[allow(dead_code)]
    pub fn render_hover_card_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.renders_rail_only() {
            return None;
        }
        let i = self.hovered_rail?;
        if self.drop_slot.is_some() {
            return None;
        }
        let session = self.sessions.get(i)?;
        let theme = cx.theme();
        let bg = elevated_surface(theme, self.ui_opacity);
        let edge = sidebar_surface(theme, self.ui_opacity, 0.10);

        // Anchor the card vertically on the cursor — its row IS the
        // icon under the cursor by construction.
        let cursor = window.mouse_position();
        let card_height = if session.subtitle.is_some() {
            64.0
        } else {
            44.0
        };
        let min_top = self.leading_top_pad + RAIL_TOP_CONTROLS_HEIGHT + 8.0;
        let top = (f32::from(cursor.y) - card_height / 2.0).max(min_top);

        let name_color = theme.foreground;
        let sub_color = theme.muted_foreground.opacity(0.65);
        let meta_color = theme.muted_foreground.opacity(0.50);
        let mono_font = theme.mono_font_family.clone();
        let sys_font = theme.font_family.clone();

        let mut card_inner = div().px(px(12.0)).py(px(8.0)).child(
            div()
                .text_size(px(12.5))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(name_color)
                .font_family(sys_font)
                .truncate()
                .child(session.name.clone()),
        );

        if let Some(sub) = session.subtitle.as_ref() {
            card_inner = card_inner.child(
                div()
                    .mt(px(2.0))
                    .text_size(px(11.0))
                    .line_height(px(14.0))
                    .text_color(sub_color)
                    .font_family(mono_font)
                    .truncate()
                    .child(sub.clone()),
            );
        }

        let mut meta = div()
            .mt(px(if session.subtitle.is_some() { 4.0 } else { 2.0 }))
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(10.5))
            .line_height(px(13.0))
            .text_color(meta_color);

        let pane_label = match session.pane_count {
            0 | 1 => "1 pane".to_string(),
            n => format!("{n} panes"),
        };
        meta = meta.child(div().child(pane_label));
        if session.is_ssh {
            meta = meta.child(div().child("·").text_color(meta_color));
            meta = meta.child(div().child("SSH"));
        }
        if session.needs_attention {
            meta = meta.child(div().child("·").text_color(meta_color)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(div().size(px(6.0)).rounded_full().bg(theme.primary))
                    .child(div().child("unread")),
            );
        }
        card_inner = card_inner.child(meta);

        let card = div()
            .id("tab-sidebar-hover-card")
            .absolute()
            .top(px(top))
            .left(px(RAIL_WIDTH + 6.0))
            .w(px(HOVER_CARD_WIDTH))
            .rounded(px(8.0))
            .bg(bg)
            .occlude()
            .child(card_inner)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(-3.0))
                    .h_full()
                    .w(px(3.0))
                    .rounded(px(2.0))
                    .bg(edge),
            );

        Some(card.into_any_element())
    }

    fn render_panel_body(
        &mut self,
        _is_overlay: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let body_bg;
        let header_color;
        let header_font;
        {
            let theme = cx.theme();
            body_bg = sidebar_surface(theme, self.ui_opacity, 0.045);
            header_color = theme.muted_foreground.opacity(0.55);
            header_font = theme.font_family.clone();
        }

        let header_top = self.leading_top_pad.max(0.0);
        let header = div()
            .flex()
            .flex_col()
            .h(px(header_top + 42.0))
            .pt(px(header_top))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(34.0))
                    .px(px(12.0))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(header_color)
                            .font_family(header_font)
                            .child(format!("{} TABS", self.sessions.len())),
                    ),
            );

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
            .px(px(6.0))
            .pt(px(2.0))
            .gap(px(2.0))
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
                    let approx_row_h = ROW_HEIGHT;
                    let last_row_bottom_estimate =
                        f32::from(event.bounds.origin.y) + (total as f32) * (approx_row_h + 2.0);
                    if f32::from(event.event.position.y) >= last_row_bottom_estimate {
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
            let session_clone = SessionEntry {
                id: self.sessions[i].id,
                name: self.sessions[i].name.clone(),
                subtitle: self.sessions[i].subtitle.clone(),
                is_ssh: self.sessions[i].is_ssh,
                needs_attention: self.sessions[i].needs_attention,
                icon: self.sessions[i].icon,
                has_user_label: self.sessions[i].has_user_label,
                pane_count: self.sessions[i].pane_count,
                color: self.sessions[i].color,
            };
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
            );
            list = list.child(row);
        }

        div()
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .flex_shrink_0()
            .bg(body_bg)
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
        _window: &mut Window,
        cx: &mut Context<Self>,
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

        let row_h = ROW_HEIGHT;

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
            let subtitle = session.subtitle.clone();
            let mono = theme.mono_font_family.clone();
            let sys = theme.font_family.clone();
            let fg = if is_active {
                theme.foreground
            } else {
                theme.muted_foreground.opacity(0.92)
            };
            let sub_fg = theme.muted_foreground.opacity(0.55);
            let mut block = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(1.0))
                .child(
                    div()
                        .overflow_hidden()
                        .truncate()
                        .text_size(px(12.5))
                        .line_height(px(16.0))
                        .font_family(sys)
                        .text_color(fg)
                        .when(is_active, |s| s.font_weight(FontWeight::MEDIUM))
                        .child(name),
                );
            if let Some(sub) = subtitle {
                block = block.child(
                    div()
                        .overflow_hidden()
                        .truncate()
                        .text_size(px(10.5))
                        .line_height(px(14.0))
                        .font_family(mono)
                        .text_color(sub_fg)
                        .child(sub),
                );
            }
            block.into_any_element()
        };

        let row_group = SharedString::from(format!("panel-tab-row-{i}"));
        let session_id = session.id;

        // Both action buttons hover-only on every row, including
        // active. Apple-quiet: visible on intent, hidden by default.
        let rename_btn = panel_icon_button_small(
            SharedString::from(format!("panel-tab-rename-{i}")),
            "phosphor/pencil-simple.svg",
            theme,
            cx.listener(move |this, _, window, cx| this.begin_rename(i, window, cx)),
        )
        .invisible()
        .group_hover(row_group.clone(), |s| s.visible());
        let close_btn = panel_icon_button_small(
            SharedString::from(format!("panel-tab-close-{i}")),
            "phosphor/x.svg",
            theme,
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
                .unwrap_or_else(|| elevated_surface(theme, self.ui_opacity))
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
        let hover_bg = sidebar_surface(theme, self.ui_opacity, 0.075);
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

        let mut icon_stack = div().relative().flex_shrink_0().child(
            svg()
                .path(session.icon)
                .size(px(15.0))
                .text_color(if is_active {
                    theme.foreground
                } else {
                    theme.muted_foreground.opacity(0.78)
                }),
        );
        if session.needs_attention && !is_active {
            icon_stack = icon_stack.child(
                div()
                    .absolute()
                    .top(px(-2.0))
                    .right(px(-2.0))
                    .size(px(6.0))
                    .rounded_full()
                    .bg(theme.primary),
            );
        }
        if is_active {
            let dot_color = if let Some(color) = session.color {
                crate::tab_colors::tab_accent_color_hsla(color, cx)
            } else {
                crate::tab_colors::active_tab_indicator_color()
            };
            icon_stack = icon_stack.child(
                div()
                    .absolute()
                    .bottom(px(-2.0))
                    .right(px(-2.0))
                    .size(px(6.0))
                    .rounded_full()
                    .bg(dot_color),
            );
        }

        let row = div()
            .id(SharedString::from(format!("panel-tab-{i}")))
            .group(row_group.clone())
            .relative()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(10.0))
            .pr(px(4.0))
            .h(px(row_h))
            .rounded(px(8.0))
            .cursor_pointer()
            .bg(row_bg)
            .text_color(if is_active {
                theme.foreground
            } else {
                theme.muted_foreground.opacity(0.92)
            })
            .hover(move |s| {
                if is_active {
                    s
                } else {
                    s.bg(inactive_hover_bg)
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
                let total = total;
                let session_id = session.id;
                let has_user_label = session.has_user_label;
                let current_color = session.color;
                let weak = cx.weak_entity();
                move |menu, _window, _cx| {
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
            .child(label_block)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(rename_btn)
                    .child(close_btn),
            );
        row.into_any_element()
    }
}

impl Render for SessionSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        // Drive the width-tween animation frame.
        let _progress = self.width_motion.value(window);

        if self.tools_panel_open || self.rail_only_override {
            let mut rail = div()
                .relative()
                .h_full()
                .w(px(RAIL_WIDTH))
                .flex_shrink_0()
                .child(self.render_rail(window, cx));
            if let Some(hover_card) = self.render_hover_card_overlay(window, cx) {
                rail = rail.child(hover_card);
            }
            return rail.into_any_element();
        }

        match self.mode {
            PanelMode::Pinned => {
                let t = self.width_motion.current().clamp(0.0, 1.0);
                let visible_w = RAIL_WIDTH + (self.effective_panel_width() - RAIL_WIDTH) * t;
                let panel = self.render_panel_body(false, window, cx);
                let panel_w = (visible_w - RAIL_WIDTH).max(0.0);
                div()
                    .flex()
                    .h_full()
                    .w(px(visible_w))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(self.render_rail(window, cx))
                    .child(
                        div()
                            .h_full()
                            .w(px(panel_w))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .child(panel),
                    )
                    .into_any_element()
            }
            PanelMode::Collapsed => {
                let mut rail = div()
                    .relative()
                    .h_full()
                    .w(px(RAIL_WIDTH))
                    .flex_shrink_0()
                    .child(self.render_rail(window, cx));
                if let Some(hover_card) = self.render_hover_card_overlay(window, cx) {
                    rail = rail.child(hover_card);
                }
                rail.into_any_element()
            }
        }
    }
}

/// Compose a surface color one perceptual step darker (light theme) or
/// lighter (dark theme) than `theme.background`. Foreground is forced
/// to a desaturated extreme-luminance overlay so the result reads
/// against any palette author's choice of `theme.background`.
fn surface_tone(theme: &gpui_component::Theme, intensity: f32) -> Hsla {
    let mut over = theme.foreground;
    over.s = 0.0;
    over.l = if theme.foreground.l < 0.5 { 0.0 } else { 1.0 };
    over.a = intensity.clamp(0.0, 1.0);
    theme.background.blend(over)
}

fn sidebar_surface(theme: &gpui_component::Theme, opacity: f32, intensity: f32) -> Hsla {
    let mut color = surface_tone(theme, intensity);
    color.a = opacity.clamp(0.55, 0.98);
    color
}

fn elevated_surface(theme: &gpui_component::Theme, opacity: f32) -> Hsla {
    theme.background.opacity((opacity + 0.06).min(0.98))
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
    //   + expand button (28 px) + RAIL_ICON_GAP
    //   + new-tab button (28 px) + RAIL_ICON_GAP
    //   + separator h(1) + my(2)*2 = 5 px + RAIL_ICON_GAP
    //   + i * (RAIL_ICON_SIZE + RAIL_ICON_GAP)
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
    handler: F,
) -> Stateful<Div>
where
    F: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    let hover_bg = surface_tone(theme, 0.065);
    div()
        .id(id)
        .size(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(svg().path(icon).size(px(14.0)).text_color(icon_color))
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

fn panel_icon_button_small<F>(
    id: SharedString,
    icon: &'static str,
    theme: &gpui_component::Theme,
    handler: F,
) -> Stateful<Div>
where
    F: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    let hover_bg = surface_tone(theme, 0.08);
    div()
        .id(id)
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(
            svg()
                .path(icon)
                .size(px(11.0))
                .text_color(theme.muted_foreground.opacity(0.72)),
        )
        .on_mouse_down(MouseButton::Left, handler)
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
    use super::{
        PanelMode, RailModeButtonAction, SessionSidebar, SidebarDragPreviewState,
        begin_rename_after_cancel_lifecycle, begin_rename_lifecycle,
        blur_should_commit_for_lifecycle, cancel_rename_lifecycle, normalize_sidebar_rename_label,
        vertical_drag_overlay_probe_position, vertical_slot_from_bounds,
    };
    use gpui::{Bounds, Point, Size, px};

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
