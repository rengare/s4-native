#!/usr/bin/env bash
# Instruction conformance: lift every instruction form of the game's code and
# compare with the processor (under Wine). S4_X86_ITERS runs per form,
# S4_X86_ONLY filters by text.
set -euo pipefail
cd "$(dirname "$0")/.."
host=$(rustc -vV | sed -n 's/host: //p')
cargo build --release --target "$host" -p lift 2>&1 | grep -E "^error" -A6 || true
E=extracted/s4/Exe
if [[ ! -s s4/x86test/src/cases.rs || -n "${REGEN:-}" ]]; then "target/$host/release/lift" cases s4/x86test/src/cases.rs $E/S4_Main.exe $E/BB_Support.dll $E/EconomyAI.dll $E/EventEngine.dll $E/GfxEngine.dll $E/GuiEngine2.dll $E/MapAccess.dll $E/MapGenerator.dll $E/RTComp.dll $E/S4_BBGCClient.dll $E/SaveGameAccess.dll $E/SoundEngine.dll $E/Utility.dll $E/VideoEngine.dll; fi
exec tools/test.sh -p s4-x86test -- --nocapture "$@"
