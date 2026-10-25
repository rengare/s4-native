# The lifter (`tools/lift`) and its proof

Strategy (after looking at how other decompilations were done: matching decomps such as
SM64/LEGO Island, literal recompilers such as N64Recomp/XenonRecomp, and OpenRCT2's
one-procedure-at-a-time replacement inside the running original): **lift every function
literally to Rust and run the game on the result**, like the literal recompilers do. (An earlier plan to
replace functions one by one with hand-written ports was dropped, see `docs/LEGAL.md`.) The lifted code is derived from the game's binaries, so it is
generated locally (`tools/lift.sh`) and never committed (`s4/lifted/*/src/` is ignored).

## Pieces
| | |
|---|---|
| `tools/lift` | iced-x86 decoder, control-flow discovery per function (jump tables included), Rust emitter. One Rust function per original function, `loop { match pc { block => ... } }`, registers and flags in locals, memory/stack/x87 through `s4-x86`. `lift cases` writes the conformance cases. |
| `s4/x86` (`s4-x86`) | Runtime: `alu` (flag-exact integer ops), `Fpu` (x87 register stack, TOP, tags, status/control words, C1/PE/IE/ZE), `mmx`, `string` ops, `Mem` (flat 32-bit address space at a base pointer), `native` (Wine only: call code that is not lifted — the original's functions, system DLLs — on the emulated stack; execute single instructions on the CPU). |
| `s4/x87` (`s4-x87`) | The software x87 arithmetic (64-bit significand, all rounding/precision modes, sqrt, NaN payloads, status of each rounding). Checked against the hardware in `tests/x87.rs`. |
| `s4/lifted/<module>` | Generated crates (`tools/lift.sh GfxEngine.dll gfxengine`). |

Calls from lifted code to anything not lifted go through `Cpu::call(addr)`: lifted dispatchers
first, then the fallback (the native bridge under Wine, shims in the native program).

## Proof that the lifter is right
1. **Instruction conformance** (`tools/test_x86.sh`, crate `s4-x86test`): every distinct
   instruction form that occurs in the game's code (S4_Main.exe and its DLLs, about 7,000
   forms: registers, addressing shape, immediate class) runs on the processor (under Wine) and as
   lifted code from the same random registers, flags, memory and x87 state; registers, defined
   flags, memory, x87 registers/tags/TOP/condition codes/IE/PE/SF/ZE must be equal
   (480,000 runs per pass). Excluded on purpose and listed here:
   * flags the manuals call undefined (also OF of multi-bit shifts and rotates);
   * divide errors (the lifted code panics where the CPU faults);
   * x87 values outside the normal extended range (denormals, overflow, underflow panic with
     `x87:` instead of guessing), and the OE/UE/DE exception flags;
   * forms not in the game's real code (segment loads, I/O, BCD, 16-bit push/pop, `enter`,
     `xlat`): `lift cases` lists them in `cases.rs.unsupported.txt`;
   * transcendental x87 instructions, `fprem`, `fscale`, `frndint`: only the CRT libraries use them
     (`MSVCRT.DLL`), not the game's code.
2. **Function level** (`s4/lifted/gfxengine/tests/diff.rs`): lifted function vs. original DLL on
   random inputs (the triangle selector so far).
3. **Whole frame** (`GFXREF_LIFTED=1` in `tools/gfxref`): the back buffer is clobbered, then
   *all 342 lifted GfxEngine functions* run `RenderFrame`; the 800x600 frame equals the
   original's pixel for pixel.

## Known limits
* The lifted code needs the module at its preferred base (Wine loads GfxEngine.dll at 0x18c00000).
* (Out of date notes removed: structured exceptions, `setjmp`/`longjmp`, `cpuid` and the
  segment-register pushes are handled now; see `docs/ARCHITECTURE.md` and `s4/docs/host.md`.)
* The conformance cases cover the instruction forms of the modules they were generated from; see
  `docs/TESTING.md` for checking a further module.
