mod common;
use common::*;
use s4_formats_clean::archive::{Archive, ReadError};
use s4_formats_clean::compress::DecompressError;

fn entry_text(e: &s4_formats_clean::archive::Entry) -> String {
    format!("{}/{}/{}/{}/{:#x}/{:#x}", hex(e.path.as_bytes()), e.offset, e.stored_size, e.size, e.flags, e.checksum)
}

#[test]
fn parse_vectors() {
    let rows = rows("archive-parse.tsv");
    assert!(rows.len() > 20, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, data, expect) = (&r[0], bytes_of(&r[1]), &r[2]);
        let got = match Archive::parse(&data) {
            Ok(a) => format!("ok:{}", a.entries.iter().map(entry_text).collect::<Vec<_>>().join(";")),
            Err(e) => err_code(&e),
        };
        if &got != expect {
            let short = |s: &str| s.chars().take(100).collect::<String>();
            failed.push(format!("{name}: got {}, expected {}", short(&got), short(expect)));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

#[test]
fn read_vectors() {
    let rows = rows("archive-read.tsv");
    assert!(rows.len() > 40, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, data, index, expect) = (&r[0], bytes_of(&r[1]), r[2].parse::<usize>().unwrap(), &r[3]);
        let archive = Archive::parse(&data).unwrap_or_else(|e| panic!("{name}: the archive must parse: {e:?}"));
        let got = match archive.read(&archive.entries[index]) {
            Ok(b) => {
                let want = bytes_of(expect.strip_prefix("ok:").map(|h| h).unwrap_or(expect));
                if expect.starts_with("ok") && b == want { expect.clone() } else { format!("ok:{} bytes", b.len()) }
            }
            Err(ReadError::Range) => "err:range".into(),
            Err(ReadError::Scrambled) => "err:scrambled".into(),
            Err(ReadError::Decompress(DecompressError::Truncated)) => "err:truncated".into(),
            Err(ReadError::Decompress(DecompressError::Corrupt)) => "err:corrupt".into(),
        };
        if &got != expect {
            failed.push(format!("{name}: got {got}, expected {:.80}", expect));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

#[test]
fn find_vectors() {
    let rows = rows("archive-find.tsv");
    assert!(rows.len() > 200, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, data, query, expect) = (&r[0], bytes_of(&r[1]), String::from_utf8(unhex(&r[2])).unwrap(), &r[3]);
        let archive = Archive::parse(&data).unwrap();
        let got = match archive.find(&query) {
            Some(e) => archive.entries.iter().position(|x| std::ptr::eq(x, e)).expect("find must return an entry of the archive").to_string(),
            None => "none".into(),
        };
        if &got != expect {
            failed.push(format!("{name}: query {query:?}: got {got}, expected {expect}"));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}
