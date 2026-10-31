//! Per-terminal native conversation identity, independent of presentation status.
use con_core::native_session::NativeAgentSession;
use std::{cell::RefCell, rc::Rc};

#[cfg(target_os = "macos")]
static AUTOMATIC_INTEGRATION: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "macos")]
static INTEGRATION_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
pub(crate) fn automatic_integration_error() -> Option<String> {
    INTEGRATION_ERROR
        .lock()
        .ok()
        .and_then(|error| error.clone())
}

/// Prepare before the first shell starts; never run from a render path.
#[cfg(target_os = "macos")]
pub(crate) fn prepare_automatic_integration(enabled: bool) -> Result<(), String> {
    let result = prepare_integration(enabled);
    if let Ok(mut error) = INTEGRATION_ERROR.lock() {
        *error = result.as_ref().err().cloned();
    }
    result
}

#[cfg(target_os = "macos")]
fn prepare_integration(enabled: bool) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    AUTOMATIC_INTEGRATION.store(false, Ordering::Relaxed);
    if !enabled {
        return Ok(());
    }
    let helper = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name("con-cli");
    let output = std::process::Command::new(helper)
        .args(["--json", "native-session", "prepare"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("Could not prepare native session recovery: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(1200)
            .collect());
    }
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    if response["prepared"].as_bool() != Some(true) {
        return Err("Native integration did not confirm setup".into());
    }
    AUTOMATIC_INTEGRATION.store(true, Ordering::Relaxed);
    Ok(())
}

pub(crate) type SharedNativeAgent = Rc<RefCell<NativeAgentState>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeAgentPhase {
    Available,
    Observed,
    Resuming,
    Started,
    Failed,
    NeedsInteraction,
}

pub(crate) struct NativeAgentState {
    pub origin: uuid::Uuid,
    pub shell_command: Option<String>,
    pub reference: Option<NativeAgentSession>,
    pub phase: NativeAgentPhase,
    pub error: Option<String>,
    pub auto_attempted: bool,
    pub restored: bool,
    pub report_sequence: u64,
    pub resume_submitted_at: Option<std::time::Instant>,
}

impl Default for NativeAgentState {
    fn default() -> Self {
        Self {
            origin: uuid::Uuid::new_v4(),
            shell_command: None,
            reference: None,
            phase: NativeAgentPhase::Available,
            error: None,
            auto_attempted: false,
            restored: false,
            report_sequence: 0,
            resume_submitted_at: None,
        }
    }
}

impl NativeAgentState {
    pub fn restore(&mut self, reference: NativeAgentSession) {
        self.reference = Some(reference);
        self.restored = true;
        self.phase = NativeAgentPhase::Available;
        if let Some(error) = self
            .reference
            .as_ref()
            .and_then(|reference| reference.validate().err())
        {
            self.phase = NativeAgentPhase::Failed;
            self.error = Some(error);
            self.auto_attempted = true;
        }
    }
    pub fn summary(&self) -> Option<String> {
        let reference = self.reference.as_ref()?;
        let state = match self.phase {
            NativeAgentPhase::Available => "Saved conversation · Resume available",
            NativeAgentPhase::Observed => "Native conversation linked",
            NativeAgentPhase::Resuming => "Resuming conversation…",
            NativeAgentPhase::Started => "Native CLI started · Waiting for session confirmation",
            NativeAgentPhase::Failed => "Could not resume conversation",
            NativeAgentPhase::NeedsInteraction => "Resume needs attention",
        };
        Some(format!(
            "{}\n{}{}",
            reference.label(),
            state,
            self.error
                .as_ref()
                .map(|e| format!("\n{e}"))
                .unwrap_or_default()
        ))
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn launch_environment(state: Option<&SharedNativeAgent>) -> Vec<(String, String)> {
    state
        .map(|state| {
            let mut environment = vec![
                (
                    "CON_TERMINAL_CONTEXT".into(),
                    state.borrow().origin.to_string(),
                ),
                (
                    "CON_SOCKET_PATH".into(),
                    con_core::control_socket_path()
                        .to_string_lossy()
                        .into_owned(),
                ),
                (
                    "CON_NATIVE_SESSION_AUTO".into(),
                    if AUTOMATIC_INTEGRATION.load(std::sync::atomic::Ordering::Relaxed) {
                        "1"
                    } else {
                        "0"
                    }
                    .into(),
                ),
            ];
            if AUTOMATIC_INTEGRATION.load(std::sync::atomic::Ordering::Relaxed) {
                let shell = state
                    .borrow()
                    .shell_command
                    .clone()
                    .or_else(|| std::env::var("SHELL").ok());
                environment.extend(shell_integration_environment(shell.as_deref()));
            }
            environment
        })
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn shell_integration_environment(shell: Option<&str>) -> Vec<(String, String)> {
    let command = shell
        .unwrap_or("")
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(['\'', '"']);
    let name = std::path::Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let base = con_paths::app_data_dir().join("native-session");
    let runtime = base.join("runtime");
    let mut environment = vec![
        (
            "CON_NATIVE_SESSION_BIN".into(),
            base.join("bin").to_string_lossy().into_owned(),
        ),
        (
            "CON_NATIVE_SHELL_ROOT".into(),
            runtime.to_string_lossy().into_owned(),
        ),
    ];
    match name {
        "zsh" => {
            environment.push((
                "CON_NATIVE_ORIGINAL_ZDOTDIR_SET".into(),
                if std::env::var_os("ZDOTDIR").is_some() {
                    "1"
                } else {
                    "0"
                }
                .into(),
            ));
            environment.push((
                "CON_NATIVE_ORIGINAL_ZDOTDIR".into(),
                std::env::var("ZDOTDIR").unwrap_or_default(),
            ));
            environment.push((
                "ZDOTDIR".into(),
                runtime.join("zsh").to_string_lossy().into_owned(),
            ));
        }
        _ => return Vec::new(),
    }
    environment
}
