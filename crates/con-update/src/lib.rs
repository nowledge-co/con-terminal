//! Signed portable updates. No scripts, package-manager writes, or process killing.
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

mod transaction;
pub use transaction::{run_helper, spawn_helper};

pub const PUBLIC_KEY: &str = match option_env!("CON_SPARKLE_PUBLIC_ED_KEY") {
    Some(key) => key,
    None => "",
};
pub const BUILD_VERSION: &str = match option_env!("CON_RELEASE_VERSION") {
    Some(version) => version,
    None => "0.1.0",
};
pub const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;
const SPARKLE: &str = "http://www.andymatuschak.org/xml-namespaces/sparkle";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub signature: String,
    pub length: u64,
    pub notes: String,
}

/// Parse fields from the same item, never combine a version with another item's URL.
pub fn parse_feed(xml: &str, platform: &str, arch: &str) -> Result<Release> {
    ensure!(xml.len() <= 2 * 1024 * 1024, "Update feed is too large");
    let document = roxmltree::Document::parse(xml)?;
    let channel = document
        .root_element()
        .children()
        .find(|n| n.has_tag_name("channel"))
        .context("Missing update channel")?;
    let item = channel
        .children()
        .find(|n| n.has_tag_name("item"))
        .context("Missing update item")?;
    let text = |name| {
        item.children()
            .find(|n| n.has_tag_name((SPARKLE, name)))
            .and_then(|n| n.text())
    };
    let version = text("shortVersionString")
        .context("Missing update version")?
        .trim()
        .to_owned();
    semver::Version::parse(version.trim_start_matches('v'))?;
    let enclosure = item
        .children()
        .find(|n| n.has_tag_name("enclosure"))
        .context("Missing update archive")?;
    let url = enclosure
        .attribute("url")
        .context("Missing archive URL")?
        .to_owned();
    let parsed = url::Url::parse(&url)?;
    ensure!(
        parsed.scheme() == "https" && parsed.username().is_empty() && parsed.password().is_none(),
        "Update archive requires HTTPS"
    );
    let suffix = match platform {
        "windows" => format!("-windows-{arch}.zip"),
        "linux" => format!("-linux-{arch}.tar.gz"),
        _ => bail!("Unsupported portable update platform"),
    };
    ensure!(
        parsed.path().ends_with(&suffix),
        "Archive platform does not match this installation"
    );
    let signature = enclosure
        .attribute((SPARKLE, "edSignature"))
        .context("Missing update signature")?
        .to_owned();
    ensure!(
        STANDARD.decode(&signature)?.len() == 64,
        "Invalid update signature"
    );
    let length: u64 = enclosure
        .attribute("length")
        .context("Missing archive length")?
        .parse()?;
    ensure!(
        length > 0 && length <= MAX_ARCHIVE,
        "Invalid archive length"
    );
    let notes = item
        .children()
        .find(|n| n.has_tag_name("description"))
        .and_then(|n| n.text())
        .unwrap_or_default()
        .to_owned();
    Ok(Release {
        version,
        url,
        signature,
        length,
        notes,
    })
}

pub fn newer(candidate: &str, running: &str) -> bool {
    match (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(running.trim_start_matches('v')),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}

pub fn verify(bytes: &[u8], release: &Release, key: &str) -> Result<()> {
    ensure!(release.length <= MAX_ARCHIVE, "Update archive is too large");
    ensure!(
        bytes.len() as u64 == release.length,
        "Incomplete update download"
    );
    let key: [u8; 32] = STANDARD
        .decode(key.trim())
        .context("Update signing key is unavailable")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid update signing key"))?;
    let signature = Signature::from_slice(&STANDARD.decode(&release.signature)?)?;
    VerifyingKey::from_bytes(&key)?
        .verify_strict(bytes, &signature)
        .context("Update signature verification failed")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installation {
    Direct(PathBuf),
    Managed(&'static str),
}

/// Existing per-user script installs migrate without re-installing. Unknown,
/// symlinked, system and package-manager layouts are never silently adopted.
pub fn installation(
    exe: &Path,
    home: &Path,
    local_app_data: Option<&Path>,
    flatpak: bool,
) -> Installation {
    if flatpak {
        return Installation::Managed("Update con through your software manager or Flatpak.");
    }
    let Some(root) = exe.parent() else {
        return Installation::Managed("Update con using its original installation method.");
    };
    let allowed = con_paths::portable_installation_dir(home, local_app_data);
    let canonical = fs::canonicalize(root).ok();
    let expected = allowed.as_ref().and_then(|p| fs::canonicalize(p).ok());
    let redirected = root.ancestors().any(Path::is_symlink);
    if canonical.is_some() && canonical == expected && !exe.is_symlink() && !redirected {
        Installation::Direct(canonical.unwrap())
    } else {
        Installation::Managed("Update con using its original installation method.")
    }
}

pub fn host_installation() -> Installation {
    let home = dirs::home_dir().unwrap_or_default();
    installation(
        &std::env::current_exe().unwrap_or_default(),
        &home,
        std::env::var_os("LOCALAPPDATA").as_deref().map(Path::new),
        std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists(),
    )
}

pub fn binary_names() -> [&'static str; 2] {
    if cfg!(target_os = "windows") {
        ["con-app.exe", "con-cli.exe"]
    } else {
        ["con", "con-cli"]
    }
}

/// Held for the lifetime of each application instance. The helper waits for all
/// updated clients sharing this installation; it never terminates another app.
pub fn hold_installation(root: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".con-update.lock"))?;
    file.lock_shared()?;
    Ok(file)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prepared {
    pub root: PathBuf,
    pub stage: PathBuf,
    pub release: Release,
}

/// Unique private staging on the destination filesystem; no live files change.
pub async fn prepare(
    root: &Path,
    release: Release,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<Prepared> {
    prepare_with_key(root, release, cancel, progress, PUBLIC_KEY).await
}

async fn prepare_with_key(
    root: &Path,
    release: Release,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
    key: &str,
) -> Result<Prepared> {
    ensure!(
        !key.is_empty(),
        "This build has no update verification key; download the release manually"
    );
    ensure!(!cancel.load(Ordering::Relaxed), "Download cancelled");
    let download_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".con-update-download.lock"))?;
    download_lock
        .try_lock()
        .context("Another con instance is downloading an update")?;
    ensure!(
        pending(root)?.is_none(),
        "An update is already prepared; reopen con to finish it"
    );
    let stage = root.join(format!(".con-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&stage).context("Installation is not writable")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))?;
    }
    let result = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()?;
        let mut response = tokio::select! {
            result=client.get(&release.url).send()=>result?.error_for_status()?,
            _=cancelled(cancel)=>bail!("Download cancelled"),
        };
        ensure!(
            response.url().scheme() == "https" || (cfg!(test) && response.url().host_str()==Some("127.0.0.1")),
            "Update redirected away from HTTPS"
        );
        let mut bytes = Vec::new();
        loop {
            let chunk=tokio::select! {result=response.chunk()=>result?,_=cancelled(cancel)=>bail!("Download cancelled")};
            let Some(chunk)=chunk else{break;};
            ensure!(!cancel.load(Ordering::Relaxed), "Download cancelled");
            ensure!(
                bytes.len() as u64 + chunk.len() as u64 <= release.length,
                "Update exceeds signed archive length"
            );
            bytes.extend_from_slice(&chunk);
            progress(bytes.len() as u64, release.length);
        }
        verify(&bytes, &release, key)?;
        ensure!(!cancel.load(Ordering::Relaxed),"Download cancelled");
        fs::write(stage.join("archive"), &bytes)?;
        extract(&bytes, &stage)?;
        validate_identity(&stage, &release)?;
        ensure!(!cancel.load(Ordering::Relaxed),"Download cancelled");
        let prepared = Prepared {
            root: root.to_owned(),
            stage: stage.clone(),
            release,
        };
        durable_write(&stage.join("plan.json"), &serde_json::to_vec(&prepared)?)?;
        Ok(prepared)
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

/// Extract only the two shipped executables. Reject ambiguous duplicates, links,
/// traversal and unbounded expansion; never unpack arbitrary archive paths.
pub fn extract(bytes: &[u8], stage: &Path) -> Result<()> {
    extract_format(bytes, stage, cfg!(target_os = "windows"))
}
fn extract_format(bytes: &[u8], stage: &Path, zipped: bool) -> Result<()> {
    let names = [binary_names()[0], binary_names()[1], "update.json"];
    let mut seen = [false; 3];
    let mut expanded = 0_u64;
    let mut accept = |path: &Path, size: u64, reader: &mut dyn Read| -> Result<()> {
        expanded = expanded
            .checked_add(size)
            .context("Update expansion overflow")?;
        ensure!(
            expanded <= MAX_ARCHIVE * 2,
            "Update expands beyond its limit"
        );
        ensure!(
            path.as_os_str().len() <= 1024,
            "Update entry path is too long"
        );
        ensure!(
            !path.is_absolute()
                && path
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Unsafe update archive path"
        );
        let Some(index) = names
            .iter()
            .position(|name| path.file_name().is_some_and(|n| n == *name))
        else {
            return Ok(());
        };
        ensure!(!seen[index], "Duplicate update binary");
        ensure!(
            index != 2 || size <= 16 * 1024,
            "Update identity is too large"
        );
        ensure!(
            size > 0 && size <= MAX_ARCHIVE,
            "Invalid extracted binary size"
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(stage.join(names[index]))?;
        let copied = std::io::copy(&mut reader.take(size + 1), &mut file)?;
        ensure!(copied == size, "Invalid update entry length");
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(stage.join(names[index]), fs::Permissions::from_mode(0o755))?;
        }
        seen[index] = true;
        Ok(())
    };
    if zipped {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        ensure!(archive.len() <= 1024, "Too many update entries");
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            ensure!(!entry.is_symlink(), "Links in update archive");
            if entry.is_dir() {
                continue;
            }
            let path = entry
                .enclosed_name()
                .context("Unsafe update archive path")?
                .to_owned();
            let size = entry.size();
            accept(&path, size, &mut entry)?;
        }
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
        for (index, entry) in archive.entries()?.enumerate() {
            ensure!(index < 1024, "Too many update entries");
            let mut entry = entry?;
            if entry.header().entry_type().is_dir() {
                continue;
            }
            ensure!(
                entry.header().entry_type().is_file(),
                "Links in update archive"
            );
            let path = entry.path()?.into_owned();
            let size = entry.size();
            accept(&path, size, &mut entry)?;
        }
    }
    ensure!(
        seen.iter().all(|v| *v),
        "Update archive must include the app, companion CLI and signed identity"
    );
    Ok(())
}

/// Bounded plain-text reading surface; release HTML is never executed by GPUI.
pub fn notes_text(html: &str) -> String {
    // Full documents contain style/title content. Prefer the semantic article.
    if let Some(start) = html.find("<article>") {
        let rest = &html[start + 9..];
        return notes_text(rest.split("</article>").next().unwrap_or(rest));
    }
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars().take(64 * 1024) {
        match c {
            '<' => {
                in_tag = true;
                text.push('\n');
            }
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"")
}

#[derive(Serialize, Deserialize)]
struct Identity {
    protocol: u32,
    version: String,
    platform: String,
    arch: String,
}
pub fn validate_identity(stage: &Path, release: &Release) -> Result<()> {
    let identity: Identity = serde_json::from_slice(&fs::read(stage.join("update.json"))?)?;
    let platform = if cfg!(windows) { "windows" } else { "linux" };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    ensure!(
        identity.protocol == 1
            && identity.version == release.version.trim_start_matches('v')
            && identity.platform == platform
            && identity.arch == arch,
        "Signed archive identity does not match this update"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteBlock {
    Heading(String),
    Subheading(String),
    Body(String),
    Credit(String),
}
pub fn notes_blocks(html: &str) -> Vec<NoteBlock> {
    let Some(start) = html.find("<article>") else {
        return vec![NoteBlock::Body(notes_text(html))];
    };
    let Some(end) = html[start..].find("</article>") else {
        return vec![NoteBlock::Body(notes_text(html))];
    };
    let Ok(document) = roxmltree::Document::parse(&html[start..start + end + 10]) else {
        return vec![NoteBlock::Body(notes_text(html))];
    };
    let text = |node: roxmltree::Node| {
        node.descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect::<Vec<_>>()
            .join("")
    };
    let mut blocks = Vec::new();
    for node in document.descendants().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "h2" => blocks.push(NoteBlock::Heading(text(node))),
            "h3" => blocks.push(NoteBlock::Subheading(text(node))),
            "li" => {
                let body = node
                    .descendants()
                    .filter(|n| {
                        n.is_text()
                            && !n
                                .ancestors()
                                .any(|a| a.attribute("class") == Some("credit"))
                    })
                    .filter_map(|n| n.text())
                    .collect::<String>();
                blocks.push(NoteBlock::Body(body));
            }
            "span" if node.attribute("class") == Some("credit") => {
                blocks.push(NoteBlock::Credit(text(node)))
            }
            "p" if node.attribute("class") != Some("version") => {
                blocks.push(NoteBlock::Body(text(node)))
            }
            _ => {}
        }
    }
    blocks
}

/// Resume a completed download or interrupted transaction without trusting
/// cached unpacked executables. The helper always re-verifies and re-extracts.
pub fn pending(root: &Path) -> Result<Option<Prepared>> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(root)? {
        let stage = entry?.path();
        if stage.is_symlink()
            || !stage.is_dir()
            || !stage.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.strip_prefix(".con-update-")
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
            })
        {
            continue;
        }
        let Ok(bytes) = fs::read(stage.join("plan.json")) else {
            continue;
        };
        let Ok(plan) = serde_json::from_slice::<Prepared>(&bytes) else {
            continue;
        };
        if plan.root != root || plan.stage != stage {
            continue;
        }
        let Ok(archive) = read_archive(&plan) else {
            continue;
        };
        if verify(&archive, &plan.release, PUBLIC_KEY).is_err() {
            continue;
        }
        if stage.join("complete").exists() {
            if let Ok(guard) = OpenOptions::new()
                .read(true)
                .write(true)
                .open(stage.join("helper.lock"))
            {
                if guard.try_lock().is_err() {
                    continue;
                }
            }
            let garbage = root.join(format!(".con-cleanup-{}", uuid::Uuid::new_v4()));
            if fs::rename(&stage, &garbage).is_ok() {
                let _ = fs::remove_dir_all(garbage);
            }
            continue;
        }
        candidates.push(plan);
    }
    candidates.sort_by(|a, b| {
        semver::Version::parse(a.release.version.trim_start_matches('v'))
            .ok()
            .cmp(&semver::Version::parse(b.release.version.trim_start_matches('v')).ok())
    });
    Ok(candidates.pop())
}

#[cfg(test)]
mod tests;

fn durable_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().context("Missing update directory")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".con-write-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    durable_write(path, bytes)
}

async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn read_archive(plan: &Prepared) -> Result<Vec<u8>> {
    let path = plan.stage.join("archive");
    ensure!(
        plan.release.length <= MAX_ARCHIVE && fs::metadata(&path)?.len() == plan.release.length,
        "Invalid cached archive length"
    );
    Ok(fs::read(path)?)
}

/// Only a successful session snapshot followed by explicit restart may commit.
pub fn authorize_exit(plan: &Prepared) -> Result<()> {
    ensure!(
        plan.stage.join("ready").is_file(),
        "Update helper is not ready"
    );
    durable_write(&plan.stage.join("commit"), b"apply")
}
