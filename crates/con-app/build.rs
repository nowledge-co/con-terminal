fn main() {
    // Windows users hit `cargo build -p con --release` out of muscle
    // memory and get a confusing cargo warning: "binary target `con` is
    // a reserved Windows filename, this target will not work on Windows
    // platforms". That's because the default feature set targets
    // macOS/Linux (`bin-con`). Detect the bad combination and fail
    // early with a pointer at the `cargo wbuild` alias.
    //
    // Default macOS/Linux path: no feature gating — `bin-con` is the
    // default; the `con` bin target builds normally.
    // `app_display_version()` captures `CON_RELEASE_VERSION` via
    // `option_env!` on non-macOS builds. Cargo does not track env reads
    // inside macros, so a cached build would pin a stale version string
    // across a tag change — declare the dep to force invalidation.
    println!("cargo:rerun-if-env-changed=CON_RELEASE_VERSION");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let bin_con = std::env::var_os("CARGO_FEATURE_BIN_CON").is_some();
    let bin_con_app = std::env::var_os("CARGO_FEATURE_BIN_CON_APP").is_some();

    if target_os == "windows" && bin_con && !bin_con_app {
        panic!(
            "\n\n========================================================\n\
             con: `cargo build` with the default feature set targets the\n\
             `con` binary, but `CON` is a reserved DOS device name on\n\
             Windows — `con.exe` cannot be created.\n\n\
             Use the `cargo wbuild` / `cargo wcheck` / `cargo wrun` /\n\
             `cargo wtest` aliases instead — they select the `con-app`\n\
             binary via `--no-default-features --features con/bin-con-app`.\n\
             Aliases are declared in the workspace `.cargo/config.toml`.\n\n\
             See `docs/impl/windows-port.md` and `AGENTS.md` for details.\n\
             ========================================================\n"
        );
    }

    if target_os == "macos" {
        cc::Build::new()
            .file("src/objc/sparkle_trampoline.m")
            .file("src/objc/global_hotkey_trampoline.m")
            .file("src/objc/quick_terminal_trampoline.m")
            .file("src/objc/ghostty_surface_trampoline.m")
            .flag("-fobjc-arc")
            .flag("-fmodules")
            .compile("con_objc_trampolines");

        println!("cargo:rerun-if-changed=src/objc/sparkle_trampoline.m");
        println!("cargo:rerun-if-changed=src/objc/global_hotkey_trampoline.m");
        println!("cargo:rerun-if-changed=src/objc/quick_terminal_trampoline.m");
        println!("cargo:rerun-if-changed=src/objc/ghostty_surface_trampoline.m");
        println!("cargo:rustc-link-lib=framework=Carbon");
    }

    #[cfg(target_os = "windows")]
    {
        // Ship as `con-app.ico` because `CON` is a reserved DOS
        // device name. git.exe on Windows refuses to clone / pull any
        // repo whose tree contains a file literally named `con.*`,
        // even inside a subdirectory — same trap that forces the
        // binary to ship as `con-app.exe`.
        let icon = "../../assets/windows/con-app.ico";
        println!("cargo:rerun-if-changed={}", icon);

        let mut res = winresource::WindowsResource::new();
        // Some locked-down hosts (and the GitHub Actions windows-2022
        // image on occasion) can't auto-discover rc.exe; let CI point
        // us at the toolkit explicitly.
        if let Ok(toolkit) = std::env::var("CON_RC_TOOLKIT_PATH") {
            res.set_toolkit_path(&toolkit);
        }
        res.set_icon(icon);
        res.set("FileDescription", "con");
        res.set("ProductName", "con");
        if let Err(e) = res.compile() {
            eprintln!("winresource failed to embed icon: {e}");
            std::process::exit(1);
        }
    }
}
