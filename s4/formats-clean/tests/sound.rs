mod common;
use common::*;
use s4_formats_clean::sound::{decode_wav, SoundBank, WavError};

#[test]
fn wav_vectors() {
    let rows = rows("wav.tsv");
    assert!(rows.len() > 45, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, wav, expect) = (&r[0], bytes_of(&r[1]), &r[2]);
        let got = match decode_wav(&wav) {
            Ok(p) => {
                let bytes: Vec<u8> = p.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                let want = expect.rsplit(',').next().unwrap();
                let same = expect.starts_with(&format!("ok:{},{},", p.channels, p.rate)) && bytes == bytes_of(want);
                if same { expect.clone() } else { format!("ok:{},{},({} samples)", p.channels, p.rate, p.samples.len()) }
            }
            Err(WavError::NotWav) => "err:notwav".into(),
            Err(WavError::Truncated) => "err:truncated".into(),
            Err(WavError::Unsupported(t)) => format!("err:unsupported:{t}"),
        };
        if &got != expect {
            failed.push(format!("{name}: got {:.70}, expected {:.70}", got, expect));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

#[test]
fn bank_vectors() {
    let rows = rows("sound-bank.tsv");
    assert!(rows.len() > 15, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, snd, sil, expect) = (&r[0], unhex(&r[1]), unhex(&r[2]), &r[3]);
        let got = match SoundBank::parse(&snd, &sil) {
            Err(e) => err_code(&e),
            Ok(b) => {
                assert_eq!(b.is_empty(), b.len() == 0, "{name}: is_empty");
                let mut s = format!("ok:len={}", b.len());
                for n in 0..b.len() + 2 {
                    let e = match b.wav(n) {
                        Ok(None) => "none".to_string(),
                        Ok(Some(w)) => format!("some:{}", hex(w)),
                        Err(s4_formats_clean::FormatError::BadOffset { index, offset }) => format!("err:badoffset:{index}:{offset}"),
                        Err(other) => format!("err:other:{other:?}"),
                    };
                    s.push_str(&format!(";{n}:{e}"));
                }
                s
            }
        };
        if &got != expect {
            failed.push(format!("{name}: got {:.100}, expected {:.100}", got, expect));
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ:\n{}", failed.len(), rows.len(), failed.join("\n"));
}
