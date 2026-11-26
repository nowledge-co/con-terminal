#![cfg(unix)]

use con_core::program_resume_store::{ProgramResumeStore, TERMINAL_CONTEXT_ENV};
use std::path::Path;
use std::process::{Command, Output};
use uuid::Uuid;

const FIXTURE_ENV: &str = "CON_PROGRAM_RESUME_TEST_FIXTURE";

#[test]
fn replay_shell_runs_only_the_ticket_approved_for_its_terminal() {
    if unsafe { libc::geteuid() } == 0 {
        // The production store refuses records owned by root.
        return;
    }

    let home = std::env::temp_dir().join(format!("con-resume-cli-{}", Uuid::new_v4()));
    std::fs::create_dir(&home).unwrap();
    struct PrivateHome(std::path::PathBuf);
    impl Drop for PrivateHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let home = PrivateHome(home);
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "exercise_private_store",
            "--ignored",
            "--nocapture",
        ])
        .env("HOME", &home.0)
        .env("XDG_DATA_HOME", &home.0)
        .env(FIXTURE_ENV, &home.0)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "launcher fixture failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "runs in a child with isolated application data"]
fn exercise_private_store() {
    let home = std::path::PathBuf::from(std::env::var_os(FIXTURE_ENV).unwrap());
    // Keep the fixture from ever saving into a user's application data.
    assert!(con_paths::app_data_dir().starts_with(&home));
    let path = "/bin/sh";
    let ticket_directory = home.join("work");
    std::fs::create_dir(&ticket_directory).unwrap();
    let data = serde_json::json!({
        "argv": [path, "-c", "printf '<%s>\\n' \"$PWD\" \"$1\" \"$2\" \"$3\"", "_", "", "$(printf expanded)", "a; b"],
        "cwd": ticket_directory,
    });
    let resume = serde_json::from_value(data).unwrap();
    let store = ProgramResumeStore::default();
    let terminal = Uuid::new_v4();
    let candidate = Uuid::new_v4();
    store.save(candidate, &resume).unwrap();
    let cli = Path::new(env!("CARGO_BIN_EXE_con-cli"));
    assert!(!launch(cli, candidate, terminal).status.success());

    let ticket = store.approve(candidate, &resume, terminal).unwrap();
    assert!(!launch(cli, ticket, Uuid::new_v4()).status.success());
    let run = launch(cli, ticket, terminal);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        format!(
            "<{}>\n<>\n<$(printf expanded)>\n<a; b>\n",
            ticket_directory.canonicalize().unwrap().display()
        )
    );
    assert!(!launch(cli, ticket, terminal).status.success());
}

fn launch(cli: &Path, ticket: Uuid, terminal: Uuid) -> Output {
    Command::new(cli)
        .arg("+program-resume")
        .arg(ticket.to_string())
        .env(TERMINAL_CONTEXT_ENV, terminal.to_string())
        .output()
        .unwrap()
}
