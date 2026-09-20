use gpui::{App, Bounds, Pixels, point, px, size};
use std::cell::RefCell;

#[derive(Debug, Default, Clone, Copy)]
struct QuickTerminalState {
    raw_ptr: Option<usize>,
    opening: bool,
    visible: bool,
    return_pid: Option<i32>,
    last_active_pid: Option<i32>,
}

fn begin_opening(state: &mut QuickTerminalState) -> bool {
    if state.raw_ptr.is_some() || state.opening {
        return false;
    }

    state.opening = true;
    true
}

fn finish_opening(state: &mut QuickTerminalState) {
    state.opening = false;
}

fn valid_pid(pid: i32) -> Option<i32> {
    (pid > 0).then_some(pid)
}

fn capture_return_pid() -> Option<i32> {
    let current_pid = std::process::id() as i32;
    let frontmost_pid = unsafe { con_quick_terminal_frontmost_app_pid() };
    let captured = valid_pid(frontmost_pid).filter(|pid| *pid != current_pid);
    if captured.is_some() {
        QUICK_TERMINAL_STATE.with(|state| state.borrow_mut().last_active_pid = captured);
    }
    captured.or_else(|| QUICK_TERMINAL_STATE.with(|state| state.borrow().last_active_pid))
}

/// Record an externally observed active app pid. This lets the quick terminal
/// restore focus even when the global-hotkey callback runs after macOS has
/// already made con frontmost. The recorded app may be con itself: the return
/// target is "whatever app was active before Quick Terminal", not "non-con".
pub fn remember_active_app_pid(pid: i32) {
    let current_pid = std::process::id() as i32;
    if let Some(pid) = valid_pid(pid) {
        QUICK_TERMINAL_STATE.with(|state| {
            let mut state = state.borrow_mut();
            // Opening/showing Quick Terminal activates con as the owner app.
            // Do not let that synthetic activation overwrite the app that was
            // active before Quick Terminal. A con pid observed while the quick
            // terminal is not active is still a valid return target.
            if pid == current_pid && (state.opening || state.visible) {
                return;
            }
            state.last_active_pid = Some(pid);
        });
    }
}

/// Capture the frontmost app pid synchronously from the Carbon hotkey
/// callback, before macOS activates con as the hotkey owner. Call this
/// from the hotkey handler thread and pass the result to `toggle_with_pid`.
pub fn capture_frontmost_pid_before_activation() -> Option<i32> {
    capture_return_pid()
}

fn prepare_force_hide(state: &mut QuickTerminalState) -> Option<i32> {
    let was_visible = state.visible;
    state.visible = false;
    let return_pid = state.return_pid.take();
    was_visible.then_some(return_pid).flatten()
}

unsafe extern "C" {
    fn con_quick_terminal_configure(window_ptr: *mut std::ffi::c_void);
    fn con_quick_terminal_init();
    fn con_quick_terminal_prepare_destroy(window_ptr: *mut std::ffi::c_void);
    fn con_quick_terminal_slide_in(window_ptr: *mut std::ffi::c_void);
    fn con_quick_terminal_slide_out(window_ptr: *mut std::ffi::c_void, return_pid: i32);
    fn con_quick_terminal_window_from_view(
        view_ptr: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn con_quick_terminal_active_display_visible_frame(
        x: *mut f64,
        y: *mut f64,
        width: *mut f64,
        height: *mut f64,
    ) -> bool;
    fn con_quick_terminal_frontmost_app_pid() -> i32;
    fn con_quick_terminal_is_main_thread() -> bool;
}

thread_local! {
    static QUICK_TERMINAL_STATE: RefCell<QuickTerminalState> =
        const { RefCell::new(QuickTerminalState { raw_ptr: None, opening: false, visible: false, return_pid: None, last_active_pid: None }) };
}

/// Install the native observer; Quick Terminal window creation stays lazy.
pub fn init(_cx: &App) {
    unsafe { con_quick_terminal_init() };
}

pub fn store_window_ptr(window_ptr: *mut std::ffi::c_void) {
    let fallback_return_pid = capture_return_pid();
    QUICK_TERMINAL_STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.raw_ptr = Some(window_ptr as usize);
        finish_opening(&mut state);
        if state.return_pid.is_none() {
            state.return_pid = fallback_return_pid;
        }
        state.visible = true;
    });
    unsafe { con_quick_terminal_configure(window_ptr) };
    unsafe { con_quick_terminal_slide_in(window_ptr) };
}

pub fn window_from_view_ptr(view_ptr: *mut std::ffi::c_void) -> Option<*mut std::ffi::c_void> {
    let window_ptr = unsafe { con_quick_terminal_window_from_view(view_ptr) };
    (!window_ptr.is_null()).then_some(window_ptr)
}

/// Toggle called from the hotkey handler with a pid captured *before*
/// macOS activates con as the hotkey owner. This is the preferred entry
/// point from `global_hotkey.rs`.
pub fn toggle_with_pid(pre_captured_pid: Option<i32>, cx: &mut App) {
    toggle_inner(pre_captured_pid, cx);
}

/// Toggle called without a pre-captured pid (e.g. from a menu action).
/// Captures the frontmost pid at call time — may already be con itself
/// if called after activation, but is better than nothing.
pub fn toggle(cx: &mut App) {
    let pid = capture_return_pid();
    toggle_inner(pid, cx);
}

fn toggle_inner(pre_captured_pid: Option<i32>, cx: &mut App) {
    let window_ptr = QUICK_TERMINAL_STATE.with(|state| state.borrow().raw_ptr);

    if let Some(window_ptr) = window_ptr {
        QUICK_TERMINAL_STATE.with(|state| {
            let mut state = state.borrow_mut();
            let raw = window_ptr as *mut std::ffi::c_void;
            if state.visible {
                let return_pid = state.return_pid.take();
                unsafe {
                    con_quick_terminal_slide_out(raw, return_pid.unwrap_or(0));
                }
                state.visible = false;
            } else {
                // Use the pre-captured pid (taken before con was activated).
                state.return_pid = pre_captured_pid;
                unsafe {
                    con_quick_terminal_slide_in(raw);
                }
                state.visible = true;
            }
        });
        return;
    }

    let should_open = QUICK_TERMINAL_STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !begin_opening(&mut state) {
            return false;
        }
        state.return_pid = pre_captured_pid;
        true
    });
    if !should_open {
        return;
    }

    let Some(config) = crate::load_config_for_new_window() else {
        opening_failed();
        return;
    };
    crate::open_quick_terminal(
        config,
        crate::fresh_window_session_with_history_for_cwd(default_quick_terminal_cwd()),
        cx,
    );
}

pub fn opening_failed() {
    QUICK_TERMINAL_STATE.with(|state| {
        *state.borrow_mut() = QuickTerminalState::default();
    });
}

/// Returns the default working directory for the quick terminal.
/// On macOS this is the user's home directory.
pub fn default_quick_terminal_cwd() -> Option<std::path::PathBuf> {
    dirs::home_dir()
}

#[cfg(target_os = "macos")]
pub fn active_display_visible_bounds() -> Option<Bounds<Pixels>> {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut width = 0.0;
    let mut height = 0.0;
    let ok = unsafe {
        con_quick_terminal_active_display_visible_frame(&mut x, &mut y, &mut width, &mut height)
    };
    if !ok || width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(Bounds::new(
        point(px(x as f32), px(y as f32)),
        size(px(width as f32), px(height as f32)),
    ))
}

/// Always slide the window off-screen, regardless of the current visible
/// flag. Consumes the saved return pid (captured during toggle-in) so
/// focus returns to the previously active app, not the con main window.
pub fn force_hide() {
    QUICK_TERMINAL_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let return_pid = prepare_force_hide(&mut state);
        if let Some(window_ptr) = state.raw_ptr {
            unsafe {
                con_quick_terminal_slide_out(
                    window_ptr as *mut std::ffi::c_void,
                    return_pid.unwrap_or(0),
                );
            }
        }
    });
}

/// Slide the quick terminal window off-screen, but only if currently
/// visible. No-ops when already hidden to avoid double-hide during
/// resign-key animations. Does NOT consume the saved return pid —
/// macOS naturally activates whatever the user clicked on.
pub fn hide() {
    QUICK_TERMINAL_STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !state.visible {
            return;
        }
        state.visible = false;
        if let Some(window_ptr) = state.raw_ptr {
            unsafe {
                con_quick_terminal_slide_out(window_ptr as *mut std::ffi::c_void, 0);
            }
        }
    });
}

/// Called from ObjC when NSWorkspace reports an active app change.
#[unsafe(no_mangle)]
extern "C" fn con_quick_terminal_remember_active_app(pid: i32) {
    remember_active_app_pid(pid);
}

/// Called from ObjC when the quick terminal window resigns key (user clicked
/// elsewhere). Hides the window automatically like iTerm2's hotkey window.
#[unsafe(no_mangle)]
extern "C" fn con_quick_terminal_handle_resign_key() {
    debug_assert!(unsafe { con_quick_terminal_is_main_thread() });
    hide();
}

pub fn reset_destroyed_window() {
    debug_assert!(unsafe { con_quick_terminal_is_main_thread() });
    let window_ptr = QUICK_TERMINAL_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let window_ptr = state.raw_ptr;
        *state = QuickTerminalState::default();
        window_ptr
    });
    if let Some(window_ptr) = window_ptr {
        unsafe {
            con_quick_terminal_prepare_destroy(window_ptr as *mut std::ffi::c_void);
        }
    }
}

#[cfg(test)]
fn reset_state_for_tests() -> QuickTerminalState {
    QUICK_TERMINAL_STATE.with(|state| {
        *state.borrow_mut() = QuickTerminalState::default();
        *state.borrow()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        QuickTerminalState, begin_opening, default_quick_terminal_cwd, finish_opening,
        prepare_force_hide, reset_state_for_tests, valid_pid,
    };

    #[test]
    fn quick_terminal_state_defaults_are_hidden_and_empty() {
        let state = QuickTerminalState::default();
        assert_eq!(state.raw_ptr, None);
        assert!(!state.opening);
        assert!(!state.visible);
        assert_eq!(state.return_pid, None);
        assert_eq!(state.last_active_pid, None);
    }

    #[test]
    fn quick_terminal_binding_default_is_non_empty() {
        assert!(
            !con_core::config::KeybindingConfig::default()
                .quick_terminal
                .trim()
                .is_empty()
        );
    }

    #[test]
    fn valid_pid_accepts_any_positive_process_id() {
        assert_eq!(valid_pid(7), Some(7));
        assert_eq!(valid_pid(42), Some(42));
        assert_eq!(valid_pid(0), None);
        assert_eq!(valid_pid(-1), None);

        let mut slot = Some(7);
        assert_eq!(slot.take(), Some(7));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn reset_clears_destroyed_window_state() {
        let state = reset_state_for_tests();
        assert_eq!(state.raw_ptr, None);
        assert!(!state.opening);
        assert!(!state.visible);
        assert_eq!(state.return_pid, None);
    }

    #[test]
    fn force_hide_preparation_hides_and_consumes_saved_pid() {
        let mut state = QuickTerminalState {
            raw_ptr: Some(1),
            opening: false,
            visible: true,
            return_pid: Some(99),
            last_active_pid: None,
        };
        assert_eq!(prepare_force_hide(&mut state), Some(99));
        assert!(!state.visible);
        assert_eq!(state.return_pid, None);
    }

    #[test]
    fn force_hide_preparation_does_not_restore_when_already_hidden() {
        let mut state = QuickTerminalState {
            raw_ptr: Some(1),
            opening: false,
            visible: false,
            return_pid: Some(99),
            last_active_pid: None,
        };
        assert_eq!(prepare_force_hide(&mut state), None);
        assert!(!state.visible);
        assert_eq!(state.return_pid, None);
    }

    #[test]
    fn begin_opening_is_single_flight_until_window_is_stored_or_fails() {
        let mut state = QuickTerminalState::default();
        assert!(begin_opening(&mut state));
        assert!(state.opening);
        assert!(!begin_opening(&mut state));

        finish_opening(&mut state);
        assert!(!state.opening);
        assert!(begin_opening(&mut state));

        state.raw_ptr = Some(1);
        finish_opening(&mut state);
        assert!(!begin_opening(&mut state));
    }

    #[test]
    fn default_quick_terminal_cwd_uses_home_dir() {
        let cwd = default_quick_terminal_cwd().expect("macOS should expose a home directory");
        assert!(
            cwd.is_dir(),
            "quick terminal cwd must be an existing directory"
        );
    }
}
