//! Runtime of the lifted original code (`tools/lift` output): the state of
//! the processor (registers, the x87, MMX), memory as the original sees it
//! (a flat 4 GiB address space), and the bridge that lets lifted code call
//! code that is not lifted (the original's own functions and the system DLLs
//! while the original runs under Wine).

pub mod alu;
pub mod fpu;
pub mod mem;
pub mod mmx;
pub mod string;

pub use alu::Fl;
pub use fpu::Fpu;
pub use mem::Mem;

#[cfg(all(windows, target_arch = "x86"))]
pub mod native;

/// A jump into the middle of a function that is further up the (Rust) call stack: a C++ catch
/// continuation or `longjmp`. It travels as a panic payload to the lifted function that owns `target`.
#[derive(Debug)]
pub struct GuestJump {
    pub target: u32,
}

/// Where lifted code tries to call an address nobody lifted: the dispatchers
/// of the loaded modules answer first, then the fallback.
pub type Dispatch = fn(&mut Cpu, u32) -> bool;

/// Processor state shared by all lifted functions; the arguments and the
/// return address live in the emulated stack in memory, as in the original.
#[derive(Clone)]
pub struct Cpu {
    pub mem: Mem,
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esp: u32,
    pub ebp: u32,
    pub esi: u32,
    pub edi: u32,
    pub fl: Fl,
    pub fpu: Fpu,
    /// MMX registers (they alias the x87 registers on the hardware; the
    /// `emms` and TOP/tag side effects are applied to `fpu`).
    pub mm: [u64; 8],
    /// The thread information block `fs:` addresses (offset 0 is the SEH
    /// chain head).
    pub fs: Vec<u8>,
    /// Calls to addresses without lifted code: dispatchers of loaded lifted
    /// modules, tried in order.
    pub dispatchers: Vec<Dispatch>,
    /// Last resort for calls nobody lifted (imports; the native bridge).
    pub fallback: fn(&mut Cpu, u32),
    /// The time-stamp counter `rdtsc` reads (advances deterministically).
    pub tsc: u64,
}

/// CPUID leaf 1 EDX: FPU, TSC, CMOV, MMX. The host may clear bits (e.g. to make a program take its non-MMX paths).
pub static FEATURES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1 | 1 << 4 | 1 << 15 | 1 << 23);

impl Cpu {
    pub fn new(mem: Mem, esp: u32) -> Cpu {
        Cpu {
            mem,
            eax: 0,
            ecx: 0,
            edx: 0,
            ebx: 0,
            esp,
            ebp: 0,
            esi: 0,
            edi: 0,
            fl: Fl::default(),
            fpu: Fpu::new(),
            mm: [0; 8],
            fs: vec![0; 0x1000],
            dispatchers: Vec::new(),
            fallback: |_, addr| panic!("call to {addr:#x}: nothing lifted or bridged there"),
            tsc: 0,
        }
    }

    /// Calls the code at `addr` (the return address is on the stack):
    /// lifted code if any module has it, else the fallback.
    pub fn call(&mut self, addr: u32) {
        for i in 0..self.dispatchers.len() {
            let d = self.dispatchers[i];
            if d(self, addr) {
                return;
            }
        }
        (self.fallback)(self, addr);
    }

    /// `rdtsc`: a counter that advances by a fixed step per read.
    pub fn rdtsc(&mut self) -> u64 {
        self.tsc += 2_000_000;
        self.tsc
    }

    /// `cpuid` of a fixed profile: a Pentium III class "GenuineIntel" with
    /// x87, time-stamp counter, CMOV and MMX (no SSE).
    pub fn cpuid(&self, leaf: u32, _sub: u32) -> (u32, u32, u32, u32) {
        match leaf {
            0 => (1, u32::from_le_bytes(*b"Genu"), u32::from_le_bytes(*b"ntel"), u32::from_le_bytes(*b"ineI")),
            1 => (0x0000_0673, 0, 0, FEATURES.load(std::sync::atomic::Ordering::Relaxed)),
            _ => (0, 0, 0, 0),
        }
    }

    /// `jmp reg` to an address that is not a function entry of any module: the
    /// tail call of a thunk, or (for the lifted function that has `addr` as a
    /// block) a jump into its own body from deeper in the stack.
    pub fn tail_jump(&mut self, addr: u32) {
        for i in 0..self.dispatchers.len() {
            let d = self.dispatchers[i];
            if d(self, addr) {
                return;
            }
        }
        if (0xfe00_0000..0xff00_0000).contains(&addr) {
            (self.fallback)(self, addr);
            return;
        }
        if std::env::var_os("S4_JUMP_TRACE").is_some() {
            eprintln!("[{}] jump to {addr:#x}: {}", std::thread::current().name().unwrap_or("?"), std::backtrace::Backtrace::force_capture());
        }
        std::panic::resume_unwind(Box::new(GuestJump { target: addr }));
    }

    pub fn fs_r32(&self, off: u32) -> u32 {
        let o = off as usize;
        u32::from_le_bytes(self.fs[o..o + 4].try_into().unwrap())
    }

    pub fn fs_w32(&mut self, off: u32, v: u32) {
        let o = off as usize;
        self.fs[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
}
