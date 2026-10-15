#!/usr/bin/env bash
# Requires Xvfb and xdotool on PATH. Exercises X11/software rendering only.
set -eu
task_logs=$(mktemp -d /tmp/bkm-gui.XXXXXX)
app_pid=
server_pid=
cleanup() {
    for child in $app_pid $server_pid; do
        kill "$child" 2>/dev/null || true
    done
    for attempt in {1..20}; do
        alive=false
        for child in $app_pid $server_pid; do
            if kill -0 "$child" 2>/dev/null; then alive=true; fi
        done
        if ! "$alive"; then break; fi
        sleep 0.1
    done
    for child in $app_pid $server_pid; do
        if kill -0 "$child" 2>/dev/null; then kill -KILL "$child" 2>/dev/null || true; fi
        wait "$child" 2>/dev/null || true
    done
}
trap cleanup EXIT
printf 'GUI logs=%s\n' "$task_logs"
Xvfb -displayfd 3 -screen 0 800x600x24 -nolisten tcp \
    3>"$task_logs/display" >"$task_logs/xvfb.out" 2>"$task_logs/xvfb.err" &
server_pid=$!
for attempt in {1..50}; do
    test -s "$task_logs/display" && break
    sleep 0.1
done
export DISPLAY=":"$(cat "$task_logs/display")
export LIBGL_ALWAYS_SOFTWARE=1
unset WAYLAND_DISPLAY
"${1:-target/debug/bkm-gui}" >"$task_logs/gui.out" 2>"$task_logs/gui.err" &
app_pid=$!
window=$(timeout 10s xdotool search --sync --onlyvisible --pid "$app_pid" \
    --name BroKnowMyTrove | head -n 1)
test -n "$window"
xdotool getwindowgeometry "$window"
xdotool windowfocus --sync "$window"
sleep 0.5
xdotool key Tab Tab Tab Return
for attempt in {1..50}; do
    kill -0 "$app_pid" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "$app_pid" 2>/dev/null; then
    printf 'GUI did not close\n' >&2
    exit 1
fi
wait "$app_pid"
app_pid=
test ! -s "$task_logs/gui.err"
printf 'GUI exit=0, stderr empty\n'
