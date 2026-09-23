//! Background AI engine that produces a short label + curated icon
//! for each open terminal tab.
//!
//! Why this exists
//! ---
//! Tab presentation needs a compact name + icon for every tab. The
//! cheap path is `parse_focused_process(title)` — fast, deterministic,
//! never needs the network — but it caps out at things the OSC title
//! actually surfaces (`vim README.md`, `htop`, `ssh host`). It can't
//! tell that a `bash` shell that just ran `cargo test` is "Test run"
//! or that a directory full of `.tf` files is "Terraform".
//!
//! This engine asks the user's already-configured **suggestion model**
//! (the same fast/cheap model the inline shell-completion engine
//! uses) for a 1–3 word label and one icon from a closed set. It's
//! gated by `agent.suggestion_model.enabled` — the same toggle —
//! so users who turned that off get no extra LLM traffic.
//!
//! Contract with the model
//! ---
//! The prompt asks for a JSON object shaped like
//! `{"label":"...","icon":"..."}` with `icon` constrained to one of
//! the six [`TabIconKind`] variants. Anything that doesn't parse
//! cleanly is dropped on the floor — the panel falls back to the
//! heuristic name and we never silently render a garbage label.
//!
//! Throttling
//! ---
//! - Per-tab cache keyed on `(cwd, top-3-recent-commands, title)` so
//!   we don't re-ask while context hasn't moved.
//! - At most one in-flight request per tab.
//! - At most one request per 5 s per tab as a budget guard against
//!   chatty PROMPT_COMMAND cwd updates.

use con_agent::AgentConfig;
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

/// Minimum gap between two LLM requests for the same tab when the
/// previous one *succeeded*. Holds chatty PROMPT_COMMAND cwd updates
/// in check during steady-state.
const PER_TAB_REQUEST_BUDGET: Duration = Duration::from_secs(5);
/// Same idea but for the post-failure retry path. Shorter so a flaky
/// upstream (rate limit / empty response from a thinking model)
/// doesn't lock the tab out for several seconds.
const PER_TAB_RETRY_BUDGET: Duration = Duration::from_millis(750);
/// Cap on the model's label string. Sanity bound; the model is
/// instructed to stay under ~24 chars.
const LABEL_MAX_LEN: usize = 32;

/// The closed set of icons the model is allowed to choose from.
///
/// This **is** the icon vocabulary for tab presentation. The keyword
/// the model returns is mapped to a Phosphor SVG path by
/// [`TabIconKind::svg_path`]. No emoji. No free-form image references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabIconKind {
    Terminal,
    Code,
    Pulse,
    BookOpen,
    FileCode,
    Globe,
}

impl TabIconKind {
    /// Phosphor SVG path under `assets/icons/phosphor/`. Stable across
    /// callers so tests and rendering agree.
    pub fn svg_path(&self) -> &'static str {
        match self {
            TabIconKind::Terminal => "phosphor/terminal.svg",
            TabIconKind::Code => "phosphor/code.svg",
            TabIconKind::Pulse => "phosphor/pulse.svg",
            TabIconKind::BookOpen => "phosphor/book-open.svg",
            TabIconKind::FileCode => "phosphor/file-code.svg",
            TabIconKind::Globe => "phosphor/globe.svg",
        }
    }

    fn from_keyword(s: &str) -> Option<TabIconKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "terminal" | "shell" | "console" => Some(TabIconKind::Terminal),
            "code" | "editor" | "vim" | "nvim" | "emacs" | "nano" | "helix" | "kakoune" => {
                Some(TabIconKind::Code)
            }
            "pulse" | "monitor" | "htop" | "top" | "btop" => Some(TabIconKind::Pulse),
            "book" | "book-open" | "pager" | "less" | "more" | "man" => Some(TabIconKind::BookOpen),
            "file" | "file-code" | "git" | "lazygit" | "tig" => Some(TabIconKind::FileCode),
            "globe" | "ssh" | "remote" | "network" => Some(TabIconKind::Globe),
            _ => None,
        }
    }
}

/// Result the engine pushes back to the workspace once the model
/// responds.
#[derive(Debug, Clone)]
pub struct TabSummary {
    pub tab_id: u64,
    pub label: String,
    pub icon: TabIconKind,
}

/// Shape we expect the model to emit (as JSON). The completion
/// path uses the streaming text API and we deserialize this
/// ourselves with a tolerant parser that handles markdown code
/// fences and reasoning preamble.
#[derive(Debug, Clone, Deserialize)]
struct TabSummaryJson {
    label: String,
    icon: String,
}

/// Inputs the engine consults when building a prompt. Workspace fills
/// this from the focused terminal pane's live state.
#[derive(Debug, Clone, Default)]
pub struct TabSummaryRequest {
    /// Stable identifier for the tab (the workspace uses tab index +
    /// monotonic counter so reorders don't confuse the cache).
    pub tab_id: u64,
    pub cwd: Option<String>,
    /// Most recent shell commands the user explicitly executed via
    /// the input bar / control socket. Empty when the user types
    /// directly into the terminal pane (we don't intercept those).
    /// Truncated to 3 by the engine.
    pub recent_commands: Vec<String>,
    /// Tail of the visible terminal scrollback — the same lines the
    /// user can see right now in the pane. This is the strongest
    /// signal we have for "what is this tab actually doing", because
    /// it works the same whether commands came from the input bar,
    /// the agent, the control socket, or the user typing directly.
    /// Truncated to 12 lines by the engine.
    pub recent_output: Vec<String>,
    /// Live OSC title of the focused terminal.
    pub title: Option<String>,
    /// SSH hostname if known. Pre-empts the LLM call: SSH tabs are
    /// always labelled by host with a globe icon, no model needed.
    pub ssh_host: Option<String>,
    /// Interactive agent CLI detected in the focused terminal. Such TUIs
    /// redraw status lines and title spinners continuously, so the cache
    /// key ignores that volatile surface.
    pub agent_cli: Option<&'static str>,
}

#[derive(Default)]
struct PerTabState {
    /// Per-tab invalidation counter. Incremented when the workspace
    /// knows the visible context changed enough that an in-flight
    /// request must not write back.
    generation: u64,
    /// Hash key of the last request that *succeeded*. We dedupe on
    /// this — if the context hasn't moved since the last accepted
    /// label, no need to re-ask. Failures DO NOT update this so a
    /// future call with the same context will retry instead of
    /// being silently locked out (the bug in the previous version).
    last_success_key: Option<u64>,
    /// When the last request was dispatched (success or failure).
    /// Used by the budget gates below.
    last_dispatch: Option<Instant>,
    /// `true` if the last completed request returned a usable
    /// summary. False after the engine drops a malformed / empty
    /// response. Distinguishes the success-budget gate from the
    /// (much shorter) retry-budget gate.
    last_was_success: bool,
    /// `true` while a request is in flight; prevents duplicate work.
    in_flight: bool,
    /// Set when a tab context changes while a request is already in
    /// flight. We keep `in_flight` true to cap model work at one
    /// request per tab, then make the next poll eligible to dispatch
    /// immediately once the stale worker exits.
    refresh_after_in_flight: bool,
}

/// Background engine. Cheap to construct; safe to call `request` from
/// the GPUI render path (work fans out to a tokio runtime, results
/// arrive on a `crossbeam` channel that the caller drains).
pub struct TabSummaryEngine {
    state: Arc<Mutex<HashMap<u64, PerTabState>>>,
    cache_generation: Arc<AtomicU64>,
    config: AgentConfig,
    runtime: Arc<Runtime>,
}

impl TabSummaryEngine {
    pub fn new(config: AgentConfig, runtime: Arc<Runtime>) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            cache_generation: Arc::new(AtomicU64::new(0)),
            config,
            runtime,
        }
    }

    /// Reuse the per-tab cache/in-flight bookkeeping from an older
    /// engine while replacing config/runtime plumbing. This avoids
    /// duplicate model requests when settings are saved while a
    /// previous request is still in flight.
    pub fn with_state_from(mut self, previous: &Self) -> Self {
        self.state = previous.state.clone();
        self.cache_generation = previous.cache_generation.clone();
        self
    }

    /// Drop cached state — called when settings change so a new
    /// model gets a clean slate.
    pub fn clear(&self) {
        self.cache_generation.fetch_add(1, Ordering::Relaxed);
        self.state.lock().clear();
    }

    /// Keep in-flight bookkeeping but force successful summaries to
    /// be recomputed. Used when the suggestion model/provider
    /// changes; outstanding requests are still allowed to complete,
    /// but unchanged tab contexts should not be served from the old
    /// model's cache.
    pub fn clear_success_cache(&self) {
        self.cache_generation.fetch_add(1, Ordering::Relaxed);
        for state in self.state.lock().values_mut() {
            state.last_was_success = false;
            state.last_success_key = None;
        }
    }

    /// Forget a single tab's cache (e.g. after the tab is closed).
    pub fn forget(&self, tab_id: u64) {
        self.state.lock().remove(&tab_id);
    }

    /// Invalidate cached and in-flight work for one tab without
    /// affecting other tabs. If no request is running, the next
    /// request for this tab can dispatch immediately. If one is
    /// already running, keep the in-flight guard so rapid OSC-7
    /// churn cannot fan out into multiple concurrent LLM calls; the
    /// stale worker will drop itself on generation mismatch and make
    /// the next poll eligible for a fresh request.
    pub fn invalidate_tab(&self, tab_id: u64) {
        let mut guard = self.state.lock();
        let entry = guard.entry(tab_id).or_default();
        entry.generation = entry.generation.wrapping_add(1);
        entry.last_success_key = None;
        entry.last_was_success = false;
        if entry.in_flight {
            entry.refresh_after_in_flight = true;
        } else {
            entry.last_dispatch = None;
        }
    }

    /// Ask the engine to produce a [`TabSummary`] for `req`. Returns
    /// immediately. The result (if one is produced — many calls are
    /// no-ops because of caching / budget / SSH short-circuit) is
    /// delivered through `callback`, which fires from the tokio
    /// worker thread.
    ///
    /// Workspace passes a callback that just sends on a `crossbeam`
    /// channel; the main loop drains it on the next idle tick.
    pub fn request(
        &self,
        req: TabSummaryRequest,
        callback: impl FnOnce(TabSummary) + Send + 'static,
    ) {
        let tab_id = req.tab_id;
        let key = context_hash(&req);

        // SSH tab: short-circuit, no LLM call.
        if let Some(host) = req
            .ssh_host
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            {
                let mut guard = self.state.lock();
                let entry = guard.entry(tab_id).or_default();
                if entry.last_success_key == Some(key) {
                    log::trace!(
                        target: "con_core::tab_summary",
                        "tab_summary skip tab_id={} reason=ssh_cache_hit",
                        tab_id
                    );
                    return;
                }
                let budget = if entry.last_was_success {
                    PER_TAB_REQUEST_BUDGET
                } else {
                    PER_TAB_RETRY_BUDGET
                };
                if let Some(t) = entry.last_dispatch
                    && t.elapsed() < budget
                {
                    log::trace!(
                        target: "con_core::tab_summary",
                        "tab_summary skip tab_id={} reason=ssh_budget elapsed={:?} budget={:?}",
                        tab_id,
                        t.elapsed(),
                        budget,
                    );
                    return;
                }
                entry.last_success_key = Some(key);
                entry.last_dispatch = Some(Instant::now());
                entry.last_was_success = true;
            }

            // Use the host's first label as the short name (e.g.
            // `prod-1.example.com` -> `prod-1`).
            let short = host.split('.').next().unwrap_or(host).to_string();
            let summary = TabSummary {
                tab_id,
                label: truncate_label(&short),
                icon: TabIconKind::Globe,
            };
            self.runtime.spawn(async move {
                callback(summary);
            });
            return;
        }

        {
            let mut guard = self.state.lock();
            let entry = guard.entry(tab_id).or_default();

            if entry.in_flight {
                log::trace!(
                    target: "con_core::tab_summary",
                    "tab_summary skip tab_id={} reason=in_flight",
                    tab_id
                );
                return;
            }
            // Cache hit only short-circuits on the last *successful*
            // context — a failed request must not poison the slot,
            // or a single empty model response would permanently
            // lock the tab until the user typed something new.
            if entry.last_success_key == Some(key) {
                log::trace!(
                    target: "con_core::tab_summary",
                    "tab_summary skip tab_id={} reason=cache_hit",
                    tab_id
                );
                return;
            }
            let budget = if entry.last_was_success {
                PER_TAB_REQUEST_BUDGET
            } else {
                PER_TAB_RETRY_BUDGET
            };
            if let Some(t) = entry.last_dispatch
                && t.elapsed() < budget
            {
                log::trace!(
                    target: "con_core::tab_summary",
                    "tab_summary skip tab_id={} reason=budget elapsed={:?} budget={:?}",
                    tab_id,
                    t.elapsed(),
                    budget,
                );
                return;
            }
            entry.in_flight = true;
            entry.refresh_after_in_flight = false;
            entry.last_dispatch = Some(Instant::now());
            let request_tab_generation = entry.generation;
            drop(guard);

            let config = self.config.clone();
            let state = self.state.clone();
            let cache_generation = self.cache_generation.clone();
            let request_generation = cache_generation.load(Ordering::Relaxed);
            self.runtime.spawn(async move {
                let result = request_summary(&config, &req).await;

                let mut should_callback = false;
                if let Some(entry) = state.lock().get_mut(&tab_id) {
                    entry.in_flight = false;
                    if entry.generation == request_tab_generation {
                        let cache_is_current =
                            cache_generation.load(Ordering::Relaxed) == request_generation;
                        entry.last_was_success = cache_is_current && result.is_some();
                        if cache_is_current && result.is_some() {
                            entry.last_success_key = Some(key);
                            should_callback = true;
                        }
                    } else {
                        entry.last_was_success = false;
                        entry.last_success_key = None;
                        if entry.refresh_after_in_flight {
                            entry.refresh_after_in_flight = false;
                            entry.last_dispatch = None;
                        }
                    }
                }

                if should_callback && let Some((label, icon)) = result {
                    callback(TabSummary {
                        tab_id,
                        label,
                        icon,
                    });
                }
            });
            return;
        }
    }
}

fn context_hash(req: &TabSummaryRequest) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    req.cwd.hash(&mut h);
    for cmd in req.recent_commands.iter().take(3) {
        cmd.hash(&mut h);
    }
    if let Some(agent_cli) = req.agent_cli {
        // Agent CLIs tick elapsed-time/token counters and animate a
        // spinner prefix in the title every second. Keying on either
        // re-labels the same session every budget window.
        agent_cli.hash(&mut h);
        req.title
            .as_deref()
            .map(|title| title.trim_start_matches(|c: char| !c.is_alphanumeric()))
            .hash(&mut h);
        return h.finish();
    }
    req.title.hash(&mut h);
    // Hash only the last few output lines — that's where new content
    // appears. Hashing the entire scrollback would re-fire every time
    // any line scrolled out, even for stable workloads (`htop`,
    // `tail -f`).
    for line in req.recent_output.iter().rev().take(5) {
        line.hash(&mut h);
    }
    h.finish()
}

fn truncate_label(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= LABEL_MAX_LEN {
        trimmed.to_string()
    } else {
        let mut out: String = trimmed
            .chars()
            .take(LABEL_MAX_LEN.saturating_sub(1))
            .collect();
        out.push('…');
        out
    }
}

/// Build the prompt + parse the response. Returns `None` whenever the
/// model output is malformed — the panel falls back to the heuristic
/// name in that case, which is the safe default.
async fn request_summary(
    config: &AgentConfig,
    req: &TabSummaryRequest,
) -> Option<(String, TabIconKind)> {
    use con_agent::AgentProvider;

    let recent_cmds = if req.recent_commands.is_empty() {
        "(none captured by Con — user may have typed directly)".to_string()
    } else {
        req.recent_commands
            .iter()
            .take(3)
            .map(|c| format!("- {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let recent_output = if req.recent_output.is_empty() {
        "(empty)".to_string()
    } else {
        req.recent_output
            .iter()
            .rev()
            .take(12)
            .rev()
            .map(|line| {
                // Trim each line so the model isn't distracted by
                // trailing whitespace or huge ANSI-stripped runs.
                let line = line.trim_end();
                if line.chars().count() > 200 {
                    let mut s: String = line.chars().take(200).collect();
                    s.push('…');
                    s
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let prompt = format!(
        "Pick a short, professional label and one icon for this terminal tab.\n\
         \n\
         LABEL rules: 1–3 words, Title Case, ≤24 chars. Describe what the tab\n\
         is FOR. Do not echo shell names (\"bash\", \"zsh\"). Do not include\n\
         the word \"tab\". If the only signal is cwd, use Title Case of the\n\
         cwd basename (e.g. /home/u/proj/study → \"Study\"). If signal is\n\
         genuinely thin (no cwd, no commands, empty output), use \"Shell\".\n\
         \n\
         ICON keyword (pick exactly one, lowercase): one of\n\
         terminal, code, pulse, book, file, globe.\n\
         - terminal — generic shell / build / test / scripts\n\
         - code — editor in use (vim/nvim/emacs/nano/helix/code)\n\
         - pulse — monitor (htop/top/btop/k9s/tail -f)\n\
         - book — pager (less/more/man/bat)\n\
         - file — version control (git/lazygit/tig)\n\
         - globe — remote / network (ssh/curl/http)\n\
         \n\
         Context:\n\
         cwd: {cwd}\n\
         title: {title}\n\
         commands: {recent_cmds}\n\
         recent output:\n{recent_output}",
        cwd = req.cwd.as_deref().unwrap_or("(unknown)"),
        title = req.title.as_deref().unwrap_or("(unknown)"),
        recent_cmds = if recent_cmds == "(none captured by Con — user may have typed directly)" {
            "(none)".to_string()
        } else {
            recent_cmds
        },
        recent_output = recent_output,
    );

    let provider = AgentProvider::new(config.clone());
    log::debug!(
        target: "con_core::tab_summary",
        "tab_summary request tab_id={} provider={:?} model={} cwd={:?} title={:?}",
        req.tab_id,
        config.provider,
        config.effective_model(&config.provider),
        req.cwd.as_deref().unwrap_or(""),
        req.title.as_deref().unwrap_or(""),
    );
    // Ask for JSON, parse JSON ourselves (with fence-stripping). We
    // do NOT use rig's `prompt_typed` because it's a no-op on
    // providers that don't support `response_format` (Moonshot
    // logs "Structured outputs currently not supported"), and even
    // when honored, models routinely wrap responses in ```json …```
    // fences which `TypedPromptRequest::send` can't strip.
    // See postmortem/2026-04-26-tab-summary-json-shape.md.
    let preamble = "You label terminal tabs in a developer's IDE-style sidebar. \
                    Respond with a JSON object only: \
                    {\"label\": \"...\", \"icon\": \"...\"}. \
                    No prose, no code fences, no commentary.";
    let raw = match provider
        .complete_with_options(&prompt, preamble, 2048)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            log::debug!(
                target: "con_core::tab_summary",
                "tab_summary completion failed tab_id={}: {}",
                req.tab_id,
                e
            );
            return None;
        }
    };
    let trimmed = raw.trim();
    log::trace!(
        target: "con_core::tab_summary",
        "tab_summary response tab_id={} raw={:?}",
        req.tab_id,
        trimmed,
    );
    let typed = parse_summary_json(trimmed).or_else(|| {
        log::debug!(
            target: "con_core::tab_summary",
            "tab_summary parse rejected tab_id={} reason=no_valid_json raw={:?}",
            req.tab_id,
            trimmed,
        );
        None
    })?;
    let label = typed.label.trim().to_string();
    if label.is_empty() {
        log::debug!(
            target: "con_core::tab_summary",
            "tab_summary parse rejected tab_id={} reason=empty_label",
            req.tab_id,
        );
        return None;
    }
    if label_mentions_tab(&label) {
        log::debug!(
            target: "con_core::tab_summary",
            "tab_summary parse rejected tab_id={} reason=label_contains_tab raw={:?}",
            req.tab_id,
            label,
        );
        return None;
    }
    let Some(icon) = TabIconKind::from_keyword(&typed.icon) else {
        log::debug!(
            target: "con_core::tab_summary",
            "tab_summary parse rejected tab_id={} reason=unknown_icon raw_icon={:?}",
            req.tab_id,
            typed.icon,
        );
        return None;
    };
    let label = truncate_label(&label);
    log::debug!(
        target: "con_core::tab_summary",
        "tab_summary parsed tab_id={} label={:?} icon={:?}",
        req.tab_id,
        label,
        icon,
    );
    Some((label, icon))
}

/// Tolerant JSON extractor for LLM output. Walks the response with
/// a string-aware bracket-balance scanner and parses the first valid
/// `{...}` object it finds. This naturally skips markdown code
/// fences (```json … ```), reasoning preamble, malformed brace
/// examples, and trailing commentary that models commonly emit around
/// the JSON answer.
fn parse_summary_json(raw: &str) -> Option<TabSummaryJson> {
    let bytes = raw.as_bytes();
    let mut cursor = 0;
    while let Some(relative_start) = raw[cursor..].find('{') {
        let start = cursor + relative_start;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        let mut end = None;
        for (i, &b) in bytes[start..].iter().enumerate() {
            if escape {
                escape = false;
                continue;
            }
            match b {
                b'\\' if in_string => escape = true,
                b'"' => in_string = !in_string,
                b'{' if !in_string => depth += 1,
                b'}' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            cursor = start + 1;
            continue;
        };
        let json = &raw[start..end];
        if let Ok(parsed) = serde_json::from_str(json) {
            return Some(parsed);
        }
        cursor = end;
    }
    None
}

fn label_mentions_tab(label: &str) -> bool {
    label
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| matches!(word.to_ascii_lowercase().as_str(), "tab" | "tabs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary_request(
        agent_cli: Option<&'static str>,
        title: &str,
        status: &str,
    ) -> TabSummaryRequest {
        TabSummaryRequest {
            tab_id: 1,
            cwd: Some("/work/con".into()),
            recent_commands: Vec::new(),
            recent_output: vec!["> refactor the tab summary engine".into(), status.into()],
            title: Some(title.into()),
            ssh_host: None,
            agent_cli,
        }
    }

    #[test]
    fn agent_cli_cache_key_ignores_ticking_status_and_title_spinner() {
        let first = summary_request(
            Some("claude"),
            "◐ Tab summary cache",
            "✻ Ideating… (3s · ↑ 111 tokens)",
        );
        let later = summary_request(
            Some("claude"),
            "◑ Tab summary cache",
            "✻ Ideating… (4s · ↑ 148 tokens)",
        );
        assert_eq!(context_hash(&first), context_hash(&later));

        let new_task = summary_request(
            Some("claude"),
            "◑ Fix login flow",
            "✻ Ideating… (4s · ↑ 148 tokens)",
        );
        assert_ne!(context_hash(&first), context_hash(&new_task));
    }

    #[test]
    fn shell_cache_key_still_tracks_recent_output() {
        let before = summary_request(None, "zsh", "cargo test");
        let after = summary_request(None, "zsh", "cargo build");
        assert_ne!(context_hash(&before), context_hash(&after));
    }

    #[test]
    fn parse_json_clean() {
        let raw = "{\"label\": \"Disk Audit\", \"icon\": \"terminal\"}";
        let v = parse_summary_json(raw).unwrap();
        assert_eq!(v.label, "Disk Audit");
        assert_eq!(v.icon, "terminal");
    }

    #[test]
    fn parse_json_in_code_fence() {
        let raw = "```json\n{\n  \"label\": \"Shell\",\n  \"icon\": \"terminal\"\n}\n```";
        let v = parse_summary_json(raw).unwrap();
        assert_eq!(v.label, "Shell");
        assert_eq!(v.icon, "terminal");
    }

    #[test]
    fn parse_json_in_unlabeled_fence() {
        let raw = "```\n{\"label\": \"Logs\", \"icon\": \"book\"}\n```";
        let v = parse_summary_json(raw).unwrap();
        assert_eq!(v.label, "Logs");
        assert_eq!(v.icon, "book");
    }

    #[test]
    fn parse_json_with_preamble() {
        let raw = "Sure, here is the answer:\n{\"label\": \"Test Run\", \"icon\": \"terminal\"}\nLet me know if you need more.";
        let v = parse_summary_json(raw).unwrap();
        assert_eq!(v.label, "Test Run");
        assert_eq!(v.icon, "terminal");
    }

    #[test]
    fn parse_json_skips_invalid_brace_block_before_answer() {
        let raw = "Use {label, icon} as fields.\n{\"label\": \"Build\", \"icon\": \"terminal\"}";
        let v = parse_summary_json(raw).unwrap();
        assert_eq!(v.label, "Build");
        assert_eq!(v.icon, "terminal");
    }

    #[test]
    fn parse_json_skips_unbalanced_brace_before_answer() {
        let raw =
            "Reasoning started { but never closed.\n{\"label\": \"Deploy\", \"icon\": \"globe\"}";
        let v = parse_summary_json(raw).unwrap();
        assert_eq!(v.label, "Deploy");
        assert_eq!(v.icon, "globe");
    }

    #[test]
    fn parse_json_handles_braces_in_strings() {
        // A label containing curly braces shouldn't confuse the
        // bracket-balance scanner.
        let raw = "{\"label\": \"a{b}c\", \"icon\": \"terminal\"}";
        let v = parse_summary_json(raw).unwrap();
        assert_eq!(v.label, "a{b}c");
    }

    #[test]
    fn parse_json_rejects_no_json() {
        assert!(parse_summary_json("just some prose, no json here").is_none());
    }

    #[test]
    fn tab_word_filter_allows_embedded_substrings() {
        assert!(!label_mentions_tab("Stable Diffusion"));
        assert!(!label_mentions_tab("Establish Conn"));
        assert!(!label_mentions_tab("Portable Build"));
        assert!(label_mentions_tab("Tab"));
        assert!(label_mentions_tab("Terminal Tab"));
        assert!(label_mentions_tab("Project Tabs"));
    }

    #[test]
    fn icon_keyword_aliases() {
        assert_eq!(TabIconKind::from_keyword("editor"), Some(TabIconKind::Code));
        assert_eq!(TabIconKind::from_keyword("htop"), Some(TabIconKind::Pulse));
        assert_eq!(TabIconKind::from_keyword("ssh"), Some(TabIconKind::Globe));
        assert_eq!(TabIconKind::from_keyword("sparkle"), None);
    }

    #[test]
    fn truncate_long_label() {
        let long = "a".repeat(50);
        let out = truncate_label(&long);
        assert!(out.chars().count() <= LABEL_MAX_LEN);
        assert!(out.ends_with('…'));
    }

    /// PerTabState gate logic — pure simulation of the conditions in
    /// `request()` so we can lock down "a failed call must not
    /// poison the cache" without having to spin up a real engine
    /// + tokio runtime.
    fn should_dispatch(state: &PerTabState, key: u64) -> bool {
        if state.in_flight {
            return false;
        }
        if state.last_success_key == Some(key) {
            return false;
        }
        let budget = if state.last_was_success {
            PER_TAB_REQUEST_BUDGET
        } else {
            PER_TAB_RETRY_BUDGET
        };
        if let Some(t) = state.last_dispatch
            && t.elapsed() < budget
        {
            return false;
        }
        true
    }

    #[test]
    fn cache_blocks_resend_for_same_context_after_success() {
        let s = PerTabState {
            last_success_key: Some(42),
            last_was_success: true,
            last_dispatch: Some(Instant::now()),
            ..Default::default()
        };
        assert!(
            !should_dispatch(&s, 42),
            "same context after success → skip"
        );
    }

    #[test]
    fn cache_does_not_block_after_failure_with_same_context() {
        // The bug: previous version stored the dispatch key in
        // `last_key` BEFORE the LLM call, so a failed call locked
        // the tab out forever for that context. Verify the new
        // success-keyed cache lets the next call through.
        //
        // Last dispatch is 1 s ago: beyond the failure-retry budget
        // (750 ms) but well under the success budget (5 s).
        let s = PerTabState {
            last_success_key: None,
            last_was_success: false,
            last_dispatch: Some(Instant::now() - Duration::from_secs(1)),
            ..Default::default()
        };
        assert!(
            should_dispatch(&s, 99),
            "failure → context unchanged → next call must retry"
        );
    }

    #[test]
    fn retry_budget_holds_inside_750ms() {
        let s = PerTabState {
            last_dispatch: Some(Instant::now()),
            last_was_success: false,
            ..Default::default()
        };
        assert!(
            !should_dispatch(&s, 99),
            "retry budget should hold for the first 750 ms"
        );
    }

    #[test]
    fn in_flight_blocks_everything() {
        let s = PerTabState {
            in_flight: true,
            ..Default::default()
        };
        assert!(!should_dispatch(&s, 0));
    }

    #[test]
    fn new_context_after_success_dispatches() {
        let s = PerTabState {
            last_success_key: Some(1),
            last_was_success: true,
            last_dispatch: Some(Instant::now() - Duration::from_secs(10)),
            ..Default::default()
        };
        assert!(should_dispatch(&s, 2));
    }

    #[test]
    fn clear_success_cache_preserves_in_flight_guard() {
        let engine = TabSummaryEngine::new(
            AgentConfig::default(),
            Arc::new(Runtime::new().expect("test runtime")),
        );
        engine.state.lock().insert(
            7,
            PerTabState {
                in_flight: true,
                last_success_key: Some(42),
                last_was_success: true,
                last_dispatch: Some(Instant::now()),
                ..Default::default()
            },
        );

        engine.clear_success_cache();

        let state = engine.state.lock();
        let state = state.get(&7).expect("tab state");
        assert!(state.in_flight);
        assert_eq!(state.last_success_key, None);
        assert!(!state.last_was_success);
        assert!(state.last_dispatch.is_some());
    }

    #[test]
    fn invalidate_tab_preserves_in_flight_guard_for_that_tab_only() {
        let engine = TabSummaryEngine::new(
            AgentConfig::default(),
            Arc::new(Runtime::new().expect("test runtime")),
        );
        engine.state.lock().insert(
            7,
            PerTabState {
                generation: 2,
                in_flight: true,
                last_success_key: Some(42),
                last_was_success: true,
                last_dispatch: Some(Instant::now()),
                ..Default::default()
            },
        );
        engine.state.lock().insert(
            8,
            PerTabState {
                generation: 4,
                in_flight: true,
                last_success_key: Some(99),
                last_was_success: true,
                last_dispatch: Some(Instant::now()),
                ..Default::default()
            },
        );

        engine.invalidate_tab(7);

        let state = engine.state.lock();
        let invalidated = state.get(&7).expect("invalidated tab");
        assert_eq!(invalidated.generation, 3);
        assert!(invalidated.in_flight);
        assert!(invalidated.refresh_after_in_flight);
        assert_eq!(invalidated.last_success_key, None);
        assert!(invalidated.last_dispatch.is_some());
        assert!(!invalidated.last_was_success);

        let untouched = state.get(&8).expect("other tab");
        assert_eq!(untouched.generation, 4);
        assert!(untouched.in_flight);
        assert_eq!(untouched.last_success_key, Some(99));
        assert!(untouched.last_was_success);
        assert!(untouched.last_dispatch.is_some());
        assert!(!untouched.refresh_after_in_flight);
    }

    #[test]
    fn invalidate_tab_without_in_flight_unblocks_new_context_immediately() {
        let engine = TabSummaryEngine::new(
            AgentConfig::default(),
            Arc::new(Runtime::new().expect("test runtime")),
        );
        engine.state.lock().insert(
            7,
            PerTabState {
                generation: 2,
                in_flight: false,
                last_success_key: Some(42),
                last_was_success: true,
                last_dispatch: Some(Instant::now()),
                ..Default::default()
            },
        );

        engine.invalidate_tab(7);

        let state = engine.state.lock();
        let invalidated = state.get(&7).expect("invalidated tab");
        assert_eq!(invalidated.generation, 3);
        assert!(!invalidated.in_flight);
        assert!(!invalidated.refresh_after_in_flight);
        assert_eq!(invalidated.last_success_key, None);
        assert_eq!(invalidated.last_dispatch, None);
        assert!(!invalidated.last_was_success);
    }

    /// Tab reorder math (mirrors `on_sidebar_reorder` in workspace.rs).
    ///
    /// Slot semantics: `to ∈ 0..=tabs.len()`. Slot K with K < len
    /// means "insert before row K"; slot len means "after the last
    /// row". After `Vec::remove(from)` shifts subsequent indexes
    /// down by one, the resulting insert index is:
    ///   from < to → to - 1
    ///   from > to → to
    /// from == to or from + 1 == to → no-op (drop on the same row's
    /// top-half slot, or the slot just below — same place).
    ///
    /// Kept here because workspace.rs is too large to compile as a
    /// test target on this machine without bumping rustc's stack
    /// (we hit that earlier). The helper itself is pure so testing
    /// it next to other con-core logic is fine.
    fn apply_reorder<T: Clone>(items: &mut Vec<T>, from: usize, to: usize) {
        if from >= items.len() || to > items.len() {
            return;
        }
        if from == to || from + 1 == to {
            return;
        }
        let insert_at = if from < to { to - 1 } else { to };
        let item = items.remove(from);
        items.insert(insert_at, item);
    }

    #[test]
    fn reorder_drag_to_top_from_middle() {
        let mut v = vec!["a", "b", "c"];
        apply_reorder(&mut v, 1, 0);
        assert_eq!(v, vec!["b", "a", "c"]);
    }

    #[test]
    fn reorder_drag_to_top_from_bottom() {
        let mut v = vec!["a", "b", "c"];
        apply_reorder(&mut v, 2, 0);
        assert_eq!(v, vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_drag_to_bottom_from_top() {
        // Slot 3 == "after the last row" — this is the case that
        // didn't work before the half-row scheme.
        let mut v = vec!["a", "b", "c"];
        apply_reorder(&mut v, 0, 3);
        assert_eq!(v, vec!["b", "c", "a"]);
    }

    #[test]
    fn reorder_drag_to_bottom_from_middle() {
        let mut v = vec!["a", "b", "c"];
        apply_reorder(&mut v, 1, 3);
        assert_eq!(v, vec!["a", "c", "b"]);
    }

    #[test]
    fn reorder_self_drop_top_half_is_noop() {
        let mut v = vec!["a", "b", "c"];
        apply_reorder(&mut v, 1, 1);
        assert_eq!(v, vec!["a", "b", "c"]);
    }

    #[test]
    fn reorder_self_drop_bottom_half_is_noop() {
        // Slot 2 from row 1 means "below row 1" — same place.
        let mut v = vec!["a", "b", "c"];
        apply_reorder(&mut v, 1, 2);
        assert_eq!(v, vec!["a", "b", "c"]);
    }

    #[test]
    fn reorder_drag_middle_to_one_below() {
        let mut v = vec!["a", "b", "c", "d"];
        apply_reorder(&mut v, 1, 3);
        assert_eq!(v, vec!["a", "c", "b", "d"]);
    }
}
