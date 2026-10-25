//! A PE image as the loader maps it, for reading code and tables.

pub struct Section {
    pub rva: u32,
    pub vsize: u32,
    pub data: Vec<u8>,
    pub flags: u32,
    pub name: String,
}

pub struct Image {
    pub base: u32,
    pub sections: Vec<Section>,
    /// Import address table slots: address -> "dll!name".
    pub imports: std::collections::BTreeMap<u32, String>,
    /// Addresses of the 32-bit absolute pointers the loader would relocate.
    pub relocs: Vec<u32>,
    /// Exported functions (name, address) and the entry point.
    pub exports: Vec<(String, u32)>,
    pub entry: u32,
}

fn u16_at(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}

fn u32_at(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

impl Image {
    pub fn empty() -> Image {
        Image { base: 0, sections: Vec::new(), imports: Default::default(), relocs: Vec::new(), exports: Vec::new(), entry: 0 }
    }

    pub fn load(path: &str) -> Image {
        let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let pe = u32_at(&b, 0x3c);
        let count = u16_at(&b, pe + 6);
        let optional = u16_at(&b, pe + 20);
        let base = u32_at(&b, pe + 24 + 28) as u32;
        let mut sections = Vec::new();
        for i in 0..count {
            let s = pe + 24 + optional + 40 * i;
            let name = String::from_utf8_lossy(&b[s..s + 8]).trim_end_matches('\0').to_string();
            let (vsize, rva, rsize, raw, flags) = (u32_at(&b, s + 8), u32_at(&b, s + 12), u32_at(&b, s + 16), u32_at(&b, s + 20), u32_at(&b, s + 36));
            let n = rsize.min(vsize);
            let mut data = if raw == 0 { Vec::new() } else { b[raw..raw + n].to_vec() };
            data.resize(vsize.max(rsize), 0);
            sections.push(Section { rva: rva as u32, vsize: vsize as u32, data, flags: flags as u32, name });
        }
        let mut img = Image { base, sections, imports: Default::default(), relocs: Vec::new(), exports: Vec::new(), entry: 0 };
        // Import directory: data directory 1.
        let dir = u32_at(&b, pe + 24 + 96 + 8);
        if dir != 0 {
            let mut d = dir;
            loop {
                let (orig, name, first) = match (img.read32(base + d as u32), img.read32(base + d as u32 + 12), img.read32(base + d as u32 + 16)) {
                    (Some(o), Some(n), Some(f)) => (o, n, f),
                    _ => break,
                };
                if name == 0 {
                    break;
                }
                let dll = img.cstr(base + name).unwrap_or_default();
                let thunks = if orig != 0 { orig } else { first };
                let mut i = 0;
                loop {
                    let t = match img.read32(base + thunks + 4 * i) {
                        Some(0) | None => break,
                        Some(t) => t,
                    };
                    let label = if t & 0x8000_0000 != 0 {
                        format!("{dll}!#{}", t & 0xffff)
                    } else {
                        format!("{dll}!{}", img.cstr(base + t + 2).unwrap_or_default())
                    };
                    img.imports.insert(base + first + 4 * i, label);
                    i += 1;
                }
                d += 20;
            }
        }
        img.entry = u32_at(&b, pe + 24 + 16) as u32;
        if img.entry != 0 {
            img.entry += base;
        }
        // Exports (data directory 0).
        let edir = u32_at(&b, pe + 24 + 96);
        if edir != 0 {
            let e = base + edir as u32;
            let rd = |a: u32| img.read32(a).unwrap_or(0);
            let (nnames, funcs, names, ords) = (rd(e + 24), rd(e + 28), rd(e + 32), rd(e + 36));
            let mut list = Vec::new();
            for i in 0..nnames {
                let n = img.cstr(base + rd(base + names + 4 * i)).unwrap_or_default();
                let ord = (img.read8(base + ords + 2 * i).unwrap_or(0) as u32) | (img.read8(base + ords + 2 * i + 1).unwrap_or(0) as u32) << 8;
                list.push((n, base + rd(base + funcs + 4 * ord)));
            }
            img.exports = list;
        }
        // Base relocations (data directory 5).
        let (rdir, rsize) = (u32_at(&b, pe + 24 + 96 + 40), u32_at(&b, pe + 24 + 96 + 44));
        if rdir != 0 {
            let mut off = 0;
            while off + 8 <= rsize {
                let (page, block) = (img.read32(base + rdir as u32 + off as u32).unwrap_or(0), img.read32(base + rdir as u32 + off as u32 + 4).unwrap_or(0) as usize);
                if block < 8 {
                    break;
                }
                for i in 0..(block - 8) / 2 {
                    let a = base + rdir as u32 + off as u32 + 8 + 2 * i as u32;
                    let e = img.read8(a).unwrap_or(0) as u32 | (img.read8(a + 1).unwrap_or(0) as u32) << 8;
                    if e >> 12 == 3 {
                        img.relocs.push(base + page + (e & 0xfff));
                    }
                }
                off += block;
            }
        }
        img
    }

    fn find(&self, addr: u32) -> Option<(&Section, usize)> {
        let rva = addr.checked_sub(self.base)?;
        self.sections.iter().find(|s| rva >= s.rva && rva < s.rva + s.data.len() as u32).map(|s| (s, (rva - s.rva) as usize))
    }

    /// Replaces the bytes at `addr` (in the image as mapped) with `new`, after checking that they are `old`.
    pub fn patch(&mut self, addr: u32, old: &[u8], new: &[u8]) -> Result<(), String> {
        assert_eq!(old.len(), new.len(), "a patch keeps the length");
        let rva = addr.checked_sub(self.base).ok_or("address below the image base")?;
        let s = self.sections.iter_mut().find(|s| rva >= s.rva && rva < s.rva + s.data.len() as u32).ok_or("address outside the sections")?;
        let o = (rva - s.rva) as usize;
        let cur = s.data.get(o..o + old.len()).ok_or("patch runs past the section")?;
        if cur != old {
            return Err(format!("bytes at {addr:#x} are {cur:02x?}, expected {old:02x?}"));
        }
        s.data[o..o + new.len()].copy_from_slice(new);
        Ok(())
    }

    pub fn read32(&self, addr: u32) -> Option<u32> {
        let (s, o) = self.find(addr)?;
        let b = s.data.get(o..o + 4)?;
        Some(u32::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn read8(&self, addr: u32) -> Option<u8> {
        let (s, o) = self.find(addr)?;
        s.data.get(o).copied()
    }

    pub fn cstr(&self, addr: u32) -> Option<String> {
        let (s, o) = self.find(addr)?;
        let end = s.data[o..].iter().position(|&c| c == 0)?;
        Some(String::from_utf8_lossy(&s.data[o..o + end]).into_owned())
    }

    /// Code bytes from `addr` to the end of its section.
    pub fn code_from(&self, addr: u32) -> Option<&[u8]> {
        let (s, o) = self.find(addr)?;
        Some(&s.data[o..])
    }

    pub fn in_image(&self, addr: u32) -> bool {
        self.find(addr).is_some()
    }

    pub fn is_code(&self, addr: u32) -> bool {
        self.find(addr).map(|(s, _)| s.flags & 0x2000_0000 != 0).unwrap_or(false)
    }

    pub fn section_range(&self, addr: u32) -> Option<(u32, u32)> {
        let (s, _) = self.find(addr)?;
        Some((self.base + s.rva, self.base + s.rva + s.data.len() as u32))
    }
}
