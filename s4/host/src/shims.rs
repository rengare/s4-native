//! The Windows and C runtime functions the original modules import.

use crate::{heap, Shim};

macro_rules! shim {
    ($dll:expr, $name:expr, $args:expr, stdcall, $f:expr) => {
        Shim { dll: $dll, name: $name, args: $args, cdecl: false, f: $f }
    };
    ($dll:expr, $name:expr, $args:expr, cdecl, $f:expr) => {
        Shim { dll: $dll, name: $name, args: $args, cdecl: true, f: $f }
    };
}

/// Resource directory RVA by module base (filled when a module loads).
static RSRC: std::sync::Mutex<Option<std::collections::HashMap<u32, u32>>> = std::sync::Mutex::new(None);

pub fn set_rsrc(base: u32, rva: u32) {
    RSRC.lock().unwrap().get_or_insert_with(Default::default).insert(base, rva);
}

pub fn rsrc_of(base: u32) -> Option<u32> {
    RSRC.lock().unwrap().as_ref().and_then(|m| m.get(&base).copied())
}

pub fn all() -> Vec<Shim> {
    // The registry shims go first so that they win over the older "no registry" stubs when the registry is on.
    let mut v = if crate::registry::enabled() { crate::registry::all() } else { Vec::new() };
    v.extend(misc());
    v.extend(crate::kernel32::all());
    v.extend(crate::user32::all());
    v.extend(crate::user32::cdecl());
    v.extend(crate::system::all());
    v.extend(crate::compat::all());
    v
}

fn misc() -> Vec<Shim> {
    vec![
        shim!("msvcrt.dll", "malloc", 1, cdecl, |c, a| heap::alloc(c, a[0])),
        shim!("msvcrt.dll", "free", 1, cdecl, |c, a| {
            heap::free(c, a[0]);
            0
        }),
        shim!("msvcrt.dll", "??2@YAPAXI@Z", 1, cdecl, |c, a| heap::alloc(c, a[0])),
        shim!("user32.dll", "IsIconic", 1, stdcall, |_, _| 0),
        shim!("msvcrt.dll", "_initterm", 2, cdecl, |c, a| {
            // Calls every function pointer in [a[0], a[1]).
            let mut p = a[0];
            while p < a[1] {
                let f = c.mem.r32(p);
                if f != 0 {
                    guest_call(c, f, &[]);
                }
                p += 4;
            }
            0
        }),
        shim!("msvcrt.dll", "_onexit", 1, cdecl, |_, a| a[0]),
        shim!("msvcrt.dll", "__dllonexit", 3, cdecl, |_, a| a[0]),
        shim!("msvcrt.dll", "__CxxFrameHandler", 0, cdecl, |_, _| panic!("C++ exception handling is not implemented")),
        shim!("msvcrt.dll", "srand", 1, cdecl, |_, a| {
            RAND.with(|r| r.set(a[0]));
            0
        }),
        shim!("msvcrt.dll", "rand", 0, cdecl, |_, _| {
            RAND.with(|r| {
                let v = r.get().wrapping_mul(214013).wrapping_add(2531011);
                r.set(v);
                (v >> 16) & 0x7fff
            })
        }),
        shim!("msvcrt.dll", "_controlfp", 2, cdecl, |c, a| control87(c, a[0], a[1])),
        shim!("msvcrt.dll", "_control87", 2, cdecl, |c, a| control87(c, a[0], a[1])),
        shim!("bb_support.dll", "?BBSupportTracePrintF@@YAXIPBDZZ", 0, cdecl, |_, _| 0),
        shim!("ddraw.dll", "DirectDrawCreateEx", 4, stdcall, crate::ddraw::direct_draw_create_ex),
        shim!("user32.dll", "LoadBitmapA", 2, stdcall, |c, a| {
            let (inst, id) = (a[0], a[1]);
            let rsrc = rsrc_of(inst);
            let Some((rva, _)) = rsrc.and_then(|rva| crate::pe::resource(c.mem, inst, rva, 2, id).map(|r| (r.0, r.1))) else { return 0 };
            let b = crate::gdi::dib_bitmap(c, rva);
            crate::gdi::new_handle(crate::gdi::Object::Bitmap(b))
        }),
        shim!("gdi32.dll", "CreateCompatibleDC", 1, stdcall, |_, _| crate::gdi::new_handle(crate::gdi::Object::Dc { bitmap: None })),
        shim!("gdi32.dll", "SelectObject", 2, stdcall, |_, a| {
            crate::gdi::GDI.with(|g| {
                let mut g = g.borrow_mut();
                let is_bitmap = matches!(g.objects.get(&a[1]), Some(crate::gdi::Object::Bitmap(_)));
                let is_font = matches!(g.objects.get(&a[1]), Some(crate::gdi::Object::Font { .. }));
                if is_font {
                    let st = g.states.entry(a[0]).or_default();
                    return std::mem::replace(&mut st.font, a[1]).max(0x100);
                }
                match g.objects.get_mut(&a[0]) {
                    Some(crate::gdi::Object::Dc { bitmap: slot }) if is_bitmap => slot.replace(a[1]).unwrap_or(0x100),
                    _ => 0x100,
                }
            })
        }),
        shim!("gdi32.dll", "DeleteObject", 1, stdcall, |_, a| {
            crate::gdi::GDI.with(|g| g.borrow_mut().objects.remove(&a[0]));
            1
        }),
        shim!("gdi32.dll", "DeleteDC", 1, stdcall, |_, a| {
            crate::gdi::GDI.with(|g| g.borrow_mut().objects.remove(&a[0]));
            1
        }),
        shim!("gdi32.dll", "GetPixel", 3, stdcall, |c, a| {
            let bmp = crate::gdi::GDI.with(|g| {
                let g = g.borrow();
                match g.objects.get(&a[0]) {
                    Some(crate::gdi::Object::Dc { bitmap: Some(b) }) => match g.objects.get(b) {
                        Some(crate::gdi::Object::Bitmap(bm)) => Some(bm.clone()),
                        _ => None,
                    },
                    _ => None,
                }
            });
            match bmp {
                Some(b) => crate::gdi::pixel(c, &b, a[1] as i32, a[2] as i32),
                None => 0xffff_ffff,
            }
        }),
        shim!("user32.dll", "GetDC", 1, stdcall, |_, _| crate::gdi::new_handle(crate::gdi::Object::Dc { bitmap: None })),
        shim!("user32.dll", "ReleaseDC", 2, stdcall, |_, _| 1),
        shim!("user32.dll", "ShowCursor", 1, stdcall, |_, a| crate::cursor::show(a[0] != 0) as u32),
        shim!("user32.dll", "ClipCursor", 1, stdcall, |_, _| 1),
        shim!("user32.dll", "LoadCursorA", 2, stdcall, |c, a| {
            let rsrc = rsrc_of(a[0]);
            crate::cursor::load(c, a[0], rsrc, a[1])
        }),
        shim!("user32.dll", "SetCursor", 1, stdcall, |_, a| crate::cursor::set(a[0])),
        shim!("user32.dll", "SetClassLongA", 3, stdcall, |_, _| 0),
        shim!("ddraw.dll", "DirectDrawCreate", 3, stdcall, |c, a| crate::ddraw::direct_draw_create(c, &[a[0], a[1], a[2]])),
    ]
}

/// Names of imported variables and their initial value (`_adjust_fdiv`).
pub fn data() -> &'static [(&'static str, &'static str, u32)] {
    &[("msvcrt.dll", "_adjust_fdiv", 0)]
}

thread_local! {
    static RAND: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
}

/// Calls a guest function from a shim.
pub fn guest_call(c: &mut s4_x86::Cpu, addr: u32, args: &[u32]) -> u32 {
    let esp0 = c.esp;
    let m = c.mem;
    for &a in args.iter().rev() {
        c.esp -= 4;
        m.w32(c.esp, a);
    }
    c.esp -= 4;
    m.w32(c.esp, 0x7e57_0001);
    c.call(addr);
    c.esp = esp0;
    c.eax
}

// The C runtime's floating point control word, in its own bit layout.
fn hw_to_c(cw: u16) -> u32 {
    let cw = cw as u32;
    let mut r = 0;
    for (hw, c) in [(0x1, 0x10), (0x2, 0x80000), (0x4, 8), (0x8, 4), (0x10, 2), (0x20, 1)] {
        if cw & hw != 0 {
            r |= c;
        }
    }
    r |= match cw >> 10 & 3 { 0 => 0, 1 => 0x100, 2 => 0x200, _ => 0x300 };
    r |= match cw >> 8 & 3 { 0 => 0x20000, 2 => 0x10000, _ => 0 };
    if cw & 0x1000 != 0 {
        r |= 0x40000;
    }
    r
}

fn c_to_hw(v: u32, old: u16) -> u16 {
    let mut cw = old & !(0x3f | 0xc00 | 0x300 | 0x1000);
    for (hw, c) in [(0x1, 0x10), (0x2, 0x80000), (0x4, 8), (0x8, 4), (0x10, 2), (0x20, 1)] {
        if v & c != 0 {
            cw |= hw;
        }
    }
    cw |= match v & 0x300 { 0 => 0, 0x100 => 0x400, 0x200 => 0x800, _ => 0xc00 };
    cw |= match v & 0x30000 { 0x20000 => 0, 0x10000 => 0x200, _ => 0x300 };
    if v & 0x40000 != 0 {
        cw |= 0x1000;
    }
    cw
}

fn control87(c: &mut s4_x86::Cpu, new: u32, mask: u32) -> u32 {
    let cur = hw_to_c(c.fpu.cw);
    let merged = (new & mask) | (cur & !mask);
    c.fpu.cw = c_to_hw(merged, c.fpu.cw);
    hw_to_c(c.fpu.cw)
}

/// Microseconds since start (QueryPerformanceCounter).
pub fn micros() -> u64 {
    start().elapsed().as_micros() as u64
}

/// One start instant for every thread (guest threads compare timestamps with the main thread's).
fn start() -> std::time::Instant {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *START.get_or_init(std::time::Instant::now)
}

thread_local! {
    static CLOCK: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// Fixes the millisecond clock (for replays and tests); `None` is the real time.
pub fn set_clock(ms: Option<u32>) {
    CLOCK.with(|c| c.set(ms));
}

/// Milliseconds since start.
pub fn tick() -> u32 {
    if let Some(v) = CLOCK.with(|c| c.get()) {
        return v;
    }
    start().elapsed().as_millis() as u32
}
