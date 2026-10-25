# Settlers IV and Settlers III, natively

Native ports of *The Settlers IV Gold* and *The Settlers III Ultimate Collection* (both GOG editions): the original game, played from your own copy, without Wine. Linux is the supported platform; macOS (Apple Silicon) builds and runs both games but is experimental. The Settlers IV is described first; [Settlers III](#settlers-iii) follows.

**To run it** you need Linux, [Rust](https://rustup.rs), the SDL3 library (Ubuntu/Debian: `sudo apt install libsdl3-0`) and your own copy of *The Settlers IV Gold* (GOG; the folder that contains `S4.exe` and `Exe/`). Then:

```sh
git clone https://github.com/rengare/s4-native && cd s4-native
tools/setup.sh /path/to/your/game     # or the GOG installer: tools/setup.sh ~/Downloads/setup_the_settlers_4_gold_*.exe
```

`tools/setup.sh` unpacks the installer if you gave one (needs `innoextract`), checks that the game is the supported version, checks Rust / SDL3 / memory, translates the game's code on your machine and builds the program (a few minutes at `-j14`; about 1 GB of memory per parallel job). It ends by printing the command that starts the game (or add `--run`).

Window, input and sound go through SDL3, so any audio system SDL supports works. Something not working? See [docs/BUILDING.md](docs/BUILDING.md).

A native Rust port of *The Settlers IV Gold* (GOG edition 2.50.1516). It plays the real game — menus,
campaigns and single-player maps, saved games, intro videos, music and effects, LAN — without Wine, on
the Linux desktop (Wayland or X11).

> **You need your own copy of the original game to build or run this.** *The Settlers IV Gold* (GOG,
> version 2.50.1516) is not included, not downloaded and not replaceable: the program is built *from*
> your copy's executables and plays *with* its data. Buy it, for example, at GOG. See
> [Legal status](#legal-status) for exactly what this repository does and does not contain.

## Status

Verified by hand and by scripted sessions on x86-64 Linux:

| Works | Notes |
|---|---|
| Start-up, main menu, single player, saved games | In-match save and load tested end to end |
| Rendering | The game's own software renderer; menu and match start were compared against the original running under Wine (differences: text, and animations driven by the clock) |
| Mouse, keyboard, cursors, scrolling | Right-drag camera as in the original; touchpads work |
| Music, effects, intro videos with sound | Video audio is bit-identical to a reference decoder on the file tested |
| LAN | Two real machines on one network: host here, guest on a second computer running a copy built from a clean clone — discovery, join, ready, start and the match running on both. Also testable on one machine (`tools/lan_test.sh`) |

Not done yet: text uses Liberation Sans instead of Arial (layout is
metric-compatible, pixels differ); long-session soak testing; platforms other than Linux (aarch64 Linux builds and runs; macOS on Apple Silicon builds and runs, with the menus checked there but not a full match).
Details and the history of each fix: [docs/STATUS.md](docs/STATUS.md).

## Settlers III

*The Settlers III Ultimate Collection* (GOG, version 1.60) runs the same way: `S3.EXE` and its Indeo video codec are translated on your machine, and `s3play` is the program. The build is described in [s3/README.md](s3/README.md) (the game's folder is the one with `S3.EXE`):

```sh
tools/lift_s3.sh /path/to/the/s3/game
cargo build --release -j8 --target "$(rustc -vV | sed -n 's/host: //p')" -p s3-play
target/<host triple>/release/s3play /path/to/the/s3/game
```

| Works | Notes |
|---|---|
| Menus, single-player scenarios on random maps, campaign briefings with voice-overs, intro movie | Checked by scripted headless sessions; the screen modes are the game's own (640×480, 800×600, 1024×768; `--resolution`) |
| Mouse and keyboard, text entry, scrolling | Right-drag camera follows the mouse; edge scrolling is the game's option (registry `ScrollMode`, off by default) |
| In-game texts and menus | The panel texts and the Save/Quit menu (the game draws all of them into a 1-bit bitmap) |
| Music and effects | The CD tracks are the GOG `Music/TrackNN.ogg` files; the next track follows when one ends |
| LAN | The game's DirectPlay is implemented by the host (UDP discovery, TCP to the host). Tested between two real machines (x86-64 and aarch64): join, ready, start, match running on both, about half a minute |
| Text | The raster font "MS Sans Serif" is drawn from Wine's free replacement if a Wine installation is found, else with a TrueType face |

Not done: a long LAN match, campaign videos beyond the intro, screen sizes beyond 1024×768, a GPU path (S3 draws on the CPU), comparison with the original under Wine. Build time: the lifter splits the game's code into 64 crates, so a clean build takes about 5 minutes at `-j14` (about 1 GB of memory per job; one crate took 45 minutes). The Settlers IV is split into 48 crates and builds in about 2 minutes at `-j14`; the build time given in the S4 instructions above predates that.

## Quick start

The four commands are at the top of this page; details, troubleshooting and the optional Lutris launcher are in [docs/BUILDING.md](docs/BUILDING.md) and [docs/RUNNING.md](docs/RUNNING.md).

## How it works, in one paragraph

The game is a 32-bit Windows program. A lifter (`tools/lift`) translates every function of its
executable and DLLs, instruction by instruction, into Rust, on your machine, from your files. A runtime
(`s4/x86`, `s4/x87`) gives that code a 32-bit address space, flag-exact integer arithmetic, MMX and a
software x87 FPU. A host layer (`s4/host`) supplies what the game expects from Windows — files,
threads, DirectDraw, GDI, Winsock, the Miles sound system and Bink video (the latter two are the game's
own libraries, translated the same way) — and a small SDL3 front end (`s4/play`) provides the window,
input and audio output, which is what makes other platforms a matter of porting the host layer, not the front end. Nothing in the repository is the game's code. The translation is checked against the
processor itself. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/TESTING.md](docs/TESTING.md).

## Repository map

| Path | What it is |
|---|---|
| `tools/lift` | The x86 → Rust lifter |
| `s4/x86`, `s4/x87` | Runtime for translated code: CPU state, memory, MMX, software x87 |
| `s4/host` | Windows environment for the translated code (Win32, DirectDraw, GDI, Winsock, Miles, Bink support) |
| `s4/play` | The program: window, input, audio, `S4_*` debugging switches |
| `s3/play`, `s3/lifted/*` | The Settlers III program and its generated crates (git-ignored sources) |
| `s4/lifted/*` | Crates the lifter fills in locally (**git-ignored**; only their `Cargo.toml` is tracked) |
| `s4/formats-clean` | Readers and writers for the game's file formats (WAV, PNG, archives, maps, sprites, …), a **clean-room** implementation: written from a specification and test vectors only; the process log and review are in that directory |
| `s4/difftest`, `s4/x86test` | Differential tests against the original binaries and the processor |
| `tools/` | Lifter drivers, test drivers, Wine and headless-display helpers, installer for Lutris |
| `docs/` | Documentation (below) · `s4/docs/` — notes on the lifter and the host layer |

## Documentation

- [docs/BUILDING.md](docs/BUILDING.md) — requirements, building, troubleshooting
- [docs/RUNNING.md](docs/RUNNING.md) — running, controls, saves, LAN, every `S4_*` switch
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — the pipeline from binary to running game
- [docs/TESTING.md](docs/TESTING.md) — how correctness is established
- [docs/STATUS.md](docs/STATUS.md) — what works, what does not, known differences
- [s3/README.md](s3/README.md) — The Settlers III: building, running, LAN
- [docs/LEGAL.md](docs/LEGAL.md) — what the repository contains and what it does not
- [s4/docs/lift.md](s4/docs/lift.md), [s4/docs/host.md](s4/docs/host.md) — notes on the lifter and on the host layer

## Legal status

**This project owns no rights to *The Settlers IV* or *The Settlers III*.** It is a preservation effort by someone who legally bought the
game from GOG.com and wants it to keep running on current systems. The game's code, art, sound, music and
videos belong to their rights holders (Ubisoft / Blue Byte); none of it is included here, and the project is
not affiliated with or endorsed by them. To use it you need your own legally obtained copy of the game.

This is **not a clean-room reimplementation**. A clean-room project never looks at the original code;
this one is built by analysing and mechanically translating the original executables, which is a
different approach with different consequences. What keeps the repository itself free of the
publisher's material:

- no executables, DLLs, assets, installers or decompiler output are committed (`.gitignore`);
- the translated Rust is generated on your machine from your copy and is never committed;
- everything that *is* committed was written for this project: the lifter, the runtime, the host layer,
  the front end, tests, tools, documentation, and the file-format library `s4/formats-clean`, which was
  written in a clean-room process (a specification and test vectors were the implementer's only inputs; see
  `s4/formats-clean/PROCESS.md`, `PROCESS-LOG.md` and `REVIEW.md`).

You must own the game. The authors are not affiliated with Ubisoft or Blue Byte; *The Settlers* is
their trademark. Read [docs/LEGAL.md](docs/LEGAL.md) before redistributing anything built here — a
built `s4play` binary contains code translated from the game and should not be shared.
The files in this repository are under the [MIT licence](LICENSE). That covers this repository only: not
the game, and not what you generate from your copy of it.
