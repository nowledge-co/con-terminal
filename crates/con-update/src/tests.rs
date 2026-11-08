use super::*;
use ed25519_dalek::{Signer, SigningKey};
use std::io::Write;

fn signed(bytes: &[u8]) -> (Release, String) {
    let key = SigningKey::from_bytes(&[19; 32]);
    (
        Release {
            version: "0.1.0-beta.174".into(),
            url: format!(
                "https://example.com/con-0.1.0-beta.174-{}-{}.{}",
                if cfg!(windows) { "windows" } else { "linux" },
                std::env::consts::ARCH,
                if cfg!(windows) { "zip" } else { "tar.gz" }
            ),
            length: bytes.len() as u64,
            signature: STANDARD.encode(key.sign(bytes).to_bytes()),
            notes: String::new(),
        },
        STANDARD.encode(key.verifying_key().to_bytes()),
    )
}
pub(super) fn archive(duplicate: bool) -> Vec<u8> {
    archive_format(duplicate, cfg!(windows))
}
fn archive_format(duplicate: bool, zipped: bool) -> Vec<u8> {
    let mut entries: Vec<(String, Vec<u8>)> = binary_names()
        .iter()
        .map(|n| (n.to_string(), format!("new {n}").into_bytes()))
        .collect();
    entries.push(("update.json".into(),serde_json::to_vec(&serde_json::json!({"protocol":1,"version":"0.1.0-beta.174","platform":if cfg!(windows){"windows"}else{"linux"},"arch":match std::env::consts::ARCH{"aarch64"=>"arm64",v=>v}})).unwrap()));
    if duplicate {
        entries.push(entries[0].clone());
    }
    if zipped {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (index, (name, bytes)) in entries.iter().enumerate() {
            zip.start_file(
                format!("folder{index}/{name}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    } else {
        let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tar = tar::Builder::new(gzip);
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, format!("release/{name}"), bytes.as_slice())
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }
}

#[test]
fn signed_bytes_and_identity_are_both_required() {
    let bytes = archive(false);
    let (release, key) = signed(&bytes);
    verify(&bytes, &release, &key).unwrap();
    let mut changed = bytes.clone();
    changed[3] ^= 1;
    assert!(verify(&changed, &release, &key).is_err());
    assert!(verify(&bytes[..bytes.len() - 1], &release, &key).is_err());
    assert!(verify(&bytes, &release, &STANDARD.encode([0; 32])).is_err());
    let stage = tempfile::tempdir().unwrap();
    extract(&bytes, stage.path()).unwrap();
    validate_identity(stage.path(), &release).unwrap();
    let mut wrong = release.clone();
    wrong.version = "0.1.0-beta.999".into();
    assert!(validate_identity(stage.path(), &wrong).is_err());
}
#[test]
fn incomplete_and_ambiguous_archives_leave_installation_untouched() {
    let stage = tempfile::tempdir().unwrap();
    assert!(extract(&archive(true), stage.path()).is_err());
    let stage = tempfile::tempdir().unwrap();
    assert!(extract(b"not an archive", stage.path()).is_err());
}
#[test]
fn namespaced_feed_requires_signature_and_matching_platform() {
    let (release, _) = signed(b"payload");
    let platform = if cfg!(windows) { "windows" } else { "linux" };
    let arch = std::env::consts::ARCH;
    let xml = format!(
        r#"<rss xmlns:s="{}"><channel><item><s:shortVersionString>{}</s:shortVersionString><description>&lt;p&gt;A &amp;amp; B&lt;/p&gt;</description><enclosure url="{}" length="{}" s:edSignature="{}"/></item><item><s:shortVersionString>9.0.0</s:shortVersionString></item></channel></rss>"#,
        SPARKLE, release.version, release.url, release.length, release.signature
    );
    assert_eq!(
        parse_feed(&xml, platform, arch).unwrap().version,
        release.version
    );
    assert!(parse_feed(&xml.replace("https:", "http:"), platform, arch).is_err());
    assert!(parse_feed(&xml, platform, "not-this-arch").is_err());
    assert!(parse_feed(&xml.replace("s:edSignature", "signature"), platform, arch).is_err());
    assert!(parse_feed(&xml.replace(&release.version, "invalid"), platform, arch).is_err());
}
#[test]
fn semver_prevents_downgrades_and_orders_beta_numbers() {
    assert!(newer("0.1.0-beta.174", "0.1.0-beta.99"));
    assert!(!newer("0.1.0-beta.99", "0.1.0-beta.174"));
    assert!(newer("0.1.0", "0.1.0-beta.174"));
    assert!(!newer("0.1.0-beta.174", "0.1.0"));
    assert!(!newer("garbage", "0.1.0"));
}
#[test]
fn installation_ownership_is_not_inferred_from_writability() {
    let home = tempfile::tempdir().unwrap();
    let home_path = fs::canonicalize(home.path()).unwrap();
    let bin = home_path.join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let exe = bin.join(binary_names()[0]);
    fs::write(&exe, b"old").unwrap();
    assert!(matches!(
        installation(&exe, home.path(), None, true),
        Installation::Managed(_)
    ));
    assert!(matches!(
        installation(&home.path().join("custom/con"), home.path(), None, false),
        Installation::Managed(_)
    ));
    #[cfg(not(windows))]
    assert_eq!(
        installation(&exe, &home_path, None, false),
        Installation::Direct(bin)
    );
}
#[cfg(unix)]
#[test]
fn symlinks_are_never_adopted() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let home_path = fs::canonicalize(home.path()).unwrap();
    let bin = home_path.join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(home.path().join("actual"), b"old").unwrap();
    let exe = bin.join("con");
    symlink(home.path().join("actual"), &exe).unwrap();
    assert!(matches!(
        installation(&exe, &home_path, None, false),
        Installation::Managed(_)
    ));
}
#[test]
fn structured_notes_preserve_inline_text_and_hierarchy() {
    let html = r#"<html><style>not visible</style><article><header><h1>What’s new</h1><p class="version">old label</p></header><h2>Fixed</h2><h3>Windows</h3><ul><li>Keep <code>con-cli</code> working.<span class="credit">PR <a href="https://example.com">#1</a></span></li></ul></article></html>"#;
    assert_eq!(
        notes_blocks(html),
        vec![
            NoteBlock::Heading("Fixed".into()),
            NoteBlock::Subheading("Windows".into()),
            NoteBlock::Body("Keep con-cli working.".into()),
            NoteBlock::Credit("PR #1".into())
        ]
    );
}
#[tokio::test]
async fn cancellation_never_creates_staging_or_touches_live_files() {
    let root = tempfile::tempdir().unwrap();
    let (release, key) = signed(b"payload");
    let cancel = AtomicBool::new(true);
    assert!(
        prepare_with_key(root.path(), release, &cancel, |_, _| {}, &key)
            .await
            .is_err()
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
#[test]
fn shared_installation_lock_blocks_exclusive_replacement() {
    let root = tempfile::tempdir().unwrap();
    let running = hold_installation(root.path()).unwrap();
    let helper = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join(".con-update.lock"))
        .unwrap();
    assert!(matches!(
        helper.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(running);
    helper.try_lock().unwrap();
}

fn serve(bytes: Vec<u8>, advertised: usize) -> (String, std::thread::JoinHandle<()>) {
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = socket.accept().unwrap();
        let mut request = [0; 2048];
        let _ = stream.read(&mut request);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {advertised}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let _ = stream.write_all(&bytes);
    });
    (format!("http://{address}/fixture"), worker)
}
#[tokio::test]
async fn real_download_verifies_stages_and_never_replaces_live_binaries() {
    let root = tempfile::tempdir().unwrap();
    for name in binary_names() {
        fs::write(root.path().join(name), "old").unwrap();
    }
    let bytes = archive(false);
    let (mut release, key) = signed(&bytes);
    let (url, server) = serve(bytes, release.length as usize);
    release.url = url;
    let mut count = 0;
    let prepared = prepare_with_key(
        root.path(),
        release,
        &AtomicBool::new(false),
        |received, _| count = received,
        &key,
    )
    .await
    .unwrap();
    server.join().unwrap();
    assert!(count > 0);
    assert!(prepared.stage.join("plan.json").exists());
    for name in binary_names() {
        assert_eq!(fs::read_to_string(root.path().join(name)).unwrap(), "old");
    }
}
#[tokio::test]
async fn truncated_network_download_keeps_old_installation_and_removes_stage() {
    let root = tempfile::tempdir().unwrap();
    let bytes = archive(false);
    let (mut release, key) = signed(&bytes);
    for name in binary_names() {
        fs::write(root.path().join(name), "old").unwrap();
    }
    let (url, server) = serve(bytes[..bytes.len() / 2].to_vec(), bytes.len());
    release.url = url;
    assert!(
        prepare_with_key(
            root.path(),
            release,
            &AtomicBool::new(false),
            |_, _| {},
            &key
        )
        .await
        .is_err()
    );
    server.join().unwrap();
    assert!(!fs::read_dir(root.path()).unwrap().any(|e| {
        let entry = e.unwrap();
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(".con-update-")
            && entry.path().is_dir()
    }));
    for name in binary_names() {
        assert_eq!(fs::read_to_string(root.path().join(name)).unwrap(), "old");
    }
}

#[test]
fn zip_and_tar_paths_are_exercised_on_every_host() {
    for zipped in [false, true] {
        let stage = tempfile::tempdir().unwrap();
        extract_format(&archive_format(false, zipped), stage.path(), zipped).unwrap();
        for name in binary_names() {
            assert_eq!(
                fs::read_to_string(stage.path().join(name)).unwrap(),
                format!("new {name}")
            );
        }
        let stage = tempfile::tempdir().unwrap();
        assert!(extract_format(&archive_format(true, zipped), stage.path(), zipped).is_err());
    }
}

#[cfg(windows)]
#[test]
fn windows_legacy_installation_uses_canonical_paths_without_reserved_names() {
    let local = tempfile::tempdir().unwrap();
    let root = local.path().join("Programs/con-terminal");
    fs::create_dir_all(&root).unwrap();
    let exe = root.join("con-app.exe");
    fs::write(&exe, b"fixture").unwrap();
    assert_eq!(
        installation(&exe, local.path(), Some(local.path()), false),
        Installation::Direct(fs::canonicalize(&root).unwrap())
    );
}

#[test]
fn cleanup_and_handoff_require_an_owned_staging_plan() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for name in binary_names() {
        fs::write(root.join(name), "original").unwrap();
    }
    for name in [
        ".con-update-personal".to_string(),
        format!(".con-update-{}", uuid::Uuid::new_v4()),
    ] {
        let unrelated = root.join(name);
        fs::create_dir(&unrelated).unwrap();
        fs::write(unrelated.join("complete"), "not an ownership grant").unwrap();
        fs::write(unrelated.join("keep"), "unrelated work").unwrap();
        assert!(pending(&root).unwrap().is_none());
        assert_eq!(
            fs::read_to_string(unrelated.join("keep")).unwrap(),
            "unrelated work"
        );
    }
    let stage = root.join(".con-update-personal");
    let plan = Prepared {
        root: root.clone(),
        stage: stage.clone(),
        release: signed(b"fixture").0,
    };
    let path = stage.join("plan.json");
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    assert!(run_helper(&path).is_err());
    assert!(!stage.join("ready").exists());
    for name in binary_names() {
        assert_eq!(fs::read_to_string(root.join(name)).unwrap(), "original");
    }
}
