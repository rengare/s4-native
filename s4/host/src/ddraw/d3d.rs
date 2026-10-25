//! Direct3D 7 as the engine's hardware renderer uses it, drawn with `s4-gpu` (wgpu).
//!
//! What the engine does (found with `S4_D3D_TRACE=1`): it creates devices on its render surfaces, sets
//! up a few render states and texture-stage states, uploads textures with `Load`, and per scene calls
//! `BeginScene`, `SetViewport`, many `SetTexture` + `DrawPrimitive` (triangle lists of pre-transformed,
//! lit, textured vertices) and `EndScene`. The render target is an ordinary DirectDraw surface in guest
//! memory that the software code keeps drawing on, so a scene starts from the surface's pixels and
//! writes its result back.
//!
//! The device is on unless `S4_D3D=0` (or there is no usable graphics adapter): then `EnumDevices` finds none and
//! the engine uses its software renderer; `S4_D3D_TRACE=1` prints the calls; `S4_D3D_SSAA=2` draws at twice the size and averages.

use super::*;
use s4_gpu::{DrawState, RemoteGpu, TargetId, TextureId, Vertex};

pub(super) fn enabled() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| gpu_wanted())
}

/// False when `S4_D3D=0` asks for the software renderer.
pub fn gpu_wanted() -> bool {
    std::env::var("S4_D3D").map_or(true, |v| v != "0")
}

/// The graphics adapter the hardware renderer would use, or why there is none.
pub fn gpu_probe() -> Result<String, String> {
    s4_gpu::probe()
}

pub(super) fn trace() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var_os("S4_D3D_TRACE").is_some())
}

/// `S4_D3D_STATS=1`: where the time goes (printed once a second): begin, draw, textures, end, load.
mod stats {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::time::Instant;

    pub static NS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
    pub static N: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
    static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

    pub fn enabled() -> bool {
        static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *E.get_or_init(|| std::env::var_os("S4_D3D_STATS").is_some())
    }

    pub struct Timer(usize, Option<Instant>);

    pub fn time(slot: usize) -> Timer {
        Timer(slot, enabled().then(Instant::now))
    }

    impl Drop for Timer {
        fn drop(&mut self) {
            if let Some(t) = self.1 {
                NS[self.0].fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
                N[self.0].fetch_add(1, Relaxed);
                let mut last = LAST.lock().unwrap();
                let l = last.get_or_insert_with(Instant::now);
                if l.elapsed().as_secs_f32() >= 1.0 {
                    *l = Instant::now();
                    let names = ["begin", "draw", "texture", "end", "load", "sync/upload", "blt", "other"];
                    let s: Vec<String> = (0..8).map(|i| format!("{} {}x {:.1}ms", names[i], N[i].swap(0, Relaxed), NS[i].swap(0, Relaxed) as f64 / 1e6)).collect();
                    eprintln!(
                        "d3d time/s: {} | passes {}, texture flushes {} | texture uploads: {} load-rects ({} KB), {} diffs ({} KB)",
                        s.join(", "),
                        s4_gpu::PASSES.swap(0, Relaxed),
                        s4_gpu::TEXTURE_FLUSHES.swap(0, Relaxed),
                        super::UP[0].swap(0, Relaxed),
                        super::UP[1].swap(0, Relaxed) / 1024,
                        super::UP[2].swap(0, Relaxed),
                        super::UP[3].swap(0, Relaxed) / 1024
                    );
                }
            }
        }
    }
}

fn ssaa() -> u32 {
    static E: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var("S4_D3D_SSAA").ok().and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 4))
}

const IID_HAL_DEVICE: [u8; 16] = [0xe0, 0x3d, 0xe6, 0x84, 0xaa, 0x46, 0xcf, 0x11, 0x81, 0x6f, 0x00, 0x00, 0xc0, 0x20, 0x15, 0x6e];

const D3DDEVICE7: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "GetCaps", "EnumTextureFormats", "BeginScene", "EndScene", "GetDirect3D", "SetRenderTarget", "GetRenderTarget", "Clear", "SetTransform", "GetTransform",
    "SetViewport", "MultiplyTransform", "GetViewport", "SetMaterial", "GetMaterial", "SetLight", "GetLight", "SetRenderState", "GetRenderState", "BeginStateBlock", "EndStateBlock", "PreLoad",
    "DrawPrimitive", "DrawIndexedPrimitive", "SetClipStatus", "GetClipStatus", "DrawPrimitiveStrided", "DrawIndexedPrimitiveStrided", "DrawPrimitiveVB", "DrawIndexedPrimitiveVB",
    "ComputeSphereVisibility", "GetTexture", "SetTexture", "GetTextureStageState", "SetTextureStageState", "ValidateDevice", "ApplyStateBlock", "CaptureStateBlock", "DeleteStateBlock",
    "CreateStateBlock", "Load", "LightEnable", "GetLightEnable", "SetClipPlane", "GetClipPlane", "GetInfo",
];

macro_rules! logged {
    ($name:literal, $argc:expr) => {
        ($name, $argc, |_c: &mut Cpu, a: &[u32]| {
            if trace() {
                eprintln!("d3d {}({:x?})", $name, &a[1..]);
            }
            0
        })
    };
}

/// Texture upload statistics: (Load rectangles, their bytes, full-surface diffs, their bytes).
static UP: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

pub(super) fn pixel_format(c: &Cpu, at: u32, flags: u32, bits: u32, masks: [u32; 4]) {
    let m = c.mem;
    m.w32(at, 32);
    m.w32(at + 4, flags);
    m.w32(at + 8, 0);
    m.w32(at + 12, bits);
    for (i, k) in masks.iter().enumerate() {
        m.w32(at + 16 + 4 * i as u32, *k);
    }
}

/// A device description with generous capabilities (`D3DDEVICEDESC7`, 236 bytes).
fn device_desc(c: &Cpu, at: u32) {
    let m = c.mem;
    m.fill(at, 0, 236);
    m.w32(at, 0xffff_ffff); // dwDevCaps
    for prim in [at + 4, at + 60] {
        m.w32(prim, 56);
        for k in 1..12 {
            m.w32(prim + 4 * k, 0xffff_ffff);
        }
    }
    m.w32(at + 116, 0x500); // render depths 16 and 32
    m.w32(at + 120, 0x400); // z depth 16
    m.w32(at + 124, 1);
    m.w32(at + 128, 1);
    m.w32(at + 132, 4096);
    m.w32(at + 136, 4096);
    m.w32(at + 140, 32768);
    m.w32(at + 144, 4096);
    m.w32(at + 148, 1);
    m.w32(at + 176, 0x8); // FVF caps: one texture coordinate set
    m.w32(at + 180, 0xffff_ffff);
    m.w32(at + 184, 8);
    m.w32(at + 186, 8);
    m.w32(at + 188, 8);
    m.write_bytes(at + 196, &IID_HAL_DEVICE);
}

struct Device {
    surface: u32,
    blend: bool,
    specular: bool,
    linear: bool,
    /// The surface set as texture 0 (0: none).
    texture: u32,
    clip: Option<(u32, u32, u32, u32)>,
    /// Something was drawn in the scene in progress.
    drew: bool,
    /// Frames drawn (for statistics).
    scenes: u64,
    /// (draw calls, vertices, blended draws, distinct textures) of the scene in progress.
    stats: (u32, u32, u32, std::collections::HashSet<u32>),
}

enum Backend {
    Untried,
    Failed,
    Ready(Box<RemoteGpu>),
}

thread_local! {
    static GPU: RefCell<Backend> = const { RefCell::new(Backend::Untried) };
    static DEVICES: RefCell<HashMap<u32, Device>> = RefCell::new(HashMap::new());
    /// Guest texture surface -> its GPU copy.
    static TEXTURES: RefCell<HashMap<u32, GpuTexture>> = RefCell::new(HashMap::new());
    /// Guest surface -> the copy of its picture on the GPU.
    static RES: RefCell<HashMap<u32, Resident>> = RefCell::new(HashMap::new());
    /// Surfaces that are only sources of blits: (GPU texture, surface version, colour key used) per guest surface.
    static BLIT_SOURCES: RefCell<HashMap<u32, (TextureId, u64, Option<(u32, u32)>, (u32, u32))>> = RefCell::new(HashMap::new());
}

struct GpuTexture {
    id: TextureId,
    /// The surface version the GPU copy holds.
    version: u64,
    size: (u32, u32),
    /// The surface's bytes (rows packed) as the GPU copy has them, to send only the rows that changed.
    shadow: Vec<u8>,
    /// The guest's memory of the surface is older than `shadow` (a `Load` went to the GPU copy only).
    stale: bool,
}

/// A guest surface whose picture lives on the GPU (a render target of a device, or the target of blits). The guest's memory holds
/// the picture too, but it is only brought up to date when the guest looks at it (`sync_to_cpu`).
struct Resident {
    target: TargetId,
    /// The surface's bytes (rows packed, `w * 2` each) as the GPU's picture last saw them; empty until the first upload.
    shadow: Vec<u8>,
    /// The surface version the GPU's picture has taken up the guest's own changes of.
    synced: u64,
    /// The GPU's picture is newer than the guest's memory.
    gpu_ahead: bool,
}

/// Runs `f` with the GPU, opening it on first use. `None` when there is no usable graphics device.
fn with_gpu<R>(f: impl FnOnce(&mut RemoteGpu) -> R) -> Option<R> {
    GPU.with(|g| {
        let mut g = g.borrow_mut();
        if let Backend::Untried = *g {
            *g = match RemoteGpu::new() {
                Ok(gpu) => {
                    eprintln!("Direct3D 7 hardware renderer on {} (wgpu)", gpu.adapter_name);
                    Backend::Ready(Box::new(gpu))
                }
                Err(e) => {
                    eprintln!("Direct3D 7 device not available ({e}): the engine uses its software renderer");
                    Backend::Failed
                }
            };
        }
        match &mut *g {
            Backend::Ready(gpu) => Some(f(gpu)),
            _ => None,
        }
    })
}

fn available() -> bool {
    enabled() && with_gpu(|_| ()).is_some()
}

/// RGBA8 pixels of rows `y0..y1` of a 16-bit surface.
fn convert_rows(c: &Cpu, s: &Surface, y0: u32, y1: u32) -> Vec<u8> {
    convert_rect(c, s, 0, y0, s.w, y1 - y0)
}

/// RGBA8 pixels of a rectangle of a 16-bit surface.
fn convert_rect(c: &Cpu, s: &Surface, x: u32, y0: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (h * w * 4) as usize];
    for (y, dst) in (y0..y0 + h).zip(out.chunks_exact_mut((w * 4) as usize)) {
        s4_gpu::convert_row(s.fmt as u32, c.mem.view(s.ptr + y * s.pitch + x * 2, (w * 2) as usize), dst);
    }
    out
}

/// The 16-bit pixels of a rectangle of a surface, rows packed (`w * 2` bytes each).
fn pack_rect(c: &Cpu, s: &Surface, x: u32, y0: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((h * w * 2) as usize);
    for y in y0..y0 + h {
        out.extend_from_slice(c.mem.view(s.ptr + y * s.pitch + x * 2, (w * 2) as usize));
    }
    out
}

/// RGBA8 pixels of a 16-bit surface.
fn to_rgba(c: &Cpu, s: &Surface) -> Vec<u8> {
    convert_rows(c, s, 0, s.h)
}

/// Writes a region of RGBA8 pixels into a 565 surface.
fn write_region(c: &Cpu, s: &Surface, r: &s4_gpu::Region) {
    let (rx, ry, rw, rh) = r.rect;
    let mut row = vec![0u8; (rw * 2) as usize];
    for y in 0..rh {
        let src = &r.rgba[(y * rw * 4) as usize..((y + 1) * rw * 4) as usize];
        for (d, p) in row.chunks_exact_mut(2).zip(src.chunks_exact(4)) {
            let v = ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3);
            d.copy_from_slice(&v.to_le_bytes());
        }
        c.mem.write_bytes(s.ptr + (ry + y) * s.pitch + rx * 2, &row);
    }
}

/// Sends the rows of a rectangle (x, y, w, h) of a texture that differ from `shadow` (what the GPU copy holds; the rows are
/// updated) as bands, small gaps included (one upload is cheaper than two). `row(r)` is the new content of the rectangle's
/// row `r` (`w * 2` bytes). `force` sends everything. `stat` is the index in [`UP`] of the upload counters.
fn upload_bands<'a>(id: TextureId, fmt: u32, shadow: &mut [u8], row_bytes: usize, rect: (u32, u32, u32, u32), force: bool, stat: usize, row: impl Fn(u32) -> &'a [u8]) {
    let (x, y, w, h) = rect;
    let at = |r: u32| (y + r) as usize * row_bytes + x as usize * 2;
    let len = w as usize * 2;
    let changed = |r: u32, shadow: &[u8]| force || shadow[at(r)..at(r) + len] != *row(r);
    let mut r = 0;
    while r < h {
        if !changed(r, shadow) {
            r += 1;
            continue;
        }
        let (r0, mut r1, mut quiet) = (r, r + 1, 0);
        while r1 + quiet < h && quiet < 8 {
            if changed(r1 + quiet, shadow) {
                r1 += quiet + 1;
                quiet = 0;
            } else {
                quiet += 1;
            }
        }
        let mut data = Vec::with_capacity(len * (r1 - r0) as usize);
        for rr in r0..r1 {
            let bytes = row(rr);
            data.extend_from_slice(bytes);
            shadow[at(rr)..at(rr) + len].copy_from_slice(bytes);
        }
        UP[stat].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        UP[stat + 1].fetch_add(data.len() as u64 * 2, std::sync::atomic::Ordering::Relaxed);
        with_gpu(|g| g.write_texture_region16(id, (x, y + r0, w, r1 - r0), fmt, data));
        r = r1;
    }
}

/// The GPU copy of a texture surface, brought up to date (only the rows that changed since the last time are sent).
fn texture_for(c: &Cpu, obj: u32) -> Option<TextureId> {
    sync_resident(c, obj);
    let ver = version(obj);
    if let Some(id) = TEXTURES.with(|t| t.borrow().get(&obj).filter(|t| t.version == ver).map(|t| t.id)) {
        return Some(id);
    }
    let s = DD.with(|d| d.borrow().surfaces.get(&obj).cloned())?;
    let _t = stats::time(2);
    let row_bytes = (s.w * 2) as usize;
    let mut old = TEXTURES.with(|t| t.borrow_mut().remove(&obj));
    if old.as_ref().is_some_and(|o| o.size != (s.w, s.h)) {
        let o = old.take().unwrap();
        with_gpu(|g| g.destroy_texture(o.id));
    }
    let fresh = old.is_none();
    let (id, mut shadow) = match old {
        Some(o) => (o.id, o.shadow),
        None => (with_gpu(|g| g.create_texture(s.w, s.h))?, vec![0u8; row_bytes * s.h as usize]),
    };
    upload_bands(id, s.fmt as u32, &mut shadow, row_bytes, (0, 0, s.w, s.h), fresh, 2, |r| c.mem.view(s.ptr + r * s.pitch, row_bytes));
    TEXTURES.with(|t| t.borrow_mut().insert(obj, GpuTexture { id, version: ver, size: (s.w, s.h), shadow, stale: false }));
    Some(id)
}

/// `Load` of a rectangle of `src` into a texture the GPU already has a current copy of: the pixels go to the GPU copy only, and
/// the guest's memory of the texture is brought up to date when the guest looks at it (`sync_to_cpu`). False: not applicable.
fn load_direct(c: &Cpu, dst_obj: u32, dst: &Surface, dx: u32, dy: u32, src_obj: u32, src: &Surface, l: u32, t: u32, w: u32, h: u32) -> bool {
    if dst.bpp != 16 || src.bpp != 16 || dst.fmt != src.fmt || w == 0 || h == 0 || l + w > src.w || t + h > src.h || dx + w > dst.w || dy + h > dst.h {
        return false;
    }
    let Some((gv, size)) = TEXTURES.with(|t| t.borrow().get(&dst_obj).map(|g| (g.version, g.size))) else { return false };
    if gv != version(dst_obj) || size != (dst.w, dst.h) {
        return false;
    }
    sync_to_cpu(c, src_obj);
    TEXTURES.with(|tex| {
        let mut tex = tex.borrow_mut();
        let g = tex.get_mut(&dst_obj).unwrap();
        upload_bands(g.id, dst.fmt as u32, &mut g.shadow, (dst.w * 2) as usize, (dx, dy, w, h), false, 0, |r| c.mem.view(src.ptr + (t + r) * src.pitch + l * 2, (w * 2) as usize));
        g.stale = true;
    });
    touch(dst_obj);
    let v = version(dst_obj);
    TEXTURES.with(|t| {
        if let Some(g) = t.borrow_mut().get_mut(&dst_obj) {
            g.version = v;
        }
    });
    true
}

fn with_device<R>(obj: u32, f: impl FnOnce(&mut Device) -> R) -> Option<R> {
    DEVICES.with(|d| d.borrow_mut().get_mut(&obj).map(f))
}

pub(super) fn enum_devices(c: &mut Cpu, a: &[u32]) -> u32 {
    // With S4_D3D=0 (or without a graphics device) there is no Direct3D device and the engine uses its software path.
    if !available() {
        return 0;
    }
    let desc = crate::heap::alloc_zeroed(c, 236);
    device_desc(c, desc);
    let name = crate::heap::alloc_zeroed(c, 32);
    c.mem.write_bytes(name, b"Direct3D HAL\0");
    if trace() {
        eprintln!("d3d EnumDevices -> HAL");
    }
    crate::shims::guest_call(c, a[1], &[name, name, desc, a[2]]);
    0
}

pub(super) fn enum_z_formats(c: &mut Cpu, a: &[u32]) -> u32 {
    if available() {
        let pf = crate::heap::alloc_zeroed(c, 32);
        pixel_format(c, pf, 0x400, 16, [0xffff, 0, 0, 0]);
        crate::shims::guest_call(c, a[2], &[pf, a[3]]);
    }
    0
}

pub(super) fn create_device(c: &mut Cpu, a: &[u32]) -> u32 {
    if !available() {
        return 0x8876_086c; // D3DERR_INVALIDCALL: no hardware device.
    }
    let Some(s) = DD.with(|d| d.borrow().surfaces.get(&a[2]).cloned()) else { return 0x8876_086c };
    let dev = device_object(c);
    DEVICES.with(|d| d.borrow_mut().insert(dev, Device { surface: a[2], blend: false, specular: false, linear: false, texture: 0, clip: None, drew: false, scenes: 0, stats: Default::default() }));
    c.mem.w32(a[3], dev);
    if trace() {
        eprintln!("d3d CreateDevice(surface {:#x} {}x{}) -> {:#x}", a[2], s.w, s.h, dev);
    }
    0
}

/// True when the surface is drawn on by a device or by blits, i.e. its picture belongs on the GPU.
fn wants_gpu(obj: u32) -> bool {
    RES.with(|r| r.borrow().contains_key(&obj)) || DEVICES.with(|d| d.borrow().values().any(|d| d.surface == obj))
}

/// The surface's GPU target, made when first needed.
fn ensure_resident(obj: u32) -> Option<TargetId> {
    if let Some(t) = RES.with(|r| r.borrow().get(&obj).map(|r| r.target)) {
        return Some(t);
    }
    let s = DD.with(|d| d.borrow().surfaces.get(&obj).cloned())?;
    let target = with_gpu(|g| g.create_target(s.w, s.h, 1))?;
    RES.with(|r| r.borrow_mut().insert(obj, Resident { target, shadow: Vec::new(), synced: 0, gpu_ahead: false }));
    Some(target)
}

/// Brings the guest's own changes of the surface (its software layers draw on it) up to the GPU's picture. Only rows that differ
/// from what the GPU last saw are converted and sent.
fn upload_cpu_changes(c: &Cpu, obj: u32) {
    let Some(s) = DD.with(|d| d.borrow().surfaces.get(&obj).cloned()) else { return };
    let ver = version(obj);
    let Some((target, mut shadow, synced)) = RES.with(|r| r.borrow_mut().get_mut(&obj).map(|r| (r.target, std::mem::take(&mut r.shadow), r.synced))) else { return };
    let row_bytes = (s.w * 2) as usize;
    let fresh = shadow.len() != row_bytes * s.h as usize;
    if !fresh && synced == ver && !verify() {
        RES.with(|r| r.borrow_mut().get_mut(&obj).unwrap().shadow = shadow);
        return;
    }
    let _t = stats::time(5);
    if fresh {
        shadow = vec![0u8; row_bytes * s.h as usize];
    }
    let changed = |y: u32, shadow: &[u8]| fresh || c.mem.view(s.ptr + y * s.pitch, row_bytes) != &shadow[y as usize * row_bytes..(y as usize + 1) * row_bytes];
    let mut y = 0;
    while y < s.h {
        if !changed(y, &shadow) {
            y += 1;
            continue;
        }
        // A band of changed rows (small gaps are included: one upload is cheaper than two).
        let (y0, mut y1, mut quiet) = (y, y + 1, 0);
        while y1 + quiet < s.h && quiet < 16 {
            if changed(y1 + quiet, &shadow) {
                y1 += quiet + 1;
                quiet = 0;
            } else {
                quiet += 1;
            }
        }
        if verify() && !fresh && synced == ver {
            eprintln!("d3d verify: surface {obj:#x} rows {y0}..{y1} changed without a Lock/Unlock or Blt");
        }
        let rgba = convert_rows(c, &s, y0, y1);
        with_gpu(|g| g.write_target(target, (0, y0, s.w, y1 - y0), rgba));
        for yy in y0..y1 {
            shadow[yy as usize * row_bytes..(yy as usize + 1) * row_bytes].copy_from_slice(c.mem.view(s.ptr + yy * s.pitch, row_bytes));
        }
        y = y1;
    }
    RES.with(|r| {
        let mut r = r.borrow_mut();
        let e = r.get_mut(&obj).unwrap();
        e.shadow = shadow;
        e.synced = ver;
    });
}

/// `S4_D3D_VERIFY=1`: always compares the surface with the GPU's copy and reports changes made without a version bump.
fn verify() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var_os("S4_D3D_VERIFY").is_some())
}

/// Makes the guest's memory of the surface hold what the GPU drew (call before the guest reads or writes the surface).
pub(super) fn sync_to_cpu(c: &Cpu, obj: u32) {
    // A texture that `Load` filled on the GPU side only: its pixels are in `shadow`.
    let stale = TEXTURES.with(|t| {
        let mut t = t.borrow_mut();
        let g = t.get_mut(&obj).filter(|g| g.stale)?;
        g.stale = false;
        Some((g.size, std::mem::take(&mut g.shadow)))
    });
    if let Some(((w, _), shadow)) = stale {
        if let Some(s) = DD.with(|d| d.borrow().surfaces.get(&obj).cloned()) {
            for (y, row) in shadow.chunks_exact((w * 2) as usize).enumerate() {
                c.mem.write_bytes(s.ptr + y as u32 * s.pitch, row);
            }
        }
        TEXTURES.with(|t| {
            if let Some(g) = t.borrow_mut().get_mut(&obj) {
                g.shadow = shadow;
            }
        });
    }
    sync_resident(c, obj);
}

/// Makes the guest's memory of a surface that lives on the GPU hold what the GPU drew.
fn sync_resident(c: &Cpu, obj: u32) {
    let Some(target) = RES.with(|r| r.borrow().get(&obj).filter(|r| r.gpu_ahead).map(|r| r.target)) else { return };
    let Some(s) = DD.with(|d| d.borrow().surfaces.get(&obj).cloned()) else { return };
    let _t = stats::time(5);
    let (tx, rx) = std::sync::mpsc::channel();
    let sink: s4_gpu::PresentSink = std::sync::Arc::new(move |w, h, px| {
        let _ = tx.send((w, h, px));
    });
    with_gpu(|g| g.present_async(target, &sink));
    let Ok((w, h, px)) = rx.recv_timeout(std::time::Duration::from_secs(10)) else { return };
    if trace() {
        eprintln!("d3d sync_to_cpu {obj:#x}");
    }
    // The pixels are 5-6-5 expanded; back to the surface's words.
    let mut row = vec![0u8; (w * 2) as usize];
    for y in 0..h {
        for (d, p) in row.chunks_exact_mut(2).zip(&px[(y * w) as usize..((y + 1) * w) as usize]) {
            let v = (((p >> 16) & 0xff) >> 3) << 11 | (((p >> 8) & 0xff) >> 2) << 5 | ((p & 0xff) >> 3);
            d.copy_from_slice(&(v as u16).to_le_bytes());
        }
        c.mem.write_bytes(s.ptr + y * s.pitch, &row);
    }
    touch(obj);
    let ver = version(obj);
    RES.with(|r| {
        if let Some(e) = r.borrow_mut().get_mut(&obj) {
            e.gpu_ahead = false;
            e.synced = ver;
            e.shadow.clear();
            for y in 0..h {
                e.shadow.extend_from_slice(c.mem.view(s.ptr + y * s.pitch, (w * 2) as usize));
            }
        }
    });
}

/// A colour-keyed or plain copy of the guest surface as a GPU texture for blits.
fn blit_source(c: &Cpu, obj: u32, key: Option<(u32, u32)>) -> Option<(TextureId, (u32, u32))> {
    let s = DD.with(|d| d.borrow().surfaces.get(&obj).cloned())?;
    let ver = version(obj);
    let cached = BLIT_SOURCES.with(|t| t.borrow().get(&obj).copied());
    if let Some((id, v, k, size)) = cached {
        if v == ver && k == key && size == (s.w, s.h) {
            return Some((id, size));
        }
    }
    let _t = stats::time(2);
    let mut rgba = to_rgba(c, &s);
    if let Some((lo, hi)) = key {
        // The key colour becomes transparent; the blit blends by alpha.
        for y in 0..s.h {
            let row = c.mem.view(s.ptr + y * s.pitch, (s.w * 2) as usize);
            for (x, p) in row.chunks_exact(2).enumerate() {
                let v = u16::from_le_bytes([p[0], p[1]]) as u32;
                let a = if v >= lo && v <= hi { 0 } else { 255 };
                rgba[((y * s.w) as usize + x) * 4 + 3] = a;
            }
        }
    } else {
        for a in rgba.chunks_exact_mut(4) {
            a[3] = 255;
        }
    }
    let id = with_gpu(|g| {
        let id = match cached {
            Some((id, _, _, size)) if size == (s.w, s.h) => id,
            Some((old, _, _, _)) => {
                g.destroy_texture(old);
                g.create_texture(s.w, s.h)
            }
            None => g.create_texture(s.w, s.h),
        };
        g.write_texture(id, rgba);
        id
    })?;
    BLIT_SOURCES.with(|t| t.borrow_mut().insert(obj, (id, ver, key, (s.w, s.h))));
    Some((id, (s.w, s.h)))
}

/// A `Blt`/`BltFast` of `src` onto `dst` done on the GPU when `dst` lives there. False: the caller does it on the CPU.
pub(super) fn gpu_blt(c: &Cpu, dst: u32, dst_pos: (u32, u32), src: u32, src_rect: (u32, u32, u32, u32), keyed: bool) -> bool {
    if !enabled() || dst == src || !wants_gpu(dst) {
        return false;
    }
    let (d, s) = (surface(dst), surface(src));
    if d.bpp != 16 || s.bpp != 16 || d.fmt != 0 {
        return false;
    }
    let _t = stats::time(6);
    let Some(target) = ensure_resident(dst) else { return false };
    upload_cpu_changes(c, dst);
    let (tex, tsize) = if wants_gpu(src) {
        let Some(t) = ensure_resident(src) else { return false };
        upload_cpu_changes(c, src);
        let Some(tex) = with_gpu(|g| g.target_texture(t)).flatten() else { return false };
        (tex, (s.w, s.h))
    } else {
        let key = if keyed { s.color_key } else { None };
        let Some(r) = blit_source(c, src, key) else { return false };
        r
    };
    let _ = tsize;
    let (sx, sy, w, h) = src_rect;
    if sx + w > s.w || sy + h > s.h || dst_pos.0 + w > d.w || dst_pos.1 + h > d.h {
        return false;
    }
    let blend = keyed && !wants_gpu(src) && s.color_key.is_some();
    with_gpu(|g| g.blit(target, tex, src_rect, dst_pos, blend));
    RES.with(|r| {
        if let Some(e) = r.borrow_mut().get_mut(&dst) {
            e.gpu_ahead = true;
        }
    });
    true
}

/// `Flip` of the primary surface (its memory and its back buffer's have just been exchanged): the GPU's pictures follow the memory
/// they stand for, and the new front picture is sent to the window. False: nothing of this is on the GPU, show the guest's memory.
pub(super) fn flip(primary: u32, back: u32) -> bool {
    if !enabled() {
        return false;
    }
    let target = RES.with(|r| {
        let mut r = r.borrow_mut();
        let (p, b) = (r.remove(&primary), r.remove(&back));
        if let Some(b) = b {
            r.insert(primary, b);
        }
        if let Some(p) = p {
            r.insert(back, p);
        }
        r.get(&primary).filter(|e| e.gpu_ahead).map(|e| e.target)
    });
    let Some(target) = target else { return false };
    static SINK: std::sync::OnceLock<s4_gpu::PresentSink> = std::sync::OnceLock::new();
    let sink = SINK.get_or_init(|| std::sync::Arc::new(|w, h, pixels| crate::present::publish(crate::present::Frame { w, h, pixels })));
    with_gpu(|g| g.present_async(target, sink)).is_some()
}

fn begin_scene(c: &mut Cpu, a: &[u32]) -> u32 {
    let _t = stats::time(0);
    let Some(surface) = with_device(a[0], |d| d.surface) else { return 0 };
    let Some(target) = ensure_resident(surface) else { return 0 };
    if trace() {
        eprintln!("d3d BeginScene dev {:#x} surface {surface:#x}", a[0]);
    }
    // The guest's own drawing on the surface since the GPU last saw it goes up first.
    upload_cpu_changes(c, surface);
    with_gpu(|g| g.begin_scene(target));
    0
}

fn end_scene(_c: &mut Cpu, a: &[u32]) -> u32 {
    let _t = stats::time(3);
    let Some(surface) = with_device(a[0], |d| {
        d.scenes += 1;
        if trace() && d.stats.0 > 0 {
            eprintln!("d3d scene dev {:#x}: {} draws, {} vertices, {} blended, {} textures", a[0], d.stats.0, d.stats.1, d.stats.2, d.stats.3.len());
        }
        let drew = std::mem::take(&mut d.drew);
        d.stats = Default::default();
        (d.surface, drew)
    }) else {
        return 0;
    };
    if trace() {
        eprintln!("d3d EndScene dev {:#x}", a[0]);
    }
    // The picture stays on the GPU; the guest's memory is brought up to date when the guest looks at it.
    if let Some(target) = RES.with(|r| r.borrow().get(&surface.0).map(|e| e.target)) {
        with_gpu(|g| g.end_scene_lazy(target));
        if surface.1 {
            RES.with(|r| {
                if let Some(e) = r.borrow_mut().get_mut(&surface.0) {
                    e.gpu_ahead = true;
                }
            });
        }
    }
    0
}

fn draw_primitive(c: &mut Cpu, a: &[u32]) -> u32 {
    let _t = stats::time(1);
    // (this, primitive type, vertex format, vertices, vertex count, flags)
    let (kind, fvf, ptr, count) = (a[1], a[2], a[3], a[4]);
    if kind != 4 || fvf != 0x1c4 {
        if trace() {
            eprintln!("d3d DrawPrimitive: unsupported type {kind} / format {fvf:#x}");
        }
        return 0;
    }
    let Some((surface, blend, specular, linear, texture, clip)) = with_device(a[0], |d| (d.surface, d.blend, d.specular, d.linear, d.texture, d.clip)) else { return 0 };
    let Some(target) = RES.with(|r| r.borrow().get(&surface).map(|e| e.target)) else { return 0 };
    let tex = if texture != 0 { texture_for(c, texture) } else { None };
    if trace() {
        with_device(a[0], |d| {
            d.stats.0 += 1;
            d.stats.1 += count;
            d.stats.2 += blend as u32;
            d.stats.3.insert(texture);
        });
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 400 {
            eprintln!("d3d draw dev {:#x}: {count} vertices, texture surface {texture:#x} blend {blend} specular {specular} linear {linear} clip {clip:?}", a[0]);
        }
    }
    let bytes = c.mem.view(ptr, (count * 32) as usize);
    with_gpu(|g| {
        g.draw_fill(target, DrawState { texture: tex, blend, linear, clip }, count as usize, |vertices| {
            // Straight from the guest's memory into the queued batch.
            bytemuck::cast_slice_mut::<Vertex, u8>(vertices).copy_from_slice(bytes);
            if !specular {
                for v in vertices.iter_mut() {
                    v.specular = 0;
                }
            }
        })
    });
    with_device(a[0], |d| d.drew = true);
    0
}

fn load(c: &mut Cpu, a: &[u32]) -> u32 {
    let _t = stats::time(4);
    // (this, destination texture, destination point, source texture, source rect, flags)
    let (Some(dst), Some(src)) = (DD.with(|d| d.borrow().surfaces.get(&a[1]).cloned()), DD.with(|d| d.borrow().surfaces.get(&a[3]).cloned())) else { return 0 };
    let m = c.mem;
    let (dx, dy) = if a[2] != 0 { (m.r32(a[2]), m.r32(a[2] + 4)) } else { (0, 0) };
    let (l, t, r, b) = if a[4] != 0 { (m.r32(a[4]), m.r32(a[4] + 4), m.r32(a[4] + 8), m.r32(a[4] + 12)) } else { (0, 0, src.w, src.h) };
    let (w, h) = ((r - l).min(dst.w.saturating_sub(dx)), (b - t).min(dst.h.saturating_sub(dy)));
    if load_direct(c, a[1], &dst, dx, dy, a[3], &src, l, t, w, h) {
        return 0;
    }
    sync_to_cpu(c, a[1]);
    sync_to_cpu(c, a[3]);
    copy_rect(c, &dst, dx, dy, &src, l, t, w, h, false);
    touch(a[1]);
    0
}

fn device_object(c: &mut Cpu) -> u32 {
    let vt = make_vtable(
        c,
        "IDirect3DDevice7",
        D3DDEVICE7,
        &[
            ("QueryInterface", 3, |c, a| {
                c.mem.w32(a[2], 0);
                0x8000_4002
            }),
            ("AddRef", 1, |c, a| add_ref(c, a[0])),
            ("Release", 1, |c, a| {
                let n = release(c, a[0]);
                if n == 0 {
                    DEVICES.with(|d| d.borrow_mut().remove(&a[0]));
                }
                n
            }),
            ("GetCaps", 2, |c, a| {
                device_desc(c, a[1]);
                0
            }),
            ("EnumTextureFormats", 3, |c, a| {
                let pf = crate::heap::alloc_zeroed(c, 32);
                for (flags, bits, masks) in [(0x40u32, 16u32, [0xf800u32, 0x07e0, 0x001f, 0]), (0x41, 16, [0x7c00, 0x03e0, 0x001f, 0x8000]), (0x41, 16, [0x0f00, 0x00f0, 0x000f, 0xf000])] {
                    pixel_format(c, pf, flags, bits, masks);
                    if crate::shims::guest_call(c, a[1], &[pf, a[2]]) == 0 {
                        break;
                    }
                }
                0
            }),
            ("GetDirect3D", 2, |c, a| {
                c.mem.w32(a[1], 0);
                0x8876_086c
            }),
            ("BeginScene", 1, begin_scene),
            ("EndScene", 1, end_scene),
            ("SetViewport", 2, |c, a| {
                // D3DVIEWPORT7: x, y, width, height, min z, max z
                let (x, y, w, h) = (c.mem.r32(a[1]), c.mem.r32(a[1] + 4), c.mem.r32(a[1] + 8), c.mem.r32(a[1] + 12));
                with_device(a[0], |d| {
                    let (tw, th) = DD.with(|dd| dd.borrow().surfaces.get(&d.surface).map(|s| (s.w, s.h))).unwrap_or((0, 0));
                    d.clip = if x == 0 && y == 0 && w >= tw && h >= th { None } else { Some((x, y, w, h)) };
                });
                0
            }),
            ("SetRenderState", 3, |_, a| {
                // 27 ALPHABLENDENABLE, 29 SPECULARENABLE; the blend factors are always source alpha / inverse source alpha here.
                with_device(a[0], |d| match a[1] {
                    27 => d.blend = a[2] != 0,
                    29 => d.specular = a[2] != 0,
                    _ => {}
                });
                0
            }),
            ("SetTexture", 3, |_, a| {
                if a[1] == 0 {
                    with_device(a[0], |d| d.texture = a[2]);
                }
                0
            }),
            ("SetTextureStageState", 4, |_, a| {
                // stage, state, value: 16 MAGFILTER / 17 MINFILTER (1 point, 2 linear, 3 anisotropic)
                if a[1] == 0 && (a[2] == 16 || a[2] == 17) {
                    with_device(a[0], |d| d.linear = a[3] >= 2 || std::env::var_os("S4_D3D_LINEAR").is_some());
                }
                0
            }),
            ("DrawPrimitive", 6, draw_primitive),
            ("Load", 6, load),
            logged!("SetRenderTarget", 3),
            logged!("GetRenderTarget", 2),
            logged!("Clear", 7),
            logged!("SetTransform", 3),
            logged!("GetTransform", 3),
            logged!("GetViewport", 2),
            logged!("MultiplyTransform", 3),
            logged!("SetMaterial", 2),
            logged!("GetMaterial", 2),
            logged!("SetLight", 3),
            logged!("GetLight", 3),
            logged!("GetRenderState", 3),
            logged!("BeginStateBlock", 1),
            logged!("EndStateBlock", 2),
            logged!("PreLoad", 2),
            logged!("DrawIndexedPrimitive", 8),
            logged!("SetClipStatus", 2),
            logged!("GetClipStatus", 2),
            logged!("DrawPrimitiveStrided", 6),
            logged!("DrawIndexedPrimitiveStrided", 8),
            logged!("DrawPrimitiveVB", 6),
            logged!("DrawIndexedPrimitiveVB", 8),
            logged!("ComputeSphereVisibility", 5),
            logged!("GetTexture", 3),
            logged!("GetTextureStageState", 4),
            logged!("ValidateDevice", 2),
            logged!("ApplyStateBlock", 2),
            logged!("CaptureStateBlock", 2),
            logged!("DeleteStateBlock", 2),
            logged!("CreateStateBlock", 3),
            logged!("LightEnable", 3),
            logged!("GetLightEnable", 3),
            logged!("SetClipPlane", 3),
            logged!("GetClipPlane", 3),
            logged!("GetInfo", 4),
        ],
    );
    new_object(c, vt)
}
