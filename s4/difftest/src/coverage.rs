//! Branch coverage of original functions, measured while the differential
//! tests call them.
//!
//! The branch list comes from `export/<binary>/branches.tsv`
//! (`tools/branches.py`). Every function entry and every conditional jump of
//! the chosen functions gets an `int3`. A vectored exception handler sees
//! the breakpoint, puts the original byte back and single-steps the jump;
//! the single-step exception shows where it went (target or fall-through).
//! A jump keeps its breakpoint until both ways were seen. Indirect jumps
//! (switch tables) keep theirs and collect every target they reach.
//!
//! The same handler records writes ([`Writes`]): the module's writable pages
//! are made read-only, a write faults, the handler notes the address, opens
//! the page, single-steps the instruction and closes the page again. Unlike
//! comparing snapshots this also sees writes of the value already there.

use core::ffi::c_void;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::Original;

unsafe extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: extern "system" fn(*mut ExceptionPointers) -> i32) -> *mut c_void;
    fn VirtualProtect(addr: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
    fn FlushInstructionCache(process: *mut c_void, addr: *const c_void, size: usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
}

#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    context: *mut u8,
}

#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    next: *mut ExceptionRecord,
    address: usize,
    parameters: u32,
    /// For access violations: [0] 1 = write, [1] the address.
    information: [usize; 15],
}

const BREAKPOINT: u32 = 0x8000_0003;
const ACCESS_VIOLATION: u32 = 0xc000_0005;
const SINGLE_STEP: u32 = 0x8000_0004;
const CONTINUE_EXECUTION: i32 = -1;
const CONTINUE_SEARCH: i32 = 0;
/// i386 CONTEXT offsets.
const CTX_EIP: usize = 0xb8;
const CTX_EFLAGS: usize = 0xc0;
const TRAP_FLAG: u32 = 0x100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Entry,
    Jcc { len: usize, target: usize },
    Table { len: usize },
}

struct Site {
    function: usize,
    kind: Kind,
    original: u8,
    armed: bool,
    taken: bool,
    fallthrough: bool,
    entered: bool,
    targets: BTreeSet<usize>,
}

#[derive(Clone, Copy)]
enum Step {
    /// A jump (runtime address) whose destination is awaited.
    Branch(usize),
    /// A write let through on this page (runtime address of the page).
    Write(usize),
}

struct Watch {
    /// Runtime (start, end) of the watched ranges, page aligned.
    pages: Vec<(usize, usize)>,
    /// Runtime addresses written.
    /// Distinct addresses written: an open-addressing hash set (0 = empty),
    /// preallocated because the handler runs in the middle of code that may
    /// hold allocator state and must not allocate.
    writes: Box<[usize]>,
    /// More distinct addresses than the set holds.
    overflow: bool,
}

#[derive(Default)]
struct State {
    /// Runtime address -> site.
    sites: BTreeMap<usize, Site>,
    watch: Option<Watch>,
    stepping: Option<Step>,
}

static STATE: Mutex<State> = Mutex::new(State { sites: BTreeMap::new(), watch: None, stepping: None });
static HANDLER: std::sync::Once = std::sync::Once::new();
/// Binary of the running coverage measurement.
static BINARY: Mutex<Option<String>> = Mutex::new(None);

/// (address, way) -> reason (ways proven unreachable).
fn unreachable(binary: &str) -> BTreeMap<(usize, String), String> {
    let path = format!("{}/../docs/unreachable-branches.txt", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.splitn(4, '\t').collect();
            if f.len() != 4 || f[0] != binary {
                return None;
            }
            Some(((usize::from_str_radix(f[1], 16).ok()?, f[2].to_string()), f[3].to_string()))
        })
        .collect()
}

fn install_handler() {
    HANDLER.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, handler);
    });
}

const PAGE: usize = 0x1000;
const READONLY: u32 = 0x02;
const READWRITE: u32 = 0x04;

fn protect(start: usize, end: usize, protection: u32) {
    let mut old = 0;
    unsafe { VirtualProtect(start as *mut c_void, end - start, protection, &mut old) };
}

fn patch(addr: usize, byte: u8) {
    unsafe {
        let mut old = 0;
        VirtualProtect(addr as *mut c_void, 1, 0x40, &mut old);
        (addr as *mut u8).write_volatile(byte);
        VirtualProtect(addr as *mut c_void, 1, old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), addr as *const c_void, 1);
    }
}

extern "system" fn handler(info: *mut ExceptionPointers) -> i32 {
    let (record, ctx) = unsafe { (&*(*info).record, (*info).context) };
    let eip = unsafe { &mut *(ctx.add(CTX_EIP) as *mut u32) };
    let eflags = unsafe { &mut *(ctx.add(CTX_EFLAGS) as *mut u32) };
    let Ok(mut state) = STATE.try_lock() else { return CONTINUE_SEARCH };
    match record.code {
        BREAKPOINT => {
            let at = record.address;
            let Some(site) = state.sites.get_mut(&at) else { return CONTINUE_SEARCH };
            if !site.armed {
                return CONTINUE_SEARCH;
            }
            patch(at, site.original);
            site.armed = false;
            *eip = at as u32;
            let kind = site.kind;
            if !matches!(kind, Kind::Entry) {
                state.stepping = Some(Step::Branch(at));
                *eflags |= TRAP_FLAG;
            } else {
                site.entered = true;
            }
            CONTINUE_EXECUTION
        }
        ACCESS_VIOLATION if record.parameters >= 2 && record.information[0] == 1 => {
            let target = record.information[1];
            let Some(watch) = state.watch.as_mut() else { return CONTINUE_SEARCH };
            if !watch.pages.iter().any(|&(s, e)| (s..e).contains(&target)) {
                return CONTINUE_SEARCH;
            }
            // The instruction may write up to 8 bytes (or a string op one
            // element per step); record the faulting address, the rest of a
            // wider write shows as the following bytes of the same value.
            let mask = watch.writes.len() - 1;
            let mut slot = target.wrapping_mul(0x9e37_79b9) >> 4 & mask;
            let mut probes = 0;
            while watch.writes[slot] != 0 && watch.writes[slot] != target && probes <= mask {
                slot = (slot + 1) & mask;
                probes += 1;
            }
            if probes > mask {
                watch.overflow = true;
            } else {
                watch.writes[slot] = target;
            }
            let page = target & !(PAGE - 1);
            protect(page, page + PAGE, READWRITE);
            state.stepping = Some(Step::Write(page));
            *eflags |= TRAP_FLAG;
            CONTINUE_EXECUTION
        }
        SINGLE_STEP => {
            let Some(step) = state.stepping.take() else { return CONTINUE_SEARCH };
            *eflags &= !TRAP_FLAG;
            match step {
                Step::Write(page) => protect(page, page + PAGE, READONLY),
                Step::Branch(at) => {
                    let to = *eip as usize;
                    let site = state.sites.get_mut(&at).unwrap();
                    let rearm = match site.kind {
                        Kind::Jcc { len, target } => {
                            if to == target {
                                site.taken = true;
                            } else if to == at + len {
                                site.fallthrough = true;
                            }
                            !(site.taken && site.fallthrough)
                        }
                        Kind::Table { .. } => {
                            site.targets.insert(to);
                            true
                        }
                        Kind::Entry => false,
                    };
                    if rearm {
                        patch(at, 0xcc);
                        site.armed = true;
                    }
                }
            }
            CONTINUE_EXECUTION
        }
        _ => CONTINUE_SEARCH,
    }
}

/// Records which bytes of a module's writable sections get written.
pub struct Writes {
    base: usize,
    image_base: usize,
}

impl Writes {
    /// Starts watching all writable sections of `o`. Only the original's
    /// own code should run until [`Writes::finish`]: writes by the test
    /// itself are recorded too.
    pub fn start(o: &Original) -> Writes {
        install_handler();
        let pages: Vec<(usize, usize)> = o
            .writable
            .iter()
            .map(|&(rva, size)| ((o.base + rva) & !(PAGE - 1), (o.base + rva + size + PAGE - 1) & !(PAGE - 1)))
            .collect();
        for &(s, e) in &pages {
            protect(s, e, READONLY);
        }
        STATE.lock().unwrap().watch = Some(Watch { pages, writes: vec![0usize; 1 << 18].into_boxed_slice(), overflow: false });
        Writes { base: o.base, image_base: o.image_base }
    }

    /// Stops watching; Ghidra addresses of the bytes written (the first byte
    /// of each write).
    pub fn finish(self) -> BTreeSet<usize> {
        let watch = STATE.lock().unwrap().watch.take().unwrap();
        for &(s, e) in &watch.pages {
            protect(s, e, READWRITE);
        }
        assert!(!watch.overflow, "more than 2^18 distinct bytes of the original's globals written in one tracked region");
        watch.writes.iter().filter(|&&p| p != 0).map(|&p| p - self.base + self.image_base).collect()
    }
}

/// Coverage measurement over some functions of one loaded original.
pub struct Coverage {
    base: usize,
    image_base: usize,
}

/// Branches that were not driven both ways, per function (Ghidra addresses).
#[derive(Debug, Default)]
pub struct Report {
    pub functions: BTreeMap<usize, FunctionReport>,
}

#[derive(Debug, Default)]
pub struct FunctionReport {
    pub entered: bool,
    /// Missing ways listed as unreachable (proven
    /// impossible), with the reason.
    pub unreachable: Vec<(usize, &'static str, String)>,
    /// Conditional jumps: total and covered (both ways seen, or the unseen
    /// way proven impossible).
    pub jumps: usize,
    pub covered: usize,
    /// (jump address, way not seen: "taken" / "not taken").
    pub missing: Vec<(usize, &'static str)>,
    /// Switch jumps and the targets they reached.
    pub tables: Vec<(usize, BTreeSet<usize>)>,
}

impl FunctionReport {
    pub fn complete(&self) -> bool {
        self.entered && self.missing.is_empty()
    }
}

impl Coverage {
    /// Arms the functions starting at `functions` (Ghidra addresses) of
    /// `o`, whose binary is `binary` (e.g. `GfxEngine.dll`).
    pub fn start(o: &Original, binary: &str, functions: &[usize]) -> Coverage {
        BINARY.lock().unwrap().replace(binary.to_string());
        let path = format!("{}/../../export/{binary}/branches.tsv", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e} (run tools/branches.py)"));
        let wanted: BTreeSet<usize> = functions.iter().copied().collect();
        let mut state = State::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let hex = |s: &str| usize::from_str_radix(s, 16).unwrap();
            let (function, addr, len, target) = (hex(f[0]), hex(f[1]), f[2].parse::<usize>().unwrap(), hex(f[4]));
            if !wanted.contains(&function) {
                continue;
            }
            let kind = match f[3] {
                "entry" => Kind::Entry,
                "jcc" => Kind::Jcc { len, target: o.runtime(target) },
                _ => Kind::Table { len },
            };
            let at = o.runtime(addr);
            let original = unsafe { (at as *const u8).read() };
            state.sites.insert(
                at,
                Site { function, kind, original, armed: false, taken: false, fallthrough: false, entered: false, targets: BTreeSet::new() },
            );
        }
        for f in &wanted {
            assert!(state.sites.values().any(|s| s.function == *f), "{f:#x} is not a function in {path}");
        }
        for (&at, site) in state.sites.iter_mut() {
            patch(at, 0xcc);
            site.armed = true;
        }
        install_handler();
        STATE.lock().unwrap().sites = state.sites;
        Coverage { base: o.base, image_base: o.image_base }
    }

    /// Disarms everything and reports.
    pub fn finish(self) -> Report {
        let state = State { sites: std::mem::take(&mut STATE.lock().unwrap().sites), watch: None, stepping: None };
        let ghidra = |p: usize| p - self.base + self.image_base;
        let proven = unreachable(BINARY.lock().unwrap().as_deref().unwrap_or(""));
        let mut report = Report::default();
        for (&at, site) in &state.sites {
            if site.armed {
                patch(at, site.original);
            }
            let f = report.functions.entry(site.function).or_default();
            match site.kind {
                Kind::Entry => f.entered = site.entered,
                Kind::Jcc { .. } => {
                    f.jumps += 1;
                    let mut complete = true;
                    for (seen, way) in [(site.taken, "taken"), (site.fallthrough, "not taken")] {
                        if seen {
                            continue;
                        }
                        match proven.get(&(ghidra(at), way.to_string())) {
                            Some(reason) => f.unreachable.push((ghidra(at), way, reason.clone())),
                            None => {
                                f.missing.push((ghidra(at), way));
                                complete = false;
                            }
                        }
                    }
                    if complete {
                        f.covered += 1;
                    }
                }
                Kind::Table { .. } => f.tables.push((ghidra(at), site.targets.iter().map(|&t| ghidra(t)).collect())),
            }
        }
        report
    }
}

impl Report {
    /// Writes `logs/coverage/<binary>/<test>.tsv` (function, entered,
    /// branches both ways, branches, missing).
    pub fn save(&self, binary: &str, test: &str) {
        let dir = format!("{}/../../logs/coverage/{binary}", env!("CARGO_MANIFEST_DIR"));
        let _ = std::fs::create_dir_all(&dir);
        let mut s = String::new();
        for (f, r) in &self.functions {
            let missing: Vec<String> = r.missing.iter().map(|(a, w)| format!("{a:08x} {w}")).collect();
            s += &format!("{f:08x}\t{}\t{}\t{}\t{}\n", r.entered as u8, r.covered, r.jumps, missing.join(", "));
        }
        let _ = std::fs::write(format!("{dir}/{test}.tsv"), s);
    }

    /// One line per function, for test output.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        for (f, r) in &self.functions {
            s += &format!(
                "{f:08x}: {} {}/{} branches both ways{}\n",
                if r.entered { "entered" } else { "NOT ENTERED" },
                r.covered,
                r.jumps,
                if r.missing.is_empty() {
                    String::new()
                } else {
                    format!(", missing {}", r.missing.iter().map(|(a, w)| format!("{a:08x} {w}")).collect::<Vec<_>>().join(", "))
                }
            );
        }
        s
    }
}
