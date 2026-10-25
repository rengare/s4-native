//! The x87 as lifted code sees it: eight physical registers in the 80-bit
//! memory format, TOP, tags (empty or not), status and control words. The
//! arithmetic itself is `s4_x87::Value`; this module is the register
//! stack and its conditions. Exceptions are masked as in the game
//! (control word 0x027f); stack faults produce the "real indefinite" NaN.

use s4_x87::{Cmp, Control, Rounding, Value};

pub const C0: u16 = 1 << 8;
pub const C1: u16 = 1 << 9;
pub const C2: u16 = 1 << 10;
pub const C3: u16 = 1 << 14;
const CC: u16 = C0 | C1 | C2 | C3;
const IE: u16 = 1;
const SF: u16 = 1 << 6;
const PE: u16 = 1 << 5;

/// Arithmetic of the two-operand instructions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arith {
    Add,
    Mul,
    /// destination - source.
    Sub,
    /// source - destination (`fsubr`).
    SubR,
    Div,
    DivR,
}

#[derive(Clone, Debug)]
pub struct Fpu {
    /// Physical registers: the 80-bit format and the decoded value are each kept while they are current (`raw_ok`, `dec_ok`: bit
    /// i set when register i's bytes / value are up to date; one of them always is). Arithmetic works on values, so a number that
    /// stays on the stack is never converted to bytes and back.
    reg: [[u8; 10]; 8],
    dec: [Value; 8],
    raw_ok: u8,
    dec_ok: u8,
    pub top: u8,
    /// Bit i set: physical register i is empty.
    pub empty: u8,
    /// Status word without TOP.
    pub sw: u16,
    pub cw: u16,
}

impl Default for Fpu {
    fn default() -> Fpu {
        Fpu::new()
    }
}

impl Fpu {
    /// State after `fninit` (control word 0x037f; threads start with 0x027f).
    pub fn new() -> Fpu {
        Fpu { reg: [[0; 10]; 8], dec: [Value::ZERO; 8], raw_ok: 0xff, dec_ok: 0xff, top: 0, empty: 0xff, sw: 0, cw: 0x027f }
    }

    pub fn control(&self) -> Control {
        Control::from_word(self.cw)
    }

    pub fn rounding(&self) -> Rounding {
        self.control().rounding
    }

    /// The status word with TOP.
    pub fn status(&self) -> u16 {
        (self.sw & !0x3800) | (self.top as u16) << 11
    }

    pub fn set_status(&mut self, w: u16) {
        self.sw = w & !0x3800;
        self.top = (w >> 11 & 7) as u8;
    }

    fn phys(&self, i: usize) -> usize {
        (self.top as usize + i) & 7
    }

    fn set_cc(&mut self, c0: bool, c1: bool, c2: bool, c3: bool) {
        self.sw = (self.sw & !CC) | if c0 { C0 } else { 0 } | if c1 { C1 } else { 0 } | if c2 { C2 } else { 0 } | if c3 { C3 } else { 0 };
    }

    /// Register `p` (physical) as bytes, without changing what is cached.
    pub fn reg_bytes(&self, p: usize) -> [u8; 10] {
        if self.raw_ok >> p & 1 != 0 { self.reg[p] } else { self.dec[p].to_tbyte() }
    }

    /// Sets register `p` (physical) from bytes.
    pub fn set_reg_bytes(&mut self, p: usize, b: [u8; 10]) {
        self.reg[p] = b;
        self.raw_ok |= 1 << p;
        self.dec_ok &= !(1 << p);
    }

    fn value_of(&mut self, p: usize) -> Value {
        if self.dec_ok >> p & 1 == 0 {
            self.dec[p] = Value::from_tbyte(self.reg[p]);
            self.dec_ok |= 1 << p;
        }
        self.dec[p]
    }

    fn put_value(&mut self, p: usize, v: Value) {
        self.dec[p] = v;
        self.dec_ok |= 1 << p;
        self.raw_ok &= !(1 << p);
    }

    /// ST(i); an empty register is a stack underflow: invalid-operation and
    /// stack-fault flags, C1 cleared, the real indefinite as the value.
    pub fn st(&mut self, i: usize) -> Value {
        let p = self.phys(i);
        if self.empty >> p & 1 != 0 {
            self.sw = (self.sw | IE | SF) & !C1;
            return Value::INDEFINITE;
        }
        self.value_of(p)
    }

    /// Writes ST(i) (the register becomes valid).
    pub fn set_st(&mut self, i: usize, v: Value) {
        let p = self.phys(i);
        self.put_value(p, v);
        self.empty &= !(1 << p);
    }

    pub fn push(&mut self, v: Value) {
        self.top = self.top.wrapping_sub(1) & 7;
        let p = self.top as usize;
        if self.empty >> p & 1 == 0 {
            // Stack overflow: the masked response loads the indefinite.
            self.sw |= IE | SF | C1;
            self.put_value(p, Value::INDEFINITE);
        } else {
            self.sw &= !C1;
            self.put_value(p, v);
        }
        self.empty &= !(1 << p);
    }

    pub fn pop(&mut self) {
        let p = self.top as usize;
        if self.empty >> p & 1 != 0 {
            self.sw = (self.sw | IE | SF) & !C1;
        }
        self.empty |= 1 << p;
        self.top = (self.top + 1) & 7;
    }

    /// `fld st(i)`.
    pub fn fld_st(&mut self, i: usize) {
        let v = self.st(i);
        self.push(v);
    }

    pub fn fld_f32(&mut self, bits: u32) {
        self.push(Value::from_f32(f32::from_bits(bits)));
    }

    pub fn fld_f64(&mut self, bits: u64) {
        self.push(Value::from_f64(f64::from_bits(bits)));
    }

    pub fn fld_f80(&mut self, bytes: [u8; 10]) {
        self.push(Value::from_tbyte(bytes));
    }

    pub fn fild(&mut self, v: i64) {
        self.push(Value::from_i64(v));
    }

    /// `fst`/`fstp` to a float; the bits to store.
    pub fn fst_f32(&mut self, pop: bool) -> u32 {
        let v = self.st(0);
        Value::clear_status();
        let r = v.to_f32(self.rounding()).to_bits();
        self.finish();
        if pop {
            self.pop();
        }
        r
    }

    pub fn fst_f64(&mut self, pop: bool) -> u64 {
        let v = self.st(0);
        Value::clear_status();
        let r = v.to_f64(self.rounding()).to_bits();
        self.finish();
        if pop {
            self.pop();
        }
        r
    }

    pub fn fst_f80(&mut self, pop: bool) -> [u8; 10] {
        let r = self.st(0).to_tbyte();
        if pop {
            self.pop();
        }
        r
    }

    /// `fst st(i)` / `fstp st(i)`.
    pub fn fst_st(&mut self, i: usize, pop: bool) {
        let v = self.st(0);
        self.sw &= !C1;
        self.set_st(i, v);
        if pop {
            self.pop();
        }
    }

    /// `fist`/`fistp` with the control word's rounding; `bits` 16, 32 or 64.
    pub fn fist(&mut self, bits: u32, pop: bool) -> i64 {
        let v = self.st(0);
        let r = self.rounding();
        self.store_int(v, bits, r, pop)
    }

    /// `fisttp`: truncating.
    pub fn fisttp(&mut self, bits: u32, pop: bool) -> i64 {
        let v = self.st(0);
        self.store_int(v, bits, Rounding::Chop, pop)
    }

    fn store_int(&mut self, v: Value, bits: u32, mode: Rounding, pop: bool) -> i64 {
        Value::clear_status();
        let r = match bits {
            16 => {
                let x = v.to_i64(mode);
                if (i16::MIN as i64..=i16::MAX as i64).contains(&s4_x87::record_invalid_if(x, i16::MIN as i64, i16::MAX as i64)) { x } else { i16::MIN as i64 }
            }
            32 => v.to_i32(mode) as i64,
            _ => v.to_i64(mode),
        };
        self.finish();
        if pop {
            self.pop();
        }
        r
    }

    /// Applies the rounding status of the operation just done: C1 says the
    /// result was rounded up, PE (sticky) that it was inexact.
    fn finish(&mut self) {
        let st = Value::status();
        if st.zero_divide {
            self.sw |= 4;
        }
        if st.invalid {
            self.sw = (self.sw | IE) & !C1;
            return;
        }
        self.sw = (self.sw & !C1) | if st.up { C1 } else { 0 } | if st.inexact { PE } else { 0 };
    }

    fn apply(op: Arith, dst: Value, src: Value, c: Control) -> Value {
        match op {
            Arith::Add => dst.add(src, c),
            Arith::Mul => dst.mul(src, c),
            Arith::Sub => dst.sub(src, c),
            Arith::SubR => src.sub(dst, c),
            Arith::Div => dst.div(src, c),
            Arith::DivR => src.div(dst, c),
        }
    }

    /// `op st(d), st(s)` (with `pop` the `...p` forms: result in ST(d), then
    /// ST(0) is popped).
    pub fn op_st(&mut self, op: Arith, d: usize, s: usize, pop: bool) {
        let (dv, sv) = (self.st(d), self.st(s));
        Value::clear_status();
        let r = Fpu::apply(op, dv, sv, self.control());
        self.finish();
        self.set_st(d, r);
        if pop {
            self.pop();
        }
    }

    /// `op st(0), value` for the memory forms (`value` already converted).
    pub fn op_val(&mut self, op: Arith, v: Value) {
        let dv = self.st(0);
        Value::clear_status();
        let r = Fpu::apply(op, dv, v, self.control());
        self.finish();
        self.set_st(0, r);
    }

    pub fn xch(&mut self, i: usize) {
        let (a, b) = (self.phys(0), self.phys(i));
        let (ea, eb) = (self.empty >> a & 1 != 0, self.empty >> b & 1 != 0);
        // Exchanging with an empty register is a stack fault on both.
        let (va, vb) = (self.st(0), self.st(i));
        let _ = (ea, eb);
        self.sw &= !C1;
        self.set_st(0, vb);
        self.set_st(i, va);
    }

    pub fn chs(&mut self) {
        if self.empty >> self.phys(0) & 1 != 0 {
            self.sw = (self.sw | IE | SF) & !C1;
            self.set_st(0, Value::INDEFINITE);
            return;
        }
        let v = self.st(0);
        self.sw &= !C1;
        self.set_st(0, v.neg());
    }

    pub fn sqrt(&mut self) {
        let v = self.st(0);
        Value::clear_status();
        let r = v.sqrt(self.control());
        self.finish();
        self.set_st(0, r);
    }

    /// `fyl2x` / `fyl2xp1`: ST(1) = ST(1) * log2(ST(0) [+ 1]), pop.
    pub fn fyl2x(&mut self, plus_one: bool) {
        let (x, y) = (self.st(0), self.st(1));
        Value::clear_status();
        let c = self.control();
        let r = if plus_one { Value::fyl2xp1(x, y, c) } else { Value::fyl2x(x, y, c) };
        self.finish();
        self.set_st(1, r);
        self.pop();
    }

    /// `f2xm1`.
    pub fn f2xm1(&mut self) {
        let x = self.st(0);
        Value::clear_status();
        let r = x.f2xm1(self.control());
        self.finish();
        self.set_st(0, r);
    }

    /// `fsin` (0), `fcos` (1), `fptan` (2: pushes 1.0).
    pub fn sincos(&mut self, which: u8) {
        let x = self.st(0);
        Value::clear_status();
        match x.sincos(which, self.control()) {
            Some(r) => {
                self.finish();
                self.set_cc(false, false, false, false);
                self.sw &= !C2;
                self.set_st(0, r);
                if which == 2 {
                    self.push(Value::constant(s4_x87::Constant::One, Rounding::Nearest));
                }
            }
            None => {
                // Operand out of range: C2 set, operand unchanged.
                self.sw = (self.sw | C2) & !C1;
            }
        }
    }

    /// `fpatan`: ST(1) = atan2(ST(1), ST(0)), pop.
    pub fn fpatan(&mut self) {
        let (x, y) = (self.st(0), self.st(1));
        Value::clear_status();
        let r = Value::fpatan(y, x, self.control());
        self.finish();
        self.set_st(1, r);
        self.pop();
    }

    pub fn frndint(&mut self) {
        let x = self.st(0);
        Value::clear_status();
        let r = x.rndint(self.rounding());
        self.finish();
        self.set_st(0, r);
    }

    /// `fscale`.
    pub fn fscale(&mut self) {
        let (x, n) = (self.st(0), self.st(1));
        Value::clear_status();
        let r = x.scale(n);
        self.finish();
        self.set_st(0, r);
    }

    /// `fprem` / `fprem1`.
    pub fn fprem(&mut self, ieee: bool) {
        let (x, y) = (self.st(0), self.st(1));
        Value::clear_status();
        let (r, q, complete) = x.prem(y, ieee);
        self.finish();
        self.set_st(0, r);
        if complete {
            self.set_cc(q & 4 != 0, q & 1 != 0, false, q & 2 != 0);
        } else {
            self.sw = (self.sw | C2) & !C1;
        }
    }

    pub fn abs(&mut self) {
        if self.empty >> self.phys(0) & 1 != 0 {
            self.sw = (self.sw | IE | SF) & !C1;
            self.set_st(0, Value::INDEFINITE);
            return;
        }
        let v = self.st(0);
        self.sw &= !C1;
        self.set_st(0, v.abs());
    }

    /// Loads a constant (`fld1`, `fldz`, `fldpi`...), rounded as the
    /// control word's rounding demands.
    pub fn fld_const(&mut self, k: s4_x87::Constant) {
        let r = self.rounding();
        self.push(Value::constant(k, r));
    }

    /// `fcom`/`fucom` family: compare ST(0) with `other`; the caller pops.
    pub fn compare(&mut self, other: Value, ordered: bool) {
        let a = self.st(0);
        let cmp = a.compare(other);
        let (c3, c2, c0) = match cmp {
            Cmp::Greater => (false, false, false),
            Cmp::Less => (false, false, true),
            Cmp::Equal => (true, false, false),
            Cmp::Unordered => (true, true, true),
        };
        if cmp == Cmp::Unordered && (ordered || a.is_signaling_nan() || other.is_signaling_nan()) {
            self.sw |= IE;
        }
        self.set_cc(c0, false, c2, c3);
    }

    pub fn compare_st(&mut self, i: usize, ordered: bool, pops: u32) {
        let o = self.st(i);
        self.compare(o, ordered);
        for _ in 0..pops {
            self.pop();
        }
    }

    /// `ftst`.
    pub fn test(&mut self) {
        self.compare(Value::ZERO, true);
    }

    /// `fxam`.
    pub fn examine(&mut self) {
        let p = self.phys(0);
        if self.empty >> p & 1 != 0 {
            let neg = self.reg_bytes(p)[9] >> 7 != 0;
            self.set_cc(true, neg, false, true);
            return;
        }
        let neg = self.reg_bytes(p)[9] >> 7 != 0;
        let v = self.value_of(p);
        let (c3, c2, c0) = v.class();
        self.set_cc(c0, neg, c2, c3);
    }

    pub fn load_cw(&mut self, w: u16) {
        // Bits 15-13 and 7 read as 0, bit 6 as 1.
        self.cw = (w & 0x1f3f) | 0x40;
    }

    /// `fnclex`.
    pub fn clear_exceptions(&mut self) {
        self.sw &= !0x80ff;
    }

    /// `fninit`.
    pub fn init(&mut self) {
        self.cw = 0x037f;
        self.sw = 0;
        self.top = 0;
        self.empty = 0xff;
    }

    pub fn free(&mut self, i: usize) {
        let p = self.phys(i);
        self.empty |= 1 << p;
    }

    pub fn decstp(&mut self) {
        self.top = self.top.wrapping_sub(1) & 7;
        self.sw &= !C1;
    }

    pub fn incstp(&mut self) {
        self.top = (self.top + 1) & 7;
        self.sw &= !C1;
    }

    /// Tag word as `fnstenv` stores it (2 bits per physical register).
    pub fn tag_word(&self) -> u16 {
        let mut w = 0u16;
        for p in 0..8 {
            let t = if self.empty >> p & 1 != 0 {
                3
            } else {
                let bytes = self.reg_bytes(p);
                let se = u16::from_le_bytes([bytes[8], bytes[9]]);
                let mant = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                let e = se & 0x7fff;
                if e == 0x7fff || (e != 0 && mant >> 63 == 0) || (e == 0 && mant != 0) {
                    2
                } else if e == 0 && mant == 0 {
                    1
                } else {
                    0
                }
            };
            w |= t << (2 * p);
        }
        w
    }

    /// `fnstenv` (32-bit operand size): control, status and tag words; the pointers read as 0.
    pub fn store_env(&mut self, mem: crate::mem::Mem, ea: u32) {
        mem.w32(ea, self.cw as u32);
        mem.w32(ea + 4, self.status() as u32);
        mem.w32(ea + 8, self.tag_word() as u32);
        for k in [12, 16, 20, 24] {
            mem.w32(ea + k, 0);
        }
        // Storing the environment masks all exceptions.
        self.cw |= 0x3f;
    }

    /// `fldenv`.
    pub fn load_env(&mut self, mem: crate::mem::Mem, ea: u32) {
        self.load_cw(mem.r32(ea) as u16);
        self.set_status(mem.r32(ea + 4) as u16);
        let tw = mem.r32(ea + 8);
        self.empty = 0;
        for p in 0..8 {
            if tw >> (2 * p) & 3 == 3 {
                self.empty |= 1 << p;
            }
        }
    }

    /// `fnsave`: environment, then ST(0)..ST(7) as 80-bit values, then `fninit`.
    pub fn save_state(&mut self, mem: crate::mem::Mem, ea: u32) {
        let (cw, sw, tw) = (self.cw, self.status(), self.tag_word());
        mem.w32(ea, cw as u32);
        mem.w32(ea + 4, sw as u32);
        mem.w32(ea + 8, tw as u32);
        for k in [12, 16, 20, 24] {
            mem.w32(ea + k, 0);
        }
        for i in 0..8 {
            let b = self.reg_bytes(self.phys(i));
            for (j, v) in b.iter().enumerate() {
                mem.w8(ea + 28 + 10 * i as u32 + j as u32, *v);
            }
        }
        self.init();
    }

    /// `frstor`.
    pub fn restore_state(&mut self, mem: crate::mem::Mem, ea: u32) {
        self.load_env(mem, ea);
        for i in 0..8 {
            let p = self.phys(i);
            let mut b = [0u8; 10];
            for j in 0..10 {
                b[j] = mem.r8(ea + 28 + 10 * i as u32 + j as u32);
            }
            self.set_reg_bytes(p, b);
        }
    }
}
