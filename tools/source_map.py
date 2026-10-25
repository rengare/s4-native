#!/usr/bin/env python3
"""Attribute decompiled functions to the game's original source files (or classes).

Usage: source_map.py <export/binary-dir> <binary> > source_map.tsv   (summary on stderr)

Evidence: a function referencing a string that names a source file (trace
messages like "GameHost.cpp: ...", assert paths, Ghidra string labels such as
s_Simplenet_cpp__...). MSVC links each object file as one contiguous block, so a
function lying between two functions of the same file is attributed to it too
("inferred"). Functions without a source-file string but in a class namespace
(`CSimpleNet::vf3`, from RTTI) count as evidence for that class, so units are
source files where known and classes otherwise.
Output: address, function, unit, direct|inferred.
"""
import re
import sys
from collections import Counter
from functools import cache
from pathlib import Path

import pefile

# Ghidra string labels end in the string's address (s_Simplenet_cpp__Found_lo_00664f9c);
# the text in the label is truncated, so the full string is read from the binary.
LABEL = re.compile(r"\bs_\w*?_([0-9a-f]{8})\b")
SOURCE = re.compile(r"([A-Za-z][A-Za-z0-9_]*)\.(cpp|h)\b", re.I)


def normalise(name: str) -> str:
    """GameHost / Gamehost / CGameHost -> one key; keeps the first spelling seen."""
    return name.lower().removeprefix("c") if name[:2].lower() == "cg" else name.lower()


def main() -> None:
    d = Path(sys.argv[1])
    pe = pefile.PE(sys.argv[2])
    base, img = pe.OPTIONAL_HEADER.ImageBase, pe.get_memory_mapped_image()

    @cache
    def string_at(addr: str) -> str:
        off = int(addr, 16) - base
        if not 0 <= off < len(img):
            return ""
        end = img.find(b"\0", off, off + 512)
        return img[off:end].decode("latin-1") if end > off else ""

    text = (d / "decomp.c").read_text(errors="replace")
    funcs = []  # (addr, name, sources)
    spelling: dict[str, str] = {}
    for block in text.split("\n// ==== ")[1:]:
        head, _, body = block.partition("\n")
        addr, _, name = head.partition(" ")
        srcs = set()
        for addr_hex in LABEL.findall(body):
            for m in SOURCE.finditer(string_at(addr_hex)):
                key = normalise(m.group(1))
                spelling.setdefault(key, f"{m.group(1)}.{m.group(2)}")
                srcs.add(key)
        if not srcs and "::" in name:
            cls = name.rsplit("::", 1)[0]
            # Templates are emitted into many object files: no evidence of placement.
            if not cls.startswith(("std", "_", "$")) and "?$" not in cls:
                key = "class:" + cls.lower()
                spelling.setdefault(key, cls)
                srcs.add(key)
        funcs.append((int(addr, 16), name, srcs))
    funcs.sort()

    # Direct: exactly one source named; headers only count when no .cpp is named.
    direct = {}
    for i, (_, _, srcs) in enumerate(funcs):
        cpp = {s for s in srcs if spelling[s].endswith(".cpp") or s.startswith("class:")}
        pick = cpp or srcs
        if len(pick) == 1:
            direct[i] = next(iter(pick))

    known = sorted(direct)
    owner = dict(direct)
    for a, b in zip(known, known[1:]):
        if direct[a] == direct[b]:
            for i in range(a + 1, b):
                owner.setdefault(i, direct[a])

    out = Counter()
    for i, (addr, name, _) in enumerate(funcs):
        if i in owner:
            kind = "direct" if i in direct else "inferred"
            out[(spelling[owner[i]], kind)] += 1
            print(f"{addr:08x}\t{name}\t{spelling[owner[i]]}\t{kind}")

    files = sorted({f for f, _ in out}, key=lambda f: -(out[(f, "direct")] + out[(f, "inferred")]))
    n_src = sum(1 for f in files if "." in f)
    print(f"{len(owner)}/{len(funcs)} functions attributed to {n_src} source files and "
          f"{len(files) - n_src} classes; top 40:", file=sys.stderr)
    for f in files[:40]:
        print(f"  {f:28} direct {out[(f, 'direct')]:4}  inferred {out[(f, 'inferred')]:4}", file=sys.stderr)


if __name__ == "__main__":
    main()
