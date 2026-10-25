# Legal notes

These are the authors' working notes, not legal advice. If you plan to redistribute anything, get
advice for your jurisdiction.

## Rights and purpose

The author does not own any rights to *The Settlers IV* or *The Settlers III* and claims none. The project exists to
preserve the games: the author legally bought *The Settlers IV Gold* and *The Settlers III Ultimate Collection* from
GOG.com and wants to keep playing their own copies on current systems. It supplies no game content and works only with an original copy of the supported GOG versions (S4 2.50.1516, S3 Ultimate Collection 1.60). Rights holders who object to something
in this repository can open an issue and it will be looked at promptly.

## What this project is

A program that runs *The Settlers IV Gold* and *The Settlers III Ultimate Collection* from **your own copy of the game** on a modern Linux desktop
by translating the game's machine code to Rust on your machine and supplying the Windows functions the
game expects. It is **not a clean-room reimplementation**: the original executables were analysed
(disassembly, decompiler output, run-time observation) and are translated mechanically. A clean-room
project would have had to avoid exactly that. The consequences are deliberate:

- Anything built from the translation contains code derived from the game. The repository therefore
  does not contain it, and a built `s4play` or `s3play` should be treated like the game's own files: do not share it.
- The repository contains only code and text written for this project: the lifter, the runtime, the
  host layer, the front end, the tests, the tools and this documentation.
- `tools/lift/functions/*.tsv` (S4 only; The Settlers III is translated without a function list) list the start addresses of the functions of the supported build (plain
  numbers, produced with a decompiler's help, no names, no code).
- `tools/lift/patches/*.tsv` hold a few byte-level changes applied to your copy of a DLL in memory before it is
  translated (today: the size of screen mode 4, six 32-bit numbers in GfxEngine). They are addresses and numbers, no code.
- It contains one library derived from the game's *file formats*, `s4/formats-clean`, written in a
  **clean-room process**: one side wrote a specification and test vectors from observation of the formats
  (synthetic input files and expected results; no code, names or addresses; the terrain tables are constants taken from the game); a separate implementer (an AI coding agent running in a
  sandbox that showed only that repository) wrote the code from the specification and vectors alone. The
  repository of that library records the process (`PROCESS.md`, `PROCESS-LOG.md`), the evidence about the
  vectors (`vectors/PROVENANCE.md`) and the review (`REVIEW.md`: behaviour, differential fuzzing against a
  reference, token similarity). This reduces, but does not remove, the legal uncertainty: a clean-room process
  is evidence of independence, not a guarantee, and the specification side had read the original's code.
- Earlier versions of this project contained hand-written ports of single engine functions written from the
  disassembly. They were **removed** from the tree; they remain only in the history of the private repository.
  The program that runs the game (`s4/play`) uses none of them.

## What the repository contains and what it does not

| In the repository | Not in the repository (git-ignored, local only) |
|---|---|
| Lifter, runtime, host, front end, tests, tools, docs, the clean-room format library | The game's executables, DLLs, data files, installers (`extracted/`) |
| Cargo manifests of the generated crates | The generated Rust (`s4/lifted/*/src/`, `s3/lifted/*/src/`: for The Settlers III also the Indeo video codec `ir50_32.dll` of the game, translated like the executable) |
| | Decompiler output and Ghidra projects (`export/`, `ghidra/`) |
| | Screenshots and logs of the game (`logs/`) |
| Tests and the code that generates test cases | Generated conformance cases (`s4/x86test/src/cases.rs`) |

Screenshots of the game's art are not committed.

## Third-party material

- **Settlers4-Reforged** (a community reverse-engineering project) was consulted for class and function
  names only. It has no licence; no code was copied from it.
- **Wine's `sserife.fon`** (the free replacement for the raster font MS Sans Serif, LGPL 2.1+, by the Wine project)
  is read at run time from a Wine installation on your computer if there is one, for The Settlers III's text. It is
  not copied into this repository or into a build; without it a TrueType face is used. The Microsoft font itself is
  not used.
- **GOG's patched winmm** (`_INMM.dll` in the Settlers III folder) is not used: the host answers the game's calls to it
  itself, and plays the GOG edition's `Music/TrackNN.ogg` files for the game's CD-audio commands.
- **ffmpeg** is used only as a test oracle on the developer's machine; no ffmpeg code is part of this
  project.
- Rust crates used by the program (among them `sdl3-sys` (with the SDL3 library, zlib licence), `iced-x86`, `ab_glyph`,
  `symphonia`, `libc`) keep their own licences. Before releasing a binary, check them with
  `cargo license` or similar. Liberation Sans, used for text, is under the SIL Open Font License.
- *The Settlers* and related names are trademarks of their owners. This project is not affiliated
  with or endorsed by Ubisoft or Blue Byte.

## Licence of this repository

MIT (see `LICENSE`). It covers the files in this repository. It does not cover the game, the Rust
generated from your copy of it, or a binary built from that: those remain subject to the rights of the
game's owners.
