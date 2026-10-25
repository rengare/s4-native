//! The C++ exception-handling tables MSVC writes: `FuncInfo` records (magic 0x19930520) in the data
//! sections. They name the unwind funclets, the catch handlers and, through the tiny handler stubs
//! (`mov eax, <FuncInfo>; jmp __CxxFrameHandler`), more functions that no direct call reaches. With
//! these the lifter finds function boundaries without a decompiler's function list.

use crate::pe::Image;
use std::collections::BTreeSet;

#[derive(Default)]
pub struct Eh {
    pub unwind_actions: BTreeSet<u32>,
    pub catch_handlers: BTreeSet<u32>,
    pub stubs: BTreeSet<u32>,
    /// Code addresses of the instruction-pointer-to-state maps: places inside functions, never starts.
    pub ip_map: BTreeSet<u32>,
}

const MAGIC: u32 = 0x1993_0520;

pub fn scan(img: &Image) -> Eh {
    let mut eh = Eh::default();
    let mut infos: BTreeSet<u32> = BTreeSet::new();
    for sec in img.sections.iter().filter(|s| s.flags & 0x2000_0000 == 0) {
        let at = img.base + sec.rva;
        for off in (0..sec.data.len().saturating_sub(31)).step_by(4) {
            if u32::from_le_bytes(sec.data[off..off + 4].try_into().unwrap()) != MAGIC {
                continue;
            }
            let a = at + off as u32;
            let (Some(max_state), Some(p_unwind), Some(n_try), Some(p_try)) = (img.read32(a + 4), img.read32(a + 8), img.read32(a + 12), img.read32(a + 16)) else { continue };
            // A real record keeps its tables in data (never in code) and has states to map.
            let in_data = |p: u32| img.in_image(p) && !img.is_code(p);
            if max_state > 4096 || n_try > 256 || (max_state > 0 && !in_data(p_unwind)) || (n_try > 0 && !in_data(p_try)) {
                continue;
            }
            if max_state == 0 && n_try == 0 {
                continue;
            }
            infos.insert(a);
            if let (Some(n_ip), Some(p_ip)) = (img.read32(a + 20), img.read32(a + 24)) {
                if n_ip <= 65536 && (n_ip == 0 || in_data(p_ip)) {
                    for i in 0..n_ip {
                        if let Some(ip) = img.read32(p_ip + 8 * i) {
                            if img.is_code(ip) {
                                eh.ip_map.insert(ip);
                            }
                        }
                    }
                }
            }
            for i in 0..max_state {
                if let Some(action) = img.read32(p_unwind + 8 * i + 4) {
                    if img.is_code(action) {
                        eh.unwind_actions.insert(action);
                    }
                }
            }
            for i in 0..n_try {
                let entry = p_try + 20 * i; // tryLow, tryHigh, catchHigh, nCatches, pHandlers
                let (Some(n_catch), Some(handlers)) = (img.read32(entry + 12), img.read32(entry + 16)) else { continue };
                if n_catch > 64 || !img.in_image(handlers) {
                    continue;
                }
                for j in 0..n_catch {
                    if let Some(h) = img.read32(handlers + 16 * j + 12) {
                        if img.is_code(h) {
                            eh.catch_handlers.insert(h);
                        }
                    }
                }
            }
        }
    }
    // Handler stubs: b8 <FuncInfo> e9 <rel32>.
    for sec in img.sections.iter().filter(|s| s.flags & 0x2000_0000 != 0) {
        let at = img.base + sec.rva;
        let d = &sec.data;
        for off in 0..d.len().saturating_sub(10) {
            if d[off] == 0xb8 && d[off + 5] == 0xe9 {
                let imm = u32::from_le_bytes(d[off + 1..off + 5].try_into().unwrap());
                if infos.contains(&imm) {
                    eh.stubs.insert(at + off as u32);
                }
            }
        }
    }
    eh
}
