mod common;
use common::*;
use s4_formats_clean::landscape::{LandscapeFile, Record};

#[test]
fn software_flag() {
    for flags in 0..=0xffffu32 {
        let r = Record { flags: flags as u16, pages: 1, data: &[] };
        assert_eq!(r.is_software(), flags & 2 != 0, "flags {flags:#x}");
    }
}

#[test]
fn vectors() {
    let rows = rows("landscape.tsv");
    assert!(rows.len() >= 28, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, data) = (&r[0], bytes_of(&r[1]));
        let (parse_expect, pages_expect) = (&r[2], &r[3]);
        let (parse_got, pages_got) = match LandscapeFile::parse(&data) {
            Err(e) => (err_code(&e), "none".to_string()),
            Ok(f) => {
                let recs: Vec<String> = f
                    .records
                    .iter()
                    .map(|r| {
                        let start = r.data.as_ptr() as usize - data.as_ptr() as usize;
                        format!("{},{},{},{}", r.flags, r.pages, start, r.data.len())
                    })
                    .collect();
                let pages = match f.hardware_pages() {
                    None => "none".to_string(),
                    Some((side, ps)) => {
                        assert!(ps.iter().all(|p| p.len() == side * side), "{name}: page sizes");
                        let bytes: Vec<u8> = ps.iter().flatten().flat_map(|c| c.to_le_bytes()).collect();
                        let want = pages_expect.rsplit(',').next().unwrap();
                        let same = pages_expect.starts_with(&format!("ok:{side},{},", ps.len())) && bytes == bytes_of(want);
                        if same { pages_expect.clone() } else { format!("ok:{side},{} pages ({} bytes)", ps.len(), bytes.len()) }
                    }
                };
                (format!("ok:{:#x},{}:{}", f.header.magic, f.header.version, recs.join(";")), pages)
            }
        };
        if &parse_got != parse_expect || &pages_got != pages_expect {
            failed.push(format!("{name}: parse {:.80} / pages {:.60}; expected {:.80} / {:.60}", parse_got, pages_got, parse_expect, pages_expect));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

#[test]
fn never_panics_on_damaged_files() {
    for r in rows("landscape.tsv") {
        let d = bytes_of(&r[1]);
        for cut in (0..d.len().min(3000)).step_by(5).chain([d.len().saturating_sub(1)]) {
            if let Ok(f) = LandscapeFile::parse(&d[..cut]) {
                let _ = f.hardware_pages();
            }
        }
        for i in (0..d.len().min(120)).step_by(2) {
            let mut e = d.clone();
            e[i] ^= 0xff;
            if let Ok(f) = LandscapeFile::parse(&e) {
                let _ = f.hardware_pages();
            }
        }
    }
}
