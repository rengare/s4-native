# s4-formats-clean

Readers and writers for the data formats of *The Settlers IV Gold*, written in a **clean-room process**:

| module | what it handles |
|---|---|
| `cipher`, `compress` | the key-stream cipher and the adaptive-Huffman + LZ77 decompressor |
| `format`, `texts`, `archive` | file headers, index files, string tables, `.lib` archives |
| `palette`, `png`, `sound` | palettes, a PNG writer, the WAV/IMA-ADPCM decoder, sound banks |
| `dialogs`, `landscape`, `sprites` | dialog sets, landscape texture files, sprite files and animation tables |
| `map`, `terrain`, `game_files` | map files, terrain tables, the game folder lookup |

## How it was made

- **Specification side** (an AI coding agent working with the project owner) wrote `SPEC.md`, the test vectors in
  `vectors/` (synthetic input files with the expected results), the harness in `tests/` and the stubs in
  `src/lib.rs`. This side had analysed the original program's files and code; the specification states
  behaviour, constants and format tables only: no code, no function names, no addresses.
- **Implementation side** (a different AI coding agent, in a sandbox that showed only the files of this crate)
  wrote the bodies in `src/lib.rs` from `SPEC.md` and `vectors/` alone, following the rules in `PROCESS.md` and
  logging each session in `PROCESS-LOG.md`.
- **Review**: after each batch the specification side checked the result (`REVIEW.md`): the tests, differential
  fuzzing against a reference implementation that the implementer never saw, and a token-similarity measure
  against that reference.

What is and is not independent: the *implementation* is independent of the original program's code and of the
reference implementation. The *specification and the vectors* are not independent of the original: they describe
its formats, the expected outputs come from a reference implementation checked against real game files, and the
terrain tables in `vectors/terrain.tsv` are constants taken from the game. No file of the game is included;
the vector inputs are synthetic. `vectors/PROVENANCE.md` has the details and file hashes.

**History:** this crate was developed in its own repository with separate `spec:` and `impl:` commits. When the
project was published the history was squashed; `PROCESS-LOG.md` and `REVIEW.md` are the surviving record of the
process, and the commit hashes mentioned in them refer to the pre-publication history.

```sh
cargo test --target "$(rustc -vV | sed -n 's/host: //p')"   # inside the main project, whose default target is i686 Windows
```

This is a statement of what was done, not legal advice; see `docs/LEGAL.md` of the main project.
