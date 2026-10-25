// Finds code Ghidra's auto-analysis missed, in two passes, then re-analyses:
//  1. data pointers: every aligned dword in non-executable initialized memory that
//     points into an executable block, outside any function, at a plausible start
//     (after padding / a return) -> function (callbacks, handler tables, ...).
//  2. gaps: in bytes no function covers, a 16-byte aligned address right after
//     int3/nop padding that starts with a common MSVC prologue -> function.
// Run with write access (no -readOnly).
//@category Analysis

import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.address.AddressSetView;
import ghidra.program.model.listing.Function;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.mem.MemoryAccessException;
import ghidra.program.model.mem.MemoryBlock;

public class FindMoreCode extends GhidraScript {
    private Memory mem;
    private AddressSet exec;

    @Override
    protected void run() throws Exception {
        mem = currentProgram.getMemory();
        exec = new AddressSet();
        for (MemoryBlock b : mem.getBlocks()) if (b.isExecute()) exec.add(b.getStart(), b.getEnd());

        int fromData = 0;
        for (MemoryBlock b : mem.getBlocks()) {
            if (b.isExecute() || !b.isInitialized()) continue;
            for (Address a = b.getStart(); a.compareTo(b.getEnd().subtract(3)) <= 0; a = a.add(4)) {
                monitor.checkCancelled();
                Address t;
                try {
                    t = toAddr(mem.getInt(a) & 0xffffffffL);
                } catch (Exception e) {
                    continue;
                }
                if (!exec.contains(t) || getFunctionContaining(t) != null) continue;
                if (!afterBoundary(t)) continue;
                if (makeFunction(t)) fromData++;
            }
        }
        println("from data pointers: " + fromData);
        analyzeChanges(currentProgram);

        int fromGaps = 0;
        for (Address a = exec.getMinAddress(); a != null && a.compareTo(exec.getMaxAddress()) < 0; a = a.add(16)) {
            monitor.checkCancelled();
            if (!exec.contains(a) || getFunctionContaining(a) != null || getInstructionContaining(a) != null) continue;
            if (afterBoundary(a) && prologue(a) && makeFunction(a)) fromGaps++;
        }
        println("from gaps: " + fromGaps);
        analyzeChanges(currentProgram);
        println("functions=" + currentProgram.getFunctionManager().getFunctionCount());
    }

    /** Preceded by int3/nop padding or a ret, i.e. not in the middle of code. */
    private boolean afterBoundary(Address t) {
        try {
            int p = mem.getByte(t.subtract(1)) & 0xff;
            if (p == 0xcc || p == 0x90 || p == 0xc3) return true;
            // ret imm16
            return (mem.getByte(t.subtract(3)) & 0xff) == 0xc2;
        } catch (Exception e) {
            return false;
        }
    }

    private boolean prologue(Address a) throws MemoryAccessException {
        int b0 = mem.getByte(a) & 0xff, b1 = mem.getByte(a.add(1)) & 0xff, b2 = mem.getByte(a.add(2)) & 0xff;
        if (b0 == 0x55 && b1 == 0x8b && b2 == 0xec) return true;           // push ebp; mov ebp, esp
        if (b0 == 0x6a && b1 == 0xff && b2 == 0x68) return true;           // push -1; push handler (SEH)
        if (b0 == 0x83 && b1 == 0xec) return true;                         // sub esp, imm8
        if (b0 == 0x81 && b1 == 0xec) return true;                         // sub esp, imm32
        if (b0 == 0x64 && b1 == 0xa1 && b2 == 0x00) return true;           // mov eax, fs:[0]
        // push reg; push reg / push reg; mov ...
        boolean push0 = b0 >= 0x50 && b0 <= 0x57 && b0 != 0x54, push1 = b1 >= 0x50 && b1 <= 0x57 && b1 != 0x54;
        return push0 && (push1 || b1 == 0x8b);
    }

    private boolean makeFunction(Address t) {
        if (getInstructionAt(t) == null) {
            new DisassembleCommand(t, null, true).applyTo(currentProgram, monitor);
            if (getInstructionAt(t) == null) return false;
        }
        Function f = createFunction(t, null);
        return f != null;
    }
}
