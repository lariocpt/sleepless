#!/bin/sh
# The check a green compile cannot give you: does the operating system itself
# believe sleepless is holding a lock, and does it stop believing that the instant
# the process is killed outright?
#
# A real file rather than a heredoc in a workflow, so `shellcheck` sees it, so both
# the x86-64 and the arm64 CI jobs run the same thing, and so it can be run by hand:
#
#     test/smoke.sh target/debug/sleepless
#
# Layers that need a tool this host does not have announce a SKIP and move on. A
# build host without systemd is not a broken lock, and turning it into a red build
# only teaches people to ignore red builds.
set -eu

BIN=${1:-target/debug/sleepless}
[ -x "$BIN" ] || { echo "FAIL: $BIN is not executable"; exit 1; }
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# Every run gets a throwaway HOME so the suite cannot read or write real settings.
run() {
    env HOME="$WORK/home" XDG_CONFIG_HOME="$WORK/home/config" \
        XDG_STATE_HOME="$WORK/home/state" "$@"
}

# The same, backgrounded. `exec` is the whole point: without it $! names the subshell
# the function call forked, the kill below hits that wrapper, the real process keeps
# holding the lock, and the test blames the program for surviving a signal it never
# received.
run_bg() {
    exec env HOME="$WORK/home" XDG_CONFIG_HOME="$WORK/home/config" \
        XDG_STATE_HOME="$WORK/home/state" "$@"
}

say() { printf '  %s\n' "$*"; }

echo "smoke: $BIN ($(uname -s))"

# ---------------------------------------------------------------- the common path
run "$BIN" --always --smoke 2 > "$WORK/out.txt"
grep -q 'sleepless - ' "$WORK/out.txt" || {
    echo "FAIL: no status line"; cat "$WORK/out.txt"; exit 1; }
# The mode belongs on the line whether or not a lock was granted: a machine that is
# refused every inhibitor still has to say which mode it was refused in.
grep -q 'mode=always' "$WORK/out.txt" || {
    echo "FAIL: the status line does not report the mode"; cat "$WORK/out.txt"; exit 1; }
say "status line: $(head -1 "$WORK/out.txt")"

# The output is what scripts grep and what a Windows code page has to render, so it
# must be ASCII. Checked on the error path too -- that is where the D-Bus strings,
# the config paths and the --why text actually appear, and the old check only ever
# looked at the run where everything worked.
assert_ascii() {
    if LC_ALL=C grep -q '[^ -~]' "$1"; then
        echo "FAIL: non-ASCII in $1"; LC_ALL=C grep -n '[^ -~]' "$1"; exit 1
    fi
}
assert_ascii "$WORK/out.txt"

# ------------------------------------------------------------------- per-platform
case "$(uname -s)" in
Linux)
    # No buses at all, the way a container sees the world. This must still exit 0:
    # it is the regression test for the startup path that once refused to run
    # without D-Bus.
    run env DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent \
        DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent \
        "$BIN" --always --why 'caf* - unicode would break a grep' --smoke 1 \
        > "$WORK/bare.txt"
    grep -q 'NOT INHIBITING' "$WORK/bare.txt" || {
        echo "FAIL: expected NOT INHIBITING with no bus"; cat "$WORK/bare.txt"; exit 1; }
    assert_ascii "$WORK/bare.txt"
    say "with no bus: honest, ASCII, and exit 0"

    if ! command -v systemd-inhibit >/dev/null 2>&1; then
        say "SKIP: systemd-inhibit is not installed, cannot ask logind"
        exit 0
    fi
    if ! grep -q 'sleep=yes' "$WORK/out.txt"; then
        say "SKIP: this host granted no logind lock, so there is nothing to ask about"
        exit 0
    fi

    # Ask logind, not ourselves.
    run_bg "$BIN" --always --why 'ci smoke' --smoke 20 > "$WORK/held.txt" &
    pid=$!
    sleep 3
    systemd-inhibit --list > "$WORK/list.txt" 2>&1 || true
    if ! grep -q 'sleepless' "$WORK/list.txt"; then
        echo "FAIL: logind does not list our inhibitor"; cat "$WORK/list.txt"
        kill -9 "$pid" 2>/dev/null || true; exit 1
    fi
    say "logind lists the inhibitor"

    # ...and the whole point: killing it outright releases the lock, with no
    # chance for any cleanup code to run.
    kill -9 "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    sleep 2
    systemd-inhibit --list > "$WORK/after.txt" 2>&1 || true
    if grep -q 'sleepless' "$WORK/after.txt"; then
        echo "FAIL: the inhibitor survived SIGKILL"; cat "$WORK/after.txt"; exit 1
    fi
    say "and it is gone after kill -9"
    ;;
Darwin)
    run_bg "$BIN" --always --smoke 20 > "$WORK/held.txt" &
    pid=$!
    sleep 3
    pmset -g assertions > "$WORK/assertions.txt"
    # Match the owning process, not a bare name: the GitHub macOS runner is an Anka
    # VM holding its own assertion called "Anka sleeplessness", which a substring
    # match on "sleepless" happily finds.
    for want in PreventUserIdleSystemSleep PreventUserIdleDisplaySleep; do
        grep -qE "pid [0-9]+\(sleepless\).*$want" "$WORK/assertions.txt" || {
            echo "FAIL: powerd is not holding $want for us"
            cat "$WORK/assertions.txt"; kill -9 "$pid" 2>/dev/null || true; exit 1; }
    done
    say "powerd holds both assertions"

    kill -9 "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    sleep 2
    if pmset -g assertions | grep -qE 'pid [0-9]+\(sleepless\)'; then
        echo "FAIL: the assertion survived SIGKILL"; exit 1
    fi
    say "and it is gone after kill -9"
    ;;
*)
    say "SKIP: no platform probe for $(uname -s)"
    ;;
esac
echo "smoke: ok"
