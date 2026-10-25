mod common;
use common::*;
use s4_formats_clean::compress::{decompress, DecompressError};

fn expected(result: &str) -> Result<Vec<u8>, DecompressError> {
    match result {
        "err:truncated" => Err(DecompressError::Truncated),
        "err:corrupt" => Err(DecompressError::Corrupt),
        r => match r.strip_prefix("ok:") {
            Some(hex) => Ok(unhex(hex)),
            None => Ok(bytes_of(r)), // ok-file:<path>
        },
    }
}

#[test]
fn vectors() {
    let rows = rows("compress.tsv");
    assert!(rows.len() > 40, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, len, stream, result) = (&r[0], r[1].parse::<usize>().unwrap(), bytes_of(&r[2]), &r[3]);
        let want = expected(result);
        let got = decompress(&stream, len);
        if got != want {
            failed.push(format!("{name}: {}", r[4]));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

/// Damaged input must give a result or an error, never a panic or an endless loop.
#[test]
fn damaged_streams_do_not_panic() {
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for r in rows("compress.tsv").iter().filter(|r| r[3].starts_with("ok")) {
        let stream = bytes_of(&r[2]);
        let len = r[1].parse::<usize>().unwrap();
        if stream.is_empty() {
            continue;
        }
        for _ in 0..40 {
            let mut s = stream.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() % s.len() as u64) as usize;
                s[i] ^= 1 << (next() % 8);
            }
            let _ = decompress(&s, len);
            let cut = (next() % s.len() as u64) as usize;
            let _ = decompress(&s[..cut], len);
        }
    }
}
