# Program status acceptance

These fixtures exercise the real terminal path. They need only Python 3 and
do not read configuration, credentials, or session history. Run them inside a
disposable Con window, not by piping their output into a log collector.

## Lifecycle

```sh
python3 scripts/terminal/program-status-fixture.py lifecycle --seconds 3 --alternate-screen
```

Watch the tab or sidebar marker through working, a blocked child under an idle
root, clearing that child, working again, done, and error. The error survives
an alternate-screen change and process exit. Click another pane and type there:
that must not acknowledge this surface's error. Return to the emitting pane and
press a key: the unseen error marker should clear. Merely focusing it is not
acknowledgement. Tool-driven raw PTY writes intentionally do not acknowledge it.

For tmux, enable `set -g allow-passthrough on` and add `--tmux`. Capability replies
have additional active-pane limitations; see [Program status](../../docs/program-status.md).

Repeat in light/dark themes, compact/expanded tabs, and UI font sizes 12, 16,
and 24. Check the hover details, not just the icon. Native linked tests cover
malformed reports, prompt/reset behavior, and bounded record retention; this
fixture complements those tests rather than replacing them.

## Sustained output

```sh
python3 scripts/terminal/program-status-fixture.py load --seconds 30 --rate 120 --mode baseline
python3 scripts/terminal/program-status-fixture.py load --seconds 30 --rate 120 --mode status
```

Both conditions emit the same visible lines and equal-length escape sequences.
The baseline uses the unrecognized OSC 7502 opcode instead of OSC 7501. Start
from cleared status in each condition; a baseline run does not erase records
left by earlier OSC 7501 reports. The status run clears its own record on exit.
Alternate the order across several runs to reduce warmup/order effects.

While output is active, type in another pane, scroll the emitting pane, and
resize the window. Confirm the latest progress appears and the final clear
removes the marker. Use a native profiler for frame/keypress latency, with the
same app build, window geometry, theme, font, display refresh rate and PTY load
in both conditions. Record those conditions with the results.

The final JSON reports **emitter timing only**. A rate of 120 reports/second does
not establish 120 rendered frames/second. Control-socket reply latency similarly
does not measure typing or painting. Do not turn either into a UI FPS claim.

## Reducer cost

```sh
cargo run --locked --release -p con-core --example program_status_bench
cargo test --locked -p con-core --example program_status_bench
python3 -m unittest discover -s scripts/terminal -p 'test_*.py'
```

The benchmark black-boxes inputs and observed state, verifies the final progress
and bounded storage, warms both workloads and alternates sample order. It compares
OSC 7501 reduction with the existing progress observation, not a no-work loop.
It excludes parsing, detail formatting, PTY I/O, UI layout and painting. Keep the
raw JSON and build profile when recording results; no timing threshold is a CI
correctness test.
