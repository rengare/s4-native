//! Runs lifted original modules natively. The guest sees a flat 32-bit
//! address space ([`World::new`] reserves 4 GiB lazily), the PE images at
//! their preferred bases, and the functions they import answered by Rust
//! implementations ([`Shim`]) at magic addresses.

pub mod audio;
pub mod avi;
pub mod cdaudio;
pub mod dinput;
pub mod dplay;
pub mod dsound;
pub mod compat;
pub mod gil;
pub mod ddraw;
pub mod fs;
pub mod gdi;
pub mod handles;
pub mod heap;
pub mod kernel32;
pub mod cursor;
pub mod miles;
pub mod pe;
pub mod present;
pub mod registry;
pub mod seh;
pub mod shims;
pub mod system;
pub mod text;
pub mod user32;
pub mod winsock;

use s4_x86::{Cpu, Dispatch, Mem};
use std::collections::HashMap;

pub use pe::Module;

/// A host-provided function the guest can call (an import or a callback).
#[derive(Clone, Copy)]
pub struct Shim {
    pub dll: &'static str,
    pub name: &'static str,
    /// Stack dwords of arguments.
    pub args: u8,
    /// The caller pops the arguments (cdecl); else the callee does (stdcall).
    pub cdecl: bool,
    pub f: fn(&mut Cpu, &[u32]) -> u32,
}

/// Guest addresses of shims: `MAGIC + 16 * index`.
pub const MAGIC: u32 = 0xfe00_0000;
const MAGIC_END: u32 = 0xff00_0000;

struct ModInfo {
    name: String,
    base: u32,
    exports: Vec<(String, u32)>,
}

static MODULES: std::sync::Mutex<Vec<ModInfo>> = std::sync::Mutex::new(Vec::new());
static FAKE_MODULES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static LOOKUPS: std::sync::Mutex<Option<HashMap<(String, String), u32>>> = std::sync::Mutex::new(None);

fn norm(name: &str) -> String {
    let n = name.rsplit(['\\', '/']).next().unwrap_or(name).to_lowercase();
    n.trim_end_matches(".dll").trim_end_matches(".exe").to_string()
}

/// Base address of a loaded module by name (0 if none).
pub fn module_base(name: &str) -> u32 {
    let n = norm(name);
    MODULES.lock().unwrap().iter().find(|m| m.name == n).map(|m| m.base).unwrap_or(0)
}

static MAIN: std::sync::Mutex<Option<(u32, String)>> = std::sync::Mutex::new(None);

/// The program's own module: its base (what `GetModuleHandle(NULL)` returns) and its guest path.
pub fn main_module() -> (u32, String) {
    MAIN.lock().unwrap().clone().unwrap_or_else(|| (module_base("s4_main"), "C:\\S4\\Exe\\S4_Main.exe".to_string()))
}

pub fn module_name(base: u32) -> String {
    MODULES.lock().unwrap().iter().find(|m| m.base == base).map(|m| m.name.clone()).unwrap_or_default()
}

/// A handle for a system DLL that is implemented by shims (`LoadLibrary("ddraw")`).
pub fn fake_module(name: &str) -> u32 {
    let n = norm(name);
    let mut f = FAKE_MODULES.lock().unwrap();
    let i = f.iter().position(|x| *x == n).unwrap_or_else(|| {
        f.push(n);
        f.len() - 1
    });
    0x7f00_0000 + 0x1_0000 * i as u32
}

/// `GetProcAddress`.
pub fn proc_address(module: u32, name: &str) -> u32 {
    if let Some(m) = MODULES.lock().unwrap().iter().find(|m| m.base == module) {
        return m.exports.iter().find(|(n, _)| n == name).map(|e| e.1).unwrap_or(0);
    }
    let dll = {
        let f = FAKE_MODULES.lock().unwrap();
        let i = (module.wrapping_sub(0x7f00_0000) / 0x1_0000) as usize;
        match f.get(i) {
            Some(d) => d.clone(),
            None => return 0,
        }
    };
    lookup_shim(&format!("{dll}.dll"), name).unwrap_or(0)
}

/// The guest address of the shim for `dll!name`, registering it on first use.
pub fn lookup_shim(dll: &str, name: &str) -> Option<u32> {
    // The GOG build of The Settlers III imports winmm under the name "_INMM.dll".
    let dll = if dll.eq_ignore_ascii_case("_INMM.dll") {
        "winmm.dll"
    } else if dll.eq_ignore_ascii_case("wsock32.dll") {
        "ws2_32.dll"
    } else {
        dll
    };
    let key = (dll.to_lowercase(), name.to_string());
    if let Some(a) = LOOKUPS.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return Some(*a);
    }
    let s = shims::all().into_iter().find(|s| s.dll.eq_ignore_ascii_case(&key.0) && s.name == name)?;
    let a = register_shim(s);
    LOOKUPS.lock().unwrap().as_mut().unwrap().insert(key, a);
    Some(a)
}

pub struct World {
    pub cpu: Cpu,
    pub modules: Vec<Module>,
    /// Imports nobody implemented, by magic index.
    missing: HashMap<u32, String>,
}

static SHIMS: std::sync::RwLock<Vec<Shim>> = std::sync::RwLock::new(Vec::new());

/// Registers a host function and returns its guest address.
pub fn register_shim(shim: Shim) -> u32 {
    let mut s = SHIMS.write().unwrap();
    s.push(shim);
    MAGIC + (s.len() as u32 - 1) * 16
}

/// What every guest thread shares: the address space and the lifted code.
pub struct Process {
    pub mem: Mem,
    pub dispatchers: Vec<Dispatch>,
}

pub static PROCESS: std::sync::OnceLock<Process> = std::sync::OnceLock::new();

/// A fresh processor for a guest thread with its stack at `stack_top`.
pub fn new_cpu(stack_top: u32) -> Cpu {
    let p = PROCESS.get().expect("no process");
    let mut cpu = Cpu::new(p.mem, stack_top);
    cpu.dispatchers = p.dispatchers.clone();
    cpu.fallback = shim_entry;
    // S4_FPU_CW=<hex>: initial x87 control word (Windows starts threads with 0x27f).
    if let Some(v) = std::env::var("S4_FPU_CW").ok().and_then(|v| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok()) {
        cpu.fpu.load_cw(v);
    }
    cpu
}

#[cfg(unix)]
fn reserve() -> usize {
    let p = unsafe { libc::mmap(core::ptr::null_mut(), 1 << 32, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_NORESERVE, -1, 0) };
    assert!(p != libc::MAP_FAILED, "cannot reserve the 4 GiB guest address space");
    p as usize
}

#[cfg(not(unix))]
fn reserve() -> usize {
    panic!("the native host needs a Unix-like system; under Wine the original's own address space is used")
}

const STACK_TOP: u32 = 0x7ff0_0000;

impl World {
    pub fn new(dispatchers: &[Dispatch]) -> World {
        let base = reserve();
        let mem = Mem { base };
        let _ = PROCESS.set(Process { mem, dispatchers: dispatchers.to_vec() });
        let mut cpu = new_cpu(STACK_TOP);
        cpu.fs_w32(0x18, 0); // TEB self pointer is set by the thread table
        let mut w = World { cpu, modules: Vec::new(), missing: HashMap::new() };
        heap::init();
        w
    }

    pub fn mem(&self) -> Mem {
        self.cpu.mem
    }

    /// Registers a host function and returns its guest address.
    pub fn add_shim(&mut self, shim: Shim) -> u32 {
        register_shim(shim)
    }

    /// Maps a module from `bytes` at its preferred base. Imports are bound by
    /// [`World::link`] once all modules are loaded.
    pub fn load(&mut self, name: &str, bytes: &[u8]) -> usize {
        let m = pe::map(self.cpu.mem, name, bytes);
        if name.to_lowercase().ends_with(".exe") && MAIN.lock().unwrap().is_none() {
            // The program lives in the game folder itself unless the game keeps its modules in an Exe folder (The Settlers IV).
            let dir = if fs::root() == fs::GUEST_ROOT { format!("{}\\Exe", fs::root()) } else { fs::root() };
            *MAIN.lock().unwrap() = Some((m.base, format!("{dir}\\{name}")));
        }
        shims::set_rsrc(m.base, m.rsrc);
        MODULES.lock().unwrap().push(ModInfo { name: norm(name), base: m.base, exports: m.exports.clone() });
        self.modules.push(m);
        self.modules.len() - 1
    }

    /// Binds every module's imports: exports of loaded modules, shims, or a
    /// placeholder that panics with the import's name when called.
    pub fn link(&mut self) {
        for mi in 0..self.modules.len() {
            for ii in 0..self.modules[mi].imports.len() {
                let (dll, name, slot) = {
                    let imp = &self.modules[mi].imports[ii];
                    (imp.dll.clone(), imp.name.clone(), imp.slot)
                };
                let target = self.resolve(&dll, &name);
                self.cpu.mem.w32(slot, target);
            }
        }
    }

    fn resolve(&mut self, dll: &str, name: &str) -> u32 {
        let dll_l = dll.to_lowercase();
        let name = &system::ordinal_name(&dll_l, name);
        // An export of a loaded module.
        if let Some(m) = MODULES.lock().unwrap().iter().find(|m| m.name == norm(&dll_l)) {
            if let Some((_, a)) = m.exports.iter().find(|(n, _)| n == name) {
                return *a;
            }
        }
        for &(d, n, init) in shims::data() {
            if d.eq_ignore_ascii_case(&dll_l) && n == name {
                let p = heap::alloc(&mut self.cpu, 16);
                self.cpu.mem.w32(p, init);
                // The import slot holds the variable's address.
                return p;
            }
        }
        if let Some(a) = lookup_shim(&dll_l, name) {
            return a;
        }
        let label = format!("{dll}!{name}");
        let key = (dll_l.clone(), name.to_string());
        if let Some(a) = LOOKUPS.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
            return *a;
        }
        let leaked: &'static str = Box::leak(label.clone().into_boxed_str());
        let addr = register_shim(Shim { dll: "", name: leaked, args: 0, cdecl: true, f: |_, _| 0 });
        LOOKUPS.lock().unwrap().as_mut().unwrap().insert(key, addr);
        self.missing.insert((addr - MAGIC) / 16, label);
        addr
    }

    /// Runs the module's entry point as `DllMain(base, DLL_PROCESS_ATTACH, 0)`
    /// (the C runtime start-up: static constructors).
    pub fn attach(&mut self, module: usize) -> u32 {
        let (base, entry) = (self.modules[module].base, self.modules[module].entry);
        self.call(entry, &[base, 1, 0], true)
    }

    pub fn export(&self, module: usize, name: &str) -> u32 {
        self.modules[module].exports.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name} is not exported")).1
    }

    /// Calls the guest function at `addr` with stack arguments (stdcall or
    /// cdecl; `ecx` as given); returns EAX.
    pub fn call(&mut self, addr: u32, args: &[u32], callee_pops: bool) -> u32 {
        let esp0 = self.cpu.esp;
        let m = self.cpu.mem;
        for &a in args.iter().rev() {
            self.cpu.esp -= 4;
            m.w32(self.cpu.esp, a);
        }
        self.cpu.esp -= 4;
        m.w32(self.cpu.esp, 0x7e57_0000);
        self.cpu.call(addr);
        if callee_pops {
            assert_eq!(self.cpu.esp, esp0, "stdcall callee must pop its arguments");
        } else {
            self.cpu.esp = esp0;
        }
        self.cpu.eax
    }

    /// Names of imports the loaded modules call but nobody implemented.
    pub fn missing(&self) -> Vec<&String> {
        let mut v: Vec<_> = self.missing.values().collect();
        v.sort();
        v
    }
}

/// `Cpu::fallback`: a call to a magic address runs the shim.
fn shim_entry(c: &mut Cpu, addr: u32) {
    if !(MAGIC..MAGIC_END).contains(&addr) {
        // A call into the middle of a lifted function: the exception dispatcher "calling" an __except handler,
        // which never returns but continues in the frame that installed it. Unwind to that frame.
        if (0x40_0000..0x7000_0000).contains(&addr) {
            if std::env::var_os("S4_JUMP_TRACE").is_some() {
                eprintln!("[{}] call into the middle of lifted code at {addr:#x}", std::thread::current().name().unwrap_or("?"));
            }
            std::panic::resume_unwind(Box::new(s4_x86::GuestJump { target: addr }));
        }
        let m = c.mem;
        panic!("call to {addr:#x}: nothing lifted or implemented there (return to {:#x}, arguments {:x?}, ecx {:#x} edx {:#x} eax {:#x})", m.r32(c.esp), (1..5).map(|i| m.r32(c.esp + 4 * i)).collect::<Vec<_>>(), c.ecx, c.edx, c.eax);
    }
    let idx = ((addr - MAGIC) / 16) as usize;
    let shim = SHIMS.read().unwrap()[idx];
    if shim.dll.is_empty() {
        panic!("call to the import {} which is not implemented", shim.name);
    }
    let m = c.mem;
    let args: Vec<u32> = (0..shim.args as u32).map(|i| m.r32(c.esp.wrapping_add(4 + 4 * i))).collect();
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let trace = *TRACE.get_or_init(|| std::env::var_os("S4_HOST_TRACE").is_some());
    ddraw::flush_pending_show(c);
    gil::release();
    let t0 = shim_stats::enabled().then(std::time::Instant::now);
    let r = (shim.f)(c, &args);
    if let Some(t0) = t0 {
        shim_stats::note(shim.name, t0.elapsed());
    }
    gil::acquire();
    if trace {
        eprintln!("t{} shim {}!{}({:x?}) -> {:#x}  [from {:#x}] esp={:#x}", kernel32::current_thread_id(), shim.dll, shim.name, args, r, m.r32(c.esp), c.esp);
    }
    c.eax = r;
    let ra = m.r32(c.esp);
    let pop = if shim.cdecl { 0 } else { 4 * shim.args as u32 };
    c.esp = c.esp.wrapping_add(4 + pop);
    let _ = ra;
}

/// Modules in the order Windows would initialise them (dependencies first);
/// `S4_Main` is last and is the program.
pub const GAME_MODULES: &[&str] = &[
    "MSVCRT.DLL", "MSVCIRT.DLL", "MSVCP60.DLL", "BB_Support.dll", "EventEngine.dll", "GfxEngine.dll", "GuiEngine2.dll", "SoundEngine.dll", "binkw32.dll", "VideoEngine.dll", "MapAccess.dll", "SaveGameAccess.dll", "MapGenerator.dll",
    "EconomyAI.dll", "S4_BBGCClient.dll", "RTComp.dll", "S4_Main.exe",
];

impl World {
    /// Loads the game's modules from `exe_dir` and links them.
    pub fn load_game(&mut self, exe_dir: &std::path::Path) {
        for name in GAME_MODULES {
            let path = exe_dir.join(name);
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            self.load(name, &bytes);
        }
        self.link();
    }

    /// Runs the DLL start-up code (`DllMain(DLL_PROCESS_ATTACH)`) of every module but the program.
    pub fn attach_all(&mut self) {
        for i in 0..self.modules.len() {
            if self.modules[i].name.to_lowercase().ends_with(".exe") {
                continue;
            }
            let r = self.attach(i);
            if std::env::var_os("S4_HOST_TRACE").is_some() {
                eprintln!("DllMain {} -> {}", self.modules[i].name, r & 0xff);
            }
        }
    }

    /// Runs the program's entry point (the C runtime start-up and WinMain).
    pub fn run_program(&mut self) -> u32 {
        let main = self.modules.iter().position(|m| m.name.to_lowercase().ends_with(".exe")).expect("no program");
        let entry = self.modules[main].entry;
        self.call(entry, &[], false)
    }
}

/// `S4_SHIM_STATS=1`: per thread and second, the calls into the host that took the most time (count, total ms).
mod shim_stats {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    pub fn enabled() -> bool {
        static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *E.get_or_init(|| std::env::var_os("S4_SHIM_STATS").is_some())
    }

    thread_local! {
        static STATS: RefCell<(Instant, HashMap<&'static str, (u32, Duration)>)> = RefCell::new((Instant::now(), HashMap::new()));
    }

    pub fn note(name: &'static str, d: Duration) {
        STATS.with(|s| {
            let mut s = s.borrow_mut();
            let e = s.1.entry(name).or_default();
            e.0 += 1;
            e.1 += d;
            if s.0.elapsed() >= Duration::from_secs(1) {
                let mut v: Vec<_> = s.1.drain().collect();
                v.sort_by_key(|(_, (_, d))| std::cmp::Reverse(*d));
                let total: Duration = v.iter().map(|(_, (_, d))| *d).sum();
                let top: Vec<String> = v.iter().take(14).map(|(n, (c, d))| format!("{n} {c}x {:.1}ms", d.as_secs_f64() * 1000.0)).collect();
                if total > Duration::from_millis(20) {
                    eprintln!("shim time [{}] {:.0}ms in host calls: {}", std::thread::current().name().unwrap_or("?"), total.as_secs_f64() * 1000.0, top.join(", "));
                }
                s.0 = Instant::now();
            }
        });
    }
}
