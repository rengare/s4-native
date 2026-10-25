# Status

Last updated with the work of 2026-10-20. Platform tested: x86-64 Linux (Wayland).

## Working

- **Start-up and menus**: logos, intro videos with sound, main menu, single player, campaign screens,
  map selection, load game screen.
- **Matches**: a single-player map starts, plays and renders; mouse (including the right-drag camera),
  keyboard, edge scrolling, HUD panels and tooltips, minimap, cursors.
- **Saved games**: saving from the HUD and loading from the main menu, tested end to end.
- **Sound**: effects, music (MP3 streams), video audio; a few tens of milliseconds of output latency.
- **LAN**: host, discovery, join, ready, start and match between two instances on a virtual LAN
  (`tools/lan_test.sh`) and, on 2026-10-20, between two real machines on one network: the host on the
  development machine and the guest on a second computer (Ubuntu, no graphical session, run in a private
  headless compositor) running a binary built there from a clean copy of the repository plus the game files
  (nine minutes to build). Both lobbies showed 2/2 players, both ready, and both machines loaded the map.
  Only a short stretch of the match was observed; synchronisation over a long game is untested.

## Fixed along the way (each is a trap worth knowing)

| Problem | Cause |
|---|---|
| Intermittent crash entering a multiplayer match | `setjmp`/`longjmp` in the compression library; the lifter now supports it |
| Crash when saving | A jump table decoded over real code; blocks now fall through into already-decoded code |
| LAN discovery found nothing | Windows delivers broadcasts to a UDP socket bound to a unicast address, Linux does not |
| Camera jumped on right-drag | The game warps the cursor each step and reads the offset; the front end now emulates that |
| Terrain smeared while scrolling | Overlapping `Blt` on one surface copied forward instead of like `memmove` |
| Lag and late click sounds | The mixer blocked on a 64 KB pipe while holding its lock |
| Video silent, then distorted | Bink restarts audio if `AIL_sample_buffer_info` returns nonzero; then a carry bug in the 256-bit arithmetic behind `fsin`/`fcos`/`fpatan` corrupted the FFT tables |
| Save times two hours off | `GetLocalTime` returned UTC |

## Known differences from the original

- **Text** is drawn with Liberation Sans through an unhinted rasteriser. The original asks Windows for
  Arial (about a dozen size and weight combinations in the HUD alone) and gets hinted glyphs and integer advances.
  Widths agree because the fonts are metric-compatible; pixels, weights and spacing at small sizes
  do not.
- **x87 transcendental results** are correctly rounded; CPU makers' hardware differs from that in the
  last bit now and then.
- The game's writable files live in the game directory (as in the original).

## Open

- Long LAN games, and LAN between machines on different network setups (only one flat network tried).
- Pixel-level text parity (in progress).
- Long-session and whole-game replay comparison with the original.
- Platforms: Windows, macOS, ARM64 for the host layer.
- Replacing translated code with readable ports is **not** planned any more (see docs/LEGAL.md): the game runs on the translated code.

Notes on the lifter and the host layer: [`s4/docs/lift.md`](../s4/docs/lift.md), [`s4/docs/host.md`](../s4/docs/host.md).
