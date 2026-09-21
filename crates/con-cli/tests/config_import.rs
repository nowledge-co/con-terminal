use std::{fs, path::Path, process::Command};

#[test]
fn imports_custom_theme_from_explicit_xdg_home_without_a_running_app() {
    let root = std::env::temp_dir().join(format!(
        "con-cli-theme-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let xdg = root.join("custom-xdg");
    let themes = xdg.join("ghostty/themes");
    fs::create_dir_all(&themes).unwrap();
    let name = "ConImportRegressionTheme";
    let colors = "background = 13579b\nforeground = fefefe\n";
    fs::write(themes.join(name), colors).unwrap();
    let source = root.join("source");
    fs::write(&source, format!("theme = {name}\n")).unwrap();
    let destination = root.join("imported.ghostty");
    let output = Command::new(env!("CARGO_BIN_EXE_con-cli"))
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["config", "import-ghostty", "--from"])
        .arg(&source)
        .arg("--to")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(&xdg).unwrap();
    let text = fs::read_to_string(destination).unwrap();
    let path = text.trim().strip_prefix("theme = ").unwrap();
    assert!(
        Path::new(path).is_absolute(),
        "theme was not snapshotted: {text}"
    );
    assert_eq!(fs::read_to_string(path).unwrap(), colors);
    fs::remove_dir_all(root).unwrap();
}
