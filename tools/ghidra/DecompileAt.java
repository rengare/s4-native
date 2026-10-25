// Decompiles single functions with a long timeout (for ones ExportDecomp timed out on).
// Usage (headless): -postScript DecompileAt.java <outfile> <timeout-seconds> <hex-address>...
// Writes the same "// ==== <addr> <name>" blocks as ExportDecomp.
//@category Export

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;

import java.io.PrintWriter;

public class DecompileAt extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        int timeout = Integer.parseInt(args[1]);
        DecompInterface ifc = new DecompInterface();
        ifc.openProgram(currentProgram);
        try (PrintWriter c = new PrintWriter(args[0])) {
            for (int i = 2; i < args.length; i++) {
                Function fn = getFunctionAt(toAddr(args[i]));
                if (fn == null) {
                    println("no function at " + args[i]);
                    continue;
                }
                DecompileResults r = ifc.decompileFunction(fn, timeout, monitor);
                c.printf("// ==== %s %s%n", fn.getEntryPoint(), fn.getName(true));
                c.println(r.decompileCompleted() ? r.getDecompiledFunction().getC()
                        : "// decompile failed: " + r.getErrorMessage());
                println(fn.getEntryPoint() + " completed=" + r.decompileCompleted());
            }
        }
        ifc.dispose();
    }
}
