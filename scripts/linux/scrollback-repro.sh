#!/usr/bin/env bash
# Reproduce and check Linux scrollback scrolling (#497) in an isolated profile.
#
# Usage: scripts/linux/scrollback-repro.sh [path/to/con] [path/to/con-cli]
#
# The script starts the given Con build with its own config, data, cache,
# state and control socket under a new temporary directory, so it never
# touches your Con profile, your shell start-up files or your shell history.
# It fills the scrollback with numbered rows and walks you through each
# gesture. After each one, type the number of the lowest row you can see
# in Con; the script says PASS or FAIL and writes the results to the profile.
set -euo pipefail

con=${1:-target/debug/con}
cli=${2:-target/debug/con-cli}
rows=300

for bin in "$con" "$cli"; do
    if [[ ! -x $bin ]]; then
        echo "not executable: $bin (build with: cargo build -p con -p con-cli)" >&2
        exit 2
    fi
done
con=$(realpath -- "$con")
cli=$(realpath -- "$cli")

profile=$(mktemp -d "${TMPDIR:-/tmp}/con-scroll-repro.XXXXXX")
export XDG_CONFIG_HOME=$profile/config
export XDG_DATA_HOME=$profile/data
export XDG_CACHE_HOME=$profile/cache
export XDG_STATE_HOME=$profile/state
export CON_SOCKET_PATH=$profile/con.sock
export HISTFILE=$profile/bash_history
mkdir -p "$XDG_CONFIG_HOME/con" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME"
# At start-up Con runs `$SHELL -l -c env` and lets the login shell's values
# override its own environment. A login shell that exports XDG_* paths would
# send Con back to your real profile, so SHELL names a stub that prints the
# environment unchanged.
printf '#!/bin/sh\nexec /usr/bin/env\n' > "$profile/login-env-stub"
chmod +x "$profile/login-env-stub"
export SHELL=$profile/login-env-stub
# bash --norc still honours these from the environment. A custom prompt
# would hide the '$ ' the script waits for, PROMPT_COMMAND would run your
# code, readline would read ~/.inputrc, and LESS=-X would keep less off the
# alternate screen the last step tests.
export PS1='repro$ ' INPUTRC=/dev/null LESS=
unset PROMPT_COMMAND LESSOPEN LESSCLOSE LESSKEY
printf 'con.version = 1\ncommand = bash --noprofile --norc\n' > "$XDG_CONFIG_HOME/con/con.conf"
for ((i = 1; i <= rows; i++)); do printf 'scroll-repro row %03d\n' "$i"; done > "$profile/rows.txt"
results=$profile/results.txt
: > "$results"

con_pid=
cleanup() {
    if [[ -n $con_pid ]] && kill -0 "$con_pid" 2>/dev/null; then
        kill "$con_pid" 2>/dev/null || true
        wait "$con_pid" 2>/dev/null || true
    fi
    echo
    echo "Profile kept for inspection: $profile"
    echo "Results: $results"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

echo "Isolated profile: $profile"
echo "Con build:        $con"
"$con" > "$profile/con.log" 2>&1 &
con_pid=$!

cli_json() { "$cli" --json "$@"; }

deadline=$((SECONDS + 30))
until cli_json identify > "$profile/identify.json" 2>/dev/null; do
    if ! kill -0 "$con_pid" 2>/dev/null; then
        echo "Con exited during start-up; see $profile/con.log" >&2
        exit 1
    fi
    if ((SECONDS >= deadline)); then
        echo "Con's socket did not answer within 30 s; see $profile/con.log" >&2
        exit 1
    fi
    sleep 0.5
done

pane_id=$(cli_json panes list | python3 -I -c '
import json, sys
data = json.load(sys.stdin)
panes = data.get("panes", data) if isinstance(data, dict) else data
print(panes[0]["pane_id"])
')

send() { "$cli" panes send-keys --pane-id "$pane_id" "$1" > /dev/null; }
recent() { "$cli" panes read --pane-id "$pane_id" --lines "${1:-40}"; }

wait_for() {
    local pattern=$1 limit=$((SECONDS + 20))
    until recent 400 | grep -q -- "$pattern"; do
        if ((SECONDS >= limit)); then
            echo "Timed out waiting for '$pattern' in the pane" >&2
            exit 1
        fi
        sleep 0.3
    done
}

wait_for '\$ '
rows_file=$(printf %q "$profile/rows.txt")
send "clear; cat $rows_file"$'\r'
wait_for "row $(printf '%03d' "$rows")"

# Ctrl+U clears the shell's input line. It is still input, so a working
# terminal returns its view to the bottom when it arrives.
return_to_bottom() { send $'\x15'; sleep 0.3; }

record() {
    printf '%-28s %-4s %s\n' "$1" "$2" "$3" | tee -a "$results"
}

# Ask for the lowest visible row number and grade it.
#   $1 step name   $2 instruction   $3 minimum passing row   $4 maximum passing row
ask_row() {
    local name=$1 instruction=$2 min=$3 max=$4 answer
    echo
    echo "== $name"
    echo "$instruction"
    read -r -p "Lowest 'scroll-repro row' number visible in Con (blank if none): " answer
    answer=${answer//[!0-9]/}
    if [[ -n $answer ]] && ((10#$answer >= min && 10#$answer <= max)); then
        record "$name" PASS "lowest visible row $answer (expected $min-$max)"
    else
        record "$name" FAIL "lowest visible row '${answer:-none}' (expected $min-$max)"
    fi
}

ask_yes() {
    local name=$1 question=$2 answer
    echo
    echo "== $name"
    read -r -p "$question [y/n]: " answer
    if [[ $answer == [yY]* ]]; then
        record "$name" PASS "tester answered yes"
    else
        record "$name" FAIL "tester answered '${answer:-no}'"
    fi
}

# Some steps mean nothing unless an earlier one passed: input cannot
# return a view to the bottom if the wheel never moved it.
passed() { grep -Eq "^$1 +PASS " "$results"; }
skip() { record "$1" SKIP "needs '$2' to pass first"; }

last=$rows
echo
echo "Con shows rows 001-$last. Keep this terminal and Con side by side."
echo "Do each gesture in Con, then come back here and answer."

ask_row "baseline" \
    "Do nothing. Read the lowest row number shown in Con." \
    "$last" "$last"

ask_row "wheel up" \
    "Hover over Con's terminal and scroll the mouse wheel or touchpad UP about three notches." \
    1 $((last - 1))

return_to_bottom
if passed "wheel up"; then
    ask_row "input returns to bottom" \
        "The script just sent input to the shell. Do nothing in Con." \
        "$last" "$last"
else
    skip "input returns to bottom" "wheel up"
fi

ask_row "shift+wheel up" \
    "Hold Shift, hover over Con's terminal and scroll the wheel UP about three notches." \
    1 $((last - 1))
return_to_bottom

ask_row "shift+pgup" \
    "Click inside Con's terminal and press Shift+PgUp once." \
    1 $((last - 5))

# Informational only: whether bash echoes an unbound Shift+PgUp sequence
# depends on readline, so a clean line does not prove the key was withheld.
if recent 5 | tail -n 1 | grep -q '[~;]'; then
    record "shift+pgup reached shell" INFO "shell input line shows key bytes: $(recent 5 | tail -n 1)"
else
    record "shift+pgup reached shell" INFO "shell input line shows no key bytes"
fi
return_to_bottom

ask_yes "scrollbar visible" \
    "Is a scrollbar thumb visible at the right edge of Con's terminal?"

if passed "scrollbar visible"; then
    echo
    echo "Drag that thumb up past the top edge of the pane, and release the"
    echo "button above the pane, outside the terminal."
    ask_yes "scrollbar drag to top" "Is 'scroll-repro row 001' now the top row in Con?"
else
    skip "scrollbar drag to top" "scrollbar visible"
fi

return_to_bottom
send "less $rows_file"$'\r'
sleep 1
echo
echo "== less before scrolling"
read -r -p "Con now shows rows in 'less'. Lowest row number visible: " less_before
less_before=${less_before//[!0-9]/}
ask_row "wheel in less (alt screen)" \
    "Hover over 'less' in Con and scroll the wheel DOWN about three notches." \
    $((10#${less_before:-$last} + 1)) "$last"
send q
sleep 0.3

echo
echo "Summary:"
cat "$results"
if grep -q ' FAIL ' "$results"; then
    exit 1
fi
