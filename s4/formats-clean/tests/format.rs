mod common;
use common::*;
use s4_formats_clean::format::{entry_number, parse_header, parse_index};

#[test]
fn vectors() {
    let rows = rows("header.tsv");
    assert!(rows.len() > 40, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, kind, input, expect) = (&r[0], r[1].as_str(), &r[2], &r[3]);
        let got = match kind {
            "header" => match parse_header(&unhex(input)) {
                Ok(h) => format!("ok:{:#x},{:#x}", h.magic, h.version),
                Err(e) => err_code(&e),
            },
            "index" => match parse_index(&unhex(input)) {
                Ok(i) => format!("ok:{:#x},{:#x},{}", i.header.magic, i.header.version, i.offsets.iter().map(|o| o.to_string()).collect::<Vec<_>>().join(",")),
                Err(e) => err_code(&e),
            },
            "entry" => match entry_number(input.parse().unwrap()) {
                Some(n) => format!("some:{n}"),
                None => "none".into(),
            },
            other => panic!("unknown kind {other}"),
        };
        if &got != expect {
            failed.push(format!("{name}: got {got}, expected {expect}"));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}
