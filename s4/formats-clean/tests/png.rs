mod common;
use common::*;
use s4_formats_clean::png::encode_rgba;

#[test]
fn vectors() {
    let rows = rows("png.tsv");
    assert!(rows.len() >= 14, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, w, h, pixels, expect) = (&r[0], r[1].parse::<u32>().unwrap(), r[2].parse::<u32>().unwrap(), bytes_of(&r[3]), bytes_of(&r[4]));
        let got = encode_rgba(w, h, &pixels);
        if got != expect {
            let at = got.iter().zip(&expect).position(|(a, b)| a != b);
            failed.push(format!("{name}: {} bytes, expected {}, first difference at {:?}", got.len(), expect.len(), at));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}
