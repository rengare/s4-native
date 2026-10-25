#!/usr/bin/env bash
# Install the native port into Lutris (default), or our DLLs into a Wine install of Settlers 4 Gold.
#
#   tools/install_s4.sh                  build s4play and point the Lutris entry "Settlers IV (decomp: native Rust)"
#                                        at it (config ~/.local/share/lutris/games/settlers-iv-decomp-native-*.yml)
#   tools/install_s4.sh --no-build       same, without building
#   tools/install_s4.sh --wine-dlls [--restore] [game-dir]
#                                        legacy: put our Wine-side DLLs into a GOG install (the game must not be running)
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${1:-}" != --wine-dlls ]]; then
    host=$(rustc -vV | sed -n 's/host: //p')
    [[ "${1:-}" == --no-build ]] || cargo build -j 4 --release --target "$host" -p s4-play
    bin="$PWD/target/$host/release/s4play"
    [[ -x "$bin" ]] || { echo "not built: $bin" >&2; exit 1; }
    cfgdir="${LUTRIS_GAMES_DIR:-$HOME/.local/share/lutris/games}"
    shopt -s nullglob
    cfgs=("$cfgdir"/settlers-iv-decomp-native-*.yml)
    (( ${#cfgs[@]} )) || { echo "no Lutris entry settlers-iv-decomp-native-*.yml in $cfgdir" >&2; exit 1; }
    for cfg in "${cfgs[@]}"; do
        sed -i "s|^\(  exe: \).*|\1$bin|" "$cfg"
        echo "$cfg: exe -> $bin"
    done
    exit 0
fi
shift

restore=0; [[ "${1:-}" == --restore ]] && { restore=1; shift; }
game="${1:-$HOME/lutris/gog/the-settlers-iv-gold-edition/drive_c/GOG Games/Settlers 4 Gold}"
exe="$game/Exe"
build=target/i686-pc-windows-gnu/release
PROXIES=(GfxEngine)
REPLACEMENTS=(VideoEngine EventEngine MapAccess SaveGameAccess)

[[ -f "$exe/S4_Main.exe" ]] || { echo "not a Settlers 4 dir: $game" >&2; exit 1; }
# Overwriting a DLL that a running game has mapped crashes it.
if pgrep -x S4_Main.exe >/dev/null; then
    echo "Settlers 4 is running; quit it first." >&2; exit 1
fi
# Copy to a temp name and rename, so the old file's inode is never modified.
put() { cp "$1" "$2.new" && mv -f "$2.new" "$2" && echo "$1 -> $2"; }

for m in "${PROXIES[@]}" "${REPLACEMENTS[@]}"; do
    if (( restore )); then
        [[ -f "$exe/$m.dll.bak" ]] && mv -v "$exe/$m.dll.bak" "$exe/$m.dll"
        rm -fv "$exe/${m}_orig.dll"
        continue
    fi
    [[ -f "$build/$m.dll" ]] || { echo "not built: $build/$m.dll (cargo build --release)" >&2; exit 1; }
    # Keep whatever was installed before (e.g. the Lutris widescreen-patched GfxEngine), once.
    [[ -f "$exe/$m.dll.bak" ]] || cp -v "$exe/$m.dll" "$exe/$m.dll.bak"
    if [[ " ${PROXIES[*]} " == *" $m "* ]]; then
        # Proxies forward to the unmodified GOG original.
        put "extracted/s4/Exe/$m.dll" "$exe/${m}_orig.dll"
    fi
    put "$build/$m.dll" "$exe/$m.dll"
done
