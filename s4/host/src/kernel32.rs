//! KERNEL32 for the guest.

use crate::handles::{self, FileObj, FindObj, Obj, Signal, ThreadObj};
use crate::{fs, heap, shims::tick, Shim};
use s4_x86::Cpu;
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};

macro_rules! shim {
    ($name:expr, $args:expr, $f:expr) => {
        Shim { dll: "kernel32.dll", name: $name, args: $args, cdecl: false, f: $f }
    };
}

thread_local! {
    static LAST_ERROR: Cell<u32> = const { Cell::new(0) };
    static THREAD_ID: Cell<u32> = const { Cell::new(1) };
    static TLS_VALUES: std::cell::RefCell<HashMap<u32, u32>> = std::cell::RefCell::new(HashMap::new());
}

static NEXT_THREAD_ID: AtomicU32 = AtomicU32::new(2);
static TLS_NEXT: AtomicU32 = AtomicU32::new(1);

pub fn set_last_error(e: u32) {
    LAST_ERROR.with(|l| l.set(e));
}

pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NO_MORE_FILES: u32 = 18;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const WAIT_TIMEOUT: u32 = 258;
pub const INVALID_HANDLE: u32 = 0xffff_ffff;

pub fn cstr(c: &Cpu, p: u32) -> String {
    c.mem.cstr(p).iter().map(|&b| b as char).collect()
}

pub fn wstr(c: &Cpu, p: u32) -> String {
    let mut v = Vec::new();
    let mut q = p;
    loop {
        let u = c.mem.r16(q);
        if u == 0 {
            break;
        }
        v.push(u);
        q += 2;
    }
    String::from_utf16_lossy(&v)
}

fn put_cstr(c: &Cpu, dst: u32, cap: u32, s: &str) -> u32 {
    let bytes: Vec<u8> = s.chars().map(|ch| if (ch as u32) < 256 { ch as u32 as u8 } else { b'?' }).collect();
    if bytes.len() as u32 + 1 > cap {
        return bytes.len() as u32 + 1;
    }
    c.mem.write_bytes(dst, &bytes);
    c.mem.w8(dst + bytes.len() as u32, 0);
    bytes.len() as u32
}

fn put_wstr(c: &Cpu, dst: u32, cap: u32, s: &str) -> u32 {
    let u: Vec<u16> = s.encode_utf16().collect();
    if u.len() as u32 + 1 > cap {
        return u.len() as u32 + 1;
    }
    for (i, &w) in u.iter().enumerate() {
        c.mem.w16(dst + 2 * i as u32, w);
    }
    c.mem.w16(dst + 2 * u.len() as u32, 0);
    u.len() as u32
}

fn atomic(c: &Cpu, addr: u32) -> &AtomicU32 {
    unsafe { &*((c.mem.base + addr as usize) as *const AtomicU32) }
}

// ---- time -------------------------------------------------------------

const EPOCH_DIFF: u64 = 116_444_736_000_000_000;

fn unix_now() -> std::time::Duration {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default()
}

fn filetime_now() -> u64 {
    let d = unix_now();
    d.as_secs() * 10_000_000 + d.subsec_nanos() as u64 / 100 + EPOCH_DIFF
}

/// Local time's offset from UTC in seconds (east positive) at the FILETIME `t`.
fn utc_offset(t: u64) -> i64 {
    let secs = (t.saturating_sub(EPOCH_DIFF) / 10_000_000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    tm.tm_gmtoff as i64
}

fn to_local(t: u64) -> u64 {
    (t as i64 + utc_offset(t) * 10_000_000) as u64
}

fn from_local(t: u64) -> u64 {
    (t as i64 - utc_offset(t) * 10_000_000) as u64
}

fn write_filetime(c: &Cpu, p: u32, t: u64) {
    c.mem.w32(p, t as u32);
    c.mem.w32(p + 4, (t >> 32) as u32);
}

fn read_filetime(c: &Cpu, p: u32) -> u64 {
    c.mem.r32(p) as u64 | (c.mem.r32(p + 4) as u64) << 32
}

/// (year, month, day, weekday) from days since 1970-01-01.
fn civil(days: i64) -> (i64, u32, u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let wd = ((days % 7 + 11) % 7) as u32; // 1970-01-01 was a Thursday (4)
    (if m <= 2 { y + 1 } else { y }, m, d, wd)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn write_systemtime(c: &Cpu, p: u32, ft: u64) {
    let unix100 = ft.saturating_sub(EPOCH_DIFF);
    let secs = (unix100 / 10_000_000) as i64;
    let ms = (unix100 / 10_000 % 1000) as u16;
    let (y, mo, d, wd) = civil(secs.div_euclid(86400));
    let tod = secs.rem_euclid(86400);
    for (i, v) in [y as u16, mo as u16, wd as u16, d as u16, (tod / 3600) as u16, (tod / 60 % 60) as u16, (tod % 60) as u16, ms].iter().enumerate() {
        c.mem.w16(p + 2 * i as u32, *v);
    }
}

fn read_systemtime(c: &Cpu, p: u32) -> u64 {
    let f = |i: u32| c.mem.r16(p + 2 * i) as i64;
    let days = days_from_civil(f(0), f(1) as u32, f(3) as u32);
    let secs = days * 86400 + f(4) * 3600 + f(5) * 60 + f(6);
    (secs as u64) * 10_000_000 + f(7) as u64 * 10_000 + EPOCH_DIFF
}

fn meta_filetime(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() * 10_000_000 + d.subsec_nanos() as u64 / 100 + EPOCH_DIFF).unwrap_or(EPOCH_DIFF)
}

// ---- files ------------------------------------------------------------

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_NEW: u32 = 1;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const TRUNCATE_EXISTING: u32 = 5;

fn attributes(m: &std::fs::Metadata) -> u32 {
    let mut a = if m.is_dir() { 0x10 } else { 0x80 };
    if m.permissions().readonly() {
        a |= 1;
    }
    a
}

fn create_file(c: &mut Cpu, name: String, access: u32, share: u32, disp: u32) -> u32 {
    let _ = share;
    let path = fs::host_path(&name);
    if std::env::var_os("S4_HOST_TRACE").is_some() {
        eprintln!("  CreateFile {name:?} -> {} (exists {})", path.display(), path.exists());
    }
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    let write = access & GENERIC_WRITE != 0;
    if write {
        o.write(true);
    }
    let exists = path.exists();
    match disp {
        CREATE_NEW => {
            if exists {
                set_last_error(ERROR_ALREADY_EXISTS + 0x32 - 0x32 + 0x0);
                set_last_error(80);
                return INVALID_HANDLE;
            }
            o.write(true).create_new(true);
        }
        CREATE_ALWAYS => {
            o.write(true).create(true).truncate(true);
        }
        OPEN_EXISTING => {
            if !exists {
                set_last_error(if path.parent().map(|p| p.exists()).unwrap_or(false) { ERROR_FILE_NOT_FOUND } else { ERROR_PATH_NOT_FOUND });
                return INVALID_HANDLE;
            }
        }
        OPEN_ALWAYS => {
            o.write(true).create(true);
        }
        TRUNCATE_EXISTING => {
            if !exists {
                set_last_error(ERROR_FILE_NOT_FOUND);
                return INVALID_HANDLE;
            }
            o.write(true).truncate(true);
        }
        _ => return INVALID_HANDLE,
    }
    if path.is_dir() {
        // Directories can be opened for attribute queries only.
        set_last_error(ERROR_ACCESS_DENIED);
        return INVALID_HANDLE;
    }
    match o.open(&path) {
        Ok(f) => {
            set_last_error(if exists && (disp == CREATE_ALWAYS || disp == OPEN_ALWAYS) { ERROR_ALREADY_EXISTS } else { 0 });
            let _ = c;
            handles::insert(Obj::File(Arc::new(Mutex::new(FileObj { file: f, path, append: false }))))
        }
        Err(e) => {
            set_last_error(match e.kind() {
                std::io::ErrorKind::NotFound => ERROR_PATH_NOT_FOUND,
                std::io::ErrorKind::PermissionDenied => ERROR_ACCESS_DENIED,
                _ => ERROR_ACCESS_DENIED,
            });
            INVALID_HANDLE
        }
    }
}

fn find_files(pattern: &str) -> Vec<(String, std::fs::Metadata)> {
    let abs = fs::absolute(pattern);
    let (dir, pat) = abs.rsplit_once('\\').unwrap();
    let dir_host = fs::host_path(dir);
    let mut out = Vec::new();
    let pat_l = pat.to_lowercase();
    if let Ok(rd) = std::fs::read_dir(&dir_host) {
        let mut names: Vec<(String, std::fs::Metadata)> = rd.flatten().filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), e.metadata().ok()?))).collect();
        names.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        if wildcard(&pat_l, ".") || wildcard(&pat_l, "..") {
            // Windows lists the dot entries first for a real directory.
        }
        for (n, m) in names {
            if wildcard(&pat_l, &n.to_lowercase()) {
                out.push((n, m));
            }
        }
    }
    out
}

pub fn wildcard(pat: &str, name: &str) -> bool {
    fn go(p: &[u8], n: &[u8]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some((b'*', rest)) => (0..=n.len()).any(|i| go(rest, &n[i..])),
            Some((b'?', rest)) => !n.is_empty() && go(rest, &n[1..]),
            Some((&ch, rest)) => n.first() == Some(&ch) && go(rest, &n[1..]),
        }
    }
    // "*.*" matches names without a dot too.
    if pat == "*.*" {
        return true;
    }
    go(pat.as_bytes(), name.as_bytes())
}

fn fill_find_data(c: &Cpu, p: u32, name: &str, m: &std::fs::Metadata, wide: bool) {
    c.mem.fill(p, 0, if wide { 592 } else { 320 });
    c.mem.w32(p, attributes(m));
    let t = meta_filetime(m);
    for off in [4, 12, 20] {
        write_filetime(c, p + off, t);
    }
    let size = if m.is_dir() { 0 } else { m.len() };
    c.mem.w32(p + 28, (size >> 32) as u32);
    c.mem.w32(p + 32, size as u32);
    if wide {
        put_wstr(c, p + 44, 260, name);
    } else {
        put_cstr(c, p + 44, 260, name);
    }
}

static ANON_NAMED: Mutex<std::collections::BTreeMap<String, Arc<handles::AnonMap>>> = Mutex::new(std::collections::BTreeMap::new());
static ANON_VIEWS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

// ---- synchronisation ---------------------------------------------------

static CRITICAL: Mutex<Option<HashMap<u32, Arc<(Mutex<(u32, u32)>, Condvar)>>>> = Mutex::new(None);

fn cs(addr: u32) -> Arc<(Mutex<(u32, u32)>, Condvar)> {
    let mut g = CRITICAL.lock().unwrap();
    g.get_or_insert_with(HashMap::new).entry(addr).or_insert_with(|| Arc::new((Mutex::new((0, 0)), Condvar::new()))).clone()
}

pub fn current_thread_id() -> u32 {
    me()
}

/// Gives the calling host thread (a timer, say) a guest thread id of its own.
pub fn register_host_thread() {
    let id = NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst);
    THREAD_ID.with(|x| x.set(id));
}

fn me() -> u32 {
    THREAD_ID.with(|t| t.get())
}

fn obj_kind_is_mutex(h: u32) -> bool {
    matches!(handles::get(h), Some(Obj::Mutex(_)))
}

fn wait_object(h: u32, timeout: u32) -> u32 {
    let obj = match handles::get(h) {
        Some(o) => o,
        None => {
            set_last_error(ERROR_INVALID_HANDLE);
            return 0xffff_ffff;
        }
    };
    let deadline = if timeout == 0xffff_ffff { None } else { Some(std::time::Instant::now() + std::time::Duration::from_millis(timeout as u64)) };
    match obj {
        Obj::Event(s) | Obj::Mutex(s) => {
            let is_mutex = matches!(obj_kind_is_mutex(h), true);
            let me = me();
            let mut st = s.state.lock().unwrap();
            loop {
                if is_mutex {
                    let mut o = s.owner.lock().unwrap();
                    if o.1 > 0 && o.0 == me {
                        o.1 += 1;
                        return 0;
                    }
                    if *st {
                        *st = false;
                        *o = (me, 1);
                        return 0;
                    }
                }
                if *st && !is_mutex {
                    if !s.manual {
                        *st = false;
                    }
                    return 0;
                }
                match deadline {
                    None => st = s.cond.wait(st).unwrap(),
                    Some(d) => {
                        let now = std::time::Instant::now();
                        if now >= d {
                            return WAIT_TIMEOUT;
                        }
                        st = s.cond.wait_timeout(st, d - now).unwrap().0;
                    }
                }
            }
        }
        Obj::Semaphore(s) => {
            let mut st = s.state.lock().unwrap();
            loop {
                {
                    let mut o = s.owner.lock().unwrap();
                    if o.0 > 0 {
                        o.0 -= 1;
                        return 0;
                    }
                }
                match deadline {
                    None => st = s.cond.wait(st).unwrap(),
                    Some(d) => {
                        let now = std::time::Instant::now();
                        if now >= d {
                            return WAIT_TIMEOUT;
                        }
                        st = s.cond.wait_timeout(st, d - now).unwrap().0;
                    }
                }
            }
        }
        Obj::Thread(t) => {
            let mut st = t.exit.lock().unwrap();
            loop {
                if st.is_some() {
                    return 0;
                }
                match deadline {
                    None => st = t.cond.wait(st).unwrap(),
                    Some(d) => {
                        let now = std::time::Instant::now();
                        if now >= d {
                            return WAIT_TIMEOUT;
                        }
                        st = t.cond.wait_timeout(st, d - now).unwrap().0;
                    }
                }
            }
        }
        _ => 0,
    }
}

/// `WaitForMultipleObjects`: polls (the waits here are short and rare).
pub fn wait_multiple(c: &Cpu, n: u32, handles_at: u32, all: bool, timeout: u32) -> u32 {
    let hs: Vec<u32> = (0..n).map(|i| c.mem.r32(handles_at + 4 * i)).collect();
    let deadline = if timeout == 0xffff_ffff { None } else { Some(std::time::Instant::now() + std::time::Duration::from_millis(timeout as u64)) };
    let mut done = vec![false; hs.len()];
    loop {
        for (i, &h) in hs.iter().enumerate() {
            if !done[i] && wait_object(h, 0) == 0 {
                if !all {
                    return i as u32;
                }
                done[i] = true;
            }
        }
        if all && done.iter().all(|&d| d) {
            return 0;
        }
        if let Some(d) = deadline {
            if std::time::Instant::now() >= d {
                return WAIT_TIMEOUT;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

pub(crate) fn new_signal(manual: bool, initial: bool) -> Arc<Signal> {
    Arc::new(Signal { state: Mutex::new(initial), cond: Condvar::new(), manual, owner: Mutex::new((0, 0)) })
}

/// Panic payload of `ExitThread`.
pub struct ThreadExit(pub u32);

/// Starts a guest thread running `start(param)`; returns (handle, id).
pub fn spawn_guest(start: u32, param: u32) -> (u32, u32) {
    spawn_guest_flags(start, param, false)
}

pub fn spawn_guest_flags(start: u32, param: u32, suspended: bool) -> (u32, u32) {
    let id = NEXT_THREAD_ID.fetch_add(1, Ordering::SeqCst);
    let t = Arc::new(ThreadObj { exit: Mutex::new(None), cond: Condvar::new(), id, gate: Mutex::new(!suspended), gate_cond: Condvar::new() });
    let handle = handles::insert(Obj::Thread(t.clone()));
    let mut c = crate::new_cpu(0);
    let stack = heap::vm_alloc(&mut c, 1 << 20);
    c.esp = stack + (1 << 20) - 64;
    std::thread::Builder::new()
        .name(format!("guest-{id}"))
        .stack_size(64 << 20)
        .spawn(move || {
            THREAD_ID.with(|x| x.set(id));
            {
                let mut g = t.gate.lock().unwrap();
                while !*g {
                    g = t.gate_cond.wait(g).unwrap();
                }
            }
            let mut c = c;
            crate::gil::acquire();
            let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::shims::guest_call(&mut c, start, &[param]))) {
                Ok(r) => r,
                Err(p) => match p.downcast::<ThreadExit>() {
                    Ok(e) => e.0,
                    Err(p) => std::panic::resume_unwind(p),
                },
            };
            crate::gil::release();
            *t.exit.lock().unwrap() = Some(r);
            t.cond.notify_all();
        })
        .expect("thread");
    (handle, id)
}

/// System DLLs the host implements with shims.
fn is_system_dll(name: &str) -> bool {
    let n = name.rsplit('\\').next().unwrap_or(name).to_lowercase();
    let n = n.trim_end_matches(".dll");
    ["kernel32", "user32", "gdi32", "advapi32", "winmm", "ws2_32", "dsound", "dinput", "wsock32", "ddraw", "shell32", "oleaut32", "version", "ole32", "mss32", "binkw32"].contains(&n)
}

fn module_by_name(name: &str) -> u32 {
    let n = name.rsplit('\\').next().unwrap_or(name).to_lowercase();
    let n = n.trim_end_matches(".dll").trim_end_matches(".exe").to_string();
    crate::module_base(&n)
}

pub fn all() -> Vec<Shim> {
    vec![
        // errors and process
        shim!("GetLastError", 0, |_, _| LAST_ERROR.with(|l| l.get())),
        shim!("SetLastError", 1, |_, a| {
            set_last_error(a[0]);
            0
        }),
        shim!("GetVersion", 0, |_, _| 0x0a28_0105),
        shim!("GetVersionExA", 1, |c, a| {
            c.mem.fill(a[0] + 4, 0, 144);
            c.mem.w32(a[0] + 4, 5);
            c.mem.w32(a[0] + 8, 1);
            c.mem.w32(a[0] + 12, 2600);
            c.mem.w32(a[0] + 16, 2);
            1
        }),
        shim!("GetCommandLineA", 0, |c, _| {
            let p = heap::alloc(c, 64);
            put_cstr(c, p, 64, &crate::main_module().1);
            p
        }),
        shim!("GetCommandLineW", 0, |c, _| {
            let p = heap::alloc(c, 128);
            put_wstr(c, p, 64, &crate::main_module().1);
            p
        }),
        shim!("GetStartupInfoA", 1, |c, a| {
            c.mem.fill(a[0], 0, 68);
            c.mem.w32(a[0], 68);
            0
        }),
        shim!("GetModuleHandleA", 1, |c, a| {
            if a[0] == 0 {
                return crate::main_module().0;
            }
            let name = cstr(c, a[0]);
            let m = module_by_name(&name);
            if m == 0 && is_system_dll(&name) { crate::fake_module(&name) } else { m }
        }),
        shim!("GetModuleFileNameA", 3, |c, a| {
            let name = if a[0] == 0 || a[0] == crate::main_module().0 { crate::main_module().1 } else { format!("C:\\S4\\Exe\\{}.dll", crate::module_name(a[0])) };
            put_cstr(c, a[1], a[2], &name)
        }),
        shim!("GetModuleFileNameW", 3, |c, a| {
            let name = if a[0] == 0 || a[0] == crate::main_module().0 { crate::main_module().1 } else { format!("C:\\S4\\Exe\\{}.dll", crate::module_name(a[0])) };
            put_wstr(c, a[1], a[2], &name)
        }),
        shim!("LoadLibraryA", 1, |c, a| {
            let name = cstr(c, a[0]);
            let m = module_by_name(&name);
            if m != 0 { m } else if is_system_dll(&name) { crate::fake_module(&name) } else { set_last_error(ERROR_FILE_NOT_FOUND); 0 }
        }),
        shim!("FreeLibrary", 1, |_, _| 1),
        shim!("DisableThreadLibraryCalls", 1, |_, _| 1),
        shim!("GetProcAddress", 2, |c, a| {
            if a[1] < 0x10000 {
                return 0;
            }
            let name = cstr(c, a[1]);
            crate::proc_address(a[0], &name)
        }),
        shim!("GetCurrentProcess", 0, |_, _| 0xffff_ffff),
        shim!("GetCurrentProcessId", 0, |_, _| 1000),
        shim!("GetCurrentThread", 0, |_, _| 0xffff_fffe),
        shim!("GetCurrentThreadId", 0, |_, _| me()),
        shim!("ExitProcess", 1, |_, a| std::process::exit(a[0] as i32)),
        shim!("TerminateProcess", 2, |_, a| std::process::exit(a[1] as i32)),
        // Unwinds the Rust frames of the thread up to its start (spawn_guest_flags), like a jump out of the guest.
        shim!("ExitThread", 1, |_, a| std::panic::resume_unwind(Box::new(ThreadExit(a[0])))),
        shim!("SetUnhandledExceptionFilter", 1, |_, _| 0),
        shim!("UnhandledExceptionFilter", 1, |_, _| 0),
        shim!("SetErrorMode", 1, |_, _| 0),
        shim!("RaiseException", 4, crate::seh::raise_exception),
        shim!("RtlUnwind", 4, crate::seh::rtl_unwind),
        shim!("GetThreadContext", 2, |_, _| 0),
        shim!("OutputDebugStringA", 1, |c, a| {
            if std::env::var_os("S4_DEBUG_OUT").is_some() {
                eprintln!("OutputDebugString: {}", cstr(c, a[0]));
            }
            0
        }),
        shim!("Beep", 2, |_, _| 1),
        // memory
        shim!("GetProcessHeap", 0, |_, _| 0x5000_0001),
        shim!("HeapCreate", 3, |_, _| 0x5000_0001),
        shim!("HeapDestroy", 1, |_, _| 1),
        shim!("HeapAlloc", 3, |c, a| if a[1] & 8 != 0 { heap::alloc_zeroed(c, a[2]) } else { heap::alloc(c, a[2]) }),
        shim!("HeapReAlloc", 4, |c, a| heap::realloc(c, a[2], a[3])),
        shim!("HeapFree", 3, |c, a| {
            heap::free(c, a[2]);
            1
        }),
        shim!("HeapSize", 3, |c, a| heap::size(c, a[2])),
        shim!("HeapValidate", 3, |_, _| 1),
        shim!("HeapCompact", 2, |_, _| 0x10000),
        shim!("HeapWalk", 2, |_, _| 0),
        shim!("GlobalAlloc", 2, |c, a| if a[0] & 0x40 != 0 { heap::alloc_zeroed(c, a[1]) } else { heap::alloc(c, a[1]) }),
        shim!("GlobalLock", 1, |_, a| a[0]),
        shim!("GlobalUnlock", 1, |_, _| 1),
        shim!("GlobalFree", 1, |c, a| {
            heap::free(c, a[0]);
            0
        }),
        shim!("LocalFree", 1, |c, a| {
            heap::free(c, a[0]);
            0
        }),
        shim!("LocalAlloc", 2, |c, a| if a[0] & 0x40 != 0 { heap::alloc_zeroed(c, a[1]) } else { heap::alloc(c, a[1]) }),
        shim!("GlobalMemoryStatus", 1, |c, a| {
            c.mem.fill(a[0], 0, 32);
            c.mem.w32(a[0], 32);
            c.mem.w32(a[0] + 4, 30);
            for (o, v) in [(8, 1u32 << 29), (12, 1 << 28), (16, 1 << 30), (20, 1 << 29), (24, 0x7fff_0000), (28, 0x7000_0000)] {
                c.mem.w32(a[0] + o, v);
            }
            0
        }),
        shim!("GetSystemInfo", 1, |c, a| {
            c.mem.fill(a[0], 0, 36);
            c.mem.w32(a[0] + 4, 4096);
            c.mem.w32(a[0] + 8, 0x10000);
            c.mem.w32(a[0] + 12, 0x7ffe_fffe);
            c.mem.w32(a[0] + 20, 1);
            c.mem.w32(a[0] + 24, 586);
            c.mem.w32(a[0] + 28, 0x10000);
            0
        }),
        shim!("VirtualAlloc", 4, |c, a| {
            if a[0] != 0 && a[0] >= crate::heap::VM_START {
                return a[0];
            }
            heap::vm_alloc(c, a[1])
        }),
        shim!("VirtualFree", 3, |_, _| 1),
        shim!("VirtualQuery", 3, |c, a| {
            c.mem.fill(a[1], 0, 28);
            c.mem.w32(a[1], a[0] & !0xfff);
            c.mem.w32(a[1] + 4, a[0] & !0xfff);
            c.mem.w32(a[1] + 12, 0x1000);
            c.mem.w32(a[1] + 16, 0x1000);
            c.mem.w32(a[1] + 20, 0x04);
            28
        }),
        shim!("IsBadReadPtr", 2, |_, a| (a[0] == 0) as u32),
        shim!("IsBadWritePtr", 2, |_, a| (a[0] == 0) as u32),
        shim!("IsBadCodePtr", 1, |_, a| (a[0] == 0) as u32),
        // interlocked
        shim!("InterlockedIncrement", 1, |c, a| atomic(c, a[0]).fetch_add(1, Ordering::SeqCst).wrapping_add(1)),
        shim!("InterlockedDecrement", 1, |c, a| atomic(c, a[0]).fetch_sub(1, Ordering::SeqCst).wrapping_sub(1)),
        shim!("InterlockedExchange", 2, |c, a| atomic(c, a[0]).swap(a[1], Ordering::SeqCst)),
        // TLS
        shim!("TlsAlloc", 0, |_, _| TLS_NEXT.fetch_add(1, Ordering::SeqCst)),
        shim!("TlsFree", 1, |_, _| 1),
        shim!("TlsSetValue", 2, |_, a| {
            TLS_VALUES.with(|t| t.borrow_mut().insert(a[0], a[1]));
            1
        }),
        shim!("TlsGetValue", 1, |_, a| {
            set_last_error(0);
            TLS_VALUES.with(|t| t.borrow().get(&a[0]).copied().unwrap_or(0))
        }),
        // critical sections (the structure lives in guest memory; its address is the key)
        shim!("InitializeCriticalSection", 1, |c, a| {
            c.mem.fill(a[0], 0, 24);
            let _ = cs(a[0]);
            0
        }),
        shim!("DeleteCriticalSection", 1, |_, _| 0),
        shim!("EnterCriticalSection", 1, |_, a| {
            let obj = cs(a[0]);
            let mut g = obj.0.lock().unwrap();
            loop {
                if g.1 == 0 || g.0 == me() {
                    g.0 = me();
                    g.1 += 1;
                    return 0;
                }
                g = obj.1.wait(g).unwrap();
            }
        }),
        shim!("LeaveCriticalSection", 1, |_, a| {
            let obj = cs(a[0]);
            let mut g = obj.0.lock().unwrap();
            g.1 = g.1.saturating_sub(1);
            if g.1 == 0 {
                g.0 = 0;
                obj.1.notify_one();
            }
            0
        }),
        // events, mutexes, waiting, threads
        shim!("CreateEventA", 4, |_, a| handles::insert(Obj::Event(new_signal(a[1] != 0, a[2] != 0)))),
        shim!("SetEvent", 1, |_, a| match handles::get(a[0]) {
            Some(Obj::Event(s)) => {
                *s.state.lock().unwrap() = true;
                s.cond.notify_all();
                1
            }
            _ => 0,
        }),
        shim!("ResetEvent", 1, |_, a| match handles::get(a[0]) {
            Some(Obj::Event(s)) => {
                *s.state.lock().unwrap() = false;
                1
            }
            _ => 0,
        }),
        shim!("CreateMutexA", 3, |_, a| handles::insert(Obj::Mutex(new_signal(false, a[1] == 0)))),
        shim!("OpenMutexA", 3, |_, _| 0),
        shim!("WaitForSingleObject", 2, |c, a| {
            let r = wait_object(a[0], a[1]);
            // S4_WAIT_TRACE=1: where a wait that times out was called from (the return addresses on the guest stack).
            if r == WAIT_TIMEOUT && a[1] >= 100 && std::env::var_os("S4_WAIT_TRACE").is_some() {
                let calls: Vec<String> = (0..160).map(|i| c.mem.r32(c.esp + 4 * i)).filter(|v| (0x40_0000..0x80_0000).contains(v) || (0x70b3_0000..0x70c0_0000).contains(v)).take(14).map(|v| format!("{v:#x}")).collect();
                eprintln!("wait timeout t{} handle {:#x} {} ms: {}", me(), a[0], a[1], calls.join(" "));
            }
            r
        }),
        shim!("Sleep", 1, |c, a| {
            if std::env::var_os("S4_SLEEP_TRACE").is_some() {
                eprintln!("Sleep({}) at {} ms from {:#x}", a[0], crate::shims::tick(), c.mem.r32(c.esp));
            }
            if std::env::var_os("S4_NO_SLEEP").is_none() {
                std::thread::sleep(std::time::Duration::from_millis(a[0] as u64));
            }
            0
        }),
        shim!("CreateThread", 6, |c, a| {
            if std::env::var_os("S4_HOST_TRACE").is_some() {
                eprintln!("CreateThread start={:#x} param={:#x} flags={:#x}", a[2], a[3], a[4]);
            }
            let (h, id) = spawn_guest_flags(a[2], a[3], a[4] & 4 != 0);
            if a[5] != 0 {
                c.mem.w32(a[5], id);
            }
            h
        }),
        shim!("ResumeThread", 1, |_, a| match handles::get(a[0]) {
            Some(Obj::Thread(t)) => {
                *t.gate.lock().unwrap() = true;
                t.gate_cond.notify_all();
                1
            }
            _ => 0xffff_ffff,
        }),
        shim!("SuspendThread", 1, |_, _| 0),
        shim!("TerminateThread", 2, |_, _| 1),
        shim!("GetExitCodeThread", 2, |c, a| match handles::get(a[0]) {
            Some(Obj::Thread(t)) => {
                c.mem.w32(a[1], t.exit.lock().unwrap().unwrap_or(259));
                1
            }
            _ => 0,
        }),
        shim!("SetThreadPriority", 2, |_, _| 1),
        shim!("GetThreadPriority", 1, |_, _| 0),
        shim!("CloseHandle", 1, |_, a| handles::close(a[0]) as u32),
        shim!("DuplicateHandle", 7, |c, a| {
            match handles::get(a[1]) {
                Some(o) => {
                    let h = handles::insert(o);
                    c.mem.w32(a[3], h);
                    1
                }
                None => 0,
            }
        }),
        // time
        shim!("GetTickCount", 0, |_, _| tick()),
        shim!("QueryPerformanceCounter", 1, |c, a| {
            let us = crate::shims::micros();
            c.mem.w32(a[0], us as u32);
            c.mem.w32(a[0] + 4, (us >> 32) as u32);
            1
        }),
        shim!("QueryPerformanceFrequency", 1, |c, a| {
            c.mem.w32(a[0], 1_000_000);
            c.mem.w32(a[0] + 4, 0);
            1
        }),
        shim!("GetSystemTime", 1, |c, a| {
            write_systemtime(c, a[0], filetime_now());
            0
        }),
        shim!("GetLocalTime", 1, |c, a| {
            write_systemtime(c, a[0], to_local(filetime_now()));
            0
        }),
        shim!("SetLocalTime", 1, |_, _| 1),
        shim!("GetSystemTimeAsFileTime", 1, |c, a| {
            write_filetime(c, a[0], filetime_now());
            0
        }),
        shim!("FileTimeToSystemTime", 2, |c, a| {
            let t = read_filetime(c, a[0]);
            write_systemtime(c, a[1], t);
            1
        }),
        shim!("SystemTimeToFileTime", 2, |c, a| {
            let t = read_systemtime(c, a[0]);
            write_filetime(c, a[1], t);
            1
        }),
        shim!("FileTimeToLocalFileTime", 2, |c, a| {
            let t = read_filetime(c, a[0]);
            write_filetime(c, a[1], to_local(t));
            1
        }),
        shim!("LocalFileTimeToFileTime", 2, |c, a| {
            let t = read_filetime(c, a[0]);
            write_filetime(c, a[1], from_local(t));
            1
        }),
        shim!("GetTimeZoneInformation", 1, |c, a| {
            c.mem.fill(a[0], 0, 172);
            // Bias is minutes west of UTC; the standard-time name stays empty.
            c.mem.w32(a[0], (-utc_offset(filetime_now()) / 60) as i32 as u32);
            1
        }),
        // strings and locales
        shim!("lstrlenA", 1, |c, a| c.mem.cstr(a[0]).len() as u32),
        shim!("lstrlenW", 1, |c, a| wstr(c, a[0]).encode_utf16().count() as u32),
        shim!("MulDiv", 3, |_, a| {
            if a[2] == 0 {
                return 0xffff_ffff;
            }
            let v = (a[0] as i32 as i64 * a[1] as i32 as i64) as i128;
            let d = a[2] as i32 as i128;
            let r = if (v < 0) == (d < 0) { (v.abs() + d.abs() / 2) / d.abs() } else { -((v.abs() + d.abs() / 2) / d.abs()) };
            r as i32 as u32
        }),
        shim!("GetACP", 0, |_, _| 1252),
        shim!("GetOEMCP", 0, |_, _| 437),
        shim!("IsValidCodePage", 1, |_, _| 1),
        shim!("IsValidLocale", 2, |_, _| 1),
        shim!("GetUserDefaultLCID", 0, |_, _| 0x0409),
        shim!("GetCPInfo", 2, |c, a| {
            c.mem.fill(a[1], 0, 18);
            c.mem.w32(a[1], 1);
            c.mem.w8(a[1] + 4, b'?');
            1
        }),
        shim!("GetLocaleInfoA", 4, |c, a| {
            let s = match a[1] & 0xffff {
                0x0f => "English".to_string(), // LOCALE_SENGLANGUAGE style placeholder
                _ => "1".to_string(),
            };
            if a[3] == 0 {
                return s.len() as u32 + 1;
            }
            put_cstr(c, a[2], a[3], &s) + 1
        }),
        shim!("GetLocaleInfoW", 4, |c, a| {
            if a[3] == 0 {
                return 2;
            }
            put_wstr(c, a[2], a[3], "1") + 1
        }),
        shim!("EnumSystemLocalesA", 2, |_, _| 1),
        shim!("MultiByteToWideChar", 6, |c, a| {
            let (src, len) = (a[2], a[3] as i32);
            let bytes = if len < 0 { c.mem.cstr(src) } else { c.mem.read_bytes(src, len as usize) };
            let n = bytes.len() as u32 + if len < 0 { 1 } else { 0 };
            if a[5] == 0 {
                return n;
            }
            for (i, &b) in bytes.iter().enumerate() {
                c.mem.w16(a[4] + 2 * i as u32, b as u16);
            }
            if len < 0 {
                c.mem.w16(a[4] + 2 * bytes.len() as u32, 0);
            }
            n
        }),
        shim!("WideCharToMultiByte", 8, |c, a| {
            let (src, len) = (a[2], a[3] as i32);
            let mut v = Vec::new();
            let mut i = 0;
            loop {
                if len >= 0 && i >= len as u32 {
                    break;
                }
                let u = c.mem.r16(src + 2 * i);
                if len < 0 && u == 0 {
                    v.push(0u8);
                    break;
                }
                v.push(if u < 256 { u as u8 } else { b'?' });
                i += 1;
            }
            let n = v.len() as u32;
            if a[5] == 0 {
                return n;
            }
            c.mem.write_bytes(a[4], &v);
            n
        }),
        shim!("CompareStringA", 6, |c, a| {
            let flags = a[1];
            let (mut x, mut y) = (cstr(c, a[2]), cstr(c, a[4]));
            if a[3] as i32 >= 0 {
                x.truncate(a[3] as usize);
            }
            if a[5] as i32 >= 0 {
                y.truncate(a[5] as usize);
            }
            if flags & 1 != 0 {
                x = x.to_lowercase();
                y = y.to_lowercase();
            }
            match x.cmp(&y) {
                std::cmp::Ordering::Less => 1,
                std::cmp::Ordering::Equal => 2,
                std::cmp::Ordering::Greater => 3,
            }
        }),
        shim!("CompareStringW", 6, |c, a| {
            let (x, y) = (wstr(c, a[2]), wstr(c, a[4]));
            let (x, y) = if a[1] & 1 != 0 { (x.to_lowercase(), y.to_lowercase()) } else { (x, y) };
            match x.cmp(&y) {
                std::cmp::Ordering::Less => 1,
                std::cmp::Ordering::Equal => 2,
                std::cmp::Ordering::Greater => 3,
            }
        }),
        shim!("LCMapStringA", 6, |c, a| {
            let src = if a[3] as i32 >= 0 { c.mem.read_bytes(a[2], a[3] as usize) } else { let mut v = c.mem.cstr(a[2]); v.push(0); v };
            if a[5] == 0 {
                return src.len() as u32;
            }
            let out: Vec<u8> = src.iter().map(|&b| if a[1] & 0x200 != 0 { b.to_ascii_lowercase() } else if a[1] & 0x100 != 0 { b.to_ascii_uppercase() } else { b }).collect();
            c.mem.write_bytes(a[4], &out[..out.len().min(a[5] as usize)]);
            out.len() as u32
        }),
        shim!("LCMapStringW", 6, |c, a| {
            let n = if a[3] as i32 >= 0 { a[3] } else { wstr(c, a[2]).encode_utf16().count() as u32 + 1 };
            if a[5] == 0 {
                return n;
            }
            for i in 0..n.min(a[5]) {
                let u = c.mem.r16(a[2] + 2 * i);
                let u = if u < 128 && a[1] & 0x200 != 0 { (u as u8).to_ascii_lowercase() as u16 } else if u < 128 && a[1] & 0x100 != 0 { (u as u8).to_ascii_uppercase() as u16 } else { u };
                c.mem.w16(a[4] + 2 * i, u);
            }
            n
        }),
        shim!("GetStringTypeA", 5, |c, a| {
            let n = if a[3] as i32 >= 0 { a[3] } else { c.mem.cstr(a[2]).len() as u32 + 1 };
            for i in 0..n {
                let ch = c.mem.r8(a[2] + i);
                c.mem.w16(a[4] + 2 * i, ctype(ch as u16));
            }
            1
        }),
        shim!("GetStringTypeW", 4, |c, a| {
            let n = if a[2] as i32 >= 0 { a[2] } else { wstr(c, a[1]).encode_utf16().count() as u32 + 1 };
            for i in 0..n {
                let u = c.mem.r16(a[1] + 2 * i);
                c.mem.w16(a[3] + 2 * i, ctype(u));
            }
            1
        }),
        // environment
        shim!("GetEnvironmentStrings", 0, |c, _| {
            let p = heap::alloc_zeroed(c, 16);
            p
        }),
        shim!("GetEnvironmentStringsW", 0, |c, _| heap::alloc_zeroed(c, 16)),
        shim!("FreeEnvironmentStringsA", 1, |_, _| 1),
        shim!("FreeEnvironmentStringsW", 1, |_, _| 1),
        shim!("GetEnvironmentVariableA", 3, |_, _| 0),
        shim!("SetEnvironmentVariableA", 2, |_, _| 1),
        shim!("SetEnvironmentVariableW", 2, |_, _| 1),
        shim!("SetHandleCount", 1, |_, a| a[0]),
        shim!("GetComputerNameA", 2, |c, a| {
            c.mem.w32(a[1], 6);
            put_cstr(c, a[0], 16, "S4HOST");
            1
        }),
        // console (none)
        shim!("GetStdHandle", 1, |_, a| handles::std_handle(a[0])),
        shim!("SetStdHandle", 2, |_, _| 1),
        shim!("GetConsoleMode", 2, |_, _| 0),
        shim!("SetConsoleMode", 2, |_, _| 0),
        shim!("SetConsoleCtrlHandler", 2, |_, _| 1),
        shim!("GetNumberOfConsoleInputEvents", 2, |_, _| 0),
        shim!("PeekConsoleInputA", 4, |_, _| 0),
        shim!("ReadConsoleInputA", 4, |_, _| 0),
        shim!("ReadConsoleA", 5, |_, _| 0),
        shim!("WriteConsoleA", 5, |c, a| {
            c.mem.w32(a[3], a[2]);
            1
        }),
        shim!("GetFileType", 1, |_, a| match handles::get(a[0]) {
            Some(Obj::Std(_)) => 2,
            Some(Obj::File(_)) => 1,
            _ => 0,
        }),
        // files
        shim!("CreateFileA", 7, |c, a| {
            let name = cstr(c, a[0]);
            // S4_SKIP_VIDEOS=1 (for scripted tests): videos "do not exist", the game goes on without them.
            if name.to_ascii_lowercase().ends_with(".bik") && std::env::var_os("S4_SKIP_VIDEOS").is_some() {
                set_last_error(2);
                return 0xffff_ffff;
            }
            create_file(c, name, a[1], a[2], a[4])
        }),
        shim!("CreateFileW", 7, |c, a| {
            let name = wstr(c, a[0]);
            create_file(c, name, a[1], a[2], a[4])
        }),
        shim!("ReadFile", 5, |c, a| {
            let Some(Obj::File(f)) = handles::get(a[0]) else {
                if let Some(Obj::Std(_)) = handles::get(a[0]) {
                    c.mem.w32(a[3], 0);
                    return 1;
                }
                set_last_error(ERROR_INVALID_HANDLE);
                return 0;
            };
            let mut buf = vec![0u8; a[2] as usize];
            let (n, pos, path) = {
                use std::io::Seek;
                let mut g = f.lock().unwrap();
                let pos = g.file.stream_position().unwrap_or(0);
                (g.file.read(&mut buf).unwrap_or(0), pos, g.path.clone())
            };
            if std::env::var_os("S4_READ_TRACE").is_some() && path.extension().map(|e| e == "bik").unwrap_or(false) {
                eprintln!("ReadFile {} pos {pos} want {} got {n} dest {:#x}", path.display(), a[2], a[1]);
            }
            c.mem.write_bytes(a[1], &buf[..n]);
            if a[3] != 0 {
                c.mem.w32(a[3], n as u32);
            }
            1
        }),
        shim!("WriteFile", 5, |c, a| {
            let data = c.mem.read_bytes(a[1], a[2] as usize);
            match handles::get(a[0]) {
                Some(Obj::File(f)) => {
                    let n = f.lock().unwrap().file.write(&data).unwrap_or(0);
                    if a[3] != 0 {
                        c.mem.w32(a[3], n as u32);
                    }
                    1
                }
                Some(Obj::Std(k)) => {
                    if k == 2 || std::env::var_os("S4_STDOUT").is_some() {
                        let _ = std::io::stderr().write_all(&data);
                    }
                    if a[3] != 0 {
                        c.mem.w32(a[3], a[2]);
                    }
                    1
                }
                _ => {
                    set_last_error(ERROR_INVALID_HANDLE);
                    0
                }
            }
        }),
        shim!("SetFilePointer", 4, |c, a| {
            let Some(Obj::File(f)) = handles::get(a[0]) else { return 0xffff_ffff };
            let mut off = a[1] as i32 as i64;
            if a[2] != 0 {
                off |= (c.mem.r32(a[2]) as i64) << 32;
            }
            let from = match a[3] {
                0 => SeekFrom::Start(off as u64),
                1 => SeekFrom::Current(off),
                _ => SeekFrom::End(off),
            };
            match f.lock().unwrap().file.seek(from) {
                Ok(p) => {
                    if a[2] != 0 {
                        c.mem.w32(a[2], (p >> 32) as u32);
                    }
                    p as u32
                }
                Err(_) => 0xffff_ffff,
            }
        }),
        shim!("GetFileSize", 2, |c, a| {
            let Some(Obj::File(f)) = handles::get(a[0]) else { return 0xffff_ffff };
            let len = f.lock().unwrap().file.metadata().map(|m| m.len()).unwrap_or(0);
            if a[1] != 0 {
                c.mem.w32(a[1], (len >> 32) as u32);
            }
            len as u32
        }),
        shim!("SetEndOfFile", 1, |_, a| {
            let Some(Obj::File(f)) = handles::get(a[0]) else { return 0 };
            let mut g = f.lock().unwrap();
            let p = g.file.stream_position().unwrap_or(0);
            g.file.set_len(p).is_ok() as u32
        }),
        shim!("FlushFileBuffers", 1, |_, _| 1),
        shim!("LockFile", 5, |_, _| 1),
        shim!("UnlockFile", 5, |_, _| 1),
        shim!("SetFileTime", 4, |_, _| 1),
        shim!("GetFileInformationByHandle", 2, |c, a| {
            let Some(Obj::File(f)) = handles::get(a[0]) else { return 0 };
            let m = f.lock().unwrap().file.metadata().unwrap();
            c.mem.fill(a[1], 0, 52);
            c.mem.w32(a[1], attributes(&m));
            let t = meta_filetime(&m);
            for off in [4, 12, 20] {
                write_filetime(c, a[1] + off, t);
            }
            c.mem.w32(a[1] + 32, (m.len() >> 32) as u32);
            c.mem.w32(a[1] + 36, m.len() as u32);
            c.mem.w32(a[1] + 40, 1);
            1
        }),
        shim!("GetFileAttributesA", 1, |c, a| file_attributes(&cstr(c, a[0]))),
        shim!("GetFileAttributesW", 1, |c, a| file_attributes(&wstr(c, a[0]))),
        shim!("SetFileAttributesA", 2, |_, _| 1),
        shim!("SetFileAttributesW", 2, |_, _| 1),
        shim!("DeleteFileA", 1, |c, a| delete_file(&cstr(c, a[0]))),
        shim!("DeleteFileW", 1, |c, a| delete_file(&wstr(c, a[0]))),
        shim!("CopyFileA", 3, |c, a| {
            let (from, to) = (fs::host_path(&cstr(c, a[0])), fs::host_path(&cstr(c, a[1])));
            if a[2] != 0 && to.exists() {
                set_last_error(80);
                return 0;
            }
            std::fs::copy(from, to).is_ok() as u32
        }),
        shim!("MoveFileA", 2, |c, a| std::fs::rename(fs::host_path(&cstr(c, a[0])), fs::host_path(&cstr(c, a[1]))).is_ok() as u32),
        shim!("MoveFileW", 2, |c, a| std::fs::rename(fs::host_path(&wstr(c, a[0])), fs::host_path(&wstr(c, a[1]))).is_ok() as u32),
        shim!("CreateDirectoryA", 2, |c, a| create_directory(&cstr(c, a[0]))),
        shim!("CreateDirectoryW", 2, |c, a| create_directory(&wstr(c, a[0]))),
        shim!("RemoveDirectoryA", 1, |c, a| std::fs::remove_dir(fs::host_path(&cstr(c, a[0]))).is_ok() as u32),
        shim!("RemoveDirectoryW", 1, |c, a| std::fs::remove_dir(fs::host_path(&wstr(c, a[0]))).is_ok() as u32),
        shim!("GetCurrentDirectoryA", 2, |c, a| {
            let d = fs::with(|f| f.cwd.clone());
            put_cstr(c, a[1], a[0], &d)
        }),
        shim!("GetCurrentDirectoryW", 2, |c, a| {
            let d = fs::with(|f| f.cwd.clone());
            put_wstr(c, a[1], a[0], &d)
        }),
        shim!("SetCurrentDirectoryA", 1, |c, a| set_cwd(&cstr(c, a[0]))),
        shim!("SetCurrentDirectoryW", 1, |c, a| set_cwd(&wstr(c, a[0]))),
        shim!("GetFullPathNameA", 4, |c, a| {
            let abs = fs::absolute(&cstr(c, a[0]));
            if a[3] != 0 {
                c.mem.w32(a[3], 0);
            }
            put_cstr(c, a[2], a[1], &abs)
        }),
        shim!("GetFullPathNameW", 4, |c, a| {
            let abs = fs::absolute(&wstr(c, a[0]));
            if a[3] != 0 {
                c.mem.w32(a[3], 0);
            }
            put_wstr(c, a[2], a[1], &abs)
        }),
        shim!("GetLogicalDrives", 0, |_, _| 4),
        shim!("GetDriveTypeA", 1, |_, _| 3),
        shim!("GetDriveTypeW", 1, |_, _| 3),
        shim!("GetDiskFreeSpaceA", 5, |c, a| {
            for (p, v) in [(a[1], 8u32), (a[2], 512), (a[3], 1 << 20), (a[4], 1 << 20)] {
                if p != 0 {
                    c.mem.w32(p, v);
                }
            }
            1
        }),
        shim!("FindFirstFileA", 2, |c, a| {
            let entries = find_files(&cstr(c, a[0]));
            first_find(c, entries, a[1], false)
        }),
        shim!("FindFirstFileW", 2, |c, a| {
            let entries = find_files(&wstr(c, a[0]));
            first_find(c, entries, a[1], true)
        }),
        shim!("FindNextFileA", 2, |c, a| next_find(c, a[0], a[1], false)),
        shim!("FindNextFileW", 2, |c, a| next_find(c, a[0], a[1], true)),
        shim!("FindClose", 1, |_, a| handles::close(a[0]) as u32),
        // (file, security, protect, size high, size low, name): INVALID_HANDLE_VALUE makes memory (shared by name).
        shim!("CreateFileMappingA", 6, |c, a| match handles::get(a[0]) {
            Some(Obj::File(f)) => handles::insert(Obj::Mapping(f)),
            _ if a[0] == 0xffff_ffff => {
                let name = if a[5] != 0 { cstr(c, a[5]) } else { String::new() };
                let mut named = ANON_NAMED.lock().unwrap();
                let map = match named.get(&name).filter(|_| !name.is_empty()) {
                    Some(m) => m.clone(),
                    None => {
                        let m = Arc::new(handles::AnonMap { size: a[4], addr: Mutex::new(0) });
                        if !name.is_empty() {
                            named.insert(name, m.clone());
                        }
                        m
                    }
                };
                handles::insert(Obj::Anon(map))
            }
            _ => 0,
        }),
        shim!("OpenFileMappingA", 3, |c, a| {
            let name = cstr(c, a[2]);
            match ANON_NAMED.lock().unwrap().get(&name) {
                Some(m) => handles::insert(Obj::Anon(m.clone())),
                None => {
                    set_last_error(ERROR_FILE_NOT_FOUND);
                    0
                }
            }
        }),
        shim!("MapViewOfFile", 5, |c, a| {
            if let Some(Obj::Anon(m)) = handles::get(a[0]) {
                let mut addr = m.addr.lock().unwrap();
                if *addr == 0 {
                    *addr = heap::vm_alloc(c, m.size.max(1));
                    ANON_VIEWS.lock().unwrap().push(*addr);
                }
                return *addr;
            }
            let Some(Obj::Mapping(f)) = handles::get(a[0]) else { return 0 };
            let mut g = f.lock().unwrap();
            let len = g.file.metadata().map(|m| m.len()).unwrap_or(0) as usize;
            let mut buf = vec![0u8; len];
            let pos = g.file.stream_position().unwrap_or(0);
            let _ = g.file.seek(SeekFrom::Start(0));
            let _ = g.file.read(&mut buf);
            let _ = g.file.seek(SeekFrom::Start(pos));
            let p = heap::vm_alloc(c, len as u32 + 1);
            c.mem.write_bytes(p, &buf);
            p
        }),
        shim!("UnmapViewOfFile", 1, |_, a| {
            // Memory mappings stay (other handles may share them).
            if !ANON_VIEWS.lock().unwrap().contains(&a[0]) {
                heap::vm_free(a[0]);
            }
            1
        }),
        shim!("CreatePipe", 4, |_, _| 0),
        shim!("PeekNamedPipe", 6, |_, _| 0),
        shim!("CreateProcessA", 10, |_, _| 0),
        shim!("CreateProcessW", 10, |_, _| 0),
        shim!("GetExitCodeProcess", 2, |_, _| 0),
    ]
}

fn ctype(u: u16) -> u16 {
    let ch = char::from_u32(u as u32).unwrap_or('\0');
    let mut t = 0;
    if ch.is_uppercase() {
        t |= 0x1;
    }
    if ch.is_lowercase() {
        t |= 0x2;
    }
    if ch.is_ascii_digit() {
        t |= 0x4;
    }
    if ch.is_whitespace() {
        t |= 0x8;
    }
    if ch.is_ascii_punctuation() {
        t |= 0x10;
    }
    if ch.is_control() {
        t |= 0x20;
    }
    if ch == ' ' {
        t |= 0x40;
    }
    if ch.is_ascii_hexdigit() {
        t |= 0x80;
    }
    if ch.is_alphabetic() {
        t |= 0x100;
    }
    t
}

fn file_attributes(name: &str) -> u32 {
    if std::env::var_os("S4_HOST_TRACE").is_some() {
        eprintln!("  GetFileAttributes {name:?} -> {}", fs::host_path(name).display());
    }
    match std::fs::metadata(fs::host_path(name)) {
        Ok(m) => attributes(&m),
        Err(_) => {
            set_last_error(ERROR_FILE_NOT_FOUND);
            0xffff_ffff
        }
    }
}

fn delete_file(name: &str) -> u32 {
    match std::fs::remove_file(fs::host_path(name)) {
        Ok(_) => 1,
        Err(_) => {
            set_last_error(ERROR_FILE_NOT_FOUND);
            0
        }
    }
}

fn create_directory(name: &str) -> u32 {
    let p = fs::host_path(name);
    if p.exists() {
        set_last_error(ERROR_ALREADY_EXISTS);
        return 0;
    }
    std::fs::create_dir(p).is_ok() as u32
}

fn set_cwd(name: &str) -> u32 {
    let abs = fs::absolute(name);
    if fs::host_path(&abs).is_dir() {
        let trimmed = abs.trim_end_matches('\\').to_string();
        fs::with(|f| f.cwd = trimmed);
        1
    } else {
        set_last_error(ERROR_PATH_NOT_FOUND);
        0
    }
}

fn first_find(c: &mut Cpu, entries: Vec<(String, std::fs::Metadata)>, data: u32, wide: bool) -> u32 {
    if entries.is_empty() {
        set_last_error(ERROR_FILE_NOT_FOUND);
        return INVALID_HANDLE;
    }
    let (n, m) = entries[0].clone();
    fill_find_data(c, data, &n, &m, wide);
    handles::insert(Obj::Find(Arc::new(Mutex::new(FindObj { entries, next: 1 }))))
}

fn next_find(c: &mut Cpu, h: u32, data: u32, wide: bool) -> u32 {
    let Some(Obj::Find(f)) = handles::get(h) else { return 0 };
    let mut g = f.lock().unwrap();
    if g.next >= g.entries.len() {
        set_last_error(ERROR_NO_MORE_FILES);
        return 0;
    }
    let (n, m) = g.entries[g.next].clone();
    g.next += 1;
    fill_find_data(c, data, &n, &m, wide);
    1
}
