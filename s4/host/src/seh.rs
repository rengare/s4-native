//! Structured exception handling for the guest: `RaiseException` walks the
//! registration chain at `fs:[0]` and calls the handlers (MSVC's
//! `__CxxFrameHandler` among them, lifted from MSVCRT); `RtlUnwind` runs the
//! unwinding handlers. A handler that catches a C++ exception ends by jumping
//! into the middle of the catching function, which the lifted code turns into
//! a [`s4_x86::GuestJump`] unwinding the Rust frames in between.

use crate::{heap, shims::guest_call};
use s4_x86::Cpu;

const END_OF_CHAIN: u32 = 0xffff_ffff;
const EH_UNWINDING: u32 = 2;
const EH_EXIT_UNWIND: u32 = 4;

fn context(c: &mut Cpu) -> u32 {
    let ctx = heap::alloc_zeroed(c, 0xcc);
    c.mem.w32(ctx, 0x10007); // CONTEXT_FULL (i386)
    for (off, v) in [(0xb0, c.eax), (0xac, c.ecx), (0xa8, c.edx), (0xa4, c.ebx), (0xa0, c.esi), (0x9c, c.edi), (0xb4, c.ebp), (0xc4, c.esp), (0xb8, c.mem.r32(c.esp))] {
        c.mem.w32(ctx + off, v);
    }
    ctx
}

/// `RaiseException(code, flags, argc, args)`.
pub fn raise_exception(c: &mut Cpu, a: &[u32]) -> u32 {
    let rec = heap::alloc_zeroed(c, 80);
    let n = a[2].min(15);
    c.mem.w32(rec, a[0]);
    c.mem.w32(rec + 4, a[1]);
    c.mem.w32(rec + 12, c.mem.r32(c.esp));
    c.mem.w32(rec + 16, n);
    for i in 0..n {
        let v = c.mem.r32(a[3] + 4 * i);
        c.mem.w32(rec + 20 + 4 * i, v);
    }
    dispatch(c, rec)
}

pub fn dispatch(c: &mut Cpu, rec: u32) -> u32 {
    if std::env::var_os("S4_SEH_TRACE").is_some() {
        eprintln!("exception {:#x} flags {:#x} at {:#x}", c.mem.r32(rec), c.mem.r32(rec + 4), c.mem.r32(rec + 12));
        // Words on the stack that look like return addresses into the program's code.
        let calls: Vec<String> = (0..96).map(|i| c.mem.r32(c.esp + 4 * i)).filter(|v| (0x401000..0x655000).contains(v)).take(12).map(|v| format!("{v:#x}")).collect();
        eprintln!("  stack: {}", calls.join(" "));
    }
    let ctx = context(c);
    let dc = heap::alloc_zeroed(c, 16);
    let mut frame = c.fs_r32(0);
    while frame != END_OF_CHAIN && frame != 0 {
        let handler = c.mem.r32(frame + 4);
        let prev = c.mem.r32(frame);
        let disp = guest_call(c, handler, &[rec, frame, ctx, dc]);
        match disp {
            0 => return 0, // ExceptionContinueExecution
            1 => frame = prev,
            d => panic!("exception handler {handler:#x} returned disposition {d}"),
        }
    }
    let code = c.mem.r32(rec);
    panic!("unhandled exception {code:#x}");
}

/// `RtlUnwind(targetFrame, targetIp, record, returnValue)`.
pub fn rtl_unwind(c: &mut Cpu, a: &[u32]) -> u32 {
    let target = a[0];
    if std::env::var_os("S4_SEH_TRACE").is_some() && a[2] != 0 {
        eprintln!("RtlUnwind: target frame {:#x}, exception {:#x} flags {:#x} at {:#x}", a[0], c.mem.r32(a[2]), c.mem.r32(a[2] + 4), c.mem.r32(a[2] + 12));
    }
    let rec = if a[2] != 0 {
        a[2]
    } else {
        let r = heap::alloc_zeroed(c, 80);
        c.mem.w32(r, 0xc000_0027);
        r
    };
    let flags = c.mem.r32(rec + 4) | EH_UNWINDING | if target == 0 { EH_EXIT_UNWIND } else { 0 };
    c.mem.w32(rec + 4, flags);
    let ctx = context(c);
    let dc = heap::alloc_zeroed(c, 16);
    loop {
        let frame = c.fs_r32(0);
        if frame == target || frame == END_OF_CHAIN || frame == 0 {
            break;
        }
        let handler = c.mem.r32(frame + 4);
        let prev = c.mem.r32(frame);
        guest_call(c, handler, &[rec, frame, ctx, dc]);
        c.fs_w32(0, prev);
    }
    0
}
