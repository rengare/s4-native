# Review of the implementation (specification side)

Reviewed commit `a42c14e` ("impl: add cipher and decompressor"), 2026-10-21.

## Process
- `PROCESS-LOG.md` present: one session, lists the repository files read and states that nothing from the
  prohibited sources was used. No `QUESTIONS.md` was needed.
- Diff of the implementation commit: only `src/lib.rs` and `PROCESS-LOG.md` (no vectors, spec or tests touched).
- No `unsafe`, no dependencies, no `unimplemented!` left.

## Behaviour
- `cargo test`: all 7 tests pass (vectors, long key streams, chunking, involution, default helper, damaged input).
- **Differential fuzzing against the reference implementation** (not available to the implementer): 370,400
  decompressor cases (random byte strings, bit-flipped / truncated / wrong-length / trailing-junk variants of
  every vector) and 20,000 random keys for the cipher. Every result is identical except for 22 cases whose
  stream contains a rebuild with a class width above 32 bits, a corner the specification lists as unspecified
  (section 2.8). No gap in the specified behaviour was found.

## Independence
- Token similarity of the implementation with the reference code, after renaming identifiers: cipher 0.49,
  decompressor 0.28. The cipher is high because the specification states its bit operations as formulas
  (a primitive this small is determined by its formulas); the decompressor, which the specification describes
  in prose and tables, is structurally different (bit-at-a-time reader, inline tables, different control flow).

## Notes
- Slower than the reference (bit-at-a-time reading); irrelevant for the data sizes involved.

---

# Review of batch 2 (commit `da1ac5d`)

- Process: `PROCESS-LOG.md` second entry present; only `src/lib.rs` and the log changed; no questions raised.
- `cargo test`: all tests pass (cipher 5, compress 2, format 1, texts 2, archive 3).
- Differential fuzzing against the reference: 114,214 comparisons (header and index parsing of mutated vectors and
  random bytes, entry numbers, string-table parsing, language fallbacks, archive parsing, entry reads and path
  lookups on 2,758 mutated archives that still parsed). No differences.

---

# Review of batch 3 (commit `1caecc9`)

- Process: log entry present; only `src/lib.rs` and `PROCESS-LOG.md` changed; no questions raised (the log notes
  that the first test run found a zero-length lookup boundary in the palette code, fixed from the vectors).
- `cargo test`: everything passes (palette 2, png 1, sound 2, plus the earlier suites).
- Differential fuzzing against the reference: 400,202 comparisons in total over the whole crate; the batch 3
  part covers all 65,536 colours in each of three formats, extensions, random palette files and lookups
  (including offsets below the header and huge lengths), 600 random images and 8 large ones for the PNG writer,
  mutated WAV files (PCM and ADPCM variants) and mutated sound banks (`.snd` and `.sil` separately). No
  differences.
- Token similarity with the reference code (identifiers renamed): palette 0.66, png 0.33, sound 0.28. The
  palette module is small and its public items (type names, fields, the colour-scaling formula) are fixed by the
  specification and by the stubs, which explains the higher figure.

---

# Review of batch 4 (commit `2fdc359`)

- Process: log entry present; only `src/lib.rs` and `PROCESS-LOG.md` changed; no questions raised. The log
  says `cargo fmt --check` reported differences across the project and that formatting was left alone (fine).
- `cargo test`: everything passes (dialogs 3, landscape 3 plus the earlier suites).
- Differential fuzzing against the reference: 420,902 comparisons over the whole crate; the batch 4 part is
  mutated dialog-set files and mutated landscape files (parse results, record positions, hardware pages).
  No differences.
- Token similarity with the reference code (identifiers renamed): dialogs 0.43, landscape 0.52. Both formats are
  a short, fully determined walk over a table; the specification states the procedure step by step, so similar
  control flow is expected.

---

# Review of batch 5 (commit `b60d84e`)

- Process: log entry present; only `src/lib.rs` and `PROCESS-LOG.md` changed; no questions raised.
- `cargo test`: everything passes (map 4 plus the earlier suites).
- Differential fuzzing against the reference: 434,702 comparisons over the whole crate; the batch 5 part is
  mutated map files (parse, chunk positions, every chunk's decompressed data or error, `size`, the three record
  lists, file checksum) and 3,000 random byte strings for the CRC. No differences.
- Token similarity with the reference code (identifiers renamed): map 0.37.

---

# Review of batch 6 (commit `c21d2f7`)

- Process: log entry present; only `src/lib.rs` and `PROCESS-LOG.md` changed; no questions raised. The implementer
  sandbox (a container) has no `git`, so the specification side made the `impl:` commit from the implementer's
  working tree, unchanged. The log records that `git` was missing.
- `cargo test`: everything passes (sprites 6 plus the earlier suites).
- Differential fuzzing against the reference: 446,952 comparisons over the whole crate; the batch 6 part is
  image headers at several positions, whole mutated sprite files (every image and pixel list, errors included)
  and 4,000 random animation tables. No differences.
- Token similarity with the reference code (identifiers renamed): sprites 0.50 (the run-length scan and decoder
  are step-by-step procedures in the specification).

# Coverage

Batches 1 to 6 cover every module of the reference crate except the terrain colour tables (deliberately left
out: they are data, not behaviour) and the small game-folder lookup helper.

---

# Review of batch 7 (commit `d67f2ba`)

- Process: log entry present; only `src/lib.rs` and `PROCESS-LOG.md` changed; no questions raised. The implementer
  committed by itself this time.
- `cargo test`: everything passes (terrain 4, game_files 7 plus the earlier suites).
- Differential fuzzing against the reference: 2,000,000 random tiles for the minimap colour and all 65,536
  class pairs for `transition` agree with the reference (the vector files already cover them exhaustively for the
  tested bit patterns); nothing differs. `GameFiles` is covered by its folder-building tests.
- Token similarity with the reference code (identifiers renamed, number tables removed): terrain 0.65, game_files
  0.58. Both are a few lines of arithmetic or lookups that the specification gives as formulas and steps, and the
  tables are data copied from the vector file by design.

# Final coverage

With batch 7 every module of the reference crate has a specification, vectors, tests and an independent
implementation: cipher, decompressor, file header and index, string tables, archives, palettes, PNG writer, WAV
decoder, sound banks, dialog sets, landscape textures, map files, sprite files and animation tables, terrain
tables, game folder.
