//! Mouse cursors: the game loads them from module resources (RT_GROUP_CURSOR)
//! and selects one with `SetCursor`; the window front end shows the current one.

use crate::pe;
use s4_x86::Cpu;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct Cursor {
    pub w: u32,
    pub h: u32,
    pub hot: (u32, u32),
    /// RGBA, top-down.
    pub rgba: Vec<u8>,
}

#[derive(Default)]
struct State {
    by_key: HashMap<(u32, u32), u32>,
    images: HashMap<u32, Arc<Cursor>>,
    next: u32,
    current: u32,
    serial: u64,
    /// ShowCursor display counter.
    shown: i32,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap();
    f(g.get_or_insert_with(|| State { next: 0x5001, shown: 0, ..Default::default() }))
}

/// The current cursor, whether it is shown, and a counter that changes when it does (`None`: the system arrow).
pub fn current() -> (u64, bool, Option<Arc<Cursor>>) {
    with(|s| (s.serial, s.shown >= 0, s.images.get(&s.current).cloned()))
}

pub fn set(handle: u32) -> u32 {
    with(|s| {
        let old = s.current;
        if old != handle {
            s.current = handle;
            s.serial += 1;
        }
        old
    })
}

fn decode(data: &[u8]) -> Option<Cursor> {
    let u16_at = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]) as u32;
    let u32_at = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    if data.len() < 44 {
        return None;
    }
    let hot = (u16_at(0), u16_at(2));
    let dib = &data[4..];
    let u16d = |o: usize| u16::from_le_bytes([dib[o], dib[o + 1]]) as usize;
    let u32d = |o: usize| u32::from_le_bytes([dib[o], dib[o + 1], dib[o + 2], dib[o + 3]]) as usize;
    let (hsize, w, h2, bpp) = (u32d(0), u32d(4), u32d(8), u16d(14));
    let h = h2 / 2;
    if w == 0 || h == 0 || w > 256 || h > 256 {
        return None;
    }
    let colors = if u32d(32) != 0 { u32d(32) } else if bpp <= 8 { 1 << bpp } else { 0 };
    let pal = hsize;
    let bits = pal + colors * 4;
    let stride = ((w * bpp + 31) / 32) * 4;
    let mask_stride = ((w + 31) / 32) * 4;
    let mask = bits + stride * h;
    if dib.len() < mask + mask_stride * h {
        return None;
    }
    let _ = u32_at;
    let mut rgba = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        let row = bits + (h - 1 - y) * stride;
        let mrow = mask + (h - 1 - y) * mask_stride;
        for x in 0..w {
            let (b, g, r, a) = match bpp {
                32 => (dib[row + x * 4], dib[row + x * 4 + 1], dib[row + x * 4 + 2], dib[row + x * 4 + 3]),
                24 => (dib[row + x * 3], dib[row + x * 3 + 1], dib[row + x * 3 + 2], 255),
                8 | 4 | 1 => {
                    let idx = match bpp {
                        8 => dib[row + x] as usize,
                        4 => ((dib[row + x / 2] >> (if x % 2 == 0 { 4 } else { 0 })) & 15) as usize,
                        _ => ((dib[row + x / 8] >> (7 - x % 8)) & 1) as usize,
                    };
                    let p = pal + idx * 4;
                    (dib[p], dib[p + 1], dib[p + 2], 255)
                }
                _ => return None,
            };
            any_alpha |= bpp == 32 && a != 0;
            let o = (y * w + x) * 4;
            rgba[o..o + 4].copy_from_slice(&[r, g, b, a]);
        }
        if !(bpp == 32 && any_alpha) {
            for x in 0..w {
                let transparent = dib[mrow + x / 8] >> (7 - x % 8) & 1 != 0;
                let o = (y * w + x) * 4;
                // AND=1 is transparent (or, with a colour set, XOR-inverted: shown transparent).
                rgba[o + 3] = if transparent { 0 } else { 255 };
            }
        }
    }
    Some(Cursor { w: w as u32, h: h as u32, hot, rgba })
}

/// `LoadCursorA(instance, id)`: a handle (module cursors are decoded; others get a blank handle).
pub fn load(c: &Cpu, inst: u32, rsrc_rva: Option<u32>, id: u32) -> u32 {
    if let Some(h) = with(|s| s.by_key.get(&(inst, id)).copied()) {
        return h;
    }
    let image = rsrc_rva.and_then(|rva| {
        let (grp, _) = pe::resource(c.mem, inst, rva, 12, id)?;
        // GRPICONDIR: reserved, type, count, then 14-byte entries ending with the RT_CURSOR id.
        let cursor_id = c.mem.r16(grp + 6 + 12) as u32;
        let (data, size) = pe::resource(c.mem, inst, rva, 1, cursor_id)?;
        decode(&c.mem.read_bytes(data, size as usize))
    });
    if let (Some(img), Ok(dir)) = (&image, std::env::var("S4_CURSOR_DUMP")) {
        let mut out = format!("{} {}\n", img.w, img.h).into_bytes();
        out.extend_from_slice(&img.rgba);
        let _ = std::fs::write(format!("{dir}/cursor_{id:x}.rgba"), out);
    }
    with(|s| {
        let h = s.next;
        s.next += 1;
        s.by_key.insert((inst, id), h);
        if let Some(img) = image {
            s.images.insert(h, Arc::new(img));
        }
        h
    })
}

/// `ShowCursor(show)`: the new display counter.
pub fn show(show: bool) -> i32 {
    with(|s| {
        s.shown += if show { 1 } else { -1 };
        s.serial += 1;
        s.shown
    })
}
