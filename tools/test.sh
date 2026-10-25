#!/usr/bin/env bash
# Run the crates' unit tests (32-bit Windows binaries) under Wine.
# WINE defaults to the Lutris arm64ec build (i686 code via FEX on ARM64) when
# it exists, else to the system wine (x86-64 hosts run the code natively).
set -euo pipefail
cd "$(dirname "$0")/.."
arm64ec="$HOME/.local/share/lutris/runners/wine/wine-arm64ec-copr-11.18/bin/wine"
if [[ -z "${WINE:-}" ]]; then
    if [[ -x "$arm64ec" ]]; then WINE="$arm64ec"; else WINE=$(command -v wine); fi
fi
export CARGO_TARGET_I686_PC_WINDOWS_GNU_RUNNER="$WINE"
export WINEPREFIX="${WINEPREFIX:-$PWD/target/wineprefix}" WINEDEBUG="${WINEDEBUG:--all}"
# Game DLLs the tests load (from the user's extracted copy; target/ is not in git).
deps=target/i686-pc-windows-gnu/release/deps
mkdir -p "$deps"
cp -u extracted/s4/Exe/binkw32.dll "$deps/" 2>/dev/null || echo "note: extracted/s4/Exe/binkw32.dll missing, video tests will fail" >&2
exec cargo test --release "$@"
