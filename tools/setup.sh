#!/usr/bin/env bash
# First-run helper: from your copy of the game to a built, runnable program.
#
#   tools/setup.sh <game folder | GOG installer (setup_*.exe)> [--force] [--no-build] [--run]
#
# 1. unpacks the GOG installer with innoextract (only if you gave an installer),
# 2. finds the game folder (the one with Exe/S4_Main.exe) and links it as extracted/s4,
# 3. checks that it is the supported version (tools/check_game.sh),
# 4. checks the build requirements (Rust, SDL3, memory, disk),
# 5. translates the game's code on this machine (tools/lift_all.sh),
# 6. builds the program (about 20 minutes, ~16 GB RAM).
#
# The game files are only read. Nothing translated or built here is meant to be shared: it contains
# code derived from the game (see docs/LEGAL.md).
set -euo pipefail
cd "$(dirname "$0")/.."

src="" force=0 build=1 run=0
for a in "$@"; do
    case "$a" in
        --force) force=1 ;;
        --no-build) build=0 ;;
        --run) run=1 ;;
        -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
        -*) echo "unknown option $a (see --help)" >&2; exit 2 ;;
        *) src="$a" ;;
    esac
done
[[ -n "$src" ]] || { sed -n '2,15p' "$0" >&2; exit 2; }
say() { printf '\n== %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# --- 1/2: find the game folder
say "1/6 locating the game"
if [[ -f "$src" ]]; then
    command -v innoextract >/dev/null || die "$src is an installer: install innoextract first (Debian/Ubuntu: sudo apt install innoextract; Fedora: sudo dnf install innoextract)."
    mkdir -p extracted/gog
    if [[ -z "$(find extracted/gog -name S4_Main.exe -print -quit 2>/dev/null)" ]]; then
        echo "unpacking $src into extracted/gog (several GB)"
        innoextract --gog -d extracted/gog "$src" >/dev/null
    else
        echo "extracted/gog already holds an unpacked game, reusing it"
    fi
    root=extracted/gog
elif [[ -d "$src" ]]; then
    root="$src"
else
    die "$src is neither a folder nor a file"
fi
exe=$(find -L "$root" -maxdepth 4 -path '*/Exe/S4_Main.exe' -print -quit 2>/dev/null || true)
[[ -n "$exe" ]] || die "no Exe/S4_Main.exe under $root: point this at the folder that contains S4.exe and Exe/ (for a GOG installer, the unpacked game may sit in an 'app' subfolder)."
game=$(cd "$(dirname "$exe")/.." && pwd -P)
echo "game folder: $game"
mkdir -p extracted
if [[ -L extracted/s4 || ! -e extracted/s4 ]]; then
    ln -sfn "$game" extracted/s4
elif [[ "$(cd extracted/s4 && pwd -P)" != "$game" ]]; then
    die "extracted/s4 exists and is not a link to $game: move it away or remove it, then run this again."
fi

# --- 3: version check
say "2/6 checking the version"
if ! tools/check_game.sh extracted/s4; then
    (( force )) || die "unsupported version or incomplete files (add --force to try anyway: the lifter may work, but only GOG 2.50.1516 is tested)."
    echo "continuing because of --force"
fi

# --- 4: requirements
say "3/6 checking the build requirements"
ok=1
command -v cargo >/dev/null && command -v rustc >/dev/null || { echo "missing: Rust (https://rustup.rs)"; ok=0; }
if ! { ldconfig -p 2>/dev/null | grep -q 'libSDL3\.so' || pkg-config --exists sdl3 2>/dev/null; }; then
    echo "missing: the SDL3 library (Debian/Ubuntu: sudo apt install libsdl3-0; or build with --features sdl3-from-source)"; ok=0
fi
mem_gb=$(awk '/MemTotal/ {printf "%d", $2/1048576}' /proc/meminfo 2>/dev/null || echo 0)
(( mem_gb >= 15 )) || echo "warning: ${mem_gb} GB of RAM; the build wants about 16 GB (it may be killed by the out-of-memory killer; swap helps)"
free_gb=$(df -Pk . | awk 'NR==2 {printf "%d", $4/1048576}')
(( free_gb >= 10 )) || echo "warning: only ${free_gb} GB free here; the build directory needs about 10 GB"
(( ok )) || die "fix the missing requirements above and run this again."
echo "ok (RAM ${mem_gb} GB, free disk ${free_gb} GB)"

# --- 5: translate
say "4/6 translating the game's code (a few seconds to minutes)"
tools/lift_all.sh

# --- 6: build
host=$(rustc -vV | sed -n 's/host: //p')
bin="target/$host/release/s4play"
if (( build )); then
    jobs=$(nproc); (( jobs > 4 )) && jobs=4
    say "5/6 building (about 20 minutes, -j $jobs)"
    cargo build --release -j "$jobs" --target "$host" -p s4-play
else
    echo "skipping the build (--no-build)"
fi
say "6/6 done"
echo "Run it with:  $bin extracted/s4"
(( run )) && exec "$bin" extracted/s4
exit 0
