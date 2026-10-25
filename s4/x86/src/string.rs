//! String instructions (`movs`, `stos`, `lods`, `scas`, `cmps`) with and
//! without `rep`.

use crate::alu::{self, Fl};
use crate::mem::Mem;

/// Repeat prefix.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Rep {
    None,
    /// `rep` / `repe`: while ECX != 0 (and ZF for scas/cmps).
    E,
    /// `repne`.
    Ne,
}

#[inline(always)]
fn step(bits: u32, df: bool) -> u32 {
    let s = bits / 8;
    if df { s.wrapping_neg() } else { s }
}

/// `movs`.
#[inline(always)]
pub fn movs(m: Mem, bits: u32, rep: Rep, esi: &mut u32, edi: &mut u32, ecx: &mut u32, df: bool) {
    let s = step(bits, df);
    loop {
        if rep != Rep::None {
            if *ecx == 0 {
                return;
            }
        }
        m.w(bits, *edi, m.r(bits, *esi));
        *esi = esi.wrapping_add(s);
        *edi = edi.wrapping_add(s);
        if rep == Rep::None {
            return;
        }
        *ecx = ecx.wrapping_sub(1);
    }
}

/// `stos` (value in al/ax/eax).
#[inline(always)]
pub fn stos(m: Mem, bits: u32, rep: Rep, v: u32, edi: &mut u32, ecx: &mut u32, df: bool) {
    let s = step(bits, df);
    loop {
        if rep != Rep::None {
            if *ecx == 0 {
                return;
            }
        }
        m.w(bits, *edi, v);
        *edi = edi.wrapping_add(s);
        if rep == Rep::None {
            return;
        }
        *ecx = ecx.wrapping_sub(1);
    }
}

/// `lods`: returns the last value loaded (the accumulator keeps its value
/// when a repeated load runs zero times, which the caller handles).
#[inline(always)]
pub fn lods(m: Mem, bits: u32, rep: Rep, acc: u32, esi: &mut u32, ecx: &mut u32, df: bool) -> u32 {
    let s = step(bits, df);
    let mut acc = acc;
    loop {
        if rep != Rep::None {
            if *ecx == 0 {
                return acc;
            }
        }
        acc = m.r(bits, *esi);
        *esi = esi.wrapping_add(s);
        if rep == Rep::None {
            return acc;
        }
        *ecx = ecx.wrapping_sub(1);
    }
}

/// `scas`: compares the accumulator with [edi].
#[inline(always)]
pub fn scas(m: Mem, bits: u32, rep: Rep, acc: u32, edi: &mut u32, ecx: &mut u32, f: &mut Fl) {
    let s = step(bits, f.df);
    loop {
        if rep != Rep::None {
            if *ecx == 0 {
                return;
            }
        }
        alu::sub(bits, acc, m.r(bits, *edi), 0, f);
        *edi = edi.wrapping_add(s);
        if rep == Rep::None {
            return;
        }
        *ecx = ecx.wrapping_sub(1);
        if (rep == Rep::E) != f.zf {
            return;
        }
    }
}

/// `cmps`: compares [esi] with [edi].
#[inline(always)]
pub fn cmps(m: Mem, bits: u32, rep: Rep, esi: &mut u32, edi: &mut u32, ecx: &mut u32, f: &mut Fl) {
    let s = step(bits, f.df);
    loop {
        if rep != Rep::None {
            if *ecx == 0 {
                return;
            }
        }
        alu::sub(bits, m.r(bits, *esi), m.r(bits, *edi), 0, f);
        *esi = esi.wrapping_add(s);
        *edi = edi.wrapping_add(s);
        if rep == Rep::None {
            return;
        }
        *ecx = ecx.wrapping_sub(1);
        if (rep == Rep::E) != f.zf {
            return;
        }
    }
}
