# Architecture

```
 your copy of the game                         this repository (hand-written)
 ─────────────────────                         ──────────────────────────────
 S4_Main.exe, *.dll  ──► tools/lift ──► s4/lifted/*  (generated locally, git-ignored)
                                              │
                                              ▼  calls into
                              s4/x86  s4/x87     CPU state, memory, MMX, software FPU
                                              │
                                              ▼  imports resolved by
                                  s4/host        Windows, DirectDraw, GDI, Winsock, Miles, Bink glue
                                              │
                                              ▼
                                  s4/play        window, input, audio output
```

The idea: do not rewrite the game from what it appears to do; translate what it *is*, check the
translation mechanically, and only then replace parts with readable code where that pays off.

## 1. The lifter (`tools/lift`)

Input: a PE file (executable or DLL). It decodes the code with the `iced-x86` decoder, discovers
functions (exports, entry point, relocations, direct calls, code addresses in data, jump-table targets,
optional Ghidra list), builds each function's control-flow graph and writes one Rust function per
original function:

```rust
pub fn f_18c2be80(c: &mut Cpu) {
    let (mut eax, mut ecx, /* … */) = /* registers from c */;
    let mut fl = Fl::default();
    let mut pc = 0x18c2be80;
    loop { match pc {
        0x18c2be80 => { /* instruction by instruction, flags exact */ … pc = 0x18c2bea3; continue; }
        …
    } }
}
```

Registers and flags are locals; memory, the stack and the FPU go through the runtime. Things that needed
special care, each with its own notes in [`s4/docs/lift.md`](../s4/docs/lift.md):

- **Jump tables** (over-wide tables decode garbage past their end; harmless, the real entries are exact).
- **Structured exceptions** (MSVC `__try`/C++ `throw`): functions with exception frames run under
  `catch_unwind`; the runtime raises a `GuestJump` carrying the target address; catch continuations
  are discovered as extra block starts.
- **`setjmp`/`longjmp`**: callers of `_setjmp3` get a re-entry block after the call.
- **Overlapping decodings**: a block that falls into code another instruction stream already decoded
  continues there (needed by a `memcpy` jump table).
- Segment registers are not modelled (flat memory); pushes of them use the Win32 user selectors.
- Instructions the game never executes (privileged ones, port I/O) become traps that fire only if run.

Calls from lifted code first try the lifted dispatchers (one per module) and then the host's shim table.

## 2. The runtime (`s4/x86`, `s4/x87`)

- `Mem`: a flat 32-bit address space inside one reserved 4 GiB host region; every module is mapped at
  its preferred base, as Windows would.
- `alu`: integer operations with every flag exact (the instruction tests compare all defined flags).
- `Fpu`: the x87 register stack with TOP, tags, status/control words and exception flags; MMX registers
  alias the same storage as on hardware. Arithmetic is done in software (`s4/x87`): 64-bit significand,
  every rounding and precision mode, correctly rounded transcendental functions. The game changes
  the control word at run time (a graphics call leaves rounding toward zero), so this is not optional.
- `string`: `rep movs/stos/cmps/scas` with the direction flag; `mmx`: packed integer instructions.
- `cpuid` reports an MMX-capable Pentium (the game refuses to start without MMX).

## 3. The host (`s4/host`)

Imports of the translated modules are bound to *shims*: Rust functions at magic addresses
(`0xfe000000 + 16·n`) that read their arguments from the guest stack and honour stdcall/cdecl.

| Area | Notes |
|---|---|
| `kernel32` | files (mapped to host paths, case-insensitively), heap, virtual memory, threads, events, mutexes (owned, recursive), critical sections, TLS, time (one clock for all threads), `GetLocalTime` in the host's time zone |
| threads | each guest thread is a host thread with its own CPU state; `CREATE_SUSPENDED` waits for `ResumeThread` |
| `user32`, `gdi32` | window and message queue, cursors (decoded from the game's resources), GDI bitmaps and text |
| text | GDI text through `ab_glyph` and Liberation Sans (Arial's metric-compatible stand-in); see [STATUS.md](STATUS.md) |
| `ddraw` | DirectDraw surfaces live in guest memory; `Blt` has `memmove` semantics; primary-surface updates are published to the front end |
| `winsock` | Winsock over host sockets, including Windows' rule that a UDP socket bound to an address still receives broadcasts, and `gethostbyname` of the machine's own name |
| Miles (`miles.rs`) | the sound system's API as the game uses it: WAV/IMA-ADPCM samples, MP3 music streams (decoded with `symphonia`), double-buffered samples (for video), end-of-sample/buffer callbacks, a mixer thread whose output goes to the front end's SDL audio stream |
| Bink | the game's own `binkw32.dll` is translated like every other module; the host only supplies its imports |
| SEH / `longjmp` | `seh.rs` and the lifter's support, see above |

## 4. The front end (`s4/play`)

SDL3 does the platform work: window (resizable; the game's screen is drawn through an SDL renderer with
a logical size equal to the game's screen mode, letterboxed, with pixel-art scaling), events, cursors
(the images the game selects, as SDL colour cursors), text input and the audio device. The program
turns SDL events into the `WM_*` messages the game's message loop reads (mouse buttons, wheel, keys and
characters; losing focus releases held buttons), and the game's mixer thread plays into an SDL audio
stream (`audio.rs`), which also paces it: the stream is kept about 40 ms full so a click is heard when it
happens. SDL's automatic mouse capture keeps drags working outside the window.

The game moves the cursor itself while the right button is held (the camera drag puts the cursor at the
screen centre every step and reads how far it was moved), which a desktop pointer cannot do. The front
end applies real pointer motion relative to the last warp target, and when the button is released it
moves the real pointer to where the game's cursor is (`SDL_WarpMouseInWindow`; on Wayland SDL emulates
the warp through pointer constraints). A placeholder frame is drawn until the game produces one
(Wayland maps a window only after its first buffer).

Because everything platform-specific in the front end is SDL, a port to another operating system only has
to port the host layer (`s4/host`: address-space reservation, file paths, sockets, threads).

## 5. File formats

The program needs a few of the game's file formats itself (WAV sound for the Miles shim, PNG for
screenshots). They come from `s4/formats-clean`, a self-contained crate (with its own specification, vectors and process records) written
in a clean-room process (specification plus test vectors, no access to the game's code or data).

The earlier approach, hand-written ports of single engine DLLs and a map viewer, was removed from the tree
(it stays in the git history of the private repository): the game runs entirely on the translated code, and
the translated code is generated locally from your copy of the game, as in other static-recompilation
projects. `s4/difftest` remains as the differential-test harness for the runtime.
