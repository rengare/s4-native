mod common;
use common::*;
use s4_formats_clean::texts::{parse, with_fallbacks, COUNT};

#[test]
fn parse_vectors() {
    let rows = rows("texts.tsv");
    assert!(rows.len() > 25, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, input, expect) = (&r[0], bytes_of(&r[1]), &r[2]);
        let got = match parse(&input) {
            Ok(list) => format!("ok:{}:{}", list.len(), list.iter().map(|s| hex(s)).collect::<Vec<_>>().join(",")),
            Err(e) => err_code(&e),
        };
        if &got != expect {
            let short = |s: &str| s.chars().take(80).collect::<String>();
            failed.push(format!("{name}: got {}, expected {}", short(&got), short(expect)));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

#[test]
fn fallback_vectors() {
    let rows = rows("texts-fallback.tsv");
    assert!(rows.len() > 10, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, language, spec, expect) = (&r[0], r[1].parse::<usize>().unwrap(), &r[2], &r[3]);
        let mut files: Vec<Option<Vec<Vec<u8>>>> = Vec::new();
        for part in spec.split(';').filter(|p| !p.is_empty()) {
            let (n, list) = part.split_once('=').unwrap();
            assert_eq!(n.parse::<usize>().unwrap(), files.len());
            if list == "none" {
                files.push(None);
            } else {
                let mut v = vec![Vec::new(); COUNT];
                for item in list.split(',').filter(|i| !i.is_empty()) {
                    let (i, h) = item.split_once(':').unwrap();
                    v[i.parse::<usize>().unwrap()] = unhex(h);
                }
                files.push(Some(v));
            }
        }
        let got = with_fallbacks(language, &files);
        let mut ok = got.len() == COUNT;
        let text = got.iter().enumerate().filter(|(_, s)| !s.is_empty()).map(|(i, s)| format!("{i}:{}", hex(s))).collect::<Vec<_>>().join(",");
        ok &= &text == expect;
        if !ok {
            failed.push(format!("{name}: {} entries; non-empty: {text:.100}", got.len()));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}
