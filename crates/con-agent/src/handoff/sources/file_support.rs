//! Bounded, read-only storage helpers shared by the file-backed adapters.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::super::{
    HistoryExport, HistoryRecord, MAX_EXPORT_BYTES, SourceSession, digest, sanitize,
};

pub(super) const MAX_SESSIONS: usize = 2_000;
pub(super) const METADATA_BYTES: usize = 1024 * 1024;

/// Environment override or a home-relative default; `missing` is the caller's
/// own error wording when neither is available.
pub(super) fn env_home(variable: &str, default: &str, missing: &'static str) -> Result<PathBuf> {
    Ok(std::env::var_os(variable)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or(dirs::home_dir().context(missing)?.join(default)))
}

pub(super) fn home(env: &str, fallback: &str) -> Result<PathBuf> {
    let path = env_home(env, fallback, "Agent storage home is unavailable")?;
    if let Ok(rest) = path.strip_prefix("~") {
        return Ok(dirs::home_dir()
            .context("Home directory unavailable")?
            .join(rest));
    }
    Ok(path)
}

/// Store-identity digest over an already canonicalized root; the caller's
/// prefix keeps each product's existing identity format.
pub(super) fn store_identity(prefix: &str, canonical_root: &Path) -> String {
    digest(
        format!(
            "{prefix}{}:{}",
            gethostname::gethostname().to_string_lossy(),
            canonical_root.display()
        )
        .as_bytes(),
    )
}

pub(super) fn identity(root: &Path) -> Result<String> {
    Ok(store_identity("", &root.canonicalize()?))
}

pub(super) fn children(path: &Path, directories: bool) -> Result<Vec<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "Session directory must not be a symbolic link"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        if (directories && kind.is_dir()) || (!directories && kind.is_file()) {
            paths.push(entry.path());
            ensure!(
                paths.len() <= MAX_SESSIONS,
                "Agent storage contains too many entries"
            );
        }
    }
    paths.sort();
    Ok(paths)
}

/// Read up to `limit` bytes (plus one overflow sentinel) from an already
/// validated file; callers keep their own overflow checks and error wording.
pub(super) fn bounded_read(file: &fs::File, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = open_regular(path)?;
    let before = file.metadata()?;
    ensure!(
        before.len() <= limit as u64,
        "History exceeds the supported size limit"
    );
    let bytes = bounded_read(&file, limit)?;
    let after = file.metadata()?;
    ensure!(
        bytes.len() <= limit
            && before.len() == after.len()
            && before.modified()? == after.modified()?,
        "History changed during export; stop the source and retry"
    );
    Ok(bytes)
}

pub(super) fn check_dir(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "Session directory must not be a symbolic link"
    );
    Ok(())
}

fn open_regular(path: &Path) -> Result<fs::File> {
    check_dir(path.parent().context("History has no parent directory")?)?;
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink(),
        "History must be a regular file"
    );
    let file = fs::File::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        ensure!(
            meta.ino() == opened.ino() && meta.dev() == opened.dev() && opened.nlink() == 1,
            "History file was replaced or hard-linked"
        );
    }
    Ok(file)
}

pub(super) fn json(path: &Path, limit: usize) -> Result<Value> {
    serde_json::from_slice(&read(path, limit)?).context("Unsupported session JSON")
}

pub(super) fn jsonl(bytes: &[u8]) -> Result<Vec<Value>> {
    let text = std::str::from_utf8(bytes).context("History is not UTF-8")?;
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        out.push(
            serde_json::from_str(line)
                .context("History contains an incomplete or invalid JSONL record")?,
        );
        ensure!(out.len() <= 100_000, "History contains too many records");
    }
    Ok(out)
}

pub(super) fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("Session is missing {key}"))
}

/// Pick the unique stored entry with an exact session-ID match. `Ok(None)`
/// when the ID is absent; a duplicated ID is always refused.
pub(super) fn select_exact<T>(
    entries: impl IntoIterator<Item = T>,
    id: &str,
    entry_id: impl Fn(&T) -> &str,
    ambiguous: &'static str,
) -> Result<Option<T>> {
    let mut matching = entries.into_iter().filter(|entry| entry_id(entry) == id);
    let Some(selected) = matching.next() else {
        return Ok(None);
    };
    ensure!(matching.next().is_none(), "{}", ambiguous);
    Ok(Some(selected))
}

pub(super) fn valid_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control),
        "Select an exact native session ID"
    );
    Ok(())
}

/// RFC 3339 string timestamps in whole seconds. Numeric values and their
/// units stay with each caller's own policy.
pub(super) fn rfc3339_seconds(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp())
}

pub(super) fn timestamp(value: &Value) -> i64 {
    rfc3339_seconds(value)
        .or_else(|| {
            value
                .as_i64()
                .map(|n| if n > 100_000_000_000 { n / 1000 } else { n })
        })
        .unwrap_or_default()
}

pub(super) fn title(value: &str) -> String {
    sanitize(value).chars().take(160).collect()
}

/// Only explicitly textual blocks; thinking, signatures and opaque tool arguments never pass through.
pub(super) fn text(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_owned();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| {
            if part["thought"].as_bool() == Some(true) {
                return None;
            }
            match part["type"].as_str() {
                None | Some("text") => part["text"].as_str(),
                _ => None,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Shared export finalization: sanitize, require a user goal, bound the
/// encoded records, then append the caller's own trailing omissions. The
/// size-limit wording and trailing omissions stay product-specific.
pub(super) fn finish_records(
    source: SourceSession,
    version: String,
    last: String,
    mut records: Vec<HistoryRecord>,
    mut omissions: Vec<String>,
    oversized: &'static str,
    trailing: [&'static str; 2],
) -> Result<HistoryExport> {
    for record in &mut records {
        record.text = sanitize(&record.text);
    }
    records.retain(|r| !r.text.trim().is_empty());
    ensure!(
        records.iter().any(|r| r.role == "user"),
        "History has no exportable user goal"
    );
    let bytes = serde_json::to_vec(&records)?;
    ensure!(bytes.len() <= MAX_EXPORT_BYTES, "{}", oversized);
    omissions.push(trailing[0].into());
    omissions.push(trailing[1].into());
    Ok(HistoryExport {
        source,
        agent_version: version,
        last_turn_id: last,
        records,
        omissions,
        digest: digest(&bytes),
    })
}

pub(super) fn finish(
    source: SourceSession,
    version: String,
    last: String,
    records: Vec<HistoryRecord>,
    omissions: Vec<String>,
) -> Result<HistoryExport> {
    finish_records(
        source,
        version,
        last,
        records,
        omissions,
        "Filtered history exceeds the supported size limit",
        [
            "Only persisted text is included. Live activity is unknown; stop the source and its background commands before handoff.",
            "Hidden reasoning, credentials, non-text inputs, opaque tool arguments and permissions are excluded; potential credential lines are redacted.",
        ],
    )
}
