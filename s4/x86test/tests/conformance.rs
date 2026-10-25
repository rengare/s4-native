//! Every instruction form of the game's code (`src/cases.rs`, made by
//! `tools/test_x86.sh`) runs on the processor and as lifted Rust from the same
//! random state; registers, defined flags, x87 state and the memory
//! window must agree. Run under Wine on an x86 host.
#![cfg(all(windows, target_arch = "x86"))]
#![allow(dead_code)]

#[path = "../src/cases.rs"]
mod cases;

use cases::{Case, CASES};
use s4_x86::alu::Fl;
use s4_x86::native::execute;
use s4_x86::{Cpu, Mem};
use s4_x87::Value;
use std::panic::{catch_unwind, AssertUnwindSafe};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const REGION: usize = 16384;
const EDGES: [u32; 18] = [0, 1, 2, 3, 0x7f, 0x80, 0xff, 0x100, 0x7fff, 0x8000, 0xffff, 0x10000, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 0xffff_fffe, 0x5555_5555, 0xaaaa_aaaa];

fn reg_value(rng: &mut Rng) -> u32 {
    if rng.below(2) == 0 { EDGES[rng.below(EDGES.len() as u64) as usize] } else { rng.next() as u32 }
}

/// A random extended value in the 80-bit format.
fn tbyte(rng: &mut Rng) -> [u8; 10] {
    let neg = rng.below(2) == 0;
    match rng.below(10) {
        0 => {
            let mut b = [0u8; 10];
            b[9] = (neg as u8) << 7;
            b
        }
        1 | 2 | 3 | 4 => {
            let e = 16383 + rng.below(161) as i32 - 80;
            let bits = [64, 53, 24, 8, 1][rng.below(5) as usize];
            let mant = (1u64 << 63) | ((rng.next() >> 1) & !((1u64 << (64 - bits)) - 1).min(u64::MAX >> 1));
            let mut b = [0u8; 10];
            b[..8].copy_from_slice(&mant.to_le_bytes());
            b[8..].copy_from_slice(&((neg as u16) << 15 | e as u16).to_le_bytes());
            b
        }
        5 | 6 => Value::from_i64(rng.below(2001) as i64 - 1000).to_tbyte().map(|x| x),
        7 => {
            let e = [1u16, 2, 0x7ffe, 0x7ffd, 16383 - 1022, 16383 + 1023, 16383 - 126, 16383 + 127, 16383 + 62, 16383 + 63, 16383 + 31][rng.below(11) as usize];
            let mant = (1u64 << 63) | (rng.next() >> 1);
            let mut b = [0u8; 10];
            b[..8].copy_from_slice(&mant.to_le_bytes());
            b[8..].copy_from_slice(&((neg as u16) << 15 | e).to_le_bytes());
            b
        }
        8 => {
            let mut b = [0u8; 10];
            b[7] = 0x80;
            b[8] = 0xff;
            b[9] = 0x7f | (neg as u8) << 7;
            b
        }
        _ => Value::from_f64(f64::from_bits(rng.next() & 0x4fff_ffff_ffff_ffff | 0x3000_0000_0000_0000)).to_tbyte(),
    }
}

fn fill_float_window(rng: &mut Rng, kind: u8, window: &mut [u8]) {
    match kind {
        1 => {
            let e = 127 + rng.below(61) as u32 - 30;
            let bits = (rng.below(2) as u32) << 31 | e << 23 | (rng.next() as u32 & 0x7f_ffff);
            window[..4].copy_from_slice(&bits.to_le_bytes());
        }
        2 => {
            let e = 1023 + rng.below(161) - 80;
            let bits = (rng.below(2)) << 63 | e << 52 | (rng.next() & 0xf_ffff_ffff_ffff);
            window[..8].copy_from_slice(&bits.to_le_bytes());
        }
        _ => window[..10].copy_from_slice(&tbyte(rng)),
    }
}

fn fpu_state(c: &mut Cpu, rng: &mut Rng) {
    let f = &mut c.fpu;
    f.top = rng.below(8) as u8;
    let valid = rng.below(9) as usize;
    f.empty = 0xff;
    for i in 0..valid {
        let p = (f.top as usize + i) & 7;
        f.set_reg_bytes(p, tbyte(rng));
        f.empty &= !(1 << p);
    }
    let rc = rng.below(4) as u16;
    let pc = [0u16, 2, 3][rng.below(3) as usize];
    f.cw = 0x7f | pc << 8 | rc << 10;
    f.sw = (rng.below(16) as u16 & 7) << 8 | (rng.below(2) as u16) << 14;
}

/// What is compared of the status word: TOP and the condition codes.
const SW_MASK: u16 = 0x3800 | 0x4700 | 0x20 | 0x41;

struct Outcome {
    regs: [u32; 8],
    flags: u32,
    cw: u16,
    sw: u16,
    empty: u8,
    fpu: [[u8; 10]; 8],
    region: Vec<u8>,
}

fn outcome(c: &Cpu, region: *const u8, defined: u32) -> Outcome {
    let mut fpu: [[u8; 10]; 8] = std::array::from_fn(|p| c.fpu.reg_bytes(p));
    for p in 0..8 {
        if c.fpu.empty >> p & 1 != 0 {
            fpu[p] = [0; 10];
        }
    }
    Outcome {
        regs: [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi],
        flags: c.fl.to_u32() & defined,
        cw: c.fpu.cw,
        sw: c.fpu.status() & SW_MASK,
        empty: c.fpu.empty,
        fpu,
        region: unsafe { core::slice::from_raw_parts(region, REGION) }.to_vec(),
    }
}

fn describe(o: &Outcome) -> String {
    format!("regs {:x?} flags {:#x} cw {:#x} sw {:#x} empty {:#x} fpu {:02x?}", o.regs, o.flags, o.cw, o.sw, o.empty, o.fpu)
}

fn same(a: &Outcome, b: &Outcome) -> bool {
    a.regs == b.regs && a.flags == b.flags && a.cw == b.cw && a.sw == b.sw && a.empty == b.empty && a.fpu == b.fpu && a.region == b.region
}

fn run_case(case: &Case, rng: &mut Rng, region: &mut [u8]) -> Result<bool, String> {
    let base = region.as_mut_ptr() as u32;
    for b in region.iter_mut() {
        *b = rng.next() as u8;
    }
    let mut c = Cpu::new(Mem::IDENTITY, base + 8192);
    let r = [&mut c.eax as *mut u32, &mut c.ecx, &mut c.edx, &mut c.ebx, &mut 0, &mut c.ebp, &mut c.esi, &mut c.edi];
    for (i, p) in r.iter().enumerate() {
        if i != 4 {
            unsafe { **p = reg_value(rng) };
        }
    }
    c.esp = base + 8192 + (if rng.below(4) == 0 { rng.below(4) as u32 } else { 0 });
    c.fl = Fl::from_u32((rng.next() as u32 & 0x8d5) | if rng.below(4) == 0 { 0x400 } else { 0 });
    fpu_state(&mut c, rng);
    let mut bytes = case.bytes.to_vec();
    if case.text == "leave" {
        c.ebp = base + 4096 + rng.below(8192) as u32;
    }
    if case.string {
        let mid = base + 8192;
        c.esi = mid.wrapping_add(rng.below(1024) as u32).wrapping_sub(512);
        c.edi = mid.wrapping_add(rng.below(1024) as u32).wrapping_sub(512);
        c.ecx = if case.rep { rng.below(40) as u32 } else { c.ecx };
        c.fl.df = rng.below(2) == 0;
        if case.rep && rng.below(8) == 0 {
            c.ecx = 0;
        }
    }
    if case.disp_offset >= 0 {
        let reg = |c: &Cpu, r: u8| -> u32 {
            match r {
                255 => 0,
                0 => c.eax,
                1 => c.ecx,
                2 => c.edx,
                3 => c.ebx,
                4 => c.esp,
                5 => c.ebp,
                6 => c.esi,
                _ => c.edi,
            }
        };
        let target = base + 512 + rng.below(REGION as u64 - 1024) as u32;
        let ea0 = reg(&c, case.base).wrapping_add(reg(&c, case.index).wrapping_mul(case.scale as u32));
        let disp = target.wrapping_sub(ea0);
        let o = case.disp_offset as usize;
        bytes[o..o + 4].copy_from_slice(&disp.to_le_bytes());
        cases::DISP.store(disp, std::sync::atomic::Ordering::Relaxed);
        if case.memfloat != 0 {
            let w = (target - base) as usize;
            fill_float_window(rng, case.memfloat, &mut region[w..w + 16]);
        }
    }
    if case.text == "popfd" || case.text == "popf" || case.text == "popfw" {
        let v = (rng.next() as u32 & 0xcd5) | 0x202;
        region[(c.esp - base) as usize..][..4].copy_from_slice(&v.to_le_bytes());
    }
    let defined = 0xcd5 & !case.undefined;
    let saved = region.to_vec();
    if std::env::var("S4_X86_TRACE").is_ok() { eprintln!("  state ready, lifting"); }
    // Lifted first: it detects divide errors (the processor would fault).
    let mut l = c.clone();
    let lifted = catch_unwind(AssertUnwindSafe(|| (case.run)(&mut l)));
    let lifted_region = region.to_vec();
    if let Err(e) = lifted {
        let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
        if msg.contains("divide error") {
            return Ok(false);
        }
        if msg.starts_with("x87:") || msg.contains("not modelled") {
            return Ok(false);
        }
        return Err(format!("lifted code panicked: {msg}"));
    }
    region.copy_from_slice(&saved);
    if std::env::var("S4_X86_TRACE").is_ok() { eprintln!("  lifted done, native"); }
    let mut n = c.clone();
    execute(&bytes, &mut n);
    let native = outcome(&n, region.as_ptr(), defined);
    let mut lifted_out = outcome(&l, lifted_region.as_ptr(), defined);
    lifted_out.region = lifted_region;
    if same(&native, &lifted_out) {
        Ok(true)
    } else {
        let mut init = c.clone();
        init.fpu = c.fpu.clone();
        Err(format!("initial: {}\n bytes {:02x?}\n native: {}\n lifted: {}", describe(&outcome(&c, saved.as_ptr(), defined)), bytes, describe(&native), describe(&lifted_out)))
    }
}

#[test]
fn instruction_forms() {
    let iters: u64 = std::env::var("S4_X86_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let only = std::env::var("S4_X86_ONLY").ok();
    let mut region = vec![0u8; REGION + 64];
    let aligned = (region.as_mut_ptr() as usize + 15) & !15;
    let off = aligned - region.as_mut_ptr() as usize;
    let region = &mut region[off..off + REGION];
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut failures: Vec<String> = Vec::new();
    let (mut compared, mut skipped) = (0u64, 0u64);
    let trace = std::env::var("S4_X86_TRACE").is_ok();
    let (from, to): (usize, usize) = (std::env::var("S4_X86_FROM").ok().and_then(|s| s.parse().ok()).unwrap_or(0), std::env::var("S4_X86_TO").ok().and_then(|s| s.parse().ok()).unwrap_or(usize::MAX));
    for (index, case) in CASES.iter().enumerate() {
        if index < from || index >= to {
            continue;
        }
        if trace {
            eprintln!("{index} {} [{}]", case.text, case.name);
        }
        if let Some(o) = &only {
            if !case.text.contains(o.as_str()) && !case.name.contains(o.as_str()) {
                continue;
            }
        }
        let mut shown = 0;
        for _ in 0..iters {
            match run_case(case, &mut rng, region) {
                Ok(true) => compared += 1,
                Ok(false) => skipped += 1,
                Err(e) => {
                    if shown < 2 {
                        failures.push(format!("{} [{}]:\n {}", case.text, case.name, e));
                    }
                    shown += 1;
                }
            }
        }
    }
    println!("{compared} runs compared, {skipped} skipped (divide errors, unmodelled x87 values)");
    if !failures.is_empty() {
        let n = failures.len();
        for f in failures.iter().take(12) {
            println!("{f}\n");
        }
        for f in failures.iter().skip(12) {
            println!("FAIL {}", f.lines().next().unwrap_or(""));
        }
        panic!("{n} instruction forms differ from the processor");
    }
}
