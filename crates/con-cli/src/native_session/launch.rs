//! Opt-in, terminal-local compatibility for Codex versions without client hooks.
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
    let launcher = if install {
        let executable = std::env::current_exe()?.canonicalize()?;
        let script = format!(
            "#!/bin/sh\n# Managed by con native-session install.\nexec {} native-session launch --agent codex --launcher-dir {} -- \"$@\"\n",
            shell_quote(&executable)?,
            bin
        );
        let path = bin_dir().join("codex");
        let old = if path.exists() {
            ensure!(
                !path.is_symlink(),
                "Refusing to replace a symlinked Codex launcher"
            );
            let old = std::fs::read_to_string(&path)?;
            ensure!(
                old.contains("# Managed by con native-session install."),
                "Refusing to replace an unmanaged Codex launcher"
            );
            Some(old)
        } else {
            None
        };
        Some((path, script, old))
    } else {
        None
    };
    Ok(ShellIntegration {
        rc,
        old,
        new,
        launcher,
    })
}

pub struct ShellIntegration {
    pub rc: PathBuf,
    old: String,
    new: String,
    launcher: Option<(PathBuf, String, Option<String>)>,
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
        if let Some((path, script, old)) = &self.launcher {
            let current = if path.exists() {
                Some(std::fs::read_to_string(path)?)
            } else {
                None
            };
            ensure!(
                !path.is_symlink() && &current == old,
                "Codex launcher changed during installation; left unchanged"
            );
            if old.as_ref() != Some(script) {
                install::atomic_replace(path, script.as_bytes())?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        if self.old != self.new
            && let Err(error) = install::atomic_replace(&self.rc, self.new.as_bytes())
        {
            if let Some((path, _, old)) = &self.launcher {
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
            "--remote" | "--no-daemon" | "--help" | "-h" | "--version" | "-V"
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

pub fn run(args: Vec<OsString>, launcher_dir: Option<PathBuf>) -> Result<()> {
    let bin = launcher_dir.unwrap_or_else(bin_dir);
    let original = std::env::var_os("PATH").unwrap_or_default();
    let paths: Vec<_> = std::env::split_paths(&original)
        .filter(|p| p != &bin && p.canonicalize().ok() != bin.canonicalize().ok())
        .collect();
    let executable = paths
        .iter()
        .map(|p| p.join("codex"))
        .find(|p| {
            if p.canonicalize()
                .ok()
                .is_some_and(|resolved| Some(resolved) == bin.join("codex").canonicalize().ok())
            {
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
        .context("Codex is not installed on PATH; the con integration does not install it")?;
    let enabled = config_dir("codex")
        .ok()
        .and_then(|dir| std::fs::read(dir.join("hooks.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|settings| install::contains_own_hook(&settings, "codex"));
    let local = std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::env::var("CON_TERMINAL_CONTEXT").is_ok_and(|id| uuid::Uuid::parse_str(&id).is_ok())
        && std::env::var_os("CON_SOCKET_PATH").is_some()
        && std::env::var_os("CODEX_THREAD_ID").is_none()
        && std::env::var_os("SSH_CONNECTION").is_none();
    let mut command = Command::new(executable);
    if enabled && local && is_interactive(&args) {
        command.arg("--no-daemon");
    }
    command.args(args).env("PATH", std::env::join_paths(paths)?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("Could not start Codex")
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
