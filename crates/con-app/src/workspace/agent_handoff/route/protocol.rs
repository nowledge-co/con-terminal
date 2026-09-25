//! Every dispatch requires a sibling con-cli speaking protocol 2 or newer.
//! Older helpers may silently omit prompt delivery even without a model override.

use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Bounds on the helper protocol query: a stuck or chatty con-cli must fail
/// the gate (a reviewable launch error), never stall it — the reserved job is
/// already `LaunchPending` and the expiry timer only starts once the new Tab
/// exists. One deadline covers both the process wait and the pipe drain.
const HELPER_PROTOCOL_QUERY_TIMEOUT: Duration = Duration::from_secs(4);
const HELPER_PROTOCOL_MAX_OUTPUT: usize = 4096;

pub(super) fn check_helper_protocol() -> anyhow::Result<()> {
    check_helper_path(&super::super::con_cli_path()?)
}

fn check_helper_path(cli: &std::path::Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        cli.is_file(),
        "Handoff cannot start: con-cli is missing beside the Con executable ({}); rebuild Con and con-cli from the same version with `cargo build -p con -p con-cli` or reinstall Con",
        cli.display()
    );
    let protocol = query_launch_helper_protocol(cli)?;
    ensure_helper_protocol_version(protocol)
}

/// The contract's version floor: a helper older than
/// `LAUNCH_HELPER_PROTOCOL` does not understand `target_model` and must be
/// refused, never silently launched with the target's default model.
fn ensure_helper_protocol_version(protocol: u64) -> anyhow::Result<()> {
    let required = con_core::handoff::LAUNCH_HELPER_PROTOCOL;
    anyhow::ensure!(
        protocol >= u64::from(required),
        "Handoff needs launch-helper protocol {required}, but the con-cli beside Con speaks {protocol}. Upgrade con-cli and rebuild Con from the same version — refusing to launch."
    );
    Ok(())
}

/// Run `con-cli handoff protocol` and return the reported
/// `launch_helper_protocol` revision. Every failure mode (non-zero exit,
/// malformed report, timeout, oversized output) is an error so the caller
/// refuses the launch instead of silently downgrading the model.
fn query_launch_helper_protocol(cli: &std::path::Path) -> anyhow::Result<u64> {
    query_launch_helper_protocol_bounded(
        cli,
        HELPER_PROTOCOL_QUERY_TIMEOUT,
        HELPER_PROTOCOL_MAX_OUTPUT,
    )
}

fn query_launch_helper_protocol_bounded(
    cli: &std::path::Path,
    timeout: Duration,
    max_output: usize,
) -> anyhow::Result<u64> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(cli)
        .env_clear()
        .envs(con_agent::handoff::probe_environment())
        .args(["handoff", "protocol"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout_reader = read_bounded(LiveReaderSlots::global(), child.stdout.take(), max_output);
    let stderr_reader = read_bounded(LiveReaderSlots::global(), child.stderr.take(), max_output);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // Do NOT wait on the readers here: a grandchild of the killed
                // helper may still hold the pipe write ends, and waiting would
                // stall the gate until that process exits. The detached reader
                // threads finish whenever the pipes close.
                anyhow::bail!(
                    "Handoff cannot start: the con-cli beside Con did not answer the launch-helper protocol query within {timeout:?}. Upgrade con-cli and rebuild Con from the same version — refusing to launch."
                );
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    // The child exited, but a descendant may still hold the pipe write ends
    // (e.g. a wrapper script's background job): join the readers with only
    // the deadline's remaining budget so the same deadline covers the pipe
    // drain, then detach them — they finish whenever the pipes close.
    let stdout = recv_bounded(&stdout_reader, deadline)?;
    let stderr = recv_bounded(&stderr_reader, deadline)?;
    anyhow::ensure!(
        stdout.len() <= max_output && stderr.len() <= max_output,
        "Handoff cannot start: the con-cli beside Con exceeded the {max_output}-byte protocol report limit. Upgrade con-cli and rebuild Con from the same version — refusing to launch."
    );
    anyhow::ensure!(
        status.success(),
        "Handoff cannot start: the con-cli beside Con cannot report its launch-helper protocol (it predates protocol reporting). Upgrade con-cli and rebuild Con from the same version — refusing to launch. {}",
        String::from_utf8_lossy(&stderr).trim()
    );
    serde_json::from_slice::<serde_json::Value>(&stdout)?
        .get("launch_helper_protocol")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("con-cli reported a malformed launch-helper protocol; rebuild Con and con-cli from the same version"))
}

/// Detached reader threads only finish when their pipe closes, so a helper
/// whose descendant keeps a write end open would otherwise accumulate one
/// thread per outstanding query; past this cap the query fails fast instead.
const MAX_LIVE_READERS: usize = 16;

/// Live-reader slot accounting. The query path shares one process-global
/// pool; tests instantiate their own, so slot-balance assertions are exact
/// and never race readers that earlier tests' timed-out helpers left behind.
#[derive(Default)]
struct LiveReaderSlots {
    live: std::sync::atomic::AtomicUsize,
}

impl LiveReaderSlots {
    /// The query path's process-global pool.
    fn global() -> &'static std::sync::Arc<Self> {
        static GLOBAL: std::sync::OnceLock<std::sync::Arc<LiveReaderSlots>> =
            std::sync::OnceLock::new();
        GLOBAL.get_or_init(Default::default)
    }

    #[cfg(test)]
    fn live(&self) -> usize {
        self.live.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Reserve a slot, or refuse once `MAX_LIVE_READERS` readers are alive;
    /// a refused reservation is rolled back at once, not leaked.
    fn try_reserve(self: &std::sync::Arc<Self>) -> Option<LiveReaderGuard> {
        use std::sync::atomic::Ordering;
        if self.live.fetch_add(1, Ordering::SeqCst) >= MAX_LIVE_READERS {
            self.live.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(LiveReaderGuard {
            slots: self.clone(),
        })
    }
}

/// Releases the reserved live-reader slot from any exit path: the decrement
/// lives in `Drop`, so even a panicking reader thread — or a closure dropped
/// because the OS refused to spawn its thread — cannot leak its slot and
/// wedge later protocol queries against `MAX_LIVE_READERS`.
struct LiveReaderGuard {
    slots: std::sync::Arc<LiveReaderSlots>,
}

impl Drop for LiveReaderGuard {
    fn drop(&mut self) {
        self.slots
            .live
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Drain a child pipe on a helper thread, capped at `max_output + 1` bytes so
/// an over-limit report is detectable yet can never grow memory without
/// bound; once the cap is hit the reader returns, and its end of the pipe is
/// dropped, which makes a chatty child fail with EPIPE instead of blocking
/// the wait loop.
fn read_bounded(
    slots: &std::sync::Arc<LiveReaderSlots>,
    pipe: Option<impl std::io::Read + Send + 'static>,
    max_output: usize,
) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    let Some(guard) = slots.try_reserve() else {
        return rx;
    };
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("con-handoff-pipe-reader".into())
        .spawn(move || {
            // The guard releases the slot even if this thread panics; if the
            // OS refuses the spawn, the dropped closure releases it instead.
            let _guard = guard;
            use std::io::Read;
            let mut bytes = Vec::new();
            let result = match pipe {
                Some(pipe) => pipe
                    .take(max_output as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes),
                None => Ok(bytes),
            };
            let _ = tx.send(result);
        });
    if let Err(error) = spawned {
        // No thread is running, so nothing else will answer: fail the query
        // through the channel instead of silently producing an empty report.
        let _ = fallback.send(Err(error));
    }
    rx
}

/// Wait for a pipe reader, bounded by the query deadline's remaining budget.
fn recv_bounded(
    reader: &mpsc::Receiver<std::io::Result<Vec<u8>>>,
    deadline: Instant,
) -> anyhow::Result<Vec<u8>> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    match reader.recv_timeout(remaining) {
        Ok(result) => Ok(result?),
        Err(mpsc::RecvTimeoutError::Timeout) => anyhow::bail!(
            "Handoff cannot start: the con-cli beside Con left a descendant holding the protocol report pipe open past the query deadline. Upgrade con-cli and rebuild Con from the same version — refusing to launch."
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!(
            "Handoff cannot start: too many launch-helper protocol queries are stuck on inherited pipes. Upgrade con-cli and rebuild Con from the same version — refusing to launch."
        ),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        LiveReaderSlots, MAX_LIVE_READERS, ensure_helper_protocol_version,
        query_launch_helper_protocol_bounded, read_bounded,
    };
    use std::path::PathBuf;
    use std::time::Duration;

    /// `LIVE_READERS` was process-global, so every test that spawns pipe
    /// readers through a real query must hold this lock; otherwise the
    /// parallel readers could collectively hit `MAX_LIVE_READERS` and fail
    /// queries for the wrong reason. Slot-balance tests use their own
    /// `LiveReaderSlots` instance instead, so they need no lock and make
    /// exact assertions.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        SERIAL.lock().unwrap_or_else(|error| error.into_inner())
    }

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Install a fake `con-cli` as an executable shell script.
    fn fake_cli(body: &str) -> (TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "con-handoff-gate-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = dir.join("con-cli");
        std::fs::write(&cli, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        (TempDir(dir), cli)
    }

    fn query(cli: &std::path::Path) -> anyhow::Result<u64> {
        query_launch_helper_protocol_bounded(cli, Duration::from_secs(4), 4096)
    }

    #[test]
    fn every_dispatch_rejects_missing_or_old_sibling_without_a_model() {
        let _serial = serial();
        let (_guard, cli) = fake_cli("echo '{\"launch_helper_protocol\":1}'");
        assert!(
            super::check_helper_path(&cli)
                .unwrap_err()
                .to_string()
                .contains("same version")
        );
        assert!(super::check_helper_path(&cli.with_file_name("missing")).is_err());
    }

    #[test]
    fn helper_without_protocol_reporting_is_refused() {
        let _serial = serial();
        // An old con-cli predates `handoff protocol` and exits non-zero.
        let (_guard, cli) =
            fake_cli("echo \"error: unrecognized subcommand 'protocol'\" >&2\nexit 2");
        let error = query(&cli).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("predates protocol reporting"), "{message}");
        assert!(message.contains("Upgrade con-cli"), "{message}");
        assert!(message.contains("refusing to launch"), "{message}");
    }

    #[test]
    fn helper_with_older_protocol_is_refused_by_the_version_floor() {
        let _serial = serial();
        let (_guard, cli) = fake_cli("echo '{\"launch_helper_protocol\":1}'");
        let protocol = query(&cli).unwrap();
        let error = ensure_helper_protocol_version(protocol).unwrap_err();
        assert!(error.to_string().contains("Upgrade con-cli"), "{error}");
        ensure_helper_protocol_version(u64::from(con_core::handoff::LAUNCH_HELPER_PROTOCOL))
            .unwrap();
    }

    #[test]
    fn helper_with_malformed_report_is_refused() {
        let _serial = serial();
        let (_guard, cli) = fake_cli("echo '{\"unexpected\":true}'");
        let error = query(&cli).unwrap_err();
        assert!(error.to_string().contains("malformed"), "{error}");
    }

    #[test]
    fn stuck_helper_times_out_instead_of_stalling_the_gate() {
        let _serial = serial();
        let (_guard, cli) = fake_cli("sleep 30");
        let started = std::time::Instant::now();
        let error = query_launch_helper_protocol_bounded(&cli, Duration::from_millis(300), 4096)
            .unwrap_err();
        assert!(error.to_string().contains("did not answer"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5), "gate stalled");
    }

    #[test]
    fn exited_helper_with_a_pipe_holding_descendant_does_not_stall_the_gate() {
        let _serial = serial();
        // The parent prints a valid report and exits at once, but a
        // background child inherits the output pipes: the gate must stop at
        // the same deadline instead of waiting for the descendant.
        let (_guard, cli) = fake_cli("echo '{\"launch_helper_protocol\":2}'\nsleep 30 &\nexit 0");
        let started = std::time::Instant::now();
        let error = query_launch_helper_protocol_bounded(&cli, Duration::from_millis(300), 4096)
            .unwrap_err();
        assert!(error.to_string().contains("descendant"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5), "gate stalled");
    }

    #[test]
    fn chatty_helper_exceeding_the_output_limit_is_refused() {
        let _serial = serial();
        let (_guard, cli) = fake_cli("yes x");
        let error =
            query_launch_helper_protocol_bounded(&cli, Duration::from_secs(4), 64).unwrap_err();
        assert!(error.to_string().contains("report limit"), "{error}");
    }

    #[test]
    fn current_helper_protocol_is_accepted() {
        let _serial = serial();
        let (_guard, cli) = fake_cli("echo '{\"launch_helper_protocol\":2}'");
        let protocol = query(&cli).unwrap();
        ensure_helper_protocol_version(protocol).unwrap();
    }

    /// A finished reader returns its slot: the count must fall back to zero
    /// once the thread exits, or later queries would eventually hit
    /// `MAX_LIVE_READERS` and be refused. The private instance makes the
    /// assertion exact — a leaked slot can never be masked by another
    /// test's reader exiting.
    #[test]
    fn a_finished_reader_releases_its_live_reader_slot() {
        let slots = std::sync::Arc::new(LiveReaderSlots::default());
        let rx = read_bounded(&slots, None::<std::io::Empty>, 8);
        // No intermediate count assertion: an Empty pipe lets the reader
        // thread finish — and release its slot — before this thread resumes.
        let result = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        assert!(result.is_empty());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while slots.live() != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(slots.live(), 0, "reader thread leaked its slot");
    }

    /// The guard's `Drop` is the only release path, so a panicking reader
    /// thread still returns its slot instead of leaking it.
    #[test]
    fn the_reader_guard_releases_its_slot_even_on_panic() {
        let slots = std::sync::Arc::new(LiveReaderSlots::default());
        let guard = slots.try_reserve().unwrap();
        assert_eq!(slots.live(), 1);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = guard;
            panic!("simulated pipe reader panic");
        }));
        assert!(result.is_err());
        assert_eq!(slots.live(), 0, "panicking reader leaked its slot");
    }

    /// Over the cap the query is refused and the reservation is rolled back
    /// at once; releasing the held guards then returns the count to zero.
    /// Every assertion is exact because the instance is private to the test.
    #[test]
    fn over_the_cap_the_query_is_refused_without_leaking_a_slot() {
        let slots = std::sync::Arc::new(LiveReaderSlots::default());
        let held: Vec<_> = (0..MAX_LIVE_READERS)
            .map(|_| slots.try_reserve().unwrap())
            .collect();
        assert_eq!(slots.live(), MAX_LIVE_READERS);
        assert!(slots.try_reserve().is_none(), "cap not enforced");
        assert_eq!(slots.live(), MAX_LIVE_READERS, "refusal leaked a slot");
        let rx = read_bounded(&slots, None::<std::io::Empty>, 8);
        assert_eq!(slots.live(), MAX_LIVE_READERS, "refused reader leaked");
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ));
        drop(held);
        assert_eq!(slots.live(), 0, "released guards did not return slots");
    }
}
