#[cfg(unix)]
use std::ffi::OsStr;
use std::ffi::OsString;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

#[cfg(unix)]
use anyhow::Context;
use anyhow::Result;
use clap::Args;

#[derive(Args, Clone, Debug)]
pub struct PtyBridgeArgs {
    #[arg(long)]
    pub socket: PathBuf,
    #[arg(long, default_value_t = 80)]
    pub cols: u16,
    #[arg(long, default_value_t = 24)]
    pub rows: u16,
    #[arg(long, allow_hyphen_values = true)]
    pub cwd: Option<PathBuf>,
    #[arg(long, allow_hyphen_values = true)]
    pub program: Option<OsString>,
    /// Execute `program` with exactly `args`, even when the argument list is empty.
    #[arg(long)]
    pub literal_command: bool,
    /// Enable bounded host process metadata request/response frames.
    #[arg(long)]
    pub process_metadata: bool,
    #[arg(trailing_var_arg = true)]
    pub args: Vec<OsString>,
}

#[cfg(unix)]
// A 16 MiB Kitty clipboard response expands to about 22 MiB after base64.
const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[cfg(unix)]
const PTY_EXIT_DRAIN_QUIET: std::time::Duration = std::time::Duration::from_millis(25);

#[cfg(unix)]
const PTY_EXIT_DRAIN_LIMIT: std::time::Duration = std::time::Duration::from_millis(250);

#[cfg(unix)]
const MAX_PENDING_METADATA_REQUESTS: usize = 1;

#[cfg(unix)]
fn process_metadata_frame(
    sequence: u64,
    process_group_id: Option<u32>,
    processes: Vec<con_process::ProcessInfo>,
) -> Vec<u8> {
    // Metadata failure must not terminate the user's shell. Non-Unicode paths
    // and overlarge snapshots yield an unavailable observation instead.
    let payload = serde_json::to_vec(&(sequence, process_group_id, processes))
        .ok()
        .filter(|payload| payload.len() <= 256 * 1024)
        .unwrap_or_else(|| format!("[{sequence},null,[]]").into_bytes());
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(0x06);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    frame
}

#[cfg(unix)]
fn wait_for_pty_output(
    pty_fd: std::os::fd::RawFd,
    exit_fd: Option<std::os::fd::RawFd>,
    timeout: Option<std::time::Duration>,
) -> std::io::Result<(bool, bool)> {
    let mut fds = [
        libc::pollfd {
            fd: pty_fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: exit_fd.unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let timeout_ms = timeout.map_or(-1, |timeout| {
        timeout.as_millis().min(i32::MAX as u128) as i32
    });

    loop {
        // SAFETY: `fds` points to initialized values for the call, and both
        // descriptors remain owned by the bridge threads while polling.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
        if ready >= 0 {
            let pty_ready = fds[0].revents
                & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)
                != 0;
            let exit_ready = fds[1].fd >= 0
                && fds[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)
                    != 0;
            return Ok((pty_ready, exit_ready));
        }

        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

#[cfg(unix)]
fn configure_shell_startup(program: &OsStr, command: &mut portable_pty::CommandBuilder) {
    let Some(shell) = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return;
    };

    match shell {
        "fish" => {
            command.arg("--login");
            command.arg("--interactive");
        }
        "pwsh" => command.arg("-NoLogo"),
        "xonsh" => command.arg("-i"),
        "nu" => command.arg("--interactive"),
        "bash" | "zsh" | "sh" | "dash" | "ksh" | "mksh" => command.arg("-l"),
        _ => {}
    }
}

#[cfg(unix)]
pub fn run_pty_bridge(args: PtyBridgeArgs) -> Result<()> {
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, BorrowedFd};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let pty_system = native_pty_system();
    let pty_size = PtySize {
        rows: args.rows.max(1),
        cols: args.cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = pty_system
        .openpty(pty_size)
        .context("failed to open host pty")?;

    let program = args.program.unwrap_or_else(|| {
        std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/bash"))
    });

    let mut cmd = CommandBuilder::new(&program);
    if let Some(cwd) = &args.cwd {
        cmd.cwd(cwd);
    }
    if args.literal_command || !args.args.is_empty() {
        for arg in &args.args {
            cmd.arg(arg);
        }
    } else {
        configure_shell_startup(&program, &mut cmd);
    }
    cmd.env("TERM", "xterm-256color");

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .context("failed to spawn host child process on pty")?;

    drop(pair.slave);

    let stream = UnixStream::connect(&args.socket)
        .with_context(|| format!("failed to connect to socket {}", args.socket.display()))?;

    let pty_fd = pair
        .master
        .as_raw_fd()
        .context("host pty master did not expose a file descriptor")?;
    let mut pty_reader = pair.master.try_clone_reader().context("clone pty reader")?;
    let pty_writer = std::sync::Mutex::new(pair.master.take_writer().context("take pty writer")?);
    let master_mutex = Arc::new(std::sync::Mutex::new(pair.master));

    let running = Arc::new(AtomicBool::new(true));
    let (reader_exit_signal, reader_exit_wait) =
        UnixStream::pair().context("create pty reader exit signal")?;
    let socket_writer = Arc::new(std::sync::Mutex::new(
        stream.try_clone().context("clone socket writer")?,
    ));

    // Metadata collection can traverse the host process table, so keep it off
    // both terminal I/O loops. A one-slot queue bounds stale work; requests are
    // observations, and callers correlate (and may retry) by sequence number.
    let metadata_active = Arc::new(AtomicBool::new(args.process_metadata));
    let metadata_requests = args
        .process_metadata
        .then(|| {
            // The detached worker may outlive teardown. Own a descriptor rather
            // than borrowing a numeric fd that the OS could recycle meanwhile.
            let metadata_pty = unsafe { BorrowedFd::borrow_raw(pty_fd) }
                .try_clone_to_owned()
                .ok()?;
            let (sender, receiver) =
                std::sync::mpsc::sync_channel::<u64>(MAX_PENDING_METADATA_REQUESTS);
            let writer = socket_writer.clone();
            let active = metadata_active.clone();
            let worker = std::thread::Builder::new()
                .name("con-pty-metadata".into())
                .spawn(move || {
                    while let Ok(sequence) = receiver.recv() {
                        if !active.load(Ordering::Acquire) {
                            break;
                        }
                        // SAFETY: the worker owns this descriptor for the call.
                        let pgid = unsafe { libc::tcgetpgrp(metadata_pty.as_raw_fd()) };
                        let process_group_id = (pgid > 0).then_some(pgid as u32);
                        let processes = process_group_id
                            .map(|id| {
                                con_process::group_members_batch(&[id])
                                    .pop()
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default();
                        let frame = process_metadata_frame(sequence, process_group_id, processes);
                        let failed = writer.lock().map_or(true, |mut writer| {
                            if !active.load(Ordering::Acquire) {
                                return false;
                            }
                            writer.write_all(&frame).is_err() || writer.flush().is_err()
                        });
                        if failed {
                            break;
                        }
                    }
                });
            worker.ok().map(|_| sender)
        })
        .flatten();

    // Thread 1: Read raw output from host PTY master, send TAG_DATA frame to socket
    let running_r = running.clone();
    let data_writer = socket_writer.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut drain_deadline = None;
        loop {
            let draining = drain_deadline.is_some();
            let timeout = drain_deadline.map(|deadline: std::time::Instant| {
                PTY_EXIT_DRAIN_QUIET
                    .min(deadline.saturating_duration_since(std::time::Instant::now()))
            });
            if timeout == Some(std::time::Duration::ZERO) {
                break;
            }
            let (pty_ready, exit_ready) = match wait_for_pty_output(
                pty_fd,
                (!draining).then_some(reader_exit_wait.as_raw_fd()),
                timeout,
            ) {
                Ok(ready) => ready,
                Err(_) => break,
            };
            if exit_ready {
                drain_deadline = Some(std::time::Instant::now() + PTY_EXIT_DRAIN_LIMIT);
            }
            if !pty_ready {
                if drain_deadline.is_some() {
                    break;
                }
                continue;
            }

            match pty_reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut frame = Vec::with_capacity(5 + n);
                    frame.push(0x00);
                    frame.extend_from_slice(&(n as u32).to_be_bytes());
                    frame.extend_from_slice(&buf[..n]);
                    let failed = data_writer.lock().map_or(true, |mut writer| {
                        writer.write_all(&frame).is_err() || writer.flush().is_err()
                    });
                    if failed {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        running_r.store(false, Ordering::Relaxed);
    });

    // Thread 2: Read frames from socket, dispatch DATA to host PTY or RESIZE to master
    let running_w = running.clone();
    let master_for_resize = master_mutex.clone();
    let socket_reader_interrupt = stream
        .try_clone()
        .context("clone socket reader interrupt")?;
    let mut socket_reader = stream;
    let process_metadata = args.process_metadata;
    let socket_reader_thread = std::thread::spawn(move || {
        while running_w.load(Ordering::Relaxed) {
            let mut tag = [0u8; 1];
            if socket_reader.read_exact(&mut tag).is_err() {
                break;
            }
            match tag[0] {
                0x00 => {
                    let mut len_bytes = [0u8; 4];
                    if socket_reader.read_exact(&mut len_bytes).is_err() {
                        break;
                    }
                    let len = u32::from_be_bytes(len_bytes) as usize;
                    if len > MAX_FRAME_BYTES {
                        break;
                    }
                    let mut payload = vec![0u8; len];
                    if socket_reader.read_exact(&mut payload).is_err() {
                        break;
                    }
                    if let Ok(mut w) = pty_writer.lock() {
                        if w.write_all(&payload).is_err() || w.flush().is_err() {
                            break;
                        }
                    }
                }
                0x01 => {
                    let mut buf = [0u8; 8];
                    if socket_reader.read_exact(&mut buf).is_err() {
                        break;
                    }
                    let cols = u16::from_be_bytes([buf[0], buf[1]]);
                    let rows = u16::from_be_bytes([buf[2], buf[3]]);
                    let pixel_width = u16::from_be_bytes([buf[4], buf[5]]);
                    let pixel_height = u16::from_be_bytes([buf[6], buf[7]]);
                    if let Ok(m) = master_for_resize.lock() {
                        let _ = m.resize(PtySize {
                            cols: cols.max(1),
                            rows: rows.max(1),
                            pixel_width,
                            pixel_height,
                        });
                    }
                }
                0x05 if process_metadata => {
                    let mut sequence_bytes = [0u8; 8];
                    if socket_reader.read_exact(&mut sequence_bytes).is_err() {
                        break;
                    }
                    let sequence = u64::from_be_bytes(sequence_bytes);
                    // Never delay terminal input behind process-table work and
                    // never accumulate an unbounded backlog of stale queries.
                    if let Some(requests) = &metadata_requests {
                        let _ = requests.try_send(sequence);
                    }
                }
                _ => break,
            }
        }
        running_w.store(false, Ordering::Relaxed);
    });

    let status = child.wait();
    running.store(false, Ordering::Relaxed);
    metadata_active.store(false, Ordering::Release);

    // Stop waiting for more input from Con and wake the PTY reader. It drains
    // buffered output until the PTY is briefly quiet, with a hard limit for
    // descendants that inherited and kept the slave descriptor open.
    let _ = socket_reader_interrupt.shutdown(std::net::Shutdown::Read);
    let _ = reader_exit_signal.shutdown(std::net::Shutdown::Both);
    let _ = reader_thread.join();
    let _ = socket_reader_thread.join();

    let code = match status {
        Ok(status) => status.exit_code() as i32,
        Err(err) => {
            eprintln!("host pty child wait failed: {err}");
            -1
        }
    };
    let mut exit_frame = [0u8; 5];
    exit_frame[0] = 0x02; // TAG_EXIT
    exit_frame[1..5].copy_from_slice(&code.to_be_bytes());
    if let Ok(mut writer) = socket_writer.lock() {
        let _ = writer.write_all(&exit_frame);
        let _ = writer.flush();
        // This applies to every cloned descriptor. It gives the client EOF even
        // if a platform process query is stuck in the detached worker.
        let _ = writer.shutdown(std::net::Shutdown::Write);
    }

    Ok(())
}

#[cfg(not(unix))]
pub fn run_pty_bridge(_args: PtyBridgeArgs) -> Result<()> {
    anyhow::bail!("pty-bridge is only supported on Unix targets");
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn unavailable_metadata_preserves_sequence_and_valid_framing() {
        use std::os::unix::ffi::OsStringExt;
        for executable in [
            PathBuf::from("x".repeat(256 * 1024)),
            PathBuf::from(OsString::from_vec(vec![0xff])),
        ] {
            let frame = process_metadata_frame(
                7301,
                Some(17),
                vec![con_process::ProcessInfo {
                    identity: con_process::ProcessIdentity {
                        pid: 19,
                        started_at: 201,
                        executable,
                        name: "claude".into(),
                    },
                    parent_pid: 11,
                    process_group_id: Some(17),
                }],
            );
            assert_eq!(frame[0], 6);
            assert_eq!(
                u32::from_be_bytes(frame[1..5].try_into().unwrap()) as usize,
                frame.len() - 5
            );
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&frame[5..]).unwrap(),
                serde_json::json!([7301, null, []])
            );
        }
    }

    #[test]
    fn metadata_request_backlog_is_bounded_and_nonblocking() {
        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<u64>(MAX_PENDING_METADATA_REQUESTS);
        sender.try_send(1).unwrap();
        assert_eq!(sender.try_send(2).unwrap_err(), mpsc::TrySendError::Full(2));
        assert_eq!(receiver.recv().unwrap(), 1);
        sender.try_send(3).unwrap();
        assert_eq!(receiver.recv().unwrap(), 3);
    }

    fn run_finite_command(
        iteration: usize,
        script: &str,
        literal_command: bool,
        completion_timeout: Duration,
    ) -> (Vec<u8>, i32) {
        run_command(
            iteration,
            script,
            literal_command,
            completion_timeout,
            false,
        )
    }

    fn run_command(
        iteration: usize,
        script: &str,
        literal_command: bool,
        completion_timeout: Duration,
        process_metadata: bool,
    ) -> (Vec<u8>, i32) {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos();
        let socket = std::env::temp_dir().join(format!(
            "con-pty-bridge-{}-{unique}-{iteration}.sock",
            std::process::id(),
        ));
        let listener = UnixListener::bind(&socket).expect("bind bridge test socket");
        listener
            .set_nonblocking(true)
            .expect("set listener nonblocking");
        let args = PtyBridgeArgs {
            socket: socket.clone(),
            cols: 80,
            rows: 24,
            cwd: None,
            program: Some(OsString::from("/bin/sh")),
            literal_command,
            process_metadata,
            args: vec![OsString::from("-c"), OsString::from(script)],
        };
        let (done_tx, done_rx) = mpsc::channel();

        std::thread::spawn(move || {
            let _ = done_tx.send(run_pty_bridge(args));
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Ok(result) = done_rx.try_recv() {
                        result.expect("bridge should connect before returning");
                        panic!("bridge returned without connecting");
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "bridge should connect within timeout"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("accept bridge connection: {err}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        let mut frames = Vec::new();
        if process_metadata {
            // The child waits for stdin while output and metadata contend for
            // the shared socket. Fragment the request to exercise read_exact.
            stream.write_all(&[5]).unwrap();
            stream.write_all(&7301u64.to_be_bytes()).unwrap();
            stream.set_read_timeout(Some(completion_timeout)).unwrap();
            loop {
                let mut header = [0; 5];
                stream.read_exact(&mut header).unwrap();
                assert!(matches!(header[0], 0 | 6));
                let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
                assert!(len <= 256 * 1024);
                let mut payload = vec![0; len];
                stream.read_exact(&mut payload).unwrap();
                frames.extend_from_slice(&header);
                frames.extend_from_slice(&payload);
                if header[0] == 6 {
                    break;
                }
            }
            stream
                .write_all(&[0, 0, 0, 0, 3, b'g', b'o', b'\n'])
                .unwrap();
        }
        done_rx
            .recv_timeout(completion_timeout)
            .expect("bridge should return after child exit")
            .expect("bridge should exit cleanly");

        stream
            .read_to_end(&mut frames)
            .expect("read completed bridge stream");
        let mut frames = frames.as_slice();
        let mut output = Vec::new();
        let mut metadata_seen = false;
        let exit_code = loop {
            let (&tag, rest) = frames
                .split_first()
                .expect("bridge should send an exit frame");
            frames = rest;
            match tag {
                0x00 => {
                    let (len, rest) = frames.split_at(4);
                    let len = u32::from_be_bytes(len.try_into().expect("data frame length"));
                    let (payload, rest) = rest.split_at(len as usize);
                    output.extend_from_slice(payload);
                    frames = rest;
                }
                0x02 => {
                    let (code, _) = frames.split_at(4);
                    break i32::from_be_bytes(code.try_into().expect("exit status"));
                }
                0x06 => {
                    let (len, rest) = frames.split_at(4);
                    let len = u32::from_be_bytes(len.try_into().unwrap()) as usize;
                    let (payload, rest) = rest.split_at(len);
                    let (sequence, pgid, processes): (
                        u64,
                        Option<u32>,
                        Vec<con_process::ProcessInfo>,
                    ) = serde_json::from_slice(payload).unwrap();
                    assert_eq!(sequence, 7301);
                    assert!(pgid.is_some_and(|pid| pid > 0));
                    assert!(!processes.is_empty());
                    assert!(
                        processes
                            .iter()
                            .all(|process| process.process_group_id == pgid)
                    );
                    metadata_seen = true;
                    frames = rest;
                }
                tag => panic!("unexpected bridge frame tag {tag:#x}"),
            }
        };
        assert_eq!(metadata_seen, process_metadata);
        let _ = std::fs::remove_file(socket);
        (output, exit_code)
    }

    #[test]
    fn metadata_frames_coexist_with_terminal_output_and_exit() {
        for iteration in 0..8 {
            let (output, exit_code) = run_command(
                iteration,
                "printf before; read line; printf after; exit 7",
                true,
                Duration::from_secs(5),
                true,
            );
            assert_eq!(exit_code, 7);
            for marker in [b"before".as_slice(), b"after".as_slice()] {
                assert!(output.windows(marker.len()).any(|value| value == marker));
            }
        }
    }

    #[test]
    fn finite_command_drains_output_then_reports_exit_without_socket_input() {
        for iteration in 0..64 {
            let (output, exit_code) =
                run_finite_command(iteration, "printf con-marker", true, Duration::from_secs(5));
            assert_eq!(exit_code, 0, "iteration {iteration}");
            assert!(
                output
                    .windows(b"con-marker".len())
                    .any(|value| value == b"con-marker"),
                "iteration {iteration} lost buffered PTY output: {output:?}"
            );
        }
    }

    #[test]
    fn finite_command_does_not_wait_for_descendant_holding_slave_pty() {
        let (output, exit_code) = run_finite_command(
            65,
            "sleep 2 & printf con-marker",
            true,
            Duration::from_secs(1),
        );
        assert_eq!(exit_code, 0);
        assert!(
            output
                .windows(b"con-marker".len())
                .any(|value| value == b"con-marker"),
            "bridge lost buffered output: {output:?}"
        );
    }

    #[test]
    fn legacy_trailing_arguments_still_execute_without_literal_flag() {
        let (output, exit_code) =
            run_finite_command(66, "printf legacy-marker", false, Duration::from_secs(5));
        assert_eq!(exit_code, 0);
        assert!(
            output
                .windows(b"legacy-marker".len())
                .any(|value| value == b"legacy-marker"),
            "bridge ignored legacy trailing arguments: {output:?}"
        );
    }
}
