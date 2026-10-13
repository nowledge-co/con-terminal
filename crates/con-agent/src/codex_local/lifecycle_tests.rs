use super::*;

pub(super) struct FixtureDir(pub(super) PathBuf);
impl FixtureDir {
    pub(super) fn new() -> Self {
        let path = std::env::temp_dir().join(format!("con-codex-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, executable: bool) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
            )
            .unwrap();
        }
        #[cfg(not(unix))]
        let _ = executable;
        path
    }
}
impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn discovery_honors_explicit_path_and_reports_invalid_overrides_without_falling_back() {
    let fixture = FixtureDir::new();
    let executable = fixture.file("custom-codex", true);
    let bundled = fixture.file("Codex.app/Contents/Resources/codex", true);
    let empty_path = std::env::join_paths(std::iter::empty::<PathBuf>()).unwrap();
    assert_eq!(
        resolve_executable(
            Some(executable.as_os_str()),
            &empty_path,
            std::slice::from_ref(&bundled)
        )
        .unwrap(),
        executable
    );
    assert_eq!(
        resolve_executable(None, &empty_path, std::slice::from_ref(&bundled)).unwrap(),
        bundled
    );
    for invalid in [
        PathBuf::from("relative-codex"),
        fixture.0.join("missing"),
        fixture.0.clone(),
    ] {
        assert!(
            resolve_executable(
                Some(invalid.as_os_str()),
                &empty_path,
                std::slice::from_ref(&bundled)
            )
            .unwrap_err()
            .to_string()
            .contains("CON_CODEX_BIN")
        );
    }
}

#[test]
fn discovery_respects_path_order_before_bundled_clients() {
    let fixture = FixtureDir::new();
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let first = fixture.file(&format!("first/{name}"), true);
    let second = fixture.file(&format!("second/{name}"), true);
    let path = std::env::join_paths([first.parent().unwrap(), second.parent().unwrap()]).unwrap();
    assert_eq!(
        resolve_executable(None, &path, std::slice::from_ref(&second)).unwrap(),
        first
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(resolve_executable(None, &path, &[]).unwrap(), second);
        assert!(resolve_executable(Some(first.as_os_str()), &path, &[]).is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_and_dropped_futures_stop_the_dedicated_child_process_group() {
    for explicit_shutdown in [true, false] {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "sleep 300 & printf '{\"childPid\":%s}\\n' \"$!\"; wait",
        ]);
        let mut server = AppServer::spawn_command(command).unwrap();
        let pid = server.read().await.unwrap()["childPid"].as_i64().unwrap() as i32;
        if explicit_shutdown {
            server.shutdown().await;
        }
        drop(server);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                // SAFETY: signal 0 observes only the known fixture process.
                if unsafe { libc::kill(pid, 0) } == -1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("npm-style launcher descendant survived Con cancellation");
    }
}
