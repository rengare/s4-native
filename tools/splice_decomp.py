#!/usr/bin/env python3
"""Replace "// ==== <addr> <name>" blocks of an export's decomp.c with retried ones.

Usage: splice_decomp.py <export/binary/decomp.c> <retry.c>
       splice_decomp.py --failed <export/binary/decomp.c>   (print failed addresses)
"""
import sys


def blocks(text: str) -> dict[str, str]:
    out = {}
    for b in ("\n" + text).split("\n// ==== ")[1:]:
        out[b.split(" ", 1)[0]] = "// ==== " + b.rstrip("\n") + "\n"
    return out


def main() -> None:
    if sys.argv[1] == "--failed":
        for addr, b in blocks(open(sys.argv[2]).read()).items():
            if "// decompile failed" in b:
                print(addr)
        return
    path, retry = sys.argv[1], sys.argv[2]
    text = open(path).read()
    new = {a: b for a, b in blocks(open(retry).read()).items() if "// decompile failed" not in b}
    head, *rest = text.split("\n// ==== ")
    parts = [head + "\n"]
    for b in rest:
        addr = b.split(" ", 1)[0]
        parts.append(new.get(addr, "// ==== " + b.rstrip("\n") + "\n"))
    open(path, "w").write("\n".join(p.rstrip("\n") + "\n" for p in parts))
    print(f"spliced {len(new)} function(s) into {path}")


if __name__ == "__main__":
    main()
