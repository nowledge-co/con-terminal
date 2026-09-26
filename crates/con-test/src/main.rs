mod parser;
mod runner;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;

// ---------------------------------------------------------------------------
// ANSI color helpers — disabled when stdout is not a tty or NO_COLOR is set
// ---------------------------------------------------------------------------

struct Color {
    enabled: bool,
}

impl Color {
    fn detect() -> Self {
        // Respect NO_COLOR env var (https://no-color.org/)
        if std::env::var_os("NO_COLOR").is_some() {
            return Color { enabled: false };
        }
        // Check if stdout is a tty
        Color {
            enabled: is_tty_stdout(),
        }
    }

    fn green<'a>(&self, s: &'a str) -> ColorStr<'a> {
        ColorStr {
            s,
            code: "32",
            enabled: self.enabled,
        }
    }
    fn red<'a>(&self, s: &'a str) -> ColorStr<'a> {
        ColorStr {
            s,
            code: "31",
            enabled: self.enabled,
        }
    }
    fn yellow<'a>(&self, s: &'a str) -> ColorStr<'a> {
        ColorStr {
            s,
            code: "33",
            enabled: self.enabled,
        }
    }
    fn bold<'a>(&self, s: &'a str) -> ColorStr<'a> {
        ColorStr {
            s,
            code: "1",
            enabled: self.enabled,
        }
    }
    fn dim<'a>(&self, s: &'a str) -> ColorStr<'a> {
        ColorStr {
            s,
            code: "2",
            enabled: self.enabled,
        }
    }
}

struct ColorStr<'a> {
    s: &'a str,
    code: &'static str,
    enabled: bool,
}

impl std::fmt::Display for ColorStr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.enabled {
            write!(f, "\x1b[{}m{}\x1b[0m", self.code, self.s)
        } else {
            write!(f, "{}", self.s)
        }
    }
}

#[cfg(unix)]
fn is_tty_stdout() -> bool {
    unsafe extern "C" {
        fn isatty(fd: i32) -> i32;
    }
    // SAFETY: fd 1 is always valid (stdout)
    unsafe { isatty(1) != 0 }
}

#[cfg(not(unix))]
fn is_tty_stdout() -> bool {
    false
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "con-test",
    about = "Logic test runner for con-cli — drives a live con session via .test files"
)]
struct Cli {
    /// Paths to .test files or directories containing .test files
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// Path to the con binary to launch (defaults to sibling in cargo target/)
    #[arg(long, value_name = "PATH")]
    con: Option<PathBuf>,

    /// Path to the con-cli binary (defaults to sibling in cargo target/)
    #[arg(long, value_name = "PATH")]
    con_cli: Option<PathBuf>,

    /// Control endpoint path for the launched con process
    /// (default: temp Unix socket on Unix, named pipe on Windows)
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// Seconds to wait for con to start (default: 30)
    #[arg(long, default_value_t = 30)]
    startup_timeout: u64,

    /// Rewrite expected output in .test files from actual output (baseline mode)
    #[arg(long)]
    rewrite: bool,

    /// Stop after the first failing test file
    #[arg(long)]
    fail_fast: bool,

    /// Show full pass/skip results in addition to failures
    #[arg(long)]
    verbose: bool,

    /// Disable colored output
    #[arg(long)]
    no_color: bool,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn default_control_endpoint() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\con-test-{}", std::process::id()))
    }

    #[cfg(not(windows))]
    {
        std::env::temp_dir().join(format!("con-test-{}.sock", std::process::id()))
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let color = if cli.no_color {
        Color { enabled: false }
    } else {
        Color::detect()
    };

    let con_bin = match resolve_bin(cli.con.as_deref(), app_binary_name()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{} {e}", color.red("error:"));
            return ExitCode::FAILURE;
        }
    };

    let con_cli = match resolve_bin(cli.con_cli.as_deref(), "con-cli") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{} {e}", color.red("error:"));
            return ExitCode::FAILURE;
        }
    };

    let socket = cli.socket.unwrap_or_else(default_control_endpoint);

    let test_files = match collect_test_files(&cli.paths) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("{} collecting test files: {e}", color.red("error:"));
            return ExitCode::FAILURE;
        }
    };

    if test_files.is_empty() {
        eprintln!(
            "{}",
            color.yellow("no .test files found in the given paths")
        );
        return ExitCode::FAILURE;
    }

    // Launch a single con process for the entire test run.
    println!("{} con ({})...", color.dim("launching"), con_bin.display());
    let _con_process = match runner::ConProcess::launch(
        &con_bin,
        &socket,
        Duration::from_secs(cli.startup_timeout),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{} failed to launch con: {e}", color.red("error:"));
            return ExitCode::FAILURE;
        }
    };
    println!(
        "{} {}\n",
        color.dim("con ready on"),
        color.dim(&socket.display().to_string())
    );

    let config = runner::RunConfig {
        con_cli: con_cli.clone(),
        socket: socket.clone(),
        rewrite: cli.rewrite,
        verbose: cli.verbose,
    };

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    for path in &test_files {
        if let Err(e) = runner::reset_state(&con_cli, &socket) {
            eprintln!(
                "  {} reset_state failed before {}: {e}",
                color.red("error:"),
                path.display()
            );
            failed += 1;
            if cli.fail_fast {
                break;
            }
            continue;
        }

        match runner::run_file(path, &config) {
            Ok(result) => {
                print_file_result(path, &result, &color);
                passed += result.passed;
                failed += result.failed;
                skipped += result.skipped;
                if cli.fail_fast && result.failed > 0 {
                    break;
                }
            }
            Err(e) => {
                eprintln!("  {} running {}: {e}", color.red("error:"), path.display());
                failed += 1;
                if cli.fail_fast {
                    break;
                }
            }
        }
    }

    println!();

    // Summary line
    let summary = format!(
        "{} passed  {} failed  {} skipped  ({} files)",
        passed,
        failed,
        skipped,
        test_files.len()
    );
    if failed > 0 {
        println!("{}", color.bold(&format!("results: {summary}")));
        ExitCode::FAILURE
    } else {
        println!("{}", color.bold(&format!("results: {summary}")));
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn print_file_result(path: &Path, result: &runner::FileResult, color: &Color) {
    let path_str = path.display().to_string();
    let counts = format!(
        "({} passed, {} failed, {} skipped)",
        result.passed, result.failed, result.skipped
    );

    if result.failed > 0 {
        println!(
            "{}  {}  {}",
            color.red("FAIL"),
            path_str,
            color.dim(&counts)
        );
        for failure in &result.failures {
            println!("  {} {}", color.red("---"), color.bold(&failure.step_label));
            println!("      {}  {}", color.dim("cmd:"), failure.cmd);
            println!(
                "      {}  {}",
                color.dim("expected:"),
                failure.expected.trim()
            );
            println!(
                "      {}  {}",
                color.yellow("actual:  "),
                failure.actual.trim()
            );
            if !failure.diff.is_empty() {
                for line in failure.diff.lines() {
                    if line.starts_with('+') {
                        println!("      {}", color.green(line));
                    } else if line.starts_with('-') {
                        println!("      {}", color.red(line));
                    } else {
                        println!("      {}", color.dim(line));
                    }
                }
            }
        }
    } else {
        println!(
            "{}  {}  {}",
            color.green("ok  "),
            path_str,
            color.dim(&counts)
        );
    }
}

// ---------------------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------------------

/// Resolve a binary path.
/// Priority: explicit flag → documented env var → cargo target/ sibling → PATH
fn resolve_bin(flag: Option<&Path>, name: &str) -> Result<PathBuf> {
    if let Some(p) = flag {
        return Ok(p.to_path_buf());
    }

    let env_key = env_key_for_bin(name);
    if let Ok(env) = std::env::var(env_key) {
        return Ok(PathBuf::from(env));
    }

    // Sibling in the same target directory as con-test itself.
    let sibling = {
        let mut p = std::env::current_exe().unwrap_or_default();
        p.pop();
        p.push(platform_exe_name(name));
        p
    };
    if sibling.exists() {
        return Ok(sibling);
    }

    // Fall back to PATH.
    let output = Command::new(path_lookup_command()).arg(name).output();
    match output {
        Ok(o) if o.status.success() => {
            let path = String::from_utf8_lossy(&o.stdout).trim().to_string();
            Ok(PathBuf::from(path))
        }
        _ => anyhow::bail!(
            "{name} not found. Build it with `cargo build -p {name}` or pass --{name} <path>"
        ),
    }
}

fn env_key_for_bin(name: &str) -> &'static str {
    match name {
        "con" | "con-app" => "CON",
        "con-cli" => "CON_CLI",
        _ => "CON_BIN",
    }
}

fn platform_exe_name(name: &str) -> String {
    #[cfg(windows)]
    {
        if name.ends_with(".exe") {
            name.to_string()
        } else {
            format!("{name}.exe")
        }
    }

    #[cfg(not(windows))]
    {
        name.to_string()
    }
}

#[cfg(windows)]
fn app_binary_name() -> &'static str {
    "con-app"
}

#[cfg(not(windows))]
fn app_binary_name() -> &'static str {
    "con"
}

#[cfg(windows)]
fn path_lookup_command() -> &'static str {
    "where"
}

#[cfg(not(windows))]
fn path_lookup_command() -> &'static str {
    "which"
}

// ---------------------------------------------------------------------------
// File collection
// ---------------------------------------------------------------------------

fn collect_test_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            collect_from_dir(path, &mut files)?;
        } else if path.extension().map(|e| e == "test").unwrap_or(false) {
            files.push(path.clone());
        } else {
            anyhow::bail!("{} is not a .test file or directory", path.display());
        }
    }
    files.sort();
    Ok(files)
}

fn collect_from_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_from_dir(&path, out)?;
        } else if path.extension().map(|e| e == "test").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // Keep existing production item ordering.
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn env_keys_match_documented_overrides() {
        assert_eq!(env_key_for_bin("con"), "CON");
        assert_eq!(env_key_for_bin("con-app"), "CON");
        assert_eq!(env_key_for_bin("con-cli"), "CON_CLI");
    }

    #[test]
    fn default_socket_endpoint_is_platform_appropriate() {
        let endpoint = default_control_endpoint();
        let endpoint = endpoint.to_string_lossy();
        #[cfg(windows)]
        assert!(endpoint.starts_with(r"\\.\pipe\con-test-"));
        #[cfg(not(windows))]
        assert!(endpoint.ends_with(".sock"));
    }

    #[test]
    fn app_binary_name_is_platform_appropriate() {
        #[cfg(windows)]
        assert_eq!(app_binary_name(), "con-app");
        #[cfg(not(windows))]
        assert_eq!(app_binary_name(), "con");
    }

    #[test]
    fn sibling_lookup_uses_platform_exe_name() {
        #[cfg(windows)]
        assert_eq!(platform_exe_name("con-cli"), "con-cli.exe");
        #[cfg(not(windows))]
        assert_eq!(platform_exe_name("con-cli"), "con-cli");
    }

    #[test]
    fn explicit_flag_wins_over_env() {
        let _guard = EnvGuard::set("CON", "/tmp/ignored-con");
        let resolved = resolve_bin(Some(Path::new("/tmp/explicit-con")), "con").unwrap();
        assert_eq!(resolved, PathBuf::from("/tmp/explicit-con"));
    }

    #[test]
    fn documented_env_override_is_used() {
        let _guard = EnvGuard::set("CON_CLI", "/tmp/documented-con-cli");
        let resolved = resolve_bin(None, "con-cli").unwrap();
        assert_eq!(resolved, PathBuf::from("/tmp/documented-con-cli"));
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}

use std::process::Command;
