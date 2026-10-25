#!/usr/bin/env bash
# Lift The Settlers III's S3.EXE to Rust: s3/lifted/s3main/src (not committed: derived from the game's code).
# Usage: tools/lift_s3.sh [game folder, default extracted/s3]
set -euo pipefail
cd "$(dirname "$0")/.."
game="${1:-extracted/s3}"
host=$(rustc -vV | sed -n 's/host: //p')
cargo build --release --target "$host" -p lift 2>&1 | grep -E "^error" -A6 || true
# The 27,000 functions go to 64 crates (s3/lifted/s3main-pNN) that compile in parallel and in little memory each;
# s3main only picks the crate by address.
rm -rf s3/lifted/s3main/src s3/lifted/s3main-p*/src
"target/$host/release/lift" "$game/S3.EXE" tools/lift/functions/S3.EXE.tsv s3/lifted/s3main/src --parts s3/lifted/s3main-p 64 --prefix s3_ "${@:2}"
# The Indeo 5 video codec the game's movies use (a Video for Windows driver: DriverProc).
rm -rf s3/lifted/ir50/src
"target/$host/release/lift" "$game/ir50_32.dll" /nonexistent.tsv s3/lifted/ir50/src
