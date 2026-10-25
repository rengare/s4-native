# Testing and verification

Nothing here proves the port "perfect"; each layer has a check that would have caught the bugs found
so far, and the places where checks are weaker are listed at the end.

## 1. Instruction conformance (`tools/test_x86.sh`)

For every distinct instruction *form* that occurs in the translated modules (about 7,000: operation,
operand and addressing shape, immediate class), the test runs the instruction on the processor (under
Wine, 32-bit) and as translated Rust, from the same random registers, flags, memory and x87 state, and
requires equal registers, defined flags, memory window and x87 state (registers, tags, TOP, condition
codes, exception flags). Hundreds of thousands of runs per pass.

```sh
tools/test_x86.sh                      # regenerate cases from the game's modules if missing, then run
S4_X86_ITERS=200 tools/test_x86.sh     # more runs per form
S4_X86_ONLY=fsub tools/test_x86.sh     # one mnemonic
REGEN=1 tools/test_x86.sh              # regenerate the cases
```

Excluded on purpose (see [`s4/docs/lift.md`](../s4/docs/lift.md)): flags the manuals call undefined,
divide errors, x87 values outside the normal range, forms that do not occur in the game.

**Coverage is per module.** The default case list comes from `S4_Main.exe` and its DLLs. Modules added
later bring new forms: translating `binkw32.dll` introduced `fsin`, `fcos`, `fpatan` and others the game
itself never executes, and a bug in them (below) went unnoticed until the sound was wrong. To check
another module: `target/<host>/release/lift cases s4/x86test/src/cases.rs <module.dll>`, then run with
`S4_X86_FROM`/`S4_X86_TO` around the segment-register push/pop forms, which crash the native side by
design. On `binkw32.dll` the remaining differences are `fprem`/`fprem1`/`fptan`/`fscale`/`fyl2x`/`f2xm1`
on invalid arguments, `fstp m80` of some special values, the memory image written by `fnsave`/`fnstenv`
and the selector values pushed by `push cs/ds/es`.

## 2. The software x87 (`s4/x87`)

`cargo test -p s4-x87` plus `s4/x87/tests/x87.rs` compare against the hardware FPU for arithmetic in all
rounding and precision modes. The transcendental functions are correctly rounded and agree with
hardware to within an ulp or two over their normal ranges (hardware results differ between CPU makers
in the last bit; near zero crossings of `fsin`/`fcos` the hardware loses relative precision because its
internal π is only 66 bits, so those differ more in relative terms, below 1e-15 absolute).
A regression test covers the argument-reduction bug that corrupted Bink's audio.

## 3. Function level and whole frame

- `s4/difftest`: the differential-test harness (loads the original DLLs under Wine and compares translated
  functions with them on random inputs; coverage recording).
- `s4/formats-clean`: its own test suite (`cargo test` inside that directory): specification vectors for every
  format, plus the review and differential-fuzzing record in its `REVIEW.md`.

## 4. Original under Wine versus native

`tools/wine_session.sh` drives the **original** game under Wine through the same click sequence the
`S4_AUTOCLICK` switch gives the native program, on a private headless display (`tools/headless.sh`),
and screenshots it; `tools/compare_frames.py` compares frames. Results so far: the main menu differs
only in GDI text; the start of a match differs in text and in animations driven by the clock
(swaying grass, butterflies). Scripted camera moves match the original's positions.

```sh
tools/headless.sh tools/wine_session.sh logs/wine       # original
S4_AUTOCLICK="631,219@30;548,419@40;702,547@50;702,547@60;702,547@70" \
  S4_FRAME_DUMP=logs/native.ppm S4_FRAME_AT=100 target/<host>/release/s4play extracted/s4
```

## 5. Audio and video against a reference decoder

`ffmpeg` is used **only as a test oracle**: decode a `.bik` video's audio with it, run the game with
`S4_BINK_DUMP=file` to capture what the translated Bink hands to the sound system, and compare sample
by sample. For `UbiSoft_Logo.bik` the two are identical. This is how the x87 argument-reduction bug was
found: the output was close to the reference and noisy, which pointed at table generation, not at the
decoder logic. `S4_AUDIO_DUMP` captures the final mix for level checks without a speaker.

## 6. LAN

`tools/lan_test.sh` creates a virtual network between two network namespaces (no root needed), starts
two instances, creates a game on one, discovers and joins from the other, readies both and plays the
start of a match. Used to find the Winsock and `setjmp` problems; run it after changes there.

## What is not covered

- Only parts of the game have been *played*; there is no whole-game replay comparison against the
  original yet.
- Animations cannot be compared pixel by pixel without a shared clock.
- Text: Liberation Sans stands in for Arial.
- Real two-machine LAN, other CPU architectures and operating systems.
