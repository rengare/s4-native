mod common;
use common::*;
use s4_formats_clean::cipher::{apply_default, KeystreamCipher, DEFAULT_KEY};

#[test]
fn vectors() {
    let rows = rows("cipher.tsv");
    assert!(rows.len() > 100, "vectors missing");
    let mut failed = Vec::new();
    for r in &rows {
        let (name, key, input, expect) = (&r[0], unhex(&r[1]), unhex(&r[2]), unhex(&r[3]));
        let mut data = input.clone();
        KeystreamCipher::new(&key).apply(&mut data);
        if data != expect {
            failed.push(name.clone());
        }
    }
    assert!(failed.is_empty(), "{} of {} vectors differ: {:?}", failed.len(), rows.len(), &failed[..failed.len().min(10)]);
}

#[test]
fn long_key_streams() {
    for (file, key) in [("long-default", DEFAULT_KEY.to_vec()), ("long-other", vec![9u8, 8, 7])] {
        let expect = std::fs::read(vectors_dir().join(format!("data/{file}.keystream"))).unwrap();
        let mut data = vec![0u8; expect.len()];
        KeystreamCipher::new(&key).apply(&mut data);
        assert!(data == expect, "{file}: key stream differs");
    }
}

#[test]
fn chunks_continue_the_stream() {
    let mut whole = vec![0u8; 1000];
    KeystreamCipher::new(b"chunky").apply(&mut whole);
    for sizes in [[1usize, 999, 0], [7, 8, 985], [500, 500, 0], [999, 1, 0]] {
        let mut data = vec![0u8; 1000];
        let mut c = KeystreamCipher::new(b"chunky");
        let mut at = 0;
        for n in sizes {
            c.apply(&mut data[at..at + n]);
            at += n;
        }
        assert_eq!(data, whole, "{sizes:?}");
    }
}

#[test]
fn applying_twice_restores_the_data() {
    let original: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
    let mut data = original.clone();
    KeystreamCipher::new(b"some key").apply(&mut data);
    assert_ne!(data, original);
    KeystreamCipher::new(b"some key").apply(&mut data);
    assert_eq!(data, original);
}

#[test]
fn default_helper_uses_the_default_key() {
    let mut a = vec![0u8; 64];
    let mut b = vec![0u8; 64];
    apply_default(&mut a);
    KeystreamCipher::new(DEFAULT_KEY).apply(&mut b);
    assert_eq!(a, b);
}
