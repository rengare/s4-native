# Building

## Requirements

- Linux on x86-64 with a Wayland or X11 session. (Other platforms and CPUs are not supported yet.)
- Rust stable via `rustup`. The workspace default target is `i686-pc-windows-gnu` (for the older
  Wine-side DLLs), so **always pass `--target <your host triple>`** for the native program.
- RAM: about 1 GB per parallel job (`-j`). The lifter splits `S4_Main.exe` (110 MB of Rust) into 48 crates
  (`s4/lifted/s4mainNN`) that compile side by side; a clean build of everything took 2 minutes at `-j14`
  (peak 9.5 GB), instead of 10. Choose `-j` as about the free GB divided by 1.2.
  Changing `s4/x86`, `s4/x87` or the lifter rebuilds everything.
- Your copy of the game (GOG *Settlers 4 Gold* 2.50.1516 is the tested version) as a directory that
  contains `S4.exe`, `Exe/`, `Gfx/`, `Snd/`, `Video/` … By convention `extracted/s4`
  (`extracted/` is git-ignored).
- SDL3 (window, input, audio). Ubuntu/Debian: `sudo apt install libsdl3-0` is enough (the build finds `libSDL3.so.0` itself); with `libsdl3-dev`, pkg-config is used. With neither, build it from its bundled source: `cargo build … --features sdl3-from-source` (needs `cmake` and the usual SDL build dependencies). Other distributions: any SDL 3.2 or newer (3.4 gives the nicest scaling).
- Optional, for the test tooling: `wine`, the `i686-pc-windows-gnu` Rust target and
  `gcc-mingw-w64-i686` (Wine-side tests), `sway` + `grim` (a private headless display), `python3` with
  Pillow (frame comparison), `ffmpeg` (only as a test oracle for video audio).

## Getting the game files

This project **does not work without the original game** and does not include or download it. Buy
*The Settlers IV Gold* (the tested edition is GOG's, version 2.50.1516) and unpack its installer, for
example with `innoextract` (`innoextract --gog -d extracted/gog setup_*.exe`; depending on the version the
game files end up in `extracted/gog` or `extracted/gog/app`). Point the tools at the directory that contains
`S4.exe`, `Exe/`, `Gfx/`, `Snd/` and `Video/`, and check it:

```sh
tools/check_game.sh path/to/that/directory
```

It compares the executables and DLLs against recorded hashes (`tools/game-files.sha256`) and tells you
if the version is not the supported one. The game also works from a read-write copy of an installed game:
it saves into that directory.

## The short way

```sh
tools/setup.sh path/to/game-folder-or-GOG-installer   # add --run to start the game afterwards
```

It does the steps below for you and stops with a clear message when something is missing.

## Steps

```sh
HOST=$(rustc -vV | sed -n 's/host: //p')

# 1. Translate the game's code. Writes s4/lifted/<module>/src (generated, never committed).
tools/lift_all.sh

# 2. Build the program.
cargo build --release -j 8 --target "$HOST" -p s4-play

# 3. Run it (see RUNNING.md).
target/$HOST/release/s4play extracted/s4
```

`tools/lift_all.sh` translates S4_Main.exe, the engine DLLs, the C and C++ runtimes the game ships
(MSVCRT, MSVCP60, MSVCIRT), `binkw32.dll` (video) and the rest of the modules listed in the script.
To translate one module: `tools/lift.sh GfxEngine.dll gfxengine`.

### Function start addresses

Translating needs to know where each function starts. For the supported build (GOG 2.50.1516)
`tools/lift/functions/<binary>.tsv` lists the start addresses (numbers and a catch-funclet flag, no
names), checked against the binary's SHA-256; `tools/lift_all.sh` uses a list only if the hash matches.
`tools/make_function_lists.py` regenerates them from a Ghidra export (`tools/decomp_all.sh`). For any other
build, or for modules without a list (the C/C++ runtimes, `binkw32.dll`), the lifter finds functions by
itself from the exports, entry point, direct calls, code pointers in data and the C++ exception tables.
That fallback is good (98% of the list's functions in `S4_Main.exe`) but not complete, and a missing
function shows up as `nothing lifted or implemented there` at run time.

### Installing a launcher entry (Lutris)

`tools/install_s4.sh` builds `s4play` and points an existing Lutris entry named *Settlers IV (decomp:
native Rust)* (`~/.local/share/lutris/games/settlers-iv-decomp-native-*.yml`) at it. Create that entry
once by hand (runner: Linux, executable: anything, game directory arguments: your `extracted/s4`).

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `--target takes a target architecture` | `$HOST` was empty; run the `HOST=$(…)` line in the same shell |
| Build killed or machine freezes | Too many parallel jobs for the RAM; use fewer jobs (about 1 GB each) |
| `not built: …` from the installer | Build first, or pass nothing so it builds |
| No window for the first seconds | Wayland shows a window only after its first frame; a placeholder frame is drawn while the game starts |
| `unable to find library -lSDL3` | No SDL3 on the machine: install `libsdl3-0` (or `-dev`), or use `--features sdl3-from-source` |
| No sound | The log prints `audio: …`: either the playback device could not be opened (SDL picks the system default; `SDL_AUDIODRIVER=pipewire` / `pulseaudio` / `alsa` forces one) or `S4_NO_AUDIO` is set |
| Crash message `fell off the decoded code` / `lift gap` | A function was translated incompletely; re-run `tools/lift_all.sh` with the current lifter and rebuild |
