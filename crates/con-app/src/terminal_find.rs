use std::sync::Arc;
use std::time::Duration;

use con_ghostty::GhosttyTerminal;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Disableable as _, Icon, Sizable as _};

use crate::ui_scale::mono_icon_px;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

pub struct TerminalFindDismissed;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub struct TerminalFindUpdated;

pub struct TerminalFind {
    #[cfg(target_os = "macos")]
    terminal: Arc<GhosttyTerminal>,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    commands: futures::channel::mpsc::UnboundedSender<portable::Command>,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    navigation_pending: bool,
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
#[cfg(any(target_os = "linux", target_os = "windows"))]
impl EventEmitter<TerminalFindUpdated> for TerminalFind {}

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
            #[cfg(target_os = "macos")]
            terminal,
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            commands: portable::spawn(terminal, window, cx),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            navigation_pending: false,
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

    #[cfg(target_os = "macos")]
    pub fn mark_ended(&mut self) {
        self.ended = true;
        self.query_generation = self.query_generation.wrapping_add(1);
    }

    #[cfg(target_os = "macos")]
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

        if query.is_empty() || (cfg!(target_os = "macos") && query.chars().nth(2).is_some()) {
            self.submit_query(&query);
            return;
        }

        // Do not navigate or display the previous needle during debounce.
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        self.submit_query("");
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

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn submit_query(&mut self, query: &str) {
        if query.is_empty() && !self.search_active {
            return;
        }
        self.search_active = !query.is_empty();
        self.navigation_pending = false;
        let _ = self.commands.unbounded_send(portable::Command::Query(
            self.query_generation,
            query.to_owned(),
        ));
    }

    #[cfg(target_os = "macos")]
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

    #[cfg(target_os = "macos")]
    fn clear_native_search(&self) {
        if let Err(err) = self.terminal.search("") {
            log::error!("Failed to clear terminal search: {err}");
        }
    }

    fn navigate_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if self.search_active
            && let Err(err) = self.terminal.navigate_search_next()
        {
            log::error!("Failed to select next terminal search match: {err}");
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if self.search_active && !self.ended && !self.navigation_pending {
            self.navigation_pending = true;
            self.query_generation = self.query_generation.wrapping_add(1);
            let _ = self
                .commands
                .unbounded_send(portable::Command::Navigate(self.query_generation, false));
        }
        self.focus(window, cx);
    }

    fn navigate_previous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if self.search_active
            && let Err(err) = self.terminal.navigate_search_previous()
        {
            log::error!("Failed to select previous terminal search match: {err}");
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if self.search_active && !self.ended && !self.navigation_pending {
            self.navigation_pending = true;
            self.query_generation = self.query_generation.wrapping_add(1);
            let _ = self
                .commands
                .unbounded_send(portable::Command::Navigate(self.query_generation, true));
        }
        self.focus(window, cx);
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_search();
        window.focus(&self.terminal_focus, cx);
        #[cfg(target_os = "macos")]
        cx.emit(TerminalFindDismissed);
    }

    fn finish_search(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.query_generation = self.query_generation.wrapping_add(1);
        #[cfg(target_os = "macos")]
        if let Err(err) = self.terminal.end_search() {
            log::error!("Failed to end terminal search: {err}");
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let _ = self.commands.unbounded_send(portable::Command::End);
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

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod portable {
    use super::*;
    use con_ghostty::vt::SearchProgress;
    use futures::channel::mpsc::{Sender, UnboundedReceiver, UnboundedSender, channel, unbounded};
    use futures::{FutureExt, SinkExt, StreamExt};

    pub(super) enum Command {
        Query(u64, String),
        Navigate(u64, bool),
        End,
    }

    // One FIFO driver per find bar. Needle replacement, navigation and cleanup
    // can traverse many native results, so none run on GPUI's thread. End is
    // acknowledged before removing the bar, preventing an old driver's cleanup
    // from clearing a newly opened search on the same terminal.
    pub(super) fn spawn(
        terminal: Arc<GhosttyTerminal>,
        window: &mut Window,
        cx: &mut Context<TerminalFind>,
    ) -> UnboundedSender<Command> {
        let (commands, receiver) = unbounded();
        let (mut updates, mut results) = channel(1);
        let executor = cx.background_executor().clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = run(&terminal, receiver, &mut updates, &executor).await {
                    log::error!("Terminal search failed: {err}");
                }
                if let Err(err) = terminal.end_search() {
                    log::error!("Failed to end terminal search: {err}");
                }
                // Dropping updates acknowledges cleanup even on an error or pane drop.
            })
            .detach();
        cx.spawn_in(window, async move |this, cx| {
            while let Some((generation, progress)) = results.next().await {
                if !cx
                    .update(|_, cx| {
                        this.update(cx, |this, cx| {
                            if this.ended {
                                return;
                            }
                            if this.query_generation == generation {
                                this.navigation_pending = false;
                                this.set_total(progress.map(|p: SearchProgress| p.total), cx);
                                this.set_selected(progress.and_then(|p| p.selected), cx);
                            }
                            cx.emit(TerminalFindUpdated);
                        })
                    })
                    .is_ok_and(|result| result.is_ok())
                {
                    return;
                }
            }
            let _ = cx.update(|window, cx| {
                this.update(cx, |this, cx| {
                    if !this.ended && this.input.read(cx).focus_handle(cx).is_focused(window) {
                        window.focus(&this.terminal_focus, cx);
                    }
                    this.ended = true;
                    cx.emit(TerminalFindDismissed);
                })
            });
        })
        .detach();
        commands
    }

    async fn run(
        terminal: &GhosttyTerminal,
        mut commands: UnboundedReceiver<Command>,
        updates: &mut Sender<(u64, Option<SearchProgress>)>,
        executor: &BackgroundExecutor,
    ) -> Result<(), String> {
        let mut active = false;
        let mut generation = 0;
        let mut last = None;
        loop {
            let command = if active {
                let delay = if last.is_some_and(|(_, progress): (u64, Option<SearchProgress>)| {
                    progress.is_some_and(|p| p.running)
                }) {
                    Duration::from_millis(16)
                } else {
                    Duration::from_millis(100)
                };
                futures::select_biased! {
                    command = commands.next().fuse() => Some(command),
                    _ = executor.timer(delay).fuse() => None,
                }
            } else {
                Some(commands.next().await)
            };
            match command {
                Some(Some(Command::Query(revision, needle))) => {
                    generation = revision;
                    active = terminal.search(&needle)? && !needle.is_empty();
                }
                Some(Some(Command::Navigate(revision, previous))) => {
                    generation = revision;
                    if previous {
                        terminal.navigate_search_previous()?;
                    } else {
                        terminal.navigate_search_next()?;
                    }
                }
                Some(Some(Command::End)) | Some(None) => return Ok(()),
                None => {}
            }
            let update = (generation, terminal.search_step()?);
            if last != Some(update) {
                if updates.send(update).await.is_err() {
                    return Ok(());
                }
                last = Some(update);
            }
        }
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
                    .size(mono_icon_px(theme, 14.0)),
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
