#!/usr/bin/env python3
"""List the branches of every function of an original binary, for the
branch coverage the differential tests measure (s4/difftest).

    tools/branches.py <binary>   ->  export/<binary name>/branches.tsv

One line per instruction that decides where execution goes:
`function  address  length  kind  target`, kind `jcc` (conditional, two
ways: target and address + length), `jmptab` (indirect jump, e.g. a
switch table; target 0) or `entry` (one per function, so functions without
branches count too). Addresses are as in Ghidra (image base included).
"""

import bisect
import os
import re
import subprocess
import sys

LINE = re.compile(r"^\s*([0-9a-f]+):\s+((?:[0-9a-f]{2} )+)\s*(\S+)\s*(.*)$")
# objdump keeps raw bytes on continuation lines for long instructions.
CONT = re.compile(r"^\s*([0-9a-f]+):\s+((?:[0-9a-f]{2} )+)\s*$")


def main():
    binary = sys.argv[1]
    name = os.path.basename(binary)
    out_dir = os.path.join(os.path.dirname(__file__), "..", "export", name)
    funcs = []
    for line in open(os.path.join(out_dir, "functions.tsv")):
        f = line.split("\t")
        funcs.append((int(f[0], 16), int(f[1])))
    funcs.sort()
    starts = [a for a, _ in funcs]

    def owner(addr):
        i = bisect.bisect_right(starts, addr) - 1
        if i >= 0 and addr < funcs[i][0] + funcs[i][1]:
            return funcs[i][0]
        return None

    dump = subprocess.run(["i686-w64-mingw32-objdump", "-d", "-M", "intel", "-j", ".text", binary],
                          capture_output=True, text=True, check=True).stdout
    rows = [(a, a, 0, "entry", a) for a, _ in funcs]
    insn = None  # (address, length, mnemonic, operands)

    def flush():
        if not insn:
            return
        addr, length, mnem, ops = insn
        f = owner(addr)
        if f is None:
            return
        if mnem.startswith("j") and mnem != "jmp":
            target = int(ops.split()[0], 16)
            rows.append((f, addr, length, "jcc", target))
        elif mnem in ("loop", "loope", "loopne", "jecxz", "jcxz"):
            rows.append((f, addr, length, "jcc", int(ops.split()[0], 16)))
        elif mnem == "jmp" and ("[" in ops or not re.match(r"^(0x)?[0-9a-f]+\b", ops)):
            rows.append((f, addr, length, "jmptab", 0))

    for line in dump.splitlines():
        m = LINE.match(line)
        if m:
            flush()
            insn = (int(m.group(1), 16), len(m.group(2).split()), m.group(3), m.group(4))
            continue
        c = CONT.match(line)
        if c and insn:
            insn = (insn[0], insn[1] + len(c.group(2).split()), insn[2], insn[3])
    flush()
    rows.sort()
    with open(os.path.join(out_dir, "branches.tsv"), "w") as o:
        for f, a, n, k, t in rows:
            o.write(f"{f:08x}\t{a:08x}\t{n}\t{k}\t{t:08x}\n")
    kinds = {}
    for r in rows:
        kinds[r[3]] = kinds.get(r[3], 0) + 1
    print(name, kinds)


if __name__ == "__main__":
    main()
