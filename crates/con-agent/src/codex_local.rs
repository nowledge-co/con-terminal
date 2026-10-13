//! Local Codex JSONL app-server transport. Codex owns auth and model routing;
//! Con's Rig tool registry and ConHook retain execution and approval authority.

use anyhow::{Context, Result, bail};
use crossbeam_channel::Sender;
use rig::{agent::Flow, message::Message, tool::server::ToolServerHandle};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

use crate::{AgentConfig, AgentEvent, ConHook, ProviderKind};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const TURN_TIMEOUT: Duration = Duration::from_secs(900);
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const SAFETY_NOTE: &str = "Use only the Con-provided dynamic tools to inspect or change the workspace. \
    Commands must use terminal_exec or other Con terminal tools, with Con approval. \
    Never use Codex built-in shell, apply_patch, MCP, or subagent tools. \
    Codex is only the model transport; Con owns execution.";

fn executable() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let bundled = {
        let mut bundled = Vec::new();
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join("Applications"));
        }
        for root in roots {
            for app in ["Codex.app", "ChatGPT.app"] {
                let resources = root.join(app).join("Contents/Resources");
                bundled.extend([
                    resources.join("codex"),
                    resources.join("codex-cli/bin/codex"),
                ]);
            }
        }
        bundled
    };
    #[cfg(not(target_os = "macos"))]
    let bundled = Vec::new();
    resolve_executable(
        std::env::var_os("CON_CODEX_BIN").as_deref(),
        &crate::handoff::native_agent_path(),
        &bundled,
    )
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

fn resolve_executable(
    override_path: Option<&OsStr>,
    search_path: &OsStr,
    bundled: &[PathBuf],
) -> Result<PathBuf> {
    if let Some(path) = override_path.filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_absolute() || !is_executable(&path) {
            bail!("CON_CODEX_BIN must be an absolute path to an executable Codex CLI file");
        }
        return Ok(path);
    }
    let names: &[&str] = if cfg!(windows) {
        &["codex.exe", "codex.cmd"]
    } else {
        &["codex"]
    };
    for directory in std::env::split_paths(search_path) {
        for name in names {
            let path = directory.join(name);
            if is_executable(&path) {
                return Ok(path);
            }
        }
    }
    for path in bundled {
        if is_executable(path) {
            return Ok(path.clone());
        }
    }
    bail!(
        "Codex is not installed. Install the Codex CLI and run `codex login`, or set CON_CODEX_BIN to its absolute executable path."
    )
}

struct AppServer {
    child: Option<Child>,
    stdin: Box<dyn AsyncWrite + Send + Unpin>,
    stdout: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
    next_id: u64,
    pending: VecDeque<Value>,
    pending_bytes: usize,
}

impl AppServer {
    fn spawn() -> Result<Self> {
        let mut command = Command::new(executable()?);
        command
            .arg("app-server")
            .args(["--listen", "stdio://"])
            .env("PATH", crate::handoff::native_agent_path())
            // Apply before initialization: user startup hooks must not execute.
            .args([
                "-c",
                "features.hooks=false",
                "-c",
                "features.shell_snapshot=false",
                "-c",
                "notify=[]",
            ]);
        Self::spawn_command(command)
    }

    fn spawn_command(mut command: Command) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0); // Own group: cancelling an npm shim must stop its Codex child too.
        #[cfg(windows)]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW: stdio transport, not a console UI.
        let mut child = command.spawn().context("Start local Codex app-server")?;
        let stdin = child.stdin.take().context("Codex stdin unavailable")?;
        let stdout = child.stdout.take().context("Codex stdout unavailable")?;
        Ok(Self {
            child: Some(child),
            stdin: Box::new(stdin),
            stdout: BufReader::new(Box::new(stdout)),
            next_id: 1,
            pending: VecDeque::new(),
            pending_bytes: 0,
        })
    }

    async fn write(&mut self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .context("Write to Codex app-server")?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read(&mut self) -> Result<Value> {
        if let Some(message) = self.pending.pop_front() {
            self.pending_bytes -= message.to_string().len();
            return Ok(message);
        }
        self.read_wire().await
    }

    async fn read_wire(&mut self) -> Result<Value> {
        let mut bytes = Vec::new();
        let n = (&mut self.stdout)
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .await?;
        if n == 0 {
            bail!(
                "Codex app-server closed its output unexpectedly; check that `codex app-server` works and Codex is logged in"
            );
        }
        if n > MAX_FRAME_BYTES {
            bail!("Codex app-server frame exceeds the size limit");
        }
        serde_json::from_slice(&bytes).context("Invalid Codex app-server JSON frame")
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        tokio::time::timeout(RPC_TIMEOUT, async {
            self.write(json!({"id": id, "method": method, "params": params}))
                .await?;
            loop {
                let message = self.read_wire().await?;
                if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                    if let Some(error) = message.get("error") {
                        bail!(
                            "Codex {method}: {}",
                            error["message"].as_str().unwrap_or("RPC error")
                        );
                    }
                    return message
                        .get("result")
                        .cloned()
                        .context("Codex RPC response has no result");
                }
                // A fast turn may emit deltas/tool requests before its response.
                // Preserve those frames rather than losing tokens or denying a tool.
                if method == "turn/start" {
                    let bytes = message.to_string().len();
                    if self.pending.len() >= 1024 || self.pending_bytes + bytes > MAX_FRAME_BYTES {
                        bail!("Codex sent too many frames before the turn/start response");
                    }
                    self.pending_bytes += bytes;
                    self.pending.push_back(message);
                } else if message.get("id").is_some() && message.get("method").is_some() {
                    self.reject_request(&message).await?;
                }
            }
        })
        .await
        .with_context(|| format!("Codex {method} timed out"))?
    }

    async fn initialize(&mut self) -> Result<()> {
        self.request(
            "initialize",
            json!({
                "clientInfo": {"name": "con", "title": "Con", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true}
            }),
        )
        .await?;
        self.write(json!({"method": "initialized", "params": {}}))
            .await
    }

    async fn reject_request(&mut self, request: &Value) -> Result<()> {
        let id = &request["id"];
        match request["method"].as_str().unwrap_or_default() {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                self.write(json!({"id": id, "result": {"decision": "decline"}})).await
            }
            "item/permissions/requestApproval" => {
                self.write(json!({"id": id, "result": {"permissions": {}, "scope": "turn"}})).await
            }
            _ => self.write(json!({"id": id, "error": {
                "code": -32601, "message": "Con only supports registered dynamic tools; use Con tools instead"
            }})).await,
        }
    }

    #[cfg(unix)]
    fn kill_process_group(&self) {
        if let Some(id) = self.child.as_ref().and_then(Child::id) {
            // SAFETY: spawn_command creates a fresh process group whose ID is
            // our child's PID. Only this dedicated group is signalled.
            unsafe {
                libc::kill(-(id as i32), libc::SIGKILL);
            }
        }
    }

    async fn shutdown(&mut self) {
        // This is a dedicated, ephemeral process, never the user's daemon.
        #[cfg(unix)]
        self.kill_process_group();
        if let Some(child) = &mut self.child {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

#[cfg(unix)]
impl Drop for AppServer {
    fn drop(&mut self) {
        // Also cover a caller dropping the future (e.g. suggestion timeout).
        self.kill_process_group();
    }
}

fn thread_params(
    config: &AgentConfig,
    system: &str,
    cwd: &Path,
    tools: Value,
    user_config: &Value,
) -> Value {
    let mut overrides = json!({
        "features.shell_tool": false,
        "features.unified_exec": false,
        "features.hooks": false,
        "features.apps": false,
        "features.plugins": false,
        "features.multi_agent": false,
        "features.code_mode": false,
        "features.js_repl": false,
        "features.goals": false,
        "features.shell_snapshot": false,
        "features.image_generation": false,
        "web_search": "disabled",
        "notify": []
    });
    // An empty table does not remove inherited MCP entries. Disable every
    // configured server explicitly before the thread starts its MCP processes.
    if let Some(servers) = user_config["config"]["mcp_servers"].as_object() {
        // Nested keys preserve server names containing dots. app-server treats
        // dotted override strings literally (not as quoted TOML paths).
        overrides["mcp_servers"] = Value::Object(
            servers
                .keys()
                .map(|name| (name.clone(), json!({"enabled": false})))
                .collect(),
        );
    }
    let model = config.effective_model(&ProviderKind::CodexLocal).trim();
    json!({
        "model": if model.is_empty() { Value::Null } else { json!(model) },
        "cwd": cwd,
        "ephemeral": true,
        "approvalPolicy": "never",
        "sandbox": "read-only",
        "baseInstructions": format!("{system}\n\n{SAFETY_NOTE}"),
        "developerInstructions": SAFETY_NOTE,
        "config": overrides,
        "dynamicTools": tools,
        "environments": [],
        "experimentalRawEvents": false
    })
}

fn history_items(history: Vec<Message>) -> Vec<Value> {
    history
        .into_iter()
        .filter_map(|message| {
            let (role, text, kind) = match message {
                Message::User { content } => (
                    "user",
                    content
                        .iter()
                        .filter_map(|c| {
                            if let rig::message::UserContent::Text(t) = c {
                                Some(t.text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    "input_text",
                ),
                Message::Assistant { content, .. } => (
                    "assistant",
                    content
                        .iter()
                        .filter_map(|c| {
                            if let rig::message::AssistantContent::Text(t) = c {
                                Some(t.text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    "output_text",
                ),
                Message::System { .. } => return None,
            };
            Some(
                json!({"type": "message", "role": role, "content": [{"type": kind, "text": text}]}),
            )
        })
        .collect()
}

#[derive(Default)]
struct TurnOutput {
    text: String,
    // Track each item separately: completed items backfill missing deltas only.
    messages: HashMap<String, String>,
}

impl TurnOutput {
    fn notification(
        &mut self,
        message: &Value,
        event_tx: &Sender<AgentEvent>,
        thread_id: &str,
    ) -> Result<bool> {
        let params = &message["params"];
        if params["threadId"].as_str() != Some(thread_id) {
            return Ok(false);
        }
        match message["method"].as_str().unwrap_or_default() {
            "item/agentMessage/delta" => {
                let delta = params["delta"]
                    .as_str()
                    .context("Codex text delta missing")?;
                let id = params["itemId"]
                    .as_str()
                    .context("Codex text item ID missing")?;
                self.messages.entry(id.into()).or_default().push_str(delta);
                self.text.push_str(delta);
                let _ = event_tx.send(AgentEvent::Token(delta.into()));
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                if let Some(delta) = params["delta"].as_str() {
                    let _ = event_tx.send(AgentEvent::ThinkingDelta(delta.into()));
                }
            }
            "item/completed" if params["item"]["type"] == "agentMessage" => {
                let item = &params["item"];
                let id = item["id"]
                    .as_str()
                    .context("Codex message item ID missing")?;
                let text = item["text"]
                    .as_str()
                    .context("Codex message text missing")?;
                let streamed = self.messages.entry(id.into()).or_default();
                // Completed items are authoritative. Backfill an unsent suffix,
                // but never duplicate text already emitted as deltas.
                if let Some(suffix) = text
                    .strip_prefix(streamed.as_str())
                    .filter(|s| !s.is_empty())
                {
                    self.text.push_str(suffix);
                    let _ = event_tx.send(AgentEvent::Token(suffix.into()));
                }
                *streamed = text.to_owned();
            }
            "turn/completed" => {
                let turn = &params["turn"];
                match turn["status"].as_str() {
                    Some("completed") => return Ok(true),
                    Some("interrupted") => bail!("Codex turn was interrupted"),
                    _ => bail!(
                        "Codex turn failed: {}",
                        turn["error"]["message"].as_str().unwrap_or("unknown error")
                    ),
                }
            }
            "error" if params["willRetry"] != true => {
                bail!(
                    "Codex error: {}",
                    params["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown error")
                );
            }
            _ => {}
        }
        Ok(false)
    }
}

async fn run_tool(
    request: &Value,
    tools: &ToolServerHandle,
    hook: &ConHook,
    event_tx: &Sender<AgentEvent>,
) -> Result<Value> {
    let params = &request["params"];
    let call_id = params["callId"]
        .as_str()
        .context("Codex tool call ID missing")?;
    let name = params["tool"].as_str().context("Codex tool name missing")?;
    let args = serde_json::to_string(&params["arguments"])?;
    let (text, success) = match hook.on_tool_call(name, call_id, &args).await {
        Flow::Continue => {
            let result = tools
                .call_tool_structured(name, &args, &rig::tool::ToolCallExtensions::default())
                .await;
            (
                result.model_output().to_owned(),
                matches!(result.outcome(), rig::tool::ToolOutcome::Success),
            )
        }
        Flow::Skip { reason } => (reason, false),
        _ => ("Con stopped this tool call".into(), false),
    };
    let _ = event_tx.send(AgentEvent::ToolCallComplete {
        call_id: call_id.into(),
        tool_name: name.into(),
        result: text.clone(),
    });
    Ok(json!({"id": request["id"], "result": {
        "contentItems": [{"type": "inputText", "text": text}], "success": success
    }}))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn send(
    config: &AgentConfig,
    system: &str,
    history: Vec<Message>,
    prompt: &str,
    cwd: &Path,
    tools: ToolServerHandle,
    hook: ConHook,
    event_tx: &Sender<AgentEvent>,
    cancelled: &Arc<AtomicBool>,
) -> Result<String> {
    send_with_server(
        AppServer::spawn()?,
        config,
        system,
        history,
        prompt,
        cwd,
        tools,
        hook,
        event_tx,
        cancelled,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn send_with_server(
    mut server: AppServer,
    config: &AgentConfig,
    system: &str,
    history: Vec<Message>,
    prompt: &str,
    cwd: &Path,
    tools: ToolServerHandle,
    hook: ConHook,
    event_tx: &Sender<AgentEvent>,
    cancelled: &Arc<AtomicBool>,
) -> Result<String> {
    let mut output = TurnOutput::default();
    let operation = async {
        server.initialize().await?;
        let user_config = server
            .request("config/read", json!({"includeLayers": false, "cwd": cwd}))
            .await?;
        let definitions = tools.get_tool_defs(None).await?;
        let dynamic_tools = definitions.into_iter().map(|tool| json!({
            "name": tool.name, "description": tool.description, "inputSchema": tool.parameters
        })).collect::<Vec<_>>();
        let started = server
            .request(
                "thread/start",
                thread_params(config, system, cwd, json!(dynamic_tools), &user_config),
            )
            .await?;
        // Fail closed if an installed Codex ignores safety overrides.
        if started["sandbox"]["type"] != "readOnly" || started["approvalPolicy"] != "never" {
            bail!("Codex app-server did not accept Con's read-only safety policy; update Codex");
        }
        let thread_id = started["thread"]["id"]
            .as_str()
            .context("Codex thread ID missing")?
            .to_owned();
        let history = history_items(history);
        if !history.is_empty() {
            server
                .request(
                    "thread/inject_items",
                    json!({"threadId": thread_id, "items": history}),
                )
                .await?;
        }
        server
            .request(
                "turn/start",
                json!({
                    "threadId": thread_id, "input": [{"type": "text", "text": prompt}]
                }),
            )
            .await?;
        let mut tool_calls = 0usize;
        loop {
            let message = server.read().await?;
            if message.get("id").is_some() && message.get("method").is_some() {
                if message["method"] == "item/tool/call"
                    && message["params"]["threadId"] == thread_id
                {
                    tool_calls += 1;
                    if tool_calls > config.max_turns {
                        bail!("Codex exceeded Con's tool-call budget");
                    }
                    let response = run_tool(&message, &tools, &hook, event_tx).await?;
                    server.write(response).await?;
                } else {
                    server.reject_request(&message).await?;
                }
            } else if output.notification(&message, event_tx, &thread_id)? {
                return Ok(());
            }
        }
    };
    let result = tokio::select! {
        result = tokio::time::timeout(TURN_TIMEOUT, operation) => result.context("Codex turn timed out").and_then(|r| r),
        _ = wait_cancelled(cancelled) => Ok(()),
    };
    server.shutdown().await;
    result?;
    Ok(output.text)
}

async fn wait_cancelled(cancelled: &AtomicBool) {
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn complete(config: &AgentConfig, prompt: &str, system: &str) -> Result<String> {
    let (event_tx, _event_rx) = crossbeam_channel::unbounded();
    let (_approval_tx, approval_rx) = crossbeam_channel::unbounded();
    let cancelled = Arc::new(AtomicBool::new(false));
    let hook = ConHook::new(event_tx.clone(), approval_rx, false, cancelled.clone());
    let cwd = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let mut config = config.clone();
    config.max_turns = 0;
    send(
        &config,
        system,
        vec![],
        prompt,
        &cwd,
        rig::tool::server::ToolServer::new().run(),
        hook,
        &event_tx,
        &cancelled,
    )
    .await
}

/// Fetch the installed Codex model list without touching Con's auth caches.
pub async fn fetch_models() -> Result<Vec<String>> {
    let mut server = AppServer::spawn()?;
    let result = async {
        server.initialize().await?;
        let mut models = Vec::new();
        let mut cursor = Value::Null;
        for _ in 0..16 {
            let page = server
                .request("model/list", json!({"limit": 100, "cursor": cursor}))
                .await?;
            let data = page["data"]
                .as_array()
                .context("Codex model list is missing data")?;
            for model in data {
                if let Some(name) = model["model"].as_str().filter(|m| !m.is_empty())
                    && !models.iter().any(|m| m == name)
                {
                    models.push(name.to_owned());
                }
            }
            cursor = page["nextCursor"].clone();
            if cursor.is_null() {
                return Ok(models);
            }
        }
        bail!("Codex model list exceeded the page limit")
    }
    .await;
    server.shutdown().await;
    result
}

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod real_server_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transport_tests;
