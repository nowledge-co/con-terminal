use std::collections::{HashSet, VecDeque};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const GHOSTTY_REPO: &str = "https://github.com/ghostty-org/ghostty.git";
/// Pinned Ghostty revision. Bump together when updating either the
/// macOS full-libghostty build or the Windows libghostty-vt build —
/// both consume the same source tree to keep VT semantics in sync.
///
/// 2026-08-25 bump: from `ca7516bea6...` to `8867c37c55...`. This is the
/// exact upstream revision audited for the Zig 0.16 migration and the
/// portable Kitty protocol work; do not replace it with a moving branch.
///
/// Ghostty's internal macOS embedding API and libghostty-vt API are not
/// stable. Future bumps must update the handwritten FFI bindings, compile
/// the ABI assertions, and run real build/link/runtime checks on all three
/// platforms rather than treating this as a source-only dependency bump.
const GHOSTTY_REV: &str = "8867c37c55b578b9eb4cfaba41cb9023e557176d";
const GHOSTTY_ENV: &str = "CON_GHOSTTY_SOURCE_DIR";
const GHOSTTY_INITIAL_OUTPUT_REQUIRE_ENV: &str = "CON_REQUIRE_GHOSTTY_INITIAL_OUTPUT";
const GHOSTTY_VT_TARGET_ENV: &str = "CON_GHOSTTY_VT_TARGET";
const REQUIRED_ZIG_VERSION: &str = "0.16.0";
const MAX_PREFETCH_PACKAGES: usize = 256;
const MAX_PREFETCH_ARCHIVE_BYTES: &str = "134217728";

fn main() {
    // Rerun the build script when any of our own env vars flip — cargo
    // otherwise caches the last build.rs output across env changes, so
    // toggling CON_STUB_GHOSTTY_VT off wouldn't re-build libghostty-vt
    // until something unrelated invalidated the cache.
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    println!("cargo:rerun-if-env-changed={GHOSTTY_ENV}");
    println!("cargo:rerun-if-env-changed={GHOSTTY_INITIAL_OUTPUT_REQUIRE_ENV}");
    println!("cargo:rerun-if-env-changed=CON_STUB_GHOSTTY_VT");
    println!("cargo:rerun-if-env-changed=CON_SKIP_GHOSTTY_VT");
    println!("cargo:rerun-if-env-changed=CON_GHOSTTY_VT_SIMD");
    println!("cargo:rerun-if-env-changed=CON_GHOSTTY_VT_STEP");
    println!("cargo:rerun-if-env-changed={GHOSTTY_VT_TARGET_ENV}");
    println!("cargo:rerun-if-env-changed=CON_ZIG_BIN");
    println!("cargo:rerun-if-env-changed=CON_GHOSTTY_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=ZIG_GLOBAL_CACHE_DIR");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    match target_os.as_str() {
        "macos" => build_macos(),
        "windows" | "linux" => build_vt_backend(target_os.as_str()),
        other => {
            println!(
                "cargo:warning=con-ghostty: skipping native build on target_os={other} \
                 (unsupported target; terminal falls back to stub backend)"
            );
        }
    }
}

// ── macOS ─────────────────────────────────────────────────────────────

fn build_macos() {
    let zig_bin = env::var_os("CON_ZIG_BIN").unwrap_or_else(|| std::ffi::OsString::from("zig"));
    let zig_global_cache_dir = zig_global_cache_dir("macos");
    require_zig_version(&zig_bin, zig_global_cache_dir.as_deref(), "macos");

    let ghostty_dir = resolve_ghostty_source();
    let ghostty_dir = patchable_ghostty_source(ghostty_dir);
    let initial_output_restore_enabled = apply_embedded_initial_output_patch(&ghostty_dir);
    let include_dir = ghostty_dir.join("include");
    let optimize = ghostty_optimize();

    let mut ffi_abi = cc::Build::new();
    ffi_abi
        .file("src/ffi_abi.c")
        .include(&include_dir)
        .flag("-std=c11");
    if initial_output_restore_enabled {
        ffi_abi.define("CON_GHOSTTY_EMBEDDED_INITIAL_OUTPUT", None);
    }
    ffi_abi.compile("con_ghostty_ffi_abi");
    println!("cargo:rerun-if-changed=src/ffi_abi.c");

    cc::Build::new()
        .file("src/objc/desktop_notification.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag("-fmodules")
        .compile("con_ghostty_objc_trampolines");
    println!("cargo:rerun-if-changed=src/objc/desktop_notification.m");

    let build_args = vec![
        "build".to_string(),
        "-Dapp-runtime=none".to_string(),
        "-Dxcframework-target=native".to_string(),
        "-Demit-macos-app=false".to_string(),
        format!("-Doptimize={optimize}"),
    ];
    let mut cmd = Command::new(&zig_bin);
    configure_zig_command(&mut cmd, zig_global_cache_dir.as_deref());
    cmd.args(&build_args).current_dir(&ghostty_dir);

    let status = cmd.status().unwrap_or_else(|err| {
        panic!(
            "failed to run `{}` build for libghostty: {err}",
            zig_bin.to_string_lossy()
        )
    });

    if !status.success() {
        println!(
            "cargo:warning=zig build failed for libghostty; prefetching Zig package cache and retrying"
        );
        prefetch_zig_dependencies(&zig_bin, &ghostty_dir, zig_global_cache_dir.as_deref());

        let mut retry = Command::new(&zig_bin);
        configure_zig_command(&mut retry, zig_global_cache_dir.as_deref());
        retry.args(&build_args).current_dir(&ghostty_dir);
        let retry_status = retry.status().unwrap_or_else(|err| {
            panic!(
                "failed to retry `{}` build for libghostty: {err}",
                zig_bin.to_string_lossy()
            )
        });
        if !retry_status.success() {
            panic!("zig build failed for libghostty");
        }
    }

    // `libghostty-internal-fat.a` is an intermediate archive that still
    // carries Zig compiler-rt memcpy/memmove. The final archive applies
    // Ghostty's libSystem override, which avoids a measured PTY throughput
    // regression in scroll-heavy workloads.
    let xcframework_arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        Ok(arch) => panic!("unsupported macOS Ghostty architecture: {arch}"),
        Err(err) => panic!("CARGO_CFG_TARGET_ARCH is not set: {err}"),
    };
    let lib_path = ghostty_dir
        .join("macos")
        .join("GhosttyKit.xcframework")
        .join(format!("macos-{xcframework_arch}"))
        .join("libghostty-internal.a");
    if !lib_path.is_file() {
        panic!(
            "Could not find installed final archive {} after building ghostty source at {}",
            lib_path.display(),
            ghostty_dir.display()
        );
    }
    println!(
        "cargo:rustc-link-search=native={}",
        lib_path.parent().unwrap().display()
    );
    println!("cargo:rustc-link-lib=static=ghostty-internal");

    for framework in &[
        "AppKit",
        "Metal",
        "CoreGraphics",
        "CoreText",
        "CoreVideo",
        "CoreFoundation",
        "Foundation",
        "IOSurface",
        "QuartzCore",
        "Carbon",
        "UserNotifications",
    ] {
        println!("cargo:rustc-link-lib=framework={}", framework);
    }
    println!("cargo:rustc-link-lib=c++");

    if env::var_os(GHOSTTY_ENV).is_some() {
        println!(
            "cargo:rerun-if-changed={}",
            ghostty_dir.join("src").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            ghostty_dir.join("include/ghostty.h").display()
        );
    }

    println!("cargo:include={}", include_dir.display());
    println!(
        "cargo:rustc-env=CON_GHOSTTY_RESOURCES_DIR={}",
        ghostty_dir.join("zig-out/share/ghostty").display()
    );
}

fn ghostty_optimize() -> String {
    env::var("CON_GHOSTTY_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".to_string())
}

fn env_flag_enabled(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

// ── Windows ───────────────────────────────────────────────────────────

fn build_vt_backend(target_os: &str) {
    // libghostty-vt is the cross-platform VT parser carved out of
    // Ghostty (PR ghostty-org/ghostty#8840). Both the Windows backend
    // and the Linux backend consume it now: Windows feeds it from
    // ConPTY and Linux feeds it from a local Unix PTY.

    if target_os == "windows" && env::var_os("CON_STUB_GHOSTTY_VT").is_some() {
        // Fallback path: compile the C stub in `src/windows/ghostty_vt_stub.c`
        // and link it instead of libghostty-vt. The resulting binary
        // launches fully — GPUI window, HWND host view, D3D11 swapchain,
        // ConPTY spawn — but the terminal pane paints an empty grid
        // because the stub returns no rows. Useful for iterating the
        // non-VT parts of the backend while Zig / Ghostty build issues
        // are resolved separately. See docs/impl/windows-port.md.
        println!(
            "cargo:warning=CON_STUB_GHOSTTY_VT set — linking stub C implementations \
                  instead of libghostty-vt. Terminal output will be empty."
        );
        cc::Build::new()
            .file("src/windows/ghostty_vt_stub.c")
            .compile("ghostty_vt_stub");
        // The static lib name above means `-lghostty_vt_stub` is emitted
        // by cc-rs; but our vt.rs binds to `ghostty_*` symbols that the
        // stub provides, so the linker resolves them from ghostty_vt_stub.
        // No extra cargo:rustc-link-lib needed — cc-rs handles it.
        return;
    }

    if env::var_os("CON_SKIP_GHOSTTY_VT").is_some() {
        // Escape hatch: skip both upstream and stub. The vt.rs symbols
        // become unresolved at link time; intended for `cargo check`
        // flows that don't link, or when you plan to supply a static
        // library via other means.
        println!(
            "cargo:warning=CON_SKIP_GHOSTTY_VT set — skipping libghostty-vt build. \
             A subsequent `cargo build` will fail to link unless a static \
             library is provided manually.{}",
            if target_os == "windows" {
                " Consider CON_STUB_GHOSTTY_VT=1 if you want a linkable placeholder."
            } else {
                ""
            }
        );
        return;
    }

    // Detect Zig. Surface the actual error (not-found, permission-denied,
    // etc.) and the PATH we searched so the user can diagnose — previous
    // silent warnings led to confusing link-time failures.
    let zig_bin = env::var_os("CON_ZIG_BIN").unwrap_or_else(|| std::ffi::OsString::from("zig"));
    let zig_global_cache_dir = zig_global_cache_dir(target_os);
    if let Some(dir) = &zig_global_cache_dir {
        println!("cargo:warning=using zig global cache dir {}", dir.display());
    }

    require_zig_version(&zig_bin, zig_global_cache_dir.as_deref(), target_os);

    let ghostty_dir = resolve_ghostty_source();

    // Upstream Ghostty exposes libghostty-vt in two shapes depending on
    // revision:
    //   (a) Current main: option `-Demit-lib-vt=true` on the default
    //       `install` step disables the xcframework / macOS-app / docs
    //       and leaves libghostty-vt as the produced artifact.
    //   (b) Older revisions: a named step like `ghostty-vt-static`.
    // We probe `zig build -h` to pick the right invocation. Override
    // the probe via `CON_GHOSTTY_VT_STEP` (step name) or
    // `CON_GHOSTTY_VT_EMIT_OPTION=1` (force the option-based path).
    let invocation = pick_vt_invocation(&zig_bin, &ghostty_dir, zig_global_cache_dir.as_deref());

    // `-Doptimize=ReleaseFast` for terminal-class throughput. Default
    // matches macOS so debug Rust builds still get a release-grade VT
    // parser — past macOS lag (resize/reflow stalls of seconds) was
    // traced to building libghostty *without* an explicit optimize
    // flag, which left it at zig's `-ODebug`. Same risk applies on
    // Windows, so default it on regardless of cargo profile and let
    // `CON_GHOSTTY_OPTIMIZE=Debug` opt back in for VT-debugging.
    //
    // `-Dsimd`: Ghostty vendors `simdutf` (a C++ SIMD UTF-8 library)
    // when SIMD is on, but the produced static archive does not yet
    // bundle the required `simdutf` objects reliably across our target
    // environments. Default SIMD off for now so the resulting
    // `libghostty-vt` archive is self-contained on both Windows and
    // Linux. Keep the env override for experimentation once the native
    // link surface is understood well enough to ship.
    let simd_on = env::var("CON_GHOSTTY_VT_SIMD")
        .map(|s| matches!(s.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let simd_flag = if simd_on {
        "-Dsimd=true"
    } else {
        "-Dsimd=false"
    };

    let optimize = ghostty_optimize();
    let zig_target = ghostty_vt_zig_target();

    let mut build_args = Vec::new();
    build_args.push("build".to_string());
    for arg in &invocation {
        build_args.push(arg.clone());
    }
    if let Some(zig_target) = &zig_target {
        // Do not let Zig default to the build machine's native CPU for
        // release artifacts. GitHub runners can expose ISA extensions
        // that WSL / older x86_64 hosts do not support, and a native-built
        // static lib can SIGILL even when the Rust binary itself is generic.
        println!("cargo:warning=building libghostty-vt for Zig target {zig_target}");
        build_args.push(format!("-Dtarget={zig_target}"));
    } else {
        println!(
            "cargo:warning=building libghostty-vt without explicit Zig target; \
             set {GHOSTTY_VT_TARGET_ENV} to avoid native CPU codegen"
        );
    }
    build_args.push(format!("-Doptimize={optimize}"));
    build_args.push(simd_flag.to_string());

    let status = run_zig_command_status(
        &zig_bin,
        &ghostty_dir,
        zig_global_cache_dir.as_deref(),
        &build_args,
    )
    .expect("failed to run zig build for libghostty-vt");

    if !status.success() {
        println!(
            "cargo:warning=zig build failed for libghostty-vt; prefetching Zig package cache and retrying"
        );
        prefetch_zig_dependencies(&zig_bin, &ghostty_dir, zig_global_cache_dir.as_deref());

        let retry_status = run_zig_command_status(
            &zig_bin,
            &ghostty_dir,
            zig_global_cache_dir.as_deref(),
            &build_args,
        )
        .expect("failed to retry zig build for libghostty-vt");

        if !retry_status.success() {
            panic!(
                "zig build {:?} failed after package-cache prefetch; see output above.\n\
                 If this Ghostty revision doesn't expose libghostty-vt,\n\
                 bump GHOSTTY_REV in crates/con-ghostty/build.rs or set\n\
                 CON_GHOSTTY_VT_STEP to the correct step name.",
                invocation
            );
        }
    }

    // Link the static archive installed by the build we just ran. Do not scan
    // `.zig-cache`: one Ghostty checkout can contain cached archives for many
    // targets, and mtime cannot tell a Linux archive from a newer Mach-O one.
    // On Windows the distinct name also prevents accidentally linking the DLL
    // import library, which would create an unshipped runtime dependency.
    let (static_lib_name, link_name) = if target_os == "windows" {
        ("ghostty-vt-static.lib", "ghostty-vt-static")
    } else {
        ("libghostty-vt.a", "ghostty-vt")
    };
    let lib_path = ghostty_dir
        .join("zig-out")
        .join("lib")
        .join(static_lib_name);
    if !lib_path.is_file() {
        panic!(
            "Could not find installed static archive {} after building ghostty source at {}",
            lib_path.display(),
            ghostty_dir.display()
        );
    }

    println!(
        "cargo:rustc-link-search=native={}",
        lib_path.parent().unwrap().display()
    );
    println!("cargo:rustc-link-lib=static={link_name}");

    // libghostty-vt itself doesn't pull in a platform windowing stack.
    // Windows still relies on the MSVC runtime and the `windows` crate;
    // Linux relies on the regular libc toolchain.

    if env::var_os(GHOSTTY_ENV).is_some() {
        println!(
            "cargo:rerun-if-changed={}",
            ghostty_dir.join("src").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            ghostty_dir.join("include/ghostty/vt.h").display()
        );
    }

    let include_dir = ghostty_dir.join("include");
    println!("cargo:include={}", include_dir.display());
}

/// Probe `zig build -h` and pick how to invoke a libghostty-vt build.
/// Returns a list of extra arguments to append after `zig build`.
///
/// Priority:
///   1. `CON_GHOSTTY_VT_STEP` env var -> single named step.
///   2. `-Demit-lib-vt=` option in help -> `["-Demit-lib-vt=true"]`.
///   3. Named step matching one of the known candidates.
fn pick_vt_invocation(
    zig_bin: &std::ffi::OsStr,
    ghostty_dir: &PathBuf,
    zig_global_cache_dir: Option<&std::path::Path>,
) -> Vec<String> {
    if let Some(val) = env::var_os("CON_GHOSTTY_VT_STEP") {
        if let Some(s) = val.to_str() {
            return vec![s.to_string()];
        }
    }

    let help_text = zig_build_help_text(zig_bin, ghostty_dir, zig_global_cache_dir);

    // Preferred: current Ghostty exposes `-Demit-lib-vt=[bool]` which
    // configures the default `install` step to produce only
    // libghostty-vt (disables xcframework / macOS-app / docs).
    if help_text.contains("-Demit-lib-vt=") {
        return vec!["-Demit-lib-vt=true".to_string()];
    }

    // Fallback: older revisions that split the lib behind a named step.
    const CANDIDATES: &[&str] = &[
        "ghostty-vt-static",
        "libghostty-vt-static",
        "vt-static",
        "libghostty-vt",
        "ghostty-vt",
    ];
    for cand in CANDIDATES {
        if help_text.lines().any(|line| {
            let t = line.trim_start();
            t.starts_with(&format!("{cand} ")) || t == *cand
        }) {
            return vec![cand.to_string()];
        }
    }

    panic!(
        "\n========================================================\n\
         con-ghostty: couldn't find a libghostty-vt build knob in this\n\
         Ghostty checkout. The pinned revision may predate libghostty-vt\n\
         entirely (PR ghostty-org/ghostty#8840), or the upstream build\n\
         surface changed again.\n\n\
         `zig build -h` output:\n{help}\n\n\
         To work around: bump GHOSTTY_REV in crates/con-ghostty/build.rs,\n\
         or set CON_GHOSTTY_VT_STEP to an exact step name from the list\n\
         above.\n\
         ========================================================\n",
        help = help_text,
    );
}

// ── Shared helpers ─────────────────────────────────────────────────────

fn run_zig_command_status(
    zig_bin: &OsStr,
    dir: &Path,
    zig_global_cache_dir: Option<&Path>,
    args: &[String],
) -> std::io::Result<std::process::ExitStatus> {
    let mut cmd = Command::new(zig_bin);
    configure_zig_command(&mut cmd, zig_global_cache_dir);
    cmd.args(args).current_dir(dir).status()
}

fn zig_build_help_text(
    zig_bin: &OsStr,
    ghostty_dir: &Path,
    zig_global_cache_dir: Option<&Path>,
) -> String {
    let args = vec!["build".to_string(), "-h".to_string()];

    for attempt in 0..2 {
        let mut cmd = Command::new(zig_bin);
        configure_zig_command(&mut cmd, zig_global_cache_dir);
        let output = cmd.args(&args).current_dir(ghostty_dir).output();

        let out = match output {
            Ok(out) => out,
            Err(err) => {
                panic!("failed to run `zig build -h` in Ghostty checkout: {err}");
            }
        };

        let mut text = String::new();
        text.push_str(&String::from_utf8_lossy(&out.stdout));
        text.push_str(&String::from_utf8_lossy(&out.stderr));

        if out.status.success() {
            return text;
        }

        if attempt == 0 {
            println!(
                "cargo:warning=`zig build -h` failed while probing libghostty-vt; prefetching Zig package cache and retrying"
            );
            prefetch_zig_dependencies(zig_bin, ghostty_dir, zig_global_cache_dir);
            continue;
        }

        panic!(
            "\n========================================================\n\
             con-ghostty: `zig build -h` failed while probing the\n\
             libghostty-vt build surface, even after Zig package-cache\n\
             prefetch. This usually means Zig could not resolve an\n\
             upstream package dependency, not that libghostty-vt is\n\
             missing from the pinned Ghostty checkout.\n\n\
             `zig build -h` output:\n{help}\n\
             ========================================================\n",
            help = text,
        );
    }

    unreachable!("two-attempt zig build help loop must return or panic")
}

fn ghostty_vt_zig_target() -> Option<String> {
    if let Some(target) = env::var_os(GHOSTTY_VT_TARGET_ENV) {
        let target = target.to_string_lossy().trim().to_string();
        if !target.is_empty() {
            return Some(target);
        }
    }

    let cargo_target = env::var("TARGET").ok()?;
    cargo_target_to_zig_target(&cargo_target).map(str::to_string)
}

fn cargo_target_to_zig_target(cargo_target: &str) -> Option<&'static str> {
    match cargo_target {
        "x86_64-unknown-linux-gnu" => Some("x86_64-linux-gnu"),
        "aarch64-unknown-linux-gnu" => Some("aarch64-linux-gnu"),
        "x86_64-unknown-linux-musl" => Some("x86_64-linux-musl"),
        "aarch64-unknown-linux-musl" => Some("aarch64-linux-musl"),
        "x86_64-pc-windows-msvc" => Some("x86_64-windows-msvc"),
        "aarch64-pc-windows-msvc" => Some("aarch64-windows-msvc"),
        "x86_64-pc-windows-gnu" => Some("x86_64-windows-gnu"),
        "x86_64-pc-windows-gnullvm" => Some("x86_64-windows-gnu"),
        "aarch64-pc-windows-gnullvm" => Some("aarch64-windows-gnu"),
        _ => None,
    }
}

fn resolve_ghostty_source() -> PathBuf {
    if let Some(source_dir) = env::var_os(GHOSTTY_ENV) {
        return PathBuf::from(source_dir)
            .canonicalize()
            .expect("CON_GHOSTTY_SOURCE_DIR points to a missing directory");
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
    let vendor_root = out_dir.join("ghostty-src");

    if vendor_root.exists() && current_git_rev(&vendor_root).as_deref() == Some(GHOSTTY_REV) {
        return vendor_root;
    }

    if vendor_root.exists() {
        fs::remove_dir_all(&vendor_root).expect("failed to clear stale vendored ghostty source");
    }

    run(
        Command::new("git").args([
            "clone",
            "--no-checkout",
            GHOSTTY_REPO,
            vendor_root.to_str().expect("non-utf8 ghostty vendor path"),
        ]),
        "failed to clone Ghostty source",
    );
    run(
        Command::new("git")
            .args(["checkout", GHOSTTY_REV])
            .current_dir(&vendor_root),
        "failed to checkout pinned Ghostty revision",
    );

    vendor_root
}

fn patchable_ghostty_source(source: PathBuf) -> PathBuf {
    if env::var_os(GHOSTTY_ENV).is_none() {
        return source;
    }

    // Never patch a caller-provided checkout in place. `CON_GHOSTTY_SOURCE_DIR`
    // is for local upstream study; Con's embedding extension is applied to a
    // throwaway copy under OUT_DIR so `3pp/` and user checkouts stay read-only.
    println!("cargo:rerun-if-changed={}", source.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        source.join("include/ghostty.h").display()
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
    let patched = out_dir.join("ghostty-src-patched-env");
    if patched.exists() {
        fs::remove_dir_all(&patched).expect("failed to clear patched Ghostty source copy");
    }
    copy_dir_filtered(&source, &patched).unwrap_or_else(|err| {
        panic!(
            "failed to copy CON_GHOSTTY_SOURCE_DIR from {} to {}: {err}",
            source.display(),
            patched.display()
        )
    });
    patched
}

fn copy_dir_filtered(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if matches!(name.as_ref(), ".git" | ".zig-cache" | "zig-out" | "target") {
            continue;
        }

        let src_path = entry.path();
        let dst_path = dst.join(&file_name);
        let metadata = fs::symlink_metadata(&src_path)?;
        if metadata.is_dir() {
            copy_dir_filtered(&src_path, &dst_path)?;
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&src_path)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &dst_path)?;
            #[cfg(windows)]
            {
                if src_path.is_dir() {
                    std::os::windows::fs::symlink_dir(target, &dst_path)?;
                } else {
                    std::os::windows::fs::symlink_file(target, &dst_path)?;
                }
            }
        } else if metadata.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn apply_embedded_initial_output_patch(ghostty_dir: &Path) -> bool {
    if let Err(err) = try_apply_embedded_initial_output_patch(ghostty_dir) {
        if env_flag_enabled(GHOSTTY_INITIAL_OUTPUT_REQUIRE_ENV) {
            panic!(
                "con-ghostty: embedded initial_output restore hook is required but could not be applied: {err}\n\
                 Rebase Con's Ghostty embedding patch against GHOSTTY_REV={GHOSTTY_REV}, or unset \
                 {GHOSTTY_INITIAL_OUTPUT_REQUIRE_ENV} for a local best-effort build."
            );
        }
        println!("cargo:warning=con-ghostty: embedded initial_output restore hook disabled: {err}");
        return false;
    }

    true
}

fn try_apply_embedded_initial_output_patch(ghostty_dir: &Path) -> Result<(), String> {
    let embedded = ghostty_dir.join("src/apprt/embedded.zig");
    let surface = ghostty_dir.join("src/Surface.zig");
    let exec = ghostty_dir.join("src/termio/Exec.zig");
    let header = ghostty_dir.join("include/ghostty.h");

    // Prepare every source change in memory before writing anything. A Ghostty
    // bump can invalidate any one of these anchors; validating the complete
    // patch first prevents an anchor mismatch from leaving the cached checkout
    // in a partially patched, internally inconsistent state.
    let embedded_original = read_file_text(&embedded)?;
    let surface_original = read_file_text(&surface)?;
    let exec_original = read_file_text(&exec)?;
    let header_original = read_file_text(&header)?;
    let mut embedded_text = embedded_original.clone();
    let mut surface_text = surface_original.clone();
    let mut exec_text = exec_original.clone();
    let mut header_text = header_original.clone();

    patch_text_once(
        &embedded,
        &mut embedded_text,
        "initial_output: ?[*:0]const u8",
        &[(
            "        /// Input to send to the command after it is started.\n        initial_input: ?[*:0]const u8 = null,\n\n        /// Wait after the command exits\n",
            "        /// Input to send to the command after it is started.\n        initial_input: ?[*:0]const u8 = null,\n\n        /// Output to seed into the terminal state before the command starts.\n        /// This is an embedding-only hook used for visual scrollback restore;\n        /// it is parsed as terminal output and is never written to the pty.\n        initial_output: ?[*:0]const u8 = null,\n\n        /// Wait after the command exits\n",
        )],
    )?;
    patch_text_once(
        &embedded,
        &mut embedded_text,
        "initial_output_restore = if (opts.initial_output)",
        &[(
            "        // Initialize our surface right away. We're given a view that is\n        // ready to use.\n        try self.core_surface.init(\n            app.core_app.alloc,\n            &config,\n            app.core_app,\n            app,\n            self,\n        );\n",
            "        // Initialize our surface right away. We're given a view that is\n        // ready to use. `initial_output_restore` is parsed into Ghostty's\n        // terminal state before the child process starts.\n        const initial_output_restore = if (opts.initial_output) |c_output|\n            std.mem.sliceTo(c_output, 0)\n        else\n            null;\n        try self.core_surface.init(\n            app.core_app.alloc,\n            &config,\n            app.core_app,\n            app,\n            self,\n            initial_output_restore,\n        );\n",
        )],
    )?;
    patch_text_once(
        &embedded,
        &mut embedded_text,
        "Con: macOS may deny directory open/stat preflight for privacy-protected cwd",
        &[(
            "            const wd = std.mem.sliceTo(c_wd, 0);\n            if (wd.len > 0) wd: {\n",
            "            const wd = std.mem.sliceTo(c_wd, 0);\n            if (wd.len > 0) wd: {\n                if (comptime builtin.os.tag.isDarwin()) {\n                    // Con: macOS may deny directory open/stat preflight for privacy-protected cwd\n                    // (Documents, Downloads) even though chdir in the spawned shell succeeds. The\n                    // embedder already passes an absolute cwd captured from shell integration, so\n                    // trust it here and let process spawn be the authoritative failure boundary.\n                    var wd_val: configpkg.WorkingDirectory = .{ .path = wd };\n                    if (wd_val.finalize(config.arenaAlloc())) |_| {\n                        config.@\"working-directory\" = wd_val;\n                    } else |err| {\n                        log.warn(\n                            \"error finalizing working directory config dir={s} err={}\",\n                            .{ wd_val.path, err },\n                        );\n                    }\n                    break :wd;\n                }\n\n",
        )],
    )?;
    patch_text_once(
        &surface,
        &mut surface_text,
        "initial_output_restore: ?[]const u8",
        &[(
            "    rt_surface: *apprt.runtime.Surface,\n) !void {\n",
            "    rt_surface: *apprt.runtime.Surface,\n    initial_output_restore: ?[]const u8,\n) !void {\n",
        )],
    )?;
    patch_text_once(
        &exec,
        &mut exec_text,
        "Con: trust macOS cwd after embedded surface validation",
        &[(
            "            if (std.Io.Dir.cwd().access(global.io(), proposed, .{})) {\n                break :cwd proposed;\n            } else |err| {\n                log.warn(\"cannot access cwd, ignoring: {}\", .{err});\n                break :cwd null;\n            }\n",
            "            if (comptime builtin.os.tag.isDarwin()) {\n                // Con: trust macOS cwd after embedded surface validation. Privacy-protected\n                // directories can fail access/open preflight while the child shell can still\n                // chdir there and preserve the user's restored working directory.\n                break :cwd proposed;\n            }\n\n            if (std.Io.Dir.cwd().access(global.io(), proposed, .{})) {\n                break :cwd proposed;\n            } else |err| {\n                log.warn(\"cannot access cwd, ignoring: {}\", .{err});\n                break :cwd null;\n            }\n",
        )],
    )?;
    patch_text_once(
        &surface,
        &mut surface_text,
        "This keeps restored text in Ghostty's",
        &[(
            "    // Start our IO thread\n    self.io_thr = try std.Thread.spawn(\n",
            "    // Seed restored output after the renderer is alive but before the IO\n    // thread starts the child process. This keeps restored text in Ghostty's\n    // own terminal screen/scrollback layer without ever feeding it to the shell.\n    if (initial_output_restore) |initial_output| {\n        if (initial_output.len > 0) self.io.processOutput(initial_output);\n    }\n\n    // Start our IO thread\n    self.io_thr = try std.Thread.spawn(\n",
        )],
    )?;
    replace_text_if_present(
        &mut surface_text,
        "    if (opts.initial_output) |c_output| {\n        const initial_output = std.mem.sliceTo(c_output, 0);\n        if (initial_output.len > 0) self.io.processOutput(initial_output);\n    }\n",
        "    if (initial_output_restore) |initial_output| {\n        if (initial_output.len > 0) self.io.processOutput(initial_output);\n    }\n",
    );

    patch_text_once(
        &header,
        &mut header_text,
        "const char* initial_output;",
        &[(
            "  const char* initial_input;\n  bool wait_after_command;\n",
            "  const char* initial_input;\n  const char* initial_output;\n  bool wait_after_command;\n",
        )],
    )?;

    write_patch_files_with_rollback(&[
        (
            &embedded,
            embedded_original.as_str(),
            embedded_text.as_str(),
        ),
        (&surface, surface_original.as_str(), surface_text.as_str()),
        (&exec, exec_original.as_str(), exec_text.as_str()),
        (&header, header_original.as_str(), header_text.as_str()),
    ])?;

    println!("cargo:rustc-cfg=con_ghostty_embedded_initial_output");
    println!("cargo:rerun-if-changed={}", embedded.display());
    println!("cargo:rerun-if-changed={}", surface.display());
    println!("cargo:rerun-if-changed={}", exec.display());
    println!("cargo:rerun-if-changed={}", header.display());
    Ok(())
}

fn read_file_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

fn patch_text_once(
    path: &Path,
    text: &mut String,
    marker: &str,
    replacements: &[(&str, &str)],
) -> Result<(), String> {
    if text.contains(marker) {
        return Ok(());
    }

    for (from, to) in replacements {
        if !text.contains(from) {
            return Err(format!(
                "failed to patch {}: expected anchor not found while applying Con embedding extension",
                path.display()
            ));
        }
        *text = text.replacen(from, to, 1);
    }

    Ok(())
}

fn replace_text_if_present(text: &mut String, from: &str, to: &str) {
    if text.contains(from) {
        *text = text.replace(from, to);
    }
}

fn write_file_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension(format!("con-patch-{}.tmp", std::process::id()));
    fs::write(&tmp, text).map_err(|err| format!("failed to write {}: {err}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("failed to replace {}: {err}", path.display())
    })
}

fn write_patch_files_with_rollback(files: &[(&Path, &str, &str)]) -> Result<(), String> {
    let mut replaced = 0;
    for (path, _, patched) in files {
        if let Err(write_err) = write_file_atomic(path, patched) {
            let mut rollback_errors = Vec::new();
            for (rollback_path, original, _) in files[..replaced].iter().rev() {
                if let Err(err) = write_file_atomic(rollback_path, original) {
                    rollback_errors.push(err);
                }
            }
            if !rollback_errors.is_empty() {
                panic!(
                    "con-ghostty: Ghostty patch write failed and rollback could not restore the \
                     cached source tree. Refusing to continue with mixed source files. Write error: \
                     {write_err}. Rollback error(s): {}",
                    rollback_errors.join("; ")
                );
            }
            return Err(write_err);
        }
        replaced += 1;
    }
    Ok(())
}

fn require_zig_version(zig_bin: &OsStr, zig_global_cache_dir: Option<&Path>, target_os: &str) {
    let mut command = Command::new(zig_bin);
    configure_zig_command(&mut command, zig_global_cache_dir);
    let output = command.arg("version").output().unwrap_or_else(|err| {
        let path = env::var("PATH").unwrap_or_default();
        panic!(
            "\n\n========================================================\n\
             con-ghostty: could not spawn `{bin} version`: {err}\n\n\
             Zig {required} exactly is required to build Ghostty for {target_os}.\n\
             Install it from https://ziglang.org/download/ and ensure the `zig`\n\
             executable is on PATH, or set CON_ZIG_BIN to its absolute path.\n\n\
             Current PATH: {path}\n\n\
             ========================================================\n",
            bin = zig_bin.to_string_lossy(),
            required = REQUIRED_ZIG_VERSION,
        )
    });
    if !output.status.success() {
        panic!(
            "con-ghostty: `{}` version failed with status {}: {}",
            zig_bin.to_string_lossy(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version != REQUIRED_ZIG_VERSION {
        panic!(
            "con-ghostty: Zig {REQUIRED_ZIG_VERSION} exactly is required for {target_os}, \
             but `{}` reports {version}. Set CON_ZIG_BIN to the validated Zig executable.",
            zig_bin.to_string_lossy()
        );
    }
    println!(
        "cargo:warning=using zig {version} (from `{}`)",
        zig_bin.to_string_lossy()
    );
}

fn current_git_rev(repo_dir: &PathBuf) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}

fn run(command: &mut Command, context: &str) {
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("{context}: {err}"));
    if !status.success() {
        panic!("{context}");
    }
}

fn zig_global_cache_dir(target_os: &str) -> Option<PathBuf> {
    if let Some(dir) = env::var_os("ZIG_GLOBAL_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }

    if target_os != "windows" {
        return None;
    }

    let candidates = [PathBuf::from(r"C:\zc"), env::temp_dir().join("zc")];
    for candidate in candidates {
        if fs::create_dir_all(&candidate).is_ok() && writable_dir(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn writable_dir(path: &Path) -> bool {
    let probe = path.join(".con_ghostty_write_test");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe);
    match file {
        Ok(mut file) => {
            let write_ok = file.write_all(b"ok").is_ok();
            drop(file);
            let _ = fs::remove_file(&probe);
            write_ok
        }
        Err(_) => false,
    }
}

fn configure_zig_command(command: &mut Command, zig_global_cache_dir: Option<&std::path::Path>) {
    if let Some(dir) = zig_global_cache_dir {
        command.env("ZIG_GLOBAL_CACHE_DIR", dir);
    }
}

fn prefetch_zig_dependencies(zig_bin: &OsStr, root: &Path, zig_global_cache_dir: Option<&Path>) {
    let Some(cache_dir) = resolve_zig_global_cache_dir(zig_bin, zig_global_cache_dir) else {
        println!("cargo:warning=con-ghostty: could not resolve Zig global cache dir for prefetch");
        return;
    };

    let mut zon_queue = VecDeque::new();
    collect_zon_files(root, &mut zon_queue);

    let mut seen_zon_files = HashSet::new();
    let mut seen_urls = HashSet::new();
    let mut fetched = 0usize;
    let mut failed = 0usize;

    while let Some(zon_file) = zon_queue.pop_front() {
        if !seen_zon_files.insert(zon_file.clone()) {
            continue;
        }

        let Ok(text) = fs::read_to_string(&zon_file) else {
            continue;
        };

        for url in extract_zon_urls(&text) {
            if !seen_urls.insert(url.clone()) {
                continue;
            }
            if seen_urls.len() > MAX_PREFETCH_PACKAGES {
                println!(
                    "cargo:warning=con-ghostty: refusing to prefetch more than \
                     {MAX_PREFETCH_PACKAGES} unique Zig packages"
                );
                failed += 1;
                zon_queue.clear();
                break;
            }

            match prefetch_zig_url(zig_bin, root, &url, zig_global_cache_dir) {
                Some(package_hash) => {
                    fetched += 1;
                    let package_root = cache_dir.join("p").join(package_hash);
                    collect_zon_files(&package_root, &mut zon_queue);
                }
                None => {
                    failed += 1;
                    println!("cargo:warning=con-ghostty: failed to prefetch Zig package {url}");
                }
            }
        }
    }

    println!("cargo:warning=con-ghostty: prefetched {fetched} Zig package(s), {failed} failed");
}

fn collect_zon_files(root: &Path, out: &mut VecDeque<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".git" | ".zig-cache" | "zig-out" | "target") {
            continue;
        }

        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_zon_files(&path, out);
        } else if metadata.is_file() && name == "build.zig.zon" {
            out.push_back(path);
        }
    }
}

fn extract_zon_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with("//") {
            continue;
        }
        let Some(url_field) = line.find(".url") else {
            continue;
        };
        let line = &line[url_field..];
        let Some(start) = line.find('"') else {
            continue;
        };
        let rest = &line[start + 1..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        let url = &rest[..end];
        if url.starts_with("https://")
            || url.starts_with("http://")
            || url.starts_with("git+https://")
            || url.starts_with("git+http://")
        {
            urls.push(url.to_string());
        }
    }
    urls
}

fn prefetch_zig_url(
    zig_bin: &OsStr,
    project_root: &Path,
    url: &str,
    zig_global_cache_dir: Option<&Path>,
) -> Option<String> {
    if let Some((repo_url, revision)) = parse_git_package_url(url) {
        return prefetch_zig_git_url(
            zig_bin,
            project_root,
            &repo_url,
            &revision,
            zig_global_cache_dir,
        );
    }

    let package_name = url.rsplit('/').next().unwrap_or("package.tar.gz");
    let tmp = env::temp_dir().join(format!(
        "con-ghostty-zig-fetch-{}-{}",
        std::process::id(),
        sanitize_filename(package_name)
    ));

    let curl_status = Command::new("curl")
        .args([
            "-fL",
            "--retry",
            "3",
            "--retry-delay",
            "1",
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
            "--speed-limit",
            "1024",
            "--speed-time",
            "30",
            "--max-filesize",
            MAX_PREFETCH_ARCHIVE_BYTES,
            url,
            "-o",
        ])
        .arg(&tmp)
        .status();
    match curl_status {
        Ok(status) if status.success() => {
            let hash = zig_fetch_path(zig_bin, project_root, &tmp, zig_global_cache_dir);
            let _ = fs::remove_file(&tmp);
            hash
        }
        Ok(status) => {
            let _ = fs::remove_file(&tmp);
            println!("cargo:warning=con-ghostty: curl exited with status {status} for {url}");
            None
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            println!("cargo:warning=con-ghostty: failed to run curl for {url}: {err}");
            None
        }
    }
}

fn parse_git_package_url(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("git+https://")
        .map(|rest| format!("https://{rest}"))
        .or_else(|| {
            url.strip_prefix("git+http://")
                .map(|rest| format!("http://{rest}"))
        })?;
    let (repo_url, revision) = rest.rsplit_once('#')?;
    if repo_url.is_empty() || revision.is_empty() {
        return None;
    }
    Some((repo_url.to_string(), revision.to_string()))
}

fn prefetch_zig_git_url(
    zig_bin: &OsStr,
    project_root: &Path,
    repo_url: &str,
    revision: &str,
    zig_global_cache_dir: Option<&Path>,
) -> Option<String> {
    let tmp_root = env::temp_dir().join(format!(
        "con-ghostty-zig-git-{}-{}",
        std::process::id(),
        sanitize_filename(revision)
    ));
    let checkout = tmp_root.join("checkout");
    let archive = tmp_root.join("package.tar.gz");

    let result = (|| {
        fs::create_dir_all(&tmp_root).ok()?;
        fs::create_dir_all(&checkout).ok()?;
        run_status(
            Command::new("git")
                .arg("init")
                .arg("--quiet")
                .arg(&checkout),
        )?;
        run_status(
            Command::new("git")
                .args(["remote", "add", "origin", repo_url])
                .current_dir(&checkout),
        )?;
        run_status(
            Command::new("git")
                .args(["fetch", "--depth", "1", "origin", revision])
                .current_dir(&checkout),
        )?;
        run_status(
            Command::new("git")
                .args(["checkout", "--detach", "FETCH_HEAD"])
                .current_dir(&checkout),
        )?;
        run_status(
            Command::new("git")
                .args(["archive", "--format=tar.gz", "-o"])
                .arg(&archive)
                .arg("HEAD")
                .current_dir(&checkout),
        )?;
        zig_fetch_path(zig_bin, project_root, &archive, zig_global_cache_dir)
    })();

    let _ = fs::remove_dir_all(&tmp_root);
    result
}

fn run_status(command: &mut Command) -> Option<()> {
    command.status().ok().filter(|status| status.success())?;
    Some(())
}

fn zig_fetch_path(
    zig_bin: &OsStr,
    project_root: &Path,
    path: &Path,
    zig_global_cache_dir: Option<&Path>,
) -> Option<String> {
    let mut cmd = Command::new(zig_bin);
    configure_zig_command(&mut cmd, zig_global_cache_dir);
    // Zig 0.16 resolves fetch configuration through the surrounding build
    // project even without `--save`, so invoke it from Ghostty's root.
    let output = cmd
        .arg("fetch")
        .arg(path)
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!(
            "cargo:warning=con-ghostty: zig fetch {} failed: {stderr}",
            path.display()
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn resolve_zig_global_cache_dir(zig_bin: &OsStr, configured_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = configured_dir {
        return Some(dir.to_path_buf());
    }
    if let Some(dir) = env::var_os("ZIG_GLOBAL_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }

    let output = Command::new(zig_bin).arg("env").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if !line.starts_with(".global_cache_dir") {
            continue;
        }
        let Some(start) = line.find('"') else {
            continue;
        };
        let rest = &line[start + 1..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        return Some(PathBuf::from(&rest[..end]));
    }
    None
}

fn sanitize_filename(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{cargo_target_to_zig_target, extract_zon_urls, parse_git_package_url};

    #[test]
    fn maps_linux_targets_to_generic_zig_targets() {
        assert_eq!(
            cargo_target_to_zig_target("x86_64-unknown-linux-gnu"),
            Some("x86_64-linux-gnu")
        );
        assert_eq!(
            cargo_target_to_zig_target("aarch64-unknown-linux-gnu"),
            Some("aarch64-linux-gnu")
        );
    }

    #[test]
    fn maps_windows_targets_to_generic_zig_targets() {
        assert_eq!(
            cargo_target_to_zig_target("x86_64-pc-windows-msvc"),
            Some("x86_64-windows-msvc")
        );
        assert_eq!(
            cargo_target_to_zig_target("x86_64-pc-windows-gnullvm"),
            Some("x86_64-windows-gnu")
        );
        assert_eq!(
            cargo_target_to_zig_target("aarch64-pc-windows-msvc"),
            Some("aarch64-windows-msvc")
        );
    }

    #[test]
    fn leaves_unknown_targets_unmodified() {
        assert_eq!(cargo_target_to_zig_target("wasm32-unknown-unknown"), None);
    }

    #[test]
    fn extracts_package_urls_from_zon_text_without_comments() {
        let zon = r#"
        .{
            .dependencies = .{
                .real = .{
                    .url = "https://deps.files.ghostty.org/package.tar.gz",
                    .hash = "package-0.1.0-abc",
                },
                // .url = "https://example.com/commented-out.tar.gz",
                .inline_comment = .{ .url = "https://github.com/example/project.tar.gz", // keep
                },
            },
        }
        "#;

        assert_eq!(
            extract_zon_urls(zon),
            vec![
                "https://deps.files.ghostty.org/package.tar.gz".to_string(),
                "https://github.com/example/project.tar.gz".to_string(),
            ]
        );
    }

    #[test]
    fn parses_git_package_urls() {
        assert_eq!(
            parse_git_package_url(
                "git+https://github.com/jacobsandlund/uucode#5f05f8f83a75caea201f12cc8ea32a2d82ea9732"
            ),
            Some((
                "https://github.com/jacobsandlund/uucode".to_string(),
                "5f05f8f83a75caea201f12cc8ea32a2d82ea9732".to_string(),
            ))
        );
        assert_eq!(
            parse_git_package_url("https://example.com/pkg.tar.gz"),
            None
        );
    }
}
