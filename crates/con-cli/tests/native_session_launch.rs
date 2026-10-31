//! Exercise the installed entry point without any model, user profile or live app.
#![cfg(target_os = "macos")]

use std::{
    fs,
    io::Read,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    process::{Command, Output},
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    profile: PathBuf,
    rc: PathBuf,
    launcher: PathBuf,
    native_bin: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("con-native-test-{}", uuid::Uuid::new_v4()));
        let home = root.join("home ' with spaces");
        let profile = root.join("profile ' with spaces");
        let native_bin = root.join("native-bin");
        for dir in [&home, &profile, &native_bin] {
            fs::create_dir_all(dir).unwrap();
        }
        let rc = home.join(".zshrc");
        fs::write(
            &rc,
            "# preserve my config\nalias special='echo untouched'\n",
        )
        .unwrap();
        let launcher = home.join("Library/Application Support/con/native-session/bin/codex");
        let fake = native_bin.join("codex");
        fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$$\" \"$@\"\nexit 7\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            home,
            profile,
            rc,
            launcher,
            native_bin,
        }
    }
    fn install(&self, install: bool) -> Output {
        Command::new(env!("CARGO_BIN_EXE_con-cli"))
            .args([
                "--json",
                "native-session",
                if install { "install" } else { "uninstall" },
                "--agent",
                "codex",
                "--config-dir",
            ])
            .arg(&self.profile)
            .arg("--shell-rc")
            .arg(&self.rc)
            .env("HOME", &self.home)
            .env("SHELL", "/bin/zsh")
            .output()
            .unwrap()
    }
    fn launch(&self, args: &[&str], con: bool, alias_path: bool) -> (u32, Vec<String>) {
        self.launch_agent("codex", args, con, alias_path, false)
    }
    fn launch_agent(
        &self,
        agent: &str,
        args: &[&str],
        con: bool,
        alias_path: bool,
        automatic: bool,
    ) -> (u32, Vec<String>) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let executable = con_core::native_session::native_agent_adapter(agent)
            .unwrap()
            .executable;
        let mut cmd = CommandBuilder::new(self.launcher.with_file_name(executable));
        cmd.args(args);
        // A different HOME reproduces the native macOS terminal environment;
        // the installed launcher's explicit directory must prevent recursion.
        cmd.env("HOME", &self.root);
        cmd.env("CODEX_HOME", &self.profile);
        cmd.env("CLAUDE_CONFIG_DIR", &self.profile);
        cmd.env_remove("CLAUDECODE");
        cmd.env("CON_NATIVE_SESSION_AUTO", if automatic { "1" } else { "0" });
        cmd.env_remove("CODEX_THREAD_ID");
        cmd.env_remove("SSH_CONNECTION");
        cmd.env_remove("CON_TERMINAL_CONTEXT");
        cmd.env_remove("CON_SOCKET_PATH");
        if con {
            cmd.env("CON_TERMINAL_CONTEXT", uuid::Uuid::new_v4().to_string());
            cmd.env("CON_SOCKET_PATH", self.root.join("unused.sock"));
        }
        let alias = self.root.join("launcher-alias");
        let first = if alias_path {
            symlink(self.launcher.parent().unwrap(), &alias).unwrap();
            alias.as_path()
        } else {
            self.launcher.parent().unwrap()
        };
        cmd.env(
            "PATH",
            std::env::join_paths([
                first,
                self.native_bin.as_path(),
                std::path::Path::new("/usr/bin"),
                std::path::Path::new("/bin"),
            ])
            .unwrap(),
        );
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        let expected_pid = child.process_id().expect("native process ID");
        drop(pair.slave);
        let mut output = String::new();
        // macOS PTYs can finish reads with EIO after delivering all output.
        let _ = pair
            .master
            .try_clone_reader()
            .unwrap()
            .read_to_string(&mut output);
        let status = child.wait().unwrap();
        assert_eq!(status.exit_code(), 7, "{output}");
        let mut lines = output.lines();
        let pid = lines
            .find_map(|line| line.trim().parse::<u32>().ok())
            .expect("native PID");
        assert_eq!(
            pid, expected_pid,
            "the launcher must exec the native process"
        );
        (
            pid,
            lines
                .map(|line| line.trim_end_matches('\r').to_string())
                .collect(),
        )
    }
}

#[test]
fn automatic_preparation_and_launch_do_not_modify_native_profiles_or_shell_files() {
    let fixture = Fixture::new();
    fs::copy(
        fixture.native_bin.join("codex"),
        fixture.native_bin.join("claude"),
    )
    .unwrap();
    let original_rc = fs::read(&fixture.rc).unwrap();
    let settings = br#"{"theme":"keep","hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"user-hook"}]}]}}"#;
    let config = "model = 'keep'\n[profiles.work]\nmodel = 'also-keep'\n";
    fs::write(fixture.profile.join("settings.json"), settings).unwrap();
    fs::write(fixture.profile.join("hooks.json"), settings).unwrap();
    fs::write(fixture.profile.join("config.toml"), config).unwrap();
    let prepare = || {
        Command::new(env!("CARGO_BIN_EXE_con-cli"))
            .args(["--json", "native-session", "prepare"])
            .env("HOME", &fixture.home)
            .env("SHELL", "/bin/zsh")
            .output()
            .unwrap()
    };
    let output = prepare();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(prepare().status.success());
    assert_eq!(
        fixture
            .launch_agent("codex", &["exec", "hi"], true, false, true)
            .1,
        ["exec", "hi"]
    );
    assert_eq!(
        fixture.launch_agent("codex", &[], false, false, true).1,
        Vec::<String>::new()
    );
    let args = fixture
        .launch_agent("codex", &["--no-daemon"], true, false, true)
        .1;
    assert_eq!(&args[..3], ["--enable", "hooks", "-c"]);
    assert!(args[3].starts_with("hooks.SessionStart="));
    assert!(args[3].contains("native-session hook --agent codex"));
    assert_eq!(args.last().unwrap(), "--no-daemon");
    let args = fixture
        .launch_agent(
            "claude",
            &[
                "--resume",
                "exact-session",
                "--settings",
                "my-settings.json",
            ],
            true,
            false,
            true,
        )
        .1;
    assert_eq!(args[0], "--plugin-dir");
    let plugin = fixture
        .launcher
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("runtime/claude-plugin");
    assert_eq!(std::path::Path::new(&args[1]), plugin);
    assert_eq!(
        &args[2..],
        [
            "--resume",
            "exact-session",
            "--settings",
            "my-settings.json"
        ]
    );
    assert!(plugin.join("hooks/hooks.json").is_file());
    assert_eq!(fs::read(&fixture.rc).unwrap(), original_rc);
    assert_eq!(
        fs::read(fixture.profile.join("settings.json")).unwrap(),
        settings
    );
    assert_eq!(
        fs::read(fixture.profile.join("hooks.json")).unwrap(),
        settings
    );
    assert_eq!(
        fs::read_to_string(fixture.profile.join("config.toml")).unwrap(),
        config
    );
    assert_eq!(
        fs::read_dir(&fixture.profile).unwrap().count(),
        3,
        "no profile backups or hidden state may be written by con"
    );
}

#[test]
fn private_zsh_bridge_preserves_user_startup_order_and_effective_zdotdir() {
    let fixture = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_con-cli"))
        .args(["native-session", "prepare"])
        .env("HOME", &fixture.home)
        .output()
        .unwrap();
    assert!(output.status.success());
    let runtime = fixture
        .launcher
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("runtime");
    let user_config = fixture.root.join("user-zdotdir");
    fs::create_dir_all(&user_config).unwrap();
    fs::write(
        fixture.home.join(".zshenv"),
        "print -r -- env >> \"$CON_TEST_TRACE\"\nexport ZDOTDIR=\"$CON_TEST_USER_CONFIG\"\n",
    )
    .unwrap();
    for (file, event) in [
        (".zprofile", "profile"),
        (".zshrc", "rc"),
        (".zlogin", "login"),
    ] {
        fs::write(
            user_config.join(file),
            format!("print -r -- {event} >> \"$CON_TEST_TRACE\"\n"),
        )
        .unwrap();
    }
    for (mode, expected) in [("-lic", "env\nprofile\nrc\nlogin\n"), ("-ic", "env\nrc\n")] {
        let trace = fixture.root.join(format!("trace-{mode}"));
        let output = Command::new("/bin/zsh")
            .args([
                mode,
                "printf 'RESTORED:%s\\n' \"$ZDOTDIR\"; command -v codex",
            ])
            .env("HOME", &fixture.home)
            .env("ZDOTDIR", runtime.join("zsh"))
            .env("CON_NATIVE_ORIGINAL_ZDOTDIR_SET", "1")
            .env("CON_NATIVE_ORIGINAL_ZDOTDIR", &fixture.home)
            .env("CON_NATIVE_SESSION_BIN", fixture.launcher.parent().unwrap())
            .env("CON_NATIVE_SHELL_ROOT", &runtime)
            .env("CON_TEST_TRACE", &trace)
            .env("CON_TEST_USER_CONFIG", &user_config)
            .env(
                "GHOSTTY_RESOURCES_DIR",
                fixture.root.join("no-ghostty-resources"),
            )
            .env_remove("GHOSTTY_ZSH_ZDOTDIR")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_to_string(trace).unwrap(), expected);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            text.contains(&format!("RESTORED:{}", user_config.display())),
            "{text}"
        );
        assert!(
            text.contains(&fixture.launcher.display().to_string()),
            "{text}"
        );
    }
}

#[test]
fn private_zsh_bridge_preserves_native_history_and_user_overrides() {
    for custom_zdotdir in [false, true] {
        for custom_history in [false, true] {
            let fixture = Fixture::new();
            let output = Command::new(env!("CARGO_BIN_EXE_con-cli"))
                .args(["native-session", "prepare"])
                .env("HOME", &fixture.home)
                .output()
                .unwrap();
            assert!(output.status.success());
            let runtime = fixture
                .launcher
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("runtime");
            let user_config = if custom_zdotdir {
                let dir = fixture.root.join("user configuration");
                fs::create_dir_all(&dir).unwrap();
                fs::write(
                    fixture.home.join(".zshenv"),
                    "export ZDOTDIR=\"$CON_TEST_USER_CONFIG\"\n",
                )
                .unwrap();
                dir
            } else {
                fixture.home.clone()
            };
            let history_file = if custom_history {
                fixture.root.join("custom command history")
            } else {
                user_config.join(".zsh_history")
            };
            fs::write(
                user_config.join(".zshrc"),
                if custom_history {
                    "HISTFILE=\"$CON_TEST_HISTORY\"\n"
                } else {
                    "# retain the system history default\n"
                },
            )
            .unwrap();
            for mode in ["-lic", "-ic"] {
                let original = ": 1700000000:0;echo preserved-history\n";
                fs::write(&history_file, original).unwrap();
                let mut command = Command::new("/bin/zsh");
                command.args([mode, "print -r -- HISTORY_FILE:$HISTFILE; fc -R; fc -ln -1; print -s -- con-history-new-entry; fc -AI"])
                    .env("HOME", &fixture.home)
                    .env("ZDOTDIR", runtime.join("zsh"))
                    .env("CON_NATIVE_ORIGINAL_ZDOTDIR_SET", "0")
                    .env("CON_NATIVE_SHELL_ROOT", &runtime)
                    .env("CON_NATIVE_SESSION_BIN", fixture.launcher.parent().unwrap())
                    .env("CON_TEST_USER_CONFIG", &user_config)
                    .env("CON_TEST_HISTORY", &history_file)
                    .env("GHOSTTY_RESOURCES_DIR", fixture.root.join("no-ghostty-resources"))
                    .env_remove("GHOSTTY_ZSH_ZDOTDIR")
                    .env_remove("HISTFILE");
                let output = command.output().unwrap();
                let text = String::from_utf8_lossy(&output.stdout);
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    text.contains(&format!("HISTORY_FILE:{}", history_file.display())),
                    "{mode} custom_zdotdir={custom_zdotdir} custom_history={custom_history}: {text}"
                );
                assert!(text.contains("echo preserved-history"), "{text}");
                let saved = fs::read_to_string(&history_file).unwrap();
                assert!(saved.contains("echo preserved-history"), "{saved}");
                assert!(saved.contains("con-history-new-entry"), "{saved}");
                assert!(!runtime.join("zsh/.zsh_history").exists());
            }
        }
    }
}

#[test]
fn private_zsh_bridge_recovers_misdirected_history_automatically_once() {
    for mode in ["-lic", "-ic"] {
        let fixture = Fixture::new();
        let output = Command::new(env!("CARGO_BIN_EXE_con-cli"))
            .args(["native-session", "prepare"])
            .env("HOME", &fixture.home)
            .output()
            .unwrap();
        assert!(output.status.success());
        let runtime = fixture
            .launcher
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("runtime");
        let source = runtime.join("zsh/.zsh_history");
        let target = fixture.home.join(".zsh_history");
        let old = b": 1700000000:0;old-user-command\n";
        let recent = b": 1700000010:0;recent-con-command\n";
        fs::write(&target, old).unwrap();
        fs::write(&source, recent).unwrap();
        for _ in 0..2 {
            let output = Command::new("/bin/zsh")
                .args([mode, "fc -R; fc -ln -10"])
                .env("HOME", &fixture.home)
                .env("ZDOTDIR", runtime.join("zsh"))
                .env("CON_NATIVE_ORIGINAL_ZDOTDIR_SET", "0")
                .env("CON_NATIVE_SHELL_ROOT", &runtime)
                .env("CON_NATIVE_SESSION_BIN", fixture.launcher.parent().unwrap())
                .env(
                    "GHOSTTY_RESOURCES_DIR",
                    fixture.root.join("no-ghostty-resources"),
                )
                .env_remove("GHOSTTY_ZSH_ZDOTDIR")
                .env_remove("HISTFILE")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(
                text.contains("old-user-command") && text.contains("recent-con-command"),
                "{text}"
            );
            // zsh owns subsequent history serialization (e.g. a shell without
            // EXTENDED_HISTORY drops timestamps on exit). The recovery's raw
            // byte preservation is checked before that native rewrite in its
            // unit tests; both commands must remain exactly once here.
            let saved = fs::read_to_string(&target).unwrap();
            assert_eq!(saved.matches("old-user-command").count(), 1);
            assert_eq!(saved.matches("recent-con-command").count(), 1);
            assert_eq!(fs::read(&source).unwrap(), recent);
        }
        let backup_bytes: Vec<_> = fs::read_dir(runtime.join("zsh/history-recovery"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "source" || extension == "target")
            })
            .map(|entry| fs::read(entry.path()).unwrap())
            .collect();
        assert_eq!(backup_bytes.len(), 2);
        assert!(backup_bytes.contains(&old.to_vec()));
        assert!(backup_bytes.contains(&recent.to_vec()));
    }
}

#[test]
fn installed_launcher_preserves_argv_exit_status_and_other_terminal_behavior() {
    let fixture = Fixture::new();
    let result = fixture.install(true);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(fixture.launcher.is_file());
    assert_eq!(
        fixture
            .launch(&["resume", "id with ' quotes"], true, false)
            .1,
        ["--no-daemon", "resume", "id with ' quotes"]
    );
    assert_eq!(
        fixture.launch(&["resume", "exact-id"], false, false).1,
        ["resume", "exact-id"]
    );
    assert_eq!(
        fixture.launch(&["exec", "a short prompt"], true, false).1,
        ["exec", "a short prompt"]
    );
    assert_eq!(
        fixture
            .launch(
                &["resume", "id", "--remote", "unix:///explicit"],
                true,
                false
            )
            .1,
        ["resume", "id", "--remote", "unix:///explicit"]
    );
    assert_eq!(fixture.launch(&[], true, true).1, ["--no-daemon"]);
}

#[test]
fn installer_roundtrips_shell_and_refuses_partial_marker_before_native_writes() {
    let fixture = Fixture::new();
    let original = fs::read(&fixture.rc).unwrap();
    assert!(fixture.install(true).status.success());
    let first = fs::read(&fixture.rc).unwrap();
    assert!(fixture.install(true).status.success());
    assert_eq!(fs::read(&fixture.rc).unwrap(), first);
    assert!(
        Command::new("/bin/zsh")
            .arg("-n")
            .arg(&fixture.rc)
            .status()
            .unwrap()
            .success()
    );
    assert!(fixture.install(false).status.success());
    assert_eq!(fs::read(&fixture.rc).unwrap(), original);
    assert!(fixture.install(true).status.success());
    let hooks = fs::read(fixture.profile.join("hooks.json")).unwrap();
    let config = fs::read(fixture.profile.join("config.toml")).unwrap();
    let invalid = fs::read_to_string(&fixture.rc)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("# <<< con native-session"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&fixture.rc, &invalid).unwrap();
    assert!(!fixture.install(false).status.success());
    assert_eq!(fs::read_to_string(&fixture.rc).unwrap(), invalid);
    assert_eq!(fs::read(fixture.profile.join("hooks.json")).unwrap(), hooks);
    assert_eq!(
        fs::read(fixture.profile.join("config.toml")).unwrap(),
        config
    );
}

#[test]
fn cursor_installer_uses_native_xdg_store_without_touching_default_profile() {
    let fixture = Fixture::new();
    let xdg = fixture.root.join("xdg");
    let store = xdg.join("cursor");
    fs::create_dir_all(&store).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_con-cli"))
        .args(["--json", "native-session", "install", "--agent", "cursor"])
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env_remove("CURSOR_CONFIG_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let hooks: serde_json::Value =
        serde_json::from_slice(&fs::read(store.join("hooks.json")).unwrap()).unwrap();
    assert_eq!(hooks["version"], 1);
    assert!(
        hooks["hooks"]["sessionStart"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("native-session hook --agent cursor")
    );
    assert!(!fixture.home.join(".cursor/hooks.json").exists());
    assert!(!fixture.launcher.exists());
}
