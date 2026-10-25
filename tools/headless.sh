#!/usr/bin/env bash
# Run a command on a private headless display (sway, headless backend, with
# Xwayland), so the original game can run without showing up on the desktop.
#   tools/headless.sh <command...>
# Inside, WAYLAND_DISPLAY and DISPLAY point at the private display; take a
# screenshot with: grim <file.png>   (HEADLESS_SIZE, default 1920x1080)
set -euo pipefail
size="${HEADLESS_SIZE:-1920x1080}"
run=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/s4-headless.XXXX")
cat > "$run/config" <<CFG
output HEADLESS-1 resolution $size
xwayland force
CFG
rt="${XDG_RUNTIME_DIR:-/tmp}"
before_w=$(ls "$rt" | grep -E '^wayland-[0-9]+$' | sort || true)
before_x=$(ls /tmp/.X11-unix 2>/dev/null | sort || true)
unset WAYLAND_DISPLAY DISPLAY
WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
    sway --config "$run/config" >"$run/sway.log" 2>&1 &
sway_pid=$!
trap 'kill $sway_pid 2>/dev/null; rm -rf "$run"' EXIT
sock="" x=""
for _ in $(seq 100); do
    sock=$(comm -13 <(echo "$before_w") <(ls "$rt" | grep -E '^wayland-[0-9]+$' | sort) | head -1)
    x=$(comm -13 <(echo "$before_x") <(ls /tmp/.X11-unix 2>/dev/null | sort) | head -1 | sed 's/^X/:/')
    [[ -n "$sock" && -n "$x" ]] && break
    sleep 0.2
done
# The native program needs only the Wayland socket; the original under Wine also needs X (Xwayland).
[[ -n "$sock" ]] || { cat "$run/sway.log" >&2; echo "headless display did not start" >&2; exit 1; }
[[ -n "$x" ]] || echo "note: no X display (Xwayland did not start); Wayland clients only" >&2
export WAYLAND_DISPLAY="$sock"; [[ -n "$x" ]] && export DISPLAY="$x"
# The private compositor's own IPC socket (swaymsg inside the command talks to it, never to your desktop).
export SWAYSOCK="$rt/sway-ipc.$(id -u).$sway_pid.sock"
sleep 0.5
"$@"
