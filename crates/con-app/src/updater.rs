//! Cross-platform auto-update surface.
//!
//! macOS uses Sparkle (loaded dynamically from the embedded
//! `Sparkle.framework` in the app bundle). All ObjC calls go through
//! a C trampoline (`sparkle_trampoline.m`) that wraps them in
//! `@try/@catch` — Rust's `catch_unwind` cannot catch ObjC exceptions.
//!
//! Windows/Linux use the signed portable updater, respecting installation ownership.

use std::sync::{Mutex, OnceLock};
pub(crate) mod portable;

#[cfg(target_os = "macos")]
use cocoa::base::{BOOL, YES, id, nil};
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};

// FFI to the ObjC trampoline compiled by build.rs (macOS only).
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn con_sparkle_init_controller() -> *mut std::ffi::c_void;
    fn con_sparkle_check_for_updates(controller: *mut std::ffi::c_void);
}

/// Opaque handle to the Sparkle updater controller (macOS).
///
/// Stored globally so the ObjC runtime retains it for the process lifetime.
#[cfg(target_os = "macos")]
static CONTROLLER: OnceLock<usize> = OnceLock::new();
static STATUS: OnceLock<UpdaterStatus> = OnceLock::new();

/// Shared cross-platform "what we know about updates right now".
static LATEST: OnceLock<Mutex<CheckState>> = OnceLock::new();

fn latest_slot() -> &'static Mutex<CheckState> {
    LATEST.get_or_init(|| Mutex::new(CheckState::Idle))
}

/// Outcome of the last (or in-flight) update check.
///
/// Windows/Linux use the portable transaction state machine.
/// macOS delegates to Sparkle, which has
/// its own opaque state machine, so on that target the state stays
/// at `Idle` and the UI falls back to `UpdaterStatus::summary/detail`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub enum CheckState {
    /// Check has not been run yet, or the channel doesn't poll.
    Idle,
    /// A background fetch is currently in flight.
    Checking,
    /// Current binary is at or ahead of the latest published version.
    UpToDate,
    Skipped {
        version: String,
    },
    /// A newer version is published and may be prepared or downloaded manually.
    UpdateAvailable {
        version: String,
        url: String,
    },
    /// The last check failed; message is for the UI to display.
    Error(String),
    Downloading {
        version: String,
        received: u64,
        total: u64,
    },
    Ready {
        version: String,
    },
    Installing,
}

pub fn latest_check() -> CheckState {
    latest_slot()
        .lock()
        .map(|g| g.clone())
        .unwrap_or(CheckState::Idle)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdaterStatus {
    Active,
    Disabled(UpdaterDisabledReason),
}

/// Most variants are only constructed in the macOS Sparkle init
/// path; Windows uses `ChannelDoesNotPoll` and `InitPanicked` only.
/// Keep the surface platform-agnostic so `summary()`/`detail()` can
/// match exhaustively without cfg noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum UpdaterDisabledReason {
    ChannelDoesNotPoll,
    NotBundled,
    MissingFrameworksPath,
    MissingSparkleFramework,
    FailedToLoadSparkleFramework,
    MissingFeedUrl,
    ControllerInitFailed,
    InitPanicked,
}

impl UpdaterStatus {
    pub fn can_check_manually(self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::Active => "Auto-update enabled",
            Self::Disabled(_) => "Auto-update unavailable",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Active => {
                if cfg!(target_os = "macos") {
                    "Sparkle is loaded and polling this release channel."
                } else {
                    "Signed updates download while you work and install when you choose to restart."
                }
            }
            Self::Disabled(UpdaterDisabledReason::ChannelDoesNotPoll) => {
                "Development builds do not poll for updates."
            }
            Self::Disabled(UpdaterDisabledReason::NotBundled) => {
                "Updates only work from the bundled app, not cargo run."
            }
            Self::Disabled(UpdaterDisabledReason::MissingFrameworksPath) => {
                "The app bundle has no Frameworks directory."
            }
            Self::Disabled(UpdaterDisabledReason::MissingSparkleFramework) => {
                "Sparkle.framework is not embedded in the app bundle."
            }
            Self::Disabled(UpdaterDisabledReason::FailedToLoadSparkleFramework) => {
                "Sparkle.framework exists but failed to load."
            }
            Self::Disabled(UpdaterDisabledReason::MissingFeedUrl) => {
                "SUFeedURL is missing from the app bundle metadata."
            }
            Self::Disabled(UpdaterDisabledReason::ControllerInitFailed) => {
                "Sparkle failed to initialize its updater controller."
            }
            Self::Disabled(UpdaterDisabledReason::InitPanicked) => {
                "Updater initialization panicked and was disabled."
            }
        }
    }
}

/// Initialize the updater. Call once during app launch, after the
/// main window is open. Returns `true` if the update surface is live.
pub fn init() -> bool {
    if portable::seed_review() {
        let _ = STATUS.set(UpdaterStatus::Active);
        return true;
    }
    match std::panic::catch_unwind(init_inner) {
        Ok(result) => result,
        Err(_) => {
            log::error!("updater: init panicked — auto-update disabled");
            let _ = STATUS.set(UpdaterStatus::Disabled(UpdaterDisabledReason::InitPanicked));
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn init_inner() -> bool {
    if CONTROLLER.get().is_some() {
        let _ = STATUS.set(UpdaterStatus::Active);
        return true;
    }

    let channel = con_core::release_channel::current();
    if !channel.polls_for_updates() {
        log::info!(
            "updater: channel={} — skipping Sparkle init",
            channel.name()
        );
        let _ = STATUS.set(UpdaterStatus::Disabled(
            UpdaterDisabledReason::ChannelDoesNotPoll,
        ));
        return false;
    }

    unsafe {
        // Verify we're running inside an app bundle with Sparkle
        let main_bundle: id = msg_send![class!(NSBundle), mainBundle];
        if main_bundle == nil {
            log::warn!("updater: no main bundle — likely running outside .app");
            let _ = STATUS.set(UpdaterStatus::Disabled(UpdaterDisabledReason::NotBundled));
            return false;
        }

        let frameworks_path: id = msg_send![main_bundle, privateFrameworksPath];
        if frameworks_path == nil {
            log::warn!("updater: no Frameworks path");
            let _ = STATUS.set(UpdaterStatus::Disabled(
                UpdaterDisabledReason::MissingFrameworksPath,
            ));
            return false;
        }
        let sparkle_subpath: id = msg_send![
            class!(NSString),
            stringWithUTF8String: c"Sparkle.framework".as_ptr()
        ];
        let sparkle_path: id =
            msg_send![frameworks_path, stringByAppendingPathComponent: sparkle_subpath];

        let sparkle_bundle: id = msg_send![class!(NSBundle), bundleWithPath: sparkle_path];
        if sparkle_bundle == nil {
            log::info!("updater: Sparkle.framework not found — auto-update disabled");
            let _ = STATUS.set(UpdaterStatus::Disabled(
                UpdaterDisabledReason::MissingSparkleFramework,
            ));
            return false;
        }
        let mut load_error: id = nil;
        let loaded: BOOL = msg_send![sparkle_bundle, loadAndReturnError: &mut load_error];
        if loaded != YES {
            if load_error != nil {
                let localized_description: id = msg_send![load_error, localizedDescription];
                let localized_reason: id = msg_send![load_error, localizedFailureReason];

                let desc_cstr: *const std::os::raw::c_char =
                    msg_send![localized_description, UTF8String];
                let reason_cstr: *const std::os::raw::c_char =
                    msg_send![localized_reason, UTF8String];

                let description = if desc_cstr.is_null() {
                    "<unknown>"
                } else {
                    std::ffi::CStr::from_ptr(desc_cstr)
                        .to_str()
                        .unwrap_or("<invalid utf8>")
                };
                let reason = if reason_cstr.is_null() {
                    ""
                } else {
                    std::ffi::CStr::from_ptr(reason_cstr).to_str().unwrap_or("")
                };
                log::warn!(
                    "updater: failed to load Sparkle.framework: {} {}",
                    description,
                    reason
                );
            } else {
                log::warn!("updater: failed to load Sparkle.framework");
            }
            let _ = STATUS.set(UpdaterStatus::Disabled(
                UpdaterDisabledReason::FailedToLoadSparkleFramework,
            ));
            return false;
        }

        // Verify SUFeedURL is set (otherwise Sparkle will throw)
        let info_dict: id = msg_send![main_bundle, infoDictionary];
        let feed_key: id = msg_send![
            class!(NSString),
            stringWithUTF8String: c"SUFeedURL".as_ptr()
        ];
        let feed_url: id = msg_send![info_dict, objectForKey: feed_key];
        if feed_url == nil {
            log::info!("updater: SUFeedURL not set in Info.plist — auto-update disabled");
            let _ = STATUS.set(UpdaterStatus::Disabled(
                UpdaterDisabledReason::MissingFeedUrl,
            ));
            return false;
        }

        let controller = con_sparkle_init_controller();
        if controller.is_null() {
            log::warn!(
                "updater: SPUStandardUpdaterController init failed or threw — auto-update disabled"
            );
            let _ = STATUS.set(UpdaterStatus::Disabled(
                UpdaterDisabledReason::ControllerInitFailed,
            ));
            return false;
        }

        let _ = CONTROLLER.set(controller as usize);
        let _ = STATUS.set(UpdaterStatus::Active);

        log::info!(
            "updater: Sparkle initialized — channel={}, polling=true",
            channel.name()
        );
        true
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn init_inner() -> bool {
    let channel = con_core::release_channel::current();
    if !channel.polls_for_updates() {
        log::info!(
            "updater: channel={} — notify-only updater idle",
            channel.name()
        );
        let _ = STATUS.set(UpdaterStatus::Disabled(
            UpdaterDisabledReason::ChannelDoesNotPoll,
        ));
        return false;
    }

    portable::init();
    let _ = STATUS.set(UpdaterStatus::Active);
    if !matches!(latest_check(), CheckState::Error(_)) {
        portable::check(false);
    }
    log::info!(
        "updater: notify-only check started — channel={}",
        channel.name()
    );
    true
}

#[cfg(all(
    not(target_os = "macos"),
    not(target_os = "windows"),
    not(target_os = "linux")
))]
fn init_inner() -> bool {
    let _ = STATUS.set(UpdaterStatus::Disabled(
        UpdaterDisabledReason::ChannelDoesNotPoll,
    ));
    false
}

/// Trigger a manual update check (e.g. from Settings → "Check for Updates").
#[cfg(target_os = "macos")]
pub fn check_for_updates() {
    let controller = match CONTROLLER.get() {
        Some(&ptr) => ptr as *mut std::ffi::c_void,
        None => {
            log::info!("updater: not initialized — cannot check for updates");
            return;
        }
    };

    unsafe {
        con_sparkle_check_for_updates(controller);
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub fn check_for_updates() {
    let channel = con_core::release_channel::current();
    if !channel.polls_for_updates() {
        log::info!(
            "updater: channel={} — skipping manual check",
            channel.name()
        );
        return;
    }
    portable::check(true);
}

#[cfg(all(
    not(target_os = "macos"),
    not(target_os = "windows"),
    not(target_os = "linux")
))]
pub fn check_for_updates() {}

pub fn status() -> UpdaterStatus {
    *STATUS.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            if CONTROLLER.get().is_some() {
                UpdaterStatus::Active
            } else {
                UpdaterStatus::Disabled(UpdaterDisabledReason::ChannelDoesNotPoll)
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Windows + Linux notify-only path. `init_inner` flips
            // STATUS to Active when the channel polls; on Dev / no
            // network this stays Disabled.
            UpdaterStatus::Disabled(UpdaterDisabledReason::ChannelDoesNotPoll)
        }
    })
}
