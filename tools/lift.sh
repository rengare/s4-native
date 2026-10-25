#!/usr/bin/env bash
# Lift a module of the extracted game to Rust: tools/lift.sh GfxEngine.dll gfxengine
# Writes s4/lifted/<crate>/src (not committed: it is derived from the game's code).
set -euo pipefail
cd "$(dirname "$0")/.."
bin="$1"; crate="$2"; shift 2
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
cargo build --release --target "$host" -p lift 2>&1 | grep -E "^(error|warning: unused)" -A6 || true
rm -rf "s4/lifted/$crate/src"
"target/$host/release/lift" "extracted/s4/Exe/$bin" "$(function_list "$bin")" "s4/lifted/$crate/src" "$@"
