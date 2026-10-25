#!/usr/bin/env bash
# Point a Lutris entry at the native build of The Settlers IV or III (the entry is made if it does not exist).
#
#   tools/install_lutris.sh s4 /path/to/the/s4/game      [--no-build]
#   tools/install_lutris.sh s3 /path/to/the/s3/game      [--no-build]
#
# Builds s4play / s3play for this machine (release), then writes the Lutris game config
# (~/.local/share/lutris/games or $LUTRIS_GAMES_DIR -- on some systems ~/.config/lutris/games) and, for a new entry,
# its row in Lutris's database (needs sqlite3; close Lutris first). The game folder is the one with S4.exe / S3.EXE.
set -euo pipefail
cd "$(dirname "$0")/.."
which=${1:?usage: install_lutris.sh s3|s4 /path/to/game [--no-build]}
game=$(realpath "${2:?game folder}")
case "$which" in
    s4) pkg=s4-play; bin=s4play; slug=settlers-iv-decomp-native; title='Settlers IV (decomp: native Rust)'; year=2002 ;;
    s3) pkg=s3-play; bin=s3play; slug=settlers-iii-native; title='Settlers III (native Rust)'; year=2000 ;;
    *) echo "s3 or s4" >&2; exit 2 ;;
esac
host=$(rustc -vV | sed -n 's/host: //p')
[[ "${3:-}" == --no-build ]] || cargo build --release --target "$host" -p "$pkg" -j "${JOBS:-8}"
exe="$PWD/target/$host/release/$bin"
[[ -x "$exe" ]] || { echo "not built: $exe" >&2; exit 1; }
for dir in "${LUTRIS_GAMES_DIR:-}" "$HOME/.config/lutris/games" "$HOME/.local/share/lutris/games"; do
    [[ -n "$dir" && -d "$dir" ]] && { cfgdir=$dir; break; }
done
: "${cfgdir:?no Lutris games directory}"
shopt -s nullglob
existing=("$cfgdir/$slug"-*.yml)
if (( ${#existing[@]} )); then
    cfg=${existing[0]}; id=${cfg##*-}; id=${id%.yml}
else
    id=$(date +%s); cfg="$cfgdir/$slug-$id.yml"
fi
cat > "$cfg" <<YML
game:
  exe: $exe
  args: $game
  working_dir: $PWD
game_slug: $slug
linux: {}
name: '$title'
slug: $slug
YML
echo "$cfg -> $exe $game"
db="$(dirname "$cfgdir")/pga.db"
[[ -f "$db" ]] || db="$HOME/.local/share/lutris/pga.db"
if command -v sqlite3 >/dev/null && [[ -f "$db" ]]; then
    if [[ -z "$(sqlite3 "$db" "select id from games where slug='$slug'")" ]]; then
        pgrep -x lutris >/dev/null && { echo "close Lutris, then run this again to add the entry to its library" >&2; exit 1; }
        sqlite3 "$db" "insert into games (name, slug, platform, runner, directory, installed, installed_at, year, configpath, playtime, hidden, has_custom_banner, has_custom_icon, has_custom_coverart_big) values ('$title', '$slug', 'Linux', 'linux', '$PWD', 1, $id, $year, '$slug-$id', 0, 0, 0, 0, 0)"
        echo "added to $db"
    fi
fi
