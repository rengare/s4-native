# Running

```sh
target/<host triple>/release/s4play <game directory>      # e.g. extracted/s4
```

The game starts the way it does on Windows: Ubisoft and Blue Byte logos, intro, then the main menu.
The first frame appears after a second or two. The window scales to the screen; the game itself runs
in its own modes (800×600 in the menus, 1024×768 in a match).

## Controls

As in the original game: left button selects and commands, right button drag moves the camera
(joystick-like: the farther you drag from where you pressed, the faster it scrolls), arrow keys scroll,
the screen edge scrolls when *Border scrolling* is on, Escape opens the in-game menu, and the HUD's
top-left buttons open options, save, load and quit. Alt+Enter toggles full screen. Holding the right button and moving works on a
touchpad as well.

## Files the game writes

The game keeps its settings, saved games and logs **inside the game directory**, exactly as the
original does: `Config/GameSettings.cfg`, `Save/`, `Log/`. Back it up if it matters. Saved games are
written by the game's own code (`Save/<name>.exe`).

## LAN

The game's own protocol over UDP/TCP (not DirectPlay). Create a game on one machine, join from the
other through the multiplayer → LAN screens as in the original. Both machines need to reach each
other's broadcast domain. `tools/lan_test.sh` runs two instances on a private virtual network on one
machine (user + network namespaces, no root) and was the first test; play between two real machines on one network has been tried too (see
[STATUS.md](STATUS.md)). Lobby buttons in game coordinates (800×600): Multiplayer 631,254 · LAN 571,174 ·
Create 609,547 · next 693,536 (host); Search host 368,158 · the listed game 255,243 (guest); Ready 618,550;
Start 700,547 (host, once everyone is ready); the gem at 533,550 is Back.

## Launcher

`tools/install_s4.sh` points a Lutris entry at the build (see [BUILDING.md](BUILDING.md)). Launchers
change the environment; if sound is missing there, the log shows which `audio:` step failed.


## Resolution and scaling

| Option (command line / variable) | Meaning |
|---|---|
| `--resolution auto\|keep\|800x600\|1024x768\|1280x1024\|1920x1080` (`S4_RESOLUTION`) | The game's screen mode. `auto` (the default) picks the biggest that fits your desktop: 1920x1080 on a full-HD desktop or bigger. `keep` leaves the setting of the game folder alone. The choice is written to `Screenmode` in `Config/GameSettings.cfg` (1 to 4), which is where the game reads it |
| `--scale pixelart\|linear\|nearest\|integer` (`S4_SCALE`) | How the picture is scaled to the window. `pixelart` (default): sharp pixels, smooth only at the edges between scales. `linear`: smooth. `nearest`: hard pixels at any scale. `integer`: hard pixels at whole-number scales only |

The game itself has the modes 800x600, 1024x768 and 1280x1024, and a fourth, 1600x1200, that this project turns into
1920x1080 (the lifter patches the size in the translated GfxEngine, see `tools/lift/patches/`, and the host changes the
game's own size table in memory). The engine draws any size with the layout of the 1280x1024 mode, so a larger mode
shows more of the map. The menus and videos stay at their own sizes and are scaled into the window. The window starts at
the mode's size, reduced to fit the desktop; Alt+Enter toggles full screen.

## Graphics

Terrain and objects are drawn on the GPU (the game's Direct3D 7 calls are emulated with wgpu, which needs Vulkan, or OpenGL as wgpu's fallback). At start the program looks for a graphics adapter and sets `LandscapeHardware` and `ObjectHardware` in `Config/GameSettings.cfg` to match: 1 when one is found, 0 (the game's own software renderer) when none is. `S4_D3D=0` forces the software renderer. The software renderer is slower, most noticeably at 1920x1080.

## Environment switches

All optional; for debugging and for the scripted tests.

| Variable | Effect |
|---|---|
| `S4_NO_AUDIO=1` | Mix but do not play (the mixer keeps time on the clock) |
| `S4_AUDIO_DUMP=file` | Write the mixer output (raw S16 stereo at the mixer rate) to a file instead of the sound device |
| `SDL_VIDEODRIVER=wayland\|x11`, `SDL_AUDIODRIVER=…` | SDL's own switches for the window system and the sound system |
| `S4_TITLE=text` | Window title (to tell two instances apart) |
| `S4_VSYNC=1` | Present synchronised with the display (default: as soon as the game has a new picture) |
| `S4_FONT=path.ttf` | TrueType font for GDI text (default: Liberation Sans, then DejaVu) |
| `S4_D3D=0` | Use the game's software renderer instead of the GPU (default: GPU when an adapter exists) |
| `S4_FPS=1` | Print frame statistics every second |
| `S4_AUTOCLICK="x,y@t[:l\|r\|m];…"` | Click at game coordinates x,y at time t seconds |
| `S4_AUTOKEY="vk@t;…"` | Press a virtual key at time t (held 1.5 s; letters and digits also send a character) |
| `S4_AUTODRAG="x0,y0,x1,y1@t[:l\|r]"` | Press at x0,y0 and drag to x1,y1 over 2 s |
| `S4_FRAME_DUMP=file` + `S4_FRAME_AT=t` | Save the game's frame as a PPM at time t |
| `S4_CURSOR_DUMP=dir` | Save every decoded cursor as an image |
| `S4_HOST_TRACE=1` | Log every call the game makes to the host (very verbose) |
| `S4_INPUT_TRACE=1` | Log SDL events, game cursor positions and cursor warps |
| `S4_WIN_TRACE=1`, `S4_MSG_TRACE=1` | Log window and message activity |
| `S4_JUMP_TRACE=1` | Log non-local jumps (exceptions, `longjmp`) |
| `S4_MIX_TRACE=1` | Log Bink audio buffer loads and consumption |
| `S4_BINK_DUMP=file` | Append the audio data Bink hands to the sound system (used to compare against a reference decoder) |
| `S4_READ_TRACE=1` | Log reads of `.bik` video files |
| `S4_CPU_FEATURES=hex` | CPUID leaf 1 EDX the game sees (the game refuses to start without MMX) |
| `S4_FPU_CW=hex` | Initial x87 control word of every thread |
| `S4_GIL=1` | Run one guest thread at a time, switching only at host calls |
| `S4_ZERO_HEAP=1` | Zero recycled heap blocks (to test for reads of uninitialised memory) |
| `S4_SKIP_VIDEOS=1` | The intro videos "do not exist": the game goes straight to the menu (use it in scripted tests; with videos, Escape skips them but the first click afterwards is swallowed) |
| `S4_STDOUT=1` | Show what the game writes to standard output |

Coordinates for the scripted clicks are the game's own (menu 800×600: Single Player 631,219 ·
Multiplayer 631,254 · Load Game 631,359; match start 548,419 then 702,547 three times).
