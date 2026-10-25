mod common;
use common::*;
use s4_formats_clean::dialogs::{parse, CONTROL_SIZE};

#[test]
fn constants() {
    assert_eq!(CONTROL_SIZE, 36);
}

#[test]
fn vectors() {
    let rows = rows("gui-set.tsv");
    assert!(rows.len() >= 30, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, data, expect) = (&r[0], bytes_of(&r[1]), &r[2]);
        let got = match parse(&data) {
            Err(e) => err_code(&e),
            Ok(ds) => {
                let ds: Vec<String> = ds
                    .iter()
                    .map(|g| {
                        let cs: Vec<String> = g.controls.iter().map(|c| format!("{},{},{},{},{}", c.id, c.kind, c.text_slot, c.flags, hex(c.raw))).collect();
                        format!("{},{},{},{},{},{},{}:{}", g.surface, g.x, g.y, g.width, g.height, g.background, g.flags, cs.join(";"))
                    })
                    .collect();
                format!("ok:{}", ds.join("|"))
            }
        };
        if &got != expect {
            failed.push(format!("{name}: got {:.90}, expected {:.90}", got, expect));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

#[test]
fn never_panics_on_damaged_files() {
    for r in rows("gui-set.tsv") {
        let d = bytes_of(&r[1]);
        for cut in (0..d.len()).step_by(7).chain([d.len().saturating_sub(1)]) {
            let _ = parse(&d[..cut]);
        }
        for i in (0..d.len().min(200)).step_by(3) {
            let mut e = d.clone();
            e[i] ^= 0xff;
            let _ = parse(&e);
        }
    }
}
