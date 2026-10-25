#!/usr/bin/env python3
"""Writes tools/lift/functions/<binary>.tsv from the Ghidra export (export/<binary>/functions.tsv):
only the function start addresses and whether each is a C++ catch funclet (no names, no sizes),
under a header with the binary's SHA-256 so the list is only used for exactly that build.
    python3 tools/make_function_lists.py extracted/s4/Exe
"""
import hashlib, os, sys
exe = sys.argv[1] if len(sys.argv) > 1 else "extracted/s4/Exe"
os.makedirs("tools/lift/functions", exist_ok=True)
for binary in sorted(os.listdir("export")):
    src = f"export/{binary}/functions.tsv"
    path = os.path.join(exe, binary)
    if not (os.path.isfile(src) and os.path.isfile(path)):
        continue
    digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    rows = []
    for line in open(src, encoding="utf-8", errors="replace"):
        f = line.rstrip("\n").split("\t")
        if len(f) < 3:
            continue
        rows.append(f"{f[0].lower()}\t{'Catch@' if f[2].startswith('Catch@') else 'F'}")
    with open(f"tools/lift/functions/{binary}.tsv", "w") as out:
        out.write(f"# sha256 {digest}\n# function start addresses of {binary}; the lifter uses them only for this exact build\n")
        out.write("\n".join(rows) + "\n")
    print(binary, len(rows))
