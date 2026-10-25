//! GDI text (`DrawTextA`, `TextOutA`, `GetTextExtentPoint32A`) rendered with a
//! system TrueType font. The original asks Windows for "Arial" and gets its
//! hinted glyphs; no host reproduces those bytes, so text is the one place the
//! native build deliberately differs from the original's pixels.

use crate::gdi::{self, Object};
use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use s4_x86::Cpu;
use std::sync::OnceLock;

static FONT: OnceLock<Option<FontVec>> = OnceLock::new();

const CANDIDATES: [&str; 10] = [
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf",
    "/usr/share/fonts/truetype/liberation2/LiberationSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    // macOS
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    "/Library/Fonts/Arial Bold.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
];

/// The regular weight of the same families: the original asks for a bold "MS Sans Serif" (a thin bitmap font),
/// and a bold TrueType face is much heavier than that.
const REGULAR: [&str; 7] = [
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
];

static REGULAR_FACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Draw text in the regular weight whatever the program asks for (call before the first text is drawn).
pub fn use_regular_face() {
    REGULAR_FACE.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn font() -> Option<&'static FontVec> {
    FONT.get_or_init(|| {
        let list: &[&str] = if REGULAR_FACE.load(std::sync::atomic::Ordering::Relaxed) { &REGULAR } else { &CANDIDATES };
        let paths = std::env::var("S4_FONT").into_iter().chain(list.iter().map(|s| s.to_string()));
        paths.into_iter().find_map(|p| FontVec::try_from_vec(std::fs::read(p).ok()?).ok())
    })
    .as_ref()
}

fn scale_of(font: &FontVec, height: i32) -> PxScale {
    // Positive: cell height (ascent + descent); negative: the em size.
    if height >= 0 {
        PxScale::from(height.max(1) as f32)
    } else {
        let k = font.height_unscaled() / font.units_per_em().unwrap_or(1000.0);
        PxScale::from(-height as f32 * k)
    }
}

fn dc_font(dc: u32) -> (i32, i32) {
    let h = gdi::dc_state(dc).font;
    gdi::GDI.with(|g| match g.borrow().objects.get(&h) {
        Some(Object::Font { height, weight, .. }) => (*height, *weight),
        _ => (-12, 400),
    })
}

fn advance(font: &FontVec, scale: PxScale, s: &str) -> f32 {
    let f = font.as_scaled(scale);
    s.chars().map(|c| f.h_advance(f.glyph_id(c))).sum()
}

/// (width, height) of `s` in the DC's font.
pub fn extent(dc: u32, s: &str) -> (i32, i32) {
    if let Some((strike, bold)) = bitmap_for(dc) {
        return bitmap_extent(strike, bold, s);
    }
    let Some(font) = font() else { return (8 * s.len() as i32, 16) };
    let (h, _) = dc_font(dc);
    let scale = scale_of(font, h);
    let sc = font.as_scaled(scale);
    (advance(font, scale, s).ceil() as i32, sc.height().ceil() as i32)
}

/// Draws `s` with its top-left at (x, y) in the DC's bitmap.
fn draw_run(c: &Cpu, dc: u32, x: f32, y: i32, s: &str, clip: Option<[i32; 4]>) {
    if let Some((strike, bold)) = bitmap_for(dc) {
        return bitmap_run(c, dc, strike, bold, x.round() as i32, y, s, clip);
    }
    let (Some(font), Some(bmp)) = (font(), gdi::dc_bitmap(dc)) else { return };
    let st = gdi::dc_state(dc);
    let (h, _) = dc_font(dc);
    let scale = scale_of(font, h);
    let sc = font.as_scaled(scale);
    let baseline = y as f32 + sc.ascent();
    let mut pen = x;
    let (tr, tg, tb) = (st.text_color & 0xff, st.text_color >> 8 & 0xff, st.text_color >> 16 & 0xff);
    for ch in s.chars() {
        let g = font.glyph_id(ch).with_scale_and_position(scale, point(pen, baseline));
        pen += sc.h_advance(g.id);
        if let Some(o) = font.outline_glyph(g) {
            let b = o.px_bounds();
            o.draw(|gx, gy, cov| {
                if cov <= 0.0 {
                    return;
                }
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if let Some([l, t, r, b]) = clip {
                    if px < l || px >= r || py < t || py >= b {
                        return;
                    }
                }
                let under = gdi::pixel(c, &bmp, px, py);
                if under == 0xffff_ffff {
                    return;
                }
                let mix = |t: u32, u: u32| (t as f32 * cov + u as f32 * (1.0 - cov)).round() as u32;
                let col = mix(tr, under & 0xff) | mix(tg, under >> 8 & 0xff) << 8 | mix(tb, under >> 16 & 0xff) << 16;
                gdi::put_pixel(c, &bmp, px, py, col);
            });
        }
    }
}

/// `TextOutA(dc, x, y, text, count)`.
pub fn text_out(c: &Cpu, dc: u32, x: i32, y: i32, s: &str) {
    let st = gdi::dc_state(dc);
    let (w, _) = extent(dc, s);
    let x0 = match st.align & 6 {
        6 => x - w / 2,
        2 => x - w,
        _ => x,
    };
    draw_run(c, dc, x0 as f32, y, s, None);
}

const DT_CENTER: u32 = 1;
const DT_RIGHT: u32 = 2;
const DT_VCENTER: u32 = 4;
const DT_BOTTOM: u32 = 8;
const DT_WORDBREAK: u32 = 0x10;
const DT_SINGLELINE: u32 = 0x20;
const DT_CALCRECT: u32 = 0x400;

/// `DrawTextA`: returns the height of the text drawn.
pub fn draw_text(c: &Cpu, dc: u32, s: &str, rect: &mut [i32; 4], flags: u32) -> i32 {
    let (_, lh) = extent(dc, "Ag");
    let width = rect[2] - rect[0];
    // Lines: explicit breaks, then word wrap if asked.
    let mut lines: Vec<String> = Vec::new();
    for para in s.split('\n') {
        let para = para.trim_end_matches('\r');
        if flags & DT_WORDBREAK != 0 && flags & DT_SINGLELINE == 0 {
            let mut cur = String::new();
            for word in para.split(' ') {
                let trial = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
                if !cur.is_empty() && extent(dc, &trial).0 > width {
                    lines.push(std::mem::take(&mut cur));
                    cur = word.to_string();
                } else {
                    cur = trial;
                }
            }
            lines.push(cur);
        } else {
            lines.push(para.to_string());
            if flags & DT_SINGLELINE != 0 {
                break;
            }
        }
    }
    let total = lh * lines.len() as i32;
    if flags & DT_CALCRECT != 0 {
        let w = lines.iter().map(|l| extent(dc, l).0).max().unwrap_or(0);
        rect[2] = rect[0] + w;
        rect[3] = rect[1] + total;
        return total;
    }
    let height = rect[3] - rect[1];
    let top = if flags & DT_VCENTER != 0 { rect[1] + (height - total) / 2 } else if flags & DT_BOTTOM != 0 { rect[3] - total } else { rect[1] };
    for (i, line) in lines.iter().enumerate() {
        let w = extent(dc, line).0;
        let x = if flags & DT_CENTER != 0 { rect[0] + (width - w) / 2 } else if flags & DT_RIGHT != 0 { rect[2] - w } else { rect[0] };
        draw_run(c, dc, x as f32, top + lh * i as i32, line, if flags & 0x100 != 0 { None } else { Some(*rect) });
    }
    total
}

// ---- bitmap "MS Sans Serif" ----
//
// The original draws with the raster font MS Sans Serif (crisp, not smoothed). Wine's free replacement
// (`sserife.fon`, LGPL) has the same sizes and looks; it is read from a Wine installation if there is one
// (nothing is copied into this program). Bold is made the way GDI does for raster fonts: the glyph drawn twice,
// one pixel apart.

struct Strike {
    height: i32,
    ascent: i32,
    first: u32,
    /// (width, offset of the glyph's bitmap in `data`) per character from `first`.
    glyphs: Vec<(u32, usize)>,
    data: std::sync::Arc<Vec<u8>>,
}

static STRIKES: OnceLock<Vec<Strike>> = OnceLock::new();

fn wine_font_dirs() -> Vec<String> {
    let mut v: Vec<String> = ["/usr/share/wine/fonts", "/usr/lib/wine/fonts", "/usr/lib/x86_64-linux-gnu/wine/fonts", "/opt/homebrew/share/wine/fonts", "/usr/local/share/wine/fonts"].iter().map(|s| s.to_string()).collect();
    if let Ok(d) = std::env::var("S4_BITMAP_FONT_DIR") {
        v.insert(0, d);
    }
    // Wine bundles the Heroic launcher downloads (macOS, Linux).
    if let Ok(home) = std::env::var("HOME") {
        for base in [format!("{home}/Library/Application Support/heroic/tools/wine"), format!("{home}/.config/heroic/tools/wine")] {
            if let Ok(rd) = std::fs::read_dir(&base) {
                for e in rd.flatten() {
                    let p = e.path();
                    v.push(format!("{}/Contents/Resources/wine/share/wine/fonts", p.display()));
                    v.push(format!("{}/share/wine/fonts", p.display()));
                    v.push(format!("{}/lib/wine/fonts", p.display()));
                }
            }
        }
    }
    v
}

fn le16(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn le32(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

/// The FNT resources of a 16-bit .fon file (an NE executable).
fn parse_fon(data: Vec<u8>) -> Option<Vec<Strike>> {
    let data = std::sync::Arc::new(data);
    let d = &data[..];
    if d.len() < 0x40 || &d[..2] != b"MZ" {
        return None;
    }
    let ne = le32(d, 0x3c);
    if d.get(ne..ne + 2)? != b"NE" {
        return None;
    }
    let rt = ne + le16(d, ne + 0x24);
    let shift = le16(d, rt);
    let mut p = rt + 2;
    let mut out = Vec::new();
    loop {
        let t = le16(d, p);
        if t == 0 {
            break;
        }
        let n = le16(d, p + 2);
        p += 8;
        for _ in 0..n {
            let (off, _len) = (le16(d, p) << shift, le16(d, p + 2) << shift);
            p += 12;
            if t != 0x8008 {
                continue;
            }
            let ver = le16(d, off);
            let hdr = if ver >= 0x300 { 148 } else { 118 };
            let entry = if ver >= 0x300 { 6 } else { 4 };
            let (ascent, height) = (le16(d, off + 74) as i32, le16(d, off + 88) as i32);
            let (first, last) = (d[off + 95] as u32, d[off + 96] as u32);
            let count = (last - first + 1) as usize;
            let mut glyphs = Vec::new();
            for i in 0..count {
                let e = off + hdr + i * entry;
                let w = le16(d, e) as u32;
                let o = if ver >= 0x300 { le32(d, e + 2) } else { le16(d, e + 2) };
                glyphs.push((w, off + o));
            }
            out.push(Strike { height, ascent, first, glyphs, data: data.clone() });
        }
    }
    out.sort_by_key(|s| s.height);
    (!out.is_empty()).then_some(out)
}

fn strikes() -> Option<&'static Vec<Strike>> {
    let list = STRIKES.get_or_init(|| {
        if std::env::var_os("S4_NO_BITMAP_FONT").is_some() {
            return Vec::new();
        }
        for dir in wine_font_dirs() {
            if let Some(v) = std::fs::read(format!("{dir}/sserife.fon")).ok().and_then(parse_fon) {
                return v;
            }
        }
        Vec::new()
    });
    (!list.is_empty() && REGULAR_FACE.load(std::sync::atomic::Ordering::Relaxed)).then_some(list)
}

/// The strike (and whether to embolden) for the DC's font, if it is "MS Sans Serif" and a bitmap font exists:
/// the tallest strike that fits the requested height (the smallest if none does), like GDI's font mapper.
fn bitmap_for(dc: u32) -> Option<(&'static Strike, bool)> {
    let strikes = strikes()?;
    let h = gdi::dc_state(dc).font;
    let (height, weight, face) = gdi::GDI.with(|g| match g.borrow().objects.get(&h) {
        Some(Object::Font { height, weight, face, .. }) => Some((*height, *weight, face.clone())),
        _ => None,
    })?;
    if !face.eq_ignore_ascii_case("MS Sans Serif") {
        return None;
    }
    let want = if height >= 0 { height } else { -height + 3 };
    let strike = strikes.iter().rev().find(|s| s.height <= want).unwrap_or(&strikes[0]);
    Some((strike, weight >= 600))
}

fn glyph_of<'a>(s: &'a Strike, ch: char) -> Option<&'a (u32, usize)> {
    let code = if (ch as u32) < 256 { ch as u32 } else { '?' as u32 };
    s.glyphs.get(code.checked_sub(s.first)? as usize)
}

fn bitmap_extent(s: &Strike, bold: bool, text: &str) -> (i32, i32) {
    let w: u32 = text.chars().map(|c| glyph_of(s, c).map_or(0, |g| g.0) + bold as u32).sum();
    (w as i32, s.height)
}

fn bitmap_run(c: &Cpu, dc: u32, s: &Strike, bold: bool, x: i32, y: i32, text: &str, clip: Option<[i32; 4]>) {
    let Some(bmp) = gdi::dc_bitmap(dc) else { return };
    let st = gdi::dc_state(dc);
    let col = st.text_color & 0xff | (st.text_color >> 8 & 0xff) << 8 | (st.text_color >> 16 & 0xff) << 16;
    let mut pen = x;
    for ch in text.chars() {
        let Some(&(w, off)) = glyph_of(s, ch) else { continue };
        let bytes = (w as usize).div_ceil(8);
        for gx in 0..w as usize {
            for gy in 0..s.height as usize {
                if s.data.get(off + (gx / 8) * s.height as usize + gy).is_none_or(|b| b >> (7 - gx % 8) & 1 == 0) {
                    continue;
                }
                for extra in 0..=bold as i32 {
                    let (px, py) = (pen + gx as i32 + extra, y + gy as i32);
                    if let Some([l, t, r, b]) = clip {
                        if px < l || px >= r || py < t || py >= b {
                            continue;
                        }
                    }
                    if gdi::pixel(c, &bmp, px, py) != 0xffff_ffff {
                        gdi::put_pixel(c, &bmp, px, py, col);
                    }
                }
            }
        }
        let _ = bytes;
        pen += w as i32 + bold as i32;
    }
}
