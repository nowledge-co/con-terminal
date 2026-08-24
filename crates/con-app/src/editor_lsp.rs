use crossbeam_channel::{Receiver, Sender, select};
use serde_json::json;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, Ordering},
    },
    thread,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LanguageServerSpec {
    pub language_id: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
}

pub(crate) fn language_server_for_path(path: &Path) -> Option<LanguageServerSpec> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some(LanguageServerSpec {
            language_id: "rust",
            command: "rust-analyzer",
            args: &[],
        }),
        "ts" => Some(LanguageServerSpec {
            language_id: "typescript",
            command: "typescript-language-server",
            args: &["--stdio"],
        }),
        "tsx" => Some(LanguageServerSpec {
            language_id: "typescriptreact",
            command: "typescript-language-server",
            args: &["--stdio"],
        }),
        "js" | "mjs" | "cjs" => Some(LanguageServerSpec {
            language_id: "javascript",
            command: "typescript-language-server",
            args: &["--stdio"],
        }),
        "jsx" => Some(LanguageServerSpec {
            language_id: "javascriptreact",
            command: "typescript-language-server",
            args: &["--stdio"],
        }),
        "py" => Some(LanguageServerSpec {
            language_id: "python",
            command: "pyright-langserver",
            args: &["--stdio"],
        }),
        "go" => Some(LanguageServerSpec {
            language_id: "go",
            command: "gopls",
            args: &[],
        }),
        _ => None,
    }
}

pub(crate) fn encode_json_rpc_message(message: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).unwrap_or_default();
    let mut encoded = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    encoded.extend(body);
    encoded
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorDiagnostic {
    pub line: usize,
    pub start_character: usize,
    pub end_character: usize,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Debug)]
pub(crate) enum LspClientEvent {
    Diagnostics {
        path: PathBuf,
        diagnostics: Vec<EditorDiagnostic>,
    },
    Log(String),
}

enum LspClientCommand {
    DidChange { version: i32, text: String },
    Shutdown,
}

pub(crate) struct LspClient {
    path: PathBuf,
    sender: Sender<LspClientCommand>,
    version: AtomicI32,
    child: Arc<Mutex<Child>>,
}

impl LspClient {
    pub(crate) fn start(
        path: PathBuf,
        text: String,
        events: Sender<LspClientEvent>,
    ) -> io::Result<Option<Self>> {
        let Some(spec) = language_server_for_path(&path) else {
            return Ok(None);
        };

        let mut command = con_paths::host_command(spec.command);
        command
            .args(spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(parent) = path.parent() {
            command.current_dir(parent);
        }

        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("language server stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("language server stdout was not piped"))?;
        let child = Arc::new(Mutex::new(child));
        let (sender, receiver) = crossbeam_channel::unbounded();
        let (initialize_tx, initialize_rx) = crossbeam_channel::bounded(1);

        spawn_writer_thread(
            path.clone(),
            spec.clone(),
            text,
            stdin,
            receiver,
            initialize_rx,
        );
        spawn_reader_thread(path.clone(), stdout, initialize_tx, events);

        Ok(Some(Self {
            path,
            sender,
            version: AtomicI32::new(1),
            child,
        }))
    }

    pub(crate) fn did_change(&self, text: String) {
        let version = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self
            .sender
            .send(LspClientCommand::DidChange { version, text });
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.sender.send(LspClientCommand::Shutdown);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        log::debug!("[editor-lsp] stopped {}", self.path.display());
    }
}

pub(crate) fn diagnostics_from_publish_notification(
    message: &serde_json::Value,
) -> Option<(PathBuf, Vec<EditorDiagnostic>)> {
    if message.get("method")?.as_str()? != "textDocument/publishDiagnostics" {
        return None;
    }

    let params = message.get("params")?;
    let path = uri_to_file_path(params.get("uri")?.as_str()?)?;
    let diagnostics = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .filter_map(editor_diagnostic_from_lsp_value)
        .collect();

    Some((path, diagnostics))
}

pub(crate) fn strongest_diagnostic_for_line(
    diagnostics: &[EditorDiagnostic],
    line: usize,
) -> Option<&EditorDiagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.line == line)
        .min_by_key(|diagnostic| diagnostic.severity.rank())
}

fn spawn_writer_thread(
    path: PathBuf,
    spec: LanguageServerSpec,
    initial_text: String,
    mut stdin: impl Write + Send + 'static,
    receiver: Receiver<LspClientCommand>,
    initialize_rx: Receiver<()>,
) {
    let name = format!("con-lsp-writer-{}", spec.language_id);
    let _ = thread::Builder::new().name(name).spawn(move || {
        if write_message(&mut stdin, &initialize_message(&path)).is_err() {
            return;
        }

        let mut pending_changes = Vec::new();
        loop {
            select! {
                recv(initialize_rx) -> initialized => {
                    if initialized.is_err() {
                        return;
                    }
                    break;
                }
                recv(receiver) -> command => match command {
                    Ok(LspClientCommand::DidChange { version, text }) => {
                        pending_changes.push((version, text));
                    }
                    Ok(LspClientCommand::Shutdown) => {
                        write_shutdown_and_exit(&mut stdin);
                        return;
                    }
                    Err(_) => return,
                },
            }
        }

        for message in [
            initialized_message(),
            did_open_message(&path, &spec, &initial_text),
        ] {
            if write_message(&mut stdin, &message).is_err() {
                return;
            }
        }
        for (version, text) in pending_changes {
            let message = did_change_message(&path, version, &text);
            if write_message(&mut stdin, &message).is_err() {
                return;
            }
        }

        while let Ok(command) = receiver.recv() {
            match command {
                LspClientCommand::DidChange { version, text } => {
                    let message = did_change_message(&path, version, &text);
                    if write_message(&mut stdin, &message).is_err() {
                        return;
                    }
                }
                LspClientCommand::Shutdown => {
                    write_shutdown_and_exit(&mut stdin);
                    return;
                }
            }
        }
    });
}

fn spawn_reader_thread(
    path: PathBuf,
    stdout: impl Read + Send + 'static,
    initialize_tx: Sender<()>,
    events: Sender<LspClientEvent>,
) {
    let _ = thread::Builder::new()
        .name("con-lsp-reader".to_string())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut initialize_acknowledged = false;
            loop {
                match read_json_rpc_message(&mut reader) {
                    Ok(Some(message)) => {
                        if !initialize_acknowledged && is_initialize_response(&message) {
                            initialize_acknowledged = true;
                            let _ = initialize_tx.send(());
                        }
                        if let Some((diagnostic_path, diagnostics)) =
                            diagnostics_from_publish_notification(&message)
                        {
                            let _ = events.send(LspClientEvent::Diagnostics {
                                path: diagnostic_path,
                                diagnostics,
                            });
                        }
                    }
                    Ok(None) => return,
                    Err(error) => {
                        let _ = events
                            .send(LspClientEvent::Log(format!("{}: {error}", path.display())));
                        return;
                    }
                }
            }
        });
}

fn initialize_message(path: &Path) -> serde_json::Value {
    let root_uri = path
        .parent()
        .and_then(|parent| url::Url::from_directory_path(parent).ok())
        .map(|uri| uri.to_string());

    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": {
                        "relatedInformation": true
                    }
                }
            }
        }
    })
}

fn initialized_message() -> serde_json::Value {
    json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})
}

fn did_open_message(path: &Path, spec: &LanguageServerSpec, text: &str) -> serde_json::Value {
    let uri = document_uri(path).unwrap_or_else(|| path.display().to_string());
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": spec.language_id,
                "version": 1,
                "text": text
            }
        }
    })
}

fn is_initialize_response(message: &serde_json::Value) -> bool {
    message.get("id").and_then(|id| id.as_i64()) == Some(1) && message.get("result").is_some()
}

fn write_shutdown_and_exit(stdin: &mut impl Write) {
    let _ = write_message(
        stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null}),
    );
    let _ = write_message(
        stdin,
        &json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
    );
}

fn did_change_message(path: &Path, version: i32, text: &str) -> serde_json::Value {
    let uri = document_uri(path).unwrap_or_else(|| path.display().to_string());
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didChange",
        "params": {
            "textDocument": {
                "uri": uri,
                "version": version
            },
            "contentChanges": [
                { "text": text }
            ]
        }
    })
}

fn write_message(writer: &mut impl Write, message: &serde_json::Value) -> io::Result<()> {
    writer.write_all(&encode_json_rpc_message(message))?;
    writer.flush()
}

fn read_json_rpc_message(reader: &mut impl BufRead) -> io::Result<Option<serde_json::Value>> {
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }

        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid content length: {error}"),
                )
            })?);
        }
    }

    let Some(content_length) = content_length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing Content-Length header",
        ));
    };

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let message = serde_json::from_slice(&body).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid JSON-RPC body: {error}"),
        )
    })?;
    Ok(Some(message))
}

fn editor_diagnostic_from_lsp_value(value: &serde_json::Value) -> Option<EditorDiagnostic> {
    let range = value.get("range")?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    Some(EditorDiagnostic {
        line: start.get("line")?.as_u64()? as usize,
        start_character: start.get("character")?.as_u64()? as usize,
        end_character: end.get("character")?.as_u64()? as usize,
        severity: DiagnosticSeverity::from_lsp_number(
            value.get("severity").and_then(serde_json::Value::as_u64),
        ),
        message: value.get("message")?.as_str()?.to_string(),
    })
}

fn document_uri(path: &Path) -> Option<String> {
    url::Url::from_file_path(path)
        .ok()
        .map(|uri| uri.to_string())
}

fn uri_to_file_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

impl DiagnosticSeverity {
    fn from_lsp_number(value: Option<u64>) -> Self {
        match value {
            Some(1) => Self::Error,
            Some(2) => Self::Warning,
            Some(3) => Self::Info,
            Some(4) => Self::Hint,
            _ => Self::Info,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warning => 1,
            Self::Info => 2,
            Self::Hint => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    #[test]
    fn language_server_for_path_recognizes_common_code_files() {
        let cases = [
            ("src/main.rs", "rust", "rust-analyzer", &[][..]),
            (
                "web/app.tsx",
                "typescriptreact",
                "typescript-language-server",
                &["--stdio"][..],
            ),
            (
                "web/app.js",
                "javascript",
                "typescript-language-server",
                &["--stdio"][..],
            ),
            (
                "tools/check.py",
                "python",
                "pyright-langserver",
                &["--stdio"][..],
            ),
            ("cmd/con/main.go", "go", "gopls", &[][..]),
        ];

        for (path, language_id, command, args) in cases {
            let server = language_server_for_path(Path::new(path)).expect(path);
            assert_eq!(server.language_id, language_id);
            assert_eq!(server.command, command);
            assert_eq!(server.args, args);
        }

        assert!(language_server_for_path(Path::new("README.md")).is_none());
    }

    #[test]
    fn encode_json_rpc_message_uses_content_length_framing() {
        let message = json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
        let encoded = String::from_utf8(encode_json_rpc_message(&message)).unwrap();
        let (header, body) = encoded.split_once("\r\n\r\n").expect("header/body split");

        assert!(header.starts_with("Content-Length: "));
        let len = header
            .strip_prefix("Content-Length: ")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(len, body.len());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            message
        );
    }

    #[test]
    fn initialize_response_is_detected_by_id_and_result() {
        assert!(is_initialize_response(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "capabilities": {} }
        })));
        assert!(!is_initialize_response(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": { "capabilities": {} }
        })));
        assert!(!is_initialize_response(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "textDocument/publishDiagnostics"
        })));
    }

    #[test]
    fn did_open_message_is_separate_from_initialize() {
        let spec = LanguageServerSpec {
            language_id: "rust",
            command: "rust-analyzer",
            args: &[],
        };
        let path = Path::new("/tmp/main.rs");

        assert_eq!(
            initialize_message(path)
                .get("method")
                .and_then(|m| m.as_str()),
            Some("initialize")
        );
        assert_eq!(
            did_open_message(path, &spec, "fn main() {}")
                .get("method")
                .and_then(|m| m.as_str()),
            Some("textDocument/didOpen")
        );
    }

    #[test]
    fn diagnostics_from_publish_notification_extracts_file_diagnostics() {
        let path = PathBuf::from("/tmp/con editor/sample.rs");
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        let message = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": [
                    {
                        "range": {
                            "start": { "line": 2, "character": 4 },
                            "end": { "line": 2, "character": 9 }
                        },
                        "severity": 1,
                        "source": "rustc",
                        "message": "expected item"
                    }
                ]
            }
        });

        let (diagnostic_path, diagnostics) =
            diagnostics_from_publish_notification(&message).expect("diagnostics");

        assert_eq!(diagnostic_path, path);
        assert_eq!(
            diagnostics,
            vec![EditorDiagnostic {
                line: 2,
                start_character: 4,
                end_character: 9,
                severity: DiagnosticSeverity::Error,
                message: "expected item".to_string(),
            }]
        );
    }

    #[test]
    fn strongest_diagnostic_for_line_prefers_highest_severity() {
        let diagnostics = vec![
            EditorDiagnostic {
                line: 3,
                start_character: 0,
                end_character: 1,
                severity: DiagnosticSeverity::Hint,
                message: "hint".to_string(),
            },
            EditorDiagnostic {
                line: 3,
                start_character: 0,
                end_character: 1,
                severity: DiagnosticSeverity::Warning,
                message: "warning".to_string(),
            },
            EditorDiagnostic {
                line: 4,
                start_character: 0,
                end_character: 1,
                severity: DiagnosticSeverity::Error,
                message: "error".to_string(),
            },
        ];

        assert_eq!(
            strongest_diagnostic_for_line(&diagnostics, 3)
                .unwrap()
                .message,
            "warning"
        );
    }
}
