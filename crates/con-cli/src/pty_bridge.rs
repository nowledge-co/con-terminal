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
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    #[arg(long)]
    pub program: Option<String>,
    /// Execute `program` with exactly `args`, even when the argument list is empty.
    #[arg(long)]
    pub literal_command: bool,
    #[arg(trailing_var_arg = true)]
    pub args: Vec<String>,
}

#[cfg(unix)]
// A 16 MiB Kitty clipboard response expands to about 22 MiB after base64.
const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[cfg(unix)]
fn configure_shell_startup(program: &str, command: &mut portable_pty::CommandBuilder) {
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

    let program = args
        .program
        .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()));

    let mut cmd = CommandBuilder::new(&program);
    if let Some(cwd) = &args.cwd {
        cmd.cwd(cwd);
    }
    if args.literal_command {
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

    let mut pty_reader = pair.master.try_clone_reader().context("clone pty reader")?;
    let pty_writer = std::sync::Mutex::new(pair.master.take_writer().context("take pty writer")?);
    let master_mutex = Arc::new(std::sync::Mutex::new(pair.master));

    let running = Arc::new(AtomicBool::new(true));
    let mut socket_writer = stream.try_clone().context("clone socket writer")?;
    let mut exit_writer = stream.try_clone().context("clone exit writer")?;

    // Thread 1: Read raw output from host PTY master, send TAG_DATA frame to socket
    let running_r = running.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while running_r.load(Ordering::Relaxed) {
            match pty_reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let len_bytes = (n as u32).to_be_bytes();
                    if socket_writer.write_all(&[0x00]).is_err()
                        || socket_writer.write_all(&len_bytes).is_err()
                        || socket_writer.write_all(&buf[..n]).is_err()
                        || socket_writer.flush().is_err()
                    {
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
    let mut socket_reader = stream;
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
                _ => break,
            }
        }
        running_w.store(false, Ordering::Relaxed);
    });

    let status = child.wait();
    running.store(false, Ordering::Relaxed);
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
    let _ = exit_writer.write_all(&exit_frame);
    let _ = exit_writer.flush();

    Ok(())
}

#[cfg(not(unix))]
pub fn run_pty_bridge(_args: PtyBridgeArgs) -> Result<()> {
    anyhow::bail!("pty-bridge is only supported on Unix targets");
}
