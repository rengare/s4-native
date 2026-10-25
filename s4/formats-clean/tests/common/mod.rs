//! Helpers for reading the vector files.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn vectors_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors")
}

pub fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len() % 2 == 0, "odd hex length");
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

/// The rows of a TSV vector file (comment lines start with `#`), as lists of fields.
pub fn rows(file: &str) -> Vec<Vec<String>> {
    let text = std::fs::read_to_string(vectors_dir().join(file)).unwrap();
    text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| l.split('\t').map(str::to_string).collect()).collect()
}

/// A field that is either hex or `file:<path>` / `ok-file:<path>` relative to `vectors/`.
pub fn bytes_of(field: &str) -> Vec<u8> {
    for prefix in ["file:", "ok-file:"] {
        if let Some(path) = field.strip_prefix(prefix) {
            return std::fs::read(vectors_dir().join(path)).unwrap();
        }
    }
    unhex(field)
}

pub fn err_code(e: &s4_formats_clean::FormatError) -> String {
    match e {
        s4_formats_clean::FormatError::BadMagic(m) => format!("err:badmagic:{m:#x}"),
        s4_formats_clean::FormatError::Truncated { offset } => format!("err:truncated:{offset}"),
        s4_formats_clean::FormatError::BadOffset { index, offset } => format!("err:badoffset:{index}:{offset}"),
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
