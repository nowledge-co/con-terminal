use gpui::*;
use gpui_component::{
    ActiveTheme,
    input::{Input, InputEvent, InputState, Position},
    tooltip::Tooltip,
};

use crate::ui_scale::{mono_density_scale, mono_px, mono_space_px};

const INPUT_BAR_MIN_HEIGHT: f32 = 44.0;
const INPUT_BAR_ROW_HEIGHT: f32 = 20.0;
const INPUT_BAR_MAX_ROWS: usize = 6;

fn input_bar_content_rows(value: &str) -> usize {
    (value.matches('\n').count() + 1).clamp(1, INPUT_BAR_MAX_ROWS)
}

fn input_bar_rendered_height_for_rows(rows: usize, mono_scale: f32) -> f32 {
    (INPUT_BAR_MIN_HEIGHT + rows.saturating_sub(1) as f32 * INPUT_BAR_ROW_HEIGHT) * mono_scale
}

/// Append terminal-selection context to the agent input as a fenced code
/// block. The fence grows when the selection itself contains backticks so
/// the block never terminates early.
pub(crate) fn append_ask_ai_context(current: &str, context: &str) -> String {
    let longest_backtick_run = context
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest_backtick_run + 1).max(3));

    let mut updated = String::new();
    let trimmed_current = current.trim_end();
    if !trimmed_current.is_empty() {
        updated.push_str(trimmed_current);
        updated.push('\n');
    }
    updated.push_str(&fence);
    updated.push('\n');
    updated.push_str(context);
    updated.push('\n');
    updated.push_str(&fence);
    updated.push('\n');
    updated
}

pub(crate) fn text_end_position(text: &str) -> Position {
    let line = text.matches('\n').count() as u32;
    let character = text
        .rsplit('\n')
        .next()
        .map(|last| last.chars().count() as u32)
        .unwrap_or(0);
    Position::new(line, character)
}

actions!(
    input_bar,
    [
        SubmitInput,
        EscapeInput,
        AcceptSuggestion,
        AcceptSuggestionOrMoveRight,
        AcceptSuggestionOrMoveEnd,
        DeletePreviousWord,
        HistoryPrevious,
        HistoryNext
    ]
);

pub struct SkillAutocompleteChanged;
impl EventEmitter<SkillAutocompleteChanged> for InputBar {}
pub struct InputEdited;
impl EventEmitter<InputEdited> for InputBar {}
pub struct InputScopeChanged;
impl EventEmitter<InputScopeChanged> for InputBar {}
pub struct TogglePaneScopePicker;
impl EventEmitter<TogglePaneScopePicker> for InputBar {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputMode {
    Smart,
    Shell,
    Agent,
}

impl InputMode {
    fn next(self) -> Self {
        match self {
            Self::Smart => Self::Shell,
            Self::Shell => Self::Agent,
            Self::Agent => Self::Smart,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Smart => "phosphor/magic-wand.svg",
            Self::Shell => "phosphor/terminal.svg",
            Self::Agent => "phosphor/sparkle.svg",
        }
    }

    fn tint(self, cx: &App) -> Hsla {
        match self {
            Self::Smart => cx.theme().muted_foreground,
            Self::Shell => cx.theme().success,
            Self::Agent => cx.theme().primary,
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Self::Smart => "Smart mode",
            Self::Shell => "Shell mode",
            Self::Agent => "Agent mode",
        }
    }
}

/// Pane info for the pane selector
#[derive(Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: usize,
    /// Display name (title, or cwd basename, or "Pane N")
    pub name: String,
    /// SSH hostname if connected via SSH
    pub hostname: Option<String>,
    /// Whether a command is currently running
    pub is_busy: bool,
    /// Whether the PTY process is still alive
    pub is_alive: bool,
}

/// A skill available for slash-command completion.
#[derive(Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuggestionSource {
    Path,
    History,
    Ai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneScopeMode {
    Broadcast,
    Focused,
    Custom,
}

pub struct InputBar {
    agent_input_state: Entity<InputState>,
    shell_input_state: Entity<InputState>,
    mode: InputMode,
    cwd: String,
    panes: Vec<PaneInfo>,
    pane_scope_mode: PaneScopeMode,
    selected_pane_ids: Vec<usize>,
    last_single_target_id: Option<usize>,
    focused_pane_id: usize,
    recent_commands: Vec<String>,
    history_nav_index: Option<usize>,
    history_nav_draft: Option<String>,
    skills: Vec<SkillEntry>,
    /// Index of the highlighted skill in the filtered list (for arrow-key nav)
    skill_selection: usize,
    inline_suggestion_prefix: Option<String>,
    inline_suggestion_suffix: Option<String>,
    inline_suggestion_source: Option<SuggestionSource>,
    path_completion_candidates: Vec<String>,
    path_completion_selection: usize,
    /// Tracks whether shift was held on the last enter keystroke.
    /// Set by observe_keystrokes (fires before PressEnter), consumed by PressEnter handler.
    shift_enter: bool,
    ui_opacity: f32,
    _subscriptions: Vec<Subscription>,
}

impl InputBar {
    pub fn init(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new(
                "right",
                AcceptSuggestionOrMoveRight,
                Some("ConCommandInput > Input"),
            ),
            KeyBinding::new("tab", AcceptSuggestion, Some("ConCommandInput > Input")),
            KeyBinding::new(
                "ctrl-e",
                AcceptSuggestionOrMoveEnd,
                Some("ConCommandInput > Input"),
            ),
            KeyBinding::new(
                "secondary-e",
                AcceptSuggestionOrMoveEnd,
                Some("ConCommandInput > Input"),
            ),
            KeyBinding::new(
                "secondary-w",
                DeletePreviousWord,
                Some("ConCommandInput > Input"),
            ),
            KeyBinding::new(
                "secondary-right",
                AcceptSuggestionOrMoveEnd,
                Some("ConCommandInput > Input"),
            ),
            KeyBinding::new(
                "end",
                AcceptSuggestionOrMoveEnd,
                Some("ConCommandInput > Input"),
            ),
            KeyBinding::new(
                "ctrl-w",
                DeletePreviousWord,
                Some("ConCommandInput > Input"),
            ),
            KeyBinding::new("up", HistoryPrevious, Some("ConCommandInput > Input")),
            KeyBinding::new("down", HistoryNext, Some("ConCommandInput > Input")),
        ]);
    }

    fn current_input_state(&self) -> Entity<InputState> {
        match self.mode {
            InputMode::Agent => self.agent_input_state.clone(),
            InputMode::Smart | InputMode::Shell => self.shell_input_state.clone(),
        }
    }

    fn truncate_label(text: &str, truncate_at: usize, ellipsis_threshold: usize) -> String {
        if text.len() > ellipsis_threshold {
            format!("{}…", &text[..text.floor_char_boundary(truncate_at)])
        } else {
            text.to_string()
        }
    }

    fn pane_scope_title(pane: &PaneInfo) -> String {
        if let Some(host) = &pane.hostname {
            Self::truncate_label(host, 14, 18)
        } else if pane.name.is_empty() {
            format!("Pane {}", pane.id)
        } else {
            Self::truncate_label(&pane.name, 14, 18)
        }
    }

    fn effective_target_ids(&self) -> Vec<usize> {
        match self.pane_scope_mode {
            PaneScopeMode::Broadcast => {
                if self.panes.is_empty() {
                    vec![self.focused_pane_id]
                } else {
                    self.panes.iter().map(|pane| pane.id).collect()
                }
            }
            PaneScopeMode::Focused => vec![self.focused_pane_id],
            PaneScopeMode::Custom => {
                if self.selected_pane_ids.is_empty() {
                    vec![self.focused_pane_id]
                } else {
                    self.selected_pane_ids.clone()
                }
            }
        }
    }

    fn all_panes_selected(&self) -> bool {
        matches!(self.pane_scope_mode, PaneScopeMode::Broadcast)
            || (!self.panes.is_empty() && self.selected_pane_ids.len() == self.panes.len())
    }

    fn pane_scope_summary(&self) -> (String, &'static str) {
        match self.pane_scope_mode {
            PaneScopeMode::Broadcast => ("All panes".to_string(), "phosphor/squares-four.svg"),
            PaneScopeMode::Focused => ("Focused".to_string(), "phosphor/target.svg"),
            PaneScopeMode::Custom => {
                let targets = self.effective_target_ids();
                if targets.len() > 1 {
                    (
                        format!("{} panes", targets.len()),
                        "phosphor/selection-plus.svg",
                    )
                } else {
                    let title = self
                        .panes
                        .iter()
                        .find(|pane| pane.id == targets[0])
                        .map(Self::pane_scope_title)
                        .unwrap_or_else(|| "Focused".to_string());
                    (title, "phosphor/target.svg")
                }
            }
        }
    }

    fn subscribe_input_state(
        input_state: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        let tracked_state = input_state.clone();
        let tracked_entity_id = tracked_state.entity_id();
        cx.subscribe_in(&tracked_state, window, {
            move |this, _, ev: &InputEvent, window, cx| {
                if tracked_entity_id != this.current_input_state().entity_id() {
                    return;
                }

                match ev {
                    InputEvent::Change => {
                        this.clear_completion_ui();
                        let current_value = this.current_input_state().read(cx).value().to_string();
                        if let Some(history_ix) = this.history_nav_index {
                            let matches_current = this
                                .recent_commands
                                .get(history_ix)
                                .is_some_and(|entry| entry == &current_value);
                            if !matches_current {
                                this.history_nav_index = None;
                                this.history_nav_draft = None;
                            }
                        }
                        let matches = this.filtered_skills(cx);
                        if matches.is_empty() {
                            this.skill_selection = 0;
                        } else {
                            this.skill_selection =
                                this.skill_selection.min(matches.len().saturating_sub(1));
                        }
                        cx.emit(SkillAutocompleteChanged);
                        cx.emit(InputEdited);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        if this.shift_enter {
                            this.shift_enter = false;
                            return;
                        }

                        let active_state = this.current_input_state();
                        active_state.update(cx, |s, cx| {
                            let cursor = s.cursor();
                            let val = s.value().to_string();
                            if cursor > 0 && val.as_bytes().get(cursor - 1) == Some(&b'\n') {
                                let mut cleaned = val[..cursor - 1].to_string();
                                cleaned.push_str(&val[cursor..]);
                                s.set_value(&cleaned, window, cx);
                            }
                        });

                        let matches = this.filtered_skills(cx);
                        if !matches.is_empty() {
                            let idx = this.skill_selection.min(matches.len().saturating_sub(1));
                            let name = matches[idx].name.clone();
                            this.complete_skill(&name, window, cx);
                        } else if this.has_path_completion_candidates() {
                            let _ = this.accept_selected_path_completion(window, cx);
                        } else {
                            let value = this.current_input_state().read(cx).value();
                            if !value.trim().is_empty() {
                                cx.emit(SubmitInput);
                            }
                        }
                    }
                    _ => {}
                }
            }
        })
    }

    fn is_current_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.current_input_state()
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }

    fn should_fallback_handle_history_key(event: &KeystrokeEvent) -> bool {
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

    fn handle_history_navigation_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let matches = self.filtered_skills(cx);
        let has_completions = !matches.is_empty();

        match key {
            "up" if has_completions => {
                self.skill_selection = self.skill_selection.saturating_sub(1);
                cx.emit(SkillAutocompleteChanged);
                cx.notify();
                true
            }
            "up" if self.has_path_completion_candidates() => {
                self.navigate_path_completion(true, cx)
            }
            "up" => self.navigate_history(true, window, cx),
            "down" if has_completions => {
                self.skill_selection =
                    (self.skill_selection + 1).min(matches.len().saturating_sub(1));
                cx.emit(SkillAutocompleteChanged);
                cx.notify();
                true
            }
            "down" if self.has_path_completion_candidates() => {
                self.navigate_path_completion(false, cx)
            }
            "down" => self.navigate_history(false, window, cx),
            _ => false,
        }
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let agent_input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Ask anything…")
                .auto_grow(1, INPUT_BAR_MAX_ROWS)
        });
        let shell_input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Type a command or ask AI…")
                .code_editor("con-shell")
                .auto_grow(1, INPUT_BAR_MAX_ROWS)
        });
        shell_input_state.update(cx, |state, cx| {
            state.set_highlighter("con-shell", cx);
            // Warm the single-line shell highlighter up-front so first focus/type
            // does not pay parser initialization latency on the UI path.
            state.set_value(":", window, cx);
            state.set_value("", window, cx);
            state.set_cursor_position(Position::new(0, 0), window, cx);
        });
        let _subscriptions = vec![
            // Track shift state on enter keystrokes — fires BEFORE PressEnter
            cx.observe_keystrokes(|this, event, _window, _cx| {
                if event.keystroke.key == "enter" {
                    this.shift_enter = event.keystroke.modifiers.shift;
                }
            }),
            cx.observe_keystrokes(|this, event, window, cx| {
                if !Self::should_fallback_handle_history_key(event)
                    || !this.is_current_input_focused(window, cx)
                {
                    return;
                }

                let key = event.keystroke.key.clone();
                let _ = this.handle_history_navigation_key(key.as_str(), window, cx);
            }),
            Self::subscribe_input_state(&agent_input_state, window, cx),
            Self::subscribe_input_state(&shell_input_state, window, cx),
        ];

        Self {
            agent_input_state,
            shell_input_state,
            mode: InputMode::Smart,
            cwd: "~".to_string(),
            panes: Vec::new(),
            pane_scope_mode: PaneScopeMode::Focused,
            selected_pane_ids: Vec::new(),
            last_single_target_id: None,
            focused_pane_id: 0,
            recent_commands: Vec::new(),
            history_nav_index: None,
            history_nav_draft: None,
            skills: Vec::new(),
            skill_selection: 0,
            inline_suggestion_prefix: None,
            inline_suggestion_suffix: None,
            inline_suggestion_source: None,
            path_completion_candidates: Vec::new(),
            path_completion_selection: 0,
            shift_enter: false,
            ui_opacity: 0.90,
            _subscriptions,
        }
    }

    pub fn take_content(&self, window: &mut Window, cx: &mut App) -> String {
        let input_state = self.current_input_state();
        let value = input_state.read(cx).value().to_string();
        input_state.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        value
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    pub fn target_pane_ids(&self) -> Vec<usize> {
        self.effective_target_ids()
    }

    pub fn set_cwd(&mut self, cwd: String, cx: &mut Context<Self>) {
        if self.cwd == cwd {
            return;
        }
        self.cwd = cwd;
        cx.notify();
    }

    pub fn set_panes(
        &mut self,
        panes: Vec<PaneInfo>,
        focused_id: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.focused_pane_id == focused_id && self.panes == panes {
            return;
        }

        let was_multi_pane = self.panes.len() > 1;
        let previous_focused_id = self.focused_pane_id;
        self.panes = panes;
        self.focused_pane_id = focused_id;
        let valid_ids: Vec<usize> = self.panes.iter().map(|p| p.id).collect();
        self.selected_pane_ids.retain(|id| valid_ids.contains(id));
        if let Some(last_single) = self.last_single_target_id {
            if !valid_ids.contains(&last_single) {
                self.last_single_target_id = None;
            }
        }
        if self.selected_pane_ids.is_empty() && previous_focused_id != focused_id {
            self.last_single_target_id = Some(focused_id);
        }
        if !was_multi_pane && self.panes.len() > 1 {
            self.pane_scope_mode = PaneScopeMode::Broadcast;
            self.selected_pane_ids.clear();
        } else if self.panes.len() <= 1 {
            self.pane_scope_mode = PaneScopeMode::Focused;
            self.selected_pane_ids.clear();
        } else if matches!(self.pane_scope_mode, PaneScopeMode::Focused)
            && self.last_single_target_id.is_none()
        {
            self.pane_scope_mode = PaneScopeMode::Broadcast;
        } else if matches!(self.pane_scope_mode, PaneScopeMode::Custom)
            && self.selected_pane_ids.is_empty()
        {
            self.pane_scope_mode = PaneScopeMode::Broadcast;
        }
        cx.notify();
    }

    pub fn set_skills(&mut self, skills: Vec<SkillEntry>, cx: &mut Context<Self>) {
        if self.skills == skills {
            return;
        }
        self.skills = skills;
        cx.notify();
    }

    pub fn set_recent_commands(&mut self, commands: Vec<String>, cx: &mut Context<Self>) {
        let commands = commands
            .into_iter()
            .filter(|command| !command.contains('\n'))
            .collect::<Vec<_>>();

        if self.recent_commands == commands {
            return;
        }
        self.recent_commands = commands;
        self.history_nav_index = None;
        self.history_nav_draft = None;
        cx.notify();
    }

    pub fn current_text(&self, cx: &App) -> String {
        self.current_input_state().read(cx).value().to_string()
    }

    pub fn clear_inline_suggestion(&mut self) {
        self.inline_suggestion_prefix = None;
        self.inline_suggestion_suffix = None;
        self.inline_suggestion_source = None;
    }

    pub fn clear_path_completion_candidates(&mut self) {
        self.path_completion_candidates.clear();
        self.path_completion_selection = 0;
    }

    pub fn clear_completion_ui(&mut self) {
        self.clear_inline_suggestion();
        self.clear_path_completion_candidates();
    }

    fn set_inline_suggestion(&mut self, prefix: &str, suggestion: &str, source: SuggestionSource) {
        if prefix.is_empty() || suggestion.is_empty() {
            self.clear_completion_ui();
            return;
        }

        let suffix = if let Some(stripped) = suggestion.strip_prefix(prefix) {
            stripped
        } else {
            suggestion
        };
        if suffix.is_empty() || suffix.contains('\n') || suffix.contains('\r') {
            self.clear_completion_ui();
            return;
        }

        self.clear_path_completion_candidates();
        self.inline_suggestion_prefix = Some(prefix.to_string());
        self.inline_suggestion_suffix = Some(suffix.to_string());
        self.inline_suggestion_source = Some(source);
    }

    pub fn set_path_completion_candidates(&mut self, prefix: &str, candidates: Vec<String>) {
        let mut candidates = candidates
            .into_iter()
            .filter(|candidate| candidate != prefix)
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();

        if prefix.is_empty() || candidates.is_empty() {
            self.clear_path_completion_candidates();
            return;
        }

        self.clear_inline_suggestion();
        self.path_completion_candidates = candidates;
        self.path_completion_selection = 0;
    }

    pub fn path_completion_candidates(&self) -> Vec<String> {
        self.path_completion_candidates.clone()
    }

    pub fn path_completion_selection(&self) -> usize {
        self.path_completion_selection
    }

    pub fn has_path_completion_candidates(&self) -> bool {
        !self.path_completion_candidates.is_empty()
    }

    pub fn set_history_inline_suggestion(&mut self, prefix: &str, suggestion: &str) {
        self.set_inline_suggestion(prefix, suggestion, SuggestionSource::History);
    }

    pub fn set_path_inline_suggestion(&mut self, prefix: &str, suggestion: &str) {
        self.set_inline_suggestion(prefix, suggestion, SuggestionSource::Path);
    }

    pub fn set_ai_inline_suggestion(&mut self, prefix: &str, suggestion: &str) {
        self.set_inline_suggestion(prefix, suggestion, SuggestionSource::Ai);
    }

    pub fn accept_inline_suggestion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(suffix) = self.inline_suggestion_suffix.clone() else {
            return false;
        };

        let input_state = self.current_input_state();
        let text = input_state.read(cx).value().to_string();
        let cursor = input_state.read(cx).cursor();
        if cursor != text.len() {
            self.clear_completion_ui();
            return false;
        }

        let mut completed = text;
        completed.push_str(&suffix);
        let completed_chars = completed.chars().count() as u32;
        input_state.update(cx, |s, cx| {
            s.set_value(&completed, window, cx);
            s.set_cursor_position(Position::new(0, completed_chars), window, cx);
        });
        self.clear_completion_ui();
        cx.emit(InputEdited);
        cx.notify();
        true
    }

    pub fn accept_selected_path_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(candidate) = self
            .path_completion_candidates
            .get(self.path_completion_selection)
            .cloned()
        else {
            return false;
        };

        let input_state = self.current_input_state();
        let text = input_state.read(cx).value().to_string();
        let cursor = input_state.read(cx).cursor();
        if cursor != text.len() {
            self.clear_completion_ui();
            return false;
        }

        let completed_chars = candidate.chars().count() as u32;
        input_state.update(cx, |s, cx| {
            s.set_value(&candidate, window, cx);
            s.set_cursor_position(Position::new(0, completed_chars), window, cx);
        });
        self.clear_completion_ui();
        cx.emit(InputEdited);
        cx.notify();
        true
    }

    pub fn accept_path_completion_candidate_at(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if index >= self.path_completion_candidates.len() {
            return false;
        }
        self.path_completion_selection = index;
        self.accept_selected_path_completion(window, cx)
    }

    fn navigate_path_completion(&mut self, previous: bool, cx: &mut Context<Self>) -> bool {
        if self.path_completion_candidates.is_empty() {
            return false;
        }

        self.path_completion_selection = if previous {
            self.path_completion_selection.saturating_sub(1)
        } else {
            (self.path_completion_selection + 1)
                .min(self.path_completion_candidates.len().saturating_sub(1))
        };
        cx.notify();
        true
    }

    fn navigate_history(
        &mut self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.recent_commands.is_empty() {
            return false;
        }

        let next_index = match (previous, self.history_nav_index) {
            (true, None) => {
                self.history_nav_draft =
                    Some(self.current_input_state().read(cx).value().to_string());
                Some(0)
            }
            (true, Some(ix)) if ix + 1 < self.recent_commands.len() => Some(ix + 1),
            (true, Some(ix)) => Some(ix),
            (false, Some(0)) => None,
            (false, Some(ix)) => Some(ix - 1),
            (false, None) => None,
        };

        self.history_nav_index = next_index;
        let replacement = next_index
            .and_then(|ix| self.recent_commands.get(ix).cloned())
            .or_else(|| self.history_nav_draft.clone())
            .unwrap_or_default();
        let cursor = Position::new(0, replacement.chars().count() as u32);

        self.current_input_state().update(cx, |state, cx| {
            state.set_value(&replacement, window, cx);
            state.set_cursor_position(cursor, window, cx);
        });
        self.clear_completion_ui();
        cx.emit(InputEdited);
        cx.notify();
        true
    }

    /// Return matching skills if the input starts with `/`.
    /// Public so the workspace can render the popup at overlay level.
    pub fn filtered_skills(&self, cx: &App) -> Vec<&SkillEntry> {
        let text = self.current_input_state().read(cx).value().to_string();
        let trimmed = text.trim();
        if !trimmed.starts_with('/') {
            return Vec::new();
        }
        let query = &trimmed[1..].to_lowercase();
        if query.contains(' ') {
            // Already has args — no autocomplete
            return Vec::new();
        }
        self.skills
            .iter()
            .filter(|s| query.is_empty() || s.name.to_lowercase().starts_with(query))
            .collect()
    }

    pub fn skill_selection(&self) -> usize {
        self.skill_selection
    }

    pub fn complete_skill(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.current_input_state().update(cx, |s, cx| {
            s.set_value(&format!("/{name} "), window, cx);
        });
        self.skill_selection = 0;
        self.clear_completion_ui();
        cx.emit(SkillAutocompleteChanged);
        cx.emit(InputEdited);
        cx.notify();
    }

    pub fn skill_popup_offset(&self, cx: &App) -> Pixels {
        let mono_scale = mono_density_scale(cx.theme());
        px(
            (56.0 + (self.content_rows(cx).saturating_sub(1) as f32 * INPUT_BAR_ROW_HEIGHT))
                * mono_scale,
        )
    }

    pub fn rendered_height(&self, cx: &App) -> Pixels {
        px(input_bar_rendered_height_for_rows(
            self.content_rows(cx),
            mono_density_scale(cx.theme()),
        ))
    }

    fn content_rows(&self, cx: &App) -> usize {
        input_bar_content_rows(&self.current_input_state().read(cx).value())
    }

    pub fn cycle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(self.mode.next(), window, cx);
        self.clear_completion_ui();
        cx.emit(InputScopeChanged);
        cx.emit(InputEdited);
        cx.notify();
    }

    /// Switch to Agent mode and append quoted context (e.g. a terminal
    /// selection) to the input, leaving the cursor at the end so the user
    /// can type their question right away.
    pub fn ask_ai_with_context(
        &mut self,
        context: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode != InputMode::Agent {
            self.set_mode(InputMode::Agent, window, cx);
        }
        self.clear_completion_ui();

        if let Some(context) = context.map(str::trim_end).filter(|text| !text.is_empty()) {
            let state = self.current_input_state();
            let current = state.read(cx).value().to_string();
            let updated = append_ask_ai_context(&current, context);
            let end = text_end_position(&updated);
            state.update(cx, |s, cx| {
                s.set_value(&updated, window, cx);
                s.set_cursor_position(end, window, cx);
            });
        }

        cx.emit(InputScopeChanged);
        cx.emit(InputEdited);
        cx.notify();
    }

    pub fn pane_infos(&self) -> Vec<PaneInfo> {
        self.panes.clone()
    }

    pub fn is_broadcast_scope(&self) -> bool {
        matches!(self.pane_scope_mode, PaneScopeMode::Broadcast)
    }

    pub fn is_focused_scope(&self) -> bool {
        matches!(self.pane_scope_mode, PaneScopeMode::Focused)
    }

    pub fn scope_selected_ids(&self) -> Vec<usize> {
        self.effective_target_ids()
    }

    pub fn focused_pane_id(&self) -> usize {
        self.focused_pane_id
    }

    pub fn set_focused_scope(&mut self, cx: &mut Context<Self>) {
        self.pane_scope_mode = PaneScopeMode::Focused;
        self.selected_pane_ids.clear();
        self.last_single_target_id = Some(self.focused_pane_id);
        self.clear_completion_ui();
        cx.emit(InputScopeChanged);
        cx.emit(InputEdited);
        cx.notify();
    }

    pub fn set_broadcast_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pane_scope_mode = PaneScopeMode::Broadcast;
        self.selected_pane_ids.clear();
        if self.panes.len() > 1 {
            self.set_mode(InputMode::Shell, window, cx);
        }
        self.clear_completion_ui();
        cx.emit(InputScopeChanged);
        cx.emit(InputEdited);
        cx.notify();
    }

    pub fn toggle_scope_pane(
        &mut self,
        pane_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut selected = self.effective_target_ids();
        if let Some(ix) = selected.iter().position(|id| *id == pane_id) {
            selected.remove(ix);
        } else {
            selected.push(pane_id);
        }
        selected.sort_unstable();
        selected.dedup();

        if selected.is_empty() || (selected.len() == 1 && selected[0] == self.focused_pane_id) {
            self.set_focused_scope(cx);
            return;
        } else if !self.panes.is_empty() && selected.len() == self.panes.len() {
            self.set_broadcast_scope(window, cx);
            return;
        } else {
            self.pane_scope_mode = PaneScopeMode::Custom;
            self.selected_pane_ids = selected;
            if self.selected_pane_ids.len() > 1 {
                self.set_mode(InputMode::Shell, window, cx);
            }
        }
        self.clear_completion_ui();
        cx.emit(InputScopeChanged);
        cx.emit(InputEdited);
        cx.notify();
    }

    fn set_mode(&mut self, mode: InputMode, window: &mut Window, cx: &mut Context<Self>) {
        let current_state = self.current_input_state();
        let current_value = current_state.read(cx).value().to_string();
        let current_position = current_state.read(cx).cursor_position();
        let was_focused = current_state.read(cx).focus_handle(cx).is_focused(window);

        self.mode = mode;
        self.history_nav_index = None;
        self.history_nav_draft = None;
        self.clear_completion_ui();
        let placeholder = self.placeholder().to_string();
        let next_state = self.current_input_state();
        next_state.update(cx, |s, cx| {
            s.set_placeholder(&placeholder, window, cx);
            s.set_value(&current_value, window, cx);
            s.set_cursor_position(current_position, window, cx);
            if was_focused {
                s.focus(window, cx);
            }
        });
    }

    pub fn set_ui_opacity(&mut self, opacity: f32) {
        self.ui_opacity = opacity.clamp(0.35, 1.0);
    }

    fn move_cursor_to_line_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input_state = self.current_input_state();
        let cursor_position = input_state.read(cx).cursor_position();
        let line_index = cursor_position.line as usize;
        let line_length = input_state
            .read(cx)
            .value()
            .lines()
            .nth(line_index)
            .map(|line| line.chars().count() as u32)
            .unwrap_or(cursor_position.character);

        input_state.update(cx, |state, cx| {
            state.set_cursor_position(Position::new(cursor_position.line, line_length), window, cx);
        });
    }

    fn move_cursor_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input_state = self.current_input_state();
        let text = input_state.read(cx).value().to_string();
        let cursor = input_state.read(cx).cursor();
        if cursor >= text.len() {
            return;
        }

        let next_offset = text[cursor..]
            .chars()
            .next()
            .map(|ch| cursor + ch.len_utf8())
            .unwrap_or(cursor);
        let next_position = Self::position_for_offset(&text, next_offset);

        input_state.update(cx, |state, cx| {
            state.set_cursor_position(next_position, window, cx);
        });
    }

    fn delete_previous_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input_state = self.current_input_state();
        let text = input_state.read(cx).value().to_string();
        let cursor = input_state.read(cx).cursor();
        if cursor == 0 {
            return;
        }

        let mut boundary = cursor;
        while boundary > 0 {
            let prev = text[..boundary]
                .char_indices()
                .last()
                .map(|(idx, ch)| (idx, ch))
                .unwrap();
            if !prev.1.is_whitespace() {
                break;
            }
            boundary = prev.0;
        }
        while boundary > 0 {
            let prev = text[..boundary]
                .char_indices()
                .last()
                .map(|(idx, ch)| (idx, ch))
                .unwrap();
            if prev.1.is_whitespace() {
                break;
            }
            boundary = prev.0;
        }

        let mut updated = text[..boundary].to_string();
        updated.push_str(&text[cursor..]);
        let boundary_position = Self::position_for_offset(&updated, boundary);

        input_state.update(cx, |state, cx| {
            state.set_value(&updated, window, cx);
            state.set_cursor_position(boundary_position, window, cx);
        });
        self.clear_completion_ui();
        cx.emit(InputEdited);
        cx.notify();
    }

    fn position_for_offset(text: &str, offset: usize) -> Position {
        let safe_offset = offset.min(text.len());
        let prefix = &text[..safe_offset];
        let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
        let character = prefix
            .rsplit_once('\n')
            .map(|(_, tail)| tail.chars().count() as u32)
            .unwrap_or_else(|| prefix.chars().count() as u32);
        Position::new(line, character)
    }

    fn placeholder(&self) -> &str {
        match self.mode {
            InputMode::Smart => "Type a command or ask AI…",
            InputMode::Shell => "Run a command…",
            InputMode::Agent => "Ask anything…",
        }
    }

    fn command_overlay_runs(
        input: &str,
        theme: &gpui_component::Theme,
        font_size: Pixels,
        line_height: Rems,
    ) -> Option<Vec<TextRun>> {
        if input.is_empty() || input.contains('\n') {
            return None;
        }

        let base_style = TextStyle {
            color: theme.foreground.opacity(0.92),
            font_family: theme.mono_font_family.clone(),
            font_size: font_size.into(),
            line_height: line_height.into(),
            font_weight: FontWeight::NORMAL,
            font_style: FontStyle::Normal,
            white_space: WhiteSpace::Nowrap,
            ..Default::default()
        };

        let primary_color = syntax_color(theme, "primary").unwrap_or(theme.primary);
        let flag_color = syntax_color(theme, "keyword").unwrap_or(theme.warning);
        let path_color = syntax_color(theme, "string").unwrap_or(theme.success.opacity(0.88));
        let variable_color =
            syntax_color(theme, "variable.special").unwrap_or(theme.primary.opacity(0.9));
        let number_color = syntax_color(theme, "number").unwrap_or(theme.info.opacity(0.92));
        let operator_color =
            syntax_color(theme, "operator").unwrap_or(theme.muted_foreground.opacity(0.78));
        let punctuation_color = syntax_color(theme, "punctuation.delimiter")
            .unwrap_or(theme.muted_foreground.opacity(0.74));

        let mut runs = Vec::new();
        let mut idx = 0usize;
        let mut saw_command = false;

        while idx < input.len() {
            let ch = input[idx..].chars().next()?;
            let ch_len = ch.len_utf8();

            if ch.is_whitespace() {
                let start = idx;
                idx += ch_len;
                while idx < input.len() {
                    let next = input[idx..].chars().next()?;
                    if !next.is_whitespace() {
                        break;
                    }
                    idx += next.len_utf8();
                }
                runs.push(base_style.to_run(idx - start));
                continue;
            }

            let start = idx;
            let color = if ch == '\'' || ch == '"' {
                idx += ch_len;
                while idx < input.len() {
                    let next = input[idx..].chars().next()?;
                    idx += next.len_utf8();
                    if next == ch {
                        break;
                    }
                }
                path_color
            } else if ch == '$' {
                idx += ch_len;
                while idx < input.len() {
                    let next = input[idx..].chars().next()?;
                    if !(next.is_ascii_alphanumeric() || next == '_' || next == '{' || next == '}')
                    {
                        break;
                    }
                    idx += next.len_utf8();
                }
                variable_color
            } else if is_operator_char(ch) {
                idx += ch_len;
                while idx < input.len() {
                    let next = input[idx..].chars().next()?;
                    if !is_operator_char(next) {
                        break;
                    }
                    idx += next.len_utf8();
                }
                operator_color
            } else if is_punctuation_char(ch) {
                idx += ch_len;
                punctuation_color
            } else {
                idx += ch_len;
                while idx < input.len() {
                    let next = input[idx..].chars().next()?;
                    if next.is_whitespace()
                        || is_operator_char(next)
                        || is_punctuation_char(next)
                        || next == '\''
                        || next == '"'
                    {
                        break;
                    }
                    idx += next.len_utf8();
                }

                let token = &input[start..idx];
                if !saw_command {
                    saw_command = true;
                    primary_color
                } else if token.starts_with('-') {
                    flag_color
                } else if looks_path_token(token) {
                    path_color
                } else if token.starts_with('$') {
                    variable_color
                } else if token.chars().all(|c| c.is_ascii_digit()) {
                    number_color
                } else {
                    base_style.color
                }
            };

            let mut style = base_style.clone();
            style.color = color;
            runs.push(style.to_run(idx - start));
        }

        Some(runs.into_iter().filter(|run| run.len > 0).collect())
    }

    fn should_hide_native_command_text(input: &str, has_command_overlay: bool) -> bool {
        !input.is_empty() && has_command_overlay
    }
}

fn syntax_color(theme: &gpui_component::Theme, name: &str) -> Option<Hsla> {
    theme
        .highlight_theme
        .style
        .syntax
        .style(name)
        .and_then(|style| style.color)
}

fn is_operator_char(ch: char) -> bool {
    matches!(ch, '|' | '&' | '<' | '>' | '=' | ':' | '+' | '*' | '%')
}

fn is_punctuation_char(ch: char) -> bool {
    matches!(ch, ';' | ',' | '(' | ')' | '[' | ']' | '{' | '}')
}

fn looks_path_token(token: &str) -> bool {
    token.starts_with("~/")
        || token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.contains('/')
        || token.starts_with('~')
}

impl EventEmitter<SubmitInput> for InputBar {}
impl EventEmitter<EscapeInput> for InputBar {}

impl Focusable for InputBar {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.current_input_state().read(cx).focus_handle(cx).clone()
    }
}

impl Render for InputBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let input_state = self.current_input_state();
        let has_multiple_panes = self.panes.len() > 1;
        let has_text = !input_state.read(cx).value().trim().is_empty();
        let input_value = input_state.read(cx).value().to_string();
        let input_cursor = input_state.read(cx).cursor();
        let mono_scale = mono_density_scale(theme);
        let control_size = mono_space_px(theme, 28.0);
        let mode_icon_size = mono_space_px(theme, 15.0);
        let compact_icon_size = mono_space_px(theme, 14.0);

        let mode_tint = self.mode.tint(cx);

        // ── Mode prefix — icon-only, minimal ──
        let mode_prefix = div()
            .id("mode-toggle")
            .flex()
            .items_center()
            .justify_center()
            .size(control_size)
            .rounded(px(7.0 * mono_scale))
            .cursor_pointer()
            .bg(mode_tint.opacity(0.075))
            .hover(|s| s.bg(mode_tint.opacity(0.12)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.cycle_mode(window, cx);
                }),
            )
            .tooltip({
                let mode_label = self.mode.tooltip().to_string();
                move |window, cx| {
                    let stroke =
                        crate::keycaps::first_action_keystroke(&crate::CycleInputMode, window);
                    let mode_label = mode_label.clone();
                    Tooltip::element(move |_, cx| {
                        let theme = cx.theme();
                        let mut content = div().flex().items_center().gap(px(7.0)).child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(theme.popover_foreground)
                                .child(format!("{} · switch mode", mode_label)),
                        );
                        if let Some(stroke) = stroke.as_ref() {
                            content =
                                content.child(crate::keycaps::keycaps_for_stroke(stroke, theme));
                        }
                        content
                    })
                    .build(window, cx)
                }
            })
            .child(
                svg()
                    .path(self.mode.icon())
                    .size(mode_icon_size)
                    .text_color(mode_tint),
            );

        // ── Pane target control — scales to many panes without growing the row ──
        let all_selected = self.all_panes_selected();
        let pane_row = if has_multiple_panes && self.mode != InputMode::Agent {
            let (scope_label, scope_icon) = self.pane_scope_summary();
            let scope_is_expanded =
                all_selected || matches!(self.pane_scope_mode, PaneScopeMode::Custom);
            let scope_tint = if scope_is_expanded {
                theme.primary.opacity(0.86)
            } else {
                theme.muted_foreground.opacity(0.78)
            };
            Some(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("pane-scope")
                            .h(control_size)
                            .px(px(11.0 * mono_scale))
                            .rounded(px(8.0 * mono_scale))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(8.0 * mono_scale))
                            .bg(if scope_is_expanded {
                                theme.primary.opacity(0.050)
                            } else {
                                theme.foreground.opacity(0.045)
                            })
                            .hover(|s| {
                                s.bg(if scope_is_expanded {
                                    theme.primary.opacity(0.082)
                                } else {
                                    theme.foreground.opacity(0.065)
                                })
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    cx.emit(TogglePaneScopePicker);
                                    this.current_input_state()
                                        .update(cx, |state, cx| state.focus(window, cx));
                                }),
                            )
                            .child(
                                svg()
                                    .path(scope_icon)
                                    .size(compact_icon_size)
                                    .text_color(scope_tint),
                            )
                            .child(
                                div()
                                    .text_size(mono_px(theme, 11.0))
                                    .line_height(mono_px(theme, 14.0))
                                    .font_family(theme.mono_font_family.clone())
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if scope_is_expanded {
                                        theme.primary.opacity(0.92)
                                    } else {
                                        theme.muted_foreground.opacity(0.76)
                                    })
                                    .max_w(px(128.0 * mono_scale))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(scope_label),
                            ),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };

        // ── Send button — inside container, right edge ──
        let send_button = div()
            .id("send-button")
            .flex()
            .items_center()
            .justify_center()
            .size(control_size)
            .rounded(px(8.0 * mono_scale))
            .cursor_pointer()
            .flex_shrink_0()
            .bg(if has_text {
                theme.primary
            } else {
                theme.foreground.opacity(0.055)
            })
            .hover(|s| {
                if has_text {
                    s.bg(theme.primary_hover)
                } else {
                    s.bg(theme.foreground.opacity(0.075))
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _, _, cx| {
                    cx.emit(SubmitInput);
                }),
            )
            .child(
                svg()
                    .path("phosphor/arrow-up.svg")
                    .size(mono_space_px(theme, 13.0))
                    .text_color(if has_text {
                        theme.primary_foreground
                    } else {
                        theme.muted_foreground.opacity(0.44)
                    }),
            );

        // Shell input is terminal chrome; natural-language agent input uses
        // the UI font so its prose reads like the agent panel.
        let input_font = if self.mode == InputMode::Agent {
            theme.font_family.clone()
        } else {
            theme.mono_font_family.clone()
        };
        let input_text_size = match self.mode {
            InputMode::Agent => mono_px(theme, 14.0),
            _ => mono_px(theme, 13.0),
        };
        let input_line_height = match self.mode {
            InputMode::Agent => rems(1.15),
            _ => rems(1.25),
        };
        // All modes share the same centered row. Agent mode used to apply a
        // negative offset, which made its placeholder visibly sit too high.
        let input_vertical_offset = px(0.0);
        let show_inline_suggestion = self.mode != InputMode::Agent
            && input_cursor == input_value.len()
            && !input_value.is_empty()
            && !input_value.contains('\n')
            && !input_value.trim_start().starts_with('/')
            && self
                .inline_suggestion_prefix
                .as_deref()
                .is_some_and(|prefix| prefix == input_value)
            && self
                .inline_suggestion_suffix
                .as_deref()
                .is_some_and(|suffix| !suffix.is_empty());
        let ghost_tint = match self.inline_suggestion_source {
            Some(SuggestionSource::Path) => theme.success.opacity(0.68),
            Some(SuggestionSource::Ai) => theme.primary.opacity(0.64),
            _ => theme.muted_foreground.opacity(0.56),
        };
        let ghost_prefix = input_value.replace(' ', "\u{00A0}");
        let ghost_suffix = self
            .inline_suggestion_suffix
            .clone()
            .unwrap_or_default()
            .replace(' ', "\u{00A0}");
        let command_overlay_runs = if self.mode != InputMode::Agent {
            Self::command_overlay_runs(&input_value, theme, input_text_size, input_line_height)
        } else {
            None
        };
        let hide_native_command_text =
            Self::should_hide_native_command_text(&input_value, command_overlay_runs.is_some());

        let input_field = div()
            .flex_1()
            .min_h(control_size)
            .relative()
            .key_context("ConCommandInput")
            .on_action(cx.listener(|this, _: &AcceptSuggestion, window, cx| {
                let _ = this.accept_inline_suggestion(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &AcceptSuggestionOrMoveEnd, window, cx| {
                    if !this.accept_inline_suggestion(window, cx) {
                        this.move_cursor_to_line_end(window, cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &AcceptSuggestionOrMoveRight, window, cx| {
                    if !this.accept_inline_suggestion(window, cx) {
                        this.move_cursor_right(window, cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &DeletePreviousWord, window, cx| {
                this.delete_previous_word(window, cx);
            }))
            .on_action(cx.listener(|this, _: &HistoryPrevious, window, cx| {
                let _ = this.navigate_history(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &HistoryNext, window, cx| {
                let _ = this.navigate_history(false, window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let matches = this.filtered_skills(cx);
                let has_completions = !matches.is_empty();
                let mods = event.keystroke.modifiers;
                let key = event.keystroke.key.as_str();

                match key {
                    "tab" => {
                        if has_completions {
                            let idx = this.skill_selection.min(matches.len().saturating_sub(1));
                            let name = matches[idx].name.clone();
                            this.complete_skill(&name, window, cx);
                            window.prevent_default();
                            cx.stop_propagation();
                        } else if this.has_path_completion_candidates() {
                            let moved = if mods.shift {
                                this.navigate_path_completion(true, cx)
                            } else {
                                this.navigate_path_completion(false, cx)
                            };
                            if moved {
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        } else if this.accept_selected_path_completion(window, cx)
                            || this.accept_inline_suggestion(window, cx)
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }
                    "escape" => {
                        if has_completions {
                            this.current_input_state()
                                .update(cx, |s, cx| s.set_value("", window, cx));
                            this.skill_selection = 0;
                            this.clear_completion_ui();
                            cx.emit(SkillAutocompleteChanged);
                            cx.emit(InputEdited);
                            cx.notify();
                            window.prevent_default();
                            cx.stop_propagation();
                        } else if this.has_path_completion_candidates()
                            || this.inline_suggestion_suffix.is_some()
                        {
                            this.clear_completion_ui();
                            cx.emit(InputEdited);
                            cx.notify();
                            window.prevent_default();
                            cx.stop_propagation();
                        } else {
                            cx.emit(EscapeInput);
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }
                    "up" | "down" => {
                        if this.handle_history_navigation_key(key, window, cx) {
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }
                    "right" if !mods.control && !mods.platform && !mods.alt => {
                        if this.accept_selected_path_completion(window, cx)
                            || this.accept_inline_suggestion(window, cx)
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }
                    "end" => {
                        if this.accept_selected_path_completion(window, cx)
                            || this.accept_inline_suggestion(window, cx)
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }
                    "e" if mods.control || mods.platform => {
                        if !(this.accept_selected_path_completion(window, cx)
                            || this.accept_inline_suggestion(window, cx))
                        {
                            this.move_cursor_to_line_end(window, cx);
                        }
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                    "right" if mods.platform => {
                        if this.accept_selected_path_completion(window, cx)
                            || this.accept_inline_suggestion(window, cx)
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }
                    "w" if mods.control || mods.platform => {
                        this.delete_previous_word(window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .font_family(input_font.clone())
            .text_size(input_text_size)
            .children(command_overlay_runs.as_ref().map(|runs| {
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .px(px(12.0 * mono_scale))
                    .flex()
                    .items_center()
                    .line_height(input_line_height)
                    .top(input_vertical_offset)
                    .overflow_hidden()
                    .child(
                        div()
                            .w_full()
                            .overflow_hidden()
                            .font_family(theme.mono_font_family.clone())
                            .text_size(input_text_size)
                            .line_height(input_line_height)
                            .child(StyledText::new(input_value.clone()).with_runs(runs.clone())),
                    )
            }))
            .children(show_inline_suggestion.then(|| {
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .px(px(12.0 * mono_scale))
                    .flex()
                    .items_center()
                    .line_height(input_line_height)
                    .top(input_vertical_offset)
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .child(div().flex_shrink_0().opacity(0.0).child(ghost_prefix))
                            .child(div().text_color(ghost_tint).child(ghost_suffix)),
                    )
            }))
            .child(div().relative().top(input_vertical_offset).child(
                if self.mode == InputMode::Agent {
                    Input::new(&input_state)
                        .appearance(false)
                        .cleanable(false)
                        .font_family(input_font)
                        .text_color(if input_value.is_empty() {
                            theme.muted_foreground.opacity(0.88)
                        } else {
                            theme.foreground.opacity(0.94)
                        })
                        .text_size(input_text_size)
                        .line_height(input_line_height)
                        .pl(px(12.0 * mono_scale))
                        .min_h(control_size)
                        .into_any_element()
                } else {
                    Input::new(&input_state)
                        .appearance(false)
                        .cleanable(false)
                        .font_family(input_font)
                        .text_color(if hide_native_command_text {
                            gpui::transparent_black()
                        } else {
                            theme.foreground
                        })
                        .text_size(input_text_size)
                        .line_height(input_line_height)
                        .min_h(control_size)
                        .into_any_element()
                },
            ));

        // ── Main layout — flat bar, no rounded bubble ──
        div()
            .flex()
            .flex_col()
            .bg(theme.title_bar.opacity(self.ui_opacity))
            .font_family(theme.font_family.clone())
            .text_size(input_text_size)
            // ── Flat container ──
            .child(
                div()
                    .px(px(12.0 * mono_scale))
                    .py(px(6.0 * mono_scale))
                    .min_h(px(44.0 * mono_scale))
                    .flex()
                    .items_center()
                    .gap(px(9.0 * mono_scale))
                    .child(mode_prefix)
                    .children(pane_row)
                    .child(div().flex_1().min_w_0().child(input_field))
                    .child(send_button),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        INPUT_BAR_MAX_ROWS, InputBar, append_ask_ai_context, input_bar_content_rows,
        input_bar_rendered_height_for_rows, text_end_position,
    };
    use gpui_component::input::Position;

    #[test]
    fn content_rows_counts_trailing_newline_and_clamps() {
        assert_eq!(input_bar_content_rows(""), 1);
        assert_eq!(input_bar_content_rows("cmd"), 1);
        assert_eq!(input_bar_content_rows("cmd\n"), 2);
        assert_eq!(input_bar_content_rows("cmd\nnext"), 2);

        let many_rows = "1\n2\n3\n4\n5\n6\n7\n8";
        assert_eq!(input_bar_content_rows(many_rows), INPUT_BAR_MAX_ROWS);
    }

    #[test]
    fn rendered_height_scales_minimum_and_extra_rows() {
        assert_eq!(input_bar_rendered_height_for_rows(1, 1.0), 44.0);
        assert_eq!(input_bar_rendered_height_for_rows(3, 1.0), 84.0);
        assert_eq!(input_bar_rendered_height_for_rows(3, 1.25), 105.0);
    }

    #[test]
    fn native_command_text_stays_visible_when_multiline_overlay_is_unavailable() {
        assert!(!InputBar::should_hide_native_command_text(
            "echo one\necho two",
            false
        ));
    }

    #[test]
    fn native_command_text_hides_when_single_line_overlay_is_available() {
        assert!(InputBar::should_hide_native_command_text("echo one", true));
    }

    #[test]
    fn ask_ai_context_wraps_selection_in_fence() {
        assert_eq!(
            append_ask_ai_context("", "error: it broke"),
            "```\nerror: it broke\n```\n"
        );
    }

    #[test]
    fn ask_ai_context_appends_after_existing_input() {
        assert_eq!(
            append_ask_ai_context("why does this fail?  ", "error: it broke"),
            "why does this fail?\n```\nerror: it broke\n```\n"
        );
    }

    #[test]
    fn ask_ai_context_fence_grows_past_embedded_backticks() {
        let context = "see ```rust\ncode\n```";
        let updated = append_ask_ai_context("", context);
        assert!(updated.starts_with("````\n"));
        assert!(updated.ends_with("\n````\n"));
    }

    #[test]
    fn text_end_position_counts_lines_and_last_column() {
        assert_eq!(text_end_position(""), Position::new(0, 0));
        assert_eq!(text_end_position("ask"), Position::new(0, 3));
        assert_eq!(text_end_position("a\n```\nxy\n```\n"), Position::new(4, 0));
    }
}
