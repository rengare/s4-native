//! A small Windows registry for games that keep their settings there (The Settlers III). Off until [`enable`] is
//! called (The Settlers IV reads none: its registry calls fail and it falls back to defaults). Values are kept in
//! `<data dir>/registry.txt`; the first run seeds them from the game's own `regs.cmd` (the `REG ADD` lines its
//! installer would have run).

use crate::Shim;
use s4_x86::Cpu;
use std::collections::BTreeMap;
use std::sync::Mutex;

macro_rules! sh {
    ($name:expr, $args:expr, $f:expr) => {
        Shim { dll: "advapi32.dll", name: $name, args: $args, cdecl: false, f: $f }
    };
}

const REG_SZ: u32 = 1;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;

#[derive(Clone)]
struct Value {
    name: String,
    kind: u32,
    data: Vec<u8>,
}

#[derive(Default)]
struct Registry {
    /// Lower-cased key path -> (original-case path, values by lower-cased name).
    keys: BTreeMap<String, (String, BTreeMap<String, Value>)>,
    /// Open handles -> key path (lower-cased).
    handles: std::collections::HashMap<u32, String>,
    next: u32,
    file: Option<std::path::PathBuf>,
}

static REG: Mutex<Option<Registry>> = Mutex::new(None);

fn norm_path(p: &str) -> String {
    p.split('\\').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\\")
}

fn root_name(h: u32) -> Option<&'static str> {
    match h {
        0x8000_0000 => Some("HKCR"),
        0x8000_0001 => Some("HKCU"),
        0x8000_0002 => Some("HKLM"),
        _ => None,
    }
}

impl Registry {
    fn path_of(&self, h: u32, sub: &str) -> Option<String> {
        let base = match root_name(h) {
            Some(r) => r.to_string(),
            None => self.handles.get(&h)?.clone(),
        };
        let sub = norm_path(sub);
        Some(if sub.is_empty() { base } else { format!("{base}\\{sub}") })
    }

    fn make_key(&mut self, path: &str) {
        let mut cur = String::new();
        for part in path.split('\\') {
            if !cur.is_empty() {
                cur.push('\\');
            }
            cur.push_str(part);
            self.keys.entry(cur.to_lowercase()).or_insert_with(|| (cur.clone(), BTreeMap::new()));
        }
    }

    fn open(&mut self, path: &str) -> u32 {
        self.next += 1;
        let h = 0x1000 + self.next * 4;
        self.handles.insert(h, path.to_lowercase());
        h
    }

    fn save(&self) {
        let Some(f) = &self.file else { return };
        let mut out = String::new();
        for (orig, vals) in self.keys.values() {
            out.push_str(&format!("[{orig}]\n"));
            for v in vals.values() {
                let hex: String = v.data.iter().map(|b| format!("{b:02x}")).collect();
                out.push_str(&format!("{}\t{}\t{hex}\n", v.name.replace('\t', " "), v.kind));
            }
        }
        let _ = std::fs::write(f, out);
    }

    fn load(&mut self, text: &str) {
        let mut cur: Option<String> = None;
        for line in text.lines() {
            if let Some(p) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                self.make_key(p);
                cur = Some(p.to_lowercase());
            } else if let Some(k) = &cur {
                let mut it = line.splitn(3, '\t');
                if let (Some(n), Some(t), Some(h)) = (it.next(), it.next(), it.next()) {
                    let data = (0..h.len() / 2).filter_map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok()).collect();
                    self.keys.get_mut(k).unwrap().1.insert(n.to_lowercase(), Value { name: n.to_string(), kind: t.parse().unwrap_or(REG_SZ), data });
                }
            }
        }
    }

    fn set(&mut self, path: &str, name: &str, kind: u32, data: Vec<u8>) {
        self.make_key(path);
        self.keys.get_mut(&path.to_lowercase()).unwrap().1.insert(name.to_lowercase(), Value { name: name.to_string(), kind, data });
    }
}

/// Turns the registry on: loads `file` if it exists, else seeds it from `<game_dir>/regs.cmd`. `root` is how the
/// guest names the game folder (`C:\S3`), substituted for `%locpath%` in the seed.
pub fn enable(file: std::path::PathBuf, game_dir: &std::path::Path, root: &str) {
    let mut r = Registry { next: 0, file: Some(file.clone()), ..Default::default() };
    if let Ok(text) = std::fs::read_to_string(&file) {
        r.load(&text);
    } else if let Ok(text) = std::fs::read_to_string(game_dir.join("regs.cmd")) {
        seed(&mut r, &text, root);
        // The Settlers III keeps a random number here; when the key is missing it forgets the serial number it has.
        r.set("HKCR\\CLSID\\{0e9f8400-d9a5-11cf-8f4d00804880b189}", "Seed", REG_DWORD, 0x1234u32.to_le_bytes().to_vec());
        r.save();
    }
    *REG.lock().unwrap() = Some(r);
}

/// Sets a DWORD value in the registry (when it is on), creating the key if need be.
pub fn set_dword(path: &str, name: &str, value: u32) {
    with(|r| {
        r.set(path, name, REG_DWORD, value.to_le_bytes().to_vec());
        r.save();
    });
}

/// `REG ADD "<key>" /v "<name>" /t <TYPE> /d <data> /f` lines (and `/ve` for the default value).
fn seed(r: &mut Registry, text: &str, root: &str) {
    let text = text.replace("%locpath%", root);
    let mut vars: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let mut line = line.trim().to_string();
        // SET name=value (batch variables such as %regpath%).
        if line.len() > 4 && line[..4].eq_ignore_ascii_case("set ") {
            if let Some((n, v)) = line[4..].split_once('=') {
                vars.push((format!("%{}%", n.trim()), v.trim().trim_matches('"').to_string()));
            }
            continue;
        }
        for (n, v) in &vars {
            line = line.replace(n.as_str(), &format!("\"{v}\""));
        }
        let line = line.as_str();
        if !line.to_ascii_uppercase().starts_with("REG ADD ") {
            continue;
        }
        let Some(rest) = line.get(8..) else { continue };
        let toks = tokens(rest);
        let Some(key) = toks.first() else { continue };
        let key = key.replacen("HKEY_LOCAL_MACHINE", "HKLM", 1).replacen("HKEY_CURRENT_USER", "HKCU", 1);
        let (mut name, mut kind, mut data) = (String::new(), "REG_SZ".to_string(), String::new());
        let mut i = 1;
        while i < toks.len() {
            match toks[i].to_ascii_lowercase().as_str() {
                "/v" => { name = toks.get(i + 1).cloned().unwrap_or_default(); i += 1; }
                "/t" => { kind = toks.get(i + 1).cloned().unwrap_or_default(); i += 1; }
                "/d" => { data = toks.get(i + 1).cloned().unwrap_or_default(); i += 1; }
                _ => {}
            }
            i += 1;
        }
        let key = norm_path(&key);
        let (k, bytes) = match kind.to_ascii_uppercase().as_str() {
            "REG_DWORD" => (REG_DWORD, (data.parse::<i64>().unwrap_or(0) as u32).to_le_bytes().to_vec()),
            "REG_BINARY" => (REG_BINARY, (0..data.len() / 2).filter_map(|i| u8::from_str_radix(&data[i * 2..i * 2 + 2], 16).ok()).collect()),
            _ => (REG_SZ, data.replace("\\\\", "\\").into_bytes().into_iter().chain([0]).collect()),
        };
        r.set(&key, &name, k, bytes);
    }
}

/// Splits on spaces, keeping "quoted text" together (quotes removed).
fn tokens(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut q) = (Vec::new(), String::new(), false);
    let mut seen = false;
    for ch in s.chars() {
        match ch {
            '"' => { q = !q; seen = true; }
            ' ' if !q => {
                if seen || !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
                seen = false;
            }
            _ => cur.push(ch),
        }
    }
    if seen || !cur.is_empty() { out.push(cur); }
    out
}

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> Option<R> {
    REG.lock().unwrap().as_mut().map(f)
}

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NO_MORE_ITEMS: u32 = 259;

fn cs(c: &Cpu, p: u32) -> String {
    if p == 0 { String::new() } else { crate::kernel32::cstr(c, p) }
}

pub fn all() -> Vec<Shim> {
    vec![
        sh!("RegOpenKeyExA", 5, |c, a| {
            let sub = cs(c, a[1]);
            if std::env::var_os("S4_REG_TRACE").is_some() {
                eprintln!("RegOpenKeyEx({:#x}, {sub:?})", a[0]);
            }
            with(|r| {
                let Some(p) = r.path_of(a[0], &sub) else { return ERROR_FILE_NOT_FOUND };
                if !r.keys.contains_key(&p.to_lowercase()) {
                    return ERROR_FILE_NOT_FOUND;
                }
                let h = r.open(&p);
                c.mem.w32(a[4], h);
                0
            })
            .unwrap_or(ERROR_FILE_NOT_FOUND)
        }),
        // (key, subkey, reserved, class, options, sam, security, result, disposition)
        sh!("RegCreateKeyExA", 9, |c, a| {
            let sub = cs(c, a[1]);
            with(|r| {
                let Some(p) = r.path_of(a[0], &sub) else { return ERROR_FILE_NOT_FOUND };
                let existed = r.keys.contains_key(&p.to_lowercase());
                r.make_key(&p);
                let h = r.open(&p);
                c.mem.w32(a[7], h);
                if a[8] != 0 {
                    c.mem.w32(a[8], if existed { 2 } else { 1 });
                }
                r.save();
                0
            })
            .unwrap_or(ERROR_ACCESS_DENIED)
        }),
        sh!("RegCloseKey", 1, |_, a| {
            with(|r| r.handles.remove(&a[0]));
            0
        }),
        // (key, name, reserved, type*, data*, size*)
        sh!("RegQueryValueExA", 6, |c, a| {
            let name = cs(c, a[1]).to_lowercase();
            if std::env::var_os("S4_REG_TRACE").is_some() {
                eprintln!("RegQueryValueEx({:#x}, {name:?}, size {})", a[0], if a[5] != 0 { c.mem.r32(a[5]) } else { 0 });
            }
            with(|r| {
                let Some(path) = r.path_of(a[0], "") else { return ERROR_FILE_NOT_FOUND };
                let Some(v) = r.keys.get(&path.to_lowercase()).and_then(|k| k.1.get(&name)).cloned() else { return ERROR_FILE_NOT_FOUND };
                if a[3] != 0 {
                    c.mem.w32(a[3], v.kind);
                }
                if a[5] != 0 {
                    let cap = if a[4] != 0 { c.mem.r32(a[5]) as usize } else { 0 };
                    c.mem.w32(a[5], v.data.len() as u32);
                    if a[4] != 0 {
                        if cap < v.data.len() {
                            return ERROR_MORE_DATA;
                        }
                        c.mem.write_bytes(a[4], &v.data);
                    }
                }
                0
            })
            .unwrap_or(ERROR_FILE_NOT_FOUND)
        }),
        // (key, name, reserved, type, data, size)
        sh!("RegSetValueExA", 6, |c, a| {
            let name = cs(c, a[1]);
            if std::env::var_os("S4_REG_TRACE").is_some() {
                eprintln!("RegSetValueEx({:#x}, {name:?}, type {}, {} bytes: {:?})", a[0], a[3], a[5], String::from_utf8_lossy(c.mem.view(a[4], (a[5] as usize).min(64))));
            }
            let data = c.mem.view(a[4], a[5] as usize).to_vec();
            with(|r| {
                let Some(path) = r.path_of(a[0], "") else { return ERROR_FILE_NOT_FOUND };
                r.set(&path, &name, a[3], data);
                r.save();
                0
            })
            .unwrap_or(ERROR_ACCESS_DENIED)
        }),
        sh!("RegDeleteKeyA", 2, |c, a| {
            let sub = cs(c, a[1]);
            with(|r| {
                let Some(p) = r.path_of(a[0], &sub) else { return ERROR_FILE_NOT_FOUND };
                let pl = p.to_lowercase();
                let prefix = format!("{pl}\\");
                let gone: Vec<String> = r.keys.keys().filter(|k| **k == pl || k.starts_with(&prefix)).cloned().collect();
                if gone.is_empty() {
                    return ERROR_FILE_NOT_FOUND;
                }
                for k in gone {
                    r.keys.remove(&k);
                }
                r.save();
                0
            })
            .unwrap_or(ERROR_ACCESS_DENIED)
        }),
        // (key, index, name*, namelen*, reserved, class*, classlen*, filetime*)
        sh!("RegEnumKeyExA", 8, |c, a| {
            with(|r| {
                let Some(path) = r.path_of(a[0], "") else { return ERROR_NO_MORE_ITEMS };
                let prefix = format!("{}\\", path.to_lowercase());
                let kids: Vec<&String> = r.keys.iter().filter(|(k, _)| k.strip_prefix(&prefix).is_some_and(|rest| !rest.contains('\\'))).map(|(_, v)| &v.0).collect();
                let Some(full) = kids.get(a[1] as usize) else { return ERROR_NO_MORE_ITEMS };
                let leaf = full.rsplit('\\').next().unwrap_or("");
                if c.mem.r32(a[3]) as usize <= leaf.len() {
                    return ERROR_MORE_DATA;
                }
                c.mem.write_bytes(a[2], leaf.as_bytes());
                c.mem.w8(a[2] + leaf.len() as u32, 0);
                c.mem.w32(a[3], leaf.len() as u32);
                0
            })
            .unwrap_or(ERROR_NO_MORE_ITEMS)
        }),
        // (key, index, name*, namelen*, reserved, type*, data*, datalen*)
        sh!("RegEnumValueA", 8, |c, a| {
            with(|r| {
                let Some(path) = r.path_of(a[0], "") else { return ERROR_NO_MORE_ITEMS };
                let Some(v) = r.keys.get(&path.to_lowercase()).and_then(|k| k.1.values().nth(a[1] as usize)).cloned() else { return ERROR_NO_MORE_ITEMS };
                if c.mem.r32(a[3]) as usize <= v.name.len() {
                    return ERROR_MORE_DATA;
                }
                c.mem.write_bytes(a[2], v.name.as_bytes());
                c.mem.w8(a[2] + v.name.len() as u32, 0);
                c.mem.w32(a[3], v.name.len() as u32);
                if a[5] != 0 {
                    c.mem.w32(a[5], v.kind);
                }
                if a[7] != 0 {
                    let cap = if a[6] != 0 { c.mem.r32(a[7]) as usize } else { usize::MAX };
                    c.mem.w32(a[7], v.data.len() as u32);
                    if a[6] != 0 {
                        if cap < v.data.len() {
                            return ERROR_MORE_DATA;
                        }
                        c.mem.write_bytes(a[6], &v.data);
                    }
                }
                0
            })
            .unwrap_or(ERROR_NO_MORE_ITEMS)
        }),
        // (key, class*, classlen*, reserved, subkeys*, maxsubkeylen*, maxclasslen*, values*, maxvaluename*, maxvaluedata*, secdesc*, filetime*)
        sh!("RegQueryInfoKeyA", 12, |c, a| {
            with(|r| {
                let Some(path) = r.path_of(a[0], "") else { return ERROR_FILE_NOT_FOUND };
                let prefix = format!("{}\\", path.to_lowercase());
                let kids: Vec<usize> = r.keys.iter().filter(|(k, _)| k.strip_prefix(&prefix).is_some_and(|rest| !rest.contains('\\'))).map(|(_, v)| v.0.rsplit('\\').next().map_or(0, str::len)).collect();
                let vals: Vec<(usize, usize)> = r.keys.get(&path.to_lowercase()).map(|k| k.1.values().map(|v| (v.name.len(), v.data.len())).collect()).unwrap_or_default();
                let put = |p: u32, v: usize| if p != 0 { c.mem.w32(p, v as u32) };
                put(a[4], kids.len());
                put(a[5], kids.iter().copied().max().unwrap_or(0));
                put(a[6], 0);
                put(a[7], vals.len());
                put(a[8], vals.iter().map(|v| v.0).max().unwrap_or(0));
                put(a[9], vals.iter().map(|v| v.1).max().unwrap_or(0));
                0
            })
            .unwrap_or(ERROR_FILE_NOT_FOUND)
        }),
    ]
}

/// True when the registry is on (the older "no registry" stubs then stay out of the way).
pub fn enabled() -> bool {
    REG.lock().unwrap().is_some()
}
