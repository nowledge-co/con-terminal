# Windows release blocked by Cargo's intermediate layout

## What happened

The beta.178 Windows release failed before compilation with OS error 267 while
creating `target/release/build/con/<hash>/out`. macOS and Linux builds were not
affected by this Windows path restriction. Publication remained gated.

## Root cause

The floating stable toolchain advanced from Rust 1.99.1 to 1.100. Cargo's new
intermediate layout uses a bare package-name component. Our package is `con`;
Win32 reserves that name. The existing `con-app.exe` alias and short checkout
path protect different paths and cannot prevent this new directory.

## Fix

Pin Windows CI and releases to Rust 1.99.1, leaving other platforms unchanged.
This restores the compatible intermediate layout without renaming the Cargo
package or changing application identities. Existing tags remain immutable.

## Verification and follow-up

Run actionlint and the repository workflow-policy tests on both workflows.
Confirm the old intermediate layout using a dependency-free build-script fixture
on Windows. Full application builds remain verified by the Windows release job.
Remove the pin only after a linked Windows app build confirms Cargo escapes
reserved package names, or a separately reviewed package-name migration makes
the intermediate path safe. Do not assume a safe executable name implies safe
Cargo-generated paths.

Cargo documents the 1.100 layout change in its
[build cache reference](https://doc.rust-lang.org/cargo/reference/build-cache.html).
