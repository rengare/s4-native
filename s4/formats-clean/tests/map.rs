mod common;
use common::*;
use s4_formats_clean::compress::DecompressError;
use s4_formats_clean::map::{checksum, crc16_swapped, ChunkHeader, MapFile};

#[test]
fn kind_and_index_split_the_type_word() {
    for w in [0u32, 1, 0xffff, 0x10000, 0x0003_000d, 0xffff_ffff, 0x8000_8000] {
        let h = ChunkHeader { kind: w, stored: 0, size: 0, checksum: 0, param: 0 };
        assert_eq!((h.kind(), h.index()), (w as u16, (w >> 16) as u16), "{w:#x}");
    }
}

#[test]
fn checksum_vectors() {
    let rows = rows("map-checksum.tsv");
    assert!(rows.len() >= 20, "vectors missing");
    for r in &rows {
        let data = unhex(&r[1]);
        assert_eq!(crc16_swapped(&data).to_string(), r[2], "{}: crc16_swapped", r[0]);
        assert_eq!(checksum(&data).to_string(), r[3], "{}: checksum", r[0]);
    }
}

fn list<T: ToString>(v: Vec<T>) -> String {
    if v.is_empty() { "-".to_string() } else { v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(";") }
}

#[test]
fn vectors() {
    let rows = rows("map.tsv");
    assert!(rows.len() >= 50, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, data) = (&r[0], bytes_of(&r[1]));
        let expect = r[2..8].join("\t");
        let want_data = r[7].clone();
        let got = match MapFile::parse(&data) {
            Err(e) => format!("{}\t-\t-\t-\t-\t-", err_code(&e)),
            Ok(m) => {
                let chunks: Vec<String> = m
                    .chunks
                    .iter()
                    .map(|c| {
                        let start = c.stored.as_ptr() as usize - data.as_ptr() as usize;
                        format!("{},{},{},{},{},{}", c.header.kind, c.header.stored, c.header.size, c.header.checksum, c.header.param, start)
                    })
                    .collect();
                let parse = format!("ok:{},{}:{}", m.checksum, m.version, chunks.join(";"));
                let size = m.size().map_or("none".to_string(), |s| s.to_string());
                let settlers = list(m.settlers().iter().map(|s| format!("{},{},{},{}", s.x, s.y, s.kind, s.player)).collect());
                let buildings = list(m.buildings().iter().map(|b| format!("{},{},{},{},{},{}", b.x, b.y, b.kind, b.player, b.occupied, b.soldiers.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("."))).collect());
                let piles = list(m.piles().iter().map(|p| format!("{},{},{},{},{}", p.x, p.y, p.good, p.amount, p.flag)).collect());
                let datas = list(
                    m.chunks
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            let want = want_data.split(';').nth(i).unwrap_or("");
                            match c.data() {
                                Ok(bytes) => match want.strip_prefix("ok:file:") {
                                    Some(path) if std::fs::read(vectors_dir().join(path)).unwrap() == bytes => want.to_string(),
                                    Some(_) => format!("ok:(different {} bytes)", bytes.len()),
                                    None => format!("ok:{}", hex(&bytes)),
                                },
                                Err(DecompressError::Truncated) => "err:truncated".to_string(),
                                Err(DecompressError::Corrupt) => "err:corrupt".to_string(),
                            }
                        })
                        .collect(),
                );
                format!("{parse}\t{size}\t{settlers}\t{buildings}\t{piles}\t{datas}")
            }
        };
        if got != expect {
            failed.push(format!("{name}:\n  got      {:.200}\n  expected {:.200}", got, expect));
        }
        assert_eq!(checksum(&data).to_string(), r[8], "{name}: checksum of the file");
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

#[test]
fn never_panics_on_damaged_files() {
    for r in rows("map.tsv") {
        let d = bytes_of(&r[1]);
        let attempt = |d: &[u8]| {
            if let Ok(m) = MapFile::parse(d) {
                let _ = (m.size(), m.settlers(), m.buildings(), m.piles());
                for c in &m.chunks {
                    let _ = c.data();
                }
            }
        };
        for cut in (0..d.len().min(3000)).step_by(11).chain([d.len().saturating_sub(1)]) {
            attempt(&d[..cut]);
        }
        for i in (0..d.len().min(400)).step_by(3) {
            let mut e = d.clone();
            e[i] ^= 0x5a;
            attempt(&e);
        }
    }
}
