use crossbeam_channel::Sender;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::context::PaneMode;
use crate::context::{
    PaneActionRecord, PaneShellContext, RemoteWorkspaceAnchor, ssh_target_from_recent_actions,
};
use crate::control::{
    PaneAddressSpace, PaneControlCapability, PaneControlChannel, PaneProtocolAttachment,
    PaneVisibleTarget, PaneVisibleTargetKind, TmuxControlMode, TmuxControlState,
};
use crate::shell_probe::ShellProbeResult;
use crate::tmux::{TmuxCapture, TmuxExecLocation, TmuxExecResult, TmuxPaneInfo, TmuxSnapshot};

/// Error type for agent tool execution
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("command failed: {0}")]
    CommandFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

// ── Path validation ─────────────────────────────────────────────────

/// Validate and canonicalize a path, ensuring it stays within the allowed root.
/// Used by all file tools to prevent path traversal attacks.
fn validate_path(raw: &str, allowed_root: &Path) -> Result<PathBuf, ToolError> {
    let path = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        allowed_root.join(raw)
    };
    let canonical = path
        .canonicalize()
        .map_err(|e| ToolError::CommandFailed(format!("path '{}': {}", raw, e)))?;
    if !canonical.starts_with(allowed_root) {
        return Err(ToolError::CommandFailed(format!(
            "path '{}' is outside the allowed directory '{}'",
            raw,
            allowed_root.display()
        )));
    }
    Ok(canonical)
}

/// Validate a path for write operations where the file may not exist yet.
/// Canonicalizes the parent directory and verifies it's within the allowed root.
fn validate_path_for_write(raw: &str, allowed_root: &Path) -> Result<PathBuf, ToolError> {
    let path = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        allowed_root.join(raw)
    };
    let parent = path
        .parent()
        .ok_or_else(|| ToolError::CommandFailed("invalid path: no parent directory".into()))?;
    // Create parent if needed, then canonicalize
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
    }
    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| ToolError::CommandFailed(format!("parent of '{}': {}", raw, e)))?;
    if !canonical_parent.starts_with(allowed_root) {
        return Err(ToolError::CommandFailed(format!(
            "path '{}' is outside the allowed directory '{}'",
            raw,
            allowed_root.display()
        )));
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| ToolError::CommandFailed("invalid path: no filename".into()))?;
    Ok(canonical_parent.join(file_name))
}

// ── terminal_exec (visible) ─────────────────────────────────────────

/// Request to execute a command in a visible terminal pane.
/// When `pane_index` is None, targets the focused pane.
#[derive(Debug)]
pub struct TerminalExecRequest {
    pub command: String,
    pub working_dir: Option<String>,
    pub target: PaneSelector,
    pub response_tx: Sender<TerminalExecResponse>,
}

/// Response from a visible terminal execution.
#[derive(Debug, Clone)]
pub struct TerminalExecResponse {
    pub output: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneCreateLocation {
    Right,
    Down,
}

impl PaneCreateLocation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PaneSelector {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
}

impl PaneSelector {
    pub const fn new(pane_index: Option<usize>, pane_id: Option<usize>) -> Self {
        Self {
            pane_index,
            pane_id,
        }
    }

    pub fn describe(self) -> String {
        match (self.pane_index, self.pane_id) {
            (Some(index), Some(id)) => format!("pane {index} (id {id})"),
            (Some(index), None) => format!("pane {index}"),
            (None, Some(id)) => format!("pane id {id}"),
            (None, None) => "focused pane".to_string(),
        }
    }
}

fn selector_from(index: Option<usize>, id: Option<usize>) -> PaneSelector {
    PaneSelector::new(index, id)
}

fn require_pane_target(
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    tool_name: &str,
) -> Result<PaneSelector, ToolError> {
    if pane_index.is_none() && pane_id.is_none() {
        return Err(ToolError::CommandFailed(format!(
            "{tool_name} requires pane_index or pane_id"
        )));
    }
    Ok(PaneSelector::new(pane_index, pane_id))
}

#[derive(Deserialize)]
pub struct TerminalExecArgs {
    pub command: String,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
}

/// Tool that executes commands in the user's visible terminal.
///
/// Unlike `ShellExecTool` (which runs commands in a hidden subprocess),
/// this tool writes commands directly to the terminal PTY. The user sees
/// the command execute in real time — full transparency.
///
/// Communication flow:
/// 1. Tool sends `TerminalExecRequest` to the workspace via channel
/// 2. Workspace writes command to focused PTY
/// 3. Shell integration (OSC 133) reports completion with output
/// 4. Workspace sends `TerminalExecResponse` back
/// 5. Tool returns the output to the agent
pub struct TerminalExecTool {
    request_tx: Sender<TerminalExecRequest>,
}

impl TerminalExecTool {
    pub fn new(request_tx: Sender<TerminalExecRequest>) -> Self {
        Self { request_tx }
    }
}

impl Tool for TerminalExecTool {
    const NAME: &'static str = "terminal_exec";
    type Error = ToolError;
    type Args = TerminalExecArgs;
    type Output = ShellExecOutput;

    fn description(&self) -> String {
        "Execute a command visibly in a con terminal pane only when that pane's control_capabilities include exec_visible_shell. Use pane_id from list_panes for stable follow-up targeting; pane_index is only a positional snapshot. These selectors refer to a con pane, not a tmux pane/window/editor target. For tmux, agent CLIs, and other TUIs, inspect first and use tmux-native tools or send_keys intentionally.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "pane_index": {
                    "type": "integer",
                    "description": "Target pane index from the latest list_panes snapshot. Positional only; it can change when panes are added or removed."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up work."
                }
            },
            "required": ["command"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);

        self.request_tx
            .send(TerminalExecRequest {
                command: args.command,
                working_dir: None,
                target: selector_from(args.pane_index, args.pane_id),
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Terminal exec channel closed".into()))?;

        // Wait for the terminal to execute and report back.
        // 60s timeout: most commands finish quickly, but builds/tests can take longer.
        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(60))
                .map_err(|_| {
                    ToolError::CommandFailed(
                        "Terminal exec timed out (60s) — the command may still be running".into(),
                    )
                })
        })?;

        Ok(ShellExecOutput {
            stdout: response.output,
            stderr: String::new(),
            exit_code: response.exit_code,
        })
    }
}

// ── shell_exec (background) ────────────────────────────────────────

#[derive(Deserialize)]
pub struct ShellExecArgs {
    pub command: String,
    pub working_dir: Option<String>,
}

#[derive(Serialize)]
pub struct ShellExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

pub struct ShellExecTool;

impl Tool for ShellExecTool {
    const NAME: &'static str = "shell_exec";
    type Error = ToolError;
    type Args = ShellExecArgs;
    type Output = ShellExecOutput;

    fn description(&self) -> String {
        "Execute a shell command in a hidden LOCAL background process on the con workspace machine. Output is captured but not shown in the terminal. Never use this for remote SSH/tmux inspection or remote mutations.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional working directory"
                }
            },
            "required": ["command"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = std::process::Command::new(&shell);
        cmd.arg("-c").arg(&args.command);
        if let Some(dir) = &args.working_dir {
            cmd.current_dir(dir);
        }
        let output = cmd.output()?;
        Ok(ShellExecOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code(),
        })
    }
}

// ── file_read ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct FileReadArgs {
    pub path: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

pub struct FileReadTool {
    allowed_root: PathBuf,
}

impl FileReadTool {
    pub fn new(allowed_root: PathBuf) -> Self {
        Self { allowed_root }
    }
}

impl Tool for FileReadTool {
    const NAME: &'static str = "file_read";
    type Error = ToolError;
    type Args = FileReadArgs;
    type Output = String;

    fn description(&self) -> String {
        "Read a LOCAL file on the workspace machine. This tool CANNOT read files on remote SSH hosts. For remote files, use read_pane to see editor content or send_keys to run cat in a remote shell.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional start line (1-indexed)"
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional end line (1-indexed)"
                }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = validate_path(&args.path, &self.allowed_root)?;
        let content = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = content.lines().collect();
        let start = args
            .start_line
            .unwrap_or(1)
            .saturating_sub(1)
            .min(lines.len());
        let end = args
            .end_line
            .unwrap_or(lines.len())
            .min(lines.len())
            .max(start);
        Ok(lines[start..end].join("\n"))
    }
}

// ── file_write ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct FileWriteArgs {
    pub path: String,
    pub content: String,
}

pub struct FileWriteTool {
    allowed_root: PathBuf,
}

impl FileWriteTool {
    pub fn new(allowed_root: PathBuf) -> Self {
        Self { allowed_root }
    }
}

impl Tool for FileWriteTool {
    const NAME: &'static str = "file_write";
    type Error = ToolError;
    type Args = FileWriteArgs;
    type Output = String;

    fn description(&self) -> String {
        "Write content to a LOCAL file on the workspace machine. Creates the file if it doesn't exist. CANNOT write to remote SSH hosts — use send_keys to operate remote editors or shell redirects instead."
                .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = validate_path_for_write(&args.path, &self.allowed_root)?;
        std::fs::write(&path, &args.content)?;
        Ok(format!(
            "Wrote {} bytes to {}",
            args.content.len(),
            args.path
        ))
    }
}

// ── edit_file ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct EditFileArgs {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
}

/// Surgical file editing: finds `old_text` in the file and replaces it with `new_text`.
/// Much safer than file_write (which overwrites the entire file).
pub struct EditFileTool {
    allowed_root: PathBuf,
}

impl EditFileTool {
    pub fn new(allowed_root: PathBuf) -> Self {
        Self { allowed_root }
    }
}

impl Tool for EditFileTool {
    const NAME: &'static str = "edit_file";
    type Error = ToolError;
    type Args = EditFileArgs;
    type Output = String;

    fn description(&self) -> String {
        "Edit a LOCAL file on the workspace machine by replacing a specific text snippet. CANNOT edit files on remote SSH hosts — use send_keys with editor commands instead.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to find and replace (must match exactly)"
                },
                "new_text": {
                    "type": "string",
                    "description": "The replacement text"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = validate_path(&args.path, &self.allowed_root)?;
        let content = std::fs::read_to_string(&path)?;

        let count = content.matches(&args.old_text).count();
        if count == 0 {
            return Err(ToolError::CommandFailed(format!(
                "old_text not found in {}",
                args.path
            )));
        }
        if count > 1 {
            return Err(ToolError::CommandFailed(format!(
                "old_text matches {} times in {} — must be unique. Provide more surrounding context.",
                count, args.path
            )));
        }

        let new_content = content.replacen(&args.old_text, &args.new_text, 1);
        std::fs::write(&path, &new_content)?;

        let old_lines = args.old_text.lines().count();
        let new_lines = args.new_text.lines().count();
        Ok(format!(
            "Edited {}: replaced {} line(s) with {} line(s)",
            args.path, old_lines, new_lines
        ))
    }
}

// ── list_files ─────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ListFilesArgs {
    pub path: Option<String>,
    pub pattern: Option<String>,
    pub max_depth: Option<usize>,
}

/// List files in a directory, optionally filtered by glob pattern.
pub struct ListFilesTool {
    allowed_root: PathBuf,
}

impl ListFilesTool {
    pub fn new(allowed_root: PathBuf) -> Self {
        Self { allowed_root }
    }
}

impl Tool for ListFilesTool {
    const NAME: &'static str = "list_files";
    type Error = ToolError;
    type Args = ListFilesArgs;
    type Output = String;

    fn description(&self) -> String {
        "List LOCAL files and directories on the workspace machine. CANNOT list files on remote SSH hosts.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list (defaults to cwd)"
                },
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to filter (e.g. '*.rs', '**/*.toml')"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum directory depth (default: 3)"
                }
            }
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let dir = match &args.path {
            Some(p) => validate_path(p, &self.allowed_root)?,
            None => self.allowed_root.clone(),
        };
        let max_depth = args.max_depth.unwrap_or(3);

        // Try git ls-files first (respects .gitignore, fast)
        let git_listing = std::process::Command::new("git")
            .args(["ls-files", "--cached", "--others", "--exclude-standard"])
            .current_dir(&dir)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .filter(|s| !s.trim().is_empty());

        // Fall back to find for non-git directories
        let stdout = match git_listing {
            Some(listing) => {
                let filtered: Vec<&str> = listing
                    .lines()
                    .filter(|path| {
                        let depth = path.chars().filter(|c| *c == '/').count();
                        if depth >= max_depth {
                            return false;
                        }
                        if let Some(ref pattern) = args.pattern {
                            let filename = path.rsplit('/').next().unwrap_or(path);
                            glob_match(pattern, filename)
                        } else {
                            true
                        }
                    })
                    .collect();
                filtered.join("\n")
            }
            None => {
                let mut cmd = std::process::Command::new("find");
                cmd.arg(&dir);
                cmd.args(["-maxdepth", &max_depth.to_string()]);
                cmd.args(["-not", "-path", "*/.git/*"]);
                if let Some(ref pattern) = args.pattern {
                    cmd.args(["-name", pattern]);
                }
                cmd.args(["-type", "f"]);
                let output = cmd.output()?;
                String::from_utf8_lossy(&output.stdout).to_string()
            }
        };

        let mut files: Vec<&str> = stdout.lines().collect();
        files.sort();

        let total = files.len();
        let listing: String = files.into_iter().take(500).collect::<Vec<_>>().join("\n");

        if total > 500 {
            Ok(format!("{}\n... ({} more files)", listing, total - 500))
        } else {
            Ok(listing)
        }
    }
}

// ── search ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SearchArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub file_pattern: Option<String>,
}

#[derive(Serialize)]
pub struct SearchOutput {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
}

#[derive(Serialize)]
pub struct SearchMatch {
    pub file: String,
    pub line: usize,
    pub text: String,
}

pub struct SearchTool {
    allowed_root: PathBuf,
}

impl SearchTool {
    pub fn new(allowed_root: PathBuf) -> Self {
        Self { allowed_root }
    }
}

impl Tool for SearchTool {
    const NAME: &'static str = "search";
    type Error = ToolError;
    type Args = SearchArgs;
    type Output = SearchOutput;

    fn description(&self) -> String {
        "Search for text in LOCAL files on the workspace machine using grep. CANNOT search remote SSH hosts. Returns matching lines with file paths and line numbers.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (passed to grep -rn)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (defaults to cwd)"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "Glob pattern for files (e.g. '*.rs')"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let search_dir = match &args.path {
            Some(p) => validate_path(p, &self.allowed_root)?,
            None => self.allowed_root.clone(),
        };

        let mut cmd = std::process::Command::new("grep");
        cmd.args(["-rn", "--max-count=100"]);
        if let Some(ref fp) = args.file_pattern {
            cmd.args(["--include", fp]);
        }
        cmd.arg("--").arg(&args.pattern);
        cmd.arg(&search_dir);

        let output = cmd.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut matches = Vec::new();
        let lines: Vec<&str> = stdout.lines().collect();
        let truncated = lines.len() >= 100;

        for line in lines.into_iter().take(100) {
            let mut parts = line.splitn(3, ':');
            if let (Some(file), Some(line_no), Some(text)) =
                (parts.next(), parts.next(), parts.next())
            {
                matches.push(SearchMatch {
                    file: file.to_string(),
                    line: line_no.parse().unwrap_or(0),
                    text: text.to_string(),
                });
            }
        }

        Ok(SearchOutput { matches, truncated })
    }
}

// ── Pane interaction layer ──────────────────────────────────────────
//
// Panes are first-class addressable entities. The agent discovers them
// via `list_panes`, reads content via `read_pane`, and sends input via
// `send_keys`. This abstraction supports shell, TUI, and future
// agent-in-pane scenarios uniformly.
//
// Communication follows the same channel pattern as TerminalExecTool:
// Tool → PaneRequest → Workspace → PaneResponse → Tool

/// Metadata about a single terminal pane.
///
/// Includes runtime and control state so the agent does not execute against
/// the wrong target or over-trust missing backend facts.
#[derive(Debug, Clone, Serialize)]
pub struct PaneInfo {
    pub index: usize,
    /// Stable pane id within the current tab lifetime.
    pub pane_id: usize,
    pub title: String,
    pub cwd: Option<String>,
    pub is_focused: bool,
    pub rows: usize,
    pub cols: usize,
    /// Whether the embedded Ghostty surface has actually been initialized.
    pub surface_ready: bool,
    /// Whether the PTY child process is still running.
    pub is_alive: bool,
    /// Proven hostname when the backend can actually supply one.
    pub hostname: Option<String>,
    /// Confidence for the effective hostname, when detected.
    pub hostname_confidence: Option<crate::context::PaneConfidence>,
    /// Evidence source for the effective hostname, when detected.
    pub hostname_source: Option<crate::context::PaneEvidenceSource>,
    /// Best current remote SSH workspace anchor for this pane.
    pub remote_workspace: Option<RemoteWorkspaceAnchor>,
    /// Current verified front-state for the pane.
    pub front_state: crate::context::PaneFrontState,
    /// Current pane mode: shell, tmux-like multiplexer, or another TUI.
    pub mode: PaneMode,
    /// Whether shell metadata like cwd and last_command is likely fresh for the visible app.
    pub shell_metadata_fresh: bool,
    /// Whether the most recent typed shell-context snapshot still matches the current shell frame.
    pub shell_context_fresh: bool,
    /// What the backend can authoritatively observe for this pane today.
    pub observation_support: crate::context::PaneObservationSupport,
    /// The only address space valid for pane_index today.
    pub address_space: PaneAddressSpace,
    /// The best-known visible target inside this con pane.
    pub visible_target: PaneVisibleTarget,
    /// Nested runtime/control targets from outer shell toward the front-most visible app.
    pub target_stack: Vec<PaneVisibleTarget>,
    /// tmux adapter state when a tmux layer is present in this pane.
    pub tmux_control: Option<TmuxControlState>,
    /// Explicit protocol attachments currently available on this pane.
    pub control_attachments: Vec<PaneProtocolAttachment>,
    /// Control channels con can use on this pane.
    pub control_channels: Vec<PaneControlChannel>,
    /// Capabilities currently available on this pane.
    pub control_capabilities: Vec<PaneControlCapability>,
    /// Addressing and control notes the agent should respect.
    pub control_notes: Vec<String>,
    /// The top-most active runtime scope, when detected.
    pub active_scope: Option<crate::context::PaneRuntimeScope>,
    /// Detected external agent CLI, when visible.
    pub agent_cli: Option<String>,
    /// Evidence behind the current runtime summary.
    pub evidence: Vec<crate::context::PaneEvidence>,
    /// Structured runtime scopes inferred from pane-local evidence.
    pub runtime_stack: Vec<crate::context::PaneRuntimeScope>,
    /// Last verified shell-frame stack captured for this pane.
    pub last_verified_runtime_stack: Vec<crate::context::PaneRuntimeScope>,
    /// Warnings about stale or advisory runtime metadata.
    pub runtime_warnings: Vec<String>,
    /// Last typed shell-context snapshot captured for this pane.
    pub shell_context: Option<PaneShellContext>,
    /// Recent con-originated actions for this pane.
    pub recent_actions: Vec<PaneActionRecord>,
    /// Weak observation hints derived from the current visible screen snapshot.
    pub screen_hints: Vec<crate::context::PaneObservationHint>,
    /// tmux session hint when detected from the pane itself.
    pub tmux_session: Option<String>,
    /// Whether shell integration (OSC 133) is active.
    pub has_shell_integration: bool,
    /// Most recent command text when the backend can prove it.
    pub last_command: Option<String>,
    /// Exit code of the last command.
    pub last_exit_code: Option<i32>,
    /// A command is currently executing (between OSC 133 C and D).
    /// Only reliable when has_shell_integration is true.
    pub is_busy: bool,
}

/// A request from a pane tool to the workspace.
#[derive(Debug)]
pub struct PaneRequest {
    pub query: PaneQuery,
    pub response_tx: Sender<PaneResponse>,
}

/// Pane query types — the workspace interprets these against PaneTree/Grid.
#[derive(Debug)]
pub enum PaneQuery {
    /// List all panes with metadata.
    List,
    /// Read recent output from a specific pane.
    ReadContent { target: PaneSelector, lines: usize },
    /// Send raw keystrokes to a specific pane (for TUI interaction, Ctrl-C, etc.).
    SendKeys { target: PaneSelector, keys: String },
    /// Search scrollback + visible screen for a text pattern.
    SearchText {
        target: PaneSelector,
        pattern: String,
        max_matches: usize,
    },
    /// Return tmux adapter state for a pane whose target stack contains tmux.
    InspectTmux { target: PaneSelector },
    /// Query tmux windows/panes through a same-session tmux control anchor.
    TmuxList { pane: PaneSelector },
    /// Capture pane content from a tmux pane target through a same-session tmux control anchor.
    TmuxCapture {
        pane: PaneSelector,
        target: Option<String>,
        lines: usize,
    },
    /// Send literal text or tmux key names to a tmux pane target through a same-session tmux control anchor.
    TmuxSendKeys {
        pane: PaneSelector,
        target: String,
        literal_text: Option<String>,
        key_names: Vec<String>,
        append_enter: bool,
    },
    /// Run a command through tmux itself by creating a new tmux target.
    TmuxRunCommand {
        pane: PaneSelector,
        target: Option<String>,
        location: TmuxExecLocation,
        command: String,
        window_name: Option<String>,
        cwd: Option<String>,
        detached: bool,
    },
    /// Run a read-only shell-scoped probe in a pane with a proven fresh shell prompt.
    ProbeShellContext { target: PaneSelector },
    /// Lightweight busy check for a single pane (used by wait_for polling).
    /// Returns only is_busy + has_shell_integration, avoiding full List forensics.
    CheckBusy { target: PaneSelector },
    /// Wait for a pane to become idle or match a pattern.
    WaitFor {
        target: PaneSelector,
        timeout_secs: Option<u64>,
        pattern: Option<String>,
    },
    /// Create a new terminal pane (tab), optionally running a command in it.
    CreatePane {
        command: Option<String>,
        location: PaneCreateLocation,
    },
}

/// Response from the workspace to a pane tool.
#[derive(Debug, Clone)]
pub enum PaneResponse {
    PaneList(Vec<PaneInfo>),
    Content(String),
    KeysSent,
    TmuxInfo(TmuxControlState),
    TmuxList(TmuxSnapshot),
    TmuxCapture(TmuxCapture),
    TmuxExec(TmuxExecResult),
    ShellProbe(ShellProbeResult),
    /// Search results: Vec of (pane_index, line_number, line_text).
    SearchResults(Vec<(usize, usize, String)>),
    /// Lightweight busy-check response.
    BusyStatus {
        surface_ready: bool,
        is_alive: bool,
        is_busy: bool,
        has_shell_integration: bool,
    },
    /// Response from a wait_for operation.
    WaitComplete {
        status: String,
        output: String,
    },
    /// A new pane was created successfully.
    PaneCreated {
        pane_index: usize,
        pane_id: usize,
        surface_ready: bool,
        is_alive: bool,
        has_shell_integration: bool,
    },
    Error(String),
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TmuxTargetKindFilter {
    Any,
    Shell,
    AgentCli,
}

#[derive(Debug, Clone, Serialize)]
pub struct TmuxFindTargetsResult {
    pub kind: TmuxTargetKindFilter,
    pub best_match: Option<crate::tmux::TmuxPaneInfo>,
    pub matches: Vec<crate::tmux::TmuxPaneInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TmuxEnsureShellTargetResult {
    pub created: bool,
    pub pane: crate::tmux::TmuxPaneInfo,
    pub creation: Option<TmuxExecResult>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCliNativeAttachmentState {
    Unavailable,
    LaunchIntegratedOnly,
}

#[derive(Debug, Clone, Serialize)]
pub struct TmuxEnsureAgentTargetResult {
    pub agent_name: String,
    pub created: bool,
    pub pane: crate::tmux::TmuxPaneInfo,
    pub creation: Option<TmuxExecResult>,
    pub launch_command: Option<String>,
    pub native_attachment_state: AgentCliNativeAttachmentState,
    pub native_attachment_note: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteShellMatchSource {
    ProvenHost,
    ActionHistory,
    Created,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnsureRemoteShellTargetResult {
    pub host: String,
    pub created: bool,
    pub pane_index: usize,
    pub pane_id: Option<usize>,
    pub match_source: RemoteShellMatchSource,
    pub command: Option<String>,
    pub output: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkTargetIntent {
    VisibleShell,
    RemoteShell,
    TmuxWorkspace,
    TmuxShell,
    AgentCli,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkTargetControlPath {
    VisibleShellExec,
    RemoteShellTarget,
    LocalShellTarget,
    LocalAgentTarget,
    TmuxQuery,
    TmuxShellTarget,
    TmuxAgentTarget,
    VisibleAgentUi,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkTargetCandidate {
    pub pane_index: usize,
    pub pane_id: usize,
    pub pane_title: String,
    pub host: Option<String>,
    pub control_path: WorkTargetControlPath,
    pub visible_target: PaneVisibleTarget,
    pub tmux_mode: Option<TmuxControlMode>,
    pub tmux_target: Option<crate::tmux::TmuxPaneInfo>,
    pub requires_preparation: bool,
    pub suggested_tool: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolveWorkTargetResult {
    pub intent: WorkTargetIntent,
    pub best_match: Option<WorkTargetCandidate>,
    pub candidates: Vec<WorkTargetCandidate>,
}

fn recv_pane_response(
    response_rx: crossbeam_channel::Receiver<PaneResponse>,
    timeout_secs: u64,
    timeout_label: &'static str,
) -> Result<PaneResponse, ToolError> {
    tokio::task::block_in_place(|| {
        response_rx
            .recv_timeout(std::time::Duration::from_secs(timeout_secs))
            .map_err(|_| ToolError::CommandFailed(format!("{timeout_label} timed out")))
    })
}

fn is_transient_pane_read_timeout(err: &ToolError) -> bool {
    matches!(err, ToolError::CommandFailed(message) if message == "pane read timed out")
}

fn pane_query_tmux_list(
    pane_tx: &Sender<PaneRequest>,
    target: PaneSelector,
) -> Result<TmuxSnapshot, ToolError> {
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::TmuxList { pane: target },
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    match recv_pane_response(response_rx, 12, "tmux list")? {
        PaneResponse::TmuxList(snapshot) => Ok(snapshot),
        PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
        _ => Err(ToolError::CommandFailed("Unexpected response".into())),
    }
}

fn pane_query_tmux_run_command(
    pane_tx: &Sender<PaneRequest>,
    pane: PaneSelector,
    target: Option<String>,
    location: TmuxExecLocation,
    command: String,
    window_name: Option<String>,
    cwd: Option<String>,
    detached: bool,
) -> Result<TmuxExecResult, ToolError> {
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::TmuxRunCommand {
                pane,
                target,
                location,
                command,
                window_name,
                cwd,
                detached,
            },
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    match recv_pane_response(response_rx, 12, "tmux run-command")? {
        PaneResponse::TmuxExec(exec) => Ok(exec),
        PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
        _ => Err(ToolError::CommandFailed("Unexpected response".into())),
    }
}

fn pane_query_tmux_capture(
    pane_tx: &Sender<PaneRequest>,
    pane: PaneSelector,
    target: Option<String>,
    lines: usize,
) -> Result<TmuxCapture, ToolError> {
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::TmuxCapture {
                pane,
                target,
                lines,
            },
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    match recv_pane_response(response_rx, 12, "tmux capture")? {
        PaneResponse::TmuxCapture(capture) => Ok(capture),
        PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
        _ => Err(ToolError::CommandFailed("Unexpected response".into())),
    }
}

fn pane_query_read_content(
    pane_tx: &Sender<PaneRequest>,
    target: PaneSelector,
    lines: usize,
) -> Result<String, ToolError> {
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::ReadContent { target, lines },
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    match recv_pane_response(response_rx, 12, "pane read")? {
        PaneResponse::Content(content) => Ok(content),
        PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
        _ => Err(ToolError::CommandFailed("Unexpected response".into())),
    }
}

fn pane_query_send_keys(
    pane_tx: &Sender<PaneRequest>,
    target: PaneSelector,
    keys: String,
) -> Result<(), ToolError> {
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::SendKeys { target, keys },
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    match recv_pane_response(response_rx, 5, "send keys")? {
        PaneResponse::KeysSent => Ok(()),
        PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
        _ => Err(ToolError::CommandFailed("Unexpected response".into())),
    }
}

fn pane_query_tmux_send_keys(
    pane_tx: &Sender<PaneRequest>,
    pane: PaneSelector,
    target: String,
    literal_text: Option<String>,
    key_names: Vec<String>,
    append_enter: bool,
) -> Result<String, ToolError> {
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::TmuxSendKeys {
                pane,
                target,
                literal_text,
                key_names,
                append_enter,
            },
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    match recv_pane_response(response_rx, 12, "tmux send-keys")? {
        PaneResponse::Content(content) => Ok(content),
        PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
        _ => Err(ToolError::CommandFailed("Unexpected response".into())),
    }
}

async fn submit_visible_agent_prompt(
    pane_tx: &Sender<PaneRequest>,
    target: PaneSelector,
    prompt: &str,
) -> Result<(), ToolError> {
    let prompt = prompt.trim_end_matches(['\r', '\n']);
    if !prompt.is_empty() {
        pane_query_send_keys(pane_tx, target, prompt.to_string())?;
        // Interactive CLIs often suppress Enter-as-submit when it arrives in
        // the same burst as pasted text. Send submit as a separate event.
        tokio::time::sleep(std::time::Duration::from_millis(180)).await;
    }
    pane_query_send_keys(pane_tx, target, "\r".to_string())
}

async fn submit_tmux_agent_prompt(
    pane_tx: &Sender<PaneRequest>,
    pane: PaneSelector,
    target: &str,
    prompt: &str,
) -> Result<(), ToolError> {
    let prompt = prompt.trim_end_matches(['\r', '\n']);
    if !prompt.is_empty() {
        pane_query_tmux_send_keys(
            pane_tx,
            pane,
            target.to_string(),
            Some(prompt.to_string()),
            Vec::new(),
            false,
        )?;
        tokio::time::sleep(std::time::Duration::from_millis(180)).await;
    }
    pane_query_tmux_send_keys(
        pane_tx,
        pane,
        target.to_string(),
        None,
        vec!["Enter".to_string()],
        false,
    )?;
    Ok(())
}

fn pane_query_list(pane_tx: &Sender<PaneRequest>) -> Result<Vec<PaneInfo>, ToolError> {
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::List,
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    match recv_pane_response(response_rx, 5, "pane list")? {
        PaneResponse::PaneList(panes) => Ok(panes),
        PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
        _ => Err(ToolError::CommandFailed("Unexpected response".into())),
    }
}

async fn wait_for_pane_initial_output(
    pane_tx: &Sender<PaneRequest>,
    target: PaneSelector,
) -> Result<String, ToolError> {
    const POLL_MS: u64 = 500;
    const SETTLE_POLLS: u32 = 3;
    const MAX_POLLS: u32 = 30;

    let mut initial_snapshot = String::new();
    let mut last_snapshot = String::new();
    let mut stable_count: u32 = 0;
    let mut phase_changed = false;

    for _ in 0..MAX_POLLS {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
        let (tx, rx) = crossbeam_channel::bounded(1);
        pane_tx
            .send(PaneRequest {
                query: PaneQuery::ReadContent { target, lines: 50 },
                response_tx: tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let content = match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(PaneResponse::Content(c)) => c,
            Ok(PaneResponse::Error(e)) => return Err(ToolError::CommandFailed(e)),
            _ => continue,
        };

        let normalized = content
            .lines()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        if normalized.is_empty() {
            continue;
        }

        if !phase_changed {
            if initial_snapshot.is_empty() {
                initial_snapshot = normalized.clone();
                last_snapshot = normalized;
                continue;
            }
            if normalized != initial_snapshot {
                phase_changed = true;
                last_snapshot = normalized;
                stable_count = 0;
                continue;
            }
            continue;
        }

        if normalized == last_snapshot {
            stable_count += 1;
            if stable_count >= SETTLE_POLLS {
                return Ok(normalized);
            }
        } else {
            last_snapshot = normalized;
            stable_count = 0;
        }
    }

    Ok(last_snapshot)
}

fn tmux_command_basename(command: &str) -> &str {
    command.rsplit('/').next().unwrap_or(command)
}

fn canonical_agent_cli_name(name: &str) -> Option<&'static str> {
    match name
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .replace(' ', "-")
        .as_str()
    {
        "codex" => Some("codex"),
        "claude" | "claude-code" | "claudecode" => Some("claude"),
        "opencode" | "open-code" | "open-code-cli" => Some("opencode"),
        _ => None,
    }
}

fn classify_agent_cli_command(command: &str) -> Option<&'static str> {
    canonical_agent_cli_name(tmux_command_basename(command))
}

fn is_tmux_shell_command(command: &str) -> bool {
    matches!(
        tmux_command_basename(command).to_ascii_lowercase().as_str(),
        "bash" | "zsh" | "sh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu"
    )
}

fn is_tmux_agent_cli_command(command: &str) -> bool {
    classify_agent_cli_command(command).is_some()
}

fn agent_cli_matches_selector(command: &str, agent_name: Option<&String>) -> bool {
    let Some(actual) = classify_agent_cli_command(command) else {
        return false;
    };
    agent_name
        .as_ref()
        .and_then(|name| canonical_agent_cli_name(name))
        .is_none_or(|expected| expected == actual)
}

fn default_agent_launch_command(agent_name: &str) -> Option<&'static str> {
    match canonical_agent_cli_name(agent_name)? {
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        "opencode" => Some("opencode"),
        _ => None,
    }
}

fn output_contains_codex_trust_prompt(output: &str) -> bool {
    output.contains("Do you trust the contents of this directory?")
}

fn output_contains_queue_hint(output: &str) -> bool {
    output.contains("tab to queue") || output.contains("tab to queue message")
}

async fn maybe_auto_accept_known_agent_interstitial(
    pane_tx: &Sender<PaneRequest>,
    agent_name: &str,
    target: PaneSelector,
    current_output: String,
) -> Result<(String, Option<String>), ToolError> {
    if canonical_agent_cli_name(agent_name) == Some("codex")
        && output_contains_codex_trust_prompt(&current_output)
    {
        pane_query_send_keys(pane_tx, target, "\r".to_string())?;
        let (_, settled_output) = wait_for_visible_pane_settle(pane_tx, target, 80, 30).await?;
        let output = if settled_output.trim().is_empty() {
            pane_query_read_content(pane_tx, target, 80)?
        } else {
            settled_output
        };
        return Ok((
            output,
            Some(
                "Accepted the Codex trust prompt so the interactive pane is actually ready for later turns."
                    .to_string(),
            ),
        ));
    }

    Ok((current_output, None))
}

fn agent_cli_native_attachment(agent_name: &str) -> (AgentCliNativeAttachmentState, String) {
    match canonical_agent_cli_name(agent_name) {
        Some("codex") => (
            AgentCliNativeAttachmentState::LaunchIntegratedOnly,
            "Codex has a separate app-server protocol, but con can only use it when Codex is intentionally launched with app-server transport. A normal tmux Codex session is still controlled through tmux."
                .to_string(),
        ),
        Some("opencode") => (
            AgentCliNativeAttachmentState::LaunchIntegratedOnly,
            "OpenCode has a separate server mode, but con can only use it when OpenCode is intentionally launched with an explicit server/port. A normal tmux OpenCode session is still controlled through tmux."
                .to_string(),
        ),
        Some("claude") => (
            AgentCliNativeAttachmentState::Unavailable,
            "Claude Code does not expose a separate con-native control attachment here. Inside tmux it remains an interactive tmux target."
                .to_string(),
        ),
        _ => (
            AgentCliNativeAttachmentState::Unavailable,
            "This agent CLI does not have a proven con-native attachment. Treat it as an interactive tmux target."
                .to_string(),
        ),
    }
}

fn sort_tmux_matches(matches: &mut [crate::tmux::TmuxPaneInfo]) {
    matches.sort_by(|a, b| {
        b.pane_active
            .cmp(&a.pane_active)
            .then_with(|| b.window_active.cmp(&a.window_active))
            .then_with(|| a.session_name.cmp(&b.session_name))
            .then_with(|| a.window_index.cmp(&b.window_index))
            .then_with(|| a.pane_index.cmp(&b.pane_index))
    });
}

fn tmux_creation_target(
    snapshot: &TmuxSnapshot,
    session_name: &str,
    location: TmuxExecLocation,
) -> Option<String> {
    match location {
        TmuxExecLocation::NewWindow => Some(session_name.to_string()),
        TmuxExecLocation::SplitHorizontal | TmuxExecLocation::SplitVertical => snapshot
            .panes
            .iter()
            .filter(|pane| pane.session_name == session_name)
            .max_by(|a, b| {
                a.pane_active
                    .cmp(&b.pane_active)
                    .then_with(|| a.window_active.cmp(&b.window_active))
                    .then_with(|| b.window_index.cmp(&a.window_index))
                    .then_with(|| b.pane_index.cmp(&a.pane_index))
            })
            .map(|pane| pane.target.clone())
            .or_else(|| Some(session_name.to_string())),
    }
}

fn preferred_tmux_shell_session(
    panes: &[PaneInfo],
    pane: PaneSelector,
    explicit_session_name: Option<&str>,
) -> Option<String> {
    let explicit = explicit_session_name
        .map(str::trim)
        .filter(|session| !session.is_empty())
        .map(ToString::to_string);

    if explicit.is_some() {
        return explicit;
    }

    panes
        .iter()
        .find(|candidate| {
            pane.pane_index.is_none_or(|index| candidate.index == index)
                && pane.pane_id.is_none_or(|id| candidate.pane_id == id)
        })
        .and_then(|candidate| {
            candidate
                .tmux_control
                .as_ref()
                .and_then(|tmux| tmux.session_name.clone())
                .or_else(|| candidate.tmux_session.clone())
        })
}

fn normalize_lower(value: &Option<String>) -> Option<String> {
    value.as_ref().map(|v| v.to_ascii_lowercase())
}

fn expand_home_prefix(value: &str) -> String {
    if value == "~" {
        return dirs::home_dir()
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_else(|| value.to_string());
    }
    if let Some(suffix) = value.strip_prefix("~/") {
        return dirs::home_dir()
            .map(|home| home.join(suffix).to_string_lossy().into_owned())
            .unwrap_or_else(|| value.to_string());
    }
    value.to_string()
}

fn normalize_cwd_lower(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|v| expand_home_prefix(v).to_ascii_lowercase())
}

fn build_local_cwd_command_prefix(cwd: &str, create_if_missing: bool) -> String {
    let expanded = expand_home_prefix(cwd);
    let quoted = shell_quote_fragment(&expanded);
    if create_if_missing {
        format!("mkdir -p {quoted} && cd {quoted}")
    } else {
        format!("cd {quoted}")
    }
}

fn pane_matches_host(pane: &PaneInfo, host_contains: Option<&String>) -> bool {
    host_contains.is_none_or(|needle| {
        pane.hostname
            .as_ref()
            .is_some_and(|host| host.to_ascii_lowercase().contains(needle))
            || pane
                .remote_workspace
                .as_ref()
                .is_some_and(|anchor| anchor.host.to_ascii_lowercase().contains(needle))
    })
}

fn pane_workspace_cwd_hint(pane: &PaneInfo) -> Option<String> {
    crate::context::workspace_cwd_hint(pane.cwd.as_deref(), &pane.recent_actions)
}

fn pane_matches_cwd(pane: &PaneInfo, cwd_contains: Option<&String>) -> bool {
    cwd_contains.is_none_or(|needle| {
        pane_workspace_cwd_hint(pane)
            .as_ref()
            .is_some_and(|cwd| cwd.to_ascii_lowercase().contains(needle))
    })
}

fn pane_has_capability(pane: &PaneInfo, capability: PaneControlCapability) -> bool {
    pane.control_capabilities.contains(&capability)
}

fn pane_has_tmux_native(pane: &PaneInfo) -> bool {
    pane.tmux_control
        .as_ref()
        .is_some_and(|tmux| tmux.mode == TmuxControlMode::Native)
}

fn pane_has_tmux_observation(pane: &PaneInfo) -> bool {
    pane_has_screen_hint(
        pane,
        crate::context::PaneObservationHintKind::TmuxLikeScreen,
    )
}

fn pane_has_tmux_layer(pane: &PaneInfo) -> bool {
    pane.tmux_control.is_some() || pane.tmux_session.is_some() || pane_has_tmux_observation(pane)
}

fn pane_has_remote_shell_context(pane: &PaneInfo) -> bool {
    pane.hostname.is_some()
        || pane.remote_workspace.is_some()
        || pane
            .runtime_stack
            .iter()
            .any(|scope| scope.kind == crate::context::PaneScopeKind::RemoteShell)
        || pane
            .last_verified_runtime_stack
            .iter()
            .any(|scope| scope.kind == crate::context::PaneScopeKind::RemoteShell)
}

fn pane_recent_ssh_target(pane: &PaneInfo) -> Option<String> {
    ssh_target_from_recent_actions(&pane.recent_actions)
}

fn pane_remote_host_hint(pane: &PaneInfo) -> Option<String> {
    pane.hostname
        .clone()
        .or_else(|| {
            pane.remote_workspace
                .as_ref()
                .map(|anchor| anchor.host.clone())
        })
        .or_else(|| pane_recent_ssh_target(pane))
}

fn pane_is_disconnected_workspace(pane: &PaneInfo) -> bool {
    pane_has_screen_hint(
        pane,
        crate::context::PaneObservationHintKind::SshConnectionClosed,
    )
}

fn pane_matches_remote_anchor(pane: &PaneInfo, host_contains: Option<&String>) -> bool {
    if pane_is_disconnected_workspace(pane) || pane_has_tmux_observation(pane) {
        return false;
    }
    let ssh_target = pane_recent_ssh_target(pane);
    host_contains.is_none_or(|needle| {
        pane.hostname
            .as_ref()
            .is_some_and(|host| host.to_ascii_lowercase().contains(needle))
            || pane
                .remote_workspace
                .as_ref()
                .is_some_and(|anchor| anchor.host.to_ascii_lowercase().contains(needle))
            || ssh_target
                .as_ref()
                .is_some_and(|target| target.to_ascii_lowercase().contains(needle))
    })
}

fn pane_is_visible_agent_cli(pane: &PaneInfo, agent_name: Option<&String>) -> bool {
    if pane.visible_target.kind == PaneVisibleTargetKind::AgentCli
        || pane.agent_cli.as_deref().is_some_and(|value| {
            agent_name
                .as_ref()
                .and_then(|name| canonical_agent_cli_name(name))
                .is_none_or(|expected| canonical_agent_cli_name(value) == Some(expected))
        })
    {
        return true;
    }

    // Con-launched local agent panes can lose their visible header after a few
    // turns. Preserve that lane when Con itself launched the CLI there and no
    // current shell evidence contradicts it.
    let hinted =
        crate::context::workspace_agent_cli_hint(pane.agent_cli.as_deref(), &pane.recent_actions);
    let Some(hinted) = hinted.as_deref().and_then(canonical_agent_cli_name) else {
        return false;
    };
    if agent_name
        .as_ref()
        .and_then(|name| canonical_agent_cli_name(name))
        .is_some_and(|expected| expected != hinted)
    {
        return false;
    }

    !pane_has_screen_hint(
        pane,
        crate::context::PaneObservationHintKind::PromptLikeInput,
    )
}

fn pane_has_screen_hint(pane: &PaneInfo, kind: crate::context::PaneObservationHintKind) -> bool {
    pane.screen_hints.iter().any(|hint| hint.kind == kind)
}

fn pane_recent_command_matches(pane: &PaneInfo, predicate: impl Fn(&str) -> bool) -> bool {
    pane.recent_actions
        .iter()
        .filter_map(|action| action.command.as_deref())
        .any(predicate)
}

fn pane_has_local_shell_continuity(pane: &PaneInfo, cwd_contains: Option<&String>) -> bool {
    if pane_is_disconnected_workspace(pane)
        || pane_has_tmux_layer(pane)
        || pane_has_tmux_observation(pane)
        || pane_has_remote_shell_context(pane)
        || pane.remote_workspace.is_some()
        || pane_has_any_local_agent_continuity(pane)
        || !pane_has_screen_hint(
            pane,
            crate::context::PaneObservationHintKind::PromptLikeInput,
        )
    {
        return false;
    }

    if cwd_contains.is_none() && pane.cwd.is_some() {
        return true;
    }

    pane_matches_cwd(pane, cwd_contains)
        || cwd_contains.is_some_and(|needle| {
            pane_recent_command_matches(pane, |command| {
                command.to_ascii_lowercase().contains(needle)
            })
        })
}

fn pane_has_local_agent_continuity(
    pane: &PaneInfo,
    agent_name: &str,
    cwd_contains: Option<&String>,
) -> bool {
    if pane_is_disconnected_workspace(pane)
        || pane_has_tmux_layer(pane)
        || pane_has_tmux_observation(pane)
        || pane_has_remote_shell_context(pane)
        || pane.remote_workspace.is_some()
    {
        return false;
    }

    let launch = default_agent_launch_command(agent_name).unwrap_or(agent_name);
    let launch_match = pane_recent_command_matches(pane, |command| {
        command
            .split_whitespace()
            .any(|token| classify_agent_cli_command(token) == Some(agent_name))
            || command.contains(launch)
    });

    launch_match
        && (cwd_contains.is_none()
            || pane_matches_cwd(pane, cwd_contains)
            || cwd_contains.is_some_and(|needle| {
                pane_recent_command_matches(pane, |command| {
                    command.to_ascii_lowercase().contains(needle)
                })
            }))
}

fn pane_has_any_local_agent_continuity(pane: &PaneInfo) -> bool {
    if pane_is_disconnected_workspace(pane)
        || pane_has_tmux_layer(pane)
        || pane_has_tmux_observation(pane)
        || pane_has_remote_shell_context(pane)
        || pane.remote_workspace.is_some()
    {
        return false;
    }

    pane_recent_command_matches(pane, |command| {
        command
            .split_whitespace()
            .any(|token| classify_agent_cli_command(token).is_some())
    })
}

fn workspace_kind_for_pane(pane: &PaneInfo) -> crate::context::TabWorkspaceKind {
    if pane.tmux_control.is_some()
        || pane.tmux_session.is_some()
        || pane_has_tmux_observation(pane)
        || pane
            .runtime_stack
            .iter()
            .any(|scope| scope.kind == crate::context::PaneScopeKind::Multiplexer)
        || pane
            .last_verified_runtime_stack
            .iter()
            .any(|scope| scope.kind == crate::context::PaneScopeKind::Multiplexer)
    {
        crate::context::TabWorkspaceKind::TmuxWorkspace
    } else if pane.hostname.is_some() || pane.remote_workspace.is_some() {
        crate::context::TabWorkspaceKind::RemoteShell
    } else if pane.mode == PaneMode::Shell {
        crate::context::TabWorkspaceKind::LocalShell
    } else {
        crate::context::TabWorkspaceKind::Unknown
    }
}

fn workspace_state_for_pane(
    pane: &PaneInfo,
    kind: crate::context::TabWorkspaceKind,
) -> crate::context::TabWorkspaceState {
    if pane_has_screen_hint(
        pane,
        crate::context::PaneObservationHintKind::SshConnectionClosed,
    ) {
        return crate::context::TabWorkspaceState::Disconnected;
    }
    if pane.front_state == crate::context::PaneFrontState::InteractiveSurface {
        return crate::context::TabWorkspaceState::Interactive;
    }
    let prompt_like = pane_has_screen_hint(
        pane,
        crate::context::PaneObservationHintKind::PromptLikeInput,
    );
    match kind {
        crate::context::TabWorkspaceKind::TmuxWorkspace => {
            if pane_has_tmux_native(pane) {
                crate::context::TabWorkspaceState::Ready
            } else {
                crate::context::TabWorkspaceState::NeedsInspection
            }
        }
        crate::context::TabWorkspaceKind::RemoteShell
        | crate::context::TabWorkspaceKind::LocalShell => {
            if pane_has_capability(pane, PaneControlCapability::ExecVisibleShell)
                || pane.remote_workspace.is_some()
                || prompt_like
            {
                crate::context::TabWorkspaceState::Ready
            } else {
                crate::context::TabWorkspaceState::NeedsInspection
            }
        }
        crate::context::TabWorkspaceKind::Unknown => crate::context::TabWorkspaceState::Unknown,
    }
}

fn workspace_note_for_pane(
    pane: &PaneInfo,
    kind: crate::context::TabWorkspaceKind,
    state: crate::context::TabWorkspaceState,
    host: Option<&str>,
) -> String {
    let cwd = crate::context::workspace_cwd_hint(pane.cwd.as_deref(), &pane.recent_actions);
    let agent_cli =
        crate::context::workspace_agent_cli_hint(pane.agent_cli.as_deref(), &pane.recent_actions);
    match (kind, state, host, cwd.as_deref(), agent_cli.as_deref()) {
        (_, crate::context::TabWorkspaceState::Disconnected, Some(host), _, _) => {
            format!("SSH workspace for `{host}` appears disconnected.")
        }
        (
            crate::context::TabWorkspaceKind::TmuxWorkspace,
            crate::context::TabWorkspaceState::Ready,
            Some(host),
            _,
            _,
        ) if pane
            .tmux_control
            .as_ref()
            .and_then(|tmux| tmux.session_name.as_deref())
            .or(pane.tmux_session.as_deref())
            .is_none() =>
        {
            format!("Remote tmux workspace on `{host}` looks ready.")
        }
        (
            crate::context::TabWorkspaceKind::TmuxWorkspace,
            crate::context::TabWorkspaceState::Ready,
            Some(host),
            _,
            _,
        ) => {
            let session = pane
                .tmux_control
                .as_ref()
                .and_then(|tmux| tmux.session_name.as_deref())
                .or(pane.tmux_session.as_deref())
                .unwrap_or("tmux");
            format!("Remote tmux workspace on `{host}` session `{session}` is ready.")
        }
        (crate::context::TabWorkspaceKind::TmuxWorkspace, _, Some(host), _, _)
            if pane
                .tmux_control
                .as_ref()
                .and_then(|tmux| tmux.session_name.as_deref())
                .or(pane.tmux_session.as_deref())
                .is_none() =>
        {
            format!("Remote tmux workspace on `{host}` is visible, but needs inspection.")
        }
        (crate::context::TabWorkspaceKind::TmuxWorkspace, _, Some(host), _, _) => {
            let session = pane
                .tmux_control
                .as_ref()
                .and_then(|tmux| tmux.session_name.as_deref())
                .or(pane.tmux_session.as_deref())
                .unwrap_or("tmux");
            format!(
                "Remote tmux workspace on `{host}` session `{session}` exists, but needs inspection."
            )
        }
        (
            crate::context::TabWorkspaceKind::RemoteShell,
            crate::context::TabWorkspaceState::Ready,
            Some(host),
            _,
            _,
        ) => {
            format!("Remote shell workspace on `{host}` looks ready.")
        }
        (crate::context::TabWorkspaceKind::RemoteShell, _, Some(host), _, _) => {
            format!("Remote shell workspace on `{host}` exists, but needs inspection.")
        }
        (
            crate::context::TabWorkspaceKind::LocalShell,
            crate::context::TabWorkspaceState::Ready,
            _,
            Some(cwd),
            Some(agent),
        ) => format!("Local {agent} workspace at `{cwd}` looks reusable."),
        (
            crate::context::TabWorkspaceKind::LocalShell,
            crate::context::TabWorkspaceState::Ready,
            _,
            Some(cwd),
            None,
        ) => format!("Local shell workspace at `{cwd}` looks ready."),
        (
            crate::context::TabWorkspaceKind::LocalShell,
            crate::context::TabWorkspaceState::Ready,
            _,
            None,
            Some(agent),
        ) => format!("Local {agent} workspace looks reusable."),
        (crate::context::TabWorkspaceKind::LocalShell, _, _, Some(cwd), Some(agent)) => {
            format!("Local {agent} workspace at `{cwd}` exists, but needs inspection.")
        }
        (crate::context::TabWorkspaceKind::LocalShell, _, _, Some(cwd), None) => {
            format!("Local shell workspace at `{cwd}` exists, but needs inspection.")
        }
        (crate::context::TabWorkspaceKind::LocalShell, _, _, None, Some(agent)) => {
            format!("Local {agent} workspace exists, but needs inspection.")
        }
        (crate::context::TabWorkspaceKind::LocalShell, _, _, None, None) => {
            "Local shell workspace exists, but needs inspection.".to_string()
        }
        _ => "Workspace state is not yet proven.".to_string(),
    }
}

fn tab_workspaces_from_panes(panes: &[PaneInfo]) -> Vec<crate::context::TabWorkspaceSummary> {
    panes
        .iter()
        .map(|pane| {
            let host = pane.hostname.as_deref().or_else(|| {
                pane.remote_workspace
                    .as_ref()
                    .map(|anchor| anchor.host.as_str())
            });
            let kind = workspace_kind_for_pane(pane);
            let state = workspace_state_for_pane(pane, kind);
            crate::context::TabWorkspaceSummary {
                pane_index: pane.index,
                pane_id: pane.pane_id,
                host: host.map(ToString::to_string),
                tmux_session: pane
                    .tmux_control
                    .as_ref()
                    .and_then(|tmux| tmux.session_name.clone())
                    .or_else(|| pane.tmux_session.clone()),
                cwd: crate::context::workspace_cwd_hint(pane.cwd.as_deref(), &pane.recent_actions),
                agent_cli: crate::context::workspace_agent_cli_hint(
                    pane.agent_cli.as_deref(),
                    &pane.recent_actions,
                ),
                kind,
                state,
                note: workspace_note_for_pane(pane, kind, state, host),
            }
        })
        .collect()
}

fn sort_work_target_candidates(candidates: &mut [(i32, WorkTargetCandidate)]) {
    candidates.sort_by(|(score_a, cand_a), (score_b, cand_b)| {
        score_b
            .cmp(score_a)
            .then_with(|| cand_a.pane_index.cmp(&cand_b.pane_index))
            .then_with(|| cand_a.pane_title.cmp(&cand_b.pane_title))
    });
}

fn make_local_shell_preparation_candidate(pane: &PaneInfo, reason: String) -> WorkTargetCandidate {
    WorkTargetCandidate {
        pane_index: pane.index,
        pane_id: pane.pane_id,
        pane_title: pane.title.clone(),
        host: pane.hostname.clone(),
        control_path: WorkTargetControlPath::LocalShellTarget,
        visible_target: pane.visible_target.clone(),
        tmux_mode: pane.tmux_control.as_ref().map(|tmux| tmux.mode),
        tmux_target: None,
        requires_preparation: true,
        suggested_tool: "ensure_local_shell_target".to_string(),
        reason,
    }
}

fn make_remote_shell_preparation_candidate(pane: &PaneInfo, reason: String) -> WorkTargetCandidate {
    WorkTargetCandidate {
        pane_index: pane.index,
        pane_id: pane.pane_id,
        pane_title: pane.title.clone(),
        host: pane_remote_host_hint(pane),
        control_path: WorkTargetControlPath::RemoteShellTarget,
        visible_target: pane.visible_target.clone(),
        tmux_mode: pane.tmux_control.as_ref().map(|tmux| tmux.mode),
        tmux_target: None,
        requires_preparation: true,
        suggested_tool: "ensure_remote_shell_target".to_string(),
        reason,
    }
}

fn make_local_agent_preparation_candidate(pane: &PaneInfo, reason: String) -> WorkTargetCandidate {
    WorkTargetCandidate {
        pane_index: pane.index,
        pane_id: pane.pane_id,
        pane_title: pane.title.clone(),
        host: pane.hostname.clone(),
        control_path: WorkTargetControlPath::LocalAgentTarget,
        visible_target: pane.visible_target.clone(),
        tmux_mode: pane.tmux_control.as_ref().map(|tmux| tmux.mode),
        tmux_target: None,
        requires_preparation: true,
        suggested_tool: "ensure_local_agent_target".to_string(),
        reason,
    }
}

fn make_visible_shell_candidate(pane: &PaneInfo, reason: String) -> WorkTargetCandidate {
    WorkTargetCandidate {
        pane_index: pane.index,
        pane_id: pane.pane_id,
        pane_title: pane.title.clone(),
        host: pane.hostname.clone(),
        control_path: WorkTargetControlPath::VisibleShellExec,
        visible_target: pane.visible_target.clone(),
        tmux_mode: pane.tmux_control.as_ref().map(|tmux| tmux.mode),
        tmux_target: None,
        requires_preparation: false,
        suggested_tool: "terminal_exec".to_string(),
        reason,
    }
}

fn make_tmux_workspace_candidate(
    pane: &PaneInfo,
    reason: String,
    suggested_tool: &str,
) -> WorkTargetCandidate {
    WorkTargetCandidate {
        pane_index: pane.index,
        pane_id: pane.pane_id,
        pane_title: pane.title.clone(),
        host: pane.hostname.clone(),
        control_path: WorkTargetControlPath::TmuxQuery,
        visible_target: pane.visible_target.clone(),
        tmux_mode: pane.tmux_control.as_ref().map(|tmux| tmux.mode),
        tmux_target: None,
        requires_preparation: false,
        suggested_tool: suggested_tool.to_string(),
        reason,
    }
}

fn make_tmux_target_candidate(
    pane: &PaneInfo,
    tmux_target: Option<crate::tmux::TmuxPaneInfo>,
    control_path: WorkTargetControlPath,
    requires_preparation: bool,
    suggested_tool: &str,
    reason: String,
) -> WorkTargetCandidate {
    WorkTargetCandidate {
        pane_index: pane.index,
        pane_id: pane.pane_id,
        pane_title: pane.title.clone(),
        host: pane.hostname.clone(),
        control_path,
        visible_target: pane.visible_target.clone(),
        tmux_mode: pane.tmux_control.as_ref().map(|tmux| tmux.mode),
        tmux_target,
        requires_preparation,
        suggested_tool: suggested_tool.to_string(),
        reason,
    }
}

fn pane_is_local_shell_candidate(pane: &PaneInfo) -> bool {
    pane_has_capability(pane, PaneControlCapability::ExecVisibleShell)
        && !pane_has_remote_shell_context(pane)
        && pane.remote_workspace.is_none()
        && !pane_has_tmux_layer(pane)
        && !pane_has_tmux_observation(pane)
        && !pane_is_disconnected_workspace(pane)
        && pane.agent_cli.is_none()
        && pane.visible_target.kind != PaneVisibleTargetKind::AgentCli
}

fn pane_is_visible_local_agent_cli(pane: &PaneInfo, agent_name: Option<&String>) -> bool {
    pane_is_visible_agent_cli(pane, agent_name)
        && !pane_has_remote_shell_context(pane)
        && pane.remote_workspace.is_none()
        && !pane_has_tmux_layer(pane)
        && !pane_has_tmux_observation(pane)
}

fn resolve_work_target_candidates(
    pane_tx: &Sender<PaneRequest>,
    panes: Vec<PaneInfo>,
    intent: WorkTargetIntent,
    preferred_pane_index: Option<usize>,
    preferred_pane_id: Option<usize>,
    host_contains: Option<String>,
    cwd_contains: Option<String>,
    agent_name: Option<String>,
    limit: usize,
) -> Result<ResolveWorkTargetResult, ToolError> {
    let host_contains = normalize_lower(&host_contains);
    let cwd_contains = normalize_cwd_lower(&cwd_contains);
    let agent_name = normalize_lower(&agent_name);
    let mut candidates = Vec::new();

    for pane in panes {
        if let Some(preferred_index) = preferred_pane_index {
            if pane.index != preferred_index {
                continue;
            }
        }
        if let Some(preferred_id) = preferred_pane_id {
            if pane.pane_id != preferred_id {
                continue;
            }
        }
        if !pane_matches_host(&pane, host_contains.as_ref()) {
            continue;
        }

        match intent {
            WorkTargetIntent::VisibleShell => {
                if pane_is_local_shell_candidate(&pane)
                    && pane_matches_cwd(&pane, cwd_contains.as_ref())
                {
                    let mut score = 100;
                    if pane.is_focused {
                        score += 5;
                    }
                    if pane_has_remote_shell_context(&pane) {
                        score += 10;
                    }
                    let reason = if pane_has_remote_shell_context(&pane) {
                        "This pane exposes visible shell execution and has proven remote shell context."
                            .to_string()
                    } else {
                        "This pane exposes visible shell execution on the current shell prompt."
                            .to_string()
                    };
                    candidates.push((score, make_visible_shell_candidate(&pane, reason)));
                } else if pane_is_visible_local_agent_cli(&pane, agent_name.as_ref()) {
                    let mut score = 85;
                    if pane.is_focused {
                        score += 5;
                    }
                    let reason = if let Some(agent_name) =
                        agent_name.as_deref().and_then(canonical_agent_cli_name)
                    {
                        format!(
                            "The visible target is {agent_name}. Prepare a separate local shell target for file edits, tests, and other shell work instead of typing shell commands into the agent UI."
                        )
                    } else {
                        "The visible target is a local agent CLI. Prepare a separate local shell target for file edits, tests, and other shell work instead of typing shell commands into the agent UI."
                            .to_string()
                    };
                    candidates.push((score, make_local_shell_preparation_candidate(&pane, reason)));
                } else if pane_has_any_local_agent_continuity(&pane) {
                    let mut score = 80;
                    if pane.is_focused {
                        score += 5;
                    }
                    let reason = if let Some(agent_name) =
                        agent_name.as_deref().and_then(canonical_agent_cli_name)
                    {
                        format!(
                            "This pane was recently launched for local {agent_name} work. Prepare or reuse a separate local shell target instead of routing shell commands back into the interactive agent pane."
                        )
                    } else {
                        "This pane was recently launched for local agent-CLI work. Prepare or reuse a separate local shell target instead of routing shell commands back into the interactive agent pane.".to_string()
                    };
                    candidates.push((score, make_local_shell_preparation_candidate(&pane, reason)));
                }
            }
            WorkTargetIntent::RemoteShell => {
                if !pane_matches_cwd(&pane, cwd_contains.as_ref()) {
                    continue;
                }
                if pane_has_tmux_observation(&pane) {
                    continue;
                }
                let has_remote_fact = pane_has_remote_shell_context(&pane);
                let has_remote_anchor = pane_matches_remote_anchor(&pane, host_contains.as_ref());
                if pane_is_disconnected_workspace(&pane) {
                    if let Some(host) = pane_remote_host_hint(&pane).filter(|host| {
                        host_contains
                            .as_ref()
                            .is_none_or(|needle| host.to_ascii_lowercase().contains(needle))
                    }) {
                        candidates.push((
                            85 + if pane.is_focused { 5 } else { 0 },
                            make_remote_shell_preparation_candidate(
                                &pane,
                                format!(
                                    "The SSH workspace for `{host}` appears disconnected. Recover only this host with ensure_remote_shell_target instead of recreating healthy remote panes."
                                ),
                            ),
                        ));
                    }
                    continue;
                }
                if !has_remote_fact && !has_remote_anchor {
                    continue;
                }
                let can_exec = pane_has_capability(&pane, PaneControlCapability::ExecVisibleShell)
                    || pane.remote_workspace.is_some();
                if !can_exec {
                    continue;
                }
                let mut score =
                    if pane_has_capability(&pane, PaneControlCapability::ExecVisibleShell) {
                        if has_remote_fact { 120 } else { 110 }
                    } else if has_remote_fact {
                        105
                    } else {
                        95
                    };
                if pane.is_focused {
                    score += 5;
                }
                if pane.hostname.is_some() {
                    score += 10;
                }
                candidates.push((
                    score,
                    make_visible_shell_candidate(
                        &pane,
                        if has_remote_fact {
                            "This pane is a proven remote shell target with visible shell execution."
                                .to_string()
                        } else if let Some(anchor) = &pane.remote_workspace {
                            format!(
                                "This pane is a reusable remote SSH workspace for `{}` via {} continuity, even though fresh shell integration is not currently proven.",
                                anchor.host,
                                anchor.source.as_str()
                            )
                        } else {
                            let target = pane_recent_ssh_target(&pane)
                                .unwrap_or_else(|| "unknown".to_string());
                            format!(
                                "This pane has visible shell execution and recent con action history showing SSH startup for `{target}`."
                            )
                        },
                    ),
                ));
            }
            WorkTargetIntent::TmuxWorkspace => {
                if !pane_has_tmux_layer(&pane) {
                    continue;
                }
                let (score, suggested_tool, reason) = if pane_has_tmux_native(&pane) {
                    (
                        140 + if pane.is_focused { 1 } else { 0 },
                        "tmux_list_targets",
                        "This pane has a native tmux control anchor, so tmux targets can be queried directly.",
                    )
                } else if pane_has_tmux_observation(&pane) {
                    (
                        75 + if pane.is_focused { 1 } else { 0 },
                        "read_pane",
                        "This pane currently looks tmux-like on screen, but a native tmux control anchor is not established yet.",
                    )
                } else {
                    (
                        80 + if pane.is_focused { 1 } else { 0 },
                        "probe_shell_context",
                        "This pane has a tmux layer, but native tmux control is not established yet.",
                    )
                };
                candidates.push((
                    score,
                    make_tmux_workspace_candidate(&pane, reason.to_string(), suggested_tool),
                ));
            }
            WorkTargetIntent::TmuxShell => {
                if !pane_has_tmux_layer(&pane) {
                    continue;
                }
                if pane_has_tmux_native(&pane) {
                    let snapshot = pane_query_tmux_list(
                        pane_tx,
                        selector_from(Some(pane.index), Some(pane.pane_id)),
                    )?;
                    let mut matches = snapshot
                        .panes
                        .into_iter()
                        .filter(|tmux_pane| {
                            tmux_pane
                                .pane_current_command
                                .as_deref()
                                .is_some_and(is_tmux_shell_command)
                                && cwd_contains.as_ref().is_none_or(|needle| {
                                    tmux_pane
                                        .pane_current_path
                                        .as_deref()
                                        .unwrap_or_default()
                                        .to_ascii_lowercase()
                                        .contains(needle)
                                })
                        })
                        .collect::<Vec<_>>();
                    sort_tmux_matches(&mut matches);
                    if let Some(target) = matches.into_iter().next() {
                        candidates.push((
                            160 + if pane.is_focused { 1 } else { 0 },
                            make_tmux_target_candidate(
                                &pane,
                                Some(target),
                                WorkTargetControlPath::TmuxShellTarget,
                                false,
                                "tmux_send_keys",
                                "This pane has native tmux control and an existing tmux shell target that matches the request.".to_string(),
                            ),
                        ));
                    } else {
                        candidates.push((
                            150 + if pane.is_focused { 1 } else { 0 },
                            make_tmux_target_candidate(
                                &pane,
                                None,
                                WorkTargetControlPath::TmuxShellTarget,
                                true,
                                "tmux_ensure_shell_target",
                                "This pane has native tmux control, but no matching shell target is currently known. Use tmux_ensure_shell_target first.".to_string(),
                            ),
                        ));
                    }
                } else {
                    candidates.push((
                        70 + if pane.is_focused { 1 } else { 0 },
                        make_tmux_workspace_candidate(
                            &pane,
                            "This pane appears to be a tmux workspace, but a native tmux control anchor is not established yet.".to_string(),
                            "probe_shell_context",
                        ),
                    ));
                }
            }
            WorkTargetIntent::AgentCli => {
                if pane_is_visible_agent_cli(&pane, agent_name.as_ref()) {
                    candidates.push((
                        170 + if pane.is_focused { 1 } else { 0 },
                        WorkTargetCandidate {
                            pane_index: pane.index,
                            pane_id: pane.pane_id,
                            pane_title: pane.title.clone(),
                            host: pane.hostname.clone(),
                            control_path: WorkTargetControlPath::VisibleAgentUi,
                            visible_target: pane.visible_target.clone(),
                            tmux_mode: pane.tmux_control.as_ref().map(|tmux| tmux.mode),
                            tmux_target: None,
                            requires_preparation: false,
                            suggested_tool: "agent_cli_turn".to_string(),
                            reason: "The visible foreground target already appears to be the requested agent CLI, so continue it with a typed agent_cli_turn instead of raw keystrokes.".to_string(),
                        },
                    ));
                    continue;
                }
                if pane_has_tmux_native(&pane) {
                    let snapshot = pane_query_tmux_list(
                        pane_tx,
                        selector_from(Some(pane.index), Some(pane.pane_id)),
                    )?;
                    let mut matches = snapshot
                        .panes
                        .into_iter()
                        .filter(|tmux_pane| {
                            tmux_pane
                                .pane_current_command
                                .as_deref()
                                .is_some_and(|command| {
                                    agent_cli_matches_selector(command, agent_name.as_ref())
                                })
                        })
                        .collect::<Vec<_>>();
                    sort_tmux_matches(&mut matches);
                    if let Some(target) = matches.into_iter().next() {
                        candidates.push((
                            165 + if pane.is_focused { 1 } else { 0 },
                            make_tmux_target_candidate(
                                &pane,
                                Some(target),
                                WorkTargetControlPath::TmuxAgentTarget,
                                false,
                                "agent_cli_turn",
                                "This pane has native tmux control and already contains a matching agent CLI target, so use a typed agent_cli_turn instead of raw tmux keystrokes.".to_string(),
                            ),
                        ));
                    } else {
                        let label = agent_name
                            .as_deref()
                            .and_then(canonical_agent_cli_name)
                            .unwrap_or("agent CLI");
                        candidates.push((
                            155 + if pane.is_focused { 1 } else { 0 },
                            make_tmux_target_candidate(
                                &pane,
                                None,
                                WorkTargetControlPath::TmuxAgentTarget,
                                true,
                                "tmux_ensure_agent_target",
                                format!(
                                    "This pane has native tmux control, but no matching {label} target is currently known. Use tmux_ensure_agent_target first."
                                ),
                            ),
                        ));
                    }
                } else if pane_has_tmux_layer(&pane) {
                    let suggested_tool = if pane_has_tmux_observation(&pane) {
                        "read_pane"
                    } else {
                        "probe_shell_context"
                    };
                    let reason = if pane_has_tmux_observation(&pane) {
                        "This pane currently looks like a tmux workspace for agent-CLI work, but native tmux control is not established yet.".to_string()
                    } else {
                        "This pane has a tmux layer for agent-CLI work, but native tmux control is not established yet.".to_string()
                    };
                    candidates.push((
                        65 + if pane.is_focused { 1 } else { 0 },
                        make_tmux_workspace_candidate(&pane, reason, suggested_tool),
                    ));
                } else if agent_name.as_deref().is_some_and(|name| {
                    pane_has_local_agent_continuity(&pane, name, cwd_contains.as_ref())
                }) {
                    let label = agent_name
                        .as_deref()
                        .and_then(canonical_agent_cli_name)
                        .unwrap_or("agent CLI");
                    candidates.push((
                        150 + if pane.is_focused { 1 } else { 0 },
                        make_local_agent_preparation_candidate(
                            &pane,
                            format!(
                                "This pane was previously used for local {label} work, but a live interactive {label} target is not currently proven. Re-prepare the dedicated local agent target instead of sending a turn into a shell-like pane."
                            ),
                        ),
                    ));
                } else if pane_is_local_shell_candidate(&pane) {
                    let label = agent_name
                        .as_deref()
                        .and_then(canonical_agent_cli_name)
                        .unwrap_or("agent CLI");
                    candidates.push((
                        90 + if pane.is_focused { 1 } else { 0 },
                        make_local_agent_preparation_candidate(
                            &pane,
                            format!(
                                "This local shell pane is the right place to launch or reuse {label}. Prepare a dedicated local agent target instead of mixing CLI interaction into the shell."
                            ),
                        ),
                    ));
                }
            }
        }
    }

    sort_work_target_candidates(&mut candidates);
    let mut candidates = candidates
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect::<Vec<_>>();
    candidates.truncate(limit.max(1));

    Ok(ResolveWorkTargetResult {
        intent,
        best_match: candidates.first().cloned(),
        candidates,
    })
}

// ── list_panes tool ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ListPanesArgs {}

pub struct ListPanesTool {
    pane_tx: Sender<PaneRequest>,
}

impl ListPanesTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for ListPanesTool {
    const NAME: &'static str = "list_panes";
    type Error = ToolError;
    type Args = ListPanesArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "List all terminal panes currently open. Returns each pane's index, title, working directory, dimensions, current verified front-state, current runtime_stack, last_verified_runtime_stack, backend-support flags, typed shell context, recent con actions, current visible-screen observation hints, and control state: address space, visible target, nested target_stack, explicit control_attachments, control channels, control capabilities, and notes. Use this before acting in tmux/TUI panes so you do not confuse a con pane with a tmux pane or over-trust stale shell metadata. If a pane exposes query_tmux, exec_tmux_command, or send_tmux_keys, prefer tmux-native tools over outer-pane send_keys.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        log::info!("[list_panes] Tool called, sending PaneQuery::List");
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::List,
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;
        log::info!("[list_panes] PaneRequest sent, waiting for response...");

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|e| {
                    log::error!("[list_panes] Timed out waiting for response: {}", e);
                    ToolError::CommandFailed("Pane query timed out".into())
                })
        })?;
        log::info!("[list_panes] Got response");

        match response {
            PaneResponse::PaneList(panes) => {
                serde_json::to_value(&panes).map_err(|e| ToolError::CommandFailed(e.to_string()))
            }
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

// ── list_tab_workspaces tool ───────────────────────────────────────

#[derive(Deserialize)]
pub struct ListTabWorkspacesArgs {}

pub struct ListTabWorkspacesTool {
    pane_tx: Sender<PaneRequest>,
}

impl ListTabWorkspacesTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for ListTabWorkspacesTool {
    const NAME: &'static str = "list_tab_workspaces";
    type Error = ToolError;
    type Args = ListTabWorkspacesArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Summarize the current tab as typed workspaces. Returns stable pane ids, current pane indices, hosts, tmux sessions, workspace kinds, and lifecycle states such as ready, needs_inspection, disconnected, or interactive. Use this for questions like 'what panes do you have?', 'which pane is the real remote shell?', or 'which SSH pane got disconnected?'.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let panes = pane_query_list(&self.pane_tx)?;
        let workspaces = tab_workspaces_from_panes(&panes);
        serde_json::to_value(&workspaces).map_err(|e| ToolError::CommandFailed(e.to_string()))
    }
}

// ── tmux_inspect tool ──────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TmuxInspectArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
}

pub struct TmuxInspectTool {
    pane_tx: Sender<PaneRequest>,
}

impl TmuxInspectTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for TmuxInspectTool {
    const NAME: &'static str = "tmux_inspect";
    type Error = ToolError;
    type Args = TmuxInspectArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Inspect the tmux adapter state for a specific con pane. Returns tmux adapter details only when tmux has been authoritatively detected for that pane, including the explicit reason native tmux pane/window control is or is not available. Use this when a pane's target_stack includes tmux.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target con pane index from list_panes. Positional only."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up work."
                }
            },
            "required": []
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let target = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::InspectTmux { target },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|_| ToolError::CommandFailed("Pane query timed out".into()))
        })?;

        match response {
            PaneResponse::TmuxInfo(info) => {
                serde_json::to_value(&info).map_err(|e| ToolError::CommandFailed(e.to_string()))
            }
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

// ── tmux_list tool ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TmuxListArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
}

pub struct TmuxListTool {
    pane_tx: Sender<PaneRequest>,
}

impl TmuxListTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for TmuxListTool {
    const NAME: &'static str = "tmux_list_targets";
    type Error = ToolError;
    type Args = TmuxListArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "List tmux panes across the current tmux session using a proven same-session tmux control anchor from the given con pane. This is the preferred first step whenever list_panes shows tmux native control. Returns tmux session/window/pane ids plus pane_current_command and pane_current_path so the agent can target Codex CLI, Claude Code, OpenCode, or shell panes inside tmux explicitly.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target con pane index from list_panes. Positional only; it can change."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up tmux work."
                }
            },
            "required": []
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let target = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::TmuxList { pane: target },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(12))
                .map_err(|_| ToolError::CommandFailed("tmux list timed out".into()))
        })?;

        match response {
            PaneResponse::TmuxList(snapshot) => {
                serde_json::to_value(&snapshot).map_err(|e| ToolError::CommandFailed(e.to_string()))
            }
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

// ── tmux_capture_pane tool ────────────────────────────────────────

#[derive(Deserialize)]
pub struct TmuxCaptureArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub target: Option<String>,
    #[serde(default = "default_tmux_capture_lines")]
    pub lines: usize,
}

fn default_tmux_capture_lines() -> usize {
    120
}

pub struct TmuxCaptureTool {
    pane_tx: Sender<PaneRequest>,
}

impl TmuxCaptureTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for TmuxCaptureTool {
    const NAME: &'static str = "tmux_capture_pane";
    type Error = ToolError;
    type Args = TmuxCaptureArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Capture scrollback from a tmux pane through a proven same-session tmux control anchor. This is the correct way to inspect a tmux pane's content without confusing it with the outer con pane. If target is omitted, captures the current tmux pane of the anchor shell.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target con pane index from list_panes. Positional only; it can change."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up tmux work."
                },
                "target": {
                    "type": ["string", "null"],
                    "description": "Optional tmux target such as %17, @3, or session:window.pane. When omitted, captures the current tmux pane of the anchor shell."
                },
                "lines": {
                    "type": "integer",
                    "description": "Number of recent lines to capture from the tmux pane (default: 120)."
                }
            },
            "required": []
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let pane = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::TmuxCapture {
                    pane,
                    target: args.target,
                    lines: args.lines,
                },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(12))
                .map_err(|_| ToolError::CommandFailed("tmux capture timed out".into()))
        })?;

        match response {
            PaneResponse::TmuxCapture(capture) => {
                serde_json::to_value(&capture).map_err(|e| ToolError::CommandFailed(e.to_string()))
            }
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

// ── tmux_send_keys tool ───────────────────────────────────────────

#[derive(Deserialize)]
pub struct TmuxSendKeysArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub target: String,
    pub literal_text: Option<String>,
    #[serde(default)]
    pub key_names: Vec<String>,
    #[serde(default)]
    pub append_enter: bool,
}

pub struct TmuxSendKeysTool {
    pane_tx: Sender<PaneRequest>,
}

impl TmuxSendKeysTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for TmuxSendKeysTool {
    const NAME: &'static str = "tmux_send_keys";
    type Error = ToolError;
    type Args = TmuxSendKeysArgs;
    type Output = String;

    fn description(&self) -> String {
        "Send text or tmux key names to a specific tmux pane through a proven same-session tmux control anchor. This is the preferred interaction path for Codex CLI, Claude Code, OpenCode, or shell panes inside tmux. Use this instead of raw outer-pane input whenever tmux native control is available. Provide literal_text for typed text, key_names for tmux key tokens like Enter, Escape, C-c, Up, Down, or both.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target con pane index from list_panes. Positional only; it can change."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up tmux work."
                },
                "target": {
                    "type": "string",
                    "description": "tmux target such as %17, @3, or session:window.pane."
                },
                "literal_text": {
                    "type": ["string", "null"],
                    "description": "Optional literal text to send into the tmux target."
                },
                "key_names": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tmux key names such as Enter, Escape, C-c, Up, Down."
                },
                "append_enter": {
                    "type": "boolean",
                    "description": "Whether to send Enter after literal_text and key_names. Defaults to false."
                }
            },
            "required": ["target"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let pane = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::TmuxSendKeys {
                    pane,
                    target: args.target,
                    literal_text: args.literal_text,
                    key_names: args.key_names,
                    append_enter: args.append_enter,
                },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(12))
                .map_err(|_| ToolError::CommandFailed("tmux send-keys timed out".into()))
        })?;

        match response {
            PaneResponse::Content(content) => Ok(content),
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

// ── tmux_run_command tool ─────────────────────────────────────────

#[derive(Deserialize)]
pub struct TmuxRunCommandArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub location: TmuxExecLocation,
    pub command: String,
    pub target: Option<String>,
    pub window_name: Option<String>,
    pub cwd: Option<String>,
    #[serde(default = "default_true")]
    pub detached: bool,
}

fn default_true() -> bool {
    true
}

pub struct TmuxRunCommandTool {
    pane_tx: Sender<PaneRequest>,
}

impl TmuxRunCommandTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for TmuxRunCommandTool {
    const NAME: &'static str = "tmux_run_command";
    type Error = ToolError;
    type Args = TmuxRunCommandArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Run a command through tmux itself by creating a new tmux window or split pane from a proven same-session tmux control anchor. This is the preferred way to launch a fresh shell, Codex CLI, Claude Code, OpenCode, or any long-running command inside tmux without typing through the currently visible app.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target con pane index from list_panes. Positional only; it can change."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up tmux work."
                },
                "location": {
                    "type": "string",
                    "enum": ["new_window", "split_horizontal", "split_vertical"],
                    "description": "Where to create the tmux target."
                },
                "command": {
                    "type": "string",
                    "description": "Command to run inside the new tmux target."
                },
                "target": {
                    "type": ["string", "null"],
                    "description": "Optional tmux target for placement, such as a session for new_window or an existing pane/window for split operations."
                },
                "window_name": {
                    "type": ["string", "null"],
                    "description": "Optional tmux window name. Only used for new_window."
                },
                "cwd": {
                    "type": ["string", "null"],
                    "description": "Optional working directory for the new tmux target."
                },
                "detached": {
                    "type": "boolean",
                    "description": "Whether the new tmux target should start detached. Defaults to true."
                }
            },
            "required": ["location", "command"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let pane = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::TmuxRunCommand {
                    pane,
                    target: args.target,
                    location: args.location,
                    command: args.command,
                    window_name: args.window_name,
                    cwd: args.cwd,
                    detached: args.detached,
                },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(12))
                .map_err(|_| ToolError::CommandFailed("tmux run-command timed out".into()))
        })?;

        match response {
            PaneResponse::TmuxExec(exec) => {
                serde_json::to_value(&exec).map_err(|e| ToolError::CommandFailed(e.to_string()))
            }
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

// ── tmux_find_targets tool ────────────────────────────────────────

#[derive(Deserialize)]
pub struct TmuxFindTargetsArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    #[serde(default)]
    pub kind: Option<TmuxTargetKindFilter>,
    pub command_contains: Option<String>,
    pub path_contains: Option<String>,
    #[serde(default = "default_tmux_find_limit")]
    pub limit: usize,
}

fn default_tmux_find_limit() -> usize {
    8
}

pub struct TmuxFindTargetsTool {
    pane_tx: Sender<PaneRequest>,
}

impl TmuxFindTargetsTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for TmuxFindTargetsTool {
    const NAME: &'static str = "tmux_find_targets";
    type Error = ToolError;
    type Args = TmuxFindTargetsArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Find useful tmux pane targets through a proven same-session tmux control anchor. Use this helper instead of hand-filtering tmux_list_targets when you need a shell pane, an agent CLI pane, or a command/path match inside tmux.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target con pane index from list_panes. Positional only; it can change."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up tmux work."
                },
                "kind": {
                    "type": ["string", "null"],
                    "enum": ["any", "shell", "agent_cli", null],
                    "description": "Optional target class filter."
                },
                "command_contains": {
                    "type": ["string", "null"],
                    "description": "Optional case-insensitive substring match against tmux pane_current_command."
                },
                "path_contains": {
                    "type": ["string", "null"],
                    "description": "Optional case-insensitive substring match against tmux pane_current_path."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of matches to return. Defaults to 8."
                }
            },
            "required": []
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let pane = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let snapshot = pane_query_tmux_list(&self.pane_tx, pane)?;
        let kind = args.kind.unwrap_or(TmuxTargetKindFilter::Any);
        let command_contains = args
            .command_contains
            .as_ref()
            .map(|v| v.to_ascii_lowercase());
        let path_contains = args.path_contains.as_ref().map(|v| v.to_ascii_lowercase());

        let mut matches = snapshot
            .panes
            .into_iter()
            .filter(|pane| {
                let command = pane
                    .pane_current_command
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let path = pane
                    .pane_current_path
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase();

                let kind_match = match kind {
                    TmuxTargetKindFilter::Any => true,
                    TmuxTargetKindFilter::Shell => pane
                        .pane_current_command
                        .as_deref()
                        .is_some_and(is_tmux_shell_command),
                    TmuxTargetKindFilter::AgentCli => pane
                        .pane_current_command
                        .as_deref()
                        .is_some_and(is_tmux_agent_cli_command),
                };

                let command_match = command_contains
                    .as_ref()
                    .is_none_or(|needle| command.contains(needle));
                let path_match = path_contains
                    .as_ref()
                    .is_none_or(|needle| path.contains(needle));

                kind_match && command_match && path_match
            })
            .collect::<Vec<_>>();

        sort_tmux_matches(&mut matches);
        matches.truncate(args.limit.max(1));

        let result = TmuxFindTargetsResult {
            kind,
            best_match: matches.first().cloned(),
            matches,
        };
        serde_json::to_value(&result).map_err(|e| ToolError::CommandFailed(e.to_string()))
    }
}

// ── tmux_ensure_shell_target tool ─────────────────────────────────

#[derive(Deserialize)]
pub struct TmuxEnsureShellTargetArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub session_name: Option<String>,
    pub cwd: Option<String>,
    pub window_name: Option<String>,
    pub shell_command: Option<String>,
    #[serde(default = "default_tmux_shell_location")]
    pub location: TmuxExecLocation,
    #[serde(default = "default_true")]
    pub detached: bool,
}

fn default_tmux_shell_location() -> TmuxExecLocation {
    TmuxExecLocation::NewWindow
}

pub struct TmuxEnsureShellTargetTool {
    pane_tx: Sender<PaneRequest>,
}

impl TmuxEnsureShellTargetTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for TmuxEnsureShellTargetTool {
    const NAME: &'static str = "tmux_ensure_shell_target";
    type Error = ToolError;
    type Args = TmuxEnsureShellTargetArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Return a tmux pane that is suitable for shell work. Reuses an existing shell pane when one already matches, otherwise creates a fresh tmux shell target through the native tmux control channel. Use this before writing files remotely, launching commands away from a visible TUI, or preparing a clean shell workspace in tmux.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target con pane index from list_panes. Positional only; it can change."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up tmux work."
                },
                "session_name": {
                    "type": ["string", "null"],
                    "description": "Optional tmux session to constrain shell-target reuse. Defaults to the pane's active tmux workspace session when con already knows it."
                },
                "cwd": {
                    "type": ["string", "null"],
                    "description": "Optional path hint. Existing tmux shell panes whose pane_current_path contains this value are preferred, and new shell targets inherit it when created."
                },
                "window_name": {
                    "type": ["string", "null"],
                    "description": "Optional name for a newly created tmux window."
                },
                "shell_command": {
                    "type": ["string", "null"],
                    "description": "Optional command for a newly created shell target. Defaults to an interactive login shell based on $SHELL."
                },
                "location": {
                    "type": "string",
                    "enum": ["new_window", "split_horizontal", "split_vertical"],
                    "description": "Where to create a new shell target if none already exists."
                },
                "detached": {
                    "type": "boolean",
                    "description": "Whether a newly created target should start detached. Defaults to true."
                }
            },
            "required": []
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let pane = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let panes = pane_query_list(&self.pane_tx)?;
        let requested_session =
            preferred_tmux_shell_session(&panes, pane, args.session_name.as_deref());
        let snapshot = pane_query_tmux_list(&self.pane_tx, pane)?;
        let cwd_contains = args.cwd.as_ref().map(|v| v.to_ascii_lowercase());

        let creation_target = requested_session
            .as_ref()
            .and_then(|session| tmux_creation_target(&snapshot, session, args.location));
        let mut shell_matches = snapshot
            .panes
            .iter()
            .filter(|pane| {
                requested_session
                    .as_ref()
                    .is_none_or(|session| pane.session_name == *session)
                    && pane
                        .pane_current_command
                        .as_deref()
                        .is_some_and(is_tmux_shell_command)
                    && cwd_contains.as_ref().is_none_or(|needle| {
                        pane.pane_current_path
                            .as_deref()
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                            .contains(needle)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        sort_tmux_matches(&mut shell_matches);

        if let Some(pane) = shell_matches.into_iter().next() {
            let result = TmuxEnsureShellTargetResult {
                created: false,
                pane,
                creation: None,
            };
            return serde_json::to_value(&result)
                .map_err(|e| ToolError::CommandFailed(e.to_string()));
        }

        let shell_command = args
            .shell_command
            .unwrap_or_else(|| "exec \"${SHELL:-/bin/sh}\" -il".to_string());
        let creation = pane_query_tmux_run_command(
            &self.pane_tx,
            pane,
            creation_target,
            args.location,
            shell_command,
            args.window_name,
            args.cwd.clone(),
            args.detached,
        )?;

        let snapshot = pane_query_tmux_list(&self.pane_tx, pane)?;
        let pane = snapshot
            .panes
            .into_iter()
            .find(|pane| pane.target == creation.target || pane.pane_id == creation.pane_id)
            .unwrap_or_else(|| crate::tmux::TmuxPaneInfo {
                session_name: creation.session_name.clone(),
                window_id: creation.window_id.clone(),
                window_index: creation.window_index.clone(),
                window_name: creation.window_name.clone(),
                window_target: creation.window_target.clone(),
                pane_id: creation.pane_id.clone(),
                pane_index: creation.pane_index.clone(),
                target: creation.target.clone(),
                pane_active: !creation.detached,
                window_active: !creation.detached,
                pane_current_command: None,
                pane_current_path: args.cwd,
            });

        let result = TmuxEnsureShellTargetResult {
            created: true,
            pane,
            creation: Some(creation),
        };
        serde_json::to_value(&result).map_err(|e| ToolError::CommandFailed(e.to_string()))
    }
}

// ── ensure_local_agent_target tool ───────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalAgentTargetMatchSource {
    ReusedAgent,
    Created,
}

#[derive(Serialize)]
pub struct EnsureLocalAgentTargetResult {
    pub agent_name: String,
    pub created: bool,
    pub pane_index: usize,
    pub pane_id: Option<usize>,
    pub cwd: Option<String>,
    pub match_source: LocalAgentTargetMatchSource,
    pub launch_command: String,
    pub output: String,
    pub reason: String,
    pub native_attachment_state: AgentCliNativeAttachmentState,
    pub native_attachment_note: String,
}

#[derive(Deserialize)]
pub struct EnsureLocalAgentTargetArgs {
    pub agent_name: String,
    pub cwd: Option<String>,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub launch_command: Option<String>,
    #[serde(default)]
    pub create_cwd_if_missing: bool,
    #[serde(default = "default_pane_create_location")]
    pub location: PaneCreateLocation,
}

pub struct EnsureLocalAgentTargetTool {
    pane_tx: Sender<PaneRequest>,
}

impl EnsureLocalAgentTargetTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

fn ensure_local_agent_target_impl(
    pane_tx: &Sender<PaneRequest>,
    args: EnsureLocalAgentTargetArgs,
) -> Result<EnsureLocalAgentTargetResult, ToolError> {
    let Some(agent_name) = canonical_agent_cli_name(&args.agent_name) else {
        return Err(ToolError::CommandFailed(format!(
            "Unsupported agent_name `{}`. Expected codex, claude, or opencode.",
            args.agent_name
        )));
    };
    let cwd_lower = normalize_cwd_lower(&args.cwd);
    let panes = pane_query_list(pane_tx)?;
    let (native_attachment_state, native_attachment_note) = agent_cli_native_attachment(agent_name);

    let best_visible = panes
        .iter()
        .filter(|pane| args.pane_index.is_none_or(|idx| pane.index == idx))
        .filter(|pane| args.pane_id.is_none_or(|id| pane.pane_id == id))
        .filter(|pane| pane_is_visible_local_agent_cli(pane, Some(&agent_name.to_string())))
        .filter(|pane| pane_matches_cwd(pane, cwd_lower.as_ref()))
        .max_by_key(|pane| 10 + if pane.is_focused { 5 } else { 0 });
    let continuity_hint = panes
        .iter()
        .filter(|pane| args.pane_index.is_none_or(|idx| pane.index == idx))
        .filter(|pane| args.pane_id.is_none_or(|id| pane.pane_id == id))
        .filter(|pane| pane_has_local_agent_continuity(pane, agent_name, cwd_lower.as_ref()))
        .filter(|pane| pane_matches_cwd(pane, cwd_lower.as_ref()))
        .max_by_key(|pane| 10 + if pane.is_focused { 5 } else { 0 });

    let launch_command = args.launch_command.clone().unwrap_or_else(|| {
        default_agent_launch_command(agent_name)
            .unwrap_or(agent_name)
            .to_string()
    });

    if let Some(pane) = best_visible {
        return Ok(EnsureLocalAgentTargetResult {
            agent_name: agent_name.to_string(),
            created: false,
            pane_index: pane.index,
            pane_id: Some(pane.pane_id),
            cwd: args.cwd,
            match_source: LocalAgentTargetMatchSource::ReusedAgent,
            launch_command,
            output: String::new(),
            reason: format!(
                "Pane {} already exposes a matching local {} target.",
                pane.index, agent_name
            ),
            native_attachment_state,
            native_attachment_note,
        });
    }

    let command = if let Some(cwd) = args.cwd.as_ref() {
        format!(
            "{} && {}",
            build_local_cwd_command_prefix(cwd, args.create_cwd_if_missing),
            launch_command
        )
    } else {
        launch_command.clone()
    };
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::CreatePane {
                command: Some(command.clone()),
                location: args.location,
            },
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    let response = tokio::task::block_in_place(|| {
        response_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| ToolError::CommandFailed("Create pane request timed out".into()))
    })?;

    let (pane_index, pane_id) = match response {
        PaneResponse::PaneCreated {
            pane_index,
            pane_id,
            ..
        } => (pane_index, pane_id),
        PaneResponse::Error(e) => return Err(ToolError::CommandFailed(e)),
        _ => return Err(ToolError::CommandFailed("Unexpected response".into())),
    };

    Ok(EnsureLocalAgentTargetResult {
        agent_name: agent_name.to_string(),
        created: true,
        pane_index,
        pane_id: Some(pane_id),
        cwd: args.cwd.clone(),
        match_source: LocalAgentTargetMatchSource::Created,
        launch_command,
        output: String::new(),
        reason: if let Some(pane) = continuity_hint {
            let cwd_note = args
                .cwd
                .as_ref()
                .map(|cwd| format!(" at `{cwd}`"))
                .unwrap_or_default();
            format!(
                "Pane {} was previously used for local {agent_name} work{}, but no live interactive {agent_name} target is currently proven there, so con created a fresh dedicated local agent pane.",
                pane.index, cwd_note
            )
        } else if let Some(cwd) = args.cwd {
            format!(
                "No reusable local {agent_name} target matched `{cwd}`, so con created a new local agent pane there."
            )
        } else {
            format!(
                "No reusable local {agent_name} target existed, so con created a new local agent pane."
            )
        },
        native_attachment_state,
        native_attachment_note,
    })
}

impl Tool for EnsureLocalAgentTargetTool {
    const NAME: &'static str = "ensure_local_agent_target";
    type Error = ToolError;
    type Args = EnsureLocalAgentTargetArgs;
    type Output = EnsureLocalAgentTargetResult;

    fn description(&self) -> String {
        "Reuse an existing LOCAL Codex, Claude Code, or OpenCode pane when one already matches, or create a fresh local agent-cli pane when needed. Pair this with ensure_local_shell_target so local coding workflows keep interactive agent UI and shell/file/test work in separate panes.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "Agent CLI family to reuse or launch, such as codex, claude, or opencode."
                },
                "cwd": {
                    "type": ["string", "null"],
                    "description": "Optional working directory for the local agent target. Existing targets whose cwd contains this value are preferred, and new panes start there."
                },
                "pane_index": {
                    "type": ["integer", "null"],
                    "description": "Optional con pane index to constrain reuse checks. Positional only."
                },
                "pane_id": {
                    "type": ["integer", "null"],
                    "description": "Optional stable pane id to constrain reuse checks."
                },
                "launch_command": {
                    "type": ["string", "null"],
                    "description": "Optional explicit launch command. Defaults to the canonical CLI name for the requested agent."
                },
                "location": {
                    "type": "string",
                    "enum": ["right", "down"],
                    "description": "Where to place a newly created agent pane if one is needed. Defaults to right."
                }
            },
            "required": ["agent_name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut result = ensure_local_agent_target_impl(&self.pane_tx, args)?;
        let target = selector_from(Some(result.pane_index), result.pane_id);
        let output = if result.created {
            wait_for_pane_initial_output(&self.pane_tx, target).await?
        } else {
            String::new()
        };
        let (output, interstitial_note) = if result.created {
            maybe_auto_accept_known_agent_interstitial(
                &self.pane_tx,
                &result.agent_name,
                target,
                output,
            )
            .await?
        } else {
            (output, None)
        };
        if let Some(note) = interstitial_note {
            result.reason = format!("{} {}", result.reason, note);
        }
        Ok(EnsureLocalAgentTargetResult { output, ..result })
    }
}

// ── ensure_local_coding_workspace tool ───────────────────────────

#[derive(Serialize)]
pub struct EnsureLocalCodingWorkspaceResult {
    pub agent_name: String,
    pub cwd: Option<String>,
    pub agent_target: EnsureLocalAgentTargetResult,
    pub shell_target: EnsureLocalShellTargetResult,
    pub reason: String,
}

#[derive(Deserialize)]
pub struct EnsureLocalCodingWorkspaceArgs {
    pub agent_name: String,
    pub cwd: Option<String>,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub launch_command: Option<String>,
    #[serde(default = "default_pane_create_location")]
    pub location: PaneCreateLocation,
}

pub struct EnsureLocalCodingWorkspaceTool {
    pane_tx: Sender<PaneRequest>,
}

impl EnsureLocalCodingWorkspaceTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

fn ensure_local_coding_workspace_impl(
    pane_tx: &Sender<PaneRequest>,
    args: EnsureLocalCodingWorkspaceArgs,
) -> Result<EnsureLocalCodingWorkspaceResult, ToolError> {
    let agent_target = ensure_local_agent_target_impl(
        pane_tx,
        EnsureLocalAgentTargetArgs {
            agent_name: args.agent_name.clone(),
            cwd: args.cwd.clone(),
            pane_index: args.pane_index,
            pane_id: args.pane_id,
            launch_command: args.launch_command.clone(),
            create_cwd_if_missing: true,
            location: args.location,
        },
    )?;

    let mut shell_target = ensure_local_shell_target_impl(
        pane_tx,
        EnsureLocalShellTargetArgs {
            cwd: args.cwd.clone(),
            pane_index: None,
            pane_id: None,
            create_cwd_if_missing: true,
            location: args.location,
        },
    )?;

    if shell_target.pane_id == agent_target.pane_id {
        shell_target = ensure_local_shell_target_impl(
            pane_tx,
            EnsureLocalShellTargetArgs {
                cwd: args.cwd.clone(),
                pane_index: None,
                pane_id: None,
                create_cwd_if_missing: true,
                location: args.location,
            },
        )?;

        if shell_target.pane_id == agent_target.pane_id {
            let command = args
                .cwd
                .as_ref()
                .map(|cwd| build_local_cwd_command_prefix(cwd, true));
            let (response_tx, response_rx) = crossbeam_channel::bounded(1);
            pane_tx
                .send(PaneRequest {
                    query: PaneQuery::CreatePane {
                        command: command.clone(),
                        location: args.location,
                    },
                    response_tx,
                })
                .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

            let response = tokio::task::block_in_place(|| {
                response_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .map_err(|_| ToolError::CommandFailed("Create pane request timed out".into()))
            })?;

            let (pane_index, pane_id) = match response {
                PaneResponse::PaneCreated {
                    pane_index,
                    pane_id,
                    ..
                } => (pane_index, pane_id),
                PaneResponse::Error(e) => return Err(ToolError::CommandFailed(e)),
                _ => return Err(ToolError::CommandFailed("Unexpected response".into())),
            };

            shell_target = EnsureLocalShellTargetResult {
                created: true,
                pane_index,
                pane_id: Some(pane_id),
                cwd: args.cwd.clone(),
                match_source: LocalShellMatchSource::Created,
                command,
                output: String::new(),
                reason: "The existing shell candidate was the same pane as the interactive agent target, so con created a separate local shell pane to preserve the paired workspace.".to_string(),
            };
        }
    }

    let shell_created = shell_target.created;
    let agent_created = agent_target.created;
    let cwd_note = args
        .cwd
        .as_ref()
        .map(|cwd| format!(" at `{cwd}`"))
        .unwrap_or_default();

    Ok(EnsureLocalCodingWorkspaceResult {
        agent_name: agent_target.agent_name.clone(),
        cwd: args.cwd,
        reason: match (agent_created, shell_created) {
            (false, false) => format!(
                "Reused both the local {} pane and its paired shell workspace{}.",
                agent_target.agent_name, cwd_note
            ),
            (true, false) => format!(
                "Created a new local {} pane and reused an existing paired shell workspace{}.",
                agent_target.agent_name, cwd_note
            ),
            (false, true) => format!(
                "Reused the local {} pane and created a fresh paired shell workspace{}.",
                agent_target.agent_name, cwd_note
            ),
            (true, true) => format!(
                "Created both a local {} pane and a paired shell workspace{}.",
                agent_target.agent_name, cwd_note
            ),
        },
        agent_target,
        shell_target,
    })
}

impl Tool for EnsureLocalCodingWorkspaceTool {
    const NAME: &'static str = "ensure_local_coding_workspace";
    type Error = ToolError;
    type Args = EnsureLocalCodingWorkspaceArgs;
    type Output = EnsureLocalCodingWorkspaceResult;

    fn description(&self) -> String {
        "Prepare a LOCAL coding workspace as a deliberate pair: one pane for the interactive Codex / Claude Code / OpenCode UI, and a separate local shell pane for file edits, tests, git commands, and other shell work. Reuses existing matching panes when possible and creates only the missing side of the pair.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "Agent CLI family to reuse or launch, such as codex, claude, or opencode."
                },
                "cwd": {
                    "type": ["string", "null"],
                    "description": "Optional project directory the local coding workspace should be prepared in."
                },
                "pane_index": {
                    "type": ["integer", "null"],
                    "description": "Optional con pane index to bias or constrain agent-target reuse. Positional only."
                },
                "pane_id": {
                    "type": ["integer", "null"],
                    "description": "Optional stable pane id to bias or constrain agent-target reuse."
                },
                "launch_command": {
                    "type": ["string", "null"],
                    "description": "Optional explicit launch command for the agent CLI. Defaults to the canonical CLI command."
                },
                "location": {
                    "type": "string",
                    "enum": ["right", "down"],
                    "description": "Where to place newly created panes. Defaults to right."
                }
            },
            "required": ["agent_name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut result = ensure_local_coding_workspace_impl(&self.pane_tx, args)?;
        if result.agent_target.created {
            result.agent_target.output = wait_for_pane_initial_output(
                &self.pane_tx,
                selector_from(
                    Some(result.agent_target.pane_index),
                    result.agent_target.pane_id,
                ),
            )
            .await?;
        }
        if result.shell_target.created {
            result.shell_target.output = wait_for_pane_initial_output(
                &self.pane_tx,
                selector_from(
                    Some(result.shell_target.pane_index),
                    result.shell_target.pane_id,
                ),
            )
            .await?;
        }
        Ok(result)
    }
}

// ── agent_cli_turn tool ───────────────────────────────────────────

#[derive(Deserialize)]
pub struct AgentCliTurnArgs {
    pub agent_name: String,
    pub prompt: String,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub cwd_contains: Option<String>,
    #[serde(default = "default_agent_cli_turn_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_pane_lines")]
    pub lines: usize,
}

#[derive(Serialize)]
pub struct AgentCliTurnResult {
    pub agent_name: String,
    pub pane_index: usize,
    pub pane_id: usize,
    pub control_path: WorkTargetControlPath,
    pub tmux_target: Option<crate::tmux::TmuxPaneInfo>,
    pub wait_status: String,
    pub output: String,
    pub note: String,
}

pub struct AgentCliTurnTool {
    pane_tx: Sender<PaneRequest>,
}

impl AgentCliTurnTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for AgentCliTurnTool {
    const NAME: &'static str = "agent_cli_turn";
    type Error = ToolError;
    type Args = AgentCliTurnArgs;
    type Output = AgentCliTurnResult;

    fn description(&self) -> String {
        "Send one natural-language turn to an already prepared interactive agent CLI target such as Codex, Claude Code, or OpenCode, then wait for the target to settle and return a fresh screen snapshot. This is the preferred tool for continuing work inside a known local agent pane or tmux agent target. It does not prepare missing targets; use ensure_local_agent_target, ensure_local_coding_workspace, or tmux_ensure_agent_target first when preparation is still required.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "Agent CLI family to target, such as codex, claude, or opencode."
                },
                "prompt": {
                    "type": "string",
                    "description": "Natural-language turn to send into the existing agent CLI target."
                },
                "pane_index": {
                    "type": ["integer", "null"],
                    "description": "Optional con pane index to constrain target selection. Positional only."
                },
                "pane_id": {
                    "type": ["integer", "null"],
                    "description": "Optional stable con pane id to constrain target selection."
                },
                "cwd_contains": {
                    "type": ["string", "null"],
                    "description": "Optional project-path hint used to prefer a matching existing target."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "How long to wait for the agent target to settle after sending the prompt. Defaults to 90 seconds."
                },
                "lines": {
                    "type": "integer",
                    "description": "How many recent lines to read or capture after the turn settles. Defaults to 50."
                }
            },
            "required": ["agent_name", "prompt"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let Some(agent_name) = canonical_agent_cli_name(&args.agent_name) else {
            return Err(ToolError::CommandFailed(format!(
                "Unsupported agent_name `{}`. Expected codex, claude, or opencode.",
                args.agent_name
            )));
        };

        let panes = pane_query_list(&self.pane_tx)?;
        let resolved = resolve_work_target_candidates(
            &self.pane_tx,
            panes,
            WorkTargetIntent::AgentCli,
            args.pane_index,
            args.pane_id,
            None,
            args.cwd_contains.clone(),
            Some(agent_name.to_string()),
            1,
        )?;

        let best = resolved.best_match.ok_or_else(|| {
            ToolError::CommandFailed(format!(
                "No existing {agent_name} target is currently available. Prepare one first with ensure_local_agent_target, ensure_local_coding_workspace, or tmux_ensure_agent_target."
            ))
        })?;

        if best.requires_preparation {
            return Err(ToolError::CommandFailed(format!(
                "{} Use {} first.",
                best.reason, best.suggested_tool
            )));
        }

        let timeout_secs = args.timeout_secs.clamp(5, 180);
        let lines = args.lines.clamp(10, 200);

        match best.control_path {
            WorkTargetControlPath::VisibleAgentUi => {
                let target = selector_from(Some(best.pane_index), Some(best.pane_id));
                let current_output = pane_query_read_content(&self.pane_tx, target, lines)?;
                let (_, interstitial_note) = maybe_auto_accept_known_agent_interstitial(
                    &self.pane_tx,
                    agent_name,
                    target,
                    current_output,
                )
                .await?;
                submit_visible_agent_prompt(&self.pane_tx, target, &args.prompt).await?;
                let (wait_status, settled_output) =
                    wait_for_visible_agent_turn(&self.pane_tx, target, lines, timeout_secs).await?;
                let output = if settled_output.trim().is_empty() {
                    pane_query_read_content(&self.pane_tx, target, lines)?
                } else {
                    settled_output
                };
                let mut note = if wait_status == "submitted_only"
                    || output_contains_queue_hint(&output)
                {
                    "Sent the prompt into the visible agent CLI pane with a separate submit keystroke and waited for a second response phase. The pane still shows only submission-visible state, so this does not yet prove the agent answered."
                        .to_string()
                } else {
                    "Sent the prompt into the visible agent CLI pane with a separate submit keystroke, waited for a distinct response phase to settle, and returned a fresh pane snapshot."
                        .to_string()
                };
                if let Some(extra) = interstitial_note {
                    note = format!("{note} {extra}");
                }
                Ok(AgentCliTurnResult {
                    agent_name: agent_name.to_string(),
                    pane_index: best.pane_index,
                    pane_id: best.pane_id,
                    control_path: best.control_path,
                    tmux_target: None,
                    wait_status,
                    output,
                    note,
                })
            }
            WorkTargetControlPath::TmuxAgentTarget => {
                let tmux_target = best.tmux_target.clone().ok_or_else(|| {
                    ToolError::CommandFailed(
                        "Resolved tmux agent target is missing its tmux pane identity.".to_string(),
                    )
                })?;
                let pane = selector_from(Some(best.pane_index), Some(best.pane_id));
                submit_tmux_agent_prompt(&self.pane_tx, pane, &tmux_target.target, &args.prompt)
                    .await?;
                let (wait_status, output) = wait_for_tmux_capture_settle(
                    &self.pane_tx,
                    pane,
                    tmux_target.target.clone(),
                    lines,
                    timeout_secs,
                )
                .await?;
                Ok(AgentCliTurnResult {
                    agent_name: agent_name.to_string(),
                    pane_index: best.pane_index,
                    pane_id: best.pane_id,
                    control_path: best.control_path,
                    tmux_target: Some(tmux_target),
                    wait_status,
                    output,
                    note: "Sent the prompt into the tmux agent target with a separate submit keystroke, waited for captured output to settle, and returned a fresh tmux capture.".to_string(),
                })
            }
            _ => Err(ToolError::CommandFailed(format!(
                "Resolved target uses control path `{}`. Use {} instead.",
                serde_json::to_string(&best.control_path).unwrap_or_else(|_| "unknown".to_string()),
                best.suggested_tool
            ))),
        }
    }
}

// ── tmux_shell_turn tool ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct TmuxShellTurnArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub target: String,
    pub command: String,
    #[serde(default = "default_agent_cli_turn_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_pane_lines")]
    pub lines: usize,
}

#[derive(Serialize)]
pub struct TmuxShellTurnResult {
    pub pane_index: usize,
    pub pane_id: usize,
    pub target: crate::tmux::TmuxPaneInfo,
    pub wait_status: String,
    pub output: String,
    pub note: String,
}

pub struct TmuxShellTurnTool {
    pane_tx: Sender<PaneRequest>,
}

impl TmuxShellTurnTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for TmuxShellTurnTool {
    const NAME: &'static str = "tmux_shell_turn";
    type Error = ToolError;
    type Args = TmuxShellTurnArgs;
    type Output = TmuxShellTurnResult;

    fn description(&self) -> String {
        "Run one deterministic shell command inside an existing tmux shell target, wait for that target to settle, and return a fresh capture. Use this for file work, test runs, install checks, and other shell-lane work inside tmux instead of manually composing tmux_send_keys plus tmux_capture_pane.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": ["integer", "null"],
                    "description": "Optional con pane index that owns the tmux control anchor. Positional only."
                },
                "pane_id": {
                    "type": ["integer", "null"],
                    "description": "Optional stable con pane id that owns the tmux control anchor."
                },
                "target": {
                    "type": "string",
                    "description": "tmux shell target such as %17 or session:window.pane."
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to run inside the existing tmux shell target."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "How long to wait for the tmux shell target to settle after the command. Defaults to 90 seconds."
                },
                "lines": {
                    "type": "integer",
                    "description": "How many recent lines to capture after the command settles. Defaults to 50."
                }
            },
            "required": ["target", "command"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let pane = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let snapshot = pane_query_tmux_list(&self.pane_tx, pane)?;
        let target = snapshot
            .panes
            .into_iter()
            .find(|candidate| candidate.target == args.target || candidate.pane_id == args.target)
            .ok_or_else(|| {
                ToolError::CommandFailed(format!(
                    "tmux target `{}` was not found in the current tmux workspace.",
                    args.target
                ))
            })?;

        if !target
            .pane_current_command
            .as_deref()
            .is_some_and(is_tmux_shell_command)
        {
            return Err(ToolError::CommandFailed(format!(
                "tmux target `{}` is not a known shell target (current command: {}). Use tmux_ensure_shell_target for shell work or tmux_ensure_agent_target / agent_cli_turn for interactive agent CLIs.",
                target.target,
                target.pane_current_command.as_deref().unwrap_or("unknown")
            )));
        }

        pane_query_tmux_send_keys(
            &self.pane_tx,
            pane,
            target.target.clone(),
            Some(args.command),
            Vec::new(),
            true,
        )?;
        let timeout_secs = args.timeout_secs.clamp(5, 180);
        let lines = args.lines.clamp(10, 200);
        let (wait_status, output) = wait_for_tmux_capture_settle(
            &self.pane_tx,
            pane,
            target.target.clone(),
            lines,
            timeout_secs,
        )
        .await?;
        let resolved_pane = pane_query_list(&self.pane_tx)?
            .into_iter()
            .find(|candidate| {
                pane.pane_id.is_some_and(|id| candidate.pane_id == id)
                    || pane.pane_index.is_some_and(|index| candidate.index == index)
            })
            .ok_or_else(|| {
                ToolError::CommandFailed(
                    "tmux shell turn completed, but con could not resolve the owning pane afterward."
                        .to_string(),
                )
            })?;

        Ok(TmuxShellTurnResult {
            pane_index: resolved_pane.index,
            pane_id: resolved_pane.pane_id,
            target,
            wait_status,
            output,
            note: "Sent the command into the existing tmux shell target, waited for captured output to settle, and returned a fresh tmux capture.".to_string(),
        })
    }
}

// ── resolve_work_target tool ─────────────────────────────────────

#[derive(Deserialize)]
pub struct TmuxEnsureAgentTargetArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub agent_name: String,
    pub cwd: Option<String>,
    pub window_name: Option<String>,
    pub launch_command: Option<String>,
    #[serde(default = "default_tmux_shell_location")]
    pub location: TmuxExecLocation,
    #[serde(default = "default_true")]
    pub detached: bool,
}

pub struct TmuxEnsureAgentTargetTool {
    pane_tx: Sender<PaneRequest>,
}

impl TmuxEnsureAgentTargetTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for TmuxEnsureAgentTargetTool {
    const NAME: &'static str = "tmux_ensure_agent_target";
    type Error = ToolError;
    type Args = TmuxEnsureAgentTargetArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Return a tmux pane that is suitable for interacting with a specific agent CLI. Reuses an existing tmux pane already running Codex, Claude Code, or OpenCode when one matches, otherwise creates a fresh tmux target with the requested agent launch command. This stays in the tmux control plane; it does not claim app-native Codex or OpenCode control unless a separate explicit attachment exists.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target con pane index from list_panes. Positional only; it can change."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up tmux work."
                },
                "agent_name": {
                    "type": "string",
                    "description": "Agent CLI family to reuse or launch, such as codex, claude, or opencode."
                },
                "cwd": {
                    "type": ["string", "null"],
                    "description": "Optional path hint. Existing tmux agent panes whose pane_current_path contains this value are preferred, and new targets inherit it when created."
                },
                "window_name": {
                    "type": ["string", "null"],
                    "description": "Optional name for a newly created tmux window."
                },
                "launch_command": {
                    "type": ["string", "null"],
                    "description": "Optional explicit command to launch when no matching agent pane exists. Defaults to the canonical CLI name for the requested agent."
                },
                "location": {
                    "type": "string",
                    "enum": ["new_window", "split_horizontal", "split_vertical"],
                    "description": "Where to create a new agent target if none already exists."
                },
                "detached": {
                    "type": "boolean",
                    "description": "Whether a newly created target should start detached. Defaults to true."
                }
            },
            "required": ["agent_name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let Some(agent_name) = canonical_agent_cli_name(&args.agent_name) else {
            return Err(ToolError::CommandFailed(format!(
                "Unsupported agent_name `{}`. Expected codex, claude, or opencode.",
                args.agent_name
            )));
        };
        let cwd_contains = args.cwd.as_ref().map(|v| v.to_ascii_lowercase());
        let pane = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let snapshot = pane_query_tmux_list(&self.pane_tx, pane)?;
        let mut matches = snapshot
            .panes
            .into_iter()
            .filter(|pane| {
                pane.pane_current_command
                    .as_deref()
                    .is_some_and(|command| classify_agent_cli_command(command) == Some(agent_name))
                    && cwd_contains.as_ref().is_none_or(|needle| {
                        pane.pane_current_path
                            .as_deref()
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                            .contains(needle)
                    })
            })
            .collect::<Vec<_>>();
        sort_tmux_matches(&mut matches);

        let (native_attachment_state, native_attachment_note) =
            agent_cli_native_attachment(agent_name);

        if let Some(pane) = matches.into_iter().next() {
            let result = TmuxEnsureAgentTargetResult {
                agent_name: agent_name.to_string(),
                created: false,
                pane,
                creation: None,
                launch_command: None,
                native_attachment_state,
                native_attachment_note,
            };
            return serde_json::to_value(&result)
                .map_err(|e| ToolError::CommandFailed(e.to_string()));
        }

        let launch_command = match args.launch_command {
            Some(command) => command,
            None => default_agent_launch_command(agent_name)
                .ok_or_else(|| {
                    ToolError::CommandFailed(format!(
                        "No default launch command is defined for agent `{agent_name}`."
                    ))
                })?
                .to_string(),
        };
        let creation = pane_query_tmux_run_command(
            &self.pane_tx,
            pane,
            None,
            args.location,
            launch_command.clone(),
            args.window_name
                .clone()
                .or_else(|| Some(agent_name.to_string())),
            args.cwd.clone(),
            args.detached,
        )?;
        let snapshot = pane_query_tmux_list(&self.pane_tx, pane)?;
        let pane = snapshot
            .panes
            .into_iter()
            .find(|pane| pane.target == creation.target || pane.pane_id == creation.pane_id)
            .unwrap_or_else(|| crate::tmux::TmuxPaneInfo {
                session_name: creation.session_name.clone(),
                window_id: creation.window_id.clone(),
                window_index: creation.window_index.clone(),
                window_name: creation.window_name.clone(),
                window_target: creation.window_target.clone(),
                pane_id: creation.pane_id.clone(),
                pane_index: creation.pane_index.clone(),
                target: creation.target.clone(),
                pane_active: !creation.detached,
                window_active: !creation.detached,
                pane_current_command: Some(agent_name.to_string()),
                pane_current_path: args.cwd,
            });

        let result = TmuxEnsureAgentTargetResult {
            agent_name: agent_name.to_string(),
            created: true,
            pane,
            creation: Some(creation),
            launch_command: Some(launch_command),
            native_attachment_state,
            native_attachment_note,
        };
        serde_json::to_value(&result).map_err(|e| ToolError::CommandFailed(e.to_string()))
    }
}

// ── resolve_work_target tool ─────────────────────────────────────

#[derive(Deserialize)]
pub struct ResolveWorkTargetArgs {
    pub intent: WorkTargetIntent,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub host_contains: Option<String>,
    pub cwd_contains: Option<String>,
    pub agent_name: Option<String>,
    #[serde(default = "default_tmux_find_limit")]
    pub limit: usize,
}

pub struct ResolveWorkTargetTool {
    pane_tx: Sender<PaneRequest>,
}

impl ResolveWorkTargetTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for ResolveWorkTargetTool {
    const NAME: &'static str = "resolve_work_target";
    type Error = ToolError;
    type Args = ResolveWorkTargetArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Resolve the best pane or tmux target for a specific kind of work using con's typed control plane. Use this when multiple panes are open and you need to choose the right shell, the right tmux workspace, the best tmux shell pane, or a matching agent CLI target without re-deriving that logic from list_panes manually. When it returns a con pane target, prefer its stable pane_id for follow-up work.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "intent": {
                    "type": "string",
                    "enum": ["visible_shell", "remote_shell", "tmux_workspace", "tmux_shell", "agent_cli"],
                    "description": "What kind of work target you need."
                },
                "pane_index": {
                    "type": ["integer", "null"],
                    "description": "Optional con pane index to constrain resolution to a single pane. Positional only."
                },
                "pane_id": {
                    "type": ["integer", "null"],
                    "description": "Optional stable pane id to constrain resolution to a single pane."
                },
                "host_contains": {
                    "type": ["string", "null"],
                    "description": "Optional case-insensitive filter for the effective host name."
                },
                "cwd_contains": {
                    "type": ["string", "null"],
                    "description": "Optional case-insensitive filter for working directory or tmux pane path."
                },
                "agent_name": {
                    "type": ["string", "null"],
                    "description": "Optional case-insensitive filter for an agent CLI name such as codex, claude, or opencode."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of candidates to return. Defaults to 8."
                }
            },
            "required": ["intent"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let panes = pane_query_list(&self.pane_tx)?;
        let result = resolve_work_target_candidates(
            &self.pane_tx,
            panes,
            args.intent,
            args.pane_index,
            args.pane_id,
            args.host_contains,
            args.cwd_contains,
            args.agent_name,
            args.limit,
        )?;
        serde_json::to_value(&result).map_err(|e| ToolError::CommandFailed(e.to_string()))
    }
}

// ── probe_shell_context tool ───────────────────────────────────────

#[derive(Deserialize)]
pub struct ProbeShellContextArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
}

pub struct ProbeShellContextTool {
    pane_tx: Sender<PaneRequest>,
}

impl ProbeShellContextTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for ProbeShellContextTool {
    const NAME: &'static str = "probe_shell_context";
    type Error = ToolError;
    type Args = ProbeShellContextArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Run a read-only shell-scoped probe in a pane that has the probe_shell_context capability. Use this only when list_panes reports a proven fresh shell prompt. Returns authoritative shell facts from that shell frame such as hostname, pwd, SSH env, TMUX env, tmux session/window/pane ids, pane_current_command, pane_current_path, and NVIM_LISTEN_ADDRESS when available. This is shell-scope truth, not proof of the foreground app after control passes to tmux or another TUI.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target con pane index from list_panes. Positional only."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable target pane id from list_panes. Prefer this for follow-up work."
                }
            },
            "required": []
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let target = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::ProbeShellContext { target },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(12))
                .map_err(|_| ToolError::CommandFailed("Shell probe timed out".into()))
        })?;

        match response {
            PaneResponse::ShellProbe(info) => {
                serde_json::to_value(&info).map_err(|e| ToolError::CommandFailed(e.to_string()))
            }
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

// ── read_pane tool ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ReadPaneArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    #[serde(default = "default_pane_lines")]
    pub lines: usize,
}

fn default_pane_lines() -> usize {
    50
}

fn default_agent_cli_turn_timeout_secs() -> u64 {
    90
}

async fn wait_for_tmux_capture_settle(
    pane_tx: &Sender<PaneRequest>,
    pane: PaneSelector,
    target: String,
    lines: usize,
    timeout_secs: u64,
) -> Result<(String, String), ToolError> {
    const POLL_MS: u64 = 700;
    const STABLE_POLLS: u32 = 3;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut last_snapshot = String::new();
    let mut stable_count = 0u32;
    let mut saw_non_empty = false;

    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
        let capture = pane_query_tmux_capture(pane_tx, pane, Some(target.clone()), lines)?;
        let snapshot = capture.content.trim_end().to_string();
        if snapshot.is_empty() {
            continue;
        }
        saw_non_empty = true;
        if snapshot == last_snapshot {
            stable_count += 1;
            if stable_count >= STABLE_POLLS {
                return Ok(("settled".to_string(), snapshot));
            }
        } else {
            last_snapshot = snapshot;
            stable_count = 0;
        }
    }

    let status = if saw_non_empty {
        "timeout"
    } else {
        "no_output"
    };
    Ok((status.to_string(), last_snapshot))
}

async fn wait_for_visible_pane_settle(
    pane_tx: &Sender<PaneRequest>,
    target: PaneSelector,
    lines: usize,
    timeout_secs: u64,
) -> Result<(String, String), ToolError> {
    const POLL_MS: u64 = 700;
    const STABLE_POLLS: u32 = 3;

    let baseline = pane_query_read_content(pane_tx, target, lines)?
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut phase_changed = false;
    let mut last_snapshot = baseline.clone();
    let mut latest_snapshot = baseline;
    let mut stable_count = 0u32;

    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
        let snapshot = match pane_query_read_content(pane_tx, target, lines) {
            Ok(snapshot) => snapshot,
            Err(err) if is_transient_pane_read_timeout(&err) => continue,
            Err(err) => return Err(err),
        }
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
        latest_snapshot = snapshot.clone();

        if !phase_changed {
            if snapshot != last_snapshot {
                phase_changed = true;
                last_snapshot = snapshot;
                stable_count = 0;
            }
            continue;
        }

        if snapshot == last_snapshot {
            stable_count += 1;
            if stable_count >= STABLE_POLLS {
                return Ok(("settled".to_string(), snapshot));
            }
        } else {
            last_snapshot = snapshot;
            stable_count = 0;
        }
    }

    let status = if phase_changed {
        "timeout"
    } else {
        "no_change"
    };
    Ok((status.to_string(), latest_snapshot))
}

async fn wait_for_visible_agent_turn(
    pane_tx: &Sender<PaneRequest>,
    target: PaneSelector,
    lines: usize,
    timeout_secs: u64,
) -> Result<(String, String), ToolError> {
    const POLL_MS: u64 = 700;
    const STABLE_POLLS: u32 = 3;

    let baseline = pane_query_read_content(pane_tx, target, lines)?
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut submission_snapshot: Option<String> = None;
    let mut response_phase_started = false;
    let mut last_snapshot = baseline.clone();
    let mut latest_snapshot = baseline;
    let mut stable_count = 0u32;

    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
        let snapshot = match pane_query_read_content(pane_tx, target, lines) {
            Ok(snapshot) => snapshot,
            Err(err) if is_transient_pane_read_timeout(&err) => continue,
            Err(err) => return Err(err),
        }
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
        latest_snapshot = snapshot.clone();

        if submission_snapshot.is_none() {
            if snapshot != last_snapshot {
                submission_snapshot = Some(snapshot.clone());
                last_snapshot = snapshot;
                stable_count = 0;
            }
            continue;
        }

        if !response_phase_started {
            if submission_snapshot
                .as_ref()
                .is_some_and(|submitted| &snapshot != submitted)
            {
                response_phase_started = true;
                last_snapshot = snapshot;
                stable_count = 0;
            }
            continue;
        }

        if snapshot == last_snapshot {
            stable_count += 1;
            if stable_count >= STABLE_POLLS {
                return Ok(("settled".to_string(), snapshot));
            }
        } else {
            last_snapshot = snapshot;
            stable_count = 0;
        }
    }

    let status = if response_phase_started {
        "timeout"
    } else if submission_snapshot.is_some() {
        "submitted_only"
    } else {
        "no_change"
    };
    Ok((status.to_string(), latest_snapshot))
}

pub struct ReadPaneTool {
    pane_tx: Sender<PaneRequest>,
}

impl ReadPaneTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for ReadPaneTool {
    const NAME: &'static str = "read_pane";
    type Error = ToolError;
    type Args = ReadPaneArgs;
    type Output = String;

    fn description(&self) -> String {
        "Read the recent visible output from a specific terminal pane. Use list_panes first to discover available panes. Prefer stable pane_id for follow-up work; pane_index is only positional.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "The pane index from list_panes. Positional only."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable pane id from list_panes. Prefer this for follow-up work."
                },
                "lines": {
                    "type": "integer",
                    "description": "Number of recent lines to read (default: 50)"
                }
            },
            "required": []
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let target = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::ReadContent {
                    target,
                    lines: args.lines,
                },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|_| ToolError::CommandFailed("Pane query timed out".into()))
        })?;

        match response {
            PaneResponse::Content(content) => Ok(content),
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

// ── send_keys tool ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SendKeysArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub keys: String,
}

/// Send raw keystrokes to any terminal pane. This is the low-level
/// primitive for interacting with TUIs, cancelling commands (Ctrl-C),
/// or sending input to any running process — not just shells.
pub struct SendKeysTool {
    pane_tx: Sender<PaneRequest>,
}

impl SendKeysTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for SendKeysTool {
    const NAME: &'static str = "send_keys";
    type Error = ToolError;
    type Args = SendKeysArgs;
    type Output = String;

    fn description(&self) -> String {
        "Send raw keystrokes to a specific con terminal pane. Use this for direct TUI interaction, prompt-level shell input when exec_visible_shell is unavailable, and tmux prefix sequences only when tmux native control is unavailable. IMPORTANT: Always follow send_keys with read_pane to verify the action took effect. Common sequences: \\n (Enter), \\x1b (Escape), \\x03 (Ctrl-C), \\x02 (Ctrl-B, tmux prefix), \\x1b[A/B/C/D (arrow keys). For shell commands, prefer terminal_exec when exec_visible_shell is available. For tmux panes with query_tmux/send_tmux_keys, prefer tmux-native tools.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "The pane index from list_panes. Positional only."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable pane id from list_panes. Prefer this for follow-up work."
                },
                "keys": {
                    "type": "string",
                    "description": "Keystrokes to send. Supports escape sequences: \\n (Enter), \\t (Tab), \\x03 (Ctrl-C), \\x1b (Escape), \\x1b[A (Up arrow), etc."
                }
            },
            "required": ["keys"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let target = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        // Decode escape sequences in the keys string
        let decoded = decode_key_escapes(&args.keys);
        log::info!(
            "[send_keys] target={} raw={:?} decoded={:?} decoded_bytes={:?}",
            target.describe(),
            args.keys,
            decoded,
            decoded.as_bytes()
        );

        // Split at standalone-ESC boundaries to avoid VT parser ambiguity.
        // When ESC (0x1B) is immediately followed by a printable char like 'g',
        // the terminal interprets "ESC g" as a 2-byte escape sequence instead of
        // a standalone ESC key followed by 'g'. We split the write at these points
        // and insert a small delay so the terminal processes ESC on its own.
        let segments = split_at_standalone_esc(&decoded);

        for (i, segment) in segments.iter().enumerate() {
            if i > 0 {
                // Brief delay so the terminal's VT parser commits the previous ESC
                // as a standalone key before receiving the next character.
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let (response_tx, response_rx) = crossbeam_channel::bounded(1);
            self.pane_tx
                .send(PaneRequest {
                    query: PaneQuery::SendKeys {
                        target,
                        keys: segment.clone(),
                    },
                    response_tx,
                })
                .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

            let response = tokio::task::block_in_place(|| {
                response_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .map_err(|_| ToolError::CommandFailed("Pane query timed out".into()))
            })?;

            match response {
                PaneResponse::KeysSent => {}
                PaneResponse::Error(e) => return Err(ToolError::CommandFailed(e)),
                _ => return Err(ToolError::CommandFailed("Unexpected response".into())),
            }
        }

        Ok("Keys sent successfully".to_string())
    }
}

// ── batch_exec tool ──────────────────────────────────────────────
//
// Execute commands across multiple panes in parallel. This is critical
// for multi-pane workflows: "run uptime on all machines" executes
// concurrently instead of sequentially (which would take N * timeout).
//
// Works within Rig's sequential tool dispatch constraint by batching
// what would otherwise be N sequential terminal_exec calls into one.

#[derive(Deserialize)]
pub struct BatchExecArgs {
    pub commands: Vec<BatchCommand>,
}

#[derive(Deserialize, Debug)]
pub struct BatchCommand {
    pub command: String,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
}

#[derive(Serialize)]
struct BatchResult {
    pane_index: usize,
    pane_id: Option<usize>,
    output: String,
    exit_code: Option<i32>,
    error: Option<String>,
}

pub struct BatchExecTool {
    request_tx: Sender<TerminalExecRequest>,
}

impl BatchExecTool {
    pub fn new(request_tx: Sender<TerminalExecRequest>) -> Self {
        Self { request_tx }
    }
}

impl Tool for BatchExecTool {
    const NAME: &'static str = "batch_exec";
    type Error = ToolError;
    type Args = BatchExecArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Execute commands across multiple con panes in PARALLEL, but only when each pane's control_capabilities include exec_visible_shell. Prefer stable pane_id values from list_panes for follow-up work; pane_index is only positional.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "commands": {
                    "type": "array",
                    "description": "List of commands to execute, each targeting a specific pane",
                    "items": {
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "The shell command to execute"
                            },
                            "pane_index": {
                                "type": "integer",
                                "description": "Target pane index from list_panes. Positional only."
                            },
                            "pane_id": {
                                "type": "integer",
                                "description": "Stable target pane id from list_panes. Prefer this for follow-up work."
                            }
                        },
                        "required": ["command"]
                    }
                }
            },
            "required": ["commands"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.commands.is_empty() {
            return Ok(serde_json::Value::Array(vec![]));
        }

        log::info!(
            "[batch_exec] Executing {} commands in parallel",
            args.commands.len()
        );

        // Send all commands concurrently, collect response receivers
        let mut receivers = Vec::with_capacity(args.commands.len());
        for cmd in &args.commands {
            let target = require_pane_target(cmd.pane_index, cmd.pane_id, Self::NAME)?;
            let (response_tx, response_rx) = crossbeam_channel::bounded(1);
            self.request_tx
                .send(TerminalExecRequest {
                    command: cmd.command.clone(),
                    working_dir: None,
                    target,
                    response_tx,
                })
                .map_err(|_| ToolError::CommandFailed("Terminal exec channel closed".into()))?;
            receivers.push((cmd.pane_index.unwrap_or(0), cmd.pane_id, response_rx));
        }

        // Wait for all results concurrently with timeout
        let results: Vec<BatchResult> = tokio::task::block_in_place(|| {
            receivers
                .into_iter()
                .map(|(pane_index, pane_id, rx)| {
                    match rx.recv_timeout(std::time::Duration::from_secs(60)) {
                        Ok(resp) => BatchResult {
                            pane_index,
                            pane_id,
                            output: resp.output,
                            exit_code: resp.exit_code,
                            error: None,
                        },
                        Err(_) => BatchResult {
                            pane_index,
                            pane_id,
                            output: String::new(),
                            exit_code: None,
                            error: Some("Timed out (60s)".into()),
                        },
                    }
                })
                .collect()
        });

        serde_json::to_value(&results).map_err(|e| ToolError::CommandFailed(e.to_string()))
    }
}

// ── search_panes tool ─────────────────────────────────────────────
//
// Search scrollback + visible screen across one or all panes.
// Invaluable for finding previous command output, error messages,
// or any text the user has seen in any pane.

#[derive(Deserialize)]
pub struct SearchPanesArgs {
    pub pattern: String,
    #[serde(default)]
    pub pane_index: Option<usize>,
    #[serde(default)]
    pub pane_id: Option<usize>,
    #[serde(default = "default_max_matches")]
    pub max_matches: usize,
}

fn default_max_matches() -> usize {
    50
}

pub struct SearchPanesTool {
    pane_tx: Sender<PaneRequest>,
}

impl SearchPanesTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

impl Tool for SearchPanesTool {
    const NAME: &'static str = "search_panes";
    type Error = ToolError;
    type Args = SearchPanesArgs;
    type Output = String;

    fn description(&self) -> String {
        "Search terminal scrollback and visible screen for text. Searches across all panes or a specific pane. Use this to find previous command output, error messages, or any text that appeared in any terminal pane.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Text to search for (case-insensitive substring match)"
                },
                "pane_index": {
                    "type": "integer",
                    "description": "Optional pane index from list_panes. Positional only."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Optional stable pane id from list_panes. Prefer this for follow-up work."
                },
                "max_matches": {
                    "type": "integer",
                    "description": "Maximum number of matching lines to return (default: 50)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::SearchText {
                    target: selector_from(args.pane_index, args.pane_id),
                    pattern: args.pattern.clone(),
                    max_matches: args.max_matches,
                },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .map_err(|_| ToolError::CommandFailed("Search timed out".into()))
        })?;

        match response {
            PaneResponse::SearchResults(results) => {
                if results.is_empty() {
                    return Ok(format!("No matches found for '{}'", args.pattern));
                }
                let mut output = String::new();
                let mut current_pane = 0;
                for (pane_idx, line_num, text) in &results {
                    if *pane_idx != current_pane {
                        current_pane = *pane_idx;
                        output.push_str(&format!("\n── Pane {} ──\n", pane_idx));
                    }
                    output.push_str(&format!("{:>5}: {}\n", line_num, text));
                }
                Ok(output)
            }
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

// ── wait_for tool ──────────────────────────────────────────────────
//
// Polls a pane until it becomes idle (is_busy == false) or until a
// pattern appears in the pane's recent output. The polling loop lives
// entirely in the tool — the workspace just answers List and ReadContent
// queries as usual.

#[derive(Deserialize)]
pub struct WaitForArgs {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub timeout_secs: Option<u64>,
    pub pattern: Option<String>,
}

#[derive(Serialize)]
pub struct WaitForOutput {
    pub status: String,
    pub output: String,
}

pub struct WaitForTool {
    pane_tx: Sender<PaneRequest>,
    cancel_flag: Arc<AtomicBool>,
}

impl WaitForTool {
    pub fn new(pane_tx: Sender<PaneRequest>, cancel_flag: Arc<AtomicBool>) -> Self {
        Self {
            pane_tx,
            cancel_flag,
        }
    }
}

impl Tool for WaitForTool {
    const NAME: &'static str = "wait_for";
    type Error = ToolError;
    type Args = WaitForArgs;
    type Output = WaitForOutput;

    fn description(&self) -> String {
        "Wait for a terminal pane to become idle or for a specific pattern to appear. Use after launching a command to wait for it to finish. Without a pattern, waits for idle — works universally (shell integration or output quiescence). With a pattern, polls until the text appears. Prefer idle mode (no pattern). Returns status: idle, matched, or timeout. On timeout, read_pane to check progress and call wait_for again if needed.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pane_index": {
                    "type": "integer",
                    "description": "Target pane index from list_panes. Positional only."
                },
                "pane_id": {
                    "type": "integer",
                    "description": "Stable pane id from list_panes. Prefer this for follow-up work."
                },
                "pattern": {
                    "type": "string",
                    "description": "Text to wait for in pane output. If omitted, waits for the pane to become idle — preferred."
                }
            },
            "required": []
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let target = require_pane_target(args.pane_index, args.pane_id, Self::NAME)?;
        let timeout_secs = args.timeout_secs.unwrap_or(30).min(120);

        log::info!(
            "[wait_for] target={} timeout={}s pattern={:?}",
            target.describe(),
            timeout_secs,
            args.pattern
        );

        // Delegate to workspace — it has direct terminal access and runs an
        // async GPUI task with 100ms/500ms polling, no channel roundtrips per tick.
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::WaitFor {
                    target,
                    timeout_secs: Some(timeout_secs),
                    pattern: args.pattern,
                },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        // Block until workspace responds, polling cancel_flag every 200ms
        // for clean shutdown (same pattern as ConHook approval polling).
        let cancel = self.cancel_flag.clone();
        let response = tokio::task::block_in_place(|| {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs + 10);
            loop {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("Cancelled");
                }
                match response_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                    Ok(resp) => return Ok(resp),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        if std::time::Instant::now() >= deadline {
                            return Err("Wait timed out (no response from workspace)");
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        return Err("Pane query channel closed");
                    }
                }
            }
        })
        .map_err(|e| ToolError::CommandFailed(e.into()))?;

        match response {
            PaneResponse::WaitComplete { status, output } => Ok(WaitForOutput { status, output }),
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

/// Decode common escape sequences in key strings.
/// Decode escape sequences in keys strings from LLMs.
///
/// Supports multiple notations that LLMs commonly produce:
/// - Standard: `\n`, `\t`, `\r`, `\\`
/// - Hex with backslash: `\x1b`, `\x03` (preferred notation)
/// - Hex without backslash: `x1b`, `x03` (weaker models omit the backslash)
///
/// The bare-hex fallback (`x1b`) is only triggered when the two hex digits
/// form a valid control character (0x00–0x1F) or DEL (0x7F), so it won't
/// corrupt normal text like "exit" or "maximum".
fn decode_key_escapes(input: &str) -> String {
    let mut result = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'n' => {
                    result.push(b'\n');
                    i += 2;
                }
                b't' => {
                    result.push(b'\t');
                    i += 2;
                }
                b'r' => {
                    result.push(b'\r');
                    i += 2;
                }
                b'\\' => {
                    result.push(b'\\');
                    i += 2;
                }
                b'x' if i + 3 < bytes.len() => {
                    let hex = &input[i + 2..i + 4];
                    if let Ok(byte) = u8::from_str_radix(hex, 16) {
                        result.push(byte);
                        i += 4;
                    } else {
                        result.push(bytes[i]);
                        i += 1;
                    }
                }
                _ => {
                    result.push(bytes[i]);
                    i += 1;
                }
            }
        } else if bytes[i] == b'x' && i + 2 < bytes.len() {
            // Bare hex without backslash: "x1b", "x03", etc.
            // Weaker models (kimi-k2 via Groq) consistently omit the backslash.
            // Only match control characters (0x00-0x1F) and DEL (0x7F)
            // to minimize false positives in normal text.
            let hex = &input[i + 1..i + 3];
            if hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    if byte <= 0x1F || byte == 0x7F {
                        result.push(byte);
                        i += 3;
                        continue;
                    }
                }
            }
            result.push(bytes[i]);
            i += 1;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&result).into_owned()
}

/// Split a decoded key string at standalone-ESC boundaries.
///
/// In VT100 terminals, `ESC` followed by a byte in `0x40..=0x7E` forms a
/// 2-character escape sequence. When an LLM sends `\x1bggdG` intending
/// "ESC (exit insert) then gg then dG", the terminal's VT parser instead
/// consumes `ESC g` as one sequence, eating the ESC and the first `g`.
///
/// This function splits the input so that each standalone ESC (not followed
/// by `[`, `O`, or `P` which start valid multi-byte sequences like CSI, SS3,
/// DCS) becomes its own segment. The caller inserts a brief delay between
/// segments so the VT parser commits ESC as a standalone key.
///
/// Returns a vec of segments. If there are no standalone-ESC boundaries,
/// returns the original string as a single-element vec.
fn split_at_standalone_esc(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return vec![input.to_string()];
    }

    let mut segments: Vec<String> = Vec::new();
    let mut start = 0;

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1B {
            // Check what follows ESC
            let next = bytes.get(i + 1).copied();
            match next {
                // CSI (ESC [), SS3 (ESC O), DCS (ESC P) — valid multi-byte
                // sequence prefixes. Don't split these.
                Some(b'[') | Some(b'O') | Some(b'P') => {
                    i += 1;
                    continue;
                }
                // ESC at end of string — no split needed
                None => {
                    i += 1;
                    continue;
                }
                // Standalone ESC followed by a non-sequence char.
                // Split into: [start..i) as one segment, then ESC alone,
                // then [i+1..] starts the next segment.
                Some(_) => {
                    // Push any accumulated bytes before this ESC
                    if start < i {
                        let seg = String::from_utf8_lossy(&bytes[start..i]).into_owned();
                        segments.push(seg);
                    }
                    // Push ESC as its own segment
                    segments.push("\x1b".to_string());
                    start = i + 1;
                    i = start;
                    continue;
                }
            }
        }
        i += 1;
    }

    // Remaining bytes
    if start < bytes.len() {
        let seg = String::from_utf8_lossy(&bytes[start..]).into_owned();
        segments.push(seg);
    }

    if segments.is_empty() {
        vec![input.to_string()]
    } else {
        segments
    }
}

/// Simple glob matching for filename filtering.
/// Supports `*` (matches any sequence) and `?` (matches one char).
fn glob_match(pattern: &str, text: &str) -> bool {
    let mut p = pattern.chars().peekable();
    let mut t = text.chars().peekable();

    fn inner(
        p: &mut std::iter::Peekable<std::str::Chars>,
        t: &mut std::iter::Peekable<std::str::Chars>,
    ) -> bool {
        while let Some(&pc) = p.peek() {
            match pc {
                '*' => {
                    p.next();
                    while p.peek() == Some(&'*') {
                        p.next();
                    }
                    if p.peek().is_none() {
                        return true;
                    }
                    let mut t_clone = t.clone();
                    let p_save = p.clone();
                    loop {
                        let mut p_try = p_save.clone();
                        let mut t_try = t_clone.clone();
                        if inner(&mut p_try, &mut t_try) {
                            return true;
                        }
                        if t_clone.next().is_none() {
                            return false;
                        }
                    }
                }
                '?' => {
                    p.next();
                    if t.next().is_none() {
                        return false;
                    }
                }
                _ => {
                    p.next();
                    match t.next() {
                        Some(tc) if tc == pc => {}
                        _ => return false,
                    }
                }
            }
        }
        t.peek().is_none()
    }

    inner(&mut p, &mut t)
}

// ── create_pane tool ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct EnsureRemoteShellTargetArgs {
    pub host: String,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    #[serde(default = "default_pane_create_location")]
    pub location: PaneCreateLocation,
    pub startup_command: Option<String>,
}

pub struct EnsureRemoteShellTargetTool {
    pane_tx: Sender<PaneRequest>,
}

impl EnsureRemoteShellTargetTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

#[derive(Serialize)]
pub struct EnsureRemoteTmuxWorkspaceResult {
    pub host: String,
    pub session_name: String,
    pub pane_index: usize,
    pub pane_id: Option<usize>,
    pub created_remote_pane: bool,
    pub remote_match_source: RemoteShellMatchSource,
    pub bootstrap_output: String,
    pub tmux_bootstrap_output: String,
    pub tmux_native_ready: bool,
    pub tmux_snapshot: Option<TmuxSnapshot>,
    pub reason: String,
}

#[derive(Deserialize)]
pub struct EnsureRemoteTmuxWorkspaceArgs {
    pub host: String,
    pub session_name: String,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    #[serde(default = "default_pane_create_location")]
    pub location: PaneCreateLocation,
    pub startup_command: Option<String>,
}

#[derive(Serialize)]
pub struct EnsureRemoteTmuxShellTargetResult {
    pub host: String,
    pub session_name: String,
    pub pane_index: usize,
    pub pane_id: Option<usize>,
    pub created_remote_pane: bool,
    pub remote_match_source: RemoteShellMatchSource,
    pub bootstrap_output: String,
    pub tmux_bootstrap_output: String,
    pub tmux_native_ready: bool,
    pub tmux_snapshot: TmuxSnapshot,
    pub shell_target_created: bool,
    pub shell_target: TmuxPaneInfo,
    pub shell_target_creation: Option<TmuxExecResult>,
    pub reason: String,
}

#[derive(Deserialize)]
pub struct EnsureRemoteTmuxShellTargetArgs {
    pub host: String,
    pub session_name: String,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    #[serde(default = "default_pane_create_location")]
    pub location: PaneCreateLocation,
    pub startup_command: Option<String>,
    pub cwd: Option<String>,
    pub window_name: Option<String>,
    pub shell_command: Option<String>,
    #[serde(default = "default_tmux_shell_location")]
    pub tmux_location: TmuxExecLocation,
    #[serde(default = "default_true")]
    pub detached: bool,
}

pub struct EnsureRemoteTmuxWorkspaceTool {
    pane_tx: Sender<PaneRequest>,
    request_tx: Sender<TerminalExecRequest>,
}

impl EnsureRemoteTmuxWorkspaceTool {
    pub fn new(pane_tx: Sender<PaneRequest>, request_tx: Sender<TerminalExecRequest>) -> Self {
        Self {
            pane_tx,
            request_tx,
        }
    }
}

pub struct EnsureRemoteTmuxShellTargetTool {
    pane_tx: Sender<PaneRequest>,
    request_tx: Sender<TerminalExecRequest>,
}

impl EnsureRemoteTmuxShellTargetTool {
    pub fn new(pane_tx: Sender<PaneRequest>, request_tx: Sender<TerminalExecRequest>) -> Self {
        Self {
            pane_tx,
            request_tx,
        }
    }
}

fn shell_quote_fragment(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "@%_+=:,./-".contains(ch))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalShellMatchSource {
    ExactCwd,
    ReusedShell,
    Created,
}

impl LocalShellMatchSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactCwd => "exact_cwd",
            Self::ReusedShell => "reused_shell",
            Self::Created => "created",
        }
    }
}

#[derive(Serialize)]
pub struct EnsureLocalShellTargetResult {
    pub created: bool,
    pub pane_index: usize,
    pub pane_id: Option<usize>,
    pub cwd: Option<String>,
    pub match_source: LocalShellMatchSource,
    pub command: Option<String>,
    pub output: String,
    pub reason: String,
}

#[derive(Deserialize)]
pub struct EnsureLocalShellTargetArgs {
    pub cwd: Option<String>,
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    #[serde(default)]
    pub create_cwd_if_missing: bool,
    #[serde(default = "default_pane_create_location")]
    pub location: PaneCreateLocation,
}

pub struct EnsureLocalShellTargetTool {
    pane_tx: Sender<PaneRequest>,
}

impl EnsureLocalShellTargetTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }
}

fn ensure_local_shell_target_impl(
    pane_tx: &Sender<PaneRequest>,
    args: EnsureLocalShellTargetArgs,
) -> Result<EnsureLocalShellTargetResult, ToolError> {
    let panes = pane_query_list(pane_tx)?;
    let cwd_lower = normalize_cwd_lower(&args.cwd);

    let exact_match = panes
        .iter()
        .filter(|pane| args.pane_index.is_none_or(|idx| pane.index == idx))
        .filter(|pane| args.pane_id.is_none_or(|id| pane.pane_id == id))
        .filter(|pane| {
            pane_is_local_shell_candidate(pane)
                || pane_has_local_shell_continuity(pane, cwd_lower.as_ref())
        })
        .filter(|pane| pane_matches_cwd(pane, cwd_lower.as_ref()))
        .max_by_key(|pane| 10 + if pane.is_focused { 5 } else { 0 });

    if let Some(pane) = exact_match {
        return Ok(EnsureLocalShellTargetResult {
            created: false,
            pane_index: pane.index,
            pane_id: Some(pane.pane_id),
            cwd: args.cwd,
            match_source: LocalShellMatchSource::ExactCwd,
            command: None,
            output: String::new(),
            reason: format!(
                "Pane {} already exposes a local visible shell at the requested working directory.",
                pane.index
            ),
        });
    }

    if cwd_lower.is_none() {
        let reusable = panes
            .iter()
            .filter(|pane| args.pane_index.is_none_or(|idx| pane.index == idx))
            .filter(|pane| args.pane_id.is_none_or(|id| pane.pane_id == id))
            .filter(|pane| {
                pane_is_local_shell_candidate(pane) || pane_has_local_shell_continuity(pane, None)
            })
            .max_by_key(|pane| 10 + if pane.is_focused { 5 } else { 0 });

        if let Some(pane) = reusable {
            return Ok(EnsureLocalShellTargetResult {
                created: false,
                pane_index: pane.index,
                pane_id: Some(pane.pane_id),
                cwd: None,
                match_source: LocalShellMatchSource::ReusedShell,
                command: None,
                output: String::new(),
                reason: format!(
                    "Pane {} already exposes a reusable local visible shell target.",
                    pane.index
                ),
            });
        }
    }

    let command = args
        .cwd
        .as_ref()
        .map(|cwd| build_local_cwd_command_prefix(cwd, args.create_cwd_if_missing));
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::CreatePane {
                command: command.clone(),
                location: args.location,
            },
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    let response = tokio::task::block_in_place(|| {
        response_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| ToolError::CommandFailed("Create pane request timed out".into()))
    })?;

    let (pane_index, pane_id) = match response {
        PaneResponse::PaneCreated {
            pane_index,
            pane_id,
            ..
        } => (pane_index, pane_id),
        PaneResponse::Error(e) => return Err(ToolError::CommandFailed(e)),
        _ => return Err(ToolError::CommandFailed("Unexpected response".into())),
    };

    Ok(EnsureLocalShellTargetResult {
        created: true,
        pane_index,
        pane_id: Some(pane_id),
        cwd: args.cwd.clone(),
        match_source: LocalShellMatchSource::Created,
        command,
        output: String::new(),
        reason: if let Some(cwd) = args.cwd {
            format!(
                "No reusable local shell target matched `{cwd}`, so con created a new shell pane prepared for that directory."
            )
        } else {
            "No reusable local shell target existed, so con created a new shell pane.".to_string()
        },
    })
}

impl Tool for EnsureLocalShellTargetTool {
    const NAME: &'static str = "ensure_local_shell_target";
    type Error = ToolError;
    type Args = EnsureLocalShellTargetArgs;
    type Output = EnsureLocalShellTargetResult;

    fn description(&self) -> String {
        "Reuse an existing LOCAL visible shell pane when one is already suitable, or create a new one when needed. This is the preferred companion tool for local Codex, Claude Code, or OpenCode workflows: keep the agent CLI in one target, and prepare a separate shell target for file edits, test runs, and other shell commands.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "cwd": {
                    "type": ["string", "null"],
                    "description": "Optional working directory the local shell should be prepared in. If no reusable shell matches it, con creates a new pane and starts it there."
                },
                "pane_index": {
                    "type": ["integer", "null"],
                    "description": "Optional con pane index to constrain reuse checks. Positional only."
                },
                "pane_id": {
                    "type": ["integer", "null"],
                    "description": "Optional stable pane id to constrain reuse checks."
                },
                "location": {
                    "type": "string",
                    "enum": ["right", "down"],
                    "description": "Where to place a newly created shell pane if one is needed. Defaults to right."
                }
            },
            "required": []
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let result = ensure_local_shell_target_impl(&self.pane_tx, args)?;
        let output = if result.created {
            wait_for_pane_initial_output(
                &self.pane_tx,
                selector_from(Some(result.pane_index), result.pane_id),
            )
            .await?
        } else {
            String::new()
        };
        Ok(EnsureLocalShellTargetResult { output, ..result })
    }
}

fn ensure_remote_shell_target_impl(
    pane_tx: &Sender<PaneRequest>,
    args: EnsureRemoteShellTargetArgs,
) -> Result<EnsureRemoteShellTargetResult, ToolError> {
    let host_lower = args.host.to_ascii_lowercase();
    let panes = pane_query_list(pane_tx)?;
    let best_existing = panes
        .into_iter()
        .filter(|pane| args.pane_index.is_none_or(|idx| pane.index == idx))
        .filter(|pane| args.pane_id.is_none_or(|id| pane.pane_id == id))
        .filter(|pane| !pane_is_disconnected_workspace(pane))
        .filter(|pane| !pane_has_tmux_observation(pane))
        .filter_map(|pane| {
            if pane
                .hostname
                .as_ref()
                .is_some_and(|host| host.to_ascii_lowercase().contains(&host_lower))
            {
                Some((
                    30 + if pane.is_focused { 5 } else { 0 },
                    pane.index,
                    pane.pane_id,
                    RemoteShellMatchSource::ProvenHost,
                    format!(
                        "Pane {} already has visible shell execution on proven host `{}`.",
                        pane.index, args.host
                    ),
                ))
            } else if pane
                .remote_workspace
                .as_ref()
                .is_some_and(|anchor| anchor.host.to_ascii_lowercase().contains(&host_lower))
            {
                Some((
                    20 + if pane.is_focused { 5 } else { 0 },
                    pane.index,
                    pane.pane_id,
                    RemoteShellMatchSource::ActionHistory,
                    format!(
                        "Pane {} is a reusable remote SSH workspace for `{}` via {} continuity.",
                        pane.index,
                        args.host,
                        pane.remote_workspace
                            .as_ref()
                            .map(|anchor| anchor.source.as_str())
                            .unwrap_or("action_history")
                    ),
                ))
            } else if pane_recent_ssh_target(&pane)
                .is_some_and(|target| target.to_ascii_lowercase().contains(&host_lower))
            {
                Some((
                    10 + if pane.is_focused { 5 } else { 0 },
                    pane.index,
                    pane.pane_id,
                    RemoteShellMatchSource::ActionHistory,
                    format!(
                        "Pane {} has visible shell execution and recent con action history showing SSH startup for `{}`.",
                        pane.index, args.host
                    ),
                ))
            } else {
                None
            }
        })
        .max_by_key(|candidate| candidate.0);

    if let Some((_, pane_index, pane_id, match_source, reason)) = best_existing {
        return Ok(EnsureRemoteShellTargetResult {
            host: args.host,
            created: false,
            pane_index,
            pane_id: Some(pane_id),
            match_source,
            command: None,
            output: String::new(),
            reason,
        });
    }

    let command = args
        .startup_command
        .unwrap_or_else(|| format!("ssh {}", args.host));
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    pane_tx
        .send(PaneRequest {
            query: PaneQuery::CreatePane {
                command: Some(command.clone()),
                location: args.location,
            },
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

    let response = tokio::task::block_in_place(|| {
        response_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| ToolError::CommandFailed("Create pane request timed out".into()))
    })?;

    let (pane_index, pane_id) = match response {
        PaneResponse::PaneCreated {
            pane_index,
            pane_id,
            ..
        } => (pane_index, pane_id),
        PaneResponse::Error(e) => return Err(ToolError::CommandFailed(e)),
        _ => return Err(ToolError::CommandFailed("Unexpected response".into())),
    };

    Ok(EnsureRemoteShellTargetResult {
        host: args.host.clone(),
        created: true,
        pane_index,
        pane_id: Some(pane_id),
        match_source: RemoteShellMatchSource::Created,
        command: Some(command),
        output: String::new(),
        reason: format!(
            "No reusable SSH pane for `{}` was found, so con created a new pane to connect there.",
            args.host
        ),
    })
}

async fn ensure_remote_tmux_workspace_impl(
    pane_tx: &Sender<PaneRequest>,
    request_tx: &Sender<TerminalExecRequest>,
    args: EnsureRemoteTmuxWorkspaceArgs,
) -> Result<EnsureRemoteTmuxWorkspaceResult, ToolError> {
    let session_name = args.session_name.trim().to_string();
    if session_name.is_empty() {
        return Err(ToolError::CommandFailed(
            "ensure_remote_tmux_workspace requires a non-empty session_name".into(),
        ));
    }

    let ensure = ensure_remote_shell_target_impl(
        pane_tx,
        EnsureRemoteShellTargetArgs {
            host: args.host.clone(),
            pane_index: args.pane_index,
            pane_id: args.pane_id,
            location: args.location,
            startup_command: args.startup_command,
        },
    )?;

    let pane = selector_from(Some(ensure.pane_index), ensure.pane_id);
    let bootstrap_output = if ensure.created {
        wait_for_pane_initial_output(pane_tx, pane).await?
    } else {
        String::new()
    };

    let panes = pane_query_list(pane_tx)?;
    let anchor = panes
        .iter()
        .find(|candidate| candidate.pane_id == ensure.pane_id.unwrap_or(usize::MAX))
        .or_else(|| {
            panes
                .iter()
                .find(|candidate| candidate.index == ensure.pane_index)
        })
        .ok_or_else(|| {
            ToolError::CommandFailed(format!(
                "Remote pane {} disappeared before tmux workspace preparation could continue.",
                ensure.pane_index
            ))
        })?;

    if !pane_has_capability(anchor, PaneControlCapability::ExecVisibleShell) {
        return Err(ToolError::CommandFailed(format!(
            "Pane {} is routed to host `{}` but does not currently expose visible shell execution. Wait for a proven remote shell prompt before preparing tmux there.",
            ensure.pane_index, ensure.host
        )));
    }

    let tmux_command = format!(
        "tmux has-session -t {} 2>/dev/null || tmux new-session -d -s {}",
        shell_quote_fragment(&session_name),
        shell_quote_fragment(&session_name)
    );

    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    request_tx
        .send(TerminalExecRequest {
            command: tmux_command,
            working_dir: None,
            target: pane,
            response_tx,
        })
        .map_err(|_| ToolError::CommandFailed("Terminal exec channel closed".into()))?;

    let tmux_bootstrap = tokio::task::block_in_place(|| {
        response_rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .map_err(|_| {
                ToolError::CommandFailed("Remote tmux workspace bootstrap timed out (60s)".into())
            })
    })?;

    if tmux_bootstrap.exit_code.is_some_and(|code| code != 0) {
        return Err(ToolError::CommandFailed(format!(
            "Failed to prepare tmux session `{}` on host `{}`.\n{}",
            session_name, ensure.host, tmux_bootstrap.output
        )));
    }

    let panes = pane_query_list(pane_tx)?;
    let updated = panes
        .iter()
        .find(|candidate| candidate.pane_id == ensure.pane_id.unwrap_or(usize::MAX))
        .or_else(|| {
            panes
                .iter()
                .find(|candidate| candidate.index == ensure.pane_index)
        })
        .ok_or_else(|| {
            ToolError::CommandFailed(format!(
                "Remote pane {} disappeared after tmux workspace preparation.",
                ensure.pane_index
            ))
        })?;

    let tmux_native_ready = pane_has_capability(updated, PaneControlCapability::QueryTmux);
    let tmux_snapshot = if tmux_native_ready {
        Some(pane_query_tmux_list(pane_tx, pane)?)
    } else {
        None
    };

    let reason = if tmux_native_ready {
        format!(
            "Prepared tmux session `{}` on host `{}` and established a tmux-native control anchor on pane {}.",
            session_name, ensure.host, ensure.pane_index
        )
    } else {
        format!(
            "Prepared tmux session `{}` on host `{}` from pane {}, but tmux-native control is not yet established on that pane. Inspect or probe the pane before relying on tmux-native tools.",
            session_name, ensure.host, ensure.pane_index
        )
    };

    Ok(EnsureRemoteTmuxWorkspaceResult {
        host: ensure.host,
        session_name,
        pane_index: ensure.pane_index,
        pane_id: ensure.pane_id,
        created_remote_pane: ensure.created,
        remote_match_source: ensure.match_source,
        bootstrap_output,
        tmux_bootstrap_output: tmux_bootstrap.output,
        tmux_native_ready,
        tmux_snapshot,
        reason,
    })
}

impl Tool for EnsureRemoteShellTargetTool {
    const NAME: &'static str = "ensure_remote_shell_target";
    type Error = ToolError;
    type Args = EnsureRemoteShellTargetArgs;
    type Output = EnsureRemoteShellTargetResult;

    fn description(&self) -> String {
        "Reuse an existing SSH pane for a specific remote host when one is already available, or create a new pane and connect to that host when none exists. This is the preferred tool for multi-host remote orchestration so the agent does not keep creating duplicate SSH panes across turns. The result includes stable pane_id for follow-up work.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "host": {
                    "type": "string",
                    "description": "SSH host or alias to target, such as haswell or user@host."
                },
                "pane_index": {
                    "type": ["integer", "null"],
                    "description": "Optional con pane index to constrain reuse checks to a specific pane. Positional only."
                },
                "pane_id": {
                    "type": ["integer", "null"],
                    "description": "Optional stable pane id to constrain reuse checks to a specific pane."
                },
                "location": {
                    "type": "string",
                    "enum": ["right", "down"],
                    "description": "Where to place a newly created pane if one is needed. Defaults to right."
                },
                "startup_command": {
                    "type": ["string", "null"],
                    "description": "Optional command to launch when a new pane is created. Defaults to `ssh <host>`."
                }
            },
            "required": ["host"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let result = ensure_remote_shell_target_impl(&self.pane_tx, args)?;
        let output = if result.created {
            wait_for_pane_initial_output(
                &self.pane_tx,
                selector_from(Some(result.pane_index), result.pane_id),
            )
            .await?
        } else {
            String::new()
        };
        Ok(EnsureRemoteShellTargetResult { output, ..result })
    }
}

impl Tool for EnsureRemoteTmuxWorkspaceTool {
    const NAME: &'static str = "ensure_remote_tmux_workspace";
    type Error = ToolError;
    type Args = EnsureRemoteTmuxWorkspaceArgs;
    type Output = EnsureRemoteTmuxWorkspaceResult;

    fn description(&self) -> String {
        "Prepare a reusable REMOTE tmux session from a host shell anchor. con reuses or creates an SSH shell pane for the host, ensures the requested tmux session exists, and returns whether tmux-native control is immediately available from that same pane. Use this when you only need the tmux session/bootstrap fact. If you also need a clean tmux shell target for file work, prefer ensure_remote_tmux_shell_target instead of attaching tmux in the visible pane.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "host": {
                    "type": "string",
                    "description": "SSH host or alias to target, such as haswell or user@host."
                },
                "session_name": {
                    "type": "string",
                    "description": "tmux session name to ensure on that host."
                },
                "pane_index": {
                    "type": ["integer", "null"],
                    "description": "Optional con pane index to constrain SSH reuse checks. Positional only."
                },
                "pane_id": {
                    "type": ["integer", "null"],
                    "description": "Optional stable con pane id to constrain SSH reuse checks."
                },
                "location": {
                    "type": "string",
                    "enum": ["right", "down"],
                    "description": "Where to place a newly created SSH pane if one is needed. Defaults to right."
                },
                "startup_command": {
                    "type": ["string", "null"],
                    "description": "Optional startup command for a newly created remote pane. Defaults to `ssh <host>`."
                }
            },
            "required": ["host", "session_name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        ensure_remote_tmux_workspace_impl(&self.pane_tx, &self.request_tx, args).await
    }
}

impl Tool for EnsureRemoteTmuxShellTargetTool {
    const NAME: &'static str = "ensure_remote_tmux_shell_target";
    type Error = ToolError;
    type Args = EnsureRemoteTmuxShellTargetArgs;
    type Output = EnsureRemoteTmuxShellTargetResult;

    fn description(&self) -> String {
        "Prepare the full REMOTE ssh->tmux shell-work path in one step. con reuses or creates an SSH pane for the host, ensures the requested tmux session exists there, verifies tmux-native control on that same pane, and then reuses or creates a clean tmux shell target for file work. The result includes both a tmux snapshot and the selected shell target, so you can describe the tmux workspace and continue file work without attaching tmux in the visible outer pane unless the user explicitly asks to enter it.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "host": {
                    "type": "string",
                    "description": "SSH host or alias to target, such as haswell or user@host."
                },
                "session_name": {
                    "type": "string",
                    "description": "tmux session name to ensure on that host."
                },
                "pane_index": {
                    "type": ["integer", "null"],
                    "description": "Optional con pane index to constrain SSH reuse checks. Positional only."
                },
                "pane_id": {
                    "type": ["integer", "null"],
                    "description": "Optional stable con pane id to constrain SSH reuse checks."
                },
                "location": {
                    "type": "string",
                    "enum": ["right", "down"],
                    "description": "Where to place a newly created SSH pane if one is needed. Defaults to right."
                },
                "startup_command": {
                    "type": ["string", "null"],
                    "description": "Optional startup command for a newly created remote pane. Defaults to `ssh <host>`."
                },
                "cwd": {
                    "type": ["string", "null"],
                    "description": "Optional path hint for the tmux shell target. Existing shell panes whose tmux cwd contains this value are preferred, and new targets inherit it when created."
                },
                "window_name": {
                    "type": ["string", "null"],
                    "description": "Optional tmux window name for a newly created shell target."
                },
                "shell_command": {
                    "type": ["string", "null"],
                    "description": "Optional command for a newly created tmux shell target. Defaults to an interactive login shell."
                },
                "tmux_location": {
                    "type": "string",
                    "enum": ["new_window", "split_horizontal", "split_vertical"],
                    "description": "Where to create a new tmux shell target if none already exists."
                },
                "detached": {
                    "type": "boolean",
                    "description": "Whether a newly created tmux shell target should start detached. Defaults to true."
                }
            },
            "required": ["host", "session_name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let workspace = ensure_remote_tmux_workspace_impl(
            &self.pane_tx,
            &self.request_tx,
            EnsureRemoteTmuxWorkspaceArgs {
                host: args.host,
                session_name: args.session_name,
                pane_index: args.pane_index,
                pane_id: args.pane_id,
                location: args.location,
                startup_command: args.startup_command,
            },
        )
        .await?;

        if !workspace.tmux_native_ready {
            return Err(ToolError::CommandFailed(format!(
                "{} A tmux-native shell target cannot be prepared yet because the same pane does not currently expose tmux query capability.",
                workspace.reason
            )));
        }

        let pane = selector_from(Some(workspace.pane_index), workspace.pane_id);
        let snapshot = workspace.tmux_snapshot.clone().ok_or_else(|| {
            ToolError::CommandFailed(
                "tmux-native control was reported ready, but no tmux snapshot was returned."
                    .to_string(),
            )
        })?;
        let cwd_contains = args.cwd.as_ref().map(|v| v.to_ascii_lowercase());
        let mut shell_matches = snapshot
            .panes
            .iter()
            .filter(|tmux_pane| {
                tmux_pane.session_name == workspace.session_name
                    && tmux_pane
                        .pane_current_command
                        .as_deref()
                        .is_some_and(is_tmux_shell_command)
                    && cwd_contains.as_ref().is_none_or(|needle| {
                        tmux_pane
                            .pane_current_path
                            .as_deref()
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                            .contains(needle)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        sort_tmux_matches(&mut shell_matches);

        let (shell_target_created, shell_target, shell_target_creation, tmux_snapshot) =
            if let Some(shell_target) = shell_matches.into_iter().next() {
                (false, shell_target, None, snapshot)
            } else {
                let creation_target =
                    tmux_creation_target(&snapshot, &workspace.session_name, args.tmux_location);
                let shell_command = args
                    .shell_command
                    .unwrap_or_else(|| "exec \"${SHELL:-/bin/sh}\" -il".to_string());
                let creation = pane_query_tmux_run_command(
                    &self.pane_tx,
                    pane,
                    creation_target,
                    args.tmux_location,
                    shell_command,
                    args.window_name.clone(),
                    args.cwd.clone(),
                    args.detached,
                )?;
                let tmux_snapshot = pane_query_tmux_list(&self.pane_tx, pane)?;
                let shell_target = tmux_snapshot
                    .panes
                    .iter()
                    .find(|tmux_pane| {
                        tmux_pane.target == creation.target || tmux_pane.pane_id == creation.pane_id
                    })
                    .cloned()
                    .unwrap_or_else(|| TmuxPaneInfo {
                        session_name: creation.session_name.clone(),
                        window_id: creation.window_id.clone(),
                        window_index: creation.window_index.clone(),
                        window_name: creation.window_name.clone(),
                        window_target: creation.window_target.clone(),
                        pane_id: creation.pane_id.clone(),
                        pane_index: creation.pane_index.clone(),
                        target: creation.target.clone(),
                        pane_active: !creation.detached,
                        window_active: !creation.detached,
                        pane_current_command: None,
                        pane_current_path: args.cwd.clone(),
                    });
                (true, shell_target, Some(creation), tmux_snapshot)
            };

        let reason = if shell_target_created {
            format!(
                "{} con then created a dedicated tmux shell target `{}` for remote file work.",
                workspace.reason, shell_target.target
            )
        } else {
            format!(
                "{} con reused the existing tmux shell target `{}` for remote file work.",
                workspace.reason, shell_target.target
            )
        };

        Ok(EnsureRemoteTmuxShellTargetResult {
            host: workspace.host,
            session_name: workspace.session_name,
            pane_index: workspace.pane_index,
            pane_id: workspace.pane_id,
            created_remote_pane: workspace.created_remote_pane,
            remote_match_source: workspace.remote_match_source,
            bootstrap_output: workspace.bootstrap_output,
            tmux_bootstrap_output: workspace.tmux_bootstrap_output,
            tmux_native_ready: true,
            tmux_snapshot,
            shell_target_created,
            shell_target,
            shell_target_creation,
            reason,
        })
    }
}

// ── create_pane tool ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RemoteExecArgs {
    pub hosts: Vec<String>,
    pub command: String,
    #[serde(default = "default_pane_create_location")]
    pub location: PaneCreateLocation,
}

#[derive(Serialize)]
pub struct RemoteExecHostResult {
    pub host: String,
    pub pane_index: usize,
    pub pane_id: Option<usize>,
    pub created: bool,
    pub match_source: RemoteShellMatchSource,
    pub bootstrap_output: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

pub struct RemoteExecTool {
    pane_tx: Sender<PaneRequest>,
    request_tx: Sender<TerminalExecRequest>,
}

impl RemoteExecTool {
    pub fn new(pane_tx: Sender<PaneRequest>, request_tx: Sender<TerminalExecRequest>) -> Self {
        Self {
            pane_tx,
            request_tx,
        }
    }
}

impl Tool for RemoteExecTool {
    const NAME: &'static str = "remote_exec";
    type Error = ToolError;
    type Args = RemoteExecArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Reuse or create SSH workspaces for one or more hosts, then execute the same shell command on all of them in parallel. This is the preferred high-level tool for routine multi-host checks so the model does not need to stitch ensure_remote_shell_target and batch_exec together manually. Each host result includes stable pane_id for follow-up work.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "hosts": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "SSH hosts or aliases to target."
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to run on each host."
                },
                "location": {
                    "type": "string",
                    "enum": ["right", "down"],
                    "description": "Where to place a newly created pane if a host workspace does not exist yet. Defaults to right."
                }
            },
            "required": ["hosts", "command"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut hosts = Vec::new();
        for host in args.hosts {
            let trimmed = host.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !hosts.iter().any(|existing: &String| existing == trimmed) {
                hosts.push(trimmed.to_string());
            }
        }
        if hosts.is_empty() {
            return Err(ToolError::CommandFailed(
                "remote_exec requires at least one non-empty host".into(),
            ));
        }

        let mut prepared = Vec::new();
        for host in hosts {
            let ensure = ensure_remote_shell_target_impl(
                &self.pane_tx,
                EnsureRemoteShellTargetArgs {
                    host,
                    pane_index: None,
                    pane_id: None,
                    location: args.location,
                    startup_command: None,
                },
            )?;
            let bootstrap_output = if ensure.created {
                wait_for_pane_initial_output(
                    &self.pane_tx,
                    selector_from(Some(ensure.pane_index), ensure.pane_id),
                )
                .await?
            } else {
                String::new()
            };
            prepared.push((ensure, bootstrap_output));
        }

        let pane_snapshot = pane_query_list(&self.pane_tx)?;
        let pane_ids = pane_snapshot
            .into_iter()
            .map(|pane| (pane.index, pane.pane_id))
            .collect::<std::collections::HashMap<_, _>>();

        let mut receivers = Vec::with_capacity(prepared.len());
        for (ensure, bootstrap_output) in &prepared {
            let (response_tx, response_rx) = crossbeam_channel::bounded(1);
            self.request_tx
                .send(TerminalExecRequest {
                    command: args.command.clone(),
                    working_dir: None,
                    target: selector_from(Some(ensure.pane_index), ensure.pane_id),
                    response_tx,
                })
                .map_err(|_| ToolError::CommandFailed("Terminal exec channel closed".into()))?;
            receivers.push((
                ensure.host.clone(),
                ensure.pane_index,
                ensure
                    .pane_id
                    .or_else(|| pane_ids.get(&ensure.pane_index).copied()),
                ensure.created,
                ensure.match_source,
                bootstrap_output.clone(),
                response_rx,
            ));
        }

        let results = tokio::task::block_in_place(|| {
            receivers
                .into_iter()
                .map(
                    |(host, pane_index, pane_id, created, match_source, bootstrap_output, rx)| {
                        match rx.recv_timeout(std::time::Duration::from_secs(60)) {
                            Ok(resp) => RemoteExecHostResult {
                                host,
                                pane_index,
                                pane_id,
                                created,
                                match_source,
                                bootstrap_output,
                                output: resp.output,
                                exit_code: resp.exit_code,
                                error: None,
                            },
                            Err(_) => RemoteExecHostResult {
                                host,
                                pane_index,
                                pane_id,
                                created,
                                match_source,
                                bootstrap_output,
                                output: String::new(),
                                exit_code: None,
                                error: Some("Timed out (60s)".into()),
                            },
                        }
                    },
                )
                .collect::<Vec<_>>()
        });

        serde_json::to_value(&results).map_err(|e| ToolError::CommandFailed(e.to_string()))
    }
}

// ── create_pane tool ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreatePaneArgs {
    pub command: Option<String>,
    #[serde(default = "default_pane_create_location")]
    pub location: PaneCreateLocation,
}

#[derive(Serialize)]
pub struct CreatePaneOutput {
    pub pane_index: usize,
    pub pane_id: usize,
    pub command: Option<String>,
    pub location: PaneCreateLocation,
    pub surface_ready: bool,
    pub is_alive: bool,
    pub has_shell_integration: bool,
    /// Initial terminal output after pane creation and command execution.
    /// Gives the model immediate observability — no need for a follow-up read_pane.
    pub output: String,
}

fn default_pane_create_location() -> PaneCreateLocation {
    PaneCreateLocation::Right
}

pub struct CreatePaneTool {
    pane_tx: Sender<PaneRequest>,
}

impl CreatePaneTool {
    pub fn new(pane_tx: Sender<PaneRequest>) -> Self {
        Self { pane_tx }
    }

    /// Wait for the new pane's command to produce meaningful output and settle.
    ///
    /// Two-phase detection:
    ///   Phase 1 (change): Wait for content to CHANGE from the initial command echo.
    ///     The local shell echoes the command immediately, but the actual program
    ///     (SSH handshake, server connection) takes 1-5s to produce output. If we
    ///     settle on the echo, we return before the program responds.
    ///   Phase 2 (stability): Once content changed, wait for it to stabilize.
    ///     The program's output (MOTD, prompt, error) streams in and eventually stops.
    ///
    /// Budget: 30 polls × 500ms = 15s total. Enough for SSH over slow networks.
    async fn wait_for_initial_output(
        &self,
        pane_id: usize,
        initial_surface_ready: bool,
        initial_is_alive: bool,
        initial_has_shell_integration: bool,
    ) -> (String, bool, bool, bool) {
        const POLL_MS: u64 = 500;
        const SETTLE_POLLS: u32 = 3; // 1.5s of stable output after change
        const MAX_POLLS: u32 = 30; // 15s total budget
        const READY_POLLS: u32 = 3;

        let mut initial_snapshot = String::new();
        let mut last_snapshot = String::new();
        let mut stable_count: u32 = 0;
        let mut ready_count: u32 = 0;
        let mut phase_changed = false;
        let mut surface_ready = initial_surface_ready;
        let mut is_alive = initial_is_alive;
        let mut has_shell_integration = initial_has_shell_integration;

        for poll_num in 0..MAX_POLLS {
            tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;

            let (status_tx, status_rx) = crossbeam_channel::bounded(1);
            let status_sent = self.pane_tx.send(PaneRequest {
                query: PaneQuery::CheckBusy {
                    target: selector_from(None, Some(pane_id)),
                },
                response_tx: status_tx,
            });
            if status_sent.is_ok() {
                if let Ok(PaneResponse::BusyStatus {
                    surface_ready: sr,
                    is_alive: alive,
                    is_busy: _,
                    has_shell_integration: si,
                }) = status_rx.recv_timeout(std::time::Duration::from_secs(2))
                {
                    surface_ready = sr;
                    is_alive = alive;
                    has_shell_integration = si;
                }
            }

            let (tx, rx) = crossbeam_channel::bounded(1);
            let sent = self.pane_tx.send(PaneRequest {
                query: PaneQuery::ReadContent {
                    target: selector_from(None, Some(pane_id)),
                    lines: 50,
                },
                response_tx: tx,
            });
            if sent.is_err() {
                break;
            }

            let content = match rx.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(PaneResponse::Content(c)) => c,
                _ => continue,
            };

            // Normalize: trim trailing whitespace per line (cursor position artifacts)
            let normalized: String = content
                .lines()
                .map(|l| l.trim_end())
                .collect::<Vec<_>>()
                .join("\n");

            if normalized.is_empty() {
                if surface_ready && is_alive {
                    ready_count += 1;
                    if ready_count >= READY_POLLS {
                        log::info!(
                            "[create_pane] pane {} became live without immediate output at poll {}",
                            pane_id,
                            poll_num
                        );
                        return (
                            String::new(),
                            surface_ready,
                            is_alive,
                            has_shell_integration,
                        );
                    }
                } else {
                    ready_count = 0;
                }
                continue;
            }

            if !phase_changed {
                // Phase 1: waiting for content to change from initial echo
                if initial_snapshot.is_empty() {
                    // First non-empty snapshot — likely command echo
                    initial_snapshot = normalized.clone();
                    last_snapshot = normalized;
                    log::info!(
                        "[create_pane] phase 1: initial echo captured ({} bytes)",
                        initial_snapshot.len()
                    );
                    continue;
                }

                if normalized != initial_snapshot {
                    // Content changed — the command produced actual output
                    phase_changed = true;
                    last_snapshot = normalized;
                    stable_count = 0;
                    log::info!(
                        "[create_pane] phase 2: content changed at poll {}",
                        poll_num
                    );
                    continue;
                }

                // Content unchanged from echo — command hasn't responded yet
                // (SSH handshake, network latency, etc.)
                continue;
            }

            // Phase 2: content has changed, wait for stability
            if normalized == last_snapshot {
                stable_count += 1;
                if stable_count >= SETTLE_POLLS {
                    log::info!(
                        "[create_pane] output settled at poll {} ({} bytes)",
                        poll_num,
                        normalized.len()
                    );
                    return (normalized, surface_ready, is_alive, has_shell_integration);
                }
            } else {
                last_snapshot = normalized;
                stable_count = 0;
            }
        }

        log::info!(
            "[create_pane] budget exhausted (phase_changed={}), returning ({} bytes)",
            phase_changed,
            last_snapshot.len()
        );
        (
            last_snapshot,
            surface_ready,
            is_alive,
            has_shell_integration,
        )
    }
}

impl Tool for CreatePaneTool {
    const NAME: &'static str = "create_pane";
    type Error = ToolError;
    type Args = CreatePaneArgs;
    type Output = CreatePaneOutput;

    fn description(&self) -> String {
        "Create a new terminal pane (split in current tab). Optionally run a startup command (e.g. \"ssh host\"). The command executes automatically — do NOT re-send it via send_keys. You can choose split placement for better workspace layout. Returns both pane_index and stable pane_id plus the initial terminal output (waits for output to settle). Prefer pane_id for follow-up targeting.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Optional startup command executed automatically in the new pane. Do NOT re-send this command — it already ran."
                },
                "location": {
                    "type": "string",
                    "enum": ["right", "down"],
                    "description": "Where to place the new pane relative to the focused pane. Defaults to right for peer workspaces."
                }
            }
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        log::info!("[create_pane] Tool called, command: {:?}", args.command);
        let has_command = args.command.is_some();
        let (response_tx, response_rx) = crossbeam_channel::bounded(1);
        let command_echo = args.command.clone();
        self.pane_tx
            .send(PaneRequest {
                query: PaneQuery::CreatePane {
                    command: args.command,
                    location: args.location,
                },
                response_tx,
            })
            .map_err(|_| ToolError::CommandFailed("Pane query channel closed".into()))?;

        let response = tokio::task::block_in_place(|| {
            response_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .map_err(|e| {
                    log::error!("[create_pane] Timed out waiting for response: {}", e);
                    ToolError::CommandFailed("Create pane request timed out".into())
                })
        })?;

        match response {
            PaneResponse::PaneCreated {
                pane_index,
                pane_id,
                surface_ready,
                is_alive,
                has_shell_integration,
            } => {
                // If a command was provided (e.g. "ssh host"), wait for initial output
                // to settle so the model can observe the result immediately.
                // This eliminates the need for a follow-up read_pane/wait_for after SSH.
                let (output, surface_ready, is_alive, has_shell_integration) = if has_command {
                    self.wait_for_initial_output(
                        pane_id,
                        surface_ready,
                        is_alive,
                        has_shell_integration,
                    )
                    .await
                } else {
                    (
                        String::new(),
                        surface_ready,
                        is_alive,
                        has_shell_integration,
                    )
                };

                Ok(CreatePaneOutput {
                    pane_index,
                    pane_id,
                    command: command_echo,
                    location: args.location,
                    surface_ready,
                    is_alive,
                    has_shell_integration,
                    output,
                })
            }
            PaneResponse::Error(e) => Err(ToolError::CommandFailed(e)),
            _ => Err(ToolError::CommandFailed("Unexpected response".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentCliNativeAttachmentState, ResolveWorkTargetResult, TmuxTargetKindFilter,
        WorkTargetControlPath, WorkTargetIntent, agent_cli_native_attachment,
        build_local_cwd_command_prefix, canonical_agent_cli_name, decode_key_escapes,
        expand_home_prefix, is_tmux_agent_cli_command, is_tmux_shell_command, pane_matches_cwd,
        resolve_work_target_candidates, split_at_standalone_esc,
    };
    use crate::context::{
        PaneConfidence, PaneEvidenceSource, PaneFrontState, PaneObservationSupport,
        PaneRuntimeScope, PaneScopeKind, RemoteWorkspaceAnchor,
    };
    use crate::control::{
        PaneAddressSpace, PaneControlCapability, PaneVisibleTarget, PaneVisibleTargetKind,
        TmuxControlMode, TmuxControlState,
    };
    use crate::tmux::{TmuxExecLocation, TmuxPaneInfo, TmuxSnapshot};
    use crate::tools::{
        PaneSelector, preferred_tmux_shell_session, shell_quote_fragment, tmux_creation_target,
    };
    use crossbeam_channel::unbounded;

    #[test]
    fn decode_standard_escapes() {
        assert_eq!(decode_key_escapes(r"\n"), "\n");
        assert_eq!(decode_key_escapes(r"\t"), "\t");
        assert_eq!(decode_key_escapes(r"\r"), "\r");
        assert_eq!(decode_key_escapes(r"\\"), "\\");
    }

    #[test]
    fn decode_hex_with_backslash() {
        // \x1b → ESC (0x1B)
        let decoded = decode_key_escapes(r"\x1b");
        assert_eq!(decoded.as_bytes(), &[0x1B]);

        // \x03 → Ctrl-C (0x03)
        let decoded = decode_key_escapes(r"\x03");
        assert_eq!(decoded.as_bytes(), &[0x03]);

        // \x02 → Ctrl-B / tmux prefix (0x02)
        let decoded = decode_key_escapes(r"\x02");
        assert_eq!(decoded.as_bytes(), &[0x02]);
    }

    #[test]
    fn decode_bare_hex_control_chars() {
        // Weaker models (kimi-k2) omit the backslash: "x1b" → ESC
        let decoded = decode_key_escapes("x1b");
        assert_eq!(decoded.as_bytes(), &[0x1B], "bare x1b → ESC");

        let decoded = decode_key_escapes("x03");
        assert_eq!(decoded.as_bytes(), &[0x03], "bare x03 → Ctrl-C");

        // tmux prefix + window number
        let decoded = decode_key_escapes("x02c");
        assert_eq!(decoded.as_bytes(), &[0x02, b'c']);
    }

    #[test]
    fn bare_hex_safe_in_normal_text() {
        // Only control chars (≤0x1F, 0x7F) match — printable hex values don't
        assert_eq!(decode_key_escapes("exit"), "exit"); // 'x' + 'i' is not hex
        assert_eq!(decode_key_escapes("hexdump"), "hexdump"); // 'x' + 'd' + 'u' — 0xdu invalid
        assert_eq!(decode_key_escapes("x41"), "x41"); // 0x41 = 'A', not control
        assert_eq!(decode_key_escapes("maximum"), "maximum");
    }

    #[test]
    fn canonical_agent_cli_names_normalize_aliases() {
        assert_eq!(canonical_agent_cli_name("codex"), Some("codex"));
        assert_eq!(canonical_agent_cli_name("claude-code"), Some("claude"));
        assert_eq!(canonical_agent_cli_name("Claude Code"), Some("claude"));
        assert_eq!(canonical_agent_cli_name("open-code"), Some("opencode"));
        assert_eq!(canonical_agent_cli_name("opencode"), Some("opencode"));
        assert_eq!(canonical_agent_cli_name("unknown"), None);
    }

    #[test]
    fn agent_cli_native_attachment_notes_match_supported_clients() {
        let (state, note) = agent_cli_native_attachment("codex");
        assert_eq!(state, AgentCliNativeAttachmentState::LaunchIntegratedOnly);
        assert!(note.contains("app-server"));

        let (state, note) = agent_cli_native_attachment("opencode");
        assert_eq!(state, AgentCliNativeAttachmentState::LaunchIntegratedOnly);
        assert!(note.contains("server"));

        let (state, _note) = agent_cli_native_attachment("claude");
        assert_eq!(state, AgentCliNativeAttachmentState::Unavailable);
    }

    #[test]
    fn decode_vim_escape_save_sequence() {
        // LLM sends: \x1b:w\n (Escape, :w, Enter)
        let decoded = decode_key_escapes(r"\x1b:w\n");
        assert_eq!(decoded.as_bytes(), &[0x1B, b':', b'w', b'\n']);
    }

    #[test]
    fn decode_bare_hex_vim_sequence() {
        // kimi-k2 sends: x1b:wqx0a (Escape, :wq, newline)
        let decoded = decode_key_escapes("x1b:wqx0a");
        assert_eq!(decoded.as_bytes(), &[0x1B, b':', b'w', b'q', 0x0A]);
    }

    #[test]
    fn decode_mixed_content() {
        // "hello\nworld" → "hello" + newline + "world"
        let decoded = decode_key_escapes(r"hello\nworld");
        assert_eq!(decoded, "hello\nworld");

        // Normal text with no escapes
        assert_eq!(decode_key_escapes("ls -la"), "ls -la");
    }

    #[test]
    fn tmux_shell_command_detection_uses_basename() {
        assert!(is_tmux_shell_command("zsh"));
        assert!(is_tmux_shell_command("/bin/bash"));
        assert!(!is_tmux_shell_command("codex"));
    }

    #[test]
    fn tmux_agent_cli_detection_handles_known_names() {
        assert!(is_tmux_agent_cli_command("codex"));
        assert!(is_tmux_agent_cli_command("/usr/local/bin/opencode"));
        assert!(is_tmux_agent_cli_command("claude-code"));
        assert!(!is_tmux_agent_cli_command("zsh"));
    }

    #[test]
    fn tmux_target_kind_filter_serializes_snake_case() {
        let json = serde_json::to_string(&TmuxTargetKindFilter::AgentCli).expect("serialize");
        assert_eq!(json, "\"agent_cli\"");
    }

    fn test_pane(index: usize, title: &str) -> super::PaneInfo {
        super::PaneInfo {
            index,
            pane_id: index,
            title: title.to_string(),
            cwd: None,
            is_focused: false,
            rows: 24,
            cols: 80,
            surface_ready: true,
            is_alive: true,
            hostname: None,
            hostname_confidence: None,
            hostname_source: None,
            remote_workspace: None,
            front_state: PaneFrontState::Unknown,
            mode: super::PaneMode::Unknown,
            shell_metadata_fresh: false,
            shell_context_fresh: false,
            observation_support: PaneObservationSupport::default(),
            address_space: PaneAddressSpace::ConPane,
            visible_target: PaneVisibleTarget {
                kind: PaneVisibleTargetKind::Unknown,
                label: None,
                host: None,
            },
            target_stack: Vec::new(),
            tmux_control: None,
            control_attachments: Vec::new(),
            control_channels: Vec::new(),
            control_capabilities: Vec::new(),
            control_notes: Vec::new(),
            active_scope: None,
            agent_cli: None,
            evidence: Vec::new(),
            runtime_stack: Vec::new(),
            last_verified_runtime_stack: Vec::new(),
            runtime_warnings: Vec::new(),
            shell_context: None,
            recent_actions: Vec::new(),
            screen_hints: Vec::new(),
            tmux_session: None,
            has_shell_integration: false,
            last_command: None,
            last_exit_code: None,
            is_busy: false,
        }
    }

    #[test]
    fn resolve_work_target_prefers_remote_visible_shell() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut local = test_pane(1, "local");
        local.control_capabilities = vec![PaneControlCapability::ExecVisibleShell];
        local.visible_target.kind = PaneVisibleTargetKind::ShellPrompt;
        local.front_state = PaneFrontState::ShellPrompt;
        local.mode = super::PaneMode::Shell;
        local.shell_metadata_fresh = true;

        let mut remote = test_pane(2, "remote");
        remote.control_capabilities = vec![PaneControlCapability::ExecVisibleShell];
        remote.visible_target.kind = PaneVisibleTargetKind::ShellPrompt;
        remote.front_state = PaneFrontState::ShellPrompt;
        remote.mode = super::PaneMode::Shell;
        remote.shell_metadata_fresh = true;
        remote.hostname = Some("haswell".to_string());
        remote.hostname_confidence = Some(PaneConfidence::Strong);
        remote.hostname_source = Some(PaneEvidenceSource::ShellProbe);
        remote.runtime_stack = vec![PaneRuntimeScope {
            kind: PaneScopeKind::RemoteShell,
            label: Some("haswell".to_string()),
            host: Some("haswell".to_string()),
            confidence: PaneConfidence::Strong,
            evidence_source: PaneEvidenceSource::ShellProbe,
        }];

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![local, remote],
            WorkTargetIntent::RemoteShell,
            None,
            None,
            None,
            None,
            None,
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.pane_index, 2);
        assert_eq!(best.control_path, WorkTargetControlPath::VisibleShellExec);
    }

    #[test]
    fn resolve_work_target_reuses_managed_remote_workspace_without_fresh_shell_metadata() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut remote = test_pane(2, "managed remote");
        remote.remote_workspace = Some(RemoteWorkspaceAnchor {
            host: "haswell".to_string(),
            source: PaneEvidenceSource::ActionHistory,
            confidence: PaneConfidence::Advisory,
            note: "managed".to_string(),
        });
        remote.screen_hints = vec![crate::context::PaneObservationHint {
            kind: crate::context::PaneObservationHintKind::PromptLikeInput,
            confidence: PaneConfidence::Advisory,
            detail: "prompt-like".to_string(),
        }];

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![remote],
            WorkTargetIntent::RemoteShell,
            None,
            None,
            Some("haswell".to_string()),
            None,
            None,
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.pane_index, 2);
        assert_eq!(best.control_path, WorkTargetControlPath::VisibleShellExec);
    }

    #[test]
    fn resolve_work_target_prefers_native_tmux_workspace() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut inspect_only = test_pane(1, "inspect-only tmux");
        inspect_only.tmux_control = Some(TmuxControlState {
            session_name: Some("work".to_string()),
            mode: TmuxControlMode::InspectOnly,
            front_target: None,
            reason: "inspect only".to_string(),
        });
        inspect_only.tmux_session = Some("work".to_string());

        let mut native = test_pane(2, "native tmux");
        native.tmux_control = Some(TmuxControlState {
            session_name: Some("ops".to_string()),
            mode: TmuxControlMode::Native,
            front_target: None,
            reason: "native".to_string(),
        });
        native.tmux_session = Some("ops".to_string());
        native.control_capabilities = vec![PaneControlCapability::QueryTmux];

        let result: ResolveWorkTargetResult = resolve_work_target_candidates(
            &pane_tx,
            vec![inspect_only, native],
            WorkTargetIntent::TmuxWorkspace,
            None,
            None,
            None,
            None,
            None,
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.pane_index, 2);
        assert_eq!(best.control_path, WorkTargetControlPath::TmuxQuery);
        assert_eq!(best.suggested_tool, "tmux_list_targets");
    }

    #[test]
    fn resolve_work_target_recognizes_tmux_like_screen_without_native_control() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut tmux_like = test_pane(2, "haswell ❐ 0 ● 4 zsh");
        tmux_like.remote_workspace = Some(RemoteWorkspaceAnchor {
            host: "haswell".to_string(),
            source: PaneEvidenceSource::ActionHistory,
            confidence: PaneConfidence::Advisory,
            note: "managed".to_string(),
        });
        tmux_like.screen_hints = vec![crate::context::PaneObservationHint {
            kind: crate::context::PaneObservationHintKind::TmuxLikeScreen,
            confidence: PaneConfidence::Advisory,
            detail: "tmux-like".to_string(),
        }];

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![tmux_like],
            WorkTargetIntent::TmuxWorkspace,
            None,
            None,
            None,
            None,
            None,
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.pane_index, 2);
        assert_eq!(best.suggested_tool, "read_pane");
    }

    #[test]
    fn resolve_work_target_surfaces_disconnected_remote_workspace_for_recovery() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut disconnected = test_pane(2, "disconnected");
        disconnected.remote_workspace = Some(RemoteWorkspaceAnchor {
            host: "haswell".to_string(),
            source: PaneEvidenceSource::ActionHistory,
            confidence: PaneConfidence::Advisory,
            note: "managed".to_string(),
        });
        disconnected.screen_hints = vec![crate::context::PaneObservationHint {
            kind: crate::context::PaneObservationHintKind::SshConnectionClosed,
            confidence: PaneConfidence::Advisory,
            detail: "closed".to_string(),
        }];

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![disconnected],
            WorkTargetIntent::RemoteShell,
            None,
            None,
            Some("haswell".to_string()),
            None,
            None,
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.pane_index, 2);
        assert_eq!(best.control_path, WorkTargetControlPath::RemoteShellTarget);
        assert_eq!(best.suggested_tool, "ensure_remote_shell_target");
        assert!(best.requires_preparation);
    }

    #[test]
    fn resolve_work_target_detects_visible_agent_cli() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut agent = test_pane(3, "codex");
        agent.visible_target.kind = PaneVisibleTargetKind::AgentCli;
        agent.agent_cli = Some("codex".to_string());

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![agent],
            WorkTargetIntent::AgentCli,
            None,
            None,
            None,
            None,
            Some("codex".to_string()),
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.pane_index, 3);
        assert_eq!(best.control_path, WorkTargetControlPath::VisibleAgentUi);
        assert_eq!(best.suggested_tool, "agent_cli_turn");
    }

    #[test]
    fn resolve_work_target_agent_cli_can_request_local_agent_preparation() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut shell = test_pane(2, "shell");
        shell.control_capabilities = vec![PaneControlCapability::ExecVisibleShell];
        shell.visible_target.kind = PaneVisibleTargetKind::ShellPrompt;
        shell.front_state = PaneFrontState::ShellPrompt;
        shell.mode = super::PaneMode::Shell;
        shell.shell_metadata_fresh = true;

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![shell],
            WorkTargetIntent::AgentCli,
            None,
            None,
            None,
            None,
            Some("codex".to_string()),
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.pane_index, 2);
        assert_eq!(best.control_path, WorkTargetControlPath::LocalAgentTarget);
        assert_eq!(best.suggested_tool, "ensure_local_agent_target");
        assert!(best.requires_preparation);
    }

    #[test]
    fn resolve_work_target_visible_shell_can_request_local_shell_preparation() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut agent = test_pane(3, "codex");
        agent.visible_target.kind = PaneVisibleTargetKind::AgentCli;
        agent.agent_cli = Some("codex".to_string());

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![agent],
            WorkTargetIntent::VisibleShell,
            None,
            None,
            None,
            None,
            Some("codex".to_string()),
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.pane_index, 3);
        assert_eq!(best.control_path, WorkTargetControlPath::LocalShellTarget);
        assert_eq!(best.suggested_tool, "ensure_local_shell_target");
        assert!(best.requires_preparation);
    }

    #[test]
    fn resolve_work_target_visible_shell_does_not_reuse_recent_local_agent_pane() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut pane = test_pane(4, "con-bench");
        pane.control_capabilities = vec![PaneControlCapability::ExecVisibleShell];
        pane.visible_target.kind = PaneVisibleTargetKind::ShellPrompt;
        pane.front_state = PaneFrontState::ShellPrompt;
        pane.mode = super::PaneMode::Shell;
        pane.shell_metadata_fresh = true;
        pane.screen_hints.push(crate::context::PaneObservationHint {
            kind: crate::context::PaneObservationHintKind::PromptLikeInput,
            confidence: crate::context::PaneConfidence::Advisory,
            detail: "Prompt-like line is visible.".to_string(),
        });
        pane.recent_actions.push(crate::context::PaneActionRecord {
            sequence: 1,
            kind: crate::context::PaneActionKind::VisibleShellExec,
            summary: "con executed `cd ~/dev/temp/con-bench && codex` in the visible shell"
                .to_string(),
            command: Some("cd ~/dev/temp/con-bench && codex".to_string()),
            source: crate::context::PaneEvidenceSource::ActionHistory,
            confidence: crate::context::PaneConfidence::Advisory,
            input_generation: Some(5),
            note: None,
        });

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![pane],
            WorkTargetIntent::VisibleShell,
            None,
            None,
            None,
            Some("~/dev/temp/con-bench".to_string()),
            Some("codex".to_string()),
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.control_path, WorkTargetControlPath::LocalShellTarget);
        assert_eq!(best.suggested_tool, "ensure_local_shell_target");
        assert!(best.requires_preparation);
    }

    #[test]
    fn resolve_work_target_agent_cli_does_not_reuse_continuity_only_shell_pane() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut pane = test_pane(5, "con-bench-codex");
        pane.control_capabilities = vec![PaneControlCapability::ExecVisibleShell];
        pane.visible_target.kind = PaneVisibleTargetKind::ShellPrompt;
        pane.front_state = PaneFrontState::ShellPrompt;
        pane.mode = super::PaneMode::Shell;
        pane.shell_metadata_fresh = true;
        pane.screen_hints.push(crate::context::PaneObservationHint {
            kind: crate::context::PaneObservationHintKind::PromptLikeInput,
            confidence: crate::context::PaneConfidence::Advisory,
            detail: "Prompt-like line is visible.".to_string(),
        });
        pane.recent_actions.push(crate::context::PaneActionRecord {
            sequence: 1,
            kind: crate::context::PaneActionKind::PaneCreated,
            summary: "con created this pane with startup command".to_string(),
            command: Some(
                "mkdir -p /Users/weyl/dev/temp/con-bench-codex-git && cd /Users/weyl/dev/temp/con-bench-codex-git && codex"
                    .to_string(),
            ),
            source: crate::context::PaneEvidenceSource::ActionHistory,
            confidence: crate::context::PaneConfidence::Advisory,
            input_generation: None,
            note: None,
        });

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![pane],
            WorkTargetIntent::AgentCli,
            None,
            None,
            None,
            Some("/Users/weyl/dev/temp/con-bench-codex-git".to_string()),
            Some("codex".to_string()),
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.control_path, WorkTargetControlPath::LocalAgentTarget);
        assert_eq!(best.suggested_tool, "ensure_local_agent_target");
        assert!(best.requires_preparation);
    }

    #[test]
    fn resolve_work_target_agent_cli_reuses_con_launched_non_shell_pane() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut pane = test_pane(5, "con-bench-codex");
        pane.is_busy = true;
        pane.recent_actions.push(crate::context::PaneActionRecord {
            sequence: 1,
            kind: crate::context::PaneActionKind::PaneCreated,
            summary: "con created this pane with startup command".to_string(),
            command: Some(
                "mkdir -p /Users/weyl/dev/temp/con-bench-codex-git && cd /Users/weyl/dev/temp/con-bench-codex-git && codex"
                    .to_string(),
            ),
            source: crate::context::PaneEvidenceSource::ActionHistory,
            confidence: crate::context::PaneConfidence::Advisory,
            input_generation: None,
            note: None,
        });

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![pane],
            WorkTargetIntent::AgentCli,
            None,
            None,
            None,
            Some("/Users/weyl/dev/temp/con-bench-codex-git".to_string()),
            Some("codex".to_string()),
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.control_path, WorkTargetControlPath::VisibleAgentUi);
        assert_eq!(best.suggested_tool, "agent_cli_turn");
        assert!(!best.requires_preparation);
    }

    #[test]
    fn resolve_work_target_agent_cli_reuses_dedicated_lane_despite_stale_shell_metadata() {
        let (pane_tx, _pane_rx) = unbounded();
        let mut pane = test_pane(6, "con-bench-codex");
        pane.control_capabilities = vec![PaneControlCapability::ExecVisibleShell];
        pane.front_state = PaneFrontState::ShellPrompt;
        pane.visible_target.kind = PaneVisibleTargetKind::ShellPrompt;
        pane.recent_actions.push(crate::context::PaneActionRecord {
            sequence: 1,
            kind: crate::context::PaneActionKind::PaneCreated,
            summary: "con created this pane with startup command".to_string(),
            command: Some(
                "mkdir -p /Users/weyl/dev/temp/con-bench-codex-git && cd /Users/weyl/dev/temp/con-bench-codex-git && codex"
                    .to_string(),
            ),
            source: crate::context::PaneEvidenceSource::ActionHistory,
            confidence: crate::context::PaneConfidence::Advisory,
            input_generation: None,
            note: None,
        });

        let result = resolve_work_target_candidates(
            &pane_tx,
            vec![pane],
            WorkTargetIntent::AgentCli,
            None,
            None,
            None,
            Some("/Users/weyl/dev/temp/con-bench-codex-git".to_string()),
            Some("codex".to_string()),
            8,
        )
        .expect("resolve");

        let best = result.best_match.expect("best");
        assert_eq!(best.control_path, WorkTargetControlPath::VisibleAgentUi);
        assert_eq!(best.suggested_tool, "agent_cli_turn");
        assert!(!best.requires_preparation);
    }

    #[test]
    fn pane_matches_cwd_uses_workspace_hint_when_live_cwd_is_stale() {
        let mut pane = test_pane(2, "con-bench-twosum");
        pane.cwd = Some("/Users/weyl/conductor/workspaces/con/kingston".to_string());
        pane.recent_actions.push(crate::context::PaneActionRecord {
            sequence: 1,
            kind: crate::context::PaneActionKind::PaneCreated,
            summary: "con created this pane with startup command".to_string(),
            command: Some(
                "mkdir -p /Users/weyl/dev/temp/con-bench-twosum && cd /Users/weyl/dev/temp/con-bench-twosum && codex"
                    .to_string(),
            ),
            source: crate::context::PaneEvidenceSource::ActionHistory,
            confidence: crate::context::PaneConfidence::Advisory,
            input_generation: None,
            note: None,
        });

        assert!(
            pane_matches_cwd(
                &pane,
                Some(&"/users/weyl/dev/temp/con-bench-twosum".to_string())
            ),
            "workspace hint from action history should outrank stale live cwd"
        );
    }

    #[test]
    fn expand_home_prefix_rewrites_tilde_paths() {
        let home = dirs::home_dir().expect("home");
        assert_eq!(
            expand_home_prefix("~/dev/temp"),
            home.join("dev/temp").to_string_lossy()
        );
        assert_eq!(expand_home_prefix("~"), home.to_string_lossy());
    }

    #[test]
    fn build_local_cwd_command_prefix_can_create_missing_directory() {
        let home = dirs::home_dir().expect("home");
        let quoted = shell_quote_fragment(home.join("dev/temp").to_string_lossy().as_ref());
        assert_eq!(
            build_local_cwd_command_prefix("~/dev/temp", true),
            format!("mkdir -p {quoted} && cd {quoted}")
        );
        assert_eq!(
            build_local_cwd_command_prefix("~/dev/temp", false),
            format!("cd {quoted}")
        );
    }

    #[test]
    fn tmux_creation_target_prefers_requested_session() {
        let snapshot = TmuxSnapshot {
            panes: vec![
                TmuxPaneInfo {
                    session_name: "benchmark".to_string(),
                    window_id: "@16".to_string(),
                    window_index: "4".to_string(),
                    window_name: "zsh".to_string(),
                    window_target: "benchmark:4".to_string(),
                    pane_id: "%24".to_string(),
                    pane_index: "1".to_string(),
                    target: "benchmark:4.1".to_string(),
                    pane_active: true,
                    window_active: true,
                    pane_current_command: Some("zsh".to_string()),
                    pane_current_path: Some("/home/w".to_string()),
                },
                TmuxPaneInfo {
                    session_name: "con-bench".to_string(),
                    window_id: "@48".to_string(),
                    window_index: "1".to_string(),
                    window_name: "zsh".to_string(),
                    window_target: "con-bench:1".to_string(),
                    pane_id: "%59".to_string(),
                    pane_index: "1".to_string(),
                    target: "con-bench:1.1".to_string(),
                    pane_active: true,
                    window_active: true,
                    pane_current_command: Some("zsh".to_string()),
                    pane_current_path: Some("/home/w".to_string()),
                },
            ],
        };

        assert_eq!(
            tmux_creation_target(&snapshot, "con-bench", TmuxExecLocation::NewWindow),
            Some("con-bench".to_string())
        );
        assert_eq!(
            tmux_creation_target(&snapshot, "con-bench", TmuxExecLocation::SplitHorizontal),
            Some("con-bench:1.1".to_string())
        );
    }

    #[test]
    fn preferred_tmux_shell_session_defaults_to_pane_workspace_session() {
        let mut pane = test_pane(2, "tmux shell");
        pane.tmux_control = Some(TmuxControlState {
            session_name: Some("con-bench".to_string()),
            mode: TmuxControlMode::Native,
            front_target: None,
            reason: "native".to_string(),
        });

        assert_eq!(
            preferred_tmux_shell_session(&[pane], PaneSelector::new(Some(2), Some(2)), None),
            Some("con-bench".to_string())
        );
        assert_eq!(
            preferred_tmux_shell_session(
                &[test_pane(2, "tmux shell")],
                PaneSelector::new(Some(2), Some(2)),
                Some("benchmark"),
            ),
            Some("benchmark".to_string())
        );
    }

    // ── split_at_standalone_esc tests ────────────────────────────────

    #[test]
    fn split_esc_before_printable() {
        // ESC followed by 'g' — must split so ESC is processed alone
        let input = "\x1bggdG";
        let segments = split_at_standalone_esc(input);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0], "\x1b");
        assert_eq!(segments[1], "ggdG");
    }

    #[test]
    fn no_split_for_csi_sequence() {
        // ESC [ A is a CSI Up arrow — should NOT split
        let input = "\x1b[A";
        let segments = split_at_standalone_esc(input);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0], "\x1b[A");
    }

    #[test]
    fn no_split_for_ss3_sequence() {
        // ESC O P is an SS3 F1 key — should NOT split
        let input = "\x1bOP";
        let segments = split_at_standalone_esc(input);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0], "\x1bOP");
    }

    #[test]
    fn split_esc_colon_w() {
        // ESC :w\n — common vim "exit insert + save" sequence
        let input = "\x1b:w\n";
        let segments = split_at_standalone_esc(input);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0], "\x1b");
        assert_eq!(segments[1], ":w\n");
    }

    #[test]
    fn no_split_for_plain_text() {
        let segments = split_at_standalone_esc("hello world");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0], "hello world");
    }

    #[test]
    fn esc_at_end_no_split() {
        // ESC at the very end — no following char, no need to split
        let input = "hello\x1b";
        let segments = split_at_standalone_esc(input);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0], "hello\x1b");
    }

    #[test]
    fn multiple_standalone_esc() {
        // ESC gg dG ESC :wq\n — two standalone ESCs
        let input = "\x1bggdG\x1b:wq\n";
        let segments = split_at_standalone_esc(input);
        assert_eq!(segments, vec!["\x1b", "ggdG", "\x1b", ":wq\n"]);
    }

    #[test]
    fn mixed_standalone_and_csi_esc() {
        // ESC (standalone) then gg then ESC[A (CSI Up arrow)
        let input = "\x1bgg\x1b[A";
        let segments = split_at_standalone_esc(input);
        assert_eq!(segments, vec!["\x1b", "gg\x1b[A"]);
    }
}
