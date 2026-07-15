# con-test: Logic Test Framework

## Overview

`con-test` is a sqllogictest-inspired E2E test framework for con. It launches a real
con process, drives it via `con-cli` over the local socket, and verifies output using
plain-text `.test` files that inline both the command and the expected output.

## Design decisions

### One process, serial execution, state reset between files

Each `con-test` run launches exactly one con process. Test files run serially. Before
each file, `reset_state()` closes all extra tabs and panes so every file starts from a
known baseline: one tab, one pane, fresh shell.

This is simpler and more reliable than per-file process isolation (which would require
a headless con mode that doesn't exist yet) and avoids the complexity of parallel
execution against a shared process.

### .test file format

```
# comment

con-cli <arguments...>       # preferred
# Legacy: old tests may still use `cmd ...`
---- <mode>
expected output here
```

The `----` line carries the match mode inline. A blank line or the next step directive ends the
expected block.

**Match modes:**

| Mode          | Behaviour |
|---------------|-----------|
| `contains`    | actual output contains the expected string (default when `----` has no mode) |
| `exact`       | full string equality, trailing whitespace trimmed per line |
| `json-subset` | every key/value in expected JSON exists in actual JSON (ignores extra fields) |
| `ok`          | only checks exit code == 0; expected block ignored |
| `error`       | checks exit code != 0; expected block matched against stderr |
| `regex`       | not yet implemented |

**Example:**

```
# tabs/basic.test

con-cli --json tabs list  # at least one tab exists
---- json-subset
{"tabs":[{"index":1}]}

con-cli tabs new
---- ok

con-cli --json tabs list
---- contains
"index":2

con-cli --json tabs close --tab 2
---- ok
```

### json-subset semantics

`json-subset` recursively checks that every key/value in the expected JSON exists in
the actual JSON. Extra keys in actual are ignored. Arrays: every element in expected
must appear somewhere in actual (order-independent).

This is the right default for most con-cli assertions because the JSON responses
contain many fields (pane state, capabilities, etc.) that vary between runs.

### State reset

`reset_state()` runs before each `.test` file:

1. `reset_tabs` — closes all tabs with index > 1, one at a time, re-querying after
   each close so indices stay accurate.
2. `reset_panes` — sends Ctrl-D to extra panes in tab 1 until only one remains,
   keeping the pane with the lowest `pane_id`.
3. `reset_surfaces` — closes extra pane-local surfaces in the surviving pane,
   keeping the surface with the lowest `surface_id`.

### Binary resolution

The con app binary and `con-cli` are resolved in this order:

1. `--con` / `--con-cli` flag
2. `CON` / `CON_CLI` environment variable
3. Sibling binary in the same `target/` directory as `con-test`
4. `PATH`

The default app binary name is platform-aware: `con` on Unix and `con-app` on
Windows, because `CON` is a reserved DOS device name. When running from the
workspace (`cargo build && ./target/debug/con-test ...`), step 3 picks up the
freshly built binaries automatically.

### Control endpoint

If `--socket` is not provided, `con-test` creates an isolated endpoint name for
the launched app:

- Unix: a temp Unix socket path like `/tmp/con-test-<pid>.sock`
- Windows: a named pipe path like `\\.\pipe\con-test-<pid>`

Readiness is detected by attempting to connect to the endpoint, not by checking
for a filesystem path. This keeps startup detection portable across Unix sockets
and Windows named pipes.

## Repository layout

```
crates/con-test/
├── Cargo.toml
├── README.md
├── src/
│   ├── main.rs      — CLI entry point, binary resolution, file collection, result printing
│   ├── parser.rs    — .test file parser, MatchMode, shell_split
│   └── runner.rs    — ConProcess RAII guard, reset_state, run_file, step execution
└── testdata/
    ├── system/
    │   ├── identify.test   — system.identify and capabilities smoke tests
    │   └── tree.test       — workspace tree structure
    ├── tabs/
    │   ├── basic.test      — tab list / new / close lifecycle
    │   └── rename.test     — tab user_label field assertions
    └── panes/
        ├── basic.test      — pane list field assertions
        ├── exec.test       — pane read and capability checks
        └── split.test      — pane create (split right) and layout
```

## Running locally

```bash
# Build everything
cargo build -p con -p con-cli -p con-test

# Run all tests (con launched automatically)
./target/debug/con-test crates/con-test/testdata/

# Run a single file
./target/debug/con-test crates/con-test/testdata/tabs/basic.test

# Baseline mode — write actual output as new expected values
./target/debug/con-test --rewrite crates/con-test/testdata/

# Stop on first failure
./target/debug/con-test --fail-fast crates/con-test/testdata/

# Verbose — show pass/skip per step
./target/debug/con-test --verbose crates/con-test/testdata/
```

## CI

The `.github/workflows/e2e.yml` workflow runs on `macos-15` on every push/PR that
touches con-test, con-app, con-core, con-cli, or con-ghostty.

Steps:
1. Build `con`, `con-cli`, `con-test`
2. Run `cargo test -p con-test` (parser unit tests, no live process needed)
3. Run `./target/debug/con-test crates/con-test/testdata/`
4. Upload `/tmp/con-e2e.log` as an artifact on failure

## Known limitations

- `panes exec` requires shell integration (`exec_visible_shell` capability) which is
  not available immediately after con starts. Tests that need exec should wait for
  shell integration or use `panes send-keys` + `panes read` instead.
- `panes wait` reads terminal scrollback which may contain output from previous
  sessions if con restored terminal text. Tests should use unique sentinel strings
  (e.g. `CON_TEST_<name>_OK`) to avoid false matches.
- No variable support in `.test` files — `pane_id` and other dynamic values cannot
  be captured and reused across steps. Use `--pane-id` only when the ID is stable
  (e.g. always 0 for the first pane), or omit it to target the active pane.
- Parallel execution is not supported. All files run serially against one con process.

## Adding tests

1. Create a `.test` file under `testdata/` in the appropriate subdirectory.
2. Write `con-cli` / `---- <mode>` / expected blocks.
3. If unsure about exact output, run with `--rewrite` to generate the baseline:
   ```bash
   ./target/debug/con-test --rewrite crates/con-test/testdata/my-new-test.test
   ```
4. Review the diff, commit both the `.test` file and the generated expected values.
