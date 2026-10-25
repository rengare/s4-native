#!/usr/bin/env python3
"""Print a Markdown map of a game's binaries: exported classes and module dependencies.

Usage: module_map.py <export-dir> <binary-dir> [<binary-dir> ...] > map.md

<export-dir> is the ExportDecomp output (per-binary functions.tsv / exports.tsv);
binaries are read from the given directories to get their imports.
"""
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

import pefile

THIRD_PARTY = {"msvcrt.dll", "msvcp60.dll", "msvcirt.dll", "kernel32.dll", "user32.dll", "gdi32.dll",
               "advapi32.dll", "shell32.dll", "ole32.dll", "oleaut32.dll", "winmm.dll", "version.dll",
               "ddraw.dll", "dsound.dll", "dplayx.dll", "wsock32.dll", "ws2_32.dll", "dbghelp.dll",
               "binkw32.dll", "mss32.dll", "comctl32.dll", "comdlg32.dll", "rpcrt4.dll", "imm32.dll"}


def classes(exports_tsv: Path) -> Counter:
    """Count exported members per class, from the demangled signatures."""
    out: Counter = Counter()
    for line in exports_tsv.read_text().splitlines():
        parts = line.split("\t")
        dem = parts[2] if len(parts) > 2 else ""
        m = re.search(r"(\w+)::~?\w+\(", dem)
        if m:
            out[m.group(1)] += 1
        elif parts[1] and not parts[1].startswith("Ordinal_") and parts[1] != "entry":
            out["(free functions)"] += 1
    return out


def main() -> None:
    export_dir = Path(sys.argv[1])
    binaries = {p.name.lower(): p for d in sys.argv[2:] for p in Path(d).iterdir()
                if p.suffix.lower() in (".exe", ".dll")}
    rows = []
    deps: dict[str, list[str]] = defaultdict(list)
    for exp in sorted(export_dir.iterdir(), key=lambda p: p.name.lower()):
        ftsv = exp / "functions.tsv"
        if not ftsv.exists():
            continue
        funcs = ftsv.read_text().splitlines()
        named = sum(1 for f in funcs if not f.split("\t")[2].startswith("FUN_"))
        cls = classes(exp / "exports.tsv")
        rows.append((exp.name, len(funcs), named, cls))
        pe_path = binaries.get(exp.name.lower())
        if pe_path:
            pe = pefile.PE(str(pe_path), fast_load=True)
            pe.parse_data_directories([pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]])
            for d in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []):
                name = d.dll.decode()
                if name.lower() not in THIRD_PARTY:
                    deps[exp.name].append(f"{name} ({len(d.imports)})")

    print("| Binary | Functions | Named | Exported classes (members) |")
    print("|---|---:|---:|---|")
    for name, n, named, cls in rows:
        c = ", ".join(f"`{k}` ({v})" for k, v in cls.most_common(6)) or "—"
        print(f"| {name} | {n} | {named} | {c} |")
    print("\n### Game-internal dependencies (imported functions)\n")
    for name, d in sorted(deps.items(), key=lambda kv: kv[0].lower()):
        print(f"- **{name}** → {', '.join(d)}")


if __name__ == "__main__":
    main()
