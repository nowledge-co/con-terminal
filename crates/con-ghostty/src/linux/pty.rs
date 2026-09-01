use std::ffi::{OsStr, OsString};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::pty_write::{PtyWriteQueue, PtyWriteWorker};
use crate::stub::{CommandFinishedSignal, SurfaceSize, TerminalColors};
use crate::transcript::{TranscriptBuffer, snapshot_to_lines};
use crate::vt::{
    PtyWriteClass, ScreenSnapshot, SelectionAutoscroll, SelectionAutoscrollUpdate,
    SelectionGeometry, SelectionPoint, ThemeColors, VtKeyEvent, VtKeyOutcome, VtPasteResult,
    VtPasteSource, VtScreen,
};
use crate::{ClipboardWritePolicy, DesktopNotificationPolicy};

const DEFAULT_COLUMNS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
// A 16 MiB Kitty clipboard response expands to about 22 MiB after base64.
const MAX_BRIDGE_FRAME_BYTES: usize = 32 * 1024 * 1024;
const BRIDGE_READY: u8 = 0x03;
const BRIDGE_STARTUP_ERROR: u8 = 0x04;
const MAX_BRIDGE_STARTUP_ERROR_BYTES: usize = 64 * 1024;

pub type LinuxWakeCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Debug)]
pub struct LinuxPtySpawnError {
    source: anyhow::Error,
    retryable: bool,
}

impl LinuxPtySpawnError {
    fn permanent(source: anyhow::Error) -> Self {
        Self {
            source,
            retryable: false,
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }
}

impl std::fmt::Display for LinuxPtySpawnError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.source)
    }
}

impl std::error::Error for LinuxPtySpawnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

impl From<anyhow::Error> for LinuxPtySpawnError {
    fn from(source: anyhow::Error) -> Self {
        Self {
            source,
            retryable: true,
        }
    }
}

impl From<io::Error> for LinuxPtySpawnError {
    fn from(source: io::Error) -> Self {
        anyhow::Error::new(source).into()
    }
}

#[derive(Clone)]
pub struct LinuxPtyOptions {
    pub cwd: Option<PathBuf>,
    pub program: Option<String>,
    pub command_program: Option<OsString>,
    /// `None` starts the configured program as an interactive login shell.
    /// `Some`, including an empty vector, executes an explicit command exactly.
    pub command_args: Option<Vec<OsString>>,
    pub size: SurfaceSize,
    pub initial_output: Option<Vec<u8>>,
    pub wake_generation: Option<Arc<AtomicU64>>,
    pub wake_callback: Option<LinuxWakeCallback>,
    pub theme: Option<TerminalColors>,
    pub clipboard_write: bool,
    pub(crate) clipboard_write_policy: Arc<ClipboardWritePolicy>,
    pub(crate) desktop_notification_policy: Arc<DesktopNotificationPolicy>,
}

impl Default for LinuxPtyOptions {
    fn default() -> Self {
        Self {
            cwd: None,
            program: None,
            command_program: None,
            command_args: None,
            size: SurfaceSize {
                columns: DEFAULT_COLUMNS,
                rows: DEFAULT_ROWS,
                width_px: 0,
                height_px: 0,
                cell_width_px: 0,
                cell_height_px: 0,
            },
            initial_output: None,
            wake_generation: None,
            wake_callback: None,
            theme: None,
            clipboard_write: false,
            clipboard_write_policy: Arc::new(ClipboardWritePolicy::new(true)),
            desktop_notification_policy: Arc::new(DesktopNotificationPolicy::default()),
        }
    }
}

fn theme_colors_to_vt(colors: &TerminalColors) -> ThemeColors {
    ThemeColors::from_ansi16(colors.foreground, colors.background, colors.palette)
}

enum LinuxPtyBackend {
    Local {
        master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
        child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
        io_shutdown: UnixStream,
    },
    HostBridge {
        stream_shutdown: UnixStream,
        socket_path: PathBuf,
        child: Mutex<std::process::Child>,
    },
}

#[derive(Clone)]
enum LinuxPtyInput {
    Local(PtyWriteQueue),
    HostBridge(PtyWriteQueue),
}

impl LinuxPtyInput {
    fn write_data(&self, data: &[u8], class: PtyWriteClass) -> std::io::Result<()> {
        match self {
            Self::Local(queue) => match class {
                PtyWriteClass::Regular => queue.enqueue(data),
                PtyWriteClass::ReservedControl => queue.enqueue_with_reserved_capacity(data),
            },
            Self::HostBridge(queue) => {
                if data.len() > MAX_BRIDGE_FRAME_BYTES {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidInput,
                        "host PTY input frame is too large",
                    ));
                }

                let mut frame = Vec::with_capacity(5 + data.len());
                frame.push(0x00); // TAG_DATA
                frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
                frame.extend_from_slice(data);
                match class {
                    PtyWriteClass::Regular => queue.enqueue_owned(frame.into_boxed_slice()),
                    PtyWriteClass::ReservedControl => {
                        queue.enqueue_owned_with_reserved_capacity(frame.into_boxed_slice())
                    }
                }
            }
        }
    }

    fn resize(&self, size: &SurfaceSize) -> std::io::Result<()> {
        let Self::HostBridge(queue) = self else {
            return Ok(());
        };

        let cols = size.columns.max(1);
        let rows = size.rows.max(1);
        let width_px = size.width_px.min(u32::from(u16::MAX)) as u16;
        let height_px = size.height_px.min(u32::from(u16::MAX)) as u16;
        let mut frame = [0u8; 9];
        frame[0] = 0x01; // TAG_RESIZE
        frame[1..3].copy_from_slice(&cols.to_be_bytes());
        frame[3..5].copy_from_slice(&rows.to_be_bytes());
        frame[5..7].copy_from_slice(&width_px.to_be_bytes());
        frame[7..9].copy_from_slice(&height_px.to_be_bytes());
        queue.enqueue_with_reserved_capacity(&frame)
    }
}

fn duplicate_fd(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: `fd` remains owned by portable_pty for this call. The returned
    // descriptor has independent ownership and is closed by `OwnedFd`.
    unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()
}

fn set_fd_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Wait for one non-blocking PTY operation or an explicit session shutdown.
/// The cancellation socket is never consumed, so both reader and writer wake
/// when the owner shuts down its peer endpoint.
fn wait_for_pty_io(pty_fd: RawFd, cancel_fd: RawFd, events: i16) -> io::Result<bool> {
    let mut fds = [
        libc::pollfd {
            fd: cancel_fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: pty_fd,
            events,
            revents: 0,
        },
    ];
    loop {
        let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if result == -1 {
            let err = io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }

        if fds[0].revents != 0 {
            return Ok(false);
        }
        if fds[1].revents & (events | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Ok(true);
        }
    }
}

fn write_all_cancellable(
    writer: &mut dyn Write,
    readiness: &OwnedFd,
    cancel: &UnixStream,
    mut data: &[u8],
) -> io::Result<()> {
    while !data.is_empty() {
        if !wait_for_pty_io(readiness.as_raw_fd(), cancel.as_raw_fd(), libc::POLLOUT)? {
            return Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "PTY input writer was cancelled",
            ));
        }

        match writer.write(data) {
            Ok(0) => {
                return Err(io::Error::new(
                    ErrorKind::WriteZero,
                    "PTY input accepted zero bytes",
                ));
            }
            Ok(written) => data = &data[written..],
            Err(err) if matches!(err.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

pub struct LinuxPtySession {
    backend: LinuxPtyBackend,
    input: LinuxPtyInput,
    input_worker: PtyWriteWorker,
    shared: Arc<SessionShared>,
    size: Mutex<SurfaceSize>,
    title: Option<String>,
    current_dir: Option<String>,
    input_generation: AtomicU64,
    started_at: Instant,
}

struct SessionShared {
    screen: Arc<VtScreen>,
    transcript: Mutex<TranscriptBuffer>,
    alive: AtomicBool,
    needs_render: AtomicBool,
    wake_generation: Option<Arc<AtomicU64>>,
    wake_callback: Option<LinuxWakeCallback>,
    finished_signal: Mutex<Option<CommandFinishedSignal>>,
    last_exit_code: Mutex<Option<i32>>,
    last_duration: Mutex<Option<Duration>>,
}

impl SessionShared {
    fn new(
        screen: Arc<VtScreen>,
        wake_generation: Option<Arc<AtomicU64>>,
        wake_callback: Option<LinuxWakeCallback>,
    ) -> Self {
        Self {
            screen,
            transcript: Mutex::new(TranscriptBuffer::default()),
            alive: AtomicBool::new(true),
            needs_render: AtomicBool::new(false),
            wake_generation,
            wake_callback,
            finished_signal: Mutex::new(None),
            last_exit_code: Mutex::new(None),
            last_duration: Mutex::new(None),
        }
    }

    fn wake(&self) {
        if let Some(wake_generation) = self.wake_generation.as_ref() {
            wake_generation.fetch_add(1, Ordering::AcqRel);
        }
        if let Some(wake_callback) = self.wake_callback.as_ref() {
            wake_callback();
        }
    }

    fn push_output(&self, chunk: &[u8]) {
        let text = String::from_utf8_lossy(chunk);
        self.transcript.lock().push(text.as_ref());
        self.screen.feed(chunk);
        self.needs_render.store(true, Ordering::Release);
        self.wake();
    }

    fn mark_exited(&self, exit_code: Option<i32>, duration: Duration) {
        if !self.alive.swap(false, Ordering::AcqRel) {
            if exit_code.is_some() {
                let mut backfilled = false;
                let mut last_exit_code = self.last_exit_code.lock();
                if last_exit_code.is_none() {
                    *last_exit_code = exit_code;
                    *self.last_duration.lock() = Some(duration);
                    backfilled = true;
                }

                let mut finished_signal = self.finished_signal.lock();
                if finished_signal
                    .as_ref()
                    .map_or(true, |signal| signal.exit_code.is_none())
                {
                    *finished_signal = Some(CommandFinishedSignal {
                        exit_code,
                        duration,
                    });
                    backfilled = true;
                }

                if backfilled {
                    self.needs_render.store(true, Ordering::Release);
                    self.wake();
                }
            }
            return;
        }
        *self.last_exit_code.lock() = exit_code;
        *self.last_duration.lock() = Some(duration);
        *self.finished_signal.lock() = Some(CommandFinishedSignal {
            exit_code,
            duration,
        });
        self.needs_render.store(true, Ordering::Release);
        self.wake();
    }
}

impl LinuxPtySession {
    pub fn spawn(options: LinuxPtyOptions) -> std::result::Result<Self, LinuxPtySpawnError> {
        if con_paths::is_flatpak() {
            spawn_host_bridge(options)
        } else {
            spawn_local(options)
        }
    }

    pub fn size(&self) -> SurfaceSize {
        *self.size.lock()
    }

    pub fn set_pixel_size(&self, width_px: u32, height_px: u32) -> Result<()> {
        let mut size = self.size.lock();
        size.width_px = width_px;
        size.height_px = height_px;
        self.shared
            .screen
            .resize(
                size.columns.max(1),
                size.rows.max(1),
                size.cell_width_px.max(1),
                size.cell_height_px.max(1),
            )
            .context("failed to resize linux vt screen")?;

        match &self.backend {
            LinuxPtyBackend::Local { master, .. } => {
                master
                    .lock()
                    .resize(pty_size_from_surface(&size))
                    .context("failed to resize linux pty")?;
            }
            LinuxPtyBackend::HostBridge { .. } => {}
        }
        self.input
            .resize(&size)
            .context("failed to queue linux pty resize")?;

        self.mark_needs_render();
        Ok(())
    }

    pub fn resize(&self, size: SurfaceSize) -> Result<()> {
        *self.size.lock() = size;
        self.shared
            .screen
            .resize(
                size.columns.max(1),
                size.rows.max(1),
                size.cell_width_px.max(1),
                size.cell_height_px.max(1),
            )
            .context("failed to resize linux vt screen")?;

        match &self.backend {
            LinuxPtyBackend::Local { master, .. } => {
                master
                    .lock()
                    .resize(pty_size_from_surface(&size))
                    .context("failed to resize linux pty")?;
            }
            LinuxPtyBackend::HostBridge { .. } => {}
        }
        self.input
            .resize(&size)
            .context("failed to queue linux pty resize")?;

        self.mark_needs_render();
        Ok(())
    }

    /// Stamp the shared `needs_render` flag and wake the workspace
    /// loop so the next pump tick re-fetches a fresh snapshot.
    fn mark_needs_render(&self) {
        self.shared.needs_render.store(true, Ordering::Release);
        self.shared.wake();
    }

    pub fn write_input(&self, data: &[u8]) -> Result<()> {
        self.scroll_viewport_to_bottom();
        self.shared
            .screen
            .write_input(data)
            .context("failed to queue linux pty input")?;
        self.input_generation.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn write_control(&self, data: &[u8]) -> Result<()> {
        self.scroll_viewport_to_bottom();
        self.shared
            .screen
            .write_control(data)
            .context("failed to queue linux pty control input")?;
        self.input_generation.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn send_key(&self, event: &VtKeyEvent<'_>) -> Result<VtKeyOutcome> {
        let outcome = self.shared.screen.send_key(event)?;
        if outcome.output_accepted {
            self.scroll_viewport_to_bottom();
            self.input_generation.fetch_add(1, Ordering::Relaxed);
        }
        Ok(outcome)
    }

    pub fn paste_text(
        &self,
        text: &str,
        source: VtPasteSource,
        confirm_unsafe_paste: bool,
    ) -> Result<VtPasteResult> {
        let result = self
            .shared
            .screen
            .paste_text(text, source, confirm_unsafe_paste)?;
        if result == VtPasteResult::Accepted {
            self.scroll_viewport_to_bottom();
            self.input_generation.fetch_add(1, Ordering::Relaxed);
        }
        Ok(result)
    }

    pub fn clear_screen_and_scrollback(&self) {
        self.shared.screen.clear_screen_and_scrollback();
        self.mark_needs_render();
    }

    pub fn scroll_viewport_to_bottom(&self) {
        if self.shared.screen.scroll_viewport_bottom() {
            self.mark_needs_render();
        }
    }

    pub fn selection_press(
        &self,
        point: SelectionPoint,
        geometry: SelectionGeometry,
        click_count: u8,
        extend: bool,
    ) -> Result<()> {
        self.shared
            .screen
            .selection_press(point, geometry, click_count, extend)?;
        self.mark_needs_render();
        Ok(())
    }

    pub fn selection_drag(
        &self,
        point: SelectionPoint,
        geometry: SelectionGeometry,
    ) -> Result<SelectionAutoscroll> {
        let autoscroll = self.shared.screen.selection_drag(point, geometry)?;
        self.mark_needs_render();
        Ok(autoscroll)
    }

    pub fn selection_autoscroll_tick(
        &self,
        point: SelectionPoint,
        geometry: SelectionGeometry,
    ) -> Result<SelectionAutoscrollUpdate> {
        let update = self
            .shared
            .screen
            .selection_autoscroll_tick(point, geometry)?;
        if update.changed {
            self.mark_needs_render();
        }
        Ok(update)
    }

    pub fn selection_release(&self, point: Option<(u16, u16)>) -> Result<()> {
        self.shared.screen.selection_release(point)
    }

    pub fn selection_cancel_gesture(&self) {
        self.shared.screen.selection_cancel_gesture();
    }

    pub fn has_selection(&self) -> bool {
        self.shared.screen.has_selection()
    }

    pub fn selection_text(&self) -> Option<String> {
        self.shared.screen.selection_text()
    }

    pub fn take_selection_text(&self) -> Option<String> {
        let selection = self.shared.screen.take_selection_text();
        if selection.is_some() {
            self.mark_needs_render();
        }
        selection
    }

    pub fn clear_selection(&self) {
        if self.shared.screen.clear_selection() {
            self.mark_needs_render();
        }
    }

    pub fn title(&self) -> Option<String> {
        self.shared
            .screen
            .reported_title()
            .unwrap_or_else(|| self.title.clone())
    }

    pub fn take_bell(&self) -> bool {
        self.shared.screen.take_bell()
    }

    pub fn progress(&self) -> Option<crate::TerminalProgress> {
        self.shared.screen.progress()
    }

    pub fn set_clipboard_write_enabled(&self, enabled: bool) -> Result<(), String> {
        self.shared.screen.set_clipboard_write_enabled(enabled)
    }

    pub fn take_clipboard_write(&self) -> Option<String> {
        self.shared.screen.take_clipboard_write()
    }

    pub fn take_desktop_notification(&self) -> Option<crate::DesktopNotification> {
        self.shared.screen.take_desktop_notification()
    }

    pub fn current_dir(&self) -> Option<String> {
        self.shared
            .screen
            .current_dir()
            .or_else(|| self.current_dir.clone())
    }

    pub fn is_alive(&self) -> bool {
        self.poll_child_status();
        self.shared.alive.load(Ordering::Acquire) && !self.shared.screen.is_write_desynchronized()
    }

    pub fn prompt_state(&self) -> crate::TerminalPromptState {
        self.shared.screen.prompt_state()
    }

    pub fn take_command_finished(&self) -> Option<CommandFinishedSignal> {
        self.poll_child_status();
        self.shared.finished_signal.lock().take()
    }

    pub fn last_exit_code(&self) -> Option<i32> {
        self.poll_child_status();
        *self.shared.last_exit_code.lock()
    }

    pub fn last_command_duration(&self) -> Option<Duration> {
        self.poll_child_status();
        *self.shared.last_duration.lock()
    }

    pub fn input_generation(&self) -> u64 {
        self.input_generation.load(Ordering::Relaxed)
    }

    pub fn take_needs_render(&self) -> bool {
        self.shared.needs_render.swap(false, Ordering::AcqRel)
    }

    pub fn read_screen_text(&self, max_lines: usize) -> Vec<String> {
        snapshot_to_lines(&self.shared.screen.snapshot(), max_lines)
    }

    /// Drive the libghostty-vt render-state pipeline once and return a
    /// fresh `ScreenSnapshot`.
    pub fn snapshot(&self) -> Option<ScreenSnapshot> {
        let snapshot = self.shared.screen.try_snapshot();
        if snapshot.is_none() {
            self.mark_needs_render();
        }
        snapshot
    }

    pub fn acknowledge_snapshot(&self, generation: u64) {
        self.shared.screen.acknowledge_snapshot(generation);
    }

    pub fn set_theme(&self, colors: &TerminalColors) {
        let theme = theme_colors_to_vt(colors);
        self.shared.screen.set_theme(&theme);
        self.mark_needs_render();
    }

    pub fn read_recent_lines(&self, max_lines: usize) -> Vec<String> {
        self.shared.transcript.lock().recent_lines(max_lines)
    }

    pub fn search_text(&self, pattern: &str, limit: usize) -> Vec<(usize, String)> {
        self.shared.transcript.lock().search(pattern, limit)
    }

    pub fn is_decckm(&self) -> bool {
        self.shared.screen.is_decckm()
    }

    pub fn mouse_tracking_active(&self) -> bool {
        self.shared.screen.mouse_tracking_active()
    }

    pub fn mouse_motion_tracking_active(&self) -> bool {
        self.shared.screen.mouse_motion_tracking_active()
    }

    pub fn is_sgr_mouse(&self) -> bool {
        self.shared.screen.is_sgr_mouse()
    }

    pub fn set_dark_mode(&self, dark: bool) {
        self.shared.screen.set_dark_mode(dark);
        self.mark_needs_render();
    }

    fn poll_child_status(&self) {
        if !self.shared.alive.load(Ordering::Acquire) {
            return;
        }

        match &self.backend {
            LinuxPtyBackend::Local { child, .. } => {
                let Ok(Some(status)) = child.lock().try_wait() else {
                    return;
                };
                let exit_code = i32::try_from(status.exit_code()).unwrap_or(i32::MAX);
                self.shared
                    .mark_exited(Some(exit_code), self.started_at.elapsed());
            }
            LinuxPtyBackend::HostBridge { child, .. } => {
                let Ok(Some(status)) = child.lock().try_wait() else {
                    return;
                };
                let exit_code = status.code().unwrap_or(i32::MAX);
                self.shared
                    .mark_exited(Some(exit_code), self.started_at.elapsed());
            }
        }
    }
}

impl Drop for LinuxPtySession {
    fn drop(&mut self) {
        match &self.backend {
            LinuxPtyBackend::Local {
                child, io_shutdown, ..
            } => {
                let _ = io_shutdown.shutdown(std::net::Shutdown::Both);
                if let Err(err) = child.lock().kill() {
                    log::debug!("failed to terminate linux pty child during drop: {err}");
                }
                self.input_worker.shutdown();
            }
            LinuxPtyBackend::HostBridge {
                child,
                stream_shutdown,
                socket_path,
            } => {
                if let Err(err) = child.lock().kill() {
                    log::debug!("failed to terminate host pty bridge child during drop: {err}");
                }
                let _ = stream_shutdown.shutdown(std::net::Shutdown::Both);
                self.input_worker.shutdown();
                let _ = std::fs::remove_file(socket_path);
            }
        }
        self.shared.alive.store(false, Ordering::Release);
        self.shared.needs_render.store(true, Ordering::Release);
        self.shared.wake();
    }
}

fn spawn_local(
    options: LinuxPtyOptions,
) -> std::result::Result<LinuxPtySession, LinuxPtySpawnError> {
    let pty_system = native_pty_system();
    let pty_size = pty_size_from_surface(&options.size);
    let pair = pty_system
        .openpty(pty_size)
        .context("failed to open linux pty")?;
    let master_fd = pair
        .master
        .as_raw_fd()
        .context("linux pty master did not expose a file descriptor")?;
    let reader_readiness =
        duplicate_fd(master_fd).context("failed to clone linux pty reader fd")?;
    let writer_readiness =
        duplicate_fd(master_fd).context("failed to clone linux pty writer fd")?;
    set_fd_nonblocking(master_fd).context("failed to make linux pty non-blocking")?;
    let (io_shutdown, io_cancel) =
        UnixStream::pair().context("failed to create linux pty cancellation socket")?;
    let writer_cancel = io_cancel
        .try_clone()
        .context("failed to clone linux pty writer cancellation socket")?;

    let target_program = options
        .command_program
        .clone()
        .or_else(|| options.program.clone().map(OsString::from))
        .unwrap_or_else(|| {
            std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/bash"))
        });

    let mut command = CommandBuilder::new(&target_program);
    if let Some(args) = options.command_args.as_ref() {
        for arg in args {
            command.arg(arg);
        }
    } else {
        configure_shell_startup(&target_program, &mut command);
    }
    command.env("TERM", "xterm-256color");

    if let Some(cwd) = options.cwd.as_ref() {
        command.cwd(cwd);
    }

    let reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone linux pty reader")?;
    let mut writer = pair
        .master
        .take_writer()
        .context("failed to take linux pty writer")?;
    let (input_queue, input_worker) = PtyWriteQueue::spawn("con-linux-pty-writer", move |data| {
        write_all_cancellable(writer.as_mut(), &writer_readiness, &writer_cancel, data)
    })
    .context("failed to spawn linux pty writer")?;
    let input = LinuxPtyInput::Local(input_queue);
    let theme_owned = options.theme.as_ref().map(theme_colors_to_vt);
    let screen = Arc::new(
        VtScreen::new_with_write_pty(
            options.size.columns.max(1),
            options.size.rows.max(1),
            theme_owned.as_ref(),
            Some({
                let input = input.clone();
                Arc::new(move |data: &[u8], priority| input.write_data(data, priority))
            }),
        )
        .context("failed to create linux vt screen")?,
    );
    screen.set_clipboard_write_policy(options.clipboard_write_policy.clone());
    screen.set_desktop_notification_policy(options.desktop_notification_policy.clone());
    screen
        .set_clipboard_write_enabled(options.clipboard_write)
        .map_err(anyhow::Error::msg)?;
    if let Some(output) = options
        .initial_output
        .as_deref()
        .filter(|output| !output.is_empty())
    {
        screen.feed(output);
    }
    let child = pair.slave.spawn_command(command).map_err(|err| {
        LinuxPtySpawnError::permanent(err.context("failed to spawn linux pty child process"))
    })?;

    let shared = Arc::new(SessionShared::new(
        screen,
        options.wake_generation,
        options.wake_callback,
    ));
    if let Some(output) = options.initial_output.as_deref()
        && let Ok(text) = std::str::from_utf8(output)
    {
        shared.transcript.lock().push(text);
    }
    let started_at = Instant::now();
    spawn_reader_thread(
        reader,
        reader_readiness,
        io_cancel,
        shared.clone(),
        started_at,
    );

    Ok(LinuxPtySession {
        backend: LinuxPtyBackend::Local {
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            io_shutdown,
        },
        input,
        input_worker,
        shared,
        size: Mutex::new(options.size),
        title: Some(default_title(
            options.cwd.as_deref(),
            options
                .command_program
                .as_deref()
                .or_else(|| options.program.as_deref().map(OsStr::new)),
        )),
        current_dir: options.cwd.map(|cwd| cwd.to_string_lossy().to_string()),
        input_generation: AtomicU64::new(0),
        started_at,
    })
}

fn resolve_host_socket_dir() -> PathBuf {
    let runtime_dir = con_paths::runtime_dir().join("con");
    if std::fs::create_dir_all(&runtime_dir).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700));
        }
        return runtime_dir;
    }

    let cache_dir = con_paths::app_cache_dir();
    let _ = std::fs::create_dir_all(&cache_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&cache_dir, std::fs::Permissions::from_mode(0o700));
    }
    cache_dir
}

fn stop_host_bridge_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn host_bridge_option(name: &[u8], value: &OsStr) -> OsString {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let mut argument = Vec::with_capacity(name.len() + 1 + value.as_bytes().len());
    argument.extend_from_slice(name);
    argument.push(b'=');
    argument.extend_from_slice(value.as_bytes());
    OsString::from_vec(argument)
}

fn await_fallback_bridge_startup(
    stream: &mut UnixStream,
    child: &mut std::process::Child,
    timeout: Duration,
) -> std::result::Result<(), LinuxPtySpawnError> {
    if let Err(error) = stream.set_read_timeout(Some(timeout.max(Duration::from_millis(20)))) {
        stop_host_bridge_child(child);
        return Err(anyhow::Error::new(error)
            .context("failed to configure fallback bridge startup timeout")
            .into());
    }

    let mut tag = [0u8; 1];
    if let Err(error) = stream.read_exact(&mut tag) {
        let status = child.try_wait().ok().flatten();
        stop_host_bridge_child(child);
        if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
            return Err(anyhow::anyhow!("timeout waiting for fallback pty bridge startup").into());
        }
        return Err(LinuxPtySpawnError::permanent(anyhow::anyhow!(
            "fallback pty bridge exited before confirming terminal startup{}",
            status.map_or_else(String::new, |status| format!(" ({status})"))
        )));
    }

    match tag[0] {
        BRIDGE_READY => {
            if let Err(error) = stream.set_read_timeout(None) {
                stop_host_bridge_child(child);
                return Err(anyhow::Error::new(error)
                    .context("failed to clear fallback bridge startup timeout")
                    .into());
            }
            Ok(())
        }
        BRIDGE_STARTUP_ERROR => {
            let mut len_bytes = [0u8; 4];
            if let Err(error) = stream.read_exact(&mut len_bytes) {
                stop_host_bridge_child(child);
                return Err(LinuxPtySpawnError::permanent(
                    anyhow::Error::new(error).context(
                        "fallback pty bridge closed while reporting terminal startup failure",
                    ),
                ));
            }
            let len = u32::from_be_bytes(len_bytes) as usize;
            if len > MAX_BRIDGE_STARTUP_ERROR_BYTES {
                stop_host_bridge_child(child);
                return Err(LinuxPtySpawnError::permanent(anyhow::anyhow!(
                    "fallback pty bridge reported an oversized startup error"
                )));
            }
            let mut message = vec![0u8; len];
            if let Err(error) = stream.read_exact(&mut message) {
                stop_host_bridge_child(child);
                return Err(LinuxPtySpawnError::permanent(
                    anyhow::Error::new(error).context(
                        "fallback pty bridge closed while reporting terminal startup failure",
                    ),
                ));
            }
            stop_host_bridge_child(child);
            Err(LinuxPtySpawnError::permanent(anyhow::anyhow!(
                "failed to start host terminal command: {}",
                String::from_utf8_lossy(&message)
            )))
        }
        tag => {
            stop_host_bridge_child(child);
            Err(LinuxPtySpawnError::permanent(anyhow::anyhow!(
                "fallback pty bridge sent unknown startup frame 0x{tag:02x}"
            )))
        }
    }
}

fn spawn_host_bridge(
    options: LinuxPtyOptions,
) -> std::result::Result<LinuxPtySession, LinuxPtySpawnError> {
    use std::os::unix::net::UnixListener;

    let socket_dir = resolve_host_socket_dir();
    let session_id = uuid::Uuid::new_v4();
    let socket_path = socket_dir.join(format!("pty-{session_id}.sock"));
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind pty socket at {}", socket_path.display()))?;

    let mut cmd = std::process::Command::new("flatpak-spawn");
    cmd.arg("--host");
    cmd.arg("--unset-env=FLATPAK_ID");
    cmd.arg("--unset-env=container");

    let host_cli_probe = con_paths::host_command("con-cli")
        .args(["pty-bridge", "--help"])
        .output();
    let use_con_cli = host_cli_probe
        .map(|output| {
            bridge_help_supports_literal_commands(output.status.success(), &output.stdout)
        })
        .unwrap_or(false);

    if use_con_cli {
        cmd.arg("con-cli");
        cmd.arg("pty-bridge");
        cmd.arg("--socket").arg(&socket_path);
        cmd.arg("--cols")
            .arg(options.size.columns.max(1).to_string());
        cmd.arg("--rows").arg(options.size.rows.max(1).to_string());
        if let Some(cwd) = &options.cwd {
            cmd.arg(host_bridge_option(b"--cwd", cwd.as_os_str()));
        }
        if let Some(prog) = options
            .command_program
            .as_deref()
            .or_else(|| options.program.as_deref().map(OsStr::new))
        {
            cmd.arg(host_bridge_option(b"--program", prog));
        }
        if let Some(args) = options.command_args.as_ref() {
            cmd.arg("--literal-command");
            cmd.arg("--").args(args);
        }
    } else {
        cmd.arg("python3");
        cmd.arg("-c");
        cmd.arg(EMBEDDED_PYTHON_BRIDGE);
        cmd.arg("--socket").arg(&socket_path);
        cmd.arg("--cols")
            .arg(options.size.columns.max(1).to_string());
        cmd.arg("--rows").arg(options.size.rows.max(1).to_string());
        if let Some(cwd) = &options.cwd {
            cmd.arg(host_bridge_option(b"--cwd", cwd.as_os_str()));
        }
        if let Some(prog) = options
            .command_program
            .as_deref()
            .or_else(|| options.program.as_deref().map(OsStr::new))
        {
            cmd.arg(host_bridge_option(b"--program", prog));
        }
        if let Some(args) = options.command_args.as_ref() {
            cmd.arg("--literal-command");
            cmd.arg("--").args(args);
        }
    }

    log::info!(
        "spawn_host_bridge: executing flatpak host pty bridge (socket={})",
        socket_path.display()
    );
    let mut child = cmd.spawn().context("failed to spawn host pty bridge")?;

    listener.set_nonblocking(true)?;
    let connect_deadline = Instant::now() + Duration::from_secs(5);
    let mut stream: Option<std::os::unix::net::UnixStream> = None;
    while Instant::now() < connect_deadline {
        match listener.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false)?;
                stream = Some(s);
                break;
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                if let Some(status) = child
                    .try_wait()
                    .context("failed to inspect host pty bridge while connecting")?
                {
                    return Err(LinuxPtySpawnError::permanent(anyhow::anyhow!(
                        "host pty bridge exited before opening the terminal connection ({status})"
                    )));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(anyhow::Error::new(err)
                    .context("error waiting for pty bridge connection")
                    .into());
            }
        }
    }

    let mut stream = match stream {
        Some(stream) => stream,
        None => {
            let status = child
                .try_wait()
                .context("failed to inspect host pty bridge after connection timeout")?;
            if let Some(status) = status {
                return Err(LinuxPtySpawnError::permanent(anyhow::anyhow!(
                    "host pty bridge exited before opening the terminal connection ({status})"
                )));
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::anyhow!("timeout waiting for host pty bridge connection").into());
        }
    };
    if !use_con_cli {
        let handshake_timeout = connect_deadline.saturating_duration_since(Instant::now());
        await_fallback_bridge_startup(&mut stream, &mut child, handshake_timeout)?;
    }
    let stream_writer = stream.try_clone().context("clone socket writer")?;
    let stream_reader = stream.try_clone().context("clone socket reader")?;
    let mut stream_writer = stream_writer;
    let (input_queue, input_worker) =
        PtyWriteQueue::spawn("con-linux-pty-bridge-writer", move |frame| {
            stream_writer.write_all(frame)?;
            stream_writer.flush()
        })
        .context("failed to spawn host pty bridge writer")?;
    let input = LinuxPtyInput::HostBridge(input_queue);

    let theme_owned = options.theme.as_ref().map(theme_colors_to_vt);
    let screen = Arc::new(
        VtScreen::new_with_write_pty(
            options.size.columns.max(1),
            options.size.rows.max(1),
            theme_owned.as_ref(),
            Some({
                let input = input.clone();
                Arc::new(move |data: &[u8], priority| input.write_data(data, priority))
            }),
        )
        .context("failed to create linux vt screen")?,
    );
    screen.set_clipboard_write_policy(options.clipboard_write_policy.clone());
    screen.set_desktop_notification_policy(options.desktop_notification_policy.clone());
    screen
        .set_clipboard_write_enabled(options.clipboard_write)
        .map_err(|message| {
            stop_host_bridge_child(&mut child);
            let _ = std::fs::remove_file(&socket_path);
            anyhow::Error::msg(message)
        })?;

    if let Some(output) = options
        .initial_output
        .as_deref()
        .filter(|output| !output.is_empty())
    {
        screen.feed(output);
    }

    let shared = Arc::new(SessionShared::new(
        screen,
        options.wake_generation,
        options.wake_callback,
    ));
    if let Some(output) = options.initial_output.as_deref()
        && let Ok(text) = std::str::from_utf8(output)
    {
        shared.transcript.lock().push(text);
    }
    let started_at = Instant::now();
    spawn_bridge_reader_thread(stream_reader, shared.clone(), started_at);

    Ok(LinuxPtySession {
        backend: LinuxPtyBackend::HostBridge {
            stream_shutdown: stream,
            socket_path,
            child: Mutex::new(child),
        },
        input,
        input_worker,
        shared,
        size: Mutex::new(options.size),
        title: Some(default_title(
            options.cwd.as_deref(),
            options
                .command_program
                .as_deref()
                .or_else(|| options.program.as_deref().map(OsStr::new)),
        )),
        current_dir: options.cwd.map(|cwd| cwd.to_string_lossy().to_string()),
        input_generation: AtomicU64::new(0),
        started_at,
    })
}

fn bridge_help_supports_literal_commands(status_success: bool, stdout: &[u8]) -> bool {
    status_success
        && stdout
            .windows(b"--literal-command".len())
            .any(|window| window == b"--literal-command")
}

fn spawn_bridge_reader_thread(
    mut stream_reader: std::os::unix::net::UnixStream,
    shared: Arc<SessionShared>,
    started_at: Instant,
) {
    std::thread::Builder::new()
        .name("con-linux-pty-bridge-reader".into())
        .spawn(move || {
            loop {
                let mut tag = [0u8; 1];
                if stream_reader.read_exact(&mut tag).is_err() {
                    shared.mark_exited(None, started_at.elapsed());
                    break;
                }
                match tag[0] {
                    0x00 => {
                        let mut len_bytes = [0u8; 4];
                        if stream_reader.read_exact(&mut len_bytes).is_err() {
                            shared.mark_exited(None, started_at.elapsed());
                            break;
                        }
                        let len = u32::from_be_bytes(len_bytes) as usize;
                        if len > MAX_BRIDGE_FRAME_BYTES {
                            shared.mark_exited(None, started_at.elapsed());
                            break;
                        }
                        let mut payload = vec![0u8; len];
                        if stream_reader.read_exact(&mut payload).is_err() {
                            shared.mark_exited(None, started_at.elapsed());
                            break;
                        }
                        shared.push_output(&payload);
                    }
                    0x02 => {
                        let mut code_bytes = [0u8; 4];
                        let code = if stream_reader.read_exact(&mut code_bytes).is_ok() {
                            Some(i32::from_be_bytes(code_bytes))
                        } else {
                            None
                        };
                        shared.mark_exited(code, started_at.elapsed());
                        break;
                    }
                    _ => {
                        shared.mark_exited(None, started_at.elapsed());
                        break;
                    }
                }
            }
        })
        .expect("failed to spawn linux pty bridge reader thread");
}

fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    readiness: OwnedFd,
    cancel: UnixStream,
    shared: Arc<SessionShared>,
    started_at: Instant,
) {
    std::thread::Builder::new()
        .name("con-linux-pty-reader".into())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match wait_for_pty_io(readiness.as_raw_fd(), cancel.as_raw_fd(), libc::POLLIN) {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(err) => {
                        log::debug!("linux pty reader poll terminated: {err}");
                        shared.mark_exited(None, started_at.elapsed());
                        break;
                    }
                }
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        shared.mark_exited(None, started_at.elapsed());
                        break;
                    }
                    Ok(read) => {
                        shared.push_output(&buffer[..read]);
                    }
                    Err(err)
                        if matches!(err.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) =>
                    {
                        continue;
                    }
                    Err(err) => {
                        log::debug!("linux pty reader terminated: {err}");
                        shared.mark_exited(None, started_at.elapsed());
                        break;
                    }
                }
            }
        })
        .expect("failed to spawn linux pty reader thread");
}

fn default_title(cwd: Option<&Path>, program: Option<&OsStr>) -> String {
    if let Some(name) = cwd
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
    {
        return name.to_string();
    }

    if let Some(name) = program
        .and_then(|program| Path::new(program).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
    {
        return name.to_string();
    }

    "shell".to_string()
}

fn configure_shell_startup(program: &OsStr, command: &mut CommandBuilder) {
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
        "pwsh" => {
            command.arg("-NoLogo");
        }
        "xonsh" => {
            command.arg("-i");
        }
        "nu" => {
            command.arg("--interactive");
        }
        "bash" | "zsh" | "sh" | "dash" | "ksh" | "mksh" => {
            command.arg("-l");
        }
        _ => {
            // Do not pass arbitrary flags to unknown binaries
        }
    }
}

const EMBEDDED_PYTHON_BRIDGE: &str = r#"
import argparse, fcntl, os, pty, select, socket, struct, sys, termios, time

PTY_EXIT_DRAIN_QUIET = 0.025
PTY_EXIT_DRAIN_LIMIT = 0.250
BRIDGE_READY = 0x03
BRIDGE_STARTUP_ERROR = 0x04
MAX_STARTUP_ERROR_BYTES = 64 * 1024

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--socket', required=True)
    parser.add_argument('--cols', type=int, default=80)
    parser.add_argument('--rows', type=int, default=24)
    parser.add_argument('--cwd')
    parser.add_argument('--program')
    parser.add_argument('--literal-command', action='store_true')
    args, remaining = parser.parse_known_args()
    if remaining and remaining[0] == '--':
        remaining = remaining[1:]

    # Validate the host working directory before connecting. Exiting before the
    # handshake lets Con classify this as a permanent launch error instead of
    # attaching a shell in an unrelated inherited directory.
    if args.cwd:
        try:
            os.chdir(args.cwd)
        except OSError as e:
            sys.stderr.write(f"failed to change directory to {args.cwd}: {e}\n")
            sys.exit(126)

    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(args.socket)
    except Exception as e:
        sys.stderr.write(f"failed to connect to socket {args.socket}: {e}\n")
        sys.exit(1)

    master, slave = pty.openpty()
    ws = struct.pack('HHHH', max(args.rows, 1), max(args.cols, 1), 0, 0)
    try:
        fcntl.ioctl(master, termios.TIOCSWINSZ, ws)
    except OSError:
        pass

    exec_status_read, exec_status_write = os.pipe()
    os.set_inheritable(exec_status_write, False)
    pid = os.fork()
    if pid == 0:
        os.close(exec_status_read)
        try:
            os.close(master)
            os.setsid()
            try:
                fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
            except OSError:
                pass
            os.dup2(slave, 0)
            os.dup2(slave, 1)
            os.dup2(slave, 2)
            if slave > 2:
                os.close(slave)
            os.environ['TERM'] = 'xterm-256color'
            prog = args.program or os.environ.get('SHELL') or '/bin/bash'
            argv = [prog]
            if not args.literal_command and not remaining:
                shell_name = os.path.basename(prog)
                if shell_name in ('bash', 'zsh', 'sh', 'dash', 'ksh', 'mksh'):
                    argv.append('-l')
                elif shell_name == 'fish':
                    argv.extend(['--login', '--interactive'])
                elif shell_name == 'pwsh':
                    argv.append('-NoLogo')
                elif shell_name == 'nu':
                    argv.append('--interactive')
                elif shell_name == 'xonsh':
                    argv.append('-i')
            else:
                argv.extend(remaining)
            os.execvp(prog, argv)
        except BaseException as e:
            message = f"failed to start {args.program or 'host shell'}: {e}".encode('utf-8', errors='replace')
            try:
                os.write(exec_status_write, message[:MAX_STARTUP_ERROR_BYTES])
            except OSError:
                pass
            os._exit(127)

    os.close(exec_status_write)
    os.close(slave)

    exec_error = b''
    while len(exec_error) < MAX_STARTUP_ERROR_BYTES:
        try:
            chunk = os.read(exec_status_read, MAX_STARTUP_ERROR_BYTES - len(exec_error))
        except OSError as e:
            exec_error = f"failed to confirm command startup: {e}".encode('utf-8', errors='replace')
            break
        if not chunk:
            break
        exec_error += chunk
    os.close(exec_status_read)

    if exec_error:
        frame = bytes([BRIDGE_STARTUP_ERROR]) + struct.pack('>I', len(exec_error)) + exec_error
        try:
            sock.sendall(frame)
        except OSError:
            pass
        try:
            os.waitpid(pid, 0)
        except OSError:
            pass
        sock.close()
        sys.exit(127)

    try:
        sock.sendall(bytes([BRIDGE_READY]))
    except OSError:
        sys.exit(1)

    child_status = None
    drain_deadline = None
    quiet_deadline = None

    while True:
        if child_status is None:
            try:
                exited_pid, status = os.waitpid(pid, os.WNOHANG)
            except OSError:
                exited_pid, status = pid, 0
            if exited_pid == pid:
                child_status = status
                now = time.monotonic()
                drain_deadline = now + PTY_EXIT_DRAIN_LIMIT
                quiet_deadline = now + PTY_EXIT_DRAIN_QUIET

        if child_status is None:
            readable = [sock, master]
            timeout = PTY_EXIT_DRAIN_QUIET
        else:
            now = time.monotonic()
            timeout = min(drain_deadline - now, quiet_deadline - now)
            if timeout <= 0:
                break
            readable = [master]

        try:
            r, _, _ = select.select(readable, [], [], timeout)
        except (OSError, select.error):
            break

        if not r:
            if child_status is not None:
                break
            continue

        if child_status is None and sock in r:
            try:
                tag = sock.recv(1)
            except OSError:
                break
            if not tag:
                break
            if tag[0] == 0:
                raw_len = b''
                while len(raw_len) < 4:
                    chunk = sock.recv(4 - len(raw_len))
                    if not chunk:
                        break
                    raw_len += chunk
                if len(raw_len) < 4:
                    break
                payload_len = struct.unpack('>I', raw_len)[0]
                payload = b''
                while len(payload) < payload_len:
                    chunk = sock.recv(payload_len - len(payload))
                    if not chunk:
                        break
                    payload += chunk
                try:
                    os.write(master, payload)
                except OSError:
                    break
            elif tag[0] == 1:
                buf = b''
                while len(buf) < 8:
                    chunk = sock.recv(8 - len(buf))
                    if not chunk:
                        break
                    buf += chunk
                if len(buf) == 8:
                    cols, rows, wp, hp = struct.unpack('>HHHH', buf)
                    ws = struct.pack('HHHH', max(rows, 1), max(cols, 1), wp, hp)
                    try:
                        fcntl.ioctl(master, termios.TIOCSWINSZ, ws)
                    except OSError:
                        pass
        if master in r:
            try:
                data = os.read(master, 8192)
            except OSError:
                break
            if not data:
                break
            frame = bytes([0]) + struct.pack('>I', len(data)) + data
            try:
                sock.sendall(frame)
            except OSError:
                break

            if child_status is not None:
                quiet_deadline = min(
                    time.monotonic() + PTY_EXIT_DRAIN_QUIET,
                    drain_deadline,
                )

    if child_status is None:
        try:
            _, child_status = os.waitpid(pid, 0)
        except OSError:
            child_status = 0
    exit_code = os.waitstatus_to_exitcode(child_status) if hasattr(os, 'waitstatus_to_exitcode') else (child_status >> 8)

    try:
        frame = bytes([2]) + struct.pack('>i', exit_code)
        sock.sendall(frame)
    except OSError:
        pass

    try:
        sock.close()
    except OSError:
        pass

if __name__ == '__main__':
    main()
"#;

fn pty_size_from_surface(size: &SurfaceSize) -> PtySize {
    PtySize {
        rows: size.rows.max(1),
        cols: size.columns.max(1),
        pixel_width: size.width_px.min(u32::from(u16::MAX)) as u16,
        pixel_height: size.height_px.min(u32::from(u16::MAX)) as u16,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::io::{ErrorKind, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixListener;
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime};

    use crate::transcript::{TranscriptBuffer, sanitize_terminal_output};
    use crate::vt::VtScreen;

    use super::{
        BRIDGE_READY, BRIDGE_STARTUP_ERROR, EMBEDDED_PYTHON_BRIDGE, LinuxPtyOptions,
        LinuxPtySession, SessionShared, bridge_help_supports_literal_commands, duplicate_fd,
        host_bridge_option, set_fd_nonblocking, write_all_cancellable,
    };

    fn unique_test_socket(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "con-python-bridge-{name}-{}-{unique}.sock",
            std::process::id()
        ))
    }

    #[test]
    fn host_bridge_probe_requires_literal_command_capability() {
        assert!(bridge_help_supports_literal_commands(
            true,
            b"Usage: con-cli pty-bridge [OPTIONS]\n    --literal-command"
        ));
        assert!(!bridge_help_supports_literal_commands(
            true,
            b"Usage: con-cli pty-bridge [OPTIONS]"
        ));
        assert!(!bridge_help_supports_literal_commands(
            false,
            b"--literal-command"
        ));
    }

    #[test]
    fn host_bridge_options_preserve_leading_dashes_and_non_utf8_bytes() {
        let value = OsStr::from_bytes(b"-tool-\xff");

        assert_eq!(
            host_bridge_option(b"--program", value).as_bytes(),
            b"--program=-tool-\xff"
        );
        assert_eq!(
            host_bridge_option(b"--cwd", value).as_bytes(),
            b"--cwd=-tool-\xff"
        );
    }

    #[test]
    fn python_bridge_rejects_invalid_working_directory_before_connecting() {
        let socket = unique_test_socket("cwd");
        let missing_cwd = socket.with_extension("missing");
        let output = Command::new("python3")
            .arg("-c")
            .arg(EMBEDDED_PYTHON_BRIDGE)
            .arg("--socket")
            .arg(&socket)
            .arg("--cwd")
            .arg(&missing_cwd)
            .output()
            .expect("run embedded Python bridge");

        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(126));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("failed to change directory"),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn python_bridge_bounds_drain_when_descendant_keeps_pty_open() {
        let socket = unique_test_socket("drain");
        let listener = UnixListener::bind(&socket).expect("bind Python bridge test socket");
        let started_at = std::time::Instant::now();
        let mut bridge = Command::new("python3")
            .arg("-c")
            .arg(EMBEDDED_PYTHON_BRIDGE)
            .arg("--socket")
            .arg(&socket)
            .arg("--program")
            .arg("/bin/sh")
            .arg("--")
            .arg("-c")
            .arg("sleep 2 & printf con-marker")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn embedded Python bridge");
        let (mut stream, _) = listener.accept().expect("accept Python bridge");
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("bound bridge read");

        let mut frames = Vec::new();
        if let Err(error) = stream.read_to_end(&mut frames) {
            let _ = bridge.kill();
            let _ = bridge.wait();
            panic!("Python bridge did not finish bounded drain: {error}");
        }
        let status = bridge.wait().expect("wait for Python bridge");
        let _ = std::fs::remove_file(&socket);

        assert!(status.success());
        assert!(started_at.elapsed() < Duration::from_secs(1));
        assert_eq!(frames.first(), Some(&BRIDGE_READY));
        assert!(
            frames
                .windows(b"con-marker".len())
                .any(|window| window == b"con-marker"),
            "final command output should precede exit: {frames:?}"
        );
        assert!(
            frames.len() >= 5 && frames[frames.len() - 5] == 0x02,
            "bridge stream should end with TAG_EXIT: {frames:?}"
        );
    }

    #[test]
    fn python_bridge_reports_exec_failure_before_ready() {
        let socket = unique_test_socket("exec");
        let listener = UnixListener::bind(&socket).expect("bind Python bridge test socket");
        let mut bridge = Command::new("python3")
            .arg("-c")
            .arg(EMBEDDED_PYTHON_BRIDGE)
            .arg("--socket")
            .arg(&socket)
            .arg("--program")
            .arg("/definitely/missing/con-terminal-test-program")
            .arg("--literal-command")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn embedded Python bridge");
        let (mut stream, _) = listener.accept().expect("accept Python bridge");
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("bound bridge read");

        let mut frames = Vec::new();
        stream
            .read_to_end(&mut frames)
            .expect("read startup error frame");
        let status = bridge.wait().expect("wait for Python bridge");
        let _ = std::fs::remove_file(&socket);

        assert_eq!(status.code(), Some(127));
        assert_eq!(frames.first(), Some(&BRIDGE_STARTUP_ERROR));
        assert_ne!(frames.first(), Some(&BRIDGE_READY));
        assert!(
            frames
                .windows(b"failed to start".len())
                .any(|window| window == b"failed to start"),
            "startup error should explain the exec failure: {frames:?}"
        );
    }

    #[test]
    fn missing_explicit_program_is_a_permanent_startup_error() {
        let result = LinuxPtySession::spawn(LinuxPtyOptions {
            command_program: Some(OsString::from(
                "/definitely/missing/con-terminal-test-program",
            )),
            command_args: Some(Vec::new()),
            ..LinuxPtyOptions::default()
        });

        let error = match result {
            Ok(_) => panic!("missing explicit program should fail"),
            Err(error) => error,
        };
        assert!(!error.is_retryable());
        assert!(
            error
                .to_string()
                .contains("failed to spawn linux pty child")
        );
    }

    #[test]
    fn explicit_command_preserves_argument_boundaries() {
        let session = LinuxPtySession::spawn(LinuxPtyOptions {
            command_program: Some(OsString::from("/bin/sh")),
            command_args: Some(vec![
                OsString::from("-c"),
                OsString::from("printf '<%s>|<%s>\\n' \"$1\" \"$2\""),
                OsString::from("con-command-test"),
                OsString::from("hello world"),
                OsString::from("-leading-dash"),
            ]),
            ..LinuxPtyOptions::default()
        })
        .expect("spawn explicit command");

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let output = loop {
            session.poll_child_status();
            let output = session.read_recent_lines(20).join("\n");
            if output.contains("<hello world>|<-leading-dash>") {
                break output;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for explicit command output: {output:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        };

        assert_eq!(output.matches("<hello world>|<-leading-dash>").count(), 1);
    }

    #[test]
    fn blocked_writer_stops_when_session_is_cancelled() {
        let (mut writer, _unread_peer) = UnixStream::pair().expect("create blocked writer socket");
        set_fd_nonblocking(writer.as_raw_fd()).expect("make test writer non-blocking");
        let readiness = duplicate_fd(writer.as_raw_fd()).expect("clone test writer fd");

        let fill = [0_u8; 8 * 1024];
        loop {
            match writer.write(&fill) {
                Ok(0) => panic!("test writer accepted zero bytes"),
                Ok(_) => {}
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(err) => panic!("failed to fill test writer: {err}"),
            }
        }

        let (shutdown, cancel) = UnixStream::pair().expect("create cancellation socket");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            entered_tx.send(()).expect("signal writer entry");
            result_tx
                .send(write_all_cancellable(
                    &mut writer,
                    &readiness,
                    &cancel,
                    b"blocked",
                ))
                .expect("send writer result");
        });

        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("writer thread started");
        shutdown
            .shutdown(std::net::Shutdown::Both)
            .expect("cancel writer");
        let err = result_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("cancelled writer returned")
            .expect_err("cancelled writer must fail");
        assert_eq!(err.kind(), ErrorKind::BrokenPipe);
        handle.join().expect("writer thread joined");
    }

    #[test]
    fn transcript_buffer_returns_recent_lines_in_order() {
        let mut transcript = TranscriptBuffer::default();
        transcript.push("one\ntwo\nthree\nfour\n");

        assert_eq!(
            transcript.recent_lines(2),
            vec!["three".to_string(), "four".to_string()]
        );
    }

    #[test]
    fn transcript_buffer_search_is_bounded() {
        let mut transcript = TranscriptBuffer::default();
        transcript.push("alpha\nbeta\nalphabet\n");

        assert_eq!(
            transcript.search("alpha", 1),
            vec![(0, "alpha".to_string())]
        );
    }

    #[test]
    fn sanitize_terminal_output_strips_ansi_sequences() {
        assert_eq!(
            sanitize_terminal_output("\x1b]0;title\x07\x1b[31mhello\x1b[0m"),
            "hello"
        );
    }

    #[test]
    fn sanitize_terminal_output_honors_carriage_return_rewrites() {
        assert_eq!(sanitize_terminal_output("loading\rready"), "ready");
    }

    #[test]
    fn mark_exited_backfills_exit_code_after_eof() {
        let shared = SessionShared::new(
            Arc::new(VtScreen::new(80, 24, None).expect("create vt screen")),
            None,
            None,
        );

        shared.mark_exited(None, Duration::from_millis(10));
        shared
            .needs_render
            .store(false, std::sync::atomic::Ordering::Release);
        shared.mark_exited(Some(7), Duration::from_millis(20));

        assert_eq!(*shared.last_exit_code.lock(), Some(7));
        assert_eq!(
            *shared.last_duration.lock(),
            Some(Duration::from_millis(20))
        );
        assert!(
            shared
                .needs_render
                .load(std::sync::atomic::Ordering::Acquire)
        );
        let signal_guard = shared.finished_signal.lock();
        let finished = signal_guard
            .as_ref()
            .expect("exit signal should be backfilled");
        assert_eq!(finished.exit_code, Some(7));
        assert_eq!(finished.duration, Duration::from_millis(20));
    }
}
