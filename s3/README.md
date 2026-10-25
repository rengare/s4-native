# The Settlers III

`s3play` runs the GOG edition of *The Settlers III* (Ultimate Collection 1.60) the same way `s4play` runs
*The Settlers IV*: `S3.EXE` is translated to Rust on your machine by `tools/lift` and runs on the host layer
(`s4/host`), with an SDL3 window, mouse, keyboard and sound. You need your own copy of the game; nothing from it
is in this repository.

## Build and run

```sh
tools/lift_s3.sh /path/to/the/game      # S3.EXE and ir50_32.dll -> s3/lifted/*/src (generated, never committed)
cargo build --release -j8 --target "$(rustc -vV | sed -n 's/host: //p')" -p s3-play
target/<host>/release/s3play /path/to/the/game
```

The game folder is the one with `S3.EXE`. The files the installer puts in its `app/` subfolder (the empty marker
files such as `S3MCD1.DAT`) must be next to `S3.EXE`.

Build time and memory: the lifter splits `S3.EXE` into 64 crates (`s3/lifted/s3main-pNN`) that rustc compiles in
parallel; each rustc process needs about 1 GB. Measured on 14 cores (clean build of the game and the front end):

| `-j` | time | peak memory (all rustc) |
|---|---|---|
| 14 | 5 min | 11.4 GB |
| 8 | 7.6 min | 7.4 GB |
| 4 | 12 min | 4.4 GB |

Pick `-j` as about the free memory in GB divided by 1.2. (As one crate it took 45 minutes and did not fit in 16 GB.)

macOS (Apple Silicon, experimental): `brew install sdl3`, then the same commands with `--target aarch64-apple-darwin`;
start the program with `DYLD_FALLBACK_LIBRARY_PATH=/opt/homebrew/lib` set. An 8 GB M1 builds at `-j3` (S4 11 min,
then S3 27 min) and plays the intro; window input and sound there have been tried less than on Linux.

`s3play --resolution auto|keep|640x480|800x600|1024x768` picks one of the game's own three screen modes (default `auto`: the
biggest that fits the desktop). The game's `Resolution` setting is written to the registry for you.

Settings (the game keeps them in the Windows registry) live in `~/.local/share/s3-native/registry.txt`; the first
run seeds them from the game's own `regs.cmd`.

## What works

Menus, mouse and keyboard, DirectDraw 16-bit screen, DirectSound (mixed by the host), the CD music (the GOG
`Music/TrackNN.ogg` files answer the game's MCI "cdaudio" commands; the next track starts when one ends), movies
(AVI with Indeo 5 video, decoded by the game's own `ir50_32.dll`, also lifted), the briefing voice-overs, LAN games
(below). Scrolling at the screen edge is the game's own option: it is off while the registry value `ScrollMode` is 0
(`~/.local/share/s3-native/registry.txt`); 1 turns it on.

## LAN

The game plays over DirectPlay; the host implements IDirectPlay4A itself (`s4/host/src/dplay.rs`) on the local
network. Sessions are found by a broadcast on UDP port 47624 (also sent to every interface's broadcast address and,
if the "Host IP Address" field of the LAN screen is filled, straight to that address); the players of a session
connect over TCP to the host, which relays the messages. Both machines need to reach each other on UDP 47624 and the
host's TCP port (any, chosen at start). Create a game with "New Game", join from the other machine with
"Search for games", pick it, "Join Game"; the guest presses the red button next to its name (green = ready), then
the host presses OK. Two instances on one machine find each other too (loopback). `S4_DP_TRACE=1` logs the session.

## Debugging switches

`S4_HOST_TRACE=1` (every host call), `S4_SEH_TRACE=1` (C++ exceptions), `S4_DD_TRACE=1` (surfaces), `S4_REG_TRACE=1`
(registry), `S4_MCI_TRACE=1` (MCI commands and movies), `S4_MISSING=1` (imports nothing implements), `S4_CD_SECONDS=n` (every music track ends after n seconds), `S4_PEEK=hexaddr@secs` (print a guest dword), `S4_FONT_TRACE=1`, `S4_DP_TRACE=1`,
`S4_FRAME_DUMP=<file.ppm> S4_FRAME_AT=<seconds>` (save the screen), `S4_AUDIO_DUMP=<file>` (raw S16 stereo 44.1 kHz).
`LIFT_TRACE_CALLS=1` when lifting makes every function print its entry (slow; for finding where a codec fails).
