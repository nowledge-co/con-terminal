//! ConPTY wrapper.
//!
//! ConPTY (Windows 10 1809+) is the modern pseudo-console API. The
//! host:
//!
//! 1. Creates two anonymous pipe pairs via `CreatePipe`.
//! 2. Calls `CreatePseudoConsole(size, hInputRead, hOutputWrite, 0,
//!    &hpcon)`. The host keeps `hInputWrite` and `hOutputRead`, drops
//!    the child-side ends.
//! 3. Spawns the shell via `CreateProcessW` with a `STARTUPINFOEXW`
//!    whose attribute list contains
//!    `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = HPCON`.
//! 4. Reads/writes via the kept handles; resizes via
//!    `ResizePseudoConsole`; tears down via `ClosePseudoConsole`.
//!
//! Reference: https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session

use std::ffi::OsString;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use parking_lot::Mutex;
use windows::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, TRUE,
};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    STARTUPINFOEXW, STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::PWSTR;

use crate::pty_write::{PtyWriteQueue, PtyWriteWorker};
use crate::vt::PtyWriteClass;

fn perf_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("CON_GHOSTTY_PROFILE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

/// Close the pseudo-console if it hasn't been closed yet, clearing the
/// slot so a second caller becomes a no-op. Called from two places —
/// `ConPty::drop` (teardown triggered by the UI) and the exit-watcher
/// thread (teardown triggered by the child shell exiting on its own,
/// e.g. the user typing `exit`). Whichever runs first closes; the
/// other finds `None` and returns immediately.
fn close_hpcon_slot(slot: &Mutex<Option<HPCON>>) {
    if let Some(hpcon) = slot.lock().take() {
        // SAFETY: We own the HPCON; the `take` guarantees no one
        // else can close it. `ClosePseudoConsole` blocks until
        // conhost drains the output pipe, which is why the caller
        // force-kills the shell first when needed (see
        // `ConPty::drop`).
        unsafe { ClosePseudoConsole(hpcon) }
    }
}

/// Owned HANDLE that closes itself on drop.
///
/// We store the raw pointer as `usize` so the wrapper is unconditionally
/// `Send`/`Sync` — windows-rs 0.61's `HANDLE` is a tuple newtype around
/// `*mut c_void` whose auto-trait derivation propagates `!Send`. Rather
/// than rely on `unsafe impl Send for OwnedHandle` (which has flaky
/// inference inside generic closures), we keep the raw value in a
/// scalar and reconstruct the `HANDLE` inside `as_handle()`.
struct OwnedHandle(usize);

impl OwnedHandle {
    fn from_handle(h: HANDLE) -> Self {
        Self(h.0 as usize)
    }
    fn as_handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut _)
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let h = self.as_handle();
        if !h.is_invalid() {
            // SAFETY: handle ownership is unique (we never clone).
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

struct PendingPseudoConsole(Option<HPCON>);

impl PendingPseudoConsole {
    fn new(handle: HPCON) -> Self {
        Self(Some(handle))
    }

    fn handle(&self) -> HPCON {
        self.0.expect("pending pseudo-console handle was taken")
    }

    fn take(&mut self) -> HPCON {
        self.0
            .take()
            .expect("pending pseudo-console handle was already taken")
    }
}

impl Drop for PendingPseudoConsole {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            // SAFETY: this guard exclusively owns the HPCON until a
            // successfully-created ConPty takes it.
            unsafe { ClosePseudoConsole(handle) }
        }
    }
}

/// Cloneable write-only view of the host side of ConPTY's input pipe.
///
/// libghostty-vt callbacks receive this instead of the full `ConPty` so
/// terminal replies cannot re-enter `RenderSession` while the VT lock is
/// held. One bounded writer queue preserves ordering with user input without
/// allowing a blocked child pipe to stall parser output or rendering.
#[derive(Clone)]
pub(crate) struct ConPtyWriter {
    queue: PtyWriteQueue,
}

impl ConPtyWriter {
    pub(crate) fn write_all(&self, bytes: &[u8], class: PtyWriteClass) -> io::Result<()> {
        match class {
            PtyWriteClass::Regular => self.queue.enqueue(bytes),
            PtyWriteClass::ReservedControl => self.queue.enqueue_with_reserved_capacity(bytes),
        }
    }
}

fn write_conpty_input(handle: &OwnedHandle, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let mut written = 0u32;
        // SAFETY: the dedicated writer thread exclusively owns this handle.
        unsafe {
            WriteFile(handle.as_handle(), Some(bytes), Some(&mut written), None)
                .map_err(|e| io::Error::other(e.message()))?;
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "ConPTY input pipe accepted zero bytes",
            ));
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

/// Pipe endpoints created before the child process starts.
///
/// This two-phase construction lets the VT capture a `ConPtyWriter` before
/// the shell can emit its first query or output byte.
pub(crate) struct PreparedConPty {
    input_read: OwnedHandle,
    input_writer: ConPtyWriter,
    input_worker: PtyWriteWorker,
    output_read: OwnedHandle,
    output_write: OwnedHandle,
}

impl PreparedConPty {
    pub(crate) fn writer(&self) -> ConPtyWriter {
        self.input_writer.clone()
    }

    pub(crate) fn spawn<F>(
        self,
        command_line: &str,
        cwd: Option<&Path>,
        size: PtySize,
        on_output: F,
    ) -> Result<ConPty>
    where
        F: FnMut(&[u8]) + Send + 'static,
    {
        ConPty::spawn_prepared(self, command_line, cwd, size, on_output)
    }
}

/// A live ConPTY session.
pub struct ConPty {
    /// Pseudo-console handle, shared with the exit-watcher thread.
    /// Either `ConPty::drop` or the watcher takes the `Option<HPCON>`
    /// out and calls `ClosePseudoConsole`; the loser finds `None` and
    /// skips. See `close_hpcon_slot`.
    pcon: Arc<Mutex<Option<HPCON>>>,
    /// Host end of the pipe the child reads from.
    _input_writer: ConPtyWriter,
    /// Sole owner of the blocking pipe handle. Joined only after the child and
    /// pseudo-console have been terminated so a pending write cannot hang Drop.
    input_worker: PtyWriteWorker,
    /// Process handle (kept so callers can `WaitForSingleObject` for exit).
    process: OwnedHandle,
    /// Child thread handle.
    _thread: OwnedHandle,
    /// Output reader thread; joined on drop.
    output_thread: Option<JoinHandle<()>>,
    /// Background thread that waits on the child process handle and
    /// closes the pseudo-console when the shell exits on its own
    /// (e.g. user typed `exit`). Without this, conhost keeps the
    /// output pipe alive after the shell dies and the reader sits in
    /// `ReadFile` forever — the pane looks frozen and typing into it
    /// writes into a dead PTY. Joined on drop.
    exit_watcher: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

impl ConPty {
    pub(crate) fn prepare() -> Result<PreparedConPty> {
        let (input_read, input_write) = create_pipe().context("input pipe")?;
        let (output_read, output_write) = create_pipe().context("output pipe")?;
        let (input_queue, input_worker) = PtyWriteQueue::spawn("con-conpty-writer", move |bytes| {
            write_conpty_input(&input_write, bytes)
        })
        .context("spawn ConPTY writer")?;
        Ok(PreparedConPty {
            input_read,
            input_writer: ConPtyWriter { queue: input_queue },
            input_worker,
            output_read,
            output_write,
        })
    }

    /// Spawn a child shell, wire up ConPTY, and start a background reader
    /// that calls `on_output` for each chunk of bytes the shell writes.
    pub fn spawn<F>(
        command_line: &str,
        cwd: Option<&Path>,
        size: PtySize,
        on_output: F,
    ) -> Result<Self>
    where
        F: FnMut(&[u8]) + Send + 'static,
    {
        Self::prepare()?.spawn(command_line, cwd, size, on_output)
    }

    fn spawn_prepared<F>(
        prepared: PreparedConPty,
        command_line: &str,
        cwd: Option<&Path>,
        size: PtySize,
        on_output: F,
    ) -> Result<Self>
    where
        F: FnMut(&[u8]) + Send + 'static,
    {
        let PreparedConPty {
            input_read,
            input_writer,
            input_worker,
            output_read,
            output_write,
        } = prepared;

        let coord = COORD {
            X: size.cols as i16,
            Y: size.rows as i16,
        };

        // SAFETY: input_read and output_write are valid owned handles
        // captured by ConPTY. `0` flags = no special behavior.
        let hpcon: HPCON = unsafe {
            CreatePseudoConsole(coord, input_read.as_handle(), output_write.as_handle(), 0)
        }
        .context("CreatePseudoConsole failed")?;
        let mut pending_pcon = PendingPseudoConsole::new(hpcon);

        // Per Microsoft docs, after CreatePseudoConsole the captured
        // child-side ends should be closed by the host so only the
        // PCON owns them.
        drop(input_read);
        drop(output_write);

        let (startup_info, attribute_buffer) = build_startup_info(pending_pcon.handle())?;

        let mut command_line_w: Vec<u16> = OsString::from(command_line)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut process_info = PROCESS_INFORMATION::default();

        // Only two flags are safe with `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`:
        // CREATE_NO_WINDOW, DETACHED_PROCESS, and CREATE_NEW_CONSOLE are
        // all INCOMPATIBLE with ConPTY — they prevent the conhost.exe
        // helper (which the pty attribute launches internally) from
        // starting, so the child never writes anything to the pipe.
        // Seen by the user: powershell spawned successfully but zero
        // output bytes ever reached the reader thread. The visible
        // console flash on startup is conhost briefly initializing;
        // the hide-window story is a later polish item.
        let creation_flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT;

        // SAFETY: command_line_w is mutable + NUL-terminated;
        // `attribute_buffer` keeps the attribute list alive until after
        // CreateProcessW returns. The HPCON travels through the
        // PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE attribute, so the child
        // must NOT inherit the host process' unrelated stdout/stderr
        // handles. This matters when con itself is launched with
        // redirected logs (`*> con-profile.log`): inheriting those
        // handles lets PowerShell write its banner/prompt to the log
        // instead of through ConPTY, leaving the pane blank.
        let cwd_w: Option<Vec<u16>> = cwd.map(|path| {
            path.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        });
        let cwd_ptr = cwd_w
            .as_ref()
            .map(|wide| windows::core::PCWSTR(wide.as_ptr()))
            .unwrap_or(windows::core::PCWSTR::null());

        log::info!(
            "ConPTY CreateProcess: inherit_handles=false shell={command_line} cwd={:?}",
            cwd
        );
        let create_result = unsafe {
            CreateProcessW(
                None,
                Some(PWSTR(command_line_w.as_mut_ptr())),
                None,
                None,
                false,
                creation_flags,
                None,
                cwd_ptr,
                &startup_info.StartupInfo as *const _,
                &mut process_info,
            )
        };

        // Free the attribute list buffer regardless of outcome.
        unsafe {
            DeleteProcThreadAttributeList(startup_info.lpAttributeList);
        }
        // Keep the buffer alive until after DeleteProcThreadAttributeList.
        drop(attribute_buffer);

        create_result.context("CreateProcessW failed for ConPTY child")?;

        let process = OwnedHandle::from_handle(process_info.hProcess);
        let thread = OwnedHandle::from_handle(process_info.hThread);

        let pcon = Arc::new(Mutex::new(Some(pending_pcon.take())));
        let output_thread = spawn_output_reader(output_read, on_output);

        // Duplicate the process handle so the watcher thread can wait
        // on it independently of the `OwnedHandle` stored on `Self`.
        // The original is still needed for `process_handle()` and gets
        // closed by `OwnedHandle::drop` when `ConPty` is dropped; the
        // duplicate is closed by the watcher thread when it finishes.
        // SAFETY: GetCurrentProcess returns a pseudo-handle that does
        // not need closing; process_info.hProcess is a valid handle we
        // own; DUPLICATE_SAME_ACCESS copies the ACCESS mask verbatim.
        let mut dup_handle = HANDLE::default();
        let dup_ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                process_info.hProcess,
                GetCurrentProcess(),
                &mut dup_handle,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        };
        let exit_watcher = if dup_ok.is_ok() {
            let dup_owned = OwnedHandle::from_handle(dup_handle);
            let pcon_for_watcher = Arc::clone(&pcon);
            let watcher = thread::Builder::new()
                .name("conpty-exit-watcher".into())
                .spawn(move || {
                    let h = dup_owned.as_handle();
                    // SAFETY: dup_owned keeps the handle alive for
                    // the duration of this thread; INFINITE blocks
                    // until the child process terminates.
                    let wait_result = unsafe { WaitForSingleObject(h, INFINITE) };
                    log::info!(
                        "conpty exit-watcher: child exited (wait_result={:?}), closing pseudo-console",
                        wait_result
                    );
                    // Close the pseudo-console so conhost releases
                    // the pipe write-end; the output reader's
                    // `ReadFile` will then see EOF and the reader
                    // thread exits. If `ConPty::drop` raced us and
                    // already closed the HPCON, `close_hpcon_slot`
                    // finds `None` and returns immediately.
                    close_hpcon_slot(&pcon_for_watcher);
                    // dup_owned drops here, closing the duplicate.
                })
                .expect("conpty exit-watcher spawn failed");
            Some(watcher)
        } else {
            log::warn!(
                "DuplicateHandle for exit-watcher failed; pane will not auto-close on `exit`"
            );
            None
        };

        Ok(Self {
            pcon,
            _input_writer: input_writer,
            input_worker,
            process,
            _thread: thread,
            output_thread: Some(output_thread),
            exit_watcher,
        })
    }

    pub fn resize(&self, size: PtySize) -> Result<()> {
        let coord = COORD {
            X: size.cols as i16,
            Y: size.rows as i16,
        };
        // Read the HPCON through the shared slot. If the shell has
        // already exited (exit-watcher took the handle), the slot is
        // `None` and the resize is a no-op — there's nothing to resize.
        let guard = self.pcon.lock();
        let Some(hpcon) = *guard else {
            return Ok(());
        };
        // SAFETY: hpcon is a valid HPCON owned by this ConPty; the
        // lock guards against a concurrent close.
        unsafe { ResizePseudoConsole(hpcon, coord) }
            .map_err(|e| anyhow!("ResizePseudoConsole failed: {}", e.message()))
    }

    pub fn process_handle(&self) -> HANDLE {
        self.process.as_handle()
    }

    /// `true` while the pseudo-console is still open. Flips to `false`
    /// atomically when either `Drop` or the exit-watcher thread takes
    /// the HPCON out of the slot — i.e. when the child shell has
    /// exited and `ClosePseudoConsole` has been (or is being) called.
    pub fn is_alive(&self) -> bool {
        self.pcon.lock().is_some()
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        // Teardown order matters here. The reader thread reads the
        // host side of the output pipe; the pipe's write-end lives
        // inside conhost (spawned by CreatePseudoConsole), NOT inside
        // the child shell. So terminating the child is not enough —
        // conhost keeps the pipe open and `ReadFile` never returns
        // EOF. We must close the pseudo-console to make conhost exit
        // and release the write-end, THEN join the reader.
        //
        // But `ClosePseudoConsole` itself blocks waiting for the child
        // to drain its output — a hung shell blocks forever. So the
        // correct sequence is:
        //   1. TerminateProcess(child)      — makes (2) return promptly
        //   2. ClosePseudoConsole(hpcon)    — releases the pipe write-end
        //   3. JoinHandle::join(reader)     — reader's ReadFile now sees EOF
        //   4. JoinHandle::join(watcher)    — watcher's WaitForSingleObject
        //                                     returns now that the child died
        //
        // SAFETY: process handle is owned by us; TerminateProcess with
        // exit code 0 is the Windows equivalent of SIGKILL. We don't
        // care about the returned bool — even if the process already
        // exited naturally, this is a no-op.
        let process = self.process.as_handle();
        if !process.is_invalid() {
            unsafe {
                let _ = TerminateProcess(process, 0);
            }
        }

        // Close the pseudo-console (idempotent with the watcher's
        // close via `close_hpcon_slot`).
        close_hpcon_slot(&self.pcon);
        self.input_worker.shutdown();

        if let Some(handle) = self.output_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.exit_watcher.take() {
            let _ = handle.join();
        }
    }
}

fn create_pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: TRUE,
    };
    // SAFETY: `read` and `write` are out parameters; `security` lives on
    // this stack frame for the duration of the call.
    unsafe { CreatePipe(&mut read, &mut write, Some(&security), 0) }
        .context("CreatePipe failed")?;
    Ok((
        OwnedHandle::from_handle(read),
        OwnedHandle::from_handle(write),
    ))
}

/// Build a `STARTUPINFOEXW` whose attribute list binds the pseudo-console
/// handle. The returned buffer backs the attribute list and must outlive
/// the `STARTUPINFOEXW`.
fn build_startup_info(hpcon: HPCON) -> Result<(STARTUPINFOEXW, Vec<u8>)> {
    let mut required_size: usize = 0;
    // First call to determine required buffer size. Documented to fail
    // with ERROR_INSUFFICIENT_BUFFER and write the size; we ignore the
    // error and just use the size.
    // SAFETY: NULL list + dwAttributeCount=1 means "tell me how big".
    unsafe {
        let _ = InitializeProcThreadAttributeList(
            Some(LPPROC_THREAD_ATTRIBUTE_LIST(ptr::null_mut())),
            1,
            None,
            &mut required_size,
        );
    }

    let mut buffer = vec![0u8; required_size];
    let attribute_list = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr() as *mut _);

    // SAFETY: buffer sized correctly; attribute_list points into buffer.
    unsafe {
        InitializeProcThreadAttributeList(Some(attribute_list), 1, None, &mut required_size)
            .context("InitializeProcThreadAttributeList failed")?;
    }

    // SAFETY: for PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE the kernel
    // stores `lpValue` directly as the pseudo-console handle — NOT a
    // pointer to a value to copy. Match the canonical Microsoft
    // sample (github.com/microsoft/terminal ConPTY sample): pass the
    // HPCON (which is itself a pointer/void*) as lpValue.
    //
    // The previous `&hpcon` form had two bugs: (1) it pointed at a
    // local in `build_startup_info` that died when the function
    // returned, leaving a dangling stack pointer in the attribute
    // list for CreateProcessW to dereference; (2) even on a
    // longer-lived HPCON, this attribute expects the handle as
    // lpValue, not a pointer to it. Result seen by the user:
    // powershell spawned without PTY binding, opened its own console
    // window, wrote nothing to our pipe.
    unsafe {
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            Some(hpcon.0 as *const _),
            size_of::<HPCON>(),
            None,
            None,
        )
        .context("UpdateProcThreadAttribute failed")?;
    }

    let startup_info = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
            ..Default::default()
        },
        lpAttributeList: attribute_list,
    };

    Ok((startup_info, buffer))
}

fn spawn_output_reader<F>(read_handle: OwnedHandle, mut on_output: F) -> JoinHandle<()>
where
    F: FnMut(&[u8]) + Send + 'static,
{
    thread::Builder::new()
        .name("conpty-output-reader".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            let mut total_bytes: u64 = 0;
            let mut chunk_index: u64 = 0;
            let started = perf_trace_enabled().then(Instant::now);
            let mut last_chunk_at: Option<Instant> = None;
            loop {
                let mut bytes_read: u32 = 0;
                let handle = read_handle.as_handle();
                // SAFETY: handle is valid for the lifetime of this
                // thread (OwnedHandle moved in via `read_handle`).
                let result = unsafe {
                    ReadFile(handle, Some(&mut buf), Some(&mut bytes_read), None)
                };
                if result.is_err() || bytes_read == 0 {
                    log::info!(
                        "conpty reader: EOF after {total_bytes} bytes, err={:?}",
                        result.err()
                    );
                    break; // EOF or error; child exited.
                }
                total_bytes += bytes_read as u64;
                if let Some(started) = started {
                    let now = Instant::now();
                    let since_prev_ms = last_chunk_at
                        .map(|last| now.duration_since(last).as_secs_f64() * 1000.0)
                        .unwrap_or(0.0);
                    let since_start_ms = now.duration_since(started).as_secs_f64() * 1000.0;
                    log::info!(
                        target: "con::perf",
                        "conpty_read chunk={} bytes={} total_bytes={} since_prev_ms={:.3} since_start_ms={:.3}",
                        chunk_index,
                        bytes_read,
                        total_bytes,
                        since_prev_ms,
                        since_start_ms,
                    );
                    last_chunk_at = Some(now);
                }
                chunk_index = chunk_index.wrapping_add(1);
                log::trace!(
                    "conpty reader: +{bytes_read} bytes (total {total_bytes})"
                );
                on_output(&buf[..bytes_read as usize]);
            }
            // OwnedHandle::Drop closes the handle.
        })
        .expect("conpty reader thread spawn failed")
}

/// Discover a sensible default shell.
///
/// Users who want to force a different shell can point us at it via
/// `CON_SHELL` (future: a config-file field). Otherwise we try to honor
/// Windows Terminal's configured default profile before falling back to a
/// simple executable search. Matching the user's Windows Terminal shell is
/// important when comparing prompt latency: PowerShell profile scripts,
/// prompt frameworks, and WSL startup can dominate the first 400-500ms.
pub fn default_shell_command() -> String {
    let command = if let Some(cmd) = std::env::var("CON_SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        cmd
    } else if let Some(cmd) = windows_terminal_default_shell_command() {
        cmd
    } else if let Some(candidate) = ["pwsh.exe", "powershell.exe"]
        .into_iter()
        .find(|candidate| path_lookup(candidate).is_some())
    {
        candidate.to_string()
    } else if let Some(cmd) = std::env::var("COMSPEC")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        cmd
    } else {
        "cmd.exe".to_string()
    };

    with_shell_integration(command)
}

fn with_shell_integration(command: String) -> String {
    with_powershell_cwd_integration(&command).unwrap_or(command)
}

fn with_powershell_cwd_integration(command: &str) -> Option<String> {
    let script_path = powershell_cwd_integration_script_path()?;
    with_powershell_cwd_integration_script(command, &script_path)
}

fn with_powershell_cwd_integration_script(command: &str, script_path: &Path) -> Option<String> {
    let (executable, rest) = split_command_head(command.trim())?;
    let basename = command_basename(executable);
    if !matches_ignore_ascii_case_any(
        basename,
        &["pwsh", "pwsh.exe", "powershell", "powershell.exe"],
    ) {
        return None;
    }

    let args = split_command_args(rest);
    if args.iter().any(|arg| {
        matches_ignore_ascii_case_any(
            arg,
            &[
                "-command",
                "/command",
                "-c",
                "/c",
                "-encodedcommand",
                "/encodedcommand",
                "-ec",
                "/ec",
                "-file",
                "/file",
                "-f",
                "/f",
            ],
        )
    }) {
        return None;
    }

    let mut integrated = command.trim().to_string();
    if !args
        .iter()
        .any(|arg| matches_ignore_ascii_case_any(arg, &["-noexit", "/noexit"]))
    {
        integrated.push_str(" -NoExit");
    }
    integrated.push_str(" -ExecutionPolicy Bypass -File ");
    integrated.push_str(&quote_path_for_command(script_path));
    Some(integrated)
}

fn powershell_cwd_integration_script_path() -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("con-terminal");
    if let Err(err) = fs::create_dir_all(&dir) {
        log::warn!(
            "could not create PowerShell cwd integration directory {}: {err}",
            dir.display()
        );
        return None;
    }

    let path = dir.join("con-powershell-cwd-integration.ps1");
    if let Err(err) = fs::write(&path, POWERSHELL_CWD_INTEGRATION_SCRIPT) {
        log::warn!(
            "could not write PowerShell cwd integration script {}: {err}",
            path.display()
        );
        return None;
    }
    Some(path)
}

const POWERSHELL_CWD_INTEGRATION_SCRIPT: &str = r#"
function global:__ConTerminalEmitOsc7 {
    try {
        $providerPath = (Get-Location).ProviderPath
        if ([string]::IsNullOrWhiteSpace($providerPath)) {
            return
        }
        $uri = ([System.Uri]::new($providerPath)).AbsoluteUri
        [Console]::Write("$([char]0x1b)]7;$uri$([char]0x07)")
    } catch {
    }
}

if (-not $global:__ConTerminalPromptHookInstalled) {
    $global:__ConTerminalPromptHookInstalled = $true
    $global:__ConTerminalOriginalPrompt = (Get-Command prompt -CommandType Function -ErrorAction SilentlyContinue).ScriptBlock
    function global:prompt {
        global:__ConTerminalEmitOsc7
        if ($global:__ConTerminalOriginalPrompt) {
            & $global:__ConTerminalOriginalPrompt
        } else {
            "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
        }
    }
}

global:__ConTerminalEmitOsc7
"#;

fn split_command_head(command: &str) -> Option<(&str, &str)> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }

    if let Some(stripped) = command.strip_prefix('"') {
        let end = stripped.find('"')?;
        let executable = &stripped[..end];
        let rest = stripped[end + 1..].trim_start();
        Some((executable, rest))
    } else {
        let split = command.find(char::is_whitespace).unwrap_or(command.len());
        let executable = &command[..split];
        let rest = command[split..].trim_start();
        Some((executable, rest))
    }
}

fn command_basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn split_command_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in input.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ch if ch.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

fn matches_ignore_ascii_case_any(input: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| input.eq_ignore_ascii_case(candidate))
}

fn windows_terminal_default_shell_command() -> Option<String> {
    for settings_path in windows_terminal_settings_paths() {
        let Ok(settings) = fs::read_to_string(&settings_path) else {
            continue;
        };
        match windows_terminal_profile_command_from_settings(&settings) {
            Some(command) => {
                log::info!(
                    "using Windows Terminal default profile command from {}: {}",
                    settings_path.display(),
                    command
                );
                return Some(command);
            }
            None => {
                log::debug!(
                    "Windows Terminal settings did not resolve to a supported shell: {}",
                    settings_path.display()
                );
            }
        }
    }
    None
}

fn windows_terminal_settings_paths() -> Vec<PathBuf> {
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return Vec::new();
    };

    vec![
        local_app_data
            .join("Packages")
            .join("Microsoft.WindowsTerminal_8wekyb3d8bbwe")
            .join("LocalState")
            .join("settings.json"),
        local_app_data
            .join("Packages")
            .join("Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe")
            .join("LocalState")
            .join("settings.json"),
        local_app_data
            .join("Microsoft")
            .join("Windows Terminal")
            .join("settings.json"),
        local_app_data
            .join("Microsoft")
            .join("Windows Terminal Preview")
            .join("settings.json"),
    ]
}

fn windows_terminal_profile_command_from_settings(settings: &str) -> Option<String> {
    let json = strip_jsonc_for_settings(settings);
    let root = serde_json::from_str::<serde_json::Value>(&json).ok()?;
    let default_profile = root.get("defaultProfile")?.as_str()?.trim();
    if default_profile.is_empty() {
        return None;
    }

    let profiles = root.get("profiles")?.get("list")?.as_array()?;
    let profile = profiles.iter().find(|profile| {
        ["guid", "name"].iter().any(|key| {
            profile_string(profile, key)
                .is_some_and(|value| value.eq_ignore_ascii_case(default_profile))
        })
    })?;

    windows_terminal_profile_command(profile)
}

fn windows_terminal_profile_command(profile: &serde_json::Value) -> Option<String> {
    if let Some(command) = profile_string(profile, "commandline").and_then(clean_commandline) {
        return Some(command);
    }

    match profile_string(profile, "source")?.as_str() {
        "Windows.Terminal.PowershellCore" => powershell_core_command(),
        "Windows.Terminal.WindowsPowerShell" => Some("powershell.exe".to_string()),
        "Windows.Terminal.Cmd" => Some(
            std::env::var("COMSPEC")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "cmd.exe".to_string()),
        ),
        // Windows Terminal dynamic WSL profiles don't always persist the
        // distro command line, and `name` is user-editable display text,
        // not a stable distro id. Prefer a valid default WSL launch over
        // constructing `wsl.exe -d <display-name>` and failing startup for
        // renamed profiles.
        "Windows.Terminal.Wsl" => Some("wsl.exe".to_string()),
        _ => None,
    }
}

fn profile_string(profile: &serde_json::Value, key: &str) -> Option<String> {
    profile.get(key)?.as_str().map(str::to_string)
}

fn clean_commandline(commandline: String) -> Option<String> {
    let expanded = expand_percent_env_vars(commandline.trim());
    let command = expanded.trim();
    if command.is_empty() {
        None
    } else {
        Some(command.to_string())
    }
}

fn powershell_core_command() -> Option<String> {
    if path_lookup("pwsh.exe").is_some() {
        return Some("pwsh.exe".to_string());
    }

    for path in powershell_core_candidate_paths() {
        if path.is_file() {
            return Some(quote_path_for_command(&path));
        }
    }
    None
}

fn powershell_core_candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        paths.push(PathBuf::from(program_files).join("PowerShell\\7\\pwsh.exe"));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        paths.push(PathBuf::from(program_files_x86).join("PowerShell\\7\\pwsh.exe"));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local_app_data)
                .join("Microsoft")
                .join("WindowsApps")
                .join("pwsh.exe"),
        );
    }
    paths
}

fn path_lookup(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for entry in std::env::split_paths(&path) {
        let candidate = entry.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn quote_path_for_command(path: &Path) -> String {
    quote_command_arg(&path.display().to_string())
}

fn quote_command_arg(arg: &str) -> String {
    if arg.chars().any(|ch| matches!(ch, ' ' | '\t' | '"')) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

fn expand_percent_env_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];

        let Some(end) = rest.find('%') else {
            out.push('%');
            out.push_str(rest);
            return out;
        };

        let name = &rest[..end];
        if name.is_empty() {
            out.push_str("%%");
        } else if let Ok(value) = std::env::var(name) {
            out.push_str(&value);
        } else {
            out.push('%');
            out.push_str(name);
            out.push('%');
        }
        rest = &rest[end + 1..];
    }

    out.push_str(rest);
    out
}

fn strip_jsonc_for_settings(input: &str) -> String {
    let without_comments = strip_jsonc_comments(input);
    strip_jsonc_trailing_commas(&without_comments)
}

fn strip_jsonc_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                for comment_ch in chars.by_ref() {
                    if comment_ch == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for comment_ch in chars.by_ref() {
                    if comment_ch == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && comment_ch == '/' {
                        break;
                    }
                    prev = comment_ch;
                }
            }
            _ => out.push(ch),
        }
    }

    out
}

fn strip_jsonc_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < chars.len() {
        let ch = chars[i];
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            ',' => {
                let mut next = i + 1;
                while next < chars.len() && chars[next].is_whitespace() {
                    next += 1;
                }
                if next < chars.len() && matches!(chars[next], '}' | ']') {
                    i += 1;
                    continue;
                }
                out.push(ch);
            }
            _ => out.push(ch),
        }
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_windows_terminal_default_profile_commandline() {
        let settings = r#"
        {
          // settings.json is JSONC
          "defaultProfile": "{pwsh}",
          "profiles": {
            "list": [
              { "guid": "{cmd}", "commandline": "cmd.exe", },
              { "guid": "{pwsh}", "commandline": "pwsh.exe -NoLogo", },
            ],
          },
        }
        "#;

        assert_eq!(
            windows_terminal_profile_command_from_settings(settings).as_deref(),
            Some("pwsh.exe -NoLogo")
        );
    }

    #[test]
    fn maps_windows_terminal_dynamic_profiles() {
        let settings = r#"
        {
          "defaultProfile": "Windows PowerShell",
          "profiles": {
            "list": [
              {
                "name": "Windows PowerShell",
                "source": "Windows.Terminal.WindowsPowerShell"
              }
            ]
          }
        }
        "#;

        assert_eq!(
            windows_terminal_profile_command_from_settings(settings).as_deref(),
            Some("powershell.exe")
        );
    }

    #[test]
    fn wraps_powershell_shells_with_cwd_integration() {
        let script = Path::new(r"C:\Users\me\AppData\Local\Temp\Con Terminal\con-cwd.ps1");

        let wrapped = with_powershell_cwd_integration_script("pwsh.exe -NoLogo", script)
            .expect("pwsh should be integrated");

        assert!(wrapped.starts_with("pwsh.exe -NoLogo -NoExit "));
        assert!(wrapped.contains("-ExecutionPolicy Bypass -File "));
        assert!(wrapped.contains(r#""C:\Users\me\AppData\Local\Temp\Con Terminal\con-cwd.ps1""#));
    }

    #[test]
    fn preserves_existing_noexit_when_wrapping_powershell() {
        let script = Path::new(r"C:\Temp\con-cwd.ps1");

        let wrapped = with_powershell_cwd_integration_script(
            r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo -NoExit"#,
            script,
        )
        .expect("quoted pwsh path should be integrated");

        assert_eq!(wrapped.matches("-NoExit").count(), 1);
        assert!(wrapped.contains("-ExecutionPolicy Bypass -File "));
    }

    #[test]
    fn leaves_command_mode_powershell_commands_unchanged() {
        let script = Path::new(r"C:\Temp\con-cwd.ps1");

        assert!(
            with_powershell_cwd_integration_script("pwsh.exe -Command Get-Location", script)
                .is_none()
        );
        assert!(
            with_powershell_cwd_integration_script("powershell.exe -File init.ps1", script)
                .is_none()
        );
    }

    #[test]
    fn does_not_wrap_non_powershell_shells() {
        let script = Path::new(r"C:\Temp\con-cwd.ps1");

        assert!(with_powershell_cwd_integration_script("cmd.exe", script).is_none());
        assert!(with_powershell_cwd_integration_script("wsl.exe", script).is_none());
    }
}
