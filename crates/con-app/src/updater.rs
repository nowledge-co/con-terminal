//! Cross-platform auto-update surface.
//!
//! macOS uses Sparkle (loaded dynamically from the embedded
//! `Sparkle.framework` in the app bundle). All ObjC calls go through
//! a C trampoline (`sparkle_trampoline.m`) that wraps them in
//! `@try/@catch` — Rust's `catch_unwind` cannot catch ObjC exceptions.
//!
//! Windows uses a lightweight notify-only checker: on startup we
//! fetch the same Sparkle-shaped appcast XML and compare the latest
//! published version to the running binary. If newer we surface a
//! "download" link in Settings → Updates — no in-app download, no
//! exe replacement. Users grab the new ZIP and unpack it themselves.
//! Full auto-update is a follow-up.

// On Linux nothing consumes most of this module — the Updates card in
// settings_panel is cfg-gated to macOS/Windows. Keeping the surface
// compiled (rather than cfg-ing out every item) means main.rs can call
// `init()` unconditionally without #[cfg] noise.
#![cfg_attr(
    all(not(target_os = "macos"), not(target_os = "windows")),
    allow(dead_code)
)]

use std::sync::{Mutex, OnceLock};

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
/// Non-`Idle` variants are only constructed by the Windows
/// notify-only checker today. macOS delegates to Sparkle, which has
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
    /// A newer version is published; user can follow the URL to grab it.
    UpdateAvailable { version: String, url: String },
    /// The last check failed; message is for the UI to display.
    Error(String),
}

pub fn latest_check() -> CheckState {
    latest_slot()
        .lock()
        .map(|g| g.clone())
        .unwrap_or(CheckState::Idle)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn set_latest(state: CheckState) {
    if let Ok(mut g) = latest_slot().lock() {
        *g = state;
    }
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
                    // Windows + Linux: notify-only checker that polls
                    // the same Sparkle-shaped appcast XML and re-runs
                    // install.ps1 / install.sh on apply.
                    "Periodic checks against the release feed; the in-app installer applies the update."
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

    let _ = STATUS.set(UpdaterStatus::Active);
    notify_impl::spawn_check(channel);
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
    notify_impl::spawn_check(channel);
}

#[cfg(all(
    not(target_os = "macos"),
    not(target_os = "windows"),
    not(target_os = "linux")
))]
pub fn check_for_updates() {}

/// Re-run `install.ps1` in a new console and exit this process so the
/// installer can replace `con-app.exe`. Windows only.
///
/// The script already does the full lifecycle — stop running instance,
/// download, verify SHA256, unpack, update PATH, relaunch — so we spawn
/// it and get out of the way. `CREATE_NEW_CONSOLE` gives the user
/// visible progress; without it a GUI-subsystem binary has no console
/// to inherit and the script would run silently.
#[cfg(target_os = "windows")]
pub fn apply_update_in_place() {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    const INSTALL_URL: &str = "https://con-releases.nowledge.co/install.ps1";

    let command = format!("irm {INSTALL_URL} | iex");
    match std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &command,
        ])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
    {
        Ok(_) => {
            // Give the installer a beat to grab the ZIP before we drop
            // our exe locks. `install.ps1` also does `Stop-Process -Name
            // con-app` as a belt-and-braces step, but exiting cleanly
            // lets pending writes (config, sessions) flush normally.
            std::thread::sleep(std::time::Duration::from_millis(400));
            std::process::exit(0);
        }
        Err(e) => {
            log::error!("updater: failed to spawn install.ps1: {e}");
            set_latest(CheckState::Error(format!(
                "could not launch installer: {e}"
            )));
        }
    }
}

/// Re-run `install.sh` in a detached background process and exit
/// cleanly so the script can replace `~/.local/bin/con` and any
/// other staged files. Linux only.
///
/// `install.sh` does the full lifecycle: download, sha-check,
/// extract, drop the .desktop entry, and atomically move the new
/// binary into place. Move-then-overwrite means the running con
/// inode survives — the kernel keeps the old text in memory until
/// this process exits — so re-running con after the installer
/// finishes picks up the new build. We `setsid -f` to detach so
/// killing con does not kill the installer.
///
/// Unlike Windows, we do **not** open a new terminal here. Linux
/// has no portable "spawn a visible console" API equivalent to
/// `CREATE_NEW_CONSOLE`; the user already has a Settings → Updates
/// card showing the same install command they can paste into any
/// shell if they want to watch progress.
#[cfg(target_os = "linux")]
pub fn apply_update_in_place() {
    use std::process::{Command, Stdio};

    const DEFAULT_INSTALL_URL: &str = "https://con-releases.nowledge.co/install.sh";

    // `CON_INSTALL_URL` overrides the script source for offline /
    // local-server verification. Same env-only opt-in as
    // `CON_APPCAST_BASE` — release builds default to the public
    // gh-pages-served install.sh; tests can point at their own.
    let install_url = std::env::var("CON_INSTALL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_INSTALL_URL.to_string());

    // Pull the version the appcast advertised — this is what the
    // user just clicked "Update now" on. install.sh's default path
    // queries GitHub's `/releases/latest`, which silently skips
    // prereleases — so a beta-channel user clicking through to the
    // installer would otherwise risk getting a stable downgrade
    // instead of the beta the appcast actually pointed at. Pin the
    // installer to the exact version the channel resolved to.
    let target_version = match latest_slot().lock() {
        Ok(g) => match &*g {
            CheckState::UpdateAvailable { version, .. } => Some(version.clone()),
            _ => None,
        },
        Err(_) => None,
    };

    // Download the script to a tempfile FIRST, then run it. The
    // obvious-looking `curl ... | sh` pipeline has two problems we
    // need to dodge:
    //
    //   1. POSIX `sh` returns the rightmost command's status, so a
    //      `curl` failure (404, network error, DNS) silently
    //      becomes a successful pipeline exit (`sh` reads empty
    //      input and returns 0). The user clicks "Update now",
    //      `con` exits as if the install kicked off, and they're
    //      stranded on the old version with no error surfaced.
    //   2. `pipefail` would solve (1) but isn't in POSIX —
    //      depending on it would break on minimal busybox-style
    //      shells.
    //
    // `mktemp && curl -o tmp && sh tmp` short-circuits cleanly via
    // `&&`: any failure aborts before the next command runs.
    //
    // `export CON_INSTALL_VERSION=...` (rather than the
    // `VAR=value <cmd>` env-prefix form) lifts the var onto the
    // outer shell so both `curl` and `sh tmp` inherit it.
    //
    // `setsid -f` puts the spawned shell in its own session so
    // it survives `con`'s exit; the trailing `>/dev/null 2>&1
    // </dev/null` on the spawn detaches stdio so the script
    // doesn't inherit our terminal.
    //
    // Both `install_url` and `version` go through `shell_quote()`
    // so user-supplied env (`CON_INSTALL_URL`) and appcast-supplied
    // version strings can't break out of the single-quoted argument
    // even if they contain `&`, `;`, `$`, etc.
    let url_arg = shell_quote(install_url.trim());
    let pipeline = match target_version.as_deref() {
        Some(version) => format!(
            "export CON_INSTALL_VERSION={version}; \
             tmp=$(mktemp) && curl -fsSL {url} -o \"$tmp\" && sh \"$tmp\"; \
             rc=$?; rm -f \"$tmp\"; exit $rc",
            // Strip any leading 'v' the appcast might carry;
            // install.sh re-adds the prefix when it builds the
            // `/releases/tags/v<version>` URL so a value of either
            // shape works.
            version = shell_quote(version.trim_start_matches('v')),
            url = url_arg,
        ),
        None => format!(
            "tmp=$(mktemp) && curl -fsSL {url} -o \"$tmp\" && sh \"$tmp\"; \
             rc=$?; rm -f \"$tmp\"; exit $rc",
            url = url_arg,
        ),
    };

    let setsid = which_first(["setsid", "/usr/bin/setsid"]);

    let spawn = if let Some(setsid) = setsid {
        Command::new(setsid)
            .arg("-f")
            .arg("sh")
            .arg("-c")
            .arg(&pipeline)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    } else {
        // setsid is part of util-linux and present on every modern
        // distro, but be defensive: fall back to a plain `sh -c`
        // and let the OS clean it up.
        Command::new("sh")
            .arg("-c")
            .arg(&pipeline)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };

    match spawn {
        Ok(_) => {
            // Same 400ms grace as Windows: let the installer get its
            // first network call out before our pending writes
            // (config, sessions) flush and we drop the exe inode.
            std::thread::sleep(std::time::Duration::from_millis(400));
            std::process::exit(0);
        }
        Err(e) => {
            log::error!("updater: failed to spawn install.sh: {e}");
            set_latest(CheckState::Error(format!(
                "could not launch installer: {e}"
            )));
        }
    }
}

/// Single-quote a value safely for inclusion in a `sh -c` script.
/// Versions are tag-derived (SemVer ASCII), but the quoting is cheap
/// and prevents accidental injection if the appcast ever serves a
/// version string with shell metacharacters.
#[cfg(target_os = "linux")]
fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(target_os = "linux")]
fn which_first<I, S>(candidates: I) -> Option<std::path::PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    use std::path::PathBuf;
    for cand in candidates {
        let p = PathBuf::from(cand.as_ref());
        if p.is_absolute() {
            if p.is_file() {
                return Some(p);
            }
            continue;
        }
        // Plain name — search PATH.
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in path_var.split(':') {
                let probe = std::path::Path::new(dir).join(&p);
                if probe.is_file() {
                    return Some(probe);
                }
            }
        }
    }
    None
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
#[allow(dead_code)]
pub fn apply_update_in_place() {}

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

/// Cross-platform notify-only updater shared by the Windows and
/// Linux backends. Polls the Sparkle-shaped appcast XML at
/// `https://con-releases.nowledge.co/appcast/{channel}-{platform}-{arch}.xml`,
/// compares the published version against the running binary, and
/// flips the shared `LATEST` slot. macOS uses Sparkle directly and
/// doesn't need this path.
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod notify_impl {
    use super::{CheckState, set_latest};
    use con_core::release_channel::{self, ReleaseChannel};

    /// Spawn a one-shot check on a native thread. Uses a fresh tokio
    /// runtime for the HTTP fetch rather than relying on a shared
    /// app-level runtime — the check runs at most a few times per
    /// session so the overhead of building a runtime is fine, and it
    /// keeps this module fully self-contained.
    pub(super) fn spawn_check(channel: ReleaseChannel) {
        set_latest(CheckState::Checking);
        let url = channel.feed_url(
            release_channel::host_platform(),
            release_channel::host_arch(),
        );
        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|rt| rt.block_on(async { fetch_and_compare(&url).await }));
            match result {
                Ok(state) => set_latest(state),
                Err(e) => {
                    log::warn!("updater: notify-only check failed: {e}");
                    set_latest(CheckState::Error(e));
                }
            }
        });
    }

    async fn fetch_and_compare(feed_url: &str) -> Result<CheckState, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let body = client
            .get(feed_url)
            .send()
            .await
            .map_err(|e| format!("fetch: {e}"))?
            .error_for_status()
            .map_err(|e| format!("status: {e}"))?
            .text()
            .await
            .map_err(|e| format!("read body: {e}"))?;

        let (version, url) = parse_latest(&body)
            .ok_or_else(|| "appcast missing shortVersionString or enclosure".to_string())?;

        let running = crate::app_display_version();
        let newer = is_newer(&version, &running);
        log::info!(
            "updater: appcast advertises version={version} (running {running}); is_newer={newer}",
            version = version,
            running = running,
            newer = newer,
        );
        if newer {
            Ok(CheckState::UpdateAvailable { version, url })
        } else {
            Ok(CheckState::UpToDate)
        }
    }

    /// Extract the first `<sparkle:shortVersionString>` and
    /// `<enclosure url="...">` from the feed. Sparkle appcasts list
    /// the newest item first, so we only need to parse the head of
    /// the document.
    fn parse_latest(xml: &str) -> Option<(String, String)> {
        let version = between(
            xml,
            "<sparkle:shortVersionString>",
            "</sparkle:shortVersionString>",
        )?;
        let enclosure_start = xml.find("<enclosure")?;
        let enclosure_end = xml[enclosure_start..]
            .find('>')
            .map(|i| enclosure_start + i)?;
        let enclosure_tag = &xml[enclosure_start..=enclosure_end];
        let url = attr(enclosure_tag, "url")?;
        Some((version.trim().to_string(), url.to_string()))
    }

    fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
        let i = s.find(open)? + open.len();
        let j = s[i..].find(close)? + i;
        Some(&s[i..j])
    }

    fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let key = format!("{name}=\"");
        let i = tag.find(&key)? + key.len();
        let j = tag[i..].find('"')? + i;
        Some(&tag[i..j])
    }

    /// Compare two dotted versions numerically, ignoring any
    /// pre-release suffix (`-beta.1` etc). Returns true iff
    /// `latest > running`.
    /// SemVer-lite comparison with prerelease awareness.
    ///
    /// Splits `MAJOR.MINOR.PATCH` from any `-prerelease` suffix and
    /// compares each half independently. Within the prerelease tail a
    /// missing suffix outranks a present one (SemVer rule: `1.0.0`
    /// beats `1.0.0-beta.1`), and otherwise numeric segments compare
    /// numerically while string segments compare lexically. This is the
    /// behavior we need for `0.1.0-beta.31` to read as newer than
    /// `0.1.0-beta.30` — the previous implementation stripped the
    /// suffix and treated every beta build as equivalent.
    fn is_newer(latest: &str, running: &str) -> bool {
        compare(latest, running) == std::cmp::Ordering::Greater
    }

    fn compare(a: &str, b: &str) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        let split = |v: &str| -> (Vec<u64>, Option<String>) {
            let (core, pre) = match v.split_once('-') {
                Some((c, p)) => (c, Some(p.split('+').next().unwrap_or(p).to_string())),
                None => (v.split('+').next().unwrap_or(v), None),
            };
            let nums = core
                .split('.')
                .map(|s| s.parse::<u64>().unwrap_or(0))
                .collect();
            (nums, pre)
        };

        let (an, ap) = split(a);
        let (bn, bp) = split(b);

        let len = an.len().max(bn.len());
        for i in 0..len {
            let x = an.get(i).copied().unwrap_or(0);
            let y = bn.get(i).copied().unwrap_or(0);
            match x.cmp(&y) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }

        match (ap, bp) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(ap), Some(bp)) => {
                for (x, y) in ap.split('.').zip(bp.split('.')) {
                    let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                        (Ok(xn), Ok(yn)) => xn.cmp(&yn),
                        _ => x.cmp(y),
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                ap.split('.').count().cmp(&bp.split('.').count())
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_sparkle_feed() {
            let xml = r#"<?xml version="1.0"?>
            <rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
              <channel>
                <item>
                  <sparkle:shortVersionString>0.8.0</sparkle:shortVersionString>
                  <enclosure url="https://example.com/con-0.8.0.zip" length="12345" type="application/zip" sparkle:edSignature="abc"/>
                </item>
              </channel>
            </rss>"#;
            let (v, u) = parse_latest(xml).unwrap();
            assert_eq!(v, "0.8.0");
            assert_eq!(u, "https://example.com/con-0.8.0.zip");
        }

        #[test]
        fn version_comparison() {
            assert!(is_newer("0.8.0", "0.7.9"));
            assert!(is_newer("0.7.10", "0.7.9"));
            assert!(!is_newer("0.7.9", "0.7.9"));
            assert!(!is_newer("0.7.9", "0.8.0"));
            assert!(is_newer("1.0.0", "0.9.99"));
            // Pre-release bumps within the same core version.
            assert!(is_newer("0.1.0-beta.31", "0.1.0-beta.30"));
            assert!(!is_newer("0.1.0-beta.30", "0.1.0-beta.31"));
            // GA outranks any prerelease of the same core version.
            assert!(is_newer("0.1.0", "0.1.0-beta.30"));
            assert!(!is_newer("0.1.0-beta.30", "0.1.0"));
            // A newer core trumps prerelease ordering.
            assert!(is_newer("0.8.0-beta.1", "0.7.9"));
        }
    }
}
