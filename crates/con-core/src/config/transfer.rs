//! Self-contained, no-clobber configuration transfer helpers used by the CLI.

use super::Config;
use anyhow::{Context, Result, bail};
use con_terminal::TerminalTheme;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 16;
const MAX_FILES: usize = 128;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Default)]
struct TransferState {
    files: usize,
    bytes: u64,
    output_bytes: u64,
    names: HashMap<PathBuf, PathBuf>,
    visiting: HashSet<PathBuf>,
    staged: Vec<(PathBuf, Vec<u8>)>,
    strip_comments: bool,
}

fn assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, raw) = line.split_once('=')?;
    let value = raw.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    Some((key.trim(), value))
}

fn expand_home(value: &str) -> Result<PathBuf> {
    if value == "~" || value.starts_with("~/") || value.starts_with("~\\") {
        let home = dirs::home_dir().context("cannot expand `~`: home directory is unavailable")?;
        return Ok(if value.len() == 1 {
            home
        } else {
            home.join(&value[2..])
        });
    }
    Ok(PathBuf::from(value))
}

fn regular(path: &Path, description: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("cannot read {description} `{}`", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!(
            "{description} `{}` must be a regular file (symlinks are not accepted)",
            path.display()
        );
    }
    Ok(())
}

fn resolve(value: &str, parent: &Path, required: bool) -> Result<Option<PathBuf>> {
    let raw = expand_home(value)?;
    let candidate = if raw.is_absolute() {
        raw.clone()
    } else {
        parent.join(&raw)
    };
    if candidate.exists() {
        regular(&candidate, "referenced resource")?;
        return Ok(Some(candidate.canonicalize()?));
    }
    if required || raw.is_absolute() || value.contains('/') || value.contains('\\') {
        bail!(
            "referenced resource `{value}` does not exist relative to `{}`",
            parent.display()
        );
    }
    Ok(None)
}

fn resolve_optional(value: &str, parent: &Path) -> Result<Option<PathBuf>> {
    let raw = expand_home(value)?;
    let candidate = if raw.is_absolute() {
        raw
    } else {
        parent.join(raw)
    };
    if !candidate.exists() {
        return Ok(None);
    }
    regular(&candidate, "optional configuration file")?;
    Ok(Some(candidate.canonicalize()?))
}

fn ghostty_xdg_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .map(|root| root.join("ghostty"))
}

fn resolve_theme(value: &str, parent: &Path, include_con: bool) -> Result<Option<PathBuf>> {
    if let Some(path) = resolve(value, parent, false)? {
        return Ok(Some(path));
    }
    let mut roots = Vec::new();
    if include_con {
        roots.push(con_paths::user_themes_dir());
    }
    if let Some(config) = ghostty_xdg_dir() {
        roots.push(config.join("themes"));
    }
    for root in roots {
        let path = root.join(value);
        if path.exists() {
            regular(&path, "Ghostty theme")?;
            return Ok(Some(path.canonicalize()?));
        }
    }
    Ok(None) // Built-in Ghostty theme name.
}

fn account(state: &mut TransferState, bytes: u64) -> Result<()> {
    state.files += 1;
    state.bytes = state
        .bytes
        .checked_add(bytes)
        .context("transfer size overflow")?;
    if state.files > MAX_FILES || state.bytes > MAX_BYTES {
        bail!(
            "configuration dependency graph exceeds the transfer limit ({MAX_FILES} files or {MAX_BYTES} bytes)"
        );
    }
    Ok(())
}

fn account_output(state: &mut TransferState, bytes: usize) -> Result<()> {
    state.output_bytes = state.output_bytes.saturating_add(bytes as u64);
    if state.output_bytes > MAX_BYTES {
        bail!("rewritten configuration exceeds the {MAX_BYTES} byte transfer limit");
    }
    Ok(())
}

fn read_bounded(path: &Path, state: &mut TransferState) -> Result<Vec<u8>> {
    regular(path, "configuration resource")?;
    let len = fs::metadata(path)?.len();
    if len > MAX_BYTES || state.bytes.saturating_add(len) > MAX_BYTES {
        bail!("configuration dependency graph exceeds the {MAX_BYTES} byte transfer limit");
    }
    account(state, len)?;
    let mut bytes = Vec::with_capacity(len as usize);
    File::open(path)?.take(len + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        bail!(
            "resource `{}` changed while it was being read",
            path.display()
        );
    }
    Ok(bytes)
}

fn reserve(state: &mut TransferState, source: &Path, extension: &str) -> PathBuf {
    if let Some(path) = state.names.get(source) {
        return path.clone();
    }
    let path = PathBuf::from(format!("resource-{:04}{extension}", state.names.len() + 1));
    state.names.insert(source.to_owned(), path.clone());
    path
}

fn copy_binary(source: &Path, state: &mut TransferState) -> Result<PathBuf> {
    if let Some(path) = state.names.get(source) {
        return Ok(path.clone());
    }
    let bytes = read_bounded(source, state)?;
    let extension = source
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{s}"))
        .unwrap_or_default();
    let target = reserve(state, source, &extension);
    account_output(state, bytes.len())?;
    state.staged.push((target.clone(), bytes));
    Ok(target)
}

fn transform_theme_value(
    value: &str,
    parent: &Path,
    resources: &Path,
    depth: usize,
    state: &mut TransferState,
) -> Result<Option<String>> {
    let conditional = value.contains("light:") || value.contains("dark:");
    let mut changed = false;
    let mut output = String::new();
    let mut append = |part: &str| -> Result<()> {
        let separator = usize::from(!output.is_empty());
        if output
            .len()
            .saturating_add(part.len())
            .saturating_add(separator)
            > MAX_BYTES as usize
        {
            bail!("rewritten theme exceeds the {MAX_BYTES} byte transfer limit");
        }
        if separator != 0 {
            output.push(',');
        }
        output.push_str(part);
        Ok(())
    };
    for part in value.split(',') {
        let trimmed = part.trim();
        let (prefix, name) = if conditional {
            trimmed.split_once(':').with_context(|| {
                format!("unsupported conditional theme `{value}`; expected `light:name,dark:name`")
            })?
        } else {
            ("", trimmed)
        };
        let source = resolve_theme(name, parent, state.strip_comments)?;
        if source.is_none()
            && state.strip_comments
            && let Some(theme) = TerminalTheme::legacy_builtin(name)
        {
            let text = theme.to_ghostty_format();
            let identity = PathBuf::from(format!("con-builtin-theme:{name}"));
            let relative = reserve(state, &identity, ".ghostty");
            if !state.staged.iter().any(|(path, _)| path == &relative) {
                account(state, text.len() as u64)?;
                account_output(state, text.len())?;
                state.staged.push((relative.clone(), text.into_bytes()));
            }
            let mapped = resources.join(relative).to_string_lossy().into_owned();
            append(&if prefix.is_empty() {
                mapped
            } else {
                format!("{prefix}:{mapped}")
            })?;
            changed = true;
            continue;
        }
        let Some(source) = source else {
            append(trimmed)?;
            continue;
        };
        let relative = reserve(state, &source, ".ghostty");
        let text = transform(&source, resources, depth + 1, true, state)?;
        if !state.staged.iter().any(|(path, _)| path == &relative) {
            state.staged.push((relative.clone(), text.into_bytes()));
        }
        let mapped = resources.join(relative).to_string_lossy().into_owned();
        append(&if prefix.is_empty() {
            mapped
        } else {
            format!("{prefix}:{mapped}")
        })?;
        changed = true;
    }
    Ok(changed.then_some(output))
}

fn transform(
    source: &Path,
    resources: &Path,
    depth: usize,
    included: bool,
    state: &mut TransferState,
) -> Result<String> {
    if depth > MAX_DEPTH {
        bail!(
            "config-file include depth exceeds {MAX_DEPTH} at `{}`",
            source.display()
        );
    }
    regular(source, "configuration file")?; // Reject a symlink before canonicalization.
    let source = source.canonicalize()?;
    if !state.visiting.insert(source.clone()) {
        bail!("config-file include cycle involving `{}`", source.display());
    }
    let bytes = read_bounded(&source, state)?;
    let text = String::from_utf8(bytes)
        .with_context(|| format!("configuration `{}` is not valid UTF-8", source.display()))?;
    let parent = source.parent().unwrap_or(Path::new("."));
    let mut output = String::new();
    for (index, line) in text.lines().enumerate() {
        if state.strip_comments && line.trim_start().starts_with('#') {
            continue;
        }
        if let Some((key, value)) = assignment(line) {
            if key.starts_with("con.") {
                if included && !state.strip_comments {
                    bail!(
                        "included file `{}` line {} contains `{key}`",
                        source.display(),
                        index + 1
                    );
                }
                continue;
            }
            let mapped = match key {
                "config-file" | "theme" | "background-image" | "custom-shader"
                | "gtk-custom-css"
                    if value.is_empty() =>
                {
                    None
                }
                "config-file" => {
                    let (optional, value) = value
                        .strip_prefix('?')
                        .map_or((false, value), |v| (true, v.trim_start()));
                    let child = if optional {
                        resolve_optional(value, parent)?
                    } else {
                        resolve(value, parent, true)?
                    };
                    match child {
                        Some(child) => {
                            let relative = reserve(state, &child, ".ghostty");
                            let transformed = transform(&child, resources, depth + 1, true, state)?;
                            if !state.staged.iter().any(|(p, _)| p == &relative) {
                                state
                                    .staged
                                    .push((relative.clone(), transformed.into_bytes()));
                            }
                            Some(format!(
                                "{}{}",
                                if optional { "?" } else { "" },
                                resources.join(relative).display()
                            ))
                        }
                        // An absent optional include is absent in this snapshot;
                        // do not retain a link back to a future source file.
                        None => continue,
                    }
                }
                "background-image" | "custom-shader" | "gtk-custom-css" => {
                    resolve(value, parent, true)?
                        .map(|p| copy_binary(&p, state))
                        .transpose()?
                        .map(|p| resources.join(p).display().to_string())
                }
                "theme" => transform_theme_value(value, parent, resources, depth, state)?,
                _ => None,
            };
            if let Some(value) = mapped {
                account_output(state, key.len() + value.len() + 4)?;
                output.push_str(key);
                output.push_str(" = ");
                output.push_str(&value);
                output.push('\n');
                continue;
            }
        }
        account_output(state, line.len() + 1)?;
        output.push_str(line);
        output.push('\n');
    }
    state.visiting.remove(&source);
    Ok(output)
}

fn private_write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temporary = parent.join(format!(".con-transfer-{}-{token}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::hard_link(&temporary, path).with_context(|| {
            format!(
                "destination `{}` already exists or cannot be created",
                path.display()
            )
        })?;
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn transfer(from: &Path, to: &Path, strip_comments: bool) -> Result<()> {
    if to.exists() {
        bail!("destination `{}` already exists", to.display());
    }
    let requested_parent = to
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(requested_parent)?;
    let parent = requested_parent
        .canonicalize()
        .context("cannot canonicalize destination parent")?;
    let file_name = to.file_name().context("destination must name a file")?;
    let destination = parent.join(file_name);
    let token = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let base = file_name.to_string_lossy();
    let resources = parent.join(format!("{base}.resources-{token}"));
    let mut state = TransferState {
        strip_comments,
        ..TransferState::default()
    };
    let rendered = transform(from, &resources, 0, false, &mut state)?;
    Config::parse_ghostty(&rendered).context("transferred configuration failed Con validation")?;
    let created_resources = if state.staged.is_empty() {
        false
    } else {
        fs::create_dir(&resources)?;
        let result = state
            .staged
            .iter()
            .try_for_each(|(relative, bytes)| private_write_new(&resources.join(relative), bytes));
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&resources);
            return Err(error);
        }
        true
    };
    if let Err(error) = private_write_new(&destination, rendered.as_bytes()) {
        if created_resources {
            let _ = fs::remove_dir_all(&resources);
        }
        return Err(error);
    }
    Ok(())
}

/// Import a Ghostty configuration and its bounded file dependencies without overwriting anything.
pub fn import_ghostty(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    transfer(from.as_ref(), to.as_ref(), false)
}

/// Export an authored, self-contained Ghostty snapshot, recursively excluding comments and Con settings.
pub fn export_ghostty(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    transfer(from.as_ref(), to.as_ref(), true)
}

/// Existing default sources, in Ghostty's load order (XDG before App Support,
/// legacy `config` before `config.ghostty`). This lists sources for explicit
/// selection; it does not merge Ghostty's layered default configuration.
pub fn ghostty_config_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(xdg) = ghostty_xdg_dir() {
        roots.push(xdg);
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Library/Application Support/com.mitchellh.ghostty"));
    }
    roots
        .into_iter()
        .flat_map(|root| [root.join("config"), root.join("config.ghostty")])
        .filter(|path| path.is_file())
        .collect()
}

/// A private snapshot awaiting user confirmation. Dropping it before commit
/// removes only the staging directory created by this operation.
pub struct PreparedImport {
    directory: PathBuf,
    destination: PathBuf,
    previous: Option<String>,
    rendered: String,
    candidate: Config,
    committed: bool,
}

impl PreparedImport {
    pub fn config(&self) -> &Config {
        &self.candidate
    }

    /// First-run import must never replace even an empty existing file.
    pub fn commit_new(self) -> Result<()> {
        if self.previous.is_some() {
            bail!("Con configuration already exists; use Settings to replace it");
        }
        self.commit().map(|_| ())
    }

    pub fn commit(mut self) -> Result<Option<PathBuf>> {
        let actual = match fs::read_to_string(&self.destination) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if actual != self.previous {
            bail!("Con configuration changed on disk; cancel and prepare the import again");
        }
        let backup = if let Some(previous) = &self.previous {
            let backup = self.directory.join("previous.ghostty");
            private_write_new(&backup, previous.as_bytes())?;
            Some(backup)
        } else {
            None
        };
        if self.previous.is_some() {
            super::write_private_atomic(&self.destination, self.rendered.as_bytes())?;
        } else {
            private_write_new(&self.destination, self.rendered.as_bytes())?;
        }
        self.committed = true;
        Ok(backup)
    }
}

impl Drop for PreparedImport {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}

/// Prepare a detached native snapshot while retaining Con-owned settings.
/// The caller must validate with its native backend before offering commit.
pub fn prepare_import(from: &Path, current: &Config, destination: &Path) -> Result<PreparedImport> {
    let previous = match fs::read_to_string(destination) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    {
        let source = current.source.lock().expect("config source mutex poisoned");
        if source.path.as_deref() != Some(destination)
            || previous.as_deref().unwrap_or("") != source.source
        {
            bail!("Con configuration changed on disk; restart Con before importing");
        }
    }
    let parent = destination
        .parent()
        .context("configuration has no parent directory")?;
    fs::create_dir_all(parent)?;
    let directory = parent.join(format!(
        "import-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&directory)?;
    let mut prepared = PreparedImport {
        directory,
        destination: destination.to_owned(),
        previous,
        rendered: String::new(),
        candidate: current.clone(),
        committed: false,
    };
    let snapshot = prepared.directory.join("snapshot.ghostty");
    import_ghostty(from, &snapshot)?;
    let mut rendered = fs::read_to_string(&snapshot)?;
    fs::remove_file(&snapshot)?;
    rendered.push('\n');
    // Use the existing lossless renderer, not a second serializer. Only Con
    // assignments survive; all old native entries are replaced as one unit.
    for line in super::ghostty::render(current)?.lines() {
        if assignment(line).is_some_and(|(key, _)| key.starts_with("con.")) {
            rendered.push_str(line);
            rendered.push('\n');
        }
    }
    if rendered.len() as u64 > MAX_BYTES {
        bail!("merged configuration exceeds the {MAX_BYTES} byte transfer limit");
    }
    prepared.candidate = super::ghostty::parse(&rendered, Some(destination.to_owned()))?;
    prepared.rendered = rendered;
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn dir() -> PathBuf {
        loop {
            let p = std::env::temp_dir().join(format!(
                "con-transfer-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            if fs::create_dir(&p).is_ok() {
                return p;
            }
        }
    }

    #[test]
    fn settings_import_replaces_native_preserves_con_and_backs_up_exact_bytes() {
        let d = dir();
        let target = d.join("config.ghostty");
        let original = "# keep backup\nfont-size = 12\nbackground = 123456\ncon.agent.max_turns = 9\ncon.skills.project_paths = private\n";
        fs::write(&target, original).unwrap();
        let config = Config::load_from_path(&target).unwrap();
        let source = d.join("ghostty");
        let imported = "font-size = 19\nforeground = abcdef\ncon.agent.max_turns = 1\n";
        fs::write(&source, imported).unwrap();
        let prepared = prepare_import(&source, &config, &target).unwrap();
        assert_eq!(prepared.config().terminal.font_size, 19.0);
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
        let backup = prepared.commit().unwrap().unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(fs::read_to_string(&source).unwrap(), imported);
        let result = Config::load_from_path(&target).unwrap();
        assert_eq!(result.agent.max_turns, 9);
        assert_eq!(result.skills.project_paths, ["private"]);
        assert_eq!(result.terminal.font_size, 19.0);
        assert!(!fs::read_to_string(&target).unwrap().contains("123456"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(backup).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn first_run_import_never_replaces_an_existing_empty_file() {
        let d = dir();
        let target = d.join(con_paths::CONFIG_FILE_NAME);
        let source = d.join("ghostty");
        fs::write(&source, "font-size = 19\n").unwrap();
        let config = Config::load_from_path(&target).unwrap();
        // Even an empty file appearing between eligibility and preparation
        // belongs to its creator, not to the first-run importer.
        fs::write(&target, "").unwrap();
        let prepared = prepare_import(&source, &config, &target).unwrap();
        let staging = prepared.directory.clone();
        assert!(prepared.commit_new().is_err());
        assert!(!staging.exists());
        assert_eq!(fs::read_to_string(&target).unwrap(), "");
        fs::remove_file(&target).unwrap();
        let prepared = prepare_import(&source, &config, &target).unwrap();
        fs::write(&target, "font-size = 23\n").unwrap();
        assert!(prepared.commit_new().is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "font-size = 23\n");
        fs::remove_file(&target).unwrap();
        prepare_import(&source, &config, &target)
            .unwrap()
            .commit_new()
            .unwrap();
        assert_eq!(
            Config::load_from_path(&target).unwrap().terminal.font_size,
            19.0
        );
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn settings_import_detects_external_changes_before_and_after_preparation() {
        let d = dir();
        let target = d.join("config.ghostty");
        let source = d.join("ghostty");
        fs::write(&target, "font-size = 12\n").unwrap();
        fs::write(&source, "font-size = 19\n").unwrap();
        let config = Config::load_from_path(&target).unwrap();
        let prepared = prepare_import(&source, &config, &target).unwrap();
        let staging = prepared.directory.clone();
        fs::write(&target, "font-size = 23\n").unwrap();
        assert!(prepared.commit().is_err());
        assert!(!staging.exists());
        assert!(prepare_import(&source, &config, &target).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "font-size = 23\n");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn settings_import_cancel_cleans_resources_and_first_import_needs_no_backup() {
        let d = dir();
        let target = d.join("config.ghostty");
        let source = d.join("ghostty");
        fs::write(d.join("colors"), "background = 13579b\n").unwrap();
        fs::write(&source, "config-file = colors\n").unwrap();
        let config = Config::load_from_path(&target).unwrap();
        let prepared = prepare_import(&source, &config, &target).unwrap();
        let staging = prepared.directory.clone();
        drop(prepared);
        assert!(!staging.exists());
        assert!(!target.exists());
        let prepared = prepare_import(&source, &config, &target).unwrap();
        assert!(prepared.commit().unwrap().is_none());
        fs::remove_file(d.join("colors")).unwrap();
        let text = fs::read_to_string(&target).unwrap();
        let included = text
            .lines()
            .filter_map(assignment)
            .find(|(key, _)| *key == "config-file")
            .unwrap()
            .1;
        assert_eq!(
            fs::read_to_string(included).unwrap(),
            "background = 13579b\n"
        );
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn expanded_cached_references_respect_output_budget() {
        let d = dir();
        let source = d.join("source");
        fs::write(d.join("image"), b"image").unwrap();
        fs::write(&source, "background-image = image\n".repeat(100_000)).unwrap();
        let resources = d.join("long-destination-".repeat(20));
        let mut state = TransferState::default();
        let error = transform(&source, &resources, 0, false, &mut state).unwrap_err();
        assert!(error.to_string().contains("rewritten configuration"));
        assert!(state.bytes < MAX_BYTES);
        assert_eq!(state.staged.len(), 1);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn relative_destination_is_canonical_and_snapshot_survives_source_removal() {
        let d = dir();
        let source = d.join("source");
        fs::write(d.join("image"), b"image").unwrap();
        fs::write(&source, "background-image = image\n").unwrap();
        let target = PathBuf::from(format!(
            "con-transfer-relative-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        import_ghostty(&source, &target).unwrap();
        fs::remove_file(source).unwrap();
        fs::remove_file(d.join("image")).unwrap();
        let out = fs::read_to_string(&target).unwrap();
        let path = assignment(&out).unwrap().1;
        assert!(Path::new(path).is_file());
        fs::remove_dir_all(Path::new(path).parent().unwrap()).unwrap();
        fs::remove_file(target).unwrap();
    }

    #[test]
    fn missing_optional_includes_are_detached_and_native_resets_are_preserved() {
        let d = dir();
        let source = d.join("source");
        fs::write(
            &source,
            "config-file = ?~/certainly-missing-con-file\nconfig-file =\ntheme =\nbackground-image =\ncustom-shader =\n",
        )
        .unwrap();
        let target = d.join("target");
        import_ghostty(&source, &target).unwrap();
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            "config-file =\ntheme =\nbackground-image =\ncustom-shader =\n"
        );
    }

    #[test]
    fn failure_and_collision_publish_nothing_and_source_is_unchanged() {
        let d = dir();
        let source = d.join("source");
        let original = "config-file = child\n";
        fs::write(&source, original).unwrap();
        fs::write(d.join("child"), "con.secret = value\n").unwrap();
        let target = d.join("target");
        assert!(import_ghostty(&source, &target).is_err());
        assert!(!target.exists());
        assert!(!fs::read_dir(&d).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("resources-")
        }));
        fs::write(&target, "keep").unwrap();
        assert!(import_ghostty(&source, &target).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "keep");
        assert_eq!(fs::read_to_string(source).unwrap(), original);
    }

    #[test]
    fn export_filters_comments_and_recursive_con_settings() {
        let d = dir();
        let source = d.join("source");
        let image = d.join("image");
        fs::write(&image, b"image").unwrap();
        fs::write(&source, "# root secret\nconfig-file = child\n").unwrap();
        fs::write(
            d.join("child"),
            "# nested secret\nfont-size = 12\ncon.agent.max_turns = 7\nbackground-image = image\n",
        )
        .unwrap();
        let target = d.join("target");
        export_ghostty(&source, &target).unwrap();
        fs::remove_file(source).unwrap();
        fs::remove_file(image).unwrap();
        let exported = fs::read_to_string(target).unwrap();
        assert!(!exported.contains("root secret"));
        let child = exported
            .lines()
            .filter_map(assignment)
            .find(|(key, _)| *key == "config-file")
            .unwrap()
            .1;
        let child = fs::read_to_string(child).unwrap();
        assert!(!child.contains("nested secret"));
        assert!(!child.contains("con.agent"));
        let image = child
            .lines()
            .filter_map(assignment)
            .find(|(key, _)| *key == "background-image")
            .unwrap()
            .1;
        assert!(Path::new(image).is_file());
    }

    #[test]
    fn export_stages_legacy_themes_but_preserves_native_names_and_imports() {
        let d = dir();
        let source = d.join("source");
        fs::write(&source, "theme = light:flexoki-light,dark:Dracula\n").unwrap();

        let exported = d.join("exported");
        export_ghostty(&source, &exported).unwrap();
        let output = fs::read_to_string(exported).unwrap();
        let value = assignment(&output).unwrap().1;
        let (light, dark) = value.split_once(',').unwrap();
        let light = light.strip_prefix("light:").unwrap();
        assert!(Path::new(light).is_file());
        assert!(fs::read_to_string(light).unwrap().contains("palette = 15="));
        assert_eq!(dark, "dark:Dracula");

        let imported = d.join("imported");
        import_ghostty(&source, &imported).unwrap();
        assert_eq!(
            fs::read_to_string(imported).unwrap(),
            fs::read_to_string(source).unwrap()
        );
    }
}
