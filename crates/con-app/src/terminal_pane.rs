//! TerminalPane — Ghostty-backed terminal pane wrapper.

use con_agent::context::{PaneObservationFrame, PaneObservationSupport, derive_screen_hints};
use con_ghostty::TerminalColors;
use con_terminal::TerminalTheme;
use gpui::*;

use crate::ghostty_view::GhosttyView;
use crate::workspace::ConWorkspace;

#[derive(Clone)]
pub struct TerminalPane {
    entity: Entity<GhosttyView>,
}

impl TerminalPane {
    pub fn new(entity: Entity<GhosttyView>) -> Self {
        Self { entity }
    }

    pub fn title(&self, cx: &App) -> Option<String> {
        self.entity.read(cx).title()
    }

    pub fn current_dir(&self, cx: &App) -> Option<String> {
        self.entity.read(cx).current_dir()
    }

    pub fn is_alive(&self, cx: &App) -> bool {
        self.entity.read(cx).is_alive()
    }

    pub fn surface_ready(&self, cx: &App) -> bool {
        self.entity.read(cx).surface_ready()
    }

    pub fn is_busy(&self, cx: &App) -> bool {
        self.entity
            .read(cx)
            .terminal()
            .map(|terminal| terminal.is_busy())
            .unwrap_or(false)
    }

    pub fn has_shell_integration(&self, cx: &App) -> bool {
        self.entity
            .read(cx)
            .terminal()
            .map(|terminal| {
                terminal.current_dir().is_some() || !terminal.command_history().is_empty()
            })
            .unwrap_or(false)
    }

    pub fn is_alt_screen(&self, _cx: &App) -> bool {
        false
    }

    pub fn write(&self, data: &[u8], cx: &mut App) {
        self.entity.update(cx, |view, _| view.write_or_queue(data));
    }

    pub fn ensure_surface(&self, window: &mut Window, cx: &mut App) {
        self.entity.update(cx, |view, cx| {
            view.ensure_initialized_for_control(window, cx)
        });
    }

    pub fn sync_surface_layout(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        self.entity.update(cx, |view, cx| {
            view.sync_surface_layout_for_host(bounds, window, cx)
        });
    }

    pub fn set_theme(
        &self,
        theme: &TerminalTheme,
        colors: &TerminalColors,
        font_family: &str,
        font_size: f32,
        background_opacity: f32,
        background_blur: bool,
        cursor_style: &str,
        background_image: Option<&str>,
        background_image_opacity: f32,
        background_image_position: Option<&str>,
        background_image_fit: Option<&str>,
        background_image_repeat: bool,
        cx: &mut App,
    ) {
        let is_dark = theme.name.to_lowercase().contains("dark");
        if let Some(terminal) = self.entity.read(cx).terminal() {
            if let Err(err) = terminal.update_appearance(
                colors,
                font_family,
                font_size,
                background_opacity,
                background_blur,
                cursor_style,
                background_image,
                background_image_opacity,
                background_image_position,
                background_image_fit,
                background_image_repeat,
            ) {
                log::error!("Failed to update Ghostty surface appearance: {}", err);
            }
            terminal.set_color_scheme(is_dark);
        }
    }

    pub fn clear_scrollback(&self, cx: &mut App) {
        if let Some(terminal) = self.entity.read(cx).terminal() {
            if let Err(err) = terminal.clear_screen_and_scrollback() {
                log::error!("Failed to clear Ghostty scrollback: {}", err);
            }
        }
    }

    pub fn notify(&self, cx: &mut App) {
        self.entity.update(cx, |_, cx| cx.notify());
    }

    pub fn set_native_view_visible(&self, visible: bool, cx: &App) {
        self.entity.read(cx).set_visible(visible);
    }

    #[cfg(target_os = "macos")]
    pub fn set_native_transition_underlay_visible(&self, visible: bool, cx: &App) {
        self.entity
            .read(cx)
            .set_transition_underlay_visible(visible);
    }

    pub fn shutdown_surface(&self, cx: &mut App) {
        self.entity.update(cx, |view, _| view.shutdown_surface());
    }

    pub fn set_focus_state(&self, focused: bool, cx: &mut App) {
        self.entity
            .update(cx, |view, _| view.set_surface_focus_state(focused));
    }

    pub fn refresh_surface(&self, cx: &App) {
        if let Some(terminal) = self.entity.read(cx).terminal() {
            terminal.refresh();
        }
    }

    pub fn drain_surface_state_with_native_scroll(
        &self,
        sync_native_scroll: bool,
        cx: &mut App,
    ) -> bool {
        self.entity.update(cx, |view, cx| {
            view.drain_surface_state(sync_native_scroll, cx)
        })
    }

    pub fn pump_surface_deferred_work(&self, cx: &mut App) -> bool {
        self.entity
            .update(cx, |view, cx| view.pump_deferred_work(cx))
    }

    pub fn sync_window_background_blur(&self, cx: &mut App) {
        self.entity
            .update(cx, |view, _| view.sync_window_background_blur());
    }

    #[cfg(target_os = "macos")]
    pub fn mark_native_layout_pending(&self, cx: &mut App) {
        self.entity
            .update(cx, |view, cx| view.mark_native_layout_pending(cx));
    }

    pub fn selection_text(&self, cx: &App) -> Option<String> {
        self.entity.read(cx).selection_text()
    }

    pub fn release_mouse_selection(&self, cx: &App) {
        if let Some(terminal) = self.entity.read(cx).terminal() {
            terminal.send_mouse_button(false, con_ghostty::MouseButton::Left, 0);
        }
    }

    pub fn entity_id(&self) -> EntityId {
        self.entity.entity_id()
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.entity.focus_handle(cx)
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle(cx).focus(window, cx);
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle(cx).is_focused(window)
    }

    pub fn render_child(&self) -> AnyElement {
        self.entity.clone().into_any_element()
    }

    pub fn content_lines(&self, n: usize, cx: &App) -> Vec<String> {
        self.entity
            .read(cx)
            .terminal()
            .map(|terminal| terminal.read_screen_text(n))
            .unwrap_or_default()
    }

    pub fn recent_lines(&self, n: usize, cx: &App) -> Vec<String> {
        self.entity
            .read(cx)
            .terminal()
            .map(|terminal| terminal.read_recent_lines(n))
            .unwrap_or_default()
    }

    pub fn last_command(&self, _cx: &App) -> Option<String> {
        None
    }

    pub fn last_exit_code(&self, cx: &App) -> Option<i32> {
        self.entity
            .read(cx)
            .terminal()
            .and_then(|terminal| terminal.last_exit_code())
    }

    pub fn take_command_finished(&self, cx: &App) -> Option<(Option<i32>, std::time::Duration)> {
        self.entity
            .read(cx)
            .terminal()
            .and_then(|terminal| terminal.take_command_finished())
            .map(|signal| (signal.exit_code, signal.duration))
    }

    pub fn last_command_duration(&self, cx: &App) -> Option<std::time::Duration> {
        self.entity
            .read(cx)
            .terminal()
            .and_then(|terminal| terminal.last_command_duration())
    }

    pub fn input_generation(&self, cx: &App) -> u64 {
        self.entity
            .read(cx)
            .terminal()
            .map(|terminal| terminal.input_generation())
            .unwrap_or(0)
    }

    pub fn recover_shell_prompt_state(&self, cx: &App) {
        if let Some(terminal) = self.entity.read(cx).terminal() {
            terminal.recover_shell_prompt_state();
        }
    }

    pub fn observation_frame(&self, recent_output_lines: usize, cx: &App) -> PaneObservationFrame {
        let visible_output = self.content_lines(recent_output_lines, cx);
        let recent_output = self.recent_lines(recent_output_lines, cx);
        let title = self.title(cx);
        PaneObservationFrame {
            title: title.clone(),
            cwd: self.current_dir(cx),
            screen_hints: derive_screen_hints(
                title.as_deref(),
                if visible_output.is_empty() {
                    &recent_output
                } else {
                    &visible_output
                },
            ),
            recent_output: if recent_output.is_empty() {
                visible_output
            } else {
                recent_output
            },
            last_command: self.last_command(cx),
            last_exit_code: self.last_exit_code(cx),
            last_command_duration_secs: self.last_command_duration(cx).map(|d| d.as_secs_f64()),
            support: PaneObservationSupport::default(),
            has_shell_integration: self.has_shell_integration(cx),
            is_alt_screen: self.is_alt_screen(cx),
            is_busy: self.is_busy(cx),
            input_generation: self.input_generation(cx),
            last_command_finished_input_generation: self
                .entity
                .read(cx)
                .terminal()
                .map(|terminal| terminal.last_command_finished_input_generation())
                .unwrap_or(0),
        }
    }

    pub fn grid_size(&self, cx: &App) -> (usize, usize) {
        let size = self
            .entity
            .read(cx)
            .terminal()
            .map(|terminal| terminal.size());
        match size {
            Some(size) => (size.columns as usize, size.rows as usize),
            None => (80, 24),
        }
    }

    pub fn search_text(&self, pattern: &str, limit: usize, cx: &App) -> Vec<(usize, String)> {
        self.entity
            .read(cx)
            .terminal()
            .map(|terminal| terminal.search_text(pattern, limit))
            .unwrap_or_default()
    }
}

pub fn subscribe_terminal_pane(
    pane: &TerminalPane,
    window: &mut Window,
    cx: &mut Context<ConWorkspace>,
) {
    cx.subscribe_in(
        &pane.entity,
        window,
        ConWorkspace::on_terminal_focus_changed,
    )
    .detach();
    cx.subscribe_in(
        &pane.entity,
        window,
        ConWorkspace::on_terminal_process_exited,
    )
    .detach();
    cx.subscribe_in(
        &pane.entity,
        window,
        ConWorkspace::on_terminal_title_changed,
    )
    .detach();
    cx.subscribe_in(&pane.entity, window, ConWorkspace::on_terminal_cwd_changed)
        .detach();
    cx.subscribe_in(
        &pane.entity,
        window,
        ConWorkspace::on_terminal_split_requested,
    )
    .detach();
}
