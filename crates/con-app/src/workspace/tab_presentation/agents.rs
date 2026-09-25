/// Map a cached agent-CLI classification to its brand logo.
///
/// Unknown values and `None` fall through so the existing SSH / AI /
/// heuristic icon path can run.
pub(in crate::workspace) fn agent_cli_icon(agent_cli: Option<&str>) -> Option<&'static str> {
    match agent_cli? {
        "codex" => Some("agents/codex.svg"),
        "claude" => Some("agents/claude.svg"),
        "opencode" => Some("agents/opencode.svg"),
        "gemini" => Some("agents/gemini.svg"),
        "herdr" => Some("agents/herdr.svg"),
        "kimi" => Some("agents/kimi.svg"),
        "cursor" => Some("agents/cursor.svg"),
        "grok" => Some("agents/grok.svg"),
        "pi" => Some("agents/pi.svg"),
        "mimo" => Some("agents/mimo.svg"),
        "qoder" => Some("agents/qoder.svg"),
        "droid" => Some("agents/droid.svg"),
        "hermes" => Some("agents/hermes.svg"),
        "kiro" => Some("agents/kiro.svg"),
        "cline" => Some("agents/cline.svg"),
        "qwen" => Some("agents/qwen.svg"),
        "amp" => Some("agents/amp.svg"),
        "kilo" => Some("agents/kilo.svg"),
        "goose" => Some("agents/goose.svg"),
        _ => None,
    }
}

/// Map a foreground process name to a known interactive agent/TUI.
///
/// Screen-text classification (`classify_screen_agent_cli`) cannot see
/// these TUIs: some never print a stable brand marker, and others ship
/// as interpreter scripts whose process name is just `node`/`python3`.
/// A native binary's foreground process name is the most reliable
/// signal we have.
///
/// `grok` is matched by prefix because its binary carries the version,
/// e.g. `grok-1.0.34-macos-aarch64`.
pub(in crate::workspace) fn agent_from_process_name(name: &str) -> Option<&'static str> {
    let name = name.trim();
    if name.starts_with("grok-") {
        return Some("grok");
    }
    match name {
        "codex" => Some("codex"),
        "cursor-agent" => Some("cursor"),
        "opencode" => Some("opencode"),
        "kimi-code" => Some("kimi"),
        "pi" => Some("pi"),
        "gemini" => Some("gemini"),
        "copilot" => Some("copilot"),
        "herdr" => Some("herdr"),
        "kimi" => Some("kimi"),
        "mimo" => Some("mimo"),
        "droid" => Some("droid"),
        "kiro-cli" | "kiro" => Some("kiro"),
        "crush" => Some("crush"),
        "goose" => Some("goose"),
        "amp" | "amp.exe" => Some("amp"),
        _ => None,
    }
}

/// A foreground shell is an authoritative negative signal: old agent banners
/// can remain visible after the TUI exits and must not keep the tab branded.
pub(in crate::workspace) fn process_name_is_shell(name: &str) -> bool {
    let name = name.trim().trim_start_matches('-').to_ascii_lowercase();
    matches!(
        name.as_str(),
        "sh" | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "nu"
            | "elvish"
            | "xonsh"
            | "ksh"
            | "csh"
            | "tcsh"
            | "pwsh"
            | "pwsh.exe"
            | "powershell"
            | "powershell.exe"
            | "cmd"
            | "cmd.exe"
    )
}

/// Map an OSC-set terminal title to a known agent CLI.
///
/// Some CLIs announce themselves only through the window title, and
/// their visible screen changes too much to anchor on. `grok` titles
/// itself `grok` (or `<session> - grok`); `pi` prefixes its title with
/// the `π` glyph; qwen/crush/kilo ship as interpreter scripts and put
/// their product name (plus the current directory) in the title.
pub(in crate::workspace) fn agent_from_osc_title(title: Option<&str>) -> Option<&'static str> {
    let title = title?.trim();
    if title.is_empty() {
        return None;
    }
    let lower = title.to_ascii_lowercase();
    if lower == "grok" || lower.ends_with(" - grok") {
        return Some("grok");
    }
    if title.contains('π') {
        return Some("pi");
    }
    if lower == "qwen" || lower.starts_with("qwen - ") || lower.starts_with("qwen code") {
        return Some("qwen");
    }
    if lower == "crush" || lower.starts_with("crush ") {
        return Some("crush");
    }
    if lower == "kilo" || lower.starts_with("kilo cli") || lower.starts_with("kilo code") {
        return Some("kilo");
    }
    if lower.contains(" - amp - ") {
        return Some("amp");
    }
    None
}

/// Map visible screen text to a known agent CLI.
///
/// Complements `classify_screen_agent_cli` (codex / claude / opencode)
/// with CLIs that ship as interpreter scripts, where the process name
/// carries no information. Markers are brand strings that survive
/// version bumps; version numbers and paths stay out of the patterns.
pub(in crate::workspace) fn agent_from_screen_text(lines: &[String]) -> Option<&'static str> {
    let joined = lines.join("\n").to_ascii_lowercase();
    if joined.contains("kimi code") {
        return Some("kimi");
    }
    if joined.contains("cursor agent") {
        return Some("cursor");
    }
    if joined.contains("grok build") {
        return Some("grok");
    }
    // Pi's prompt footer is stable while the session runs; the banner
    // ("pi v0.85.1") scrolls away, so anchor on the footer hints too.
    if joined.contains("escape interrupt") && joined.contains("ctrl+o more") {
        return Some("pi");
    }
    if joined.contains("mimocode") || joined.contains("mimo code") {
        return Some("mimo");
    }
    if joined.contains("welcome to qoder cli") {
        return Some("qoder");
    }
    if joined.contains("factory's ai coding agent") {
        return Some("droid");
    }
    if joined.contains("hermes agent") {
        return Some("hermes");
    }
    if joined.contains("welcome to kiro cli") {
        return Some("kiro");
    }
    if joined.contains("gemini cli") {
        return Some("gemini");
    }
    // Cline ships as a node script whose title carries no brand; its
    // permission prompts and question dialogs do.
    if joined.contains("cline needs permission")
        || joined.contains("cline is asking a question")
        || joined.contains("let cline use this tool")
    {
        return Some("cline");
    }
    None
}

/// Whether the cached classification should be replaced.
pub(in crate::workspace) fn next_agent_cli(
    previous: Option<&'static str>,
    detected: Option<&'static str>,
) -> Option<Option<&'static str>> {
    (previous != detected).then_some(detected)
}

pub(super) const AGENT_CLI_SCREEN_SCAN_ATTEMPTS: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) struct AgentCliObservation {
    pub(in crate::workspace) terminal_id: u64,
    pub(in crate::workspace) foreground_process_group_id: Option<u64>,
    pub(in crate::workspace) title_agent: Option<&'static str>,
    pub(in crate::workspace) input_generation: u64,
}

/// Per-tab state for bounded agent detection.
///
/// Reading a terminal's visible screen is materially more expensive than
/// observing its process, title, or input generation. A changed observation
/// opens a short retry window so script-based TUIs can finish their initial
/// paint; a stable tab performs no screen reads after that window closes.
#[derive(Default)]
pub(in crate::workspace) struct AgentCliDetectionState {
    observation: Option<AgentCliObservation>,
    screen_scan_attempts_remaining: u8,
}

impl AgentCliDetectionState {
    pub(in crate::workspace) fn terminal_changed(&self, terminal_id: u64) -> bool {
        self.observation
            .as_ref()
            .is_some_and(|previous| previous.terminal_id != terminal_id)
    }

    pub(in crate::workspace) fn observe(&mut self, observation: AgentCliObservation) -> bool {
        if self.observation.as_ref() == Some(&observation) {
            return false;
        }
        self.observation = Some(observation);
        self.screen_scan_attempts_remaining = AGENT_CLI_SCREEN_SCAN_ATTEMPTS;
        true
    }

    pub(in crate::workspace) fn take_screen_scan_attempt(&mut self) -> bool {
        if self.screen_scan_attempts_remaining == 0 {
            return false;
        }
        self.screen_scan_attempts_remaining -= 1;
        true
    }

    pub(in crate::workspace) fn finish(&mut self) {
        self.screen_scan_attempts_remaining = 0;
    }

    pub(in crate::workspace) fn is_exhausted(&self) -> bool {
        self.screen_scan_attempts_remaining == 0
    }
}

/// Pump-path throttle: refresh immediately on the first call, then at
/// most once per `min_interval`.
pub(in crate::workspace) fn should_refresh_agent_cli(
    last_refresh: Option<std::time::Instant>,
    now: std::time::Instant,
    min_interval: std::time::Duration,
) -> bool {
    last_refresh.is_none_or(|last| now.saturating_duration_since(last) >= min_interval)
}
