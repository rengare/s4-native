mod common;
use common::*;
use s4_formats_clean::format::{FileHeader, Index};
use s4_formats_clean::sprites::{job_frames, GfxFile, Image, ImageHeader, Pixel, FLAG_RAW};

fn err_text(e: &s4_formats_clean::FormatError) -> String {
    err_code(e)
}

fn image_text(img: &Image) -> String {
    let h = &img.header;
    let px = if h.width as usize * h.height as usize > 70000 {
        "-".to_string()
    } else {
        img.pixels()
            .iter()
            .map(|p| match p {
                Pixel::Transparent => "T".to_string(),
                Pixel::Shadow => "S".to_string(),
                Pixel::Index(i) => format!("[{i:02x}]"),
            })
            .collect()
    };
    format!("{},{},{},{},{},{},{},{},{},{};{}", h.width, h.height, h.x, h.y, h.flags, h.extra, h.size, img.offset, img.data.len(), img.end(), px)
}

fn index_of(offsets: &[u32]) -> Index {
    Index { header: FileHeader { magic: 0x8003, version: 0x3f }, offsets: offsets.to_vec() }
}

fn split_input(input: &str) -> (Vec<u8>, &str) {
    let (data, rest) = input.split_once(':').expect("input needs a ':'");
    (bytes_of(data), rest)
}

fn offsets_of(s: &str) -> Vec<u32> {
    if s.is_empty() { Vec::new() } else { s.split(',').map(|x| x.parse().unwrap()).collect() }
}

#[test]
fn raw_flag_constant() {
    assert_eq!(FLAG_RAW, 0x20);
}

#[test]
fn raw_needs_the_long_form() {
    let mk = |size: u8, flags: u16| ImageHeader { width: 1, height: 1, x: 0, y: 0, flags, extra: 0, size };
    assert!(mk(12, 0x20).is_raw());
    assert!(mk(12, 0xffff).is_raw());
    assert!(mk(12, 0x30).is_raw());
    assert!(!mk(12, 0).is_raw());
    assert!(!mk(12, 0xffdf).is_raw());
    assert!(!mk(8, 0x20).is_raw());
    assert!(!mk(8, 0xffff).is_raw());
}

#[test]
fn vectors() {
    let rows = rows("gfx.tsv");
    assert!(rows.len() >= 150, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, kind, input, expect) = (&r[0], r[1].as_str(), &r[2], &r[3]);
        let got = match kind {
            "hdr" => {
                let (d, at) = split_input(input);
                match ImageHeader::parse(&d, at.parse().unwrap()) {
                    Ok(h) => format!("ok:{},{},{},{},{},{},{}", h.width, h.height, h.x, h.y, h.flags, h.extra, h.size),
                    Err(e) => err_text(&e),
                }
            }
            "image" => {
                let (d, at) = split_input(input);
                match Image::at(&d, at.parse().unwrap()) {
                    Ok(i) => format!("ok:{}", image_text(&i)),
                    Err(e) => err_text(&e),
                }
            }
            "images" => {
                let d = bytes_of(input);
                match GfxFile::parse(&d) {
                    Err(e) => err_text(&e),
                    Ok(f) => {
                        let items: Vec<String> = f.images().map(|r| r.map(|i| image_text(&i)).unwrap_or_else(|e| err_text(&e))).collect();
                        format!("ok:{:#x},{}:{}", f.header.magic, f.header.version, items.join("|"))
                    }
                }
            }
            "indexed" => {
                let (d, offs) = split_input(input);
                let f = GfxFile::parse(&d).unwrap();
                match f.indexed(&index_of(&offsets_of(offs))) {
                    Ok(v) => format!("ok:{}", v.iter().map(|e| e.as_ref().map_or("none".to_string(), image_text)).collect::<Vec<_>>().join("|")),
                    Err(e) => err_text(&e),
                }
            }
            "jobs" => {
                let parts: Vec<&str> = input.split(':').collect();
                let jobs = job_frames(&index_of(&offsets_of(parts[0])), &index_of(&offsets_of(parts[1])), parts[2].parse().unwrap());
                jobs.iter()
                    .map(|j| format!("J({})", j.iter().map(|d| d.iter().map(|f| f.to_string()).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join(";")))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            other => panic!("unknown vector kind {other}"),
        };
        if &got != expect {
            failed.push(format!("{name}: got {:.120}, expected {:.120}", got, expect));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

#[test]
fn image_at_a_single_file_offset_equals_image_at_from_the_file() {
    for r in rows("gfx.tsv").iter().filter(|r| r[1] == "images" && r[3].starts_with("ok:")) {
        let d = bytes_of(&r[2]);
        let f = GfxFile::parse(&d).unwrap();
        for img in f.images().flatten() {
            let again = f.image_at(img.offset as u32).unwrap();
            assert_eq!(again, img, "{}", r[0]);
        }
    }
}

#[test]
fn never_panics_on_damaged_files() {
    for r in rows("gfx.tsv").iter().filter(|r| r[1] == "images") {
        let d = bytes_of(&r[2]);
        let attempt = |d: &[u8]| {
            if let Ok(f) = GfxFile::parse(d) {
                for img in f.images().flatten() {
                    if (img.header.width as usize) * (img.header.height as usize) <= 70000 {
                        let _ = img.pixels();
                    }
                }
                let _ = f.indexed(&index_of(&[0, 20, 21, 25, d.len() as u32, u32::MAX]));
            }
        };
        for cut in (0..d.len().min(2000)).step_by(3).chain([d.len().saturating_sub(1)]) {
            attempt(&d[..cut]);
        }
        for i in (0..d.len().min(300)).step_by(2) {
            let mut e = d.clone();
            e[i] ^= 0xa5;
            attempt(&e);
        }
    }
}

#[test]
fn job_frames_never_panics_on_odd_tables() {
    let vals = [0u32, 1, 19, 20, 23, 24, 28, 100, 4000];
    for a in vals {
        for b in vals {
            for c in vals {
                for len in [0usize, 1, 5, 1000] {
                    let _ = job_frames(&index_of(&[a, b]), &index_of(&[b, c, a]), len);
                }
            }
        }
    }
}
