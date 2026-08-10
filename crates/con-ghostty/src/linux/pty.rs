use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::stub::{CommandFinishedSignal, SurfaceSize, TerminalColors};
use crate::transcript::{TranscriptBuffer, snapshot_to_lines};
use crate::vt::{ScreenSnapshot, ThemeColors, VtScreen};

const DEFAULT_COLUMNS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

pub type LinuxWakeCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
pub struct LinuxPtyOptions {
    pub cwd: Option<PathBuf>,
    pub program: Option<String>,
    pub size: SurfaceSize,
    pub initial_output: Option<Vec<u8>>,
    pub wake_generation: Option<Arc<AtomicU64>>,
    pub wake_callback: Option<LinuxWakeCallback>,
    pub theme: Option<TerminalColors>,
}

impl Default for LinuxPtyOptions {
    fn default() -> Self {
        Self {
            cwd: None,
            program: None,
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
        }
    }
}

fn theme_colors_to_vt(colors: &TerminalColors) -> ThemeColors {
    ThemeColors::from_ansi16(colors.foreground, colors.background, colors.palette)
}

pub struct LinuxPtySession {
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
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

    fn push_output(&self, chunk: &str) {
        self.transcript.lock().push(chunk);
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
    pub fn spawn(options: LinuxPtyOptions) -> Result<Self> {
        let pty_system = native_pty_system();
        let pty_size = pty_size_from_surface(&options.size);
        let pair = pty_system
            .openpty(pty_size)
            .context("failed to open linux pty")?;

        let mut command = match options.program.as_ref() {
            Some(program) => {
                let mut command = CommandBuilder::new(program);
                configure_shell_startup(program, &mut command);
                command
            }
            None => CommandBuilder::new_default_prog(),
        };
        command.env("TERM", "xterm-256color");
        if let Some(cwd) = options.cwd.as_ref() {
            command.cwd(cwd);
        }

        let reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone linux pty reader")?;
        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .context("failed to take linux pty writer")?,
        ));
        let theme_owned = options.theme.as_ref().map(theme_colors_to_vt);
        let screen = Arc::new(
            VtScreen::new_with_write_pty(
                options.size.columns.max(1),
                options.size.rows.max(1),
                theme_owned.as_ref(),
                Some({
                    let writer = writer.clone();
                    Arc::new(move |data: &[u8]| {
                        if let Err(err) = writer.lock().write_all(data) {
                            log::debug!("linux vt write_pty failed: {err:#}");
                        }
                    })
                }),
            )
            .context("failed to create linux vt screen")?,
        );
        if let Some(output) = options
            .initial_output
            .as_deref()
            .filter(|output| !output.is_empty())
        {
            screen.feed(output);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .context("failed to spawn shell in linux pty")?;

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
        spawn_reader_thread(reader, shared.clone(), started_at);

        Ok(Self {
            master: Mutex::new(pair.master),
            writer,
            child: Mutex::new(child),
            shared,
            size: Mutex::new(options.size),
            title: Some(default_title(
                options.cwd.as_deref(),
                options.program.as_deref(),
            )),
            current_dir: options.cwd.map(|cwd| cwd.to_string_lossy().to_string()),
            input_generation: AtomicU64::new(0),
            started_at,
        })
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
        self.master
            .lock()
            .resize(pty_size_from_surface(&size))
            .context("failed to resize linux pty")?;
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
        self.master
            .lock()
            .resize(pty_size_from_surface(&size))
            .context("failed to resize linux pty")?;
        self.mark_needs_render();
        Ok(())
    }

    /// Stamp the shared `needs_render` flag and wake the workspace
    /// loop so the next pump tick re-fetches a fresh snapshot. Used
    /// after resize / theme / mode changes that mutate the parser
    /// state without going through a PTY-output path. Without this,
    /// idle-shell resizes silently keep painting the previous grid
    /// dimensions until new shell output arrives.
    fn mark_needs_render(&self) {
        self.shared.needs_render.store(true, Ordering::Release);
        self.shared.wake();
    }

    pub fn write_input(&self, data: &[u8]) -> Result<()> {
        self.scroll_viewport_to_bottom();
        self.writer
            .lock()
            .write_all(data)
            .context("failed to write to linux pty")?;
        self.input_generation.fetch_add(1, Ordering::Relaxed);
        Ok(())
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

    pub fn title(&self) -> Option<String> {
        self.title.clone()
    }

    pub fn current_dir(&self) -> Option<String> {
        self.shared
            .screen
            .current_dir()
            .or_else(|| self.current_dir.clone())
    }

    pub fn is_alive(&self) -> bool {
        self.poll_child_status();
        self.shared.alive.load(Ordering::Acquire)
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
    /// fresh `ScreenSnapshot`. Used by the GPUI-owned Linux paint
    /// path to access per-cell fg/bg/attrs alongside the codepoint.
    pub fn snapshot(&self) -> ScreenSnapshot {
        self.shared.screen.snapshot()
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

    pub fn is_bracketed_paste(&self) -> bool {
        self.shared.screen.is_bracketed_paste()
    }

    pub fn is_decckm(&self) -> bool {
        self.shared.screen.is_decckm()
    }

    /// True when the child app has enabled terminal mouse reporting
    /// (DECSET 1000/1002/1003). View mouse handlers gate SGR reports on
    /// this so clicks don't leak escape sequences into shells that
    /// didn't ask for them.
    pub fn mouse_tracking_active(&self) -> bool {
        self.shared.screen.mouse_tracking_active()
    }

    pub fn set_dark_mode(&self, dark: bool) {
        self.shared.screen.set_dark_mode(dark);
        // Same shape as `set_theme` / `resize`: a parser-state mutation
        // that doesn't go through a PTY-output path. Without
        // `mark_needs_render` the next pump tick won't re-fetch a
        // snapshot and Linux panes can keep painting the previous
        // mode-derived colors until new shell output arrives.
        self.mark_needs_render();
    }

    fn poll_child_status(&self) {
        if !self.shared.alive.load(Ordering::Acquire) {
            return;
        }

        let Ok(Some(status)) = self.child.lock().try_wait() else {
            return;
        };

        let exit_code = i32::try_from(status.exit_code()).unwrap_or(i32::MAX);
        self.shared
            .mark_exited(Some(exit_code), self.started_at.elapsed());
    }
}

impl Drop for LinuxPtySession {
    fn drop(&mut self) {
        if let Err(err) = self.child.lock().kill() {
            log::debug!("failed to terminate linux pty child during drop: {err}");
        }
        self.shared.alive.store(false, Ordering::Release);
        self.shared.needs_render.store(true, Ordering::Release);
        self.shared.wake();
    }
}

fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    shared: Arc<SessionShared>,
    started_at: Instant,
) {
    std::thread::Builder::new()
        .name("con-linux-pty-reader".into())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        shared.mark_exited(None, started_at.elapsed());
                        break;
                    }
                    Ok(read) => {
                        shared.screen.feed(&buffer[..read]);
                        let chunk = String::from_utf8_lossy(&buffer[..read]);
                        shared.push_output(chunk.as_ref());
                    }
                    Err(err) if err.kind() == ErrorKind::Interrupted => continue,
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

fn default_title(cwd: Option<&Path>, program: Option<&str>) -> String {
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

fn configure_shell_startup(program: &str, command: &mut CommandBuilder) {
    let Some(shell) = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return;
    };

    match shell {
        "fish" => {
            command.arg("--interactive");
        }
        "pwsh" => {
            command.arg("-NoLogo");
        }
        "sh" | "dash" | "ksh" | "mksh" | "xonsh" | "nu" => {
            if let Some(flag) = interactive_shell_flag(program) {
                command.arg(flag);
            }
        }
        _ => {
            if let Some(flag) = interactive_shell_flag(program) {
                command.arg(flag);
            }
        }
    }
}

fn interactive_shell_flag(program: &str) -> Option<&'static str> {
    let shell = Path::new(program).file_name()?.to_str()?;
    match shell {
        "bash" | "sh" | "zsh" | "dash" | "ksh" | "mksh" | "nu" | "xonsh" => Some("-i"),
        _ => None,
    }
}

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
    use std::sync::Arc;
    use std::time::Duration;

    use crate::transcript::{TranscriptBuffer, sanitize_terminal_output};
    use crate::vt::VtScreen;

    use super::SessionShared;

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
        let finished = shared
            .finished_signal
            .lock()
            .expect("exit signal should be backfilled");
        assert_eq!(finished.exit_code, Some(7));
        assert_eq!(finished.duration, Duration::from_millis(20));
    }
}
