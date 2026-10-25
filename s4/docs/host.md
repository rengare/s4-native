# The native host (`s4-host`) and the player (`s4-play`)

`s4-host` runs the lifted modules (`s4/docs/lift.md`) natively, on a Unix-like host, with no Wine
and no original code: a 4 GiB guest address space (lazy `mmap`), the PE images mapped at their
preferred bases, imports bound to the lifted modules' exports or to Rust implementations
("shims"), guest threads as host threads (own `Cpu`, shared memory).

## Run it
```
tools/lift_all.sh                       # lifts every module of extracted/s4 (not committed)
cargo build --release --target <host> -p s4-play      # use -j 4 on a 16 GB machine
target/<host>/release/s4play extracted/s4             # window, mouse, keyboard
```
With the opt-level 1 build of the lifted S4_Main (workspace `Cargo.toml`), the menu appears after ~30 s and
a match (Single Player, first map, Start twice) is running ~60 s later: landscape, buildings, settlers and the
side panel render. The minimap is drawn (only the revealed area; the rest is fog at match start). The menus work with the mouse. Env: `S4_HOST_TRACE=1` (every shim call),
`S4_MSG_TRACE`, `S4_INPUT_TRACE`, `S4_AUTOCLICK="x,y@secs;..."` (scripted clicks, game coordinates),
`S4_SHOT=<png> S4_SHOT_FLIP=<n>` (screenshot of the n-th flip, `run_game` example),
`S4_FONT=<ttf>`.

## What is implemented (`s4/host/src`)
| module | |
|---|---|
| `lib.rs` | loader, import linking (`World::load_game`), shim registry, `GetProcAddress`/`LoadLibrary` tables |
| `kernel32.rs` | files (case-insensitive, `C:\S4` = the game directory), heap, threads, events, critical sections, TLS, time, locales, console stubs |
| `user32.rs` | windows, message queue, input state, `wsprintf`; the front end posts messages with `post_message` |
| `gdi.rs`, `text.rs` | bitmaps/DCs, `GetPixel`/`BitBlt`, text with a system font (not the original's GDI glyphs) |
| `ddraw.rs` | DirectDraw 7 (and v1 `DirectDrawCreate`): system-memory surfaces in guest memory, Lock/Blt/Flip, no Direct3D device |
| `winsock.rs` | Winsock on BSD sockets (LAN) |
| `seh.rs` | `RaiseException`/`RtlUnwind`; C++ exceptions work through the lifted MSVCRT `__CxxFrameHandler`, catch continuations are `GuestJump` panics caught by the owning lifted function |
| `system.rs`, `audio.rs` | registry (empty), winmm timers, version, oleaut32, shell32; Miles (`mss32`) and Bink (`binkw32`) answered without sound/video |
| `present.rs` | the last primary-surface image for the window front end |

## Known deviations / open
* GDI text uses a host font; sound and video are stubs; no real cursors (system cursor shown).
* The lifter treats code addresses it cannot see (function pointers in data, catch continuations) heuristically;
  a missed one shows up as "nothing lifted at <address>" or an unowned `GuestJump` (see `S4_JUMP_TRACE`).
* Windows and C runtime behaviour comes from the lifted MSVCRT/MSVCP60; kernel32 etc. are re-implemented,
  so their edge cases are not proven equal to Windows.

## Scripted input (s4play)
- `S4_AUTOCLICK="x,y@secs[:l|r|m];..."` clicks in game coordinates (`l` left, `r` right, `m` move only and stay).
- `S4_AUTOKEY="vk@secs;..."` presses virtual keys (decimal or `0x..`) for 1.5 s.
- Checked in a running match: arrow keys scroll the view; the cursor at the top edge scrolls with `BorderScroll = 1`
  in `Config/GameSettings.cfg` (the option is off by default). The game's display mode (not the desktop size) is what
  `GetSystemMetrics` reports.

## Equivalence check against the original (main menu, 2026-10-19)
`S4_FRAME_DUMP=out.ppm S4_FRAME_AT=12 s4play ...` writes the game's frame; the original runs under Wine in
`tools/headless.sh` (`WINEPREFIX=target/wineprefix-game wine S4.exe`, menu after ~120 s because of the intro videos)
and is captured with `grim`; `tools/compare_frames.py native.ppm shot.png diff.png` compares both as RGB565.
Result for the main menu: 7,815 of 480,000 pixels differ, all of them the GDI text (button labels, footer) -
the host draws GDI text with a substitute font. Everything the lifted engine renders is identical.
The native port skips the intro videos (Bink is not implemented).

### In-match comparison (start of the first campaign map, 2026-10-19)
`tools/headless.sh tools/wine_session.sh logs/wine` drives the original under Wine with XTest input (`tools/xinput.py`;
Escape skips the intro videos, the script waits for the menu, then clicks through to the match) and screenshots it;
`s4play` runs the same clicks (`S4_AUTOCLICK`, `S4_FRAME_DUMP`). The match is 1024x768. 72,431 of 786,432 pixels
(9%) differ; the difference image shows only: GDI text (resource numbers), and time-driven animation (swaying
grass and trees, butterflies, shoreline shimmer) because the two runs are at different game times. Terrain, buildings,
GUI frame, panels and minimap are identical. Identical timers would be needed to compare animation exactly.

## LAN (2026-10-19)
The game asks `gethostname` + `gethostbyname` for its own address; the host now returns the real host name and, for it,
the address of the interface that routes out (UDP connect trick, nothing is sent). With that the Multiplayer -> LAN
lobby opens natively (sockets, broadcasts, `select` polling). Two-instance host/join not tested yet.

## Right-drag camera test (2026-10-19)
`S4_AUTODRAG="x0,y0,x1,y1@secs[:l|r]"` (s4play) and `xinput.py "drag x0 y0 x1 y1 3 2.0"` (original under Wine) do the same
2-second right-button drag from (500,300) to (300,300) at the start of the first map. Both end with the view at the
top-left corner of the map (black fog): the native port behaves like the original in this test. `S4_FPS=1` prints
draw time and game frames per second (about 160 game fps in a match on the dev machine).

### LAN host and discovery (2026-10-19)
`tools/headless.sh tools/lan_test.sh logs/lan`: two native instances on a private virtual LAN (user+net namespaces, veth,
10.0.0.1 / 10.0.0.2; no root). A hosts a game (Multiplayer, LAN, Create, next), B opens the LAN lobby and searches:
B lists "ren's game / 4 isles / 1-2". Needed: the game binds its UDP sockets to its own address and expects broadcasts on
them (Windows semantics); the host binds such sockets to the wildcard address and reports the original address from
`getsockname`. Two instances on one machine without namespaces cannot work (the fixed port is taken), as on Windows.
Not tested yet: joining and playing across instances, and over a real second machine.

### LAN match across two instances (2026-10-19)
`tools/lan_test.sh` (env `HOST_CLICKS`, `GUEST_CLICKS`, `EXTRA_SHOTS`, `SHOT_EVERY`): host Create + next, guest Search + join
(2/2 players), guest Ready (oval), host Ready (oval), host Start (right gem): both instances load the map and run the
match (host red, guest blue). The host crashed intermittently at the start with "a jump to 0x10003757 found no
owner": RTComp uses `setjmp`/`longjmp` (error handling around its file reads) and a `longjmp` is a non-local jump to the
address after `call _setjmp3`. The lifter now treats functions that call `setjmp` (directly or through an import thunk)
like SEH functions: wrapped in `catch_unwind`, with the address after the call as a block that a `GuestJump` resumes.
Changed output: one file each in S4_Main, RTComp and Utility. After the fix: 4 of 4 matches started and ran on both instances without a crash (before: about 3 of 5 starts crashed the host).

## Save and load in a match

Scripted path (800x600 menu, 1024x768 match): HUD top-left button (18,62) opens the
Save/Briefing/Quit tabs; Save is the first icon (22,230), the name field is at (90,521) and the
save button at (57,574). Loading is main menu Load Game (631,359), the map-icon list (360,350),
the entry (323,120), the gem (690,548). A saved and reloaded match shows the same HUD
(settler count etc.). `S4_AUTOKEY` now also posts `WM_CHAR` for letters, digits and space.

Two bugs found on the way:

- Saving crashed in `memcpy` (MSVCRT 0x7801024a): the jump table is over-wide, so the lifter's
  decode of its garbage entries overlapped the real code, and a block that fell into
  already-decoded code of another instruction stream ended in `panic!("fell off")`. The
  lifter now makes such a join a leader and emits `pc = next; continue` (all modules re-lifted).
- `GetLocalTime` and the file-time conversions returned UTC, so save times were two hours off.
  They now use the host's UTC offset (`GetTimeZoneInformation` bias too).

The lifter also handles `fnstenv`, `fldenv`, `fnsave` and `frstor` now (MSVCRT's trig and
`fmod` slow paths use them); they were the only real x87 gaps. The other `gaps.txt` entries
(`push es`, `pop es`, `les`, `cli`, ...) come from garbage decoded after over-wide jump tables.

## Audio check without a speaker

`S4_AUDIO_DUMP=file.pcm` writes the mixer's raw S16 stereo (paced by the clock) instead of
playing it (the front end now plays through SDL3). A 28 s run through menu music and match start gave non-silent audio
(RMS 1000-3300, peak about 21000 of 32767, no clipping). Not yet listened to by a person.

## Scrolling glitches (overlapping Blt) and right-drag

- The landscape scroll blits a surface onto itself with overlapping rectangles; `copy_rect`
  copied forward pixel by pixel, so each scroll step smeared the image (streaks, wrong tiles).
  It now reads the source rectangle first (memmove semantics). After the fix a scripted
  scroll looks like the original's.
- The game warps the cursor to the screen centre (`SetCursorPos(512, 384)`) on every step of
  a right-drag and reads the pointer's distance from it. The front end now applies real pointer
  motion relative to the last warp target (Wayland cannot warp the pointer), and drops that
  offset when all buttons are released. `S4_INPUT_TRACE` logs the warps.
- The audio thread no longer holds the mixer lock while writing to the sink (the pipe blocked,
  stalling the game thread's Miles calls: visible lag).
- Original under Wine: `WINE_EXTRA` in `tools/wine_session.sh` is now word-split, so several
  quoted xinput commands can be passed.

## Video (the original binkw32.dll, lifted)

`binkw32.dll` is lifted like the other modules (`tools/lift_all.sh`, crate `lifted/binkw32`) and
loaded before VideoEngine; the game's `Bink*` imports bind to it. It needs, beyond the usual
shims: its window/GDI/waveOut imports (`audio.rs`; waveOut is refused, Bink then uses Miles),
real suspended threads (`CREATE_SUSPENDED` + `ResumeThread`, two worker threads per video),
recursive Windows mutexes with an owner, `WaitForMultipleObjects`, one clock shared by all
threads (`QueryPerformanceCounter`/`timeGetTime` used to be per thread), and the Miles
double-buffer API: `AIL_set_sample_type` (format), `load_sample_buffer`, `sample_buffer_ready`
(a free buffer or -1), `minimum_sample_buffer_size`, `register_EOB_callback`, sample user data.
Playback starts once both buffers are loaded (Bink never calls start). Pitfall: if
`AIL_sample_buffer_info` returns nonzero, Bink restarts its audio at every frame (silence).
Check without a speaker: `S4_AUDIO_DUMP`; `S4_MIX_TRACE=1` traces buffer loads and consumption.
`S4_CPU_FEATURES=<hex>` sets CPUID.1:EDX (the game itself refuses to start without MMX).

Video sound was distorted because of an x87 bug, not the audio path: `Big::add` in `s4/x87`
(the 256-bit arithmetic behind the transcendental instructions) picked the larger operand
by comparing mantissa limbs from the wrong end, so `fsin`/`fcos`/`fpatan` argument reduction
was wrong just above multiples of pi/2. Bink builds its FFT twiddle tables with them.
Found by decoding the same video's audio with ffmpeg as a reference (a test oracle only) and
comparing sample by sample (`S4_BINK_DUMP=file` writes what Bink hands to Miles); after the
fix the output is bit-identical to ffmpeg's. `tools/test_x86.sh` only covers the instruction
forms of the game modules; to check another module (e.g. binkw32.dll) regenerate cases
(`lift cases s4/x86test/src/cases.rs <dll>`) and skip the segment-register push/pop forms
(`S4_X86_FROM`/`S4_X86_TO`): after the fix only fprem/fprem1/fptan/fscale/fyl2x/f2xm1 on
invalid arguments, fstp m80, fnsave/fnstenv images and the selector pushes differ.
