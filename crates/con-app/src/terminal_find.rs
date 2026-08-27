use std::sync::Arc;
use std::time::Duration;

use con_ghostty::GhosttyTerminal;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Disableable as _, Icon, Sizable as _};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

pub struct TerminalFindDismissed;

pub struct TerminalFind {
    terminal: Arc<GhosttyTerminal>,
    terminal_focus: FocusHandle,
    input: Entity<InputState>,
    query: String,
    query_generation: u64,
    search_active: bool,
    total: Option<usize>,
    selected: Option<usize>,
    ended: bool,
}

impl EventEmitter<TerminalFindDismissed> for TerminalFind {}

impl TerminalFind {
    pub fn new(
        terminal: Arc<GhosttyTerminal>,
        terminal_focus: FocusHandle,
        needle: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("Find");
            if !needle.is_empty() {
                let mut state = state;
                state.set_value(needle.clone(), window, cx);
                state
            } else {
                state
            }
        });

        cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query_changed(cx);
                }
            },
        )
        .detach();

        let mut find = Self {
            terminal,
            terminal_focus,
            input,
            query: needle.clone(),
            query_generation: 0,
            search_active: false,
            total: None,
            selected: None,
            ended: false,
        };
        if !needle.is_empty() {
            find.request_search(needle, cx);
        }
        find
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    pub fn set_needle(&mut self, needle: String, window: &mut Window, cx: &mut Context<Self>) {
        if !needle.is_empty() && needle != self.query {
            self.query = needle.clone();
            self.input
                .update(cx, |input, cx| input.set_value(needle.clone(), window, cx));
            self.request_search(needle, cx);
        }
        self.focus(window, cx);
    }

    pub fn set_total(&mut self, total: Option<usize>, cx: &mut Context<Self>) {
        if self.total != total {
            self.total = total;
            cx.notify();
        }
    }

    pub fn set_selected(&mut self, selected: Option<usize>, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
            cx.notify();
        }
    }

    pub fn mark_ended(&mut self) {
        self.ended = true;
        self.query_generation = self.query_generation.wrapping_add(1);
    }

    pub fn end(&mut self) {
        self.finish_search();
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        if query == self.query {
            return;
        }
        self.query = query.clone();
        self.request_search(query, cx);
    }

    fn request_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.query_generation = self.query_generation.wrapping_add(1);
        let generation = self.query_generation;
        self.total = None;
        self.selected = None;
        cx.notify();

        if query.is_empty() || query.chars().nth(2).is_some() {
            self.submit_query(&query);
            return;
        }

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                if this.query_generation == generation && this.query == query && !this.ended {
                    this.submit_query(&query);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn submit_query(&mut self, query: &str) {
        if query.is_empty() && !self.search_active {
            return;
        }

        let search_was_active = self.search_active;
        match self.terminal.search(query) {
            Ok(true) => self.search_active = !query.is_empty(),
            Ok(false) => {
                self.search_active = false;
                if search_was_active {
                    self.clear_native_search();
                }
                if !query.is_empty() {
                    log::warn!("Ghostty rejected terminal search");
                }
            }
            Err(err) => {
                self.search_active = false;
                if search_was_active {
                    self.clear_native_search();
                }
                log::error!("Failed to search terminal: {err}");
            }
        }
    }

    fn clear_native_search(&self) {
        if let Err(err) = self.terminal.search("") {
            log::error!("Failed to clear terminal search: {err}");
        }
    }

    fn navigate_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_active
            && let Err(err) = self.terminal.navigate_search_next()
        {
            log::error!("Failed to select next terminal search match: {err}");
        }
        self.focus(window, cx);
    }

    fn navigate_previous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_active
            && let Err(err) = self.terminal.navigate_search_previous()
        {
            log::error!("Failed to select previous terminal search match: {err}");
        }
        self.focus(window, cx);
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_search();
        window.focus(&self.terminal_focus, cx);
        cx.emit(TerminalFindDismissed);
    }

    fn finish_search(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.query_generation = self.query_generation.wrapping_add(1);
        if let Err(err) = self.terminal.end_search() {
            log::error!("Failed to end terminal search: {err}");
        }
    }

    fn count_label(&self) -> Option<String> {
        match (self.selected, self.total) {
            (Some(selected), Some(total)) => Some(format!("{}/{total}", selected + 1)),
            (Some(selected), None) => Some(format!("{}/?", selected + 1)),
            (None, Some(total)) => Some(format!("0/{total}")),
            (None, None) => None,
        }
    }
}

impl Drop for TerminalFind {
    fn drop(&mut self) {
        self.finish_search();
    }
}

impl Render for TerminalFind {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let icon_color = theme.muted_foreground.opacity(0.82);
        let disabled = !self.search_active;
        let entity = cx.entity().downgrade();

        let previous = Button::new("terminal-find-previous")
            .icon(
                Icon::default()
                    .path("phosphor/caret-up.svg")
                    .text_color(icon_color),
            )
            .xsmall()
            .compact()
            .ghost()
            .tab_stop(false)
            .disabled(disabled)
            .tooltip("Previous match")
            .on_click({
                let entity = entity.clone();
                move |_, window, cx| {
                    let _ = entity.update(cx, |this, cx| this.navigate_previous(window, cx));
                }
            });
        let next = Button::new("terminal-find-next")
            .icon(
                Icon::default()
                    .path("phosphor/caret-down.svg")
                    .text_color(icon_color),
            )
            .xsmall()
            .compact()
            .ghost()
            .tab_stop(false)
            .disabled(disabled)
            .tooltip("Next match")
            .on_click({
                let entity = entity.clone();
                move |_, window, cx| {
                    let _ = entity.update(cx, |this, cx| this.navigate_next(window, cx));
                }
            });
        let close = Button::new("terminal-find-close")
            .icon(
                Icon::default()
                    .path("phosphor/x.svg")
                    .text_color(icon_color),
            )
            .xsmall()
            .compact()
            .ghost()
            .tab_stop(false)
            .tooltip("Close")
            .on_click({
                let entity = entity.clone();
                move |_, window, cx| {
                    let _ = entity.update(cx, |this, cx| this.dismiss(window, cx));
                }
            });

        div()
            .id("terminal-find")
            .absolute()
            .top(px(8.0))
            .right(px(8.0))
            .h(px(40.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(8.0))
            .rounded(px(8.0))
            .overflow_hidden()
            .occlude()
            .bg(theme.popover.opacity(0.96))
            .font_family(theme.mono_font_family.clone())
            .text_size(px(12.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => {
                        if event.keystroke.modifiers.shift {
                            this.navigate_previous(window, cx);
                        } else {
                            this.navigate_next(window, cx);
                        }
                    }
                    "escape" => this.dismiss(window, cx),
                    _ => return,
                }
                window.prevent_default();
                cx.stop_propagation();
            }))
            .child(
                Icon::default()
                    .path("phosphor/magnifying-glass.svg")
                    .text_color(icon_color)
                    .size(px(14.0)),
            )
            .child(
                Input::new(&self.input)
                    .appearance(false)
                    .focus_bordered(false)
                    .small()
                    .w(px(180.0)),
            )
            .child(
                div()
                    .w(px(48.0))
                    .text_align(TextAlign::Right)
                    .text_color(theme.muted_foreground.opacity(0.78))
                    .child(self.count_label().unwrap_or_default()),
            )
            .child(previous)
            .child(next)
            .child(close)
    }
}
