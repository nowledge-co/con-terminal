//! A deliberately restricted ACP history client: no prompt, tools, files or terminal API.
use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use crate::handoff::MAX_EXPORT_BYTES;

pub(super) struct Reader {
    _child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
    pub capabilities: Value,
}

impl Reader {
    pub async fn open(executable: &Path, cwd: &Path) -> Result<Self> {
        let mut child = Command::new(executable)
            .arg("acp")
            .current_dir(cwd)
            .env_clear()
            .envs(crate::handoff::environment::reader_environment(
                crate::handoff::AgentKind::Cursor,
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("Start ACP history reader")?;
        let input = child.stdin.take().context("ACP stdin unavailable")?;
        let output = BufReader::new(child.stdout.take().context("ACP stdout unavailable")?);
        let mut reader = Self {
            _child: child,
            input,
            output,
            next_id: 1,
            capabilities: Value::Null,
        };
        let (result, _) = reader.request("initialize", json!({
            "protocolVersion":1, "clientInfo":{"name":"con-handoff-history","version":"1"},
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}
        }), None).await?;
        ensure!(
            result["protocolVersion"] == 1,
            "Unsupported ACP protocol version"
        );
        reader.capabilities = result["agentCapabilities"].clone();
        // Existing login is used by the server. Never call authenticate: it can open a browser.
        Ok(reader)
    }

    async fn send(&mut self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        Ok(())
    }

    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<(Value, Vec<Value>)> {
        ensure!(
            matches!(method, "initialize" | "session/list" | "session/load"),
            "History client rejects this ACP method"
        );
        let id = self.next_id;
        self.next_id += 1;
        tokio::time::timeout(Duration::from_secs(30), async {
            self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await?;
            let mut updates = Vec::new();
            let mut total = 0;
            loop {
                let value = self
                    .read_message(&mut total)
                    .await?
                    .context("ACP history reader closed unexpectedly")?;
                if self.handle(&value, session, &mut updates).await? {
                    continue;
                }
                if value.get("id") != Some(&json!(id)) { continue; }
                if let Some(error) = value.get("error") {
                    bail!("ACP {method} failed (code {}); check the native agent login and source session", error["code"]);
                }
                let result = value.get("result").cloned().context("Missing ACP result")?;
                if session.is_some() {
                    // The result acknowledges the load; it is not an
                    // end-of-replay marker. Servers may keep streaming
                    // session/update events after it, so drain until the
                    // stream goes quiet or closes instead of silently
                    // truncating the exported history.
                    loop {
                        match tokio::time::timeout(REPLAY_QUIESCENCE, self.read_message(&mut total))
                            .await
                        {
                            Err(_) | Ok(Ok(None)) => break,
                            Ok(Ok(Some(value))) => {
                                self.handle(&value, session, &mut updates).await?;
                            }
                            Ok(Err(error)) => return Err(error),
                        }
                    }
                }
                return Ok((result, updates));
            }
        }).await.context("ACP history request timed out")?
    }

    /// One framed message; `None` means the server closed the stream.
    async fn read_message(&mut self, total: &mut usize) -> Result<Option<Value>> {
        let mut bytes = Vec::new();
        let size = (&mut self.output)
            .take(MAX_EXPORT_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes)
            .await?;
        if size == 0 {
            return Ok(None);
        }
        *total += size;
        ensure!(
            size <= MAX_EXPORT_BYTES && *total <= MAX_EXPORT_BYTES,
            "ACP history exceeds the 10 MiB limit"
        );
        Ok(Some(
            serde_json::from_slice(&bytes).context("Unsupported ACP framing")?,
        ))
    }

    /// Consume a server-to-client request (denied) or a session/update
    /// notification (collected when it names the selected session). Returns
    /// false for responses, which the caller matches against its pending id.
    async fn handle(
        &mut self,
        value: &Value,
        session: Option<&str>,
        updates: &mut Vec<Value>,
    ) -> Result<bool> {
        if value.get("method").is_some() && value.get("id").is_some() {
            let denied = if value["method"] == "session/request_permission" {
                json!({"jsonrpc":"2.0","id":value["id"],"result":{"outcome":{"outcome":"cancelled"}}})
            } else {
                json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32601,"message":"Con history client does not execute tools, access files or create terminals"}})
            };
            self.send(denied).await?;
            return Ok(true);
        }
        if value["method"] == "session/update" {
            if let Some(selected) = session {
                ensure!(
                    value["params"]["sessionId"].as_str() == Some(selected),
                    "ACP replay returned a different session"
                );
                updates.push(value["params"]["update"].clone());
            }
            return Ok(true);
        }
        Ok(false)
    }
}

/// How long the replay stream may stay silent after the session/load result
/// before the export is considered complete.
const REPLAY_QUIESCENCE: Duration = Duration::from_millis(500);
