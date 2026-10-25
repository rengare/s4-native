mod common;
use common::*;
use s4_formats_clean::palette::{format_from_extension, parse_palette, to_rgba, PixelFormat};

fn name_of(f: PixelFormat) -> &'static str {
    match f {
        PixelFormat::Argb4444 => "argb4444",
        PixelFormat::Rgb555 => "rgb555",
        PixelFormat::Rgb565 => "rgb565",
    }
}

fn format_named(n: &str) -> PixelFormat {
    match n {
        "argb4444" => PixelFormat::Argb4444,
        "rgb555" => PixelFormat::Rgb555,
        "rgb565" => PixelFormat::Rgb565,
        _ => panic!("{n}"),
    }
}

#[test]
fn every_colour_in_every_format() {
    for (name, f) in [("argb4444", PixelFormat::Argb4444), ("rgb555", PixelFormat::Rgb555), ("rgb565", PixelFormat::Rgb565)] {
        let expect = std::fs::read(vectors_dir().join(format!("data/palette-{name}.rgba"))).unwrap();
        assert_eq!(expect.len(), 65536 * 4);
        let mut wrong = Vec::new();
        for c in 0..=65535u32 {
            let got = to_rgba(f, c as u16);
            if got[..] != expect[c as usize * 4..c as usize * 4 + 4] {
                wrong.push(c);
            }
        }
        assert!(wrong.is_empty(), "{name}: {} colours differ, first {:?}", wrong.len(), &wrong[..wrong.len().min(8)]);
    }
}

#[test]
fn vectors() {
    let rows = rows("palette.tsv");
    assert!(rows.len() > 100, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, kind, input, expect) = (&r[0], r[1].as_str(), &r[2], &r[3]);
        let got = match kind {
            "ext" => format_from_extension(std::str::from_utf8(&unhex(input)).unwrap()).map(name_of).unwrap_or("none").to_string(),
            "file" => {
                let (f, h) = input.split_once(':').unwrap();
                match parse_palette(&unhex(h), format_named(f)) {
                    Ok(p) => format!("ok:{:#x},{:#x},{}", p.header.magic, p.header.version, p.colors.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",")),
                    Err(e) => err_code(&e),
                }
            }
            "at" => {
                let parts: Vec<&str> = input.split(':').collect();
                let p = parse_palette(&unhex(parts[1]), format_named(parts[0])).unwrap();
                match p.at(parts[2].parse().unwrap(), parts[3].parse().unwrap()) {
                    Ok(s) => format!("ok:{}", s.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",")),
                    Err(e) => format!("err:badpalette:{}", e.len),
                }
            }
            k => panic!("{k}"),
        };
        if &got != expect {
            failed.push(format!("{name}: got {:.80}, expected {:.80}", got, expect));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}
