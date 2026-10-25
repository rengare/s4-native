#!/usr/bin/env bash
# Full decompilation pipeline for Settlers 4 Gold (GOG 2.50.1516).
# Per binary: import + auto-analysis (if not in the project yet), RTTI vftable scan,
# ApplyVtables (names + undiscovered virtual functions), FindMoreCode (data pointers,
# gaps), ExportDecomp -> export/<binary>/. Editor binaries go to export/editor/.
# Usage: tools/decomp_all.sh [binary-name ...]   (default: all)
set -euo pipefail
cd "$(dirname "$0")/.."
GH=(flatpak run --command=/app/lib/ghidra/support/analyzeHeadless org.ghidra_sre.Ghidra "$PWD/ghidra")
S=tools/ghidra
mkdir -p logs export/editor

# project-folder  source-path
BINARIES=(
  "/ extracted/s4/S4.exe"
  "/ extracted/s4/bbinst.dll"
  "/ extracted/s4/Exe/S4_Main.exe"
)
for d in BB_Support EconomyAI EventEngine GfxEngine GuiEngine2 MapAccess MapGenerator RTComp \
         S4_BBGCClient SaveGameAccess SoundEngine Utility VideoEngine; do
  BINARIES+=("/ extracted/s4/Exe/$d.dll")
done
BINARIES+=(
  "/editor extracted/s4/Editor/S4Editor.exe"
  "/editor extracted/s4/Editor/BB_Support.dll"
  "/editor extracted/s4/Editor/LngSat00.dll"
  "/editor extracted/s4/Editor/LngSat01.dll"
)

for entry in "${BINARIES[@]}"; do
  read -r folder src <<<"$entry"
  name=$(basename "$src")
  [[ $# -gt 0 && ! " $* " =~ " $name " ]] && continue
  out=export; [[ $folder == /editor ]] && out=export/editor
  tag=$(echo "${folder#/}_$name" | tr -c 'A-Za-z0-9_.\n' _)
  echo "== $folder/$name"
  # An earlier export means the binary is already in the project.
  if [[ ! -f "$out/$name/functions.tsv" ]]; then
    "${GH[@]}" "settlers$folder" -import "$PWD/$src" -max-cpu 8 > "logs/import_$tag.log" 2>&1
  fi
  python3 -I tools/rtti_scan.py "$src" > "logs/vtables_$tag.tsv"
  "${GH[@]}" "settlers$folder" -process "$name" -noanalysis -scriptPath "$PWD/$S" \
    -postScript ApplyVtables.java "$PWD/logs/vtables_$tag.tsv" \
    -postScript FindMoreCode.java \
    -postScript ExportDecomp.java "$PWD/$out" > "logs/pipeline_$tag.log" 2>&1
  grep -hE 'vftables=|from |functions=|exported' "logs/pipeline_$tag.log" | sed 's/^INFO  //; s/ (GhidraScript)//' \
    || { echo "  failed, see logs/pipeline_$tag.log"; }
  # Functions that hit ExportDecomp's 60 s limit: retry with 15 min each.
  mapfile -t failed < <(python3 -I tools/splice_decomp.py --failed "$out/$name/decomp.c" 2>/dev/null)
  if (( ${#failed[@]} )); then
    "${GH[@]}" "settlers$folder" -process "$name" -noanalysis -readOnly -scriptPath "$PWD/$S" \
      -postScript DecompileAt.java "$PWD/logs/retry_$tag.c" 900 "${failed[@]}" > "logs/retry_$tag.log" 2>&1
    python3 -I tools/splice_decomp.py "$out/$name/decomp.c" "logs/retry_$tag.c"
  fi
done
