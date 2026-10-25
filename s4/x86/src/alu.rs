//! Integer arithmetic with the flags the processor sets. Operands are the
//! zero-extended values; `bits` is the operand size (8, 16 or 32) and is a
//! constant at every call site. Flags the manual calls undefined get the
//! value the hardware gives where it is simple, and are not compared by the
//! conformance tests (`s4/x86test`).

/// Flags (EFLAGS arithmetic bits and the direction flag).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fl {
    pub cf: bool,
    pub pf: bool,
    pub af: bool,
    pub zf: bool,
    pub sf: bool,
    pub of: bool,
    pub df: bool,
    /// Other writable EFLAGS bits (AC, VIF, VIP, ID): kept so CPU-detection code that toggles them works.
    pub extra: u32,
}

pub const CF: u32 = 1;
pub const PF: u32 = 1 << 2;
pub const AF: u32 = 1 << 4;
pub const ZF: u32 = 1 << 6;
pub const SF: u32 = 1 << 7;
pub const DF: u32 = 1 << 10;
pub const OF: u32 = 1 << 11;

impl Fl {
    /// EFLAGS as `pushfd` stores them (IF set, reserved bit 1 set).
    pub fn to_u32(self) -> u32 {
        0x202 | self.extra | self.cf as u32 | (self.pf as u32) << 2 | (self.af as u32) << 4 | (self.zf as u32) << 6 | (self.sf as u32) << 7 | (self.df as u32) << 10 | (self.of as u32) << 11
    }

    pub fn from_u32(v: u32) -> Fl {
        Fl { cf: v & CF != 0, pf: v & PF != 0, af: v & AF != 0, zf: v & ZF != 0, sf: v & SF != 0, df: v & DF != 0, of: v & OF != 0, extra: v & 0x003c_0000 }
    }

    /// The flags `lahf` copies into AH.
    pub fn to_ah(self) -> u32 {
        (self.to_u32() & 0xd5) | 2
    }

    /// `sahf`.
    pub fn set_ah(&mut self, ah: u32) {
        self.cf = ah & 1 != 0;
        self.pf = ah & 4 != 0;
        self.af = ah & 0x10 != 0;
        self.zf = ah & 0x40 != 0;
        self.sf = ah & 0x80 != 0;
    }
}

#[inline(always)]
pub fn mask(bits: u32) -> u32 {
    if bits == 32 { u32::MAX } else { (1 << bits) - 1 }
}

#[inline(always)]
fn msb(bits: u32) -> u32 {
    1 << (bits - 1)
}

#[inline(always)]
fn parity(v: u32) -> bool {
    (v as u8).count_ones() % 2 == 0
}

/// Sets ZF, SF, PF from a result.
#[inline(always)]
fn szp(f: &mut Fl, bits: u32, r: u32) {
    f.zf = r == 0;
    f.sf = r & msb(bits) != 0;
    f.pf = parity(r);
}

#[inline(always)]
pub fn sext(bits: u32, v: u32) -> i32 {
    ((v << (32 - bits)) as i32) >> (32 - bits)
}

/// `add` / `adc` (`cin` is the carry for adc).
#[inline(always)]
pub fn add(bits: u32, a: u32, b: u32, cin: u32, f: &mut Fl) -> u32 {
    let full = a as u64 + b as u64 + cin as u64;
    let r = full as u32 & mask(bits);
    f.cf = full >> bits != 0;
    f.of = (a ^ r) & (b ^ r) & msb(bits) != 0;
    f.af = (a ^ b ^ r) & 0x10 != 0;
    szp(f, bits, r);
    r
}

/// `sub` / `sbb` / `cmp`.
#[inline(always)]
pub fn sub(bits: u32, a: u32, b: u32, bin: u32, f: &mut Fl) -> u32 {
    let full = (a as u64).wrapping_sub(b as u64).wrapping_sub(bin as u64);
    let r = full as u32 & mask(bits);
    f.cf = (a as u64) < b as u64 + bin as u64;
    f.of = (a ^ b) & (a ^ r) & msb(bits) != 0;
    f.af = (a ^ b ^ r) & 0x10 != 0;
    szp(f, bits, r);
    r
}

/// `and` / `or` / `xor` / `test` results (the operation is done by the
/// caller on the zero-extended values).
#[inline(always)]
pub fn logic(bits: u32, r: u32, f: &mut Fl) -> u32 {
    let r = r & mask(bits);
    f.cf = false;
    f.of = false;
    f.af = false;
    szp(f, bits, r);
    r
}

/// `inc` (CF unchanged).
#[inline(always)]
pub fn inc(bits: u32, a: u32, f: &mut Fl) -> u32 {
    let cf = f.cf;
    let r = add(bits, a, 1, 0, f);
    f.cf = cf;
    r
}

/// `dec` (CF unchanged).
#[inline(always)]
pub fn dec(bits: u32, a: u32, f: &mut Fl) -> u32 {
    let cf = f.cf;
    let r = sub(bits, a, 1, 0, f);
    f.cf = cf;
    r
}

#[inline(always)]
pub fn neg(bits: u32, a: u32, f: &mut Fl) -> u32 {
    sub(bits, 0, a, 0, f)
}

/// `not` (no flags).
#[inline(always)]
pub fn not(bits: u32, a: u32) -> u32 {
    !a & mask(bits)
}

/// `shl` / `sal`.
#[inline(always)]
pub fn shl(bits: u32, a: u32, count: u32, f: &mut Fl) -> u32 {
    let n = count & 31;
    if n == 0 {
        return a;
    }
    let wide = (a as u64) << n;
    let r = wide as u32 & mask(bits);
    f.cf = if n <= bits { wide >> bits & 1 != 0 } else { false };
    f.of = (r & msb(bits) != 0) != f.cf;
    f.af = false;
    szp(f, bits, r);
    r
}

/// `shr`.
#[inline(always)]
pub fn shr(bits: u32, a: u32, count: u32, f: &mut Fl) -> u32 {
    let n = count & 31;
    if n == 0 {
        return a;
    }
    let r = if n >= 32 { 0 } else { a >> n };
    f.cf = if n <= bits { (a as u64) >> (n - 1) & 1 != 0 } else { false };
    f.of = a & msb(bits) != 0;
    f.af = false;
    szp(f, bits, r);
    r
}

/// `sar`.
#[inline(always)]
pub fn sar(bits: u32, a: u32, count: u32, f: &mut Fl) -> u32 {
    let n = count & 31;
    if n == 0 {
        return a;
    }
    let s = sext(bits, a) as i64;
    let r = (s >> n.min(63)) as u32 & mask(bits);
    f.cf = (s >> (n - 1).min(63)) & 1 != 0;
    f.of = false;
    f.af = false;
    szp(f, bits, r);
    r
}

/// `rol`.
#[inline(always)]
pub fn rol(bits: u32, a: u32, count: u32, f: &mut Fl) -> u32 {
    let n = count & 31;
    if n == 0 {
        return a;
    }
    let k = n % bits;
    let r = if k == 0 { a } else { ((a << k) | (a >> (bits - k))) & mask(bits) };
    f.cf = r & 1 != 0;
    f.of = (r & msb(bits) != 0) != f.cf;
    r
}

/// `ror`.
#[inline(always)]
pub fn ror(bits: u32, a: u32, count: u32, f: &mut Fl) -> u32 {
    let n = count & 31;
    if n == 0 {
        return a;
    }
    let k = n % bits;
    let r = if k == 0 { a } else { ((a >> k) | (a << (bits - k))) & mask(bits) };
    f.cf = r & msb(bits) != 0;
    f.of = (r & msb(bits) != 0) != (r & (msb(bits) >> 1) != 0);
    r
}

/// `rcl`.
#[inline(always)]
pub fn rcl(bits: u32, a: u32, count: u32, f: &mut Fl) -> u32 {
    let n = (count & 31) % (bits + 1);
    if count & 31 == 0 {
        return a;
    }
    // The (bits + 1)-bit value CF:a rotates left.
    let wide = (f.cf as u64) << bits | a as u64;
    let r = if n == 0 { wide } else { ((wide << n) | (wide >> (bits + 1 - n))) & ((1u64 << (bits + 1)) - 1) };
    f.cf = r >> bits & 1 != 0;
    let r = r as u32 & mask(bits);
    f.of = (r & msb(bits) != 0) != f.cf;
    r
}

/// `rcr`.
#[inline(always)]
pub fn rcr(bits: u32, a: u32, count: u32, f: &mut Fl) -> u32 {
    let n = (count & 31) % (bits + 1);
    if count & 31 == 0 {
        return a;
    }
    let wide = (f.cf as u64) << bits | a as u64;
    // OF uses the value before the rotate.
    f.of = (a & msb(bits) != 0) != f.cf;
    let r = if n == 0 { wide } else { ((wide >> n) | (wide << (bits + 1 - n))) & ((1u64 << (bits + 1)) - 1) };
    f.cf = r >> bits & 1 != 0;
    r as u32 & mask(bits)
}

/// `shld`: shifts `a` left, filling from the top of `b`.
#[inline(always)]
pub fn shld(bits: u32, a: u32, b: u32, count: u32, f: &mut Fl) -> u32 {
    let n = count & 31;
    if n == 0 {
        return a;
    }
    if n > bits {
        // Undefined for 16-bit operands; the hardware result is unspecified.
        return a;
    }
    let wide = (a as u64) << bits | b as u64;
    let r = ((wide << n) >> bits) as u32 & mask(bits);
    f.cf = (a as u64) >> (bits - n) & 1 != 0;
    f.of = (r & msb(bits) != 0) != (a & msb(bits) != 0);
    f.af = false;
    szp(f, bits, r);
    r
}

/// `shrd`.
#[inline(always)]
pub fn shrd(bits: u32, a: u32, b: u32, count: u32, f: &mut Fl) -> u32 {
    let n = count & 31;
    if n == 0 {
        return a;
    }
    if n > bits {
        return a;
    }
    let wide = (b as u64) << bits | a as u64;
    let r = (wide >> n) as u32 & mask(bits);
    f.cf = (a as u64) >> (n - 1) & 1 != 0;
    f.of = (r & msb(bits) != 0) != (a & msb(bits) != 0);
    f.af = false;
    szp(f, bits, r);
    r
}

/// One-operand `mul`: (low, high) of the unsigned product.
#[inline(always)]
pub fn mul(bits: u32, a: u32, b: u32, f: &mut Fl) -> (u32, u32) {
    let p = a as u64 * b as u64;
    let (lo, hi) = (p as u32 & mask(bits), (p >> bits) as u32 & mask(bits));
    f.cf = hi != 0;
    f.of = hi != 0;
    szp(f, bits, lo);
    f.af = false;
    (lo, hi)
}

/// One-operand `imul`: (low, high) of the signed product.
#[inline(always)]
pub fn imul1(bits: u32, a: u32, b: u32, f: &mut Fl) -> (u32, u32) {
    let p = sext(bits, a) as i64 * sext(bits, b) as i64;
    let (lo, hi) = (p as u32 & mask(bits), (p >> bits) as u32 & mask(bits));
    let fits = p == sext(bits, lo) as i64;
    f.cf = !fits;
    f.of = !fits;
    szp(f, bits, lo);
    f.af = false;
    (lo, hi)
}

/// Two- and three-operand `imul`: the truncated product.
#[inline(always)]
pub fn imul(bits: u32, a: u32, b: u32, f: &mut Fl) -> u32 {
    imul1(bits, a, b, f).0
}

/// `div`: (quotient, remainder) of the double-width dividend `hi:lo`; panics
/// where the hardware raises #DE.
#[inline(always)]
pub fn div(bits: u32, lo: u32, hi: u32, d: u32) -> (u32, u32) {
    assert!(d != 0, "divide error (division by zero)");
    let n = (hi as u64) << bits | lo as u64;
    let (q, r) = (n / d as u64, n % d as u64);
    assert!(q <= mask(bits) as u64, "divide error (quotient overflow)");
    (q as u32, r as u32)
}

/// `idiv`.
#[inline(always)]
pub fn idiv(bits: u32, lo: u32, hi: u32, d: u32) -> (u32, u32) {
    let d = sext(bits, d) as i64;
    assert!(d != 0, "divide error (division by zero)");
    let n = ((hi as u64) << bits | lo as u64) as i64;
    let n = if bits == 32 { n } else { (n << (64 - 2 * bits)) >> (64 - 2 * bits) };
    let (q, r) = (n.wrapping_div(d), n.wrapping_rem(d));
    let (min, max) = (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1);
    assert!(n != i64::MIN && q >= min && q <= max, "divide error (quotient overflow)");
    (q as u32 & mask(bits), r as u32 & mask(bits))
}

/// `bsf`: the index, `None` for zero (ZF set, destination unchanged).
#[inline(always)]
pub fn bsf(a: u32, f: &mut Fl) -> Option<u32> {
    f.zf = a == 0;
    if a == 0 { None } else { Some(a.trailing_zeros()) }
}

/// `bsr`.
#[inline(always)]
pub fn bsr(a: u32, f: &mut Fl) -> Option<u32> {
    f.zf = a == 0;
    if a == 0 { None } else { Some(31 - a.leading_zeros()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_flags() {
        let mut f = Fl::default();
        assert_eq!(add(8, 0xff, 1, 0, &mut f), 0);
        assert!(f.cf && f.zf && f.af && !f.sf && !f.of && f.pf);
        assert_eq!(add(8, 0x7f, 1, 0, &mut f), 0x80);
        assert!(!f.cf && f.of && f.sf);
    }
}
