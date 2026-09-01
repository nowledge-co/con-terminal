//! Con's compatibility implementation of Ghostty's `+ssh` helper.
//!
//! The shell integration is intentionally a thin wrapper around this command.
//! SSH arguments are kept as OsStrings and are passed to the real ssh process
//! without going through a shell.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ssh_cache::Cache;

const TERMINFO_NAME: &str = "xterm-ghostty";
const FALLBACK_TERM: &str = "xterm-256color";
const CON_TERM: &str = "xterm-ghostty";

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    forward_env: bool,
    terminfo: bool,
    cache: bool,
    ssh: OsString,
    verbose: bool,
    ssh_args: Vec<OsString>,
}

impl Options {
    fn parse(args: &[OsString]) -> Result<Self, String> {
        let mut options = Self {
            forward_env: true,
            terminfo: true,
            cache: true,
            ssh: OsString::from("ssh"),
            ..Default::default()
        };
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if arg == "--" {
                options.ssh_args.extend(args[index + 1..].iter().cloned());
                break;
            }
            if !arg.to_string_lossy().starts_with("--") {
                options.ssh_args.extend(args[index..].iter().cloned());
                break;
            }

            let value = arg.to_string_lossy();
            match value.as_ref() {
                "--help" => return Err(usage()),
                "--verbose" => options.verbose = true,
                "--forward-env" => options.forward_env = true,
                "--forward-env=false" => options.forward_env = false,
                "--terminfo" => options.terminfo = true,
                "--terminfo=false" => options.terminfo = false,
                "--cache" => options.cache = true,
                "--cache=false" => options.cache = false,
                value if value.starts_with("--ssh=") => {
                    let path = &value[6..];
                    if path.is_empty() {
                        return Err("--ssh requires a path".to_string());
                    }
                    options.ssh = OsString::from(path);
                }
                value if value.starts_with("--forward-env=") => {
                    options.forward_env = parse_bool("--forward-env", &value[14..])?;
                }
                value if value.starts_with("--terminfo=") => {
                    options.terminfo = parse_bool("--terminfo", &value[11..])?;
                }
                value if value.starts_with("--cache=") => {
                    options.cache = parse_bool("--cache", &value[8..])?;
                }
                _ => return Err(format!("unknown +ssh flag: {arg:?}\n\n{}", usage())),
            }
            index += 1;
        }
        if options.ssh_args.is_empty() {
            return Err(format!("no ssh arguments provided\n\n{}", usage()));
        }
        Ok(options)
    }
}

fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{name} expects true or false")),
    }
}

fn usage() -> String {
    "Usage: con-cli +ssh [flags] [--] <ssh args...>\n\n\
Flags:\n\
  --forward-env[=bool]  Forward terminal environment (default: true)\n\
  --terminfo[=bool]     Install xterm-ghostty terminfo (default: true)\n\
  --cache[=bool]        Use the Con terminfo cache (default: true)\n\
  --ssh=<path>          SSH executable (default: ssh)\n\
  --verbose             Print setup diagnostics\n\
  --help                Show this help\n"
        .to_string()
}

pub fn run(args: &[OsString]) -> i32 {
    let options = match Options::parse(args) {
        Ok(options) => options,
        Err(error) if error.starts_with("Usage:") => {
            println!("{error}");
            return 0;
        }
        Err(error) => {
            eprintln!("con-cli +ssh: {error}");
            return 2;
        }
    };
    run_with_options(&options)
}

fn run_with_options(options: &Options) -> i32 {
    let destination = resolve_destination(&options.ssh, &options.ssh_args);
    let mut term = FALLBACK_TERM;
    let mut cache_entry = None;
    let mut control_path = None;

    if options.terminfo {
        if let Some(destination) = destination.as_deref() {
            let cached =
                options.cache && Cache::user().contains(destination, None).unwrap_or(false);
            if cached {
                term = CON_TERM;
                verbose(options, "terminfo cache hit for {destination}");
            } else if let Some(payload) = local_terminfo() {
                match install_terminfo(options, &payload) {
                    Ok(path) => {
                        term = CON_TERM;
                        control_path = Some(path);
                        if options.cache {
                            cache_entry = Some(destination.to_string());
                        }
                    }
                    Err(error) => verbose(options, &format!("terminfo setup skipped: {error}")),
                }
            } else {
                verbose(
                    options,
                    "terminfo setup skipped: local {TERMINFO_NAME} entry unavailable",
                );
            }
        } else {
            verbose(
                options,
                "terminfo setup skipped: could not resolve SSH destination",
            );
        }
    }

    let mut command = Command::new(&options.ssh);
    if options.forward_env {
        let term_option = format!("SetEnv=TERM={term}");
        command.arg("-o").arg(term_option);
        command.args(["-o", "SendEnv=COLORTERM"]);
        command.args(["-o", "SendEnv=TERM_PROGRAM"]);
        command.args(["-o", "SendEnv=TERM_PROGRAM_VERSION"]);
    }
    if let Some(path) = control_path.as_ref() {
        let control_option = format!("ControlPath={}", path);
        command.arg("-o").arg(control_option);
    }
    command.args(&options.ssh_args);
    verbose(options, &format!("exec: {:?}", command));
    let status = match command.status() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("con-cli +ssh: failed to run {:?}: {error}", options.ssh);
            return 1;
        }
    };

    if status.success() {
        if let Some(destination) = cache_entry {
            if Cache::user().add(&destination).is_ok() {
                verbose(options, &format!("cache: wrote {destination}"));
            }
        }
    }
    if let Some(path) = control_path {
        let _ = fs::remove_file(&path);
        if let Some(parent) = Path::new(&path).parent() {
            let _ = fs::remove_dir(parent);
        }
    }
    status.code().unwrap_or(1)
}

fn verbose(options: &Options, message: &str) {
    if options.verbose {
        eprintln!("+ssh: {message}");
    }
}

fn resolve_destination(ssh: &OsStr, args: &[OsString]) -> Option<String> {
    let output = Command::new(ssh).arg("-G").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut user = None;
    let mut host = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "user" => user = Some(value),
            "hostname" => host = Some(value),
            _ => {}
        }
    }
    Some(format!("{}@{}", user?, host?))
}

fn local_terminfo() -> Option<Vec<u8>> {
    let mut command = Command::new("infocmp");
    command.args(["-0", "-Q2", "-q", TERMINFO_NAME]);
    if std::env::var_os("TERMINFO").is_none() {
        if let Some(path) = bundled_terminfo_dir() {
            command.env("TERMINFO", path);
        }
    }
    let output = command.output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn bundled_terminfo_dir() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(|app| app.join("Contents/Resources/terminfo"))
        .filter(|path| path.is_dir())
}

fn install_terminfo(options: &Options, payload: &[u8]) -> io::Result<String> {
    let directory = unique_temp_dir()?;
    let control_path = directory.join("socket");
    let control_path_string = control_path.to_string_lossy().into_owned();
    let control_option = format!("ControlPath={control_path_string}");
    let result = (|| {
        let mut command = Command::new(&options.ssh);
        command
            .args(["-o", "ControlMaster=yes"])
            .args(["-o", "ControlPersist=no"])
            .args(["-o", &control_option])
            .args(&options.ssh_args)
            .arg("command -v tic >/dev/null 2>&1 || exit 1; mkdir -p ~/.terminfo 2>/dev/null && tic -x - 2>/dev/null");
        command.stdin(Stdio::piped()).stdout(Stdio::null());
        if !options.verbose {
            command.stderr(Stdio::null());
        }
        let mut child = command.spawn()?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(payload)?;
        }
        let status = child.wait()?;
        if status.success() {
            Ok(control_path_string)
        } else {
            Err(io::Error::other("remote terminfo installation failed"))
        }
    })();
    if result.is_err() {
        let _ = fs::remove_dir(&directory);
    }
    result
}

fn unique_temp_dir() -> io::Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("con-ssh-{}-{timestamp}", std::process::id()));
    fs::create_dir(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_wrapper_flags_and_forwards_ssh_args() {
        let parsed = Options::parse(&args(&[
            "--forward-env=false",
            "--terminfo=false",
            "--cache=false",
            "--verbose",
            "--ssh=/custom/ssh",
            "--",
            "-p",
            "2222",
            "user@example.com",
        ]))
        .unwrap();
        assert!(!parsed.forward_env);
        assert!(!parsed.terminfo);
        assert!(!parsed.cache);
        assert!(parsed.verbose);
        assert_eq!(parsed.ssh, "/custom/ssh");
        assert_eq!(parsed.ssh_args, args(&["-p", "2222", "user@example.com"]));
    }

    #[test]
    fn first_ssh_argument_stops_wrapper_parsing() {
        let parsed = Options::parse(&args(&["-p", "2222", "user@example.com"])).unwrap();
        assert_eq!(parsed.ssh_args, args(&["-p", "2222", "user@example.com"]));
    }

    #[test]
    fn rejects_unknown_wrapper_flags() {
        assert!(Options::parse(&args(&["--wat", "host"])).is_err());
    }

    #[test]
    fn parses_destination_from_ssh_config_output() {
        let output = "hostname example.com\nuser alice\n";
        let mut user = None;
        let mut host = None;
        for line in output.lines() {
            let (key, value) = line.split_once(' ').unwrap();
            match key {
                "user" => user = Some(value),
                "hostname" => host = Some(value),
                _ => {}
            }
        }
        assert_eq!(
            format!("{}@{}", user.unwrap(), host.unwrap()),
            "alice@example.com"
        );
    }
}
