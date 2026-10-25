//! A small GDI: bitmaps (from resources), device contexts, `GetPixel`,
//! `BitBlt`. Handles are opaque numbers.

use s4_x86::Cpu;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Bitmap {
    pub w: i32,
    pub h: i32,
    pub bpp: u32,
    /// Guest address of the first row in memory order and the row stride.
    pub bits: u32,
    pub pitch: i32,
    pub palette: Vec<u32>,
    /// RGB565 (else the 24/8-bit layouts of a DIB).
    pub rgb565: bool,
}

#[derive(Clone, Debug)]
pub enum Object {
    Bitmap(Bitmap),
    Dc { bitmap: Option<u32> },
    Font { height: i32, weight: i32, italic: bool, face: String },
    Other,
}

/// A device context's text state.
#[derive(Clone, Copy)]
pub struct DcState {
    pub text_color: u32,
    pub bk_color: u32,
    /// 1 = transparent, 2 = opaque.
    pub bk_mode: u32,
    pub align: u32,
    pub font: u32,
}

impl Default for DcState {
    fn default() -> DcState {
        DcState { text_color: 0, bk_color: 0xff_ffff, bk_mode: 2, align: 0, font: 0 }
    }
}

#[derive(Default)]
pub struct Gdi {
    next: u32,
    pub objects: HashMap<u32, Object>,
    pub states: HashMap<u32, DcState>,
}

thread_local! {
    pub static GDI: RefCell<Gdi> = RefCell::new(Gdi { next: 0x1000, objects: HashMap::new(), states: HashMap::new() });
}

pub fn new_handle(o: Object) -> u32 {
    GDI.with(|g| {
        let mut g = g.borrow_mut();
        let h = g.next;
        g.next += 4;
        g.objects.insert(h, o);
        h
    })
}

/// The colour at (x, y) of a bitmap as a COLORREF (0x00BBGGRR).
pub fn pixel(c: &Cpu, b: &Bitmap, x: i32, y: i32) -> u32 {
    if x < 0 || y < 0 || x >= b.w || y >= b.h {
        return 0xffff_ffff;
    }
    let row = b.bits.wrapping_add((y * b.pitch) as u32);
    let m = c.mem;
    match b.bpp {
        4 => {
            let v = m.r8(row + x as u32 / 2);
            let i = if x & 1 == 0 { v >> 4 } else { v & 15 };
            b.palette.get(i as usize).copied().unwrap_or(0)
        }
        1 => {
            let v = m.r8(row + x as u32 / 8);
            b.palette.get(((v >> (7 - x % 8)) & 1) as usize).copied().unwrap_or(0)
        }
        8 => b.palette.get(m.r8(row + x as u32) as usize).copied().unwrap_or(0),
        16 if b.rgb565 => {
            let v = m.r16(row + 2 * x as u32) as u32;
            let (r, g, bl) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
            let (r, g, bl) = ((r << 3) | (r >> 2), (g << 2) | (g >> 4), (bl << 3) | (bl >> 2));
            r | g << 8 | bl << 16
        }
        24 => {
            let p = row + 3 * x as u32;
            m.r8(p + 2) as u32 | (m.r8(p + 1) as u32) << 8 | (m.r8(p) as u32) << 16
        }
        32 => {
            let p = row + 4 * x as u32;
            m.r8(p + 2) as u32 | (m.r8(p + 1) as u32) << 8 | (m.r8(p) as u32) << 16
        }
        bpp => panic!("GetPixel on a {bpp}-bit bitmap"),
    }
}

/// A DIB resource (BITMAPINFOHEADER, palette, bits) as a bitmap whose bits stay in the image.
pub fn dib_bitmap(c: &Cpu, addr: u32) -> Bitmap {
    let m = c.mem;
    let (size, w, h, bpp, used) = (m.r32(addr), m.r32(addr + 4) as i32, m.r32(addr + 8) as i32, m.r16(addr + 14) as u32, m.r32(addr + 32));
    let colors = if bpp <= 8 { if used != 0 { used } else { 1 << bpp } } else { 0 };
    let palette: Vec<u32> = (0..colors).map(|i| {
        let e = addr + size + 4 * i;
        m.r8(e + 2) as u32 | (m.r8(e + 1) as u32) << 8 | (m.r8(e) as u32) << 16
    }).collect();
    let bits = addr + size + 4 * colors;
    let pitch = (((w as u32 * bpp + 31) / 32) * 4) as i32;
    let top_down = h < 0;
    let h = h.abs();
    if top_down {
        Bitmap { w, h, bpp, bits, pitch, palette, rgb565: false }
    } else {
        // Bottom-up: the first row in memory is the last scan line.
        Bitmap { w, h, bpp, bits: bits + (pitch * (h - 1)) as u32, pitch: -pitch, palette, rgb565: false }
    }
}

/// The bitmap behind a device context.
pub fn dc_bitmap(dc: u32) -> Option<Bitmap> {
    GDI.with(|g| {
        let g = g.borrow();
        match g.objects.get(&dc) {
            Some(Object::Dc { bitmap: Some(b) }) => match g.objects.get(b) {
                Some(Object::Bitmap(bm)) => Some(bm.clone()),
                _ => None,
            },
            _ => None,
        }
    })
}

pub fn put_pixel(c: &Cpu, b: &Bitmap, x: i32, y: i32, colorref: u32) {
    if x < 0 || y < 0 || x >= b.w || y >= b.h {
        return;
    }
    let row = b.bits.wrapping_add((y * b.pitch) as u32);
    let (r, g, bl) = (colorref & 0xff, colorref >> 8 & 0xff, colorref >> 16 & 0xff);
    match b.bpp {
        16 => c.mem.w16(row + 2 * x as u32, ((r >> 3) << 11 | (g >> 2) << 5 | bl >> 3) as u16),
        24 => {
            let p = row + 3 * x as u32;
            c.mem.w8(p, bl as u8);
            c.mem.w8(p + 1, g as u8);
            c.mem.w8(p + 2, r as u8);
        }
        32 => c.mem.w32(row + 4 * x as u32, bl | g << 8 | r << 16),
        // Paletted (S3 draws its in-game texts into a 1-bit DIB section and reads the bits back): the
        // nearest palette entry, as GDI does.
        1 | 4 | 8 => {
            let i = nearest(&b.palette, colorref);
            match b.bpp {
                8 => c.mem.w8(row + x as u32, i),
                4 => {
                    let p = row + x as u32 / 2;
                    let v = c.mem.r8(p);
                    c.mem.w8(p, if x & 1 == 0 { (v & 0x0f) | i << 4 } else { (v & 0xf0) | (i & 15) });
                }
                _ => {
                    let p = row + x as u32 / 8;
                    let bit = 0x80u8 >> (x % 8);
                    let v = c.mem.r8(p);
                    c.mem.w8(p, if i & 1 != 0 { v | bit } else { v & !bit });
                }
            }
        }
        _ => {}
    }
}

/// The index of the palette entry closest to a COLORREF.
fn nearest(palette: &[u32], colorref: u32) -> u8 {
    let ch = |v: u32, s: u32| ((v >> s) & 0xff) as i32;
    palette
        .iter()
        .enumerate()
        .min_by_key(|&(_, &p)| (0..3).map(|k| (ch(p, 8 * k) - ch(colorref, 8 * k)).pow(2)).sum::<i32>())
        .map_or(0, |(i, _)| i as u8)
}

/// `BitBlt(dst, x, y, w, h, src, sx, sy, rop)`: SRCCOPY between bitmaps.
pub fn bit_blt(c: &mut Cpu, a: &[u32]) -> u32 {
    let (Some(dst), Some(src)) = (dc_bitmap(a[0]), dc_bitmap(a[5])) else { return 0 };
    let (x, y, w, h, sx, sy) = (a[1] as i32, a[2] as i32, a[3] as i32, a[4] as i32, a[6] as i32, a[7] as i32);
    for j in 0..h {
        for i in 0..w {
            let v = pixel(c, &src, sx + i, sy + j);
            if v != 0xffff_ffff {
                put_pixel(c, &dst, x + i, y + j, v);
            }
        }
    }
    1
}

pub fn dc_state(dc: u32) -> DcState {
    GDI.with(|g| g.borrow().states.get(&dc).copied().unwrap_or_default())
}

pub fn update_state(dc: u32, f: impl FnOnce(&mut DcState)) {
    GDI.with(|g| f(g.borrow_mut().states.entry(dc).or_default()));
}
