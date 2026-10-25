# Process (read before writing any code)

This repository is the **implementation side** of a clean-room process. The point of the process is that the
code written here is independent of any original program's code. These rules are the process.

## You may use

- The files in this repository: `SPEC.md`, `vectors/`, `tests/`, `src/lib.rs`, this file.
- General knowledge and the Rust standard library documentation.
- General algorithms in their textbook form (LFSRs, Huffman coding, LZ77).

## You must not use

- Any decompiled, disassembled or otherwise recovered code of the original program or of its libraries, or
  any text derived from it (function names, addresses, pseudo-code).
- Any other implementation of these two primitives *for this game's data*: public reimplementation or
  modding projects of the game, their repositories, forks or snippets of them. Do not search for them.
- The original program or its data files. You are not given them and must not look for them.
- Network access, except to read Rust documentation. Do not add dependencies; none are needed.

If you recognise a piece of the specification as something you have seen elsewhere in connection with this
game, say so in `QUESTIONS.md` and do not use that memory.

## When something is unclear or a vector fails

Do not guess from outside sources. Write the question (and the failing vector name) in `QUESTIONS.md` and stop
on that point. The specification side answers by correcting `SPEC.md` or adding vectors; commits that do so
start with `spec:`. Your commits start with `impl:`.

## Log

Keep `PROCESS-LOG.md`: for each working session the date, which files of this repository you read, and one
line confirming that you used nothing from the "must not use" list (or what you did instead). This log is the
evidence that the process was followed.

## Done when

`cargo test` passes, with no unsafe code and no `unimplemented!` left.
