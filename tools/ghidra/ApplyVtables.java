// Applies tools/rtti_scan.py output: labels each vftable <Class>::vftable, creates
// functions Ghidra never discovered (code reached only through vftables), and names
// unnamed ones <Class>::vf<index> when exactly one class uses them (shared ones get
// a comment). Then re-runs auto-analysis so code called from the new functions is
// found too. Run with write access (no -readOnly).
// Usage (headless): -postScript ApplyVtables.java <vtables.tsv>
//@category Analysis

import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Namespace;
import ghidra.program.model.symbol.SourceType;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public class ApplyVtables extends GhidraScript {
    @Override
    protected void run() throws Exception {
        Map<Address, List<String>> slots = new LinkedHashMap<>(); // target -> "Class::vfN"
        Map<Address, String> tables = new LinkedHashMap<>();       // vftable -> Class
        for (String line : Files.readAllLines(Path.of(getScriptArgs()[0]))) {
            String[] f = line.split("\t");
            Address vt = toAddr(Long.decode(f[1]));
            Address target = toAddr(Long.decode(f[3]));
            tables.putIfAbsent(vt, f[0]);
            slots.computeIfAbsent(target, k -> new ArrayList<>()).add(f[0] + "::vf" + f[2]);
        }

        for (var e : tables.entrySet()) {
            Namespace ns = namespaceFor(e.getValue());
            createLabel(e.getKey(), "vftable", ns, true, SourceType.ANALYSIS);
        }

        int created = 0, named = 0, shared = 0, failed = 0;
        for (var e : slots.entrySet()) {
            monitor.checkCancelled();
            Address a = e.getKey();
            Function fn = getFunctionAt(a);
            if (fn == null) {
                if (getInstructionAt(a) == null) {
                    new DisassembleCommand(a, null, true).applyTo(currentProgram, monitor);
                }
                fn = createFunction(a, null);
                if (fn == null) {
                    failed++;
                    continue;
                }
                created++;
            }
            if (fn.getSymbol().getSource() != SourceType.DEFAULT) continue; // keep real names
            List<String> uses = e.getValue();
            if (uses.size() == 1) {
                String full = uses.get(0);
                int sep = full.lastIndexOf("::vf");
                fn.setParentNamespace(namespaceFor(full.substring(0, sep)));
                fn.setName(full.substring(sep + 2), SourceType.ANALYSIS);
                named++;
            } else {
                fn.setComment("Virtual in: " + String.join(", ", uses));
                shared++;
            }
        }
        println("vftables=" + tables.size() + " created=" + created + " named=" + named
                + " shared=" + shared + " failed=" + failed);
        analyzeChanges(currentProgram);
        println("re-analysis done, functions=" + currentProgram.getFunctionManager().getFunctionCount());
    }

    /** "A::B@0x4" -> namespace A::B (secondary-base tables share the class namespace). */
    private Namespace namespaceFor(String cls) throws Exception {
        int at = cls.indexOf('@');
        if (at >= 0) cls = cls.substring(0, at);
        Namespace ns = currentProgram.getGlobalNamespace();
        for (String part : cls.split("::")) {
            part = part.replaceAll("[^A-Za-z0-9_$?<>]", "_");
            Namespace next = getNamespace(ns, part);
            ns = next != null ? next
                    : currentProgram.getSymbolTable().createClass(ns, part, SourceType.ANALYSIS);
        }
        return ns;
    }
}
