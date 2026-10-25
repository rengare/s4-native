//! Mapping a PE image into the guest address space.

use s4_x86::Mem;

pub struct Import {
    pub dll: String,
    pub name: String,
    /// Guest address of the import address table slot.
    pub slot: u32,
}

pub struct Module {
    pub name: String,
    pub base: u32,
    pub size: u32,
    pub entry: u32,
    pub imports: Vec<Import>,
    pub exports: Vec<(String, u32)>,
    pub sections: Vec<(String, u32, u32, u32)>,
    /// RVA of the resource directory (0 if none).
    pub rsrc: u32,
}

fn u16_at(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}

fn u32_at(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

fn cstr(b: &[u8], o: usize) -> String {
    let end = b[o..].iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf8_lossy(&b[o..o + end]).into_owned()
}

/// Maps `bytes` (a PE file) at its preferred base into `mem`.
pub fn map(mem: Mem, name: &str, bytes: &[u8]) -> Module {
    let pe = u32_at(bytes, 0x3c);
    let count = u16_at(bytes, pe + 6);
    let optional = u16_at(bytes, pe + 20);
    let opt = pe + 24;
    let entry_rva = u32_at(bytes, opt + 16);
    let base = u32_at(bytes, opt + 28) as u32;
    let size = u32_at(bytes, opt + 56) as u32;
    let headers = u32_at(bytes, opt + 60);
    mem.write_bytes(base, &bytes[..headers]);
    let mut sections = Vec::new();
    for i in 0..count {
        let s = opt + optional + 40 * i;
        let sname = String::from_utf8_lossy(&bytes[s..s + 8]).trim_end_matches('\0').to_string();
        let (vsize, rva, rsize, raw) = (u32_at(bytes, s + 8), u32_at(bytes, s + 12), u32_at(bytes, s + 16), u32_at(bytes, s + 20));
        let n = rsize.min(vsize.max(rsize));
        if raw != 0 {
            mem.write_bytes(base + rva as u32, &bytes[raw..raw + n]);
        }
        sections.push((sname, base + rva as u32, vsize.max(rsize) as u32, 0));
    }
    let rva_to_off = |rva: usize| -> usize {
        for i in 0..count {
            let s = opt + optional + 40 * i;
            let (vsize, va, rsize, raw) = (u32_at(bytes, s + 8), u32_at(bytes, s + 12), u32_at(bytes, s + 16), u32_at(bytes, s + 20));
            if rva >= va && rva < va + vsize.max(rsize) {
                return raw + rva - va;
            }
        }
        if rva < headers { rva } else { panic!("rva {rva:#x} outside the sections") }
    };
    let mut imports = Vec::new();
    let dir = u32_at(bytes, opt + 96 + 8);
    if dir != 0 {
        let mut d = rva_to_off(dir);
        loop {
            let (orig, nm, first) = (u32_at(bytes, d), u32_at(bytes, d + 12), u32_at(bytes, d + 16));
            if nm == 0 {
                break;
            }
            let dll = cstr(bytes, rva_to_off(nm));
            let thunks = rva_to_off(if orig != 0 { orig } else { first });
            let mut i = 0;
            loop {
                let t = u32_at(bytes, thunks + 4 * i);
                if t == 0 {
                    break;
                }
                let label = if t & 0x8000_0000 != 0 { format!("#{}", t & 0xffff) } else { cstr(bytes, rva_to_off(t) + 2) };
                imports.push(Import { dll: dll.clone(), name: label, slot: base + first as u32 + 4 * i as u32 });
                i += 1;
            }
            d += 20;
        }
    }
    let mut exports = Vec::new();
    let edir = u32_at(bytes, opt + 96);
    if edir != 0 {
        let e = rva_to_off(edir);
        let (nfuncs, nnames, funcs, names, ords) = (u32_at(bytes, e + 20), u32_at(bytes, e + 24), u32_at(bytes, e + 28), u32_at(bytes, e + 32), u32_at(bytes, e + 36));
        let _ = nfuncs;
        for i in 0..nnames {
            let n = cstr(bytes, rva_to_off(u32_at(bytes, rva_to_off(names) + 4 * i)));
            let ord = u16_at(bytes, rva_to_off(ords) + 2 * i);
            let f = u32_at(bytes, rva_to_off(funcs) + 4 * ord);
            exports.push((n, base + f as u32));
        }
    }
    Module { name: name.to_string(), base, size, entry: if entry_rva == 0 { 0 } else { base + entry_rva as u32 }, imports, exports, sections, rsrc: u32_at(bytes, opt + 96 + 16) as u32 }
}

/// A resource (`kind`: 2 = bitmap, 3 = icon, 14 = group icon, 12/1 = cursors)
/// by integer id in the mapped image: (guest address, size).
pub fn resource(mem: Mem, base: u32, rsrc_rva: u32, kind: u32, id: u32) -> Option<(u32, u32)> {
    if rsrc_rva == 0 {
        return None;
    }
    let root = base + rsrc_rva;
    // A directory: (named entries, id entries) then 8-byte entries (id, offset).
    let find = |dir: u32, want: u32| -> Option<u32> {
        let (named, ids) = (mem.r16(dir + 12) as u32, mem.r16(dir + 14) as u32);
        for i in 0..named + ids {
            let e = dir + 16 + 8 * i;
            if i >= named && mem.r32(e) == want {
                return Some(mem.r32(e + 4));
            }
        }
        None
    };
    let by_type = find(root, kind)?;
    let by_id = find(root + (by_type & 0x7fff_ffff), id)?;
    // The first language.
    let lang_dir = root + (by_id & 0x7fff_ffff);
    let first = mem.r32(lang_dir + 16 + 4);
    let data = root + first;
    Some((base + mem.r32(data), mem.r32(data + 4)))
}
