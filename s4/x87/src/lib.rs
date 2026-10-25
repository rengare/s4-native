//! A software x87: values with the FPU's 64-bit significand, the operations
//! the ported code needs (load, add, subtract, multiply, divide, store as
//! float, store as integer) with the control word's rounding and precision
//! control applied the way the hardware does. Everything is integer
//! arithmetic, so the result is the same on every host (the game's own code
//! runs on the x87, whose rounding mode it changes at run time:
//! `SetGfxLayerAccess` leaves it rounding toward zero). `tests/x87.rs`
//! compares every operation against the real FPU under Wine.
//!
//! Infinities and the default NaN ("real indefinite") come out of division by
//! zero and invalid operations as on the FPU (exceptions are masked in the
//! game); NaN payloads and denormal extended values are not modelled.

/// Control word bits 10-11.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rounding {
    Nearest,
    Down,
    Up,
    /// Toward zero.
    Chop,
}

/// Control word bits 8-9: significand bits of the result of add, subtract,
/// multiply and divide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Precision {
    P24,
    P53,
    P64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Control {
    pub rounding: Rounding,
    pub precision: Precision,
}

impl Control {
    /// A thread's initial control word 0x027f.
    pub const DEFAULT: Control = Control { rounding: Rounding::Nearest, precision: Precision::P53 };
    /// Control word 0x0e7f, what `_controlfp(0x300, 0x300)` selects.
    pub const CHOP: Control = Control { rounding: Rounding::Chop, precision: Precision::P53 };

    /// From a control word (exception masks are ignored).
    pub fn from_word(word: u16) -> Control {
        Control {
            rounding: match word >> 10 & 3 {
                0 => Rounding::Nearest,
                1 => Rounding::Down,
                2 => Rounding::Up,
                _ => Rounding::Chop,
            },
            precision: match word >> 8 & 3 {
                0 => Precision::P24,
                2 => Precision::P53,
                _ => Precision::P64,
            },
        }
    }

    /// The control word with all exceptions masked.
    pub fn word(self) -> u16 {
        let rc = match self.rounding {
            Rounding::Nearest => 0,
            Rounding::Down => 1,
            Rounding::Up => 2,
            Rounding::Chop => 3,
        };
        let pc = match self.precision {
            Precision::P24 => 0,
            Precision::P53 => 2,
            Precision::P64 => 3,
        };
        0x007f | pc << 8 | rc << 10
    }

    fn bits(self) -> u32 {
        match self.precision {
            Precision::P24 => 24,
            Precision::P53 => 53,
            Precision::P64 => 64,
        }
    }
}

/// An extended-precision value: zero (`mant` 0, `exp` 0), an infinity
/// (`mant` 0, `exp` [`INF`]), a NaN (`mant` its 64-bit significand with the
/// integer bit set, `exp` [`NAN`]), or
/// `mant * 2^exp` with bit 63 of `mant` set.
const INF: i32 = i32::MAX;
const NAN: i32 = i32::MIN;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Value {
    neg: bool,
    mant: u64,
    exp: i32,
}

/// What the last rounding did: whether it lost bits (the precision
/// exception) and whether the magnitude was rounded up (condition code C1).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Status {
    pub inexact: bool,
    pub up: bool,
    /// An invalid operation (integer store that does not fit).
    pub invalid: bool,
    /// A finite nonzero value divided by zero.
    pub zero_divide: bool,
}

thread_local! {
    static LAST: core::cell::Cell<Status> = const { core::cell::Cell::new(Status { inexact: false, up: false, invalid: false, zero_divide: false }) };
}

fn record_status(inexact: bool, up: bool) {
    LAST.with(|l| l.set(Status { inexact, up, invalid: false, zero_divide: false }));
}

/// The operation was invalid (the masked response is returned).
pub fn record_invalid() {
    LAST.with(|l| l.set(Status { inexact: false, up: false, invalid: true, zero_divide: false }));
}

impl Value {
    /// Forgets the status of earlier operations.
    pub fn clear_status() {
        LAST.with(|l| l.set(Status::default()));
    }

    /// The rounding status of the operation since [`Value::clear_status`].
    pub fn status() -> Status {
        LAST.with(|l| l.get())
    }
}

mod big;
mod trans;

/// Right shift of `m` by `shift` bits (at least 1), rounded by `mode`;
/// `sticky` stands for a nonzero fraction below the least significant bit.
fn shift_round(m: u128, shift: u32, sticky: bool, neg: bool, mode: Rounding) -> u128 {
    debug_assert!(shift >= 1);
    let (kept, rem_nonzero, above, tie) = if shift > 128 {
        // The value is below half of the rounding unit.
        (0, m != 0, false, false)
    } else if shift == 128 {
        (0, m != 0, m > 1 << 127, m == 1 << 127)
    } else {
        let (rem, half) = (m & ((1u128 << shift) - 1), 1u128 << (shift - 1));
        (m >> shift, rem != 0, rem > half, rem == half)
    };
    let inexact = rem_nonzero || sticky;
    let up = shift_round_up(mode, above, tie, sticky, kept, neg, inexact);
    record_status(inexact, up);
    kept + up as u128
}

fn shift_round_up(mode: Rounding, above: bool, tie: bool, sticky: bool, kept: u128, neg: bool, inexact: bool) -> bool {
    match mode {
        Rounding::Nearest => above || (tie && (sticky || kept & 1 == 1)),
        Rounding::Chop => false,
        Rounding::Up => !neg && inexact,
        Rounding::Down => neg && inexact,
    }
}

impl Value {
    pub const ZERO: Value = Value { neg: false, mant: 0, exp: 0 };

    fn is_zero(self) -> bool {
        self.mant == 0 && self.exp == 0
    }

    fn is_inf(self) -> bool {
        self.mant == 0 && self.exp == INF
    }

    fn is_nan(self) -> bool {
        self.exp == NAN
    }

    fn zero(neg: bool) -> Value {
        Value { neg, mant: 0, exp: 0 }
    }

    fn inf(neg: bool) -> Value {
        Value { neg, mant: 0, exp: INF }
    }

    /// The "real indefinite" NaN the FPU makes for invalid operations.
    /// The indefinite NaN as the result of an invalid operation.
    fn invalid() -> Value {
        record_invalid();
        Value::INDEFINITE
    }

    pub const INDEFINITE: Value = Value { neg: true, mant: 0xc000_0000_0000_0000, exp: NAN };

    /// The NaN an operation with NaN operand(s) returns: a signaling NaN is
    /// quieted, and of two NaNs the one with the larger significand wins.
    fn nan_pick(a: Value, b: Value) -> Value {
        let q = |v: Value| if v.is_nan() { Value { mant: v.mant | 1 << 62, ..v } } else { v };
        if a.is_signaling_nan() || b.is_signaling_nan() {
            record_invalid();
        }
        let (a, b) = (q(a), q(b));
        match (a.is_nan(), b.is_nan()) {
            (true, false) => a,
            (false, true) => b,
            _ => {
                if a.mant > b.mant || (a.mant == b.mant && !a.neg) { a } else { b }
            }
        }
    }

    /// Normalizes `m * 2^exp` for a 128-bit `m` (nonzero) that fits in 64 significant bits after shifting.
    fn normalized_u128(neg: bool, m: u128, exp: i32) -> Value {
        let lz = m.leading_zeros();
        let (top, e) = (m << lz, exp - lz as i32 + 64);
        // The significand is the top 64 bits; the rest is zero for the exact remainders used here.
        Value { neg, mant: (top >> 64) as u64, exp: e }
    }

    /// Normalizes `mant * 2^exp` (mant nonzero).
    fn normalized(neg: bool, mant: u64, exp: i32) -> Value {
        let lz = mant.leading_zeros();
        Value { neg, mant: mant << lz, exp: exp - lz as i32 }
    }

    pub fn from_i32(v: i32) -> Value {
        Value::from_i64(v as i64)
    }

    pub fn from_i64(v: i64) -> Value {
        if v == 0 {
            return Value::ZERO;
        }
        Value::normalized(v < 0, v.unsigned_abs(), 0)
    }

    pub fn from_f32(v: f32) -> Value {
        let bits = v.to_bits();
        let (neg, e, frac) = (bits >> 31 != 0, (bits >> 23 & 0xff) as i32, (bits & 0x7f_ffff) as u64);
        match (e, frac) {
            (0xff, 0) => Value::inf(neg),
            (0xff, f) => Value { neg, mant: 3 << 62 | f << 40, exp: NAN },
            (0, 0) => Value::zero(neg),
            (0, f) => Value::normalized(neg, f, -149),
            (e, f) => Value::normalized(neg, f | 1 << 23, e - 150),
        }
    }

    pub fn from_f64(v: f64) -> Value {
        let bits = v.to_bits();
        let (neg, e, frac) = (bits >> 63 != 0, (bits >> 52 & 0x7ff) as i32, bits & ((1 << 52) - 1));
        match (e, frac) {
            (0x7ff, 0) => Value::inf(neg),
            (0x7ff, f) => Value { neg, mant: 3 << 62 | f << 11, exp: NAN },
            (0, 0) => Value::zero(neg),
            (0, f) => Value::normalized(neg, f, -1074),
            (e, f) => Value::normalized(neg, f | 1 << 52, e - 1075),
        }
    }

    /// From the 80-bit memory format (`fld tbyte`): 64-bit significand, then
    /// sign and 15-bit biased exponent.
    pub fn from_tbyte(bytes: [u8; 10]) -> Value {
        let mant = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let se = u16::from_le_bytes([bytes[8], bytes[9]]);
        let (neg, e) = (se >> 15 != 0, (se & 0x7fff) as i32);
        if e == 0x7fff {
            assert!(mant >> 63 == 1, "x87: pseudo-NaN or pseudo-infinity");
            return if mant << 1 == 0 { Value::inf(neg) } else { Value { neg, mant, exp: NAN } };
        }
        if mant == 0 {
            return Value::zero(neg);
        }
        assert!(e != 0 && mant >> 63 == 1, "x87: denormal extended value");
        Value { neg, mant, exp: e - 16383 - 63 }
    }

    /// The 80-bit memory format (`fstp tbyte`).
    pub fn to_tbyte(self) -> [u8; 10] {
        let mut out = [0u8; 10];
        let (mant, se) = if self.is_zero() {
            (0, (self.neg as u16) << 15)
        } else if self.is_inf() {
            (1 << 63, (self.neg as u16) << 15 | 0x7fff)
        } else if self.is_nan() {
            (self.mant, (self.neg as u16) << 15 | 0x7fff)
        } else {
            (self.mant, (self.neg as u16) << 15 | (self.exp + 63 + 16383) as u16)
        };
        out[..8].copy_from_slice(&mant.to_le_bytes());
        out[8..].copy_from_slice(&se.to_le_bytes());
        out
    }

    /// Rounds `m * 2^e` (+ a nonzero fraction if `sticky`) to the control
    /// word's precision.
    fn round(neg: bool, e: i32, m: u128, sticky: bool, c: Control) -> Value {
        debug_assert!(m != 0);
        let lz = m.leading_zeros();
        let (m, e) = (m << lz, e - lz as i32);
        let bits = c.bits();
        let shift = 128 - bits;
        let mut kept = shift_round(m, shift, sticky, neg, c.rounding);
        // value = kept * 2^(e + shift) = (kept << (64 - bits)) * 2^(e + 64)
        let mut e = e + 64;
        if kept >> bits != 0 {
            kept >>= 1;
            e += 1;
        }
        assert!((-16382..=16383).contains(&(e + 63)), "x87: result outside the normal extended exponent range");
        Value { neg, mant: (kept << (64 - bits)) as u64, exp: e }
    }

    /// The MSB's exponent.
    fn magnitude(self) -> i32 {
        self.exp + 63
    }

    pub fn neg(self) -> Value {
        Value { neg: !self.neg, ..self }
    }

    /// `fadd`: the exact sum rounded by the control word.
    pub fn add(self, other: Value, c: Control) -> Value {
        if self.is_nan() || other.is_nan() {
            return Value::nan_pick(self, other);
        }
        if self.is_inf() || other.is_inf() {
            return match (self.is_inf(), other.is_inf()) {
                (true, true) if self.neg != other.neg => Value::invalid(),
                (true, _) => self,
                _ => other,
            };
        }
        if self.is_zero() && other.is_zero() {
            let neg = if self.neg == other.neg { self.neg } else { c.rounding == Rounding::Down };
            return Value::zero(neg);
        }
        if other.is_zero() {
            return Value::round(self.neg, self.exp, self.mant as u128, false, c);
        }
        if self.is_zero() {
            return Value::round(other.neg, other.exp, other.mant as u128, false, c);
        }
        let (big, small) = if (self.magnitude(), self.mant) >= (other.magnitude(), other.mant) { (self, other) } else { (other, self) };
        let d = (big.magnitude() - small.magnitude()) as u32;
        let a = (big.mant as u128) << 63;
        let s = (small.mant as u128) << 63;
        let (s, sticky) = if d >= 127 {
            (0, true)
        } else {
            (s >> d, s & ((1u128 << d) - 1) != 0)
        };
        let e = big.exp - 63;
        if big.neg == small.neg {
            Value::round(big.neg, e, a + s, sticky, c)
        } else {
            // a - (s + fraction) = (a - s - 1) + (1 - fraction)
            let diff = a - s;
            if diff == 0 && !sticky {
                return Value::zero(c.rounding == Rounding::Down);
            }
            // With a fraction (d >= 1) the difference is at least a / 2.
            let (m, sticky) = if sticky { (diff - 1, true) } else { (diff, false) };
            Value::round(big.neg, e, m, sticky, c)
        }
    }

    /// `fsub`.
    pub fn sub(self, other: Value, c: Control) -> Value {
        // A NaN operand passes through unchanged (its sign is not flipped).
        self.add(if other.is_nan() { other } else { other.neg() }, c)
    }

    /// `fmul`.
    pub fn mul(self, other: Value, c: Control) -> Value {
        let neg = self.neg != other.neg;
        if self.is_nan() || other.is_nan() {
            return Value::nan_pick(self, other);
        }
        if self.is_inf() || other.is_inf() {
            return if self.is_zero() || other.is_zero() { Value::invalid() } else { Value::inf(neg) };
        }
        if self.is_zero() || other.is_zero() {
            return Value::zero(neg);
        }
        Value::round(neg, self.exp + other.exp, self.mant as u128 * other.mant as u128, false, c)
    }

    /// `fdiv`.
    pub fn div(self, other: Value, c: Control) -> Value {
        let neg = self.neg != other.neg;
        if self.is_nan() || other.is_nan() {
            return Value::nan_pick(self, other);
        }
        if (self.is_inf() && other.is_inf()) || (self.is_zero() && other.is_zero()) {
            return Value::invalid();
        }
        if other.is_zero() && !self.is_inf() {
            LAST.with(|l| l.set(Status { zero_divide: true, ..Status::default() }));
        }
        if self.is_inf() || other.is_zero() {
            return Value::inf(neg);
        }
        if self.is_zero() || other.is_inf() {
            return Value::zero(neg);
        }
        let d = other.mant as u128;
        let n = (self.mant as u128) << 63;
        let (q1, r1) = (n / d, n % d);
        let (q2, r2) = ((r1 << 64) / d, (r1 << 64) % d);
        let quotient = q1 << 64 | q2;
        Value::round(neg, self.exp - other.exp - 63 - 64, quotient, r2 != 0, c)
    }

    /// The magnitude rounded to an integer by `mode`; `None` from 2^64 up.
    fn to_integer(self, mode: Rounding) -> Option<u128> {
        if self.is_zero() {
            return Some(0);
        }
        if self.is_inf() || self.is_nan() {
            return None;
        }
        if self.magnitude() >= 64 {
            return None;
        }
        Some(if self.exp >= 0 {
            (self.mant as u128) << self.exp
        } else {
            shift_round(self.mant as u128, (-self.exp) as u32, false, self.neg, mode)
        })
    }

    /// `fistp dword`: the 32-bit integer, 0x80000000 ("integer indefinite")
    /// when it does not fit.
    pub fn to_i32(self, mode: Rounding) -> i32 {
        match self.to_integer(mode) {
            Some(m) if self.neg && m <= 1 << 31 => (m as i64).wrapping_neg() as i32,
            Some(m) if !self.neg && m < 1 << 31 => m as i32,
            _ => {
                record_invalid();
                i32::MIN
            }
        }
    }

    /// `fistp qword`.
    pub fn to_i64(self, mode: Rounding) -> i64 {
        match self.to_integer(mode) {
            Some(m) if self.neg && m <= 1 << 63 => (m as i128).wrapping_neg() as i64,
            Some(m) if !self.neg && m < 1 << 63 => m as i64,
            _ => {
                record_invalid();
                i64::MIN
            }
        }
    }

    /// `fstp dword`: rounded to a 32-bit float by the rounding mode
    /// (overflow gives infinity or the largest float, as the mode demands).
    pub fn to_f32(self, mode: Rounding) -> f32 {
        f32::from_bits(self.to_float(mode, 24, 8, 127) as u32)
    }

    /// `fstp qword`.
    pub fn to_f64(self, mode: Rounding) -> f64 {
        f64::from_bits(self.to_float(mode, 53, 11, 1023))
    }

    /// IEEE bits of `self` rounded to a float with `p` significand bits and
    /// `e_bits` exponent bits.
    fn to_float(self, mode: Rounding, p: u32, e_bits: u32, bias: i32) -> u64 {
        let sign = (self.neg as u64) << (p - 1 + e_bits);
        if self.is_zero() {
            return sign;
        }
        let exponent_field = ((1u64 << e_bits) - 1) << (p - 1);
        if self.is_inf() {
            return sign | exponent_field;
        }
        if self.is_nan() {
            return sign | exponent_field | (self.mant >> (64 - p) & ((1 << (p - 1)) - 1)) | 1 << (p - 2);
        }
        let min_exp = 1 - bias; // exponent of the smallest normal number
        let max_exp = bias;
        let msb = self.magnitude();
        // The unit of the last significand bit: 2^(msb + 1 - p) for normal
        // numbers, the fixed 2^(min_exp + 1 - p) for denormals.
        let quantum = if msb >= min_exp { msb + 1 - p as i32 } else { min_exp + 1 - p as i32 };
        let shift = quantum - self.exp; // bits of mant below the quantum
        let mut units = if shift <= 0 {
            (self.mant as u128) << (-shift)
        } else {
            shift_round(self.mant as u128, shift as u32, false, self.neg, mode)
        };
        // A denormal that rounds up to 2^(p-1) units is the smallest normal
        // number, whose exponent is min_exp already.
        let mut msb_out = if msb >= min_exp { msb } else { min_exp };
        if msb >= min_exp && units >> p != 0 {
            // A normal number that rounded up to 2^p units.
            units >>= 1;
            msb_out += 1;
        }
        if msb_out > max_exp {
            let infinity = match mode {
                Rounding::Nearest => true,
                Rounding::Chop => false,
                Rounding::Up => !self.neg,
                Rounding::Down => self.neg,
            };
            record_status(true, infinity);
            let exp_field = if infinity { (1u64 << e_bits) - 1 } else { (1u64 << e_bits) - 2 };
            let frac = if infinity { 0 } else { (1u64 << (p - 1)) - 1 };
            return sign | exp_field << (p - 1) | frac;
        }
        if units >> (p - 1) == 0 {
            // Denormal (or zero).
            return sign | units as u64;
        }
        let exp_field = (msb_out + bias) as u64;
        sign | exp_field << (p - 1) | (units as u64 & ((1 << (p - 1)) - 1))
    }
}

/// Result of a comparison.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmp {
    Less,
    Equal,
    Greater,
    Unordered,
}

/// The constants `fld1`, `fldz`, `fldpi`, `fldl2e`, `fldl2t`, `fldln2`, `fldlg2`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Constant {
    One,
    Zero,
    Pi,
    L2E,
    L2T,
    Ln2,
    Lg2,
}

impl Value {
    /// The value with the sign cleared (`fabs`; NaNs keep their sign cleared too).
    pub fn abs(self) -> Value {
        Value { neg: false, ..self }
    }

    /// Whether this is a signaling NaN.
    pub fn is_signaling_nan(self) -> bool {
        self.is_nan() && self.mant >> 62 & 1 == 0
    }

    /// `fxam` class as (C3, C2, C0).
    pub fn class(self) -> (bool, bool, bool) {
        if self.is_nan() {
            (false, false, true)
        } else if self.is_inf() {
            (false, true, true)
        } else if self.is_zero() {
            (true, false, false)
        } else {
            (false, true, false)
        }
    }

    /// `fcom`: ordering, unordered if either is a NaN.
    pub fn compare(self, other: Value) -> Cmp {
        if self.is_nan() || other.is_nan() {
            return Cmp::Unordered;
        }
        // Rank by sign and magnitude (zeros equal; infinity above all finite).
        fn key(v: Value) -> (i8, i32, u64) {
            if v.is_zero() {
                (0, 0, 0)
            } else if v.is_inf() {
                (if v.neg { -1 } else { 1 }, i32::MAX, 0)
            } else {
                (if v.neg { -1 } else { 1 }, v.exp + 63, v.mant)
            }
        }
        let (a, b) = (key(self), key(other));
        let ord = |x: (i8, i32, u64), y: (i8, i32, u64)| (x.1, x.2).cmp(&(y.1, y.2));
        let o = if a.0 != b.0 {
            a.0.cmp(&b.0)
        } else if a.0 < 0 {
            ord(b, a)
        } else {
            ord(a, b)
        };
        match o {
            core::cmp::Ordering::Less => Cmp::Less,
            core::cmp::Ordering::Equal => Cmp::Equal,
            core::cmp::Ordering::Greater => Cmp::Greater,
        }
    }

    /// A load-constant instruction's value for the rounding mode.
    pub fn constant(k: Constant, mode: Rounding) -> Value {
        // The first 128 significand bits and the binary exponent of each
        // irrational constant; the bits beyond are not zero.
        let (m, e): (u128, i32) = match k {
            Constant::One => return Value { neg: false, mant: 1 << 63, exp: -63 },
            Constant::Zero => return Value::ZERO,
            Constant::Pi => (0xc90fdaa22168c234c4c6628b80dc1cd1, 1),
            Constant::L2E => (0xb8aa3b295c17f0bbbe87fed0691d3e88, 0),
            Constant::L2T => (0xd49a784bcd1b8afe492bf6ff4dafdb4c, 1),
            Constant::Ln2 => (0xb17217f7d1cf79abc9e3b39803f2f6af, -1),
            Constant::Lg2 => (0x9a209a84fbcff7988f8959ac0b7c9178, -2),
        };
        let mut kept = shift_round(m, 64, true, false, mode) as u128;
        let mut e = e;
        if kept >> 64 != 0 {
            kept >>= 1;
            e += 1;
        }
        Value { neg: false, mant: kept as u64, exp: e - 63 }
    }
}

/// Floor of the square root of the 256-bit number `hi * 2^128 + lo`, and
/// whether the root is exact.
fn isqrt_u256(hi: u128, lo: u128) -> (u128, bool) {
    // (rem_hi, rem_lo) = remainder; digit-by-digit square root, two bits of
    // the radicand at a time.
    let (mut rem_hi, mut rem_lo, mut root) = (0u128, 0u128, 0u128);
    for i in (0..128).rev() {
        let two = if i >= 64 { (hi >> (2 * (i - 64))) & 3 } else { (lo >> (2 * i)) & 3 };
        // rem = rem * 4 + two
        rem_hi = rem_hi << 2 | rem_lo >> 126;
        rem_lo = rem_lo << 2 | two;
        // trial = root * 4 + 1 (up to 130 bits)
        let (t_hi, t_lo) = (root >> 126, root << 2 | 1);
        if (rem_hi, rem_lo) >= (t_hi, t_lo) {
            let (l, borrow) = rem_lo.overflowing_sub(t_lo);
            rem_lo = l;
            rem_hi = rem_hi - t_hi - borrow as u128;
            root = root << 1 | 1;
        } else {
            root <<= 1;
        }
    }
    (root, rem_hi == 0 && rem_lo == 0)
}

impl Value {
    /// `fsqrt`.
    pub fn sqrt(self, c: Control) -> Value {
        if self.is_nan() {
            return Value::nan_pick(self, self);
        }
        if self.is_zero() {
            return self;
        }
        if self.neg {
            return Value::invalid();
        }
        if self.is_inf() {
            return self;
        }
        // mant * 2^exp with an even exponent, scaled up for guard bits.
        let (mant, exp) = if self.exp & 1 == 0 { (self.mant as u128, self.exp) } else { ((self.mant as u128) << 1, self.exp - 1) };
        // N = mant << 120 (< 2^185).
        let (hi, lo) = (mant >> 8, mant << 120);
        let (root, exact) = isqrt_u256(hi, lo);
        Value::round(false, exp / 2 - 60, root, !exact, c)
    }
}

/// Passes `x` through, recording an invalid operation when it is outside `min..=max`.
pub fn record_invalid_if(x: i64, min: i64, max: i64) -> i64 {
    if x < min || x > max {
        record_invalid();
    }
    x
}
