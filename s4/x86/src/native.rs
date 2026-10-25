//! Calls from lifted code into code that is not lifted, executed natively
//! (the original's functions and the system DLLs, while the original runs in
//! this process under Wine). The registers and the x87 are loaded from
//! [`Cpu`], the processor switches to the emulated stack (which should lie
//! inside the thread's real stack so that exception handling and stack
//! probes work), the target runs until it returns to a trampoline, and
//! everything is stored back.

use crate::Cpu;
use core::arch::{asm, global_asm};
use s4_x87::Value;
use std::sync::Mutex;

#[repr(C)]
struct Regs {
    eax: u32,
    ecx: u32,
    edx: u32,
    ebx: u32,
    esp: u32,
    ebp: u32,
    esi: u32,
    edi: u32,
    real_esp: u32,
}

global_asm!(
    ".globl _s4_bridge_enter",
    ".globl _s4_bridge_tramp",
    "_s4_bridge_enter:",
    "push ebx",
    "push esi",
    "push edi",
    "push ebp",
    "mov eax, [esp + 20]",
    "mov ecx, [esp + 24]",
    "mov [eax + 32], esp",
    "mov [_s4_bridge_target], ecx",
    "mov [_s4_bridge_regs], eax",
    "mov esp, [eax + 16]",
    "mov ebp, [eax + 20]",
    "mov esi, [eax + 24]",
    "mov edi, [eax + 28]",
    "mov ebx, [eax + 12]",
    "mov edx, [eax + 8]",
    "mov ecx, [eax + 4]",
    "mov eax, [eax]",
    "cld",
    "jmp dword ptr [_s4_bridge_target]",
    "_s4_bridge_tramp:",
    "cld",
    "mov [_s4_bridge_scratch], eax",
    "mov eax, [_s4_bridge_regs]",
    "mov [eax + 4], ecx",
    "mov [eax + 8], edx",
    "mov [eax + 12], ebx",
    "mov [eax + 16], esp",
    "mov [eax + 20], ebp",
    "mov [eax + 24], esi",
    "mov [eax + 28], edi",
    "mov ecx, [_s4_bridge_scratch]",
    "mov [eax], ecx",
    "mov esp, [eax + 32]",
    "pop ebp",
    "pop edi",
    "pop esi",
    "pop ebx",
    "ret",
    ".data",
    "_s4_bridge_target: .long 0",
    "_s4_bridge_regs: .long 0",
    "_s4_bridge_scratch: .long 0",
    ".text",
);

unsafe extern "C" {
    fn s4_bridge_enter(regs: *mut Regs, target: u32);
    fn s4_bridge_tramp();
}

static LOCK: Mutex<()> = Mutex::new(());

/// The 108-byte image `fnsave`/`frstor` use (32-bit protected mode).
fn fpu_image(c: &Cpu) -> [u8; 108] {
    let f = &c.fpu;
    let mut img = [0u8; 108];
    img[0..2].copy_from_slice(&f.cw.to_le_bytes());
    img[4..6].copy_from_slice(&f.status().to_le_bytes());
    img[8..10].copy_from_slice(&f.tag_word().to_le_bytes());
    for i in 0..8 {
        let p = (f.top as usize + i) & 7;
        img[28 + 10 * i..38 + 10 * i].copy_from_slice(&f.reg_bytes(p));
    }
    img
}

fn load_fpu_image(c: &mut Cpu, img: &[u8; 108]) {
    let f = &mut c.fpu;
    f.cw = u16::from_le_bytes([img[0], img[1]]);
    f.set_status(u16::from_le_bytes([img[4], img[5]]));
    let tags = u16::from_le_bytes([img[8], img[9]]);
    f.empty = 0;
    for i in 0..8 {
        let p = (f.top as usize + i) & 7;
        f.set_reg_bytes(p, img[28 + 10 * i..38 + 10 * i].try_into().unwrap());
    }
    for p in 0..8 {
        if tags >> (2 * p) & 3 == 3 {
            f.empty |= 1 << p;
        }
    }
    let _ = Value::ZERO;
}

/// Runs the code at `target` natively with `c`'s registers, stack and x87;
/// the return address must be on top of the emulated stack (the lifted
/// `call` pushed it). On return `c` holds the registers and stack pointer
/// the callee left, as after any lifted call.
pub fn call_native(c: &mut Cpu, target: u32) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let m = c.mem;
    let ret_slot = c.esp;
    let ret_addr = m.r32(ret_slot);
    m.w32(ret_slot, s4_bridge_tramp as *const () as usize as u32);
    let mut regs = Regs { eax: c.eax, ecx: c.ecx, edx: c.edx, ebx: c.ebx, esp: c.esp, ebp: c.ebp, esi: c.esi, edi: c.edi, real_esp: 0 };
    let guest = fpu_image(c);
    let mut host = [0u8; 108];
    let mut after = [0u8; 108];
    unsafe {
        asm!("fnsave [{}]", in(reg) host.as_mut_ptr(), options(nostack));
        asm!("frstor [{}]", in(reg) guest.as_ptr(), options(nostack));
        s4_bridge_enter(&mut regs, target);
        asm!("fnsave [{}]", in(reg) after.as_mut_ptr(), options(nostack));
        asm!("frstor [{}]", in(reg) host.as_ptr(), options(nostack));
    }
    m.w32(ret_slot, ret_addr);
    c.eax = regs.eax;
    c.ecx = regs.ecx;
    c.edx = regs.edx;
    c.ebx = regs.ebx;
    c.esp = regs.esp;
    c.ebp = regs.ebp;
    c.esi = regs.esi;
    c.edi = regs.edi;
    load_fpu_image(c, &after);
}

/// The [`Cpu::fallback`] for tests: everything nobody lifted runs natively.
pub fn bridge(c: &mut Cpu, target: u32) {
    call_native(c, target);
}

/// A [`Cpu`] with its own stack for calling lifted code from tests: memory
/// is this process's (the original is loaded in it), what is not lifted runs
/// natively.
pub struct Machine {
    pub cpu: Cpu,
    stack: Vec<u8>,
}

/// The "return address" of calls made by the test.
const TEST_RETURN: u32 = 0x7e57_0000;

impl Machine {
    pub fn new(dispatchers: &[crate::Dispatch]) -> Machine {
        // The emulated stack lies inside the thread's real stack (below the
        // current frames) so that the system's exception handling accepts
        // frames on it: needs a large stack (see .cargo/config.toml).
        let here: usize;
        unsafe { asm!("mov {}, esp", out(reg) here, options(nomem, nostack)) };
        let top = ((here - (4 << 20)) & !15) as u32;
        let stack = Vec::new();
        let mut cpu = Cpu::new(crate::Mem::IDENTITY, top);
        cpu.dispatchers = dispatchers.to_vec();
        cpu.fallback = bridge;
        Machine { cpu, stack }
    }

    /// Calls `f` with `args` pushed (cdecl: the caller pops them); returns EAX.
    pub fn call_cdecl(&mut self, f: fn(&mut Cpu), args: &[u32]) -> u32 {
        let esp0 = self.cpu.esp;
        self.push_call(args);
        f(&mut self.cpu);
        assert_eq!(self.cpu.esp, esp0 - 4 * args.len() as u32, "cdecl callee must leave the arguments");
        self.cpu.esp = esp0;
        self.cpu.eax
    }

    /// Calls `f` with `args` pushed (stdcall: the callee pops them).
    pub fn call_stdcall(&mut self, f: fn(&mut Cpu), args: &[u32]) -> u32 {
        let esp0 = self.cpu.esp;
        self.push_call(args);
        f(&mut self.cpu);
        assert_eq!(self.cpu.esp, esp0, "stdcall callee must pop the arguments");
        self.cpu.eax
    }

    /// Calls the code at `addr` (lifted or native) with `args` pushed; the
    /// callee pops them (`pop`, stdcall) or the caller does (cdecl).
    pub fn call_at(&mut self, addr: u32, args: &[u32], pop: bool) -> u32 {
        let esp0 = self.cpu.esp;
        self.push_call(args);
        self.cpu.call(addr);
        if pop {
            assert_eq!(self.cpu.esp, esp0, "stdcall callee must pop the arguments");
        } else {
            self.cpu.esp = esp0;
        }
        self.cpu.eax
    }

    /// Calls with ECX = `this` (thiscall, callee pops).
    pub fn call_thiscall(&mut self, f: fn(&mut Cpu), this: u32, args: &[u32]) -> u32 {
        self.cpu.ecx = this;
        self.call_stdcall(f, args)
    }

    fn push_call(&mut self, args: &[u32]) {
        let m = self.cpu.mem;
        for &a in args.iter().rev() {
            self.cpu.esp -= 4;
            m.w32(self.cpu.esp, a);
        }
        self.cpu.esp -= 4;
        m.w32(self.cpu.esp, TEST_RETURN);
    }
}

// ---- Executing single instructions natively (conformance tests) ----

#[repr(C)]
struct Run {
    eax: u32,
    ecx: u32,
    edx: u32,
    ebx: u32,
    esp: u32,
    ebp: u32,
    esi: u32,
    edi: u32,
    flags: u32,
    code: u32,
    fpu: [u8; 108],
}

global_asm!(
    ".globl _s4_run",
    ".globl _s4_run_exit",
    "_s4_run:",
    "push ebx",
    "push esi",
    "push edi",
    "push ebp",
    "mov eax, [esp + 20]",
    "mov [_s4_run_host_esp], esp",
    "mov [_s4_run_ptr], eax",
    "mov ecx, [eax + 36]",
    "mov [_s4_run_code], ecx",
    "fnsave [_s4_run_host_fpu]",
    "frstor [eax + 40]",
    "push dword ptr [eax + 32]",
    "popfd",
    "mov ecx, [eax + 4]",
    "mov edx, [eax + 8]",
    "mov ebx, [eax + 12]",
    "mov ebp, [eax + 20]",
    "mov esi, [eax + 24]",
    "mov edi, [eax + 28]",
    "mov esp, [eax + 16]",
    "mov eax, [eax]",
    "jmp dword ptr [_s4_run_code]",
    "_s4_run_exit:",
    "mov [_s4_run_scratch], eax",
    "mov eax, [_s4_run_ptr]",
    "mov [eax + 4], ecx",
    "mov [eax + 8], edx",
    "mov [eax + 12], ebx",
    "mov [eax + 16], esp",
    "mov [eax + 20], ebp",
    "mov [eax + 24], esi",
    "mov [eax + 28], edi",
    "mov esp, [_s4_run_host_esp]",
    "pushfd",
    "pop dword ptr [eax + 32]",
    "cld",
    "mov ecx, [_s4_run_scratch]",
    "mov [eax], ecx",
    "fnsave [eax + 40]",
    "frstor [_s4_run_host_fpu]",
    "pop ebp",
    "pop edi",
    "pop esi",
    "pop ebx",
    "ret",
    ".data",
    "_s4_run_host_esp: .long 0",
    "_s4_run_ptr: .long 0",
    "_s4_run_code: .long 0",
    "_s4_run_scratch: .long 0",
    "_s4_run_host_fpu: .space 108",
    ".text",
);

unsafe extern "C" {
    fn s4_run(run: *mut Run);
    fn s4_run_exit();
}

unsafe extern "system" {
    fn VirtualAlloc(addr: *mut u8, size: usize, kind: u32, protect: u32) -> *mut u8;
}

/// Executable memory for the code of [`execute`].
fn code_buffer() -> *mut u8 {
    static BUF: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let p = BUF.load(std::sync::atomic::Ordering::Relaxed);
    if p != 0 {
        return p as *mut u8;
    }
    let p = unsafe { VirtualAlloc(core::ptr::null_mut(), 4096, 0x3000, 0x40) };
    assert!(!p.is_null());
    BUF.store(p as usize, std::sync::atomic::Ordering::Relaxed);
    p
}

/// Executes the machine code `code` (one or a few instructions that do not
/// branch) on the processor with the registers, flags and x87 state of `c`,
/// and stores the resulting state back into `c`. Memory is the process's own
/// (the caller makes sure the code only touches memory it set up).
pub fn execute(code: &[u8], c: &mut Cpu) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let buf = code_buffer();
    unsafe {
        core::ptr::copy_nonoverlapping(code.as_ptr(), buf, code.len());
        // jmp dword ptr [exit_ptr]
        let exit = s4_run_exit as *const () as usize as u32;
        let slot = buf.add(64) as *mut u32;
        slot.write_unaligned(exit);
        let tail = buf.add(code.len());
        tail.write(0xff);
        tail.add(1).write(0x25);
        (tail.add(2) as *mut u32).write_unaligned(slot as u32);
    }
    let mut run = Run { eax: c.eax, ecx: c.ecx, edx: c.edx, ebx: c.ebx, esp: c.esp, ebp: c.ebp, esi: c.esi, edi: c.edi, flags: c.fl.to_u32(), code: buf as u32, fpu: fpu_image(c) };
    assert!(code.len() < 64);
    unsafe { s4_run(&mut run) };
    c.eax = run.eax;
    c.ecx = run.ecx;
    c.edx = run.edx;
    c.ebx = run.ebx;
    c.esp = run.esp;
    c.ebp = run.ebp;
    c.esi = run.esi;
    c.edi = run.edi;
    c.fl = crate::Fl::from_u32(run.flags);
    load_fpu_image(c, &run.fpu);
}
