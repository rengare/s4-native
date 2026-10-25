#!/usr/bin/env bash
# Check that a directory holds the game this project was built against (GOG "Settlers 4 Gold" 2.50.1516):
#   tools/check_game.sh path/to/game
# The hashes in tools/game-files.sha256 identify the executables and DLLs; they are not the files.
set -u
dir="${1:?usage: tools/check_game.sh <game directory>}"
list="$(dirname "$0")/game-files.sha256"
[[ -f "$dir/Exe/S4_Main.exe" ]] || { echo "not a Settlers 4 directory: $dir/Exe/S4_Main.exe is missing" >&2; exit 2; }
bad=0
while read -r sum file; do
    if [[ ! -f "$dir/$file" ]]; then echo "MISSING    $file"; bad=1
    elif [[ "$(sha256sum "$dir/$file" | cut -d' ' -f1)" != "$sum" ]]; then echo "DIFFERENT  $file"; bad=1
    fi
done < "$list"
for d in Gfx Snd Video Map Config; do [[ -d "$dir/$d" ]] || { echo "MISSING    $d/ (game data directory)"; bad=1; }; done
if (( bad )); then
    echo "This is not the supported version (GOG Gold 2.50.1516), or the files are incomplete." >&2
    echo "The lifter may still work on other builds, but only 2.50.1516 has been tested." >&2
    exit 1
fi
echo "OK: all executables and DLLs match GOG Settlers 4 Gold 2.50.1516."
