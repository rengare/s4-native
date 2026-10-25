#!/usr/bin/env bash
# Drive the ORIGINAL game under Wine through the same scripted session as s4play and screenshot it.
# Run inside tools/headless.sh:   tools/headless.sh tools/wine_session.sh <outdir>
# Skips the intro videos with Escape, waits for the menu, then clicks Single Player, the map preview and the
# start gem (game coordinates; the Wine desktop shows the 800x600 game at (2,25)). Shots: menu.png, match_<n>.png
set -u
out="$(realpath -m "${1:-logs/wine}")"; mkdir -p "$out"
root="$(cd "$(dirname "$0")/.." && pwd)"
X="python3 $root/tools/xinput.py"
ox=2; oy=25
cd "$root/extracted/s4"
WINEPREFIX="$root/target/wineprefix-game" WINEDEBUG=-all wine S4.exe >/dev/null 2>&1 &
is_menu() { grim -t ppm -g "$((ox+780)),$((oy+300)) 1x1" - 2>/dev/null | tail -c 3 | python3 -c "import sys; d=sys.stdin.buffer.read(); sys.exit(0 if d and any(d) else 1)"; }
sleep 15
for i in $(seq 150); do
    $X "key Escape"; sleep 1
    is_menu && break
done
sleep 3; grim "$out/menu.png"
click() { $X "click $((ox+$1)) $((oy+$2))"; }
click 631 219; sleep 5
click 548 419; sleep 5
click 702 547; sleep 6
click 702 547; sleep 6
click 702 547
for t in 30 60; do sleep 30; grim "$out/match_$t.png"; done
if [ -n "${WINE_EXTRA:-}" ]; then   # extra xinput commands (game coordinates are NOT shifted), then a shot
    eval "$X $WINE_EXTRA"; sleep 1; grim "$out/extra.png"
fi
pkill -f "[S]4_Main|[S]4.exe"
