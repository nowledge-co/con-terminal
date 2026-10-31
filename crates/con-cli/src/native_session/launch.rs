//! Terminal-local integration and Codex compatibility without client callbacks.
//! The native binary, argv, exit status, signals, history and approvals stay native.
use super::*;
use std::ffi::{OsStr, OsString};

const BEGIN: &str = "# >>> con native-session >>>";
const END: &str = "# <<< con native-session <<<";

pub fn bin_dir() -> PathBuf {
    con_paths::app_data_dir().join("native-session").join("bin")
}

fn shell_quote(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .context("Shell integration path is not UTF-8")?;
    ensure!(
        !text.chars().any(char::is_control),
        "Invalid shell integration path"
    );
    Ok(format!("'{}'", text.replace('\'', "'\\''")))
}

/// Marked blocks are replaced in place; ambiguous/partial markers are errors.
/// Never rewrite user aliases/functions or source files outside this block.
fn edit_rc(text: &str, block: Option<&str>, profile: &str) -> Result<String> {
    let begin = format!("{BEGIN} {profile}");
    let end_marker = format!("{END} {profile}");
    let starts: Vec<_> = text.match_indices(&begin).collect();
    let ends: Vec<_> = text.match_indices(&end_marker).collect();
    ensure!(
        starts.len() == ends.len() && starts.len() <= 1,
        "Ambiguous con shell integration markers; shell configuration left unchanged"
    );
    let mut result = text.to_string();
    if let (Some((start, _)), Some((end, _))) = (starts.first(), ends.first()) {
        ensure!(
            start < end && (*start == 0 || text.as_bytes()[start - 1] == b'\n'),
            "Invalid con shell integration block; shell configuration left unchanged"
        );
        let mut finish = end + end_marker.len();
        if text.as_bytes().get(finish) == Some(&b'\n') {
            finish += 1;
        }
        result.replace_range(*start..finish, block.unwrap_or(""));
    } else if let Some(block) = block {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(block);
    }
    Ok(result)
}

/// Explicit installer only. Other terminals never prepend this directory.
pub fn prepare(
    install: bool,
    rc_override: Option<PathBuf>,
    directory: &Path,
) -> Result<ShellIntegration> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let name = Path::new(&shell)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("");
    ensure!(
        matches!(name, "zsh" | "bash" | "fish"),
        "Automatic Codex integration supports zsh, bash and fish; use native-session launch for other shells"
    );
    let home = dirs::home_dir().context("Cannot locate shell configuration")?;
    let rc = rc_override.unwrap_or_else(|| match name {
        "zsh" => std::env::var_os("ZDOTDIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.clone())
            .join(".zshrc"),
        "bash" => home.join(if cfg!(target_os = "macos") {
            ".bash_profile"
        } else {
            ".bashrc"
        }),
        _ => std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("fish/config.fish"),
    });
    ensure!(
        rc.is_absolute() && !rc.is_symlink(),
        "Shell configuration must be an absolute, non-symlinked path"
    );
    let old = if rc.exists() {
        std::fs::read_to_string(&rc)?
    } else {
        String::new()
    };
    let bin = shell_quote(&bin_dir())?;
    let body = if name == "fish" {
        format!(
            "if set -q CON_TERMINAL_CONTEXT; and set -q CON_SOCKET_PATH\n    if not contains -- {bin} $PATH\n        set -gx PATH {bin} $PATH\n    end\nend\n"
        )
    } else {
        format!(
            "if [ -n \"${{CON_TERMINAL_CONTEXT:-}}\" ] && [ -n \"${{CON_SOCKET_PATH:-}}\" ]; then\n    case \":$PATH:\" in *:{bin}:*) ;; *) export PATH={bin}:\"$PATH\" ;; esac\nfi\n"
        )
    };
    let profile = shell_quote(directory)?;
    let block = format!("{BEGIN} {profile}\n{body}{END} {profile}\n");
    let new = edit_rc(&old, install.then_some(block.as_str()), &profile)?;
    let launchers = if install {
        vec![prepare_launcher("codex")?]
    } else {
        Vec::new()
    };
    Ok(ShellIntegration {
        rc,
        old,
        new,
        launchers,
    })
}

fn prepare_launcher(agent: &str) -> Result<(PathBuf, String, Option<String>)> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let script = format!(
        "#!/bin/sh\n# Managed by con native-session install.\nexec {} native-session launch --agent {agent} --launcher-dir {} -- \"$@\"\n",
        shell_quote(&executable)?,
        shell_quote(&bin_dir())?,
    );
    let adapter = native_agent_adapter(agent).context("Unsupported native agent")?;
    let path = bin_dir().join(adapter.executable);
    let old = if path.exists() {
        ensure!(
            !path.is_symlink(),
            "Refusing to replace a symlinked native launcher"
        );
        let old = std::fs::read_to_string(&path)?;
        ensure!(
            old.contains("# Managed by con native-session install."),
            "Refusing to replace an unmanaged native launcher"
        );
        Some(old)
    } else {
        None
    };
    Ok((path, script, old))
}

pub fn runtime_dir() -> PathBuf {
    con_paths::app_data_dir().join("native-session/runtime")
}

/// Writes only con-owned resources. User shell startup files and native agent
/// profiles are never read or modified by automatic preparation.
pub fn prepare_automatic() -> Result<()> {
    for agent in ["claude", "codex"] {
        let (path, script, _) = prepare_launcher(agent)?;
        write_runtime(&path, script.as_bytes(), true)?;
    }
    let runtime = runtime_dir();
    for (name, script) in [
        ("zsh/.zshenv", include_str!("shell/zshenv")),
        ("zsh/.zprofile", include_str!("shell/zprofile")),
        ("zsh/.zshrc", include_str!("shell/zshrc")),
        ("zsh/.zlogin", include_str!("shell/zlogin")),
    ] {
        write_runtime(&runtime.join(name), script.as_bytes(), false)?;
    }
    let plugin = runtime.join("claude-plugin");
    write_runtime(&plugin.join(".claude-plugin/plugin.json"), br#"{"name":"con-native-session","version":"1.0.0","description":"Link this native conversation to its con terminal"}"#, false)?;
    let hooks = json!({"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":hook_command("claude")?,"timeout":5}]}]}});
    write_runtime(
        &plugin.join("hooks/hooks.json"),
        &serde_json::to_vec(&hooks)?,
        false,
    )?;
    Ok(())
}

fn write_runtime(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    ensure!(
        !path.is_symlink(),
        "Refusing to replace a symlinked con integration resource"
    );
    if std::fs::read(path).ok().as_deref() != Some(bytes) {
        install::atomic_replace(path, bytes)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }),
        )?;
    }
    Ok(())
}

fn hook_command(agent: &str) -> Result<String> {
    Ok(format!(
        "{} native-session hook --agent {agent}",
        shell_quote(&std::env::current_exe()?.canonicalize()?)?
    ))
}

fn runtime_arguments(agent: &str, runtime: &Path) -> Result<Vec<OsString>> {
    match agent {
        "codex" => {
            let command = toml_edit::Value::from(hook_command(agent)?).to_string();
            Ok(vec!["--enable".into(), "hooks".into(), "-c".into(),
                format!("hooks.SessionStart=[{{hooks=[{{type=\"command\",command={command},timeout=5}}]}}]").into()])
        }
        "claude" => Ok(vec![
            "--plugin-dir".into(),
            runtime.join("claude-plugin").into_os_string(),
        ]),
        _ => Ok(Vec::new()),
    }
}

pub struct ShellIntegration {
    pub rc: PathBuf,
    old: String,
    new: String,
    launchers: Vec<(PathBuf, String, Option<String>)>,
}

impl ShellIntegration {
    pub fn apply(&self) -> Result<()> {
        let current = if self.rc.exists() {
            std::fs::read_to_string(&self.rc)?
        } else {
            String::new()
        };
        ensure!(
            !self.rc.is_symlink() && current == self.old,
            "Shell configuration changed during installation; left unchanged"
        );
        // Validate every destination before changing any of them.
        for (path, _, old) in &self.launchers {
            let current = if path.exists() {
                Some(std::fs::read_to_string(path)?)
            } else {
                None
            };
            ensure!(
                !path.is_symlink() && &current == old,
                "Native launcher changed during installation; left unchanged"
            );
        }
        let result = (|| -> Result<()> {
            for (path, script, old) in &self.launchers {
                if old.as_ref() != Some(script) {
                    install::atomic_replace(path, script.as_bytes())?;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
                }
            }
            if self.old != self.new {
                install::atomic_replace(&self.rc, self.new.as_bytes())?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            for (path, script, old) in &self.launchers {
                let current = std::fs::read_to_string(path).ok();
                if current.as_ref() == old.as_ref() || current.is_none() {
                    continue;
                }
                ensure!(
                    current.as_ref() == Some(script),
                    "Launcher changed during rollback; inspect private backups"
                );
                if let Some(old) = old {
                    install::atomic_replace(path, old.as_bytes())?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
                    }
                } else {
                    std::fs::remove_file(path)?;
                }
            }
            return Err(error);
        }
        Ok(())
    }
}

fn is_interactive(args: &[OsString]) -> bool {
    let mut value = false;
    let mut first_positional = None;
    for argument in args {
        if value {
            value = false;
            continue;
        }
        let Some(arg) = argument.to_str() else {
            return false;
        };
        if arg == "--" {
            break;
        }
        if matches!(
            arg,
            "--remote" | "--no-daemon" | "--help" | "-h" | "--version" | "-V" | "--ephemeral"
        ) || arg.starts_with("--remote=")
        {
            return false;
        }
        if matches!(
            arg,
            "-c" | "--config"
                | "-m"
                | "--model"
                | "-p"
                | "--profile"
                | "-C"
                | "--cd"
                | "-a"
                | "--ask-for-approval"
                | "-s"
                | "--sandbox"
                | "--add-dir"
                | "--enable"
                | "--disable"
                | "-i"
                | "--image"
                | "--local-provider"
                | "--remote-cwd"
        ) {
            value = true;
        } else if !arg.starts_with('-') && first_positional.is_none() {
            first_positional = Some(arg);
        }
    }
    !first_positional.is_some_and(|arg| {
        matches!(
            arg,
            "exec"
                | "e"
                | "review"
                | "login"
                | "logout"
                | "mcp"
                | "mcp-server"
                | "app-server"
                | "completion"
                | "sandbox"
                | "debug"
                | "apply"
                | "a"
                | "cloud"
                | "features"
                | "remote-control"
                | "help"
                | "agents"
        )
    })
}

fn native_interactive(agent: &str, args: &[OsString]) -> bool {
    // Native batch, management, and remote clients keep their existing behavior.
    !args.iter().any(|arg| {
        let arg = arg.to_string_lossy();
        matches!(
            arg.as_ref(),
            "--help"
                | "-h"
                | "--version"
                | "-V"
                | "--print"
                | "--headless"
                | "--bare"
                | "--safe-mode"
                | "--no-session-persistence"
                | "--acp"
                | "--remote"
                | "--remote-server"
                | "--cloud"
                | "--autopilot"
        ) || arg.starts_with("--remote=")
            || arg.starts_with("--remote-server=")
            || arg.starts_with("--print=")
            || arg.starts_with("--prompt=")
            || ((agent != "codex") && arg == "-p")
    }) && !args
        .first()
        .and_then(|arg| arg.to_str())
        .is_some_and(|arg| {
            matches!(
                arg,
                "login"
                    | "logout"
                    | "auth"
                    | "mcp"
                    | "update"
                    | "upgrade"
                    | "install"
                    | "doctor"
                    | "help"
                    | "completion"
                    | "agent"
            )
        })
}

pub fn run(agent: &str, args: Vec<OsString>, launcher_dir: Option<PathBuf>) -> Result<()> {
    let adapter = native_agent_adapter(agent).context("Unsupported native agent")?;
    let bin = launcher_dir.unwrap_or_else(bin_dir);
    let original = std::env::var_os("PATH").unwrap_or_default();
    let paths: Vec<_> = std::env::split_paths(&original)
        .filter(|p| p != &bin && p.canonicalize().ok() != bin.canonicalize().ok())
        .collect();
    let executable = paths
        .iter()
        .map(|p| p.join(adapter.executable))
        .find(|p| {
            if p.canonicalize().ok().is_some_and(|resolved| {
                Some(resolved) == bin.join(adapter.executable).canonicalize().ok()
            }) {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                p.is_file()
                    && p.metadata()
                        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                p.is_file()
            }
        })
        .with_context(|| {
            format!(
                "{} is not installed on PATH; con does not install it",
                adapter.label
            )
        })?;
    let local = std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::env::var("CON_TERMINAL_CONTEXT").is_ok_and(|id| uuid::Uuid::parse_str(&id).is_ok())
        && std::env::var_os("CON_SOCKET_PATH").is_some()
        && (agent != "codex" || std::env::var_os("CODEX_THREAD_ID").is_none())
        && (agent != "claude" || std::env::var_os("CLAUDECODE").is_none())
        && std::env::var_os("SSH_CONNECTION").is_none();
    let interactive = if agent == "codex" {
        is_interactive(
            &args
                .iter()
                .filter(|arg| arg.as_os_str() != "--no-daemon")
                .cloned()
                .collect::<Vec<_>>(),
        )
    } else {
        native_interactive(agent, &args)
    };
    let automatic =
        local && interactive && std::env::var("CON_NATIVE_SESSION_AUTO").as_deref() == Ok("1");
    let runtime_args = if automatic {
        runtime_arguments(
            agent,
            &bin.parent()
                .context("Missing launcher parent")?
                .join("runtime"),
        )?
    } else {
        Vec::new()
    };
    let enabled = !automatic
        && agent == "codex"
        && config_dir("codex")
            .ok()
            .and_then(|dir| std::fs::read(dir.join("hooks.json")).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|settings| install::contains_own_hook(&settings, "codex"));
    let mut command = Command::new(executable);
    if agent == "codex" && (enabled || automatic) && local && is_interactive(&args) {
        command.arg("--no-daemon");
    }
    command
        .args(runtime_args)
        .args(args)
        .env("PATH", std::env::join_paths(paths)?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).with_context(|| format!("Could not start {}", adapter.label))
    }
    #[cfg(not(unix))]
    {
        let status = command.status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_launch_policy_preserves_batch_and_remote_clients() {
        for agent in ["claude", "cursor", "qwen", "copilot"] {
            for args in [vec![], vec!["--resume", "exact-id"]] {
                assert!(native_interactive(
                    agent,
                    &args.into_iter().map(Into::into).collect::<Vec<_>>()
                ));
            }
            for args in [
                vec!["--help"],
                vec!["--print", "hello"],
                vec!["-p", "hello"],
                vec!["--remote", "host"],
                vec!["--acp"],
                vec!["--prompt=hello"],
                vec!["login"],
            ] {
                assert!(!native_interactive(
                    agent,
                    &args.into_iter().map(Into::into).collect::<Vec<_>>()
                ));
            }
        }
    }
    #[test]
    fn launch_policy_preserves_noninteractive_and_explicit_server_commands() {
        for args in [
            vec![],
            vec!["resume", "exact-id"],
            vec!["-m", "exec", "resume", "id"],
            vec!["--", "exec"],
        ] {
            assert!(is_interactive(
                &args.into_iter().map(Into::into).collect::<Vec<_>>()
            ));
        }
        for args in [
            vec!["exec", "hi"],
            vec!["-m", "cheap", "exec", "hi"],
            vec!["app-server"],
            vec!["--remote", "unix:///x"],
            vec!["--no-daemon"],
            vec!["resume", "id", "--remote", "unix:///x"],
            vec!["--help"],
        ] {
            assert!(!is_interactive(
                &args.into_iter().map(Into::into).collect::<Vec<_>>()
            ));
        }
    }
    #[test]
    fn shell_block_roundtrips_without_touching_user_configuration() {
        let original = "# my configuration\nalias codex='my-custom-command'\n";
        let block = format!("{BEGIN} first\nmanaged\n{END} first\n");
        let added = edit_rc(original, Some(&block), "first").unwrap();
        assert_eq!(edit_rc(&added, Some(&block), "first").unwrap(), added);
        assert_eq!(edit_rc(&added, None, "first").unwrap(), original);
        assert!(edit_rc(&format!("{BEGIN} first\npartial"), Some(&block), "first").is_err());
        assert!(edit_rc(&format!("{block}{block}"), None, "first").is_err());
        let second = format!("{BEGIN} second\nother profile\n{END} second\n");
        let both = edit_rc(&added, Some(&second), "second").unwrap();
        assert_eq!(
            edit_rc(&both, None, "first").unwrap(),
            format!("{original}{second}")
        );
    }
}
