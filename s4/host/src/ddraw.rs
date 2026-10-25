//! DirectDraw 7 as the original engine uses it: system-memory surfaces whose
//! pixels live in the guest address space, so the lifted code draws into them
//! and the host reads them. Objects are COM objects in guest memory (vtable
//! pointer, reference count) whose methods are shims.

use crate::{gdi, heap, register_shim, Shim};
use s4_x86::Cpu;
use std::cell::RefCell;
use std::collections::HashMap;

const DD7: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "Compact", "CreateClipper", "CreatePalette", "CreateSurface", "DuplicateSurface", "EnumDisplayModes", "EnumSurfaces", "FlipToGDISurface", "GetCaps", "GetDisplayMode",
    "GetFourCCCodes", "GetGDISurface", "GetMonitorFrequency", "GetScanLine", "GetVerticalBlankStatus", "Initialize", "RestoreDisplayMode", "SetCooperativeLevel", "SetDisplayMode", "WaitForVerticalBlank",
    "GetAvailableVidMem", "GetSurfaceFromDC", "RestoreAllSurfaces", "TestCooperativeLevel", "GetDeviceIdentifier", "StartModeTest", "EvaluateMode",
];

const IID_IDIRECTDRAW7: [u8; 16] = [0xc0, 0x5e, 0xe6, 0x15, 0x9c, 0x3b, 0xd2, 0x11, 0xb9, 0x2f, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b];
const IID_IDIRECTDRAW1: [u8; 16] = [0x80, 0xdb, 0x14, 0x6c, 0x33, 0xa7, 0xce, 0x11, 0xa5, 0x21, 0x00, 0x20, 0xaf, 0x0b, 0xe5, 0x60];
const IID_IDIRECTDRAW2: [u8; 16] = [0xe0, 0xf3, 0xa6, 0xb3, 0x43, 0x2b, 0xcf, 0x11, 0xa2, 0xde, 0x00, 0xaa, 0x00, 0xb9, 0x33, 0x56];
const IID_IDIRECTDRAW4: [u8; 16] = [0x9a, 0x50, 0x59, 0x9c, 0xbd, 0x39, 0xd1, 0x11, 0x8c, 0x4a, 0x00, 0xc0, 0x4f, 0xd9, 0x30, 0xc5];
const IID_IDIRECT3D7: [u8; 16] = [0x77, 0x9e, 0x04, 0xf5, 0x61, 0x48, 0xd2, 0x11, 0xa4, 0x07, 0x00, 0xa0, 0xc9, 0x06, 0x29, 0xa8];

const D3D7: &[&str] = &["QueryInterface", "AddRef", "Release", "EnumDevices", "CreateDevice", "CreateLight", "CreateMaterial", "CreateViewport", "FindDevice", "CreateVertexBuffer", "EnumZBufferFormats", "EvictManagedTextures"];

mod d3d;
pub use d3d::{gpu_probe, gpu_wanted};

fn d3d7_object(c: &mut Cpu) -> u32 {
    let vt = make_vtable(
        c,
        "IDirect3D7",
        D3D7,
        &[
            ("QueryInterface", 3, |c, a| {
                c.mem.w32(a[2], 0);
                0x8000_4002
            }),
            ("AddRef", 1, |c, a| add_ref(c, a[0])),
            ("Release", 1, |c, a| release(c, a[0])),
            ("EnumDevices", 3, d3d::enum_devices),
            ("CreateDevice", 4, d3d::create_device),
            ("EnumZBufferFormats", 4, d3d::enum_z_formats),
        ],
    );
    new_object(c, vt)
}

const CLIPPER: &[&str] = &["QueryInterface", "AddRef", "Release", "GetClipList", "GetHWnd", "Initialize", "IsClipListChanged", "SetClipList", "SetHWnd"];

const SURFACE7: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "AddAttachedSurface", "AddOverlayDirtyRect", "Blt", "BltBatch", "BltFast", "DeleteAttachedSurface", "EnumAttachedSurfaces", "EnumOverlayZOrders", "Flip",
    "GetAttachedSurface", "GetBltStatus", "GetCaps", "GetClipper", "GetColorKey", "GetDC", "GetFlipStatus", "GetOverlayPosition", "GetPalette", "GetPixelFormat", "GetSurfaceDesc", "Initialize", "IsLost",
    "Lock", "ReleaseDC", "Restore", "SetClipper", "SetColorKey", "SetOverlayPosition", "SetPalette", "Unlock", "UpdateOverlay", "UpdateOverlayDisplay", "UpdateOverlayZOrder", "GetDDInterface", "PageLock",
    "PageUnlock", "SetSurfaceDesc", "SetPrivateData", "GetPrivateData", "FreePrivateData", "GetUniquenessValue", "ChangeUniquenessValue", "SetPriority", "GetPriority", "SetLOD", "GetLOD",
];

#[derive(Clone, Debug)]
struct Surface {
    /// Pixel layout: 0 RGB565, 1 ARGB1555, 2 ARGB4444.
    fmt: u8,
    w: u32,
    h: u32,
    bpp: u32,
    ptr: u32,
    pitch: u32,
    caps: u32,
    back: Option<u32>,
    dc: Option<u32>,
    color_key: Option<(u32, u32)>,
}

#[derive(Default)]
struct Dd {
    surfaces: HashMap<u32, Surface>,
    display: (u32, u32, u32),
    dd_vtable: u32,
    dd1_vtable: u32,
    surface_vtable: u32,
    /// Bumped whenever a surface's pixels may have changed (for the GPU's copies of textures).
    versions: HashMap<u32, u64>,
}

/// The DirectDraw state is shared by every guest thread (a program may set the display mode on one thread and create its
/// surfaces on another).
struct DdHolder;

static DD: DdHolder = DdHolder;

impl DdHolder {
    fn with<R>(&self, f: impl FnOnce(&RefCell<Dd>) -> R) -> R {
        static CELL: std::sync::OnceLock<std::sync::Mutex<RefCell<Dd>>> = std::sync::OnceLock::new();
        let m = CELL.get_or_init(|| std::sync::Mutex::new(RefCell::new(Dd::default())));
        let g = m.lock().unwrap_or_else(|e| e.into_inner());
        f(&g)
    }
}

const DDSCAPS_BACKBUFFER: u32 = 4;
const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
const DDERR_GENERIC: u32 = 0x8000_4005;
const DDERR_UNSUPPORTED: u32 = 0x8876_0000 + 0x10;

fn rgb565_pixel_format(c: &Cpu, at: u32) {
    let m = c.mem;
    m.w32(at, 32);
    m.w32(at + 4, 0x40);
    m.w32(at + 8, 0);
    m.w32(at + 12, 16);
    m.w32(at + 16, 0xf800);
    m.w32(at + 20, 0x07e0);
    m.w32(at + 24, 0x001f);
    m.w32(at + 28, 0);
}

pub(crate) fn make_vtable(c: &mut Cpu, iface: &'static str, names: &'static [&'static str], table: &[(&str, u8, fn(&mut Cpu, &[u32]) -> u32)]) -> u32 {
    let vt = heap::host_alloc(c, 4 * names.len() as u32);
    for (i, &name) in names.iter().enumerate() {
        let addr = match table.iter().find(|(n, _, _)| *n == name) {
            Some(&(_, args, f)) => register_shim(Shim { dll: "ddraw", name, args, cdecl: false, f }),
            None => register_shim(Shim { dll: "", name: Box::leak(format!("{iface}::{name} (slot {i})").into_boxed_str()), args: 0, cdecl: false, f: |_, _| 0 }),
        };
        c.mem.w32(vt + 4 * i as u32, addr);
    }
    vt
}

pub(crate) fn new_object(c: &mut Cpu, vtable: u32) -> u32 {
    let o = heap::host_alloc(c, 16);
    c.mem.w32(o, vtable);
    c.mem.w32(o + 4, 1);
    o
}

pub(crate) fn add_ref(c: &mut Cpu, o: u32) -> u32 {
    let n = c.mem.r32(o + 4) + 1;
    c.mem.w32(o + 4, n);
    n
}

pub(crate) fn release(c: &mut Cpu, o: u32) -> u32 {
    let n = c.mem.r32(o + 4).saturating_sub(1);
    c.mem.w32(o + 4, n);
    n
}

/// `DirectDrawCreateEx(guid, void **dd, iid, outer)`.
pub fn direct_draw_create_ex(c: &mut Cpu, a: &[u32]) -> u32 {
    create(c, a[1], false)
}

/// `DirectDrawCreate(guid, IDirectDraw **dd, outer)`: the version 1 interface
/// (`SetDisplayMode` takes no refresh rate and flags).
pub fn direct_draw_create(c: &mut Cpu, a: &[u32]) -> u32 {
    create(c, a[1], true)
}

fn create(c: &mut Cpu, out: u32, v1: bool) -> u32 {
    let o = make(c, v1);
    c.mem.w32(out, o);
    0
}

fn make(c: &mut Cpu, v1: bool) -> u32 {
    let dd_vt = if v1 { DD.with(|d| d.borrow().dd1_vtable) } else { DD.with(|d| d.borrow().dd_vtable) };
    let dd_vt = if dd_vt != 0 { dd_vt } else { create_vtable(c, v1) };
    new_object(c, dd_vt)
}

/// `CoCreateInstance(CLSID_DirectDraw, .., IID_IDirectDraw2)` (initialised later with `Initialize`); `v1` for IID_IDirectDraw.
pub fn co_create_direct_draw(c: &mut Cpu, v1: bool) -> u32 {
    make(c, v1)
}

fn create_vtable(c: &mut Cpu, v1: bool) -> u32 {
    let vt = make_vtable(
            c,
            "IDirectDraw7",
            DD7,
            &[
                ("QueryInterface", 3, |c, a| {
                    let iid = c.mem.read_bytes(a[1], 16);
                    if iid == IID_IDIRECT3D7 {
                        let o = d3d7_object(c);
                        c.mem.w32(a[2], o);
                        return 0;
                    }
                    // IDirectDraw2 and 4 have the layout of version 7 as far as the methods used here go (SetDisplayMode takes
                    // the refresh rate and flags from version 2 on); version 1 has its own.
                    if iid == IID_IDIRECTDRAW2 || iid == IID_IDIRECTDRAW4 || iid == IID_IDIRECTDRAW1 {
                        let v1 = iid == IID_IDIRECTDRAW1;
                        let vt = DD.with(|d| if v1 { d.borrow().dd1_vtable } else { d.borrow().dd_vtable });
                        let vt = if vt != 0 { vt } else { create_vtable(c, v1) };
                        let o = new_object(c, vt);
                        c.mem.w32(a[2], o);
                        return 0;
                    }
                    if iid == IID_IDIRECTDRAW7 {
                        // The version 7 interface of the same object.
                        let vt = DD.with(|d| d.borrow().dd_vtable);
                        let vt = if vt != 0 { vt } else { create_vtable(c, false) };
                        let o = new_object(c, vt);
                        c.mem.w32(a[2], o);
                        return 0;
                    }
                    c.mem.w32(a[2], a[0]);
                    add_ref(c, a[0]);
                    0
                }),
                ("AddRef", 1, |c, a| add_ref(c, a[0])),
                ("Release", 1, |c, a| release(c, a[0])),
                ("SetCooperativeLevel", 3, |_, _| 0),
                ("SetDisplayMode", if v1 { 4 } else { 6 }, |_, a| {
                    DD.with(|d| d.borrow_mut().display = (a[1], a[2], a[3]));
                    crate::user32::set_screen(a[1] as i32, a[2] as i32);
                    0
                }),
                ("RestoreDisplayMode", 1, |_, _| 0),
                // (this, DDSURFACEDESC*): the current mode, as a surface description (dwSize at +0 is the caller's)
                ("GetDisplayMode", 2, |c, a| {
                    let (w, h, bpp) = DD.with(|d| d.borrow().display);
                    let (w, h, bpp) = if w == 0 { (800, 600, 16) } else { (w, h, bpp) };
                    let size = c.mem.r32(a[1]);
                    for o in (4..size.clamp(108, 124)).step_by(4) {
                        c.mem.w32(a[1] + o, 0);
                    }
                    c.mem.w32(a[1] + 4, 0x1 | 0x2 | 0x8 | 0x1000); // height, width, pitch, pixel format
                    c.mem.w32(a[1] + 8, h);
                    c.mem.w32(a[1] + 12, w);
                    c.mem.w32(a[1] + 16, w * bpp / 8);
                    rgb565_pixel_format(c, a[1] + 72);
                    0
                }),
                ("Initialize", 2, |_, _| 0),
                ("GetCaps", 3, get_caps),
                ("GetDeviceIdentifier", 3, |c, a| {
                    c.mem.fill(a[1], 0, 1088);
                    0
                }),
                ("CreateSurface", 4, create_surface),
                ("EnumDisplayModes", 5, |c, a| {
                    // (flags, filter desc, context, callback): 16-bit modes only.
                    let desc = crate::heap::alloc_zeroed(c, 128);
                    for (w, h) in [(640u32, 480u32), (800, 600), (1024, 768), (1152, 864), (1280, 720), (1280, 960), (1280, 1024), (1600, 1200), (1920, 1080)] {
                        c.mem.fill(desc, 0, 124);
                        c.mem.w32(desc, 124);
                        c.mem.w32(desc + 4, 0x2 | 0x4 | 0x8 | 0x1000 | 0x400000);
                        c.mem.w32(desc + 8, h);
                        c.mem.w32(desc + 12, w);
                        c.mem.w32(desc + 16, w * 2);
                        c.mem.w32(desc + 24, 60);
                        rgb565_pixel_format(c, desc + 72);
                        if crate::shims::guest_call(c, a[4], &[desc, a[3]]) == 0 {
                            break;
                        }
                    }
                    0
                }),
                ("CreateClipper", 4, |c, a| {
                    let vt = make_vtable(
                        c,
                        "IDirectDrawClipper",
                        CLIPPER,
                        &[
                            ("AddRef", 1, |c, a| add_ref(c, a[0])),
                            ("Release", 1, |c, a| release(c, a[0])),
                            ("SetHWnd", 3, |_, _| 0),
                            ("GetClipList", 4, |_, _| 0),
                            ("SetClipList", 3, |_, _| 0),
                            ("IsClipListChanged", 2, |_, _| 0),
                        ],
                    );
                    let o = new_object(c, vt);
                    c.mem.w32(a[2], o);
                    0
                }),
                ("GetAvailableVidMem", 4, |c, a| {
                    if a[2] != 0 {
                        c.mem.w32(a[2], 32 << 20);
                    }
                    if a[3] != 0 {
                        c.mem.w32(a[3], 32 << 20);
                    }
                    0
                }),
                // The display refreshes at 60 Hz: a program that waits for the blank paces itself at that rate.
                ("WaitForVerticalBlank", 3, |_, _| {
                    let period = 16_667u64;
                    let now = crate::shims::micros();
                    std::thread::sleep(std::time::Duration::from_micros(period - now % period));
                    0
                }),
                ("TestCooperativeLevel", 1, |_, _| 0),
                ("RestoreAllSurfaces", 1, |_, _| 0),
            ],
        );
    DD.with(|d| if v1 { d.borrow_mut().dd1_vtable = vt } else { d.borrow_mut().dd_vtable = vt });
    vt
}

fn get_caps(c: &mut Cpu, a: &[u32]) -> u32 {
    // DDCAPS (380 bytes): what a card of the time reported. The engine needs
    // blit (0x40) from system memory (0x04000000) and colour keying (0x400000),
    // and at least 4 MB of video memory.
    for p in [a[1], a[2]] {
        if p == 0 {
            continue;
        }
        let m = c.mem;
        m.fill(p, 0, 380);
        m.w32(p, 380);
        m.w32(p + 4, 0x40 | 0x0400_0000 | 0x0040_0000);
        m.w32(p + 12, 0x40 | 0x80 | 0x200 | 0x400 | 0x2000);
        m.w32(p + 0x3c, 32 << 20);
        m.w32(p + 0x40, 32 << 20);
    }
    0
}

fn create_surface(c: &mut Cpu, a: &[u32]) -> u32 {
    let (desc, out) = (a[1], a[2]);
    let m = c.mem;
    let flags = m.r32(desc + 4);
    let caps = m.r32(desc + 104);
    let (display_w, display_h, display_bpp) = DD.with(|d| d.borrow().display);
    let primary = caps & DDSCAPS_PRIMARYSURFACE != 0;
    let (w, h) = if primary { (display_w.max(1), display_h.max(1)) } else { (m.r32(desc + 12), m.r32(desc + 8)) };
    let bpp = if primary { display_bpp.max(16) } else { 16 };
    if d3d::trace() {
        let pf = (m.r32(desc + 76), m.r32(desc + 84), m.r32(desc + 88), m.r32(desc + 92), m.r32(desc + 96), m.r32(desc + 100));
        eprintln!("d3d CreateSurface flags {flags:#x} caps {caps:#x} caps2 {:#x} {w}x{h} mips {} pixfmt(flags,bits,r,g,b,a) {pf:x?}", m.r32(desc + 108), m.r32(desc + 24));
    }
    // The pixel format of a texture or off-screen surface that names one (DDSD_PIXELFORMAT).
    let fmt = if flags & 0x1000 != 0 && !primary {
        match m.r32(desc + 100) {
            0x8000 => 1,
            0xf000 => 2,
            _ => 0,
        }
    } else {
        0
    };
    let pitch = (w * bpp / 8 + 3) & !3;
    let ptr = heap::alloc(c, pitch * h + 64);
    c.mem.fill(ptr, 0, (pitch * h) as usize);
    if std::env::var_os("S4_DD_GUARD").is_some() {
        c.mem.fill(ptr + pitch * h, 0xa5, 0x40000);
    }
    let surface_vt = DD.with(|d| d.borrow().surface_vtable);
    let surface_vt = if surface_vt != 0 {
        surface_vt
    } else {
        let vt = make_vtable(c, "IDirectDrawSurface7", SURFACE7, SURFACE_METHODS);
        DD.with(|d| d.borrow_mut().surface_vtable = vt);
        vt
    };
    let obj = new_object(c, surface_vt);
    let mut s = Surface { fmt, w, h, bpp, ptr, pitch, caps, back: None, dc: None, color_key: None };
    // A flip chain: the primary surface owns its back buffer.
    if primary && flags & 0x20 != 0 {
        let back_ptr = heap::alloc(c, pitch * h + 64);
        c.mem.fill(back_ptr, 0, (pitch * h) as usize);
        let back = new_object(c, surface_vt);
        DD.with(|d| d.borrow_mut().surfaces.insert(back, Surface { fmt: 0, w, h, bpp, ptr: back_ptr, pitch, caps: DDSCAPS_BACKBUFFER, back: None, dc: None, color_key: None }));
        s.back = Some(back);
    }
    DD.with(|d| d.borrow_mut().surfaces.insert(obj, s));
    touch(obj);
    c.mem.w32(out, obj);
    0
}

static VERSION_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Notes that the pixels of a surface may have changed (a new, never used version number).
fn touch(o: u32) {
    let v = VERSION_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    DD.with(|d| d.borrow_mut().versions.insert(o, v));
}

fn version(o: u32) -> u64 {
    DD.with(|d| d.borrow().versions.get(&o).copied().unwrap_or(0))
}

fn surface(o: u32) -> Surface {
    DD.with(|d| d.borrow().surfaces.get(&o).cloned()).unwrap_or_else(|| panic!("IDirectDrawSurface7 {o:#x} is unknown"))
}

fn fill_desc(c: &mut Cpu, desc: u32, s: &Surface, with_pointer: bool) {
    let m = c.mem;
    m.w32(desc + 4, 0x1 | 0x2 | 0x4 | 0x8 | 0x1000 | if with_pointer { 0x400 } else { 0 });
    m.w32(desc + 8, s.h);
    m.w32(desc + 12, s.w);
    m.w32(desc + 16, s.pitch);
    m.w32(desc + 36, if with_pointer { s.ptr } else { 0 });
    match s.fmt {
        1 => d3d::pixel_format(c, desc + 72, 0x41, 16, [0x7c00, 0x03e0, 0x001f, 0x8000]),
        2 => d3d::pixel_format(c, desc + 72, 0x41, 16, [0x0f00, 0x00f0, 0x000f, 0xf000]),
        _ => rgb565_pixel_format(c, desc + 72),
    }
    m.w32(desc + 104, s.caps);
}

const SURFACE_METHODS: &[(&str, u8, fn(&mut Cpu, &[u32]) -> u32)] = &[
    ("QueryInterface", 3, |c, a| {
        c.mem.w32(a[2], a[0]);
        add_ref(c, a[0]);
        0
    }),
    ("AddRef", 1, |c, a| add_ref(c, a[0])),
    ("Release", 1, |c, a| release(c, a[0])),
    ("Lock", 5, |c, a| {
        d3d::sync_to_cpu(c, a[0]);
        let s = surface(a[0]);
        if dd_trace() {
            eprintln!("dd Lock {}", describe(a[0]));
        }
        fill_desc(c, a[2], &s, true);
        if a[1] != 0 {
            // A sub-rectangle: the pointer moves to its first pixel.
            let (l, t) = (c.mem.r32(a[1]), c.mem.r32(a[1] + 4));
            c.mem.w32(a[2] + 36, s.ptr + t * s.pitch + l * s.bpp / 8);
        }
        0
    }),
    ("Unlock", 2, |c, a| {
        if std::env::var_os("S4_DD_GUARD").is_some() {
            let s = surface(a[0]);
            let tail = c.mem.view(s.ptr + s.pitch * s.h, 0x40000);
            if let Some(last) = tail.iter().rposition(|&b| b != 0xa5) {
                eprintln!("guard: a write past the end of surface {:#x} ({}x{} bpp {} pitch {} caps {:#x}): {} bytes beyond", a[0], s.w, s.h, s.bpp, s.pitch, s.caps, last + 1);
            }
        }
        touch(a[0]);
        // Drawing on the primary surface through Lock is showing it (programs without a back buffer).
        if surface(a[0]).caps & DDSCAPS_PRIMARYSURFACE != 0 && surface(a[0]).back.is_none() {
            show(c, a[0]);
        }
        0
    }),
    ("GetSurfaceDesc", 2, |c, a| {
        let s = surface(a[0]);
        c.mem.w32(a[1], 124);
        fill_desc(c, a[1], &s, false);
        0
    }),
    ("GetPixelFormat", 2, |c, a| {
        match surface(a[0]).fmt {
            1 => d3d::pixel_format(c, a[1], 0x41, 16, [0x7c00, 0x03e0, 0x001f, 0x8000]),
            2 => d3d::pixel_format(c, a[1], 0x41, 16, [0x0f00, 0x00f0, 0x000f, 0xf000]),
            _ => rgb565_pixel_format(c, a[1]),
        }
        0
    }),
    ("GetCaps", 2, |c, a| {
        let s = surface(a[0]);
        c.mem.fill(a[1], 0, 16);
        c.mem.w32(a[1], s.caps);
        0
    }),
    ("IsLost", 1, |_, _| 0),
    ("Restore", 1, |_, _| 0),
    ("GetAttachedSurface", 3, |c, a| {
        let s = surface(a[0]);
        match s.back {
            Some(b) => {
                c.mem.w32(a[2], b);
                add_ref(c, b);
                0
            }
            None => DDERR_UNSUPPORTED,
        }
    }),
    ("SetClipper", 2, |_, _| 0),
    ("SetColorKey", 3, |c, a| {
        // SetColorKey(this, flags, DDCOLORKEY*): DDCKEY_SRCBLT (8) is the key Blt and BltFast test.
        if a[1] & 8 != 0 {
            let key = if a[2] != 0 { Some((c.mem.r32(a[2]), c.mem.r32(a[2] + 4))) } else { None };
            DD.with(|d| {
                if let Some(s) = d.borrow_mut().surfaces.get_mut(&a[0]) {
                    s.color_key = key;
                }
            });
        }
        0
    }),
    ("GetDC", 2, |c, a| {
        d3d::sync_to_cpu(c, a[0]);
        let s = surface(a[0]);
        if dd_trace() {
            eprintln!("dd GetDC {}", describe(a[0]));
        }
        let bmp = gdi::Bitmap { w: s.w as i32, h: s.h as i32, bpp: s.bpp, bits: s.ptr, pitch: s.pitch as i32, palette: Vec::new(), rgb565: true };
        let b = gdi::new_handle(gdi::Object::Bitmap(bmp));
        let dc = gdi::new_handle(gdi::Object::Dc { bitmap: Some(b) });
        c.mem.w32(a[1], dc);
        0
    }),
    ("ReleaseDC", 2, |_, a| {
        touch(a[0]);
        0
    }),
    ("Blt", 6, blt),
    // Blits are done when they return; nothing is flipping in the background.
    ("GetBltStatus", 2, |_, _| 0),
    ("GetFlipStatus", 2, |_, _| 0),
    ("GetColorKey", 3, |_, _| 0x8876_0000 + 0x2c3),
    ("GetClipper", 2, |_, _| 0x8876_0000 + 0x25b),
    ("GetPalette", 2, |_, _| 0x8876_0000 + 0x3e8),
    ("SetPalette", 2, |_, _| 0),
    ("PageLock", 2, |_, _| 0),
    ("PageUnlock", 2, |_, _| 0),
    ("BltFast", 6, |c, a| {
        // BltFast(this, x, y, src, srcRect, flags)
        let (dst, src) = (surface(a[0]), surface(a[3]));
        if dd_trace() {
            eprintln!("dd BltFast dst {} at ({}, {}) src {} flags {:#x}", describe(a[0]), a[1], a[2], describe(a[3]), a[5]);
        }
        let (l, t, r, b) = if a[4] != 0 { (c.mem.r32(a[4]), c.mem.r32(a[4] + 4), c.mem.r32(a[4] + 8), c.mem.r32(a[4] + 12)) } else { (0, 0, src.w, src.h) };
        if dst.caps & DDSCAPS_PRIMARYSURFACE == 0 && d3d::gpu_blt(c, a[0], (a[1], a[2]), a[3], (l, t, r - l, b - t), a[5] & 1 != 0) {
            return 0;
        }
        d3d::sync_to_cpu(c, a[0]);
        d3d::sync_to_cpu(c, a[3]);
        copy_rect(c, &dst, a[1], a[2], &src, l, t, r - l, b - t, a[5] & 1 != 0);
        touch(a[0]);
        if dst.caps & DDSCAPS_PRIMARYSURFACE != 0 {
            show(c, a[0]);
        }
        0
    }),
    ("Flip", 3, |c, a| {
        FLIPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if dd_trace() {
            eprintln!("dd Flip {}", describe(a[0]));
        }
        let primary = a[0];
        // Exchange the memory of the primary and its back buffer.
        let this = surface(a[0]);
        if let Some(b) = this.back {
            DD.with(|d| {
                let mut d = d.borrow_mut();
                let (pp, bp) = (d.surfaces[&a[0]].ptr, d.surfaces[&b].ptr);
                d.surfaces.get_mut(&a[0]).unwrap().ptr = bp;
                d.surfaces.get_mut(&b).unwrap().ptr = pp;
            });
        }
        let on_gpu = this.back.is_some_and(|b| d3d::flip(primary, b));
        if !on_gpu {
            show(c, primary);
        }
        // S4_SHOT=<path> S4_SHOT_FLIP=<n>: save the screen after the n-th flip.
        if let (Ok(path), Ok(n)) = (std::env::var("S4_SHOT"), std::env::var("S4_SHOT_FLIP")) {
            if FLIPS.load(std::sync::atomic::Ordering::Relaxed) == n.parse::<u32>().unwrap_or(0) {
                screenshot(c, a[0], &path);
            }
        }
        0
    }),
];

/// `S4_DD_TRACE=1`: logs the surface traffic (Blt, BltFast, Lock, GetDC, Flip) with sizes, caps and rectangles.
fn dd_trace() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var_os("S4_DD_TRACE").is_some())
}

fn describe(obj: u32) -> String {
    let s = surface(obj);
    format!("#{obj:x}({}x{} caps {:#x}{})", s.w, s.h, s.caps, if s.color_key.is_some() { " key" } else { "" })
}

static FLIPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// When the screen was last published (microseconds of the host clock), and the surface a later `show` skipped.
static LAST_SHOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PENDING_SHOW: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// The guest thread that skipped it (the surfaces belong to the thread that made them).
static PENDING_THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// A picture is not published more often than this (a game that blits to the screen hundreds of times a second would
/// spend a core on converting pictures nobody shows); the last one skipped is published by the next host call.
const SHOW_INTERVAL_US: u64 = 8000;

/// Publishes the picture a `show` skipped, once its turn has come (called on every host call).
pub fn flush_pending_show(c: &Cpu) {
    use std::sync::atomic::Ordering::*;
    let obj = PENDING_SHOW.load(Relaxed);
    if obj != 0 && PENDING_THREAD.load(Relaxed) == crate::kernel32::current_thread_id() && crate::shims::micros().saturating_sub(LAST_SHOW.load(Relaxed)) >= SHOW_INTERVAL_US && PENDING_SHOW.compare_exchange(obj, 0, Relaxed, Relaxed).is_ok() {
        show(c, obj);
    }
}

/// Publishes a 16-bit surface as the current screen image.
fn show(c: &Cpu, obj: u32) {
    use std::sync::atomic::Ordering::*;
    let now = crate::shims::micros();
    if now.saturating_sub(LAST_SHOW.load(Relaxed)) < SHOW_INTERVAL_US {
        PENDING_THREAD.store(crate::kernel32::current_thread_id(), Relaxed);
        PENDING_SHOW.store(obj, Relaxed);
        return;
    }
    LAST_SHOW.store(now, Relaxed);
    PENDING_SHOW.store(0, Relaxed);
    d3d::sync_to_cpu(c, obj);
    let s = surface(obj);
    if s.bpp != 16 {
        return;
    }
    let mut pixels = Vec::with_capacity((s.w * s.h) as usize);
    for y in 0..s.h {
        let row = c.mem.view(s.ptr + y * s.pitch, (s.w * 2) as usize);
        pixels.extend(row.chunks_exact(2).map(|p| {
            let v = u16::from_le_bytes([p[0], p[1]]) as u32;
            let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
            ((r << 3 | r >> 2) << 16) | ((g << 2 | g >> 4) << 8) | (b << 3 | b >> 2)
        }));
    }
    crate::present::publish(crate::present::Frame { w: s.w, h: s.h, pixels });
}

/// Writes a 16-bit surface as a PNG.
pub fn screenshot(c: &Cpu, obj: u32, path: &str) {
    d3d::sync_to_cpu(c, obj);
    let s = surface(obj);
    let mut rgba = Vec::with_capacity((s.w * s.h * 4) as usize);
    for y in 0..s.h {
        for x in 0..s.w {
            let v = c.mem.r16(s.ptr + y * s.pitch + 2 * x) as u32;
            let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
            rgba.extend_from_slice(&[(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8, 255]);
        }
    }
    let _ = std::fs::write(path, s4_formats_clean::png::encode_rgba(s.w, s.h, &rgba));
}

/// `Blt(this, destRect, src, srcRect, flags, bltFx)`.
fn blt(c: &mut Cpu, a: &[u32]) -> u32 {
    let dst = surface(a[0]);
    let m = c.mem;
    let (dl, dt, dr, db) = if a[1] != 0 { (m.r32(a[1]), m.r32(a[1] + 4), m.r32(a[1] + 8), m.r32(a[1] + 12)) } else { (0, 0, dst.w, dst.h) };
    let flags = a[4];
    if dd_trace() {
        eprintln!("dd Blt dst {} ({dl},{dt})-({dr},{db}) src {} flags {flags:#x}", describe(a[0]), if flags & 0x400 != 0 { "fill".to_string() } else { describe(a[2]) });
    }
    if flags & 0x400 != 0 {
        d3d::sync_to_cpu(c, a[0]);
        touch(a[0]);
        // DDBLT_COLORFILL: dwFillColor at bltfx + 80.
        let color = m.r32(a[5] + 80);
        let bytes = dst.bpp / 8;
        let mut row = Vec::with_capacity(((dr - dl) * bytes) as usize);
        for _ in dl..dr {
            row.extend_from_slice(&color.to_le_bytes()[..bytes as usize]);
        }
        for y in dt..db {
            m.write_bytes(dst.ptr + y * dst.pitch + dl * bytes, &row);
        }
        return 0;
    }
    let src = surface(a[2]);
    let (sl, st, sr, sb) = if a[3] != 0 { (m.r32(a[3]), m.r32(a[3] + 4), m.r32(a[3] + 8), m.r32(a[3] + 12)) } else { (0, 0, src.w, src.h) };
    assert_eq!((dr - dl, db - dt), (sr - sl, sb - st), "stretching Blt is not implemented");
    if dst.caps & DDSCAPS_PRIMARYSURFACE == 0 && d3d::gpu_blt(c, a[0], (dl, dt), a[2], (sl, st, sr - sl, sb - st), flags & 0x8000 != 0) {
        return 0;
    }
    d3d::sync_to_cpu(c, a[0]);
    d3d::sync_to_cpu(c, a[2]);
    touch(a[0]);
    copy_rect(c, &dst, dl, dt, &src, sl, st, sr - sl, sb - st, flags & 0x8000 != 0);
    if dst.caps & DDSCAPS_PRIMARYSURFACE != 0 {
        show(c, a[0]);
    }
    0
}

fn copy_rect(c: &mut Cpu, dst: &Surface, dx: u32, dy: u32, src: &Surface, sx: u32, sy: u32, w: u32, h: u32, key: bool) {
    assert_eq!(dst.bpp, src.bpp, "format conversion in Blt is not implemented");
    let m = c.mem;
    let bytes = src.bpp / 8;
    let keyed = if key { src.color_key } else { None };
    if keyed.is_none() {
        // Row by row as memmove; a surface may be blitted onto itself with the rectangles overlapping (scrolling), so the rows
        // go bottom-up when the destination is below the source.
        let row = (w * bytes) as usize;
        let rows: Box<dyn Iterator<Item = u32>> = if dst.ptr == src.ptr && dy > sy { Box::new((0..h).rev()) } else { Box::new(0..h) };
        for y in rows {
            m.copy(dst.ptr + (dy + y) * dst.pitch + dx * bytes, src.ptr + (sy + y) * src.pitch + sx * bytes, row);
        }
        return;
    }
    // Read the whole source rectangle first: with a colour key the destination is only written where the source is not the key.
    let (lo, hi) = keyed.unwrap();
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let sp = src.ptr + (sy + y) * src.pitch + (sx + x) * bytes;
            pixels.push(if bytes == 2 { m.r16(sp) as u32 } else { m.r32(sp) });
        }
    }
    for y in 0..h {
        for x in 0..w {
            let v = pixels[(y * w + x) as usize];
            if v >= lo && v <= hi {
                continue;
            }
            let dp = dst.ptr + (dy + y) * dst.pitch + (dx + x) * bytes;
            if bytes == 2 { m.w16(dp, v as u16) } else { m.w32(dp, v) }
        }
    }
}

/// The pixels of a surface object as (width, height, pitch, guest address).
pub fn surface_bits(o: u32) -> (u32, u32, u32, u32) {
    let s = surface(o);
    (s.w, s.h, s.pitch, s.ptr)
}

/// (object, width, height, bpp, caps, number of non-zero bytes) of every surface.
pub fn debug_surfaces(c: &Cpu) -> Vec<(u32, u32, u32, u32, u32, usize)> {
    DD.with(|d| {
        let d = d.borrow();
        let mut v: Vec<_> = d.surfaces.iter().map(|(&o, s)| (o, s.w, s.h, s.bpp, s.caps, c.mem.read_bytes(s.ptr, (s.pitch * s.h) as usize).iter().filter(|&&b| b != 0).count())).collect();
        v.sort();
        v
    })
}
