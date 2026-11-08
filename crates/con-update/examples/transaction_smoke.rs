//! Native subprocess smoke test. Build only with the documented disposable key.
//! This never updates the user's installation or calls a production endpoint.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use con_update::{Prepared, Release};
use ed25519_dalek::{Signer, SigningKey};
use std::{
    fs,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

fn wait_for(mut condition: impl FnMut() -> bool) -> Result<()> {
    let started = Instant::now();
    while !condition() {
        ensure!(
            started.elapsed() < Duration::from_secs(20),
            "fixture timed out"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|s| s == "+update-helper") {
        return con_update::run_helper(Path::new(args.get(2).context("missing plan")?));
    }
    let exe = std::env::current_exe()?;
    if exe.file_name() == Some(std::ffi::OsStr::new(con_update::binary_names()[0])) {
        if args.get(1).is_some_and(|s| s == "--hold-installation") {
            let root = exe.parent().unwrap();
            let _lock = con_update::hold_installation(root)?;
            fs::write(root.join("live-app"), b"running")?;
            wait_for(|| root.join("exit-app").exists())?;
            return Ok(());
        }
        fs::write(
            exe.parent().unwrap().join("relaunched"),
            b"fixture app started",
        )?;
        return Ok(());
    }
    let key = SigningKey::from_bytes(&[19; 32]);
    ensure!(
        con_update::PUBLIC_KEY == STANDARD.encode(key.verifying_key().to_bytes()),
        "Build this example with the disposable fixture public key; never a production key"
    );
    let temp = tempfile::Builder::new()
        .prefix("con-update-smoke-")
        .tempdir()?;
    let root = fs::canonicalize(temp.path())?;
    let stage = root.join(format!(".con-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&stage)?;
    let original = fs::read(&exe)?;
    let mut replacement = original.clone();
    replacement.extend_from_slice(b"\ncon-update-verified-fixture\n");
    let mut entries = Vec::new();
    for name in con_update::binary_names() {
        fs::copy(&exe, root.join(name))?;
        entries.push((name.to_owned(), replacement.clone()));
    }
    fs::write(root.join("preferences"), b"untouched")?;
    entries.push(("update.json".into(), serde_json::to_vec(&serde_json::json!({
        "protocol": 1, "version": "0.1.0-beta.174", "platform": if cfg!(windows) {"windows"} else {"linux"},
        "arch": match std::env::consts::ARCH {"aarch64" => "arm64", other => other}
    }))?));
    let archive = if cfg!(windows) {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(name, zip::write::SimpleFileOptions::default())?;
            zip.write_all(&bytes)?;
        }
        zip.finish()?.into_inner()
    } else {
        let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gzip);
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes.as_slice())?;
        }
        tar.into_inner()?.finish()?
    };
    let plan = Prepared {
        root: root.clone(),
        stage: stage.clone(),
        release: Release {
            version: "0.1.0-beta.174".into(),
            url: "https://example.invalid/fixture".into(),
            signature: STANDARD.encode(key.sign(&archive).to_bytes()),
            length: archive.len() as u64,
            notes: String::new(),
        },
    };
    fs::write(stage.join("archive"), archive)?;
    fs::write(stage.join("plan.json"), serde_json::to_vec(&plan)?)?;
    let mut app = std::process::Command::new(root.join(con_update::binary_names()[0]))
        .arg("--hold-installation")
        .stdin(std::process::Stdio::null())
        .spawn()?;
    wait_for(|| root.join("live-app").exists())?;
    let running = con_update::hold_installation(&root)?;
    con_update::spawn_helper(&plan)?;
    std::thread::sleep(Duration::from_millis(250));
    for name in con_update::binary_names() {
        ensure!(
            fs::read(root.join(name))? == original,
            "replaced before exit"
        );
    }
    ensure!(
        !root.join("relaunched").exists(),
        "relaunched before authorization"
    );
    con_update::authorize_exit(&plan)?;
    drop(running);
    std::thread::sleep(Duration::from_millis(250));
    ensure!(
        !root.join("relaunched").exists(),
        "ignored another live instance"
    );
    fs::write(root.join("exit-app"), b"exit normally")?;
    ensure!(app.wait()?.success(), "old app did not exit normally");
    wait_for(|| root.join("relaunched").exists())?;
    for name in con_update::binary_names() {
        ensure!(
            fs::read(root.join(name))? == replacement,
            "paired replacement mismatch"
        );
    }
    ensure!(
        fs::read_to_string(root.join(".con-update-result"))? == "Update installed",
        "helper failed"
    );
    ensure!(
        fs::read(root.join("preferences"))? == b"untouched",
        "unrelated data changed"
    );
    // On Windows the copied helper is briefly still mapped after relaunch.
    wait_for(|| fs::remove_dir_all(&root).is_ok())?;
    println!(
        "PASS: real helper subprocess, live-instance lock, authorized paired replacement, executable relaunch, unrelated-file retention, cleanup ({})",
        std::env::consts::OS
    );
    Ok(())
}
