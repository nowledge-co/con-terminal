# Ghostty 1.4 integration

Tracker: [#526](https://github.com/nowledge-co/con-terminal/issues/526), with
security, build/ABI, bridge and native acceptance tasks #527–#530.

## Source and toolchain

The shared source pin is
[`wey-gu/ghostty@6e68c5f446eb929f03a17bf736b56ebc0d46731b`](https://github.com/wey-gu/ghostty/commit/6e68c5f446eb929f03a17bf736b56ebc0d46731b).
It is official Ghostty **1.4.0**, commit
`83fb95624b0439ee660c2679b43b38c8b63769cd`, plus the same four scoped OSC 7501
bridge commits carried by the previous pin and a two-line Zig 0.17 enum-builtin
port. No terminal security fixes are reimplemented in the fork.

The previous pin, `8b0ac6c4bc2b0977ef81f80d0c189b2e540f4077`, was based on
official `b115e456749e2820a14d3942a63159ff8d46d925`. The official-base comparison
contains 351 commits. The old pin already contained development-era features;
the release notes' performance comparisons with Ghostty 1.3 are not measured
incremental gains for con.

Both macOS full-libghostty and Windows/Linux libghostty-vt use this source pin.
Zig **0.17.0 exactly** is required. `mise.toml`, the build-script version check,
CI and release provisioning agree on that version. Download checksums come from
the official Zig download index. The default optimization spelling is `fast`;
existing `CON_GHOSTTY_OPTIMIZE=Debug`, `ReleaseFast`, `ReleaseSafe` and
`ReleaseSmall` overrides are translated to the Zig 0.17 spellings.

## Why the bridge remains

Official 1.4.0 and the official main source inspected on 2026-10-29 still ignore
`.program_status` in the full embedded stream handler. The upstream VT callback
alone does not provide macOS embedded support.

The public candidate preserves opt-in query replies, program reports, prompt
starts, full resets, bounded synchronous IO ingress, borrowed callback lifetime,
surface incarnation checks and report cleanup on close/shutdown. Upstream's newer
malformed-ID validation is retained. A 4,096-report burst test exercises the
ingress and clear/prompt/reset order.

[#444](https://github.com/nowledge-co/con-terminal/issues/444) remains the
official-upstream adoption and fork-removal tracker. This branch is not an
official upstream merge or submission. Replace it only when an official,
consumable revision contains the complete bridge and its lifetime contract;
repeat the ABI and native checks below when doing so. The upgrade does not
expand the scoped fork exception or change the GPUI dependency graph.

## Security applicability

The [1.4 release advisories](https://ghostty.org/docs/install/release-notes/1-4-0#security)
describe the Ghostty app as well as terminal-core defects. con's old development
pin is not equivalent to a released Ghostty 1.3.1 binary. Missing source fixes,
reachable paths and demonstrated impact must be kept distinct.

| Advisory | con boundary and old-pin assessment | Candidate / verification |
| --- | --- | --- |
| [w9q4-5jvv-mcv4](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-w9q4-5jvv-mcv4): cursor page pruned during clear | Shared terminal core; old `Screen.scrollClear` lacks the page-migration fix. Portable con normally sets 10,000 history lines, so the advisory's default-VT exposure is not proof of con's default exposure. | Includes `e257bb45e6d4d61c14eacdc717be89c6110f99ff` / upstream #14857. The two upstream style/hyperlink regressions crash on the old pin and pass on the candidate. A bounded Rust feed/snapshot regression uses a zero-history fixture to exercise page recycling. |
| [4hwh-wx3w-3fqg](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-4hwh-wx3w-3fqg): CR bypasses paste protection | Native clipboard and portable paste paths use upstream handling; old baseline predates #14687. IME/raw control-plane input is a different boundary. | Includes upstream #14687. Portable paste tests cover LF, CR and CRLF for clipboard and text sources, checking that rejection writes no PTY bytes. Bracketed paste remains supported. |
| [v8cw-fg4j-jcc4](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-v8cw-fg4j-jcc4): dragged text bypasses paste protection | con owns its GPUI clipboard/drop entry point; Ghostty's Swift fix does not automatically repair it. | Includes upstream #14679 and binds the new `ghostty_surface_paste_text` entry point. macOS clipboard/drop text now uses it; IME commits keep `ghostty_surface_text`. The existing confirmation callback fails closed, and denial does not acknowledge a program completion. |
| [mhff-mv4q-fp63](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-mhff-mv4q-fp63): Kitty bounds, paths and resource handling | Terminal core on enabled backends. The old development pin contains some earlier fixes; no blanket claim that every advisory subcase is reachable or absent. An image-storage limit alone does not prove old decoder allocation safety. | Official 1.4.0 is the advisory's patched baseline. Retain con's existing image/storage/placement limits and linked image regressions; no new graphics capability is enabled by this upgrade. |
| [hcr9-7hcr-mjm8](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-hcr9-7hcr-mjm8): tmux scroll regions | Relevant where Ghostty's tmux-control parser is enabled, not simply because con supports its own tmux surfaces. | Includes upstream #14680; no con-specific reproduction claimed. |
| [hp9c-qp4q-cpw3](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-hp9c-qp4q-cpw3): tmux pane dimensions | Same parser boundary; old baseline predates the new layout validation. | Includes upstream #14858 and terminal size validation; no con-specific reproduction claimed. |
| [2r63-vmw6-3m3c](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-2r63-vmw6-3m3c): shell directory escape injection | macOS packages upstream shell integration. Portable con also has its own integration and must not inherit an unsupported safety claim. | Packages the 1.4 resources with upstream #14859. Source review, not an end-to-end exploitation claim. |
| [cw69-wm67-5pv8](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-cw69-wm67-5pv8): OSC 8 URL opening | The advisory's Swift host opener is not con's host opener. con uses `terminal_url` and distinguishes OSC 8 from plain file links. | Preserve con's own URL policy and existing policy tests; do not claim that a library bump substitutes Ghostty's Swift confirmation UI. |
| [mfw9-xcr3-7gfp](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-mfw9-xcr3-7gfp): detected URL opening | con owns plain-link/file routing; its policy remains separate from the Ghostty app. | Upstream #14683 is in the baseline. Keep con's link handling and its tests; broader custom-scheme policy is not changed here. |
| [mxqx-236q-wrv7](https://github.com/ghostty-org/ghostty/security/advisories/GHSA-mxqx-236q-wrv7): execute files with control characters in paths | Ghostty's macOS app file-execution confirmation is not embedded in con. con's explicit execution flow remains its own boundary. | Upstream #14688 is in the baseline; no claim that this replaces con's execution approvals or validates every host filename flow. |

## ABI and behavior checks

`include/ghostty.h` adds the paste entry point; the existing bridge action tags,
runtime layout and union size still satisfy `ffi_abi.c`. Keep those compile-time
assertions and the linked Rust layout checks enabled.

The inspected VT headers remove `ghostty_terminal_vt_write_until_ground`, which
con did not bind. Existing consumed function declarations remain available;
the Windows/Linux runtime ABI-manifest test must still pass. The new render-state
image iterator is not adopted here: con retains its existing bounded placement
snapshot path, so it does not draw both old and new placement lists.

VT support-query replies now advertise accepted states and kinds after `?`,
preserving the query's BEL/ST terminator. The retained full embedded bridge
still sends the minimal support reply. Both signal support; neither reply is
user input or acknowledges a completed program. Keep the fragmented-query
regression aligned with the VT protocol rather than expecting a byte-for-byte
echo of the query.

Use the same immutable source for every check. Required evidence includes:

- linked con-ghostty tests and affected core/terminal/CLI/app tests;
- the upstream bridge tests and the old/new cursor-page regression comparison;
- real Windows and Linux VT builds/tests, including the ABI manifest, paste,
  image, selection, render-hold and OSC status tests;
- native Intel macOS linked tests, plus Apple Silicon app/socket interaction;
- isolated app checks for OSC queries/reports, clear/prompt/reset, surface close,
  resize, restore, Unicode and dark/light native presentation;
- clipboard/drop protection without changing IME or explicit command execution.

Do not count `CON_SKIP_GHOSTTY_VT`, a stub archive, or `cargo check` as linked or
runtime acceptance. Hardware/compositor and performance results must name the
actual environment; CI and local Apple Silicon checks do not certify every
Windows/Linux desktop or Intel macOS appearance.

### Local evidence (2026-10-29)

Host: macOS 15.7.9, Apple M3 Max, arm64. The normal pinned-source path was used
after publishing the candidate; the final con build/tests did not depend on
`CON_GHOSTTY_SOURCE_DIR`.

- Workspace tests: **1,289 passed, 1 existing ignored test**, including 717 app,
  35 macOS con-ghostty, 220 core and 16 terminal tests.
- Real con-test session: **27 assertions passed across 8 files** using an
  isolated home/configuration and control socket.
- Upstream `test-lib-vt`: **6,961 passed, 56 upstream skips** across the Zig and
  C API test binaries. These are native macOS VT checks, not Windows/Linux con
  renderer acceptance.
- Upstream filtered bridge tests passed; the focused clear suite passed **81
  tests**. The two cursor-page regressions crash on the old pin in an isolated
  fixture, checking style and hyperlink reference integrity.
- A raw-TTY probe received the exact OSC 7501 capability reply. User clipboard
  text containing CR outside bracketed paste produced **zero PTY bytes** and
  retained the completion marker. Safe text arrived unchanged and acknowledged
  completion. Bracketed multiline paste arrived with the expected delimiters
  and unchanged contents. No pasted text was executed as a shell command.
- Inspected native dark/light surfaces and sidebar visibility changes. The
  window continued rendering through the theme update. This is a visual smoke
  check, not a frame-rate, macOS 27 or hardware-matrix certification.

Windows/Linux con runtime ABI coverage and native Intel linked tests are
provided by the PR workflows. Record their exact-head results in the PR before
claiming cross-platform acceptance; they are not implied by the local results.

## Rollback

Revert the con integration commit as one unit: the source pin, Zig provisioning,
optimization adaptation, ABI use and paste call site belong together. The old
immutable pin remains available for diagnosis, but restoring it reintroduces
the missing security fixes and is not a long-term mitigation. Prefer a corrected
1.4-based candidate when repairing a platform regression. Do not leave a mixed
Zig/source pair or a platform-specific source pin behind.
