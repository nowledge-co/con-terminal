use std::path::PathBuf;

#[cfg(any(target_os = "linux", test))]
use std::ffi::OsString;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct StartupArgs {
    pub workspace: Option<PathBuf>,
    pub working_directory: Option<PathBuf>,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub command: Option<TerminalCommand>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl StartupArgs {
    #[cfg(target_os = "linux")]
    pub fn parse_env() -> Result<Self, String> {
        Self::parse(std::env::args_os().skip(1))
    }

    #[cfg(any(target_os = "linux", test))]
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let args = args.into_iter().collect::<Vec<_>>();
        let mut parsed = Self::default();
        let mut index = 0;

        while index < args.len() {
            let arg = &args[index];
            let text = arg
                .to_str()
                .ok_or_else(|| "startup arguments must be valid UTF-8".to_string())?;

            if text == "-e" || text == "--command" {
                let command = args[index + 1..]
                    .iter()
                    .map(|value| {
                        value
                            .to_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "command arguments must be valid UTF-8".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let Some((program, command_args)) = command.split_first() else {
                    return Err(format!("{text} requires a command"));
                };
                if program.is_empty() {
                    return Err(format!("{text} requires a non-empty command"));
                }
                parsed.command = Some(TerminalCommand {
                    program: program.clone(),
                    args: command_args.to_vec(),
                });
                break;
            }

            if let Some(value) =
                option_value(text, "--working-directory").or_else(|| option_value(text, "--dir"))
            {
                set_path_option(&mut parsed.working_directory, value, "--working-directory")?;
                index += 1;
                continue;
            }
            if text == "--working-directory" || text == "--dir" {
                let value = next_value(&args, &mut index, text)?;
                set_path_option(&mut parsed.working_directory, &value, text)?;
                index += 1;
                continue;
            }

            if let Some(value) = option_value(text, "--title") {
                set_string_option(&mut parsed.title, value, "--title")?;
                index += 1;
                continue;
            }
            if text == "--title" {
                let value = next_value(&args, &mut index, text)?;
                set_string_option(&mut parsed.title, &value, text)?;
                index += 1;
                continue;
            }

            if let Some(value) = option_value(text, "--app-id") {
                set_string_option(&mut parsed.app_id, value, "--app-id")?;
                index += 1;
                continue;
            }
            if text == "--app-id" {
                let value = next_value(&args, &mut index, text)?;
                set_string_option(&mut parsed.app_id, &value, text)?;
                index += 1;
                continue;
            }

            if text == "--" {
                let remaining = &args[index + 1..];
                if remaining.len() != 1 {
                    return Err("`--` must be followed by exactly one workspace path".to_string());
                }
                set_workspace(&mut parsed, PathBuf::from(&remaining[0]))?;
                break;
            }
            if text.starts_with('-') {
                return Err(format!("unknown startup option: {text}"));
            }

            set_workspace(&mut parsed, PathBuf::from(arg))?;
            index += 1;
        }

        if parsed.workspace.is_some() && parsed.command.is_some() {
            return Err("a workspace path cannot be combined with -e/--command".to_string());
        }
        if parsed.workspace.is_some() && parsed.working_directory.is_some() {
            return Err(
                "a workspace path cannot be combined with --working-directory/--dir".to_string(),
            );
        }
        Ok(parsed)
    }
}

#[cfg(any(target_os = "linux", test))]
fn option_value<'a>(arg: &'a str, name: &str) -> Option<&'a str> {
    arg.strip_prefix(name)?.strip_prefix('=')
}

#[cfg(any(target_os = "linux", test))]
fn next_value(args: &[OsString], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .ok_or_else(|| format!("{option} requires a value"))?
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{option} value must be valid UTF-8"))
}

#[cfg(any(target_os = "linux", test))]
fn set_string_option(
    destination: &mut Option<String>,
    value: &str,
    option: &str,
) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{option} requires a non-empty value"));
    }
    if destination.replace(value.to_string()).is_some() {
        return Err(format!("{option} may only be supplied once"));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn set_path_option(
    destination: &mut Option<PathBuf>,
    value: &str,
    option: &str,
) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{option} requires a non-empty path"));
    }
    if destination.replace(PathBuf::from(value)).is_some() {
        return Err(format!("{option} may only be supplied once"));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn set_workspace(parsed: &mut StartupArgs, path: PathBuf) -> Result<(), String> {
    if parsed.workspace.replace(path).is_some() {
        Err("only one workspace path may be supplied".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<StartupArgs, String> {
        StartupArgs::parse(args.iter().map(OsString::from))
    }

    #[test]
    fn preserves_existing_workspace_path_flow() {
        assert_eq!(
            parse(&["project/.con/workspace.toml"]).unwrap().workspace,
            Some(PathBuf::from("project/.con/workspace.toml"))
        );
    }

    #[test]
    fn parses_xdg_terminal_contract_without_reinterpreting_command_args() {
        let parsed = parse(&[
            "--working-directory=/tmp/hello world",
            "--title",
            "Build 输出",
            "--app-id=org.omarchy.test",
            "-e",
            "sh",
            "-lc",
            "printf '%s' ready",
        ])
        .unwrap();

        assert_eq!(
            parsed.working_directory,
            Some(PathBuf::from("/tmp/hello world"))
        );
        assert_eq!(parsed.title.as_deref(), Some("Build 输出"));
        assert_eq!(parsed.app_id.as_deref(), Some("org.omarchy.test"));
        assert_eq!(
            parsed.command,
            Some(TerminalCommand {
                program: "sh".into(),
                args: vec!["-lc".into(), "printf '%s' ready".into()],
            })
        );
    }

    #[test]
    fn command_consumes_leading_dash_arguments() {
        let parsed = parse(&["-e", "printf", "--", "-n"]).unwrap();
        assert_eq!(parsed.command.unwrap().args, ["--", "-n"]);
    }

    #[test]
    fn rejects_ambiguous_or_incomplete_startup_requests() {
        assert!(parse(&["one", "two"]).is_err());
        assert!(parse(&["workspace", "-e", "sh"]).is_err());
        assert!(parse(&["-e"]).is_err());
        assert!(parse(&["-e", ""]).is_err());
        assert!(parse(&["--app-id="]).is_err());
        assert!(parse(&["--dir="]).is_err());
        assert!(parse(&["--dir", "/tmp", "--dir", "/var/tmp"]).is_err());
        assert!(parse(&["workspace", "--dir", "/tmp"]).is_err());
        assert!(parse(&["--unknown"]).is_err());
    }

    #[test]
    fn parses_the_omarchy_dir_and_tui_invocations() {
        assert_eq!(
            parse(&["--dir", "/tmp/project with spaces"]).unwrap(),
            StartupArgs {
                working_directory: Some(PathBuf::from("/tmp/project with spaces")),
                ..StartupArgs::default()
            }
        );

        assert_eq!(
            parse(&[
                "--app-id",
                "com.example.编辑器",
                "--title=Editor — 项目",
                "-e",
                "nvim",
                "--",
                "-notes.md",
            ])
            .unwrap()
            .command,
            Some(TerminalCommand {
                program: "nvim".into(),
                args: vec!["--".into(), "-notes.md".into()],
            })
        );
    }
}
