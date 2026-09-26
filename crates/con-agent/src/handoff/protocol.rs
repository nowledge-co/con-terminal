use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use super::MAX_EXPORT_BYTES;

pub(super) fn executable(name: &str) -> Result<PathBuf> {
    let path = process_path();
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if candidate.metadata()?.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            return candidate
                .canonicalize()
                .with_context(|| format!("Resolve {name}"));
        }
    }
    bail!("{name} is not installed in PATH or the supported local CLI directories")
}

/// Finder-launched apps often have a minimal PATH. Keep fallback directories
/// scoped to these subprocesses; never rewrite shell configuration.
pub fn process_path() -> OsString {
    let original = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<_> = std::env::split_paths(&original).collect();
    if let Some(home) = dirs::home_dir() {
        paths.extend(
            [
                ".local/bin",
                ".npm-global/bin",
                ".kimi-code/bin",
                ".cargo/bin",
                ".bun/bin",
            ]
            .into_iter()
            .map(|part| home.join(part)),
        );
    }
    paths.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    std::env::join_paths(paths).unwrap_or(original)
}

/// Per-call error wording for `run_bounded`. `spawn` is optional because the
/// capability probe historically surfaced the raw spawn I/O error.
struct BoundedFailure {
    spawn: Option<&'static str>,
    stdout: &'static str,
    oversized: &'static str,
    failed: &'static str,
    timed_out: &'static str,
}

/// Spawn a read-only agent command and bound its stdout: stdin/stderr closed,
/// a byte limit, one timeout covering the read and the exit-status wait.
async fn run_bounded(
    mut command: Command,
    args: &[OsString],
    cwd: Option<&Path>,
    limit: usize,
    timeout: Duration,
    errors: BoundedFailure,
) -> Result<Vec<u8>> {
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = match errors.spawn {
        Some(context) => command.spawn().context(context)?,
        None => command.spawn()?,
    };
    let mut stdout = child
        .stdout
        .take()
        .context(errors.stdout)?
        .take(limit as u64 + 1);
    let mut data = Vec::new();
    tokio::time::timeout(timeout, async {
        stdout.read_to_end(&mut data).await?;
        anyhow::ensure!(data.len() <= limit, "{}", errors.oversized);
        anyhow::ensure!(child.wait().await?.success(), "{}", errors.failed);
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context(errors.timed_out)??;
    Ok(data)
}

pub(super) async fn capture(
    exe: &Path,
    args: &[OsString],
    cwd: Option<&Path>,
    limit: usize,
    agent: Option<super::AgentKind>,
) -> Result<Vec<u8>> {
    let mut command = Command::new(exe);
    command.env_clear().envs(
        agent
            .map(super::environment::reader_environment)
            .unwrap_or_else(super::probe_environment),
    );
    run_bounded(
        command,
        args,
        cwd,
        limit,
        Duration::from_secs(30),
        BoundedFailure {
            spawn: Some("Start native agent command"),
            stdout: "Missing agent stdout",
            oversized: "Agent output exceeds handoff limit",
            failed: "Native agent command failed; inspect the agent installation and login",
            timed_out: "Native agent command timed out",
        },
    )
    .await
}

pub(super) async fn output(exe: &Path, args: &[&str]) -> Result<String> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let mut command = Command::new(exe);
    command.env_clear().envs(super::probe_environment());
    let data = run_bounded(
        command,
        &args,
        None,
        128 * 1024,
        Duration::from_secs(15),
        BoundedFailure {
            spawn: None,
            stdout: "Missing stdout",
            oversized: "Capability response exceeds limit",
            failed: "Capability probe failed",
            timed_out: "Capability probe timed out",
        },
    )
    .await?;
    Ok(String::from_utf8(data)?.trim().to_owned())
}

pub(super) struct CodexReader {
    _child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}

impl CodexReader {
    pub async fn open(cwd: &Path) -> Result<Self> {
        let executable = executable("codex")?;
        let version = output(&executable, &["--version"]).await?;
        anyhow::ensure!(
            version.starts_with("codex-cli "),
            "Executable is not Codex CLI"
        );
        let mut child = Command::new(executable)
            .arg("app-server")
            .env_clear()
            .envs(super::environment::reader_environment(
                super::AgentKind::Codex,
            ))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("Start Codex history reader")?;
        let input = child.stdin.take().context("Missing Codex stdin")?;
        let output = BufReader::new(child.stdout.take().context("Missing Codex stdout")?);
        let mut this = Self {
            _child: child,
            input,
            output,
            next_id: 1,
        };
        this.request(
            "initialize",
            json!({"clientInfo":{"name":"con_handoff","version":"1"}}),
        )
        .await?;
        this.send(json!({"method":"initialized"})).await?;
        Ok(this)
    }

    async fn send(&mut self, message: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        Ok(())
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        tokio::time::timeout(Duration::from_secs(30), async {
            self.send(json!({"id":id,"method":method,"params":params})).await?;
            loop {
                let mut bytes = Vec::new();
                // A limited read prevents a corrupt or incompatible source from exhausting memory.
                let n = (&mut self.output).take(MAX_EXPORT_BYTES as u64 + 1)
                    .read_until(b'\n', &mut bytes).await?;
                anyhow::ensure!(n > 0, "Codex history reader closed unexpectedly");
                anyhow::ensure!(n <= MAX_EXPORT_BYTES, "History exceeds the 10 MiB export limit; select a smaller source session");
                let value: Value = serde_json::from_slice(&bytes)?;
                if value.get("method").is_some() && value.get("id").is_some() {
                    self.send(json!({"id":value["id"],"error":{"code":-32601,"message":"Read-only history client"}})).await?;
                    continue;
                }
                if value.get("id") != Some(&json!(id)) { continue; }
                if let Some(error) = value.get("error") {
                    bail!("Codex {method} failed (code {}); source unchanged", error["code"]);
                }
                return value.get("result").cloned().context("Missing Codex result");
            }
        }).await.context("Codex history request timed out")?
    }
}
