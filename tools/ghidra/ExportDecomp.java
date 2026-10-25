// Exports a program's functions, exports and decompiled C to <outdir>/<program>/.
// Usage (headless): -postScript ExportDecomp.java <outdir>
//   functions.tsv  address, size, name, signature, calling convention
//   exports.tsv    address, symbol name (incl. MSVC-mangled), demangled
//   decomp.c       decompiler output for every function, address-ordered
//@category Export

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.app.util.demangler.DemanglerUtil;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Symbol;

import java.io.File;
import java.io.PrintWriter;

public class ExportDecomp extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        File out = new File(args.length > 0 ? args[0] : "export", currentProgram.getName());
        out.mkdirs();

        try (PrintWriter w = new PrintWriter(new File(out, "exports.tsv"))) {
            for (Address a : currentProgram.getSymbolTable().getExternalEntryPointIterator()) {
                for (Symbol s : currentProgram.getSymbolTable().getSymbols(a)) {
                    String name = s.getName();
                    String dem = "";
                    try {
                        var d = DemanglerUtil.demangle(currentProgram, name, a);
                        if (d != null && !d.isEmpty()) dem = d.get(0).getSignature(false);
                    } catch (Exception e) { /* not mangled */ }
                    w.printf("%s\t%s\t%s%n", a, name, dem);
                }
            }
        }

        DecompInterface ifc = new DecompInterface();
        ifc.openProgram(currentProgram);
        int n = 0;
        try (PrintWriter f = new PrintWriter(new File(out, "functions.tsv"));
             PrintWriter c = new PrintWriter(new File(out, "decomp.c"))) {
            for (Function fn : currentProgram.getFunctionManager().getFunctions(true)) {
                monitor.checkCancelled();
                f.printf("%s\t%d\t%s\t%s\t%s%n", fn.getEntryPoint(), fn.getBody().getNumAddresses(),
                        fn.getName(true), fn.getPrototypeString(false, false), fn.getCallingConventionName());
                if (fn.isThunk() || fn.isExternal()) continue;
                DecompileResults r = ifc.decompileFunction(fn, 60, monitor);
                c.printf("// ==== %s %s%n", fn.getEntryPoint(), fn.getName(true));
                c.println(r.decompileCompleted() ? r.getDecompiledFunction().getC()
                        : "// decompile failed: " + r.getErrorMessage());
                if (++n % 500 == 0) println("decompiled " + n);
            }
        }
        ifc.dispose();
        println("exported " + n + " functions to " + out);
    }
}
