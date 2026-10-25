#!/usr/bin/env python3
"""Recover MSVC RTTI vftables from a 32-bit PE.

Usage: rtti_scan.py <binary> > vtables.tsv

MSVC layout: vftable[-1] -> RTTICompleteObjectLocator {sig=0, offset, cdOffset,
TypeDescriptor*, ClassHierarchyDescriptor*}; TypeDescriptor {vfptr, spare, ".?AVName@@"}.
Output, one row per vftable entry:  class  vftable_va  index  target_va
(class is the demangled name, e.g. `Lobby::CLMAddPlayerToGroup`; `offset` > 0 marks the
vftable of a secondary base and is appended as `@offset`).
"""
import re
import struct
import sys

import pefile


def demangle_type(raw: str) -> str:
    """'.?AVCLMAddPlayerToGroup@Lobby@@' -> 'Lobby::CLMAddPlayerToGroup'"""
    body = raw[4:].rstrip("@")
    return "::".join(reversed([p for p in body.split("@") if p]))


def main() -> None:
    pe = pefile.PE(sys.argv[1])
    base = pe.OPTIONAL_HEADER.ImageBase
    img = pe.get_memory_mapped_image()
    text = next(s for s in pe.sections if s.Name.startswith(b".text"))
    t_lo, t_hi = base + text.VirtualAddress, base + text.VirtualAddress + text.Misc_VirtualSize

    def u32(va: int) -> int:
        return struct.unpack_from("<I", img, va - base)[0]

    def mapped(va: int) -> bool:
        return 0 <= va - base < len(img) - 4

    # Type descriptors: ".?AV" / ".?AU" names, descriptor starts 8 bytes before.
    tds = {}
    for m in re.finditer(rb"\.\?A[VU][\w@?$]+?@@\x00", img):
        tds[base + m.start() - 8] = demangle_type(m.group()[:-1].decode())

    # Complete object locators: 20-byte {0, off, cd, td, chd} with a known td.
    cols = {}
    for va in range(base, base + len(img) - 20, 4):
        sig, off, cd, td, chd = struct.unpack_from("<5I", img, va - base)
        if sig == 0 and td in tds and mapped(chd) and off < 0x10000:
            cols[va] = (tds[td], off)

    # vftables: a dword pointing to a COL, followed by pointers into .text.
    col_refs = {}
    for va in range(base, base + len(img) - 4, 4):
        v = u32(va)
        if v in cols:
            col_refs.setdefault(va, v)

    out = sys.stdout
    seen = set()
    for ref, col in sorted(col_refs.items()):
        name, off = cols[col]
        vt = ref + 4
        if not (t_lo <= u32(vt) < t_hi) or vt in seen:
            continue
        seen.add(vt)
        cls = name if off == 0 else f"{name}@{off:#x}"
        i = 0
        while mapped(vt + 4 * i) and t_lo <= u32(vt + 4 * i) < t_hi:
            # The next vftable's COL pointer ends this one.
            if i and (vt + 4 * i) in col_refs:
                break
            out.write(f"{cls}\t{vt:#010x}\t{i}\t{u32(vt + 4 * i):#010x}\n")
            i += 1


if __name__ == "__main__":
    main()
