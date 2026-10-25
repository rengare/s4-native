#!/usr/bin/env bash
# Lift every module of the extracted game: s4/lifted/<crate>/src (not committed).
# The crates' Cargo.toml files are committed; this makes any that are missing.
set -euo pipefail
cd "$(dirname "$0")/.."
# Cargo cannot load the workspace until every generated crate has a src/lib.rs: put placeholders
# where the lifter has not written one yet (it replaces them).
for d in s4/lifted/*/; do
    [[ -f "$d/Cargo.toml" && ! -f "$d/src/lib.rs" ]] || continue
    mkdir -p "$d/src"
    printf '%s\n' '// Placeholder until tools/lift_all.sh generates this crate.' 'pub fn dispatch(_c: &mut s4_x86::Cpu, _addr: u32) -> bool { false }' > "$d/src/lib.rs"
done
# Function start addresses: a local Ghidra export if there is one, else the committed list for this
# exact build of the binary (checked by SHA-256); otherwise the lifter finds functions by itself.
function_list() {
    local bin="$1" tsv="export/$1/functions.tsv" cand="tools/lift/functions/$1.tsv"
    if [[ ! -f "$tsv" && -f "$cand" && -f "extracted/s4/Exe/$bin" ]]; then
        [[ "$(sha256sum "extracted/s4/Exe/$bin" | cut -d' ' -f1)" == "$(sed -n 's/^# sha256 //p' "$cand")" ]] && tsv="$cand"
    fi
    echo "$tsv"
}
host=$(rustc -vV | sed -n 's/host: //p')
cargo build --release --target "$host" -p lift 2>&1 | grep -E "^error" -A6 || true
while read -r bin crate; do
    mkdir -p "s4/lifted/$crate"
    if [[ ! -f "s4/lifted/$crate/Cargo.toml" ]]; then
        cat > "s4/lifted/$crate/Cargo.toml" <<TOML
[package]
name = "lifted-$crate"
version = "0.1.0"
edition = "2024"
description = "$bin lifted to Rust by tools/lift (sources are generated from the user's copy of the game and not committed)"

[lib]
name = "lifted_$crate"
path = "src/lib.rs"

[dependencies]
s4-x86 = { path = "../../x86" }
s4-x87 = { path = "../../x87" }
TOML
    fi
    rm -rf "s4/lifted/$crate/src"
    # The 110 MB S4_Main.exe goes to 48 crates (s4/lifted/s4mainNN) that compile in parallel and in little memory each.
    parts=()
    if [[ "$crate" == s4main ]]; then
        rm -rf s4/lifted/s4main-p*/src
        parts=(--parts s4/lifted/s4main-p 48 --prefix s4m_)
    fi
    tsv="$(function_list "$bin")"
    echo -n "$bin: "
    patch=()
    [[ -f "tools/lift/patches/$bin.tsv" ]] && patch=(--patch "tools/lift/patches/$bin.tsv")
    "target/$host/release/lift" "extracted/s4/Exe/$bin" "$tsv" "s4/lifted/$crate/src" ${patch[@]+"${patch[@]}"} ${parts[@]+"${parts[@]}"} "$@" 2>&1 | tail -1
done <<LIST
S4_Main.exe s4main
GfxEngine.dll gfxengine
GuiEngine2.dll guiengine2
SoundEngine.dll soundengine
EventEngine.dll eventengine
MapAccess.dll mapaccess
SaveGameAccess.dll savegameaccess
VideoEngine.dll videoengine
S4_BBGCClient.dll s4bbgcclient
BB_Support.dll bbsupport
EconomyAI.dll economyai
MapGenerator.dll mapgenerator
RTComp.dll rtcomp
Utility.dll utility
MSVCRT.DLL msvcrt
MSVCP60.DLL msvcp60
MSVCIRT.DLL msvcirt
binkw32.dll binkw32
LIST
