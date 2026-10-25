//! A 256-bit binary floating point number: just enough arithmetic to compute
//! the x87's transcendental functions with correct rounding (the instruction
//! results are rounded to 64 bits; 256 bits of working precision make the
//! error vanishingly unlikely to matter).

use core::cmp::Ordering;

type U256 = [u64; 4]; // little endian limbs

fn shl_bits(a: U256, n: u32) -> U256 {
    if n == 0 {
        return a;
    }
    if n >= 256 {
        return [0; 4];
    }
    let (limbs, bits) = ((n / 64) as usize, n % 64);
    let mut r = [0u64; 4];
    for i in (0..4).rev() {
        if i >= limbs {
            let mut v = a[i - limbs] << bits;
            if bits > 0 && i > limbs {
                v |= a[i - limbs - 1] >> (64 - bits);
            }
            r[i] = v;
        }
    }
    r
}

/// Right shift, reporting whether a one was shifted out.
fn shr_bits(a: U256, n: u32) -> (U256, bool) {
    if n == 0 {
        return (a, false);
    }
    if n >= 256 {
        return ([0; 4], a != [0; 4]);
    }
    let (limbs, bits) = ((n / 64) as usize, n % 64);
    let mut r = [0u64; 4];
    let mut lost = false;
    for i in 0..4 {
        if i < limbs {
            lost |= a[i] != 0;
        }
    }
    if bits > 0 && limbs < 4 {
        lost |= a[limbs] & ((1u64 << bits) - 1) != 0;
    }
    for i in 0..4 {
        if i + limbs < 4 {
            let mut v = a[i + limbs] >> bits;
            if bits > 0 && i + limbs + 1 < 4 {
                v |= a[i + limbs + 1] << (64 - bits);
            }
            r[i] = v;
        }
    }
    (r, lost)
}

fn add256(a: U256, b: U256) -> (U256, bool) {
    let mut r = [0u64; 4];
    let mut carry = false;
    for i in 0..4 {
        let (s1, c1) = a[i].overflowing_add(b[i]);
        let (s2, c2) = s1.overflowing_add(carry as u64);
        r[i] = s2;
        carry = c1 || c2;
    }
    (r, carry)
}

fn sub256(a: U256, b: U256) -> U256 {
    let mut r = [0u64; 4];
    let mut borrow = false;
    for i in 0..4 {
        let (s1, b1) = a[i].overflowing_sub(b[i]);
        let (s2, b2) = s1.overflowing_sub(borrow as u64);
        r[i] = s2;
        borrow = b1 || b2;
    }
    r
}

fn cmp256(a: U256, b: U256) -> Ordering {
    for i in (0..4).rev() {
        match a[i].cmp(&b[i]) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    Ordering::Equal
}

fn leading_zeros(a: U256) -> u32 {
    let mut n = 0;
    for i in (0..4).rev() {
        if a[i] == 0 {
            n += 64;
        } else {
            return n + a[i].leading_zeros();
        }
    }
    n
}

/// Zero, or `m * 2^exp` with the top bit of `m` set.
#[derive(Clone, Copy, Debug)]
pub struct Big {
    pub neg: bool,
    pub exp: i32,
    pub m: U256,
}

impl Big {
    pub const ZERO: Big = Big { neg: false, exp: 0, m: [0; 4] };

    pub fn is_zero(&self) -> bool {
        self.m == [0; 4]
    }

    pub fn from_u64(v: u64) -> Big {
        if v == 0 {
            return Big::ZERO;
        }
        let lz = v.leading_zeros();
        Big { neg: false, exp: -(192 + lz as i32), m: [0, 0, 0, v << lz] }
    }

    pub fn from_i64(v: i64) -> Big {
        let mut b = Big::from_u64(v.unsigned_abs());
        b.neg = v < 0;
        b
    }

    /// `mant * 2^exp` (the 64-bit significand of an extended value).
    pub fn from_parts(neg: bool, mant: u64, exp: i32) -> Big {
        if mant == 0 {
            return Big::ZERO;
        }
        let lz = mant.leading_zeros();
        Big { neg, exp: exp - 192 - lz as i32, m: [0, 0, 0, mant << lz] }
    }

    pub fn neg(mut self) -> Big {
        self.neg = !self.neg;
        self
    }

    pub fn abs(mut self) -> Big {
        self.neg = false;
        self
    }

    /// Multiplication by 2^k.
    pub fn scale(mut self, k: i32) -> Big {
        if !self.is_zero() {
            self.exp += k;
        }
        self
    }

    fn normalize(neg: bool, exp: i32, m: U256) -> Big {
        if m == [0; 4] {
            return Big::ZERO;
        }
        let lz = leading_zeros(m);
        Big { neg, exp: exp - lz as i32, m: shl_bits(m, lz) }
    }

    pub fn add(self, o: Big) -> Big {
        if self.is_zero() {
            return o;
        }
        if o.is_zero() {
            return self;
        }
        // The larger magnitude first (limbs are least significant first: compare with cmp256).
        let self_first = match self.exp.cmp(&o.exp) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => cmp256(self.m, o.m) != Ordering::Less,
        };
        let (a, b) = if self_first { (self, o) } else { (o, self) };
        let d = (a.exp - b.exp) as u32;
        if d > 300 {
            return a;
        }
        let (bm, _) = shr_bits(b.m, d);
        if a.neg == b.neg {
            let (s, carry) = add256(a.m, bm);
            if carry {
                let (h, _) = shr_bits(s, 1);
                let mut h = h;
                h[3] |= 1 << 63;
                Big { neg: a.neg, exp: a.exp + 1, m: h }
            } else {
                Big { neg: a.neg, exp: a.exp, m: s }
            }
        } else {
            Big::normalize(a.neg, a.exp, sub256(a.m, bm))
        }
    }

    pub fn sub(self, o: Big) -> Big {
        self.add(o.neg())
    }

    pub fn mul(self, o: Big) -> Big {
        if self.is_zero() || o.is_zero() {
            return Big::ZERO;
        }
        // 256 x 256 -> 512 bits.
        let mut p = [0u64; 8];
        for i in 0..4 {
            let mut carry = 0u128;
            for j in 0..4 {
                let t = self.m[i] as u128 * o.m[j] as u128 + p[i + j] as u128 + carry;
                p[i + j] = t as u64;
                carry = t >> 64;
            }
            p[i + 4] = carry as u64;
        }
        let hi: U256 = [p[4], p[5], p[6], p[7]];
        let neg = self.neg != o.neg;
        // The product of two normalised numbers has its top bit at 511 or 510.
        Big::normalize(neg, self.exp + o.exp + 256, hi)
    }

    pub fn div(self, o: Big) -> Big {
        assert!(!o.is_zero());
        if self.is_zero() {
            return Big::ZERO;
        }
        // q = floor(self.m * 2^256 / o.m), computed bit by bit (257 bits).
        let mut r: U256 = [0; 4];
        let mut q = [0u64; 5];
        for k in (0..=512u32).rev() {
            // Numerator = self.m << 256: bit k is bit (k - 256) of self.m.
            let nb = if (256..512).contains(&k) { (self.m[((k - 256) / 64) as usize] >> ((k - 256) % 64)) & 1 } else { 0 };
            let top = r[3] >> 63;
            r = shl_bits(r, 1);
            r[0] |= nb;
            if top != 0 || cmp256(r, o.m) != Ordering::Less {
                r = sub256(r, o.m);
                if k <= 256 {
                    q[(k / 64) as usize] |= 1 << (k % 64);
                }
            }
        }
        // q is 256 or 257 bits (bit 256 set when self.m >= o.m); the discarded remainder is below the last bit kept.
        let neg = self.neg != o.neg;
        let (m, exp) = if q[4] & 1 != 0 {
            let (s, _) = shr_bits([q[0], q[1], q[2], q[3]], 1);
            let mut s = s;
            s[3] |= 1 << 63;
            (s, self.exp - o.exp - 256 + 1)
        } else {
            ([q[0], q[1], q[2], q[3]], self.exp - o.exp - 256)
        };
        Big::normalize(neg, exp, m)
    }

    pub fn cmp_abs(&self, o: &Big) -> Ordering {
        match (self.is_zero(), o.is_zero()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => (self.exp, self.m[3], self.m[2], self.m[1], self.m[0]).cmp(&(o.exp, o.m[3], o.m[2], o.m[1], o.m[0])),
        }
    }

    /// Base-2 exponent of the leading bit (`self` nonzero).
    pub fn magnitude(&self) -> i32 {
        self.exp + 255
    }

    /// The integer nearest to `self` (ties to even), for |self| < 2^62.
    pub fn round_to_i64(&self) -> i64 {
        if self.is_zero() {
            return 0;
        }
        assert!(self.magnitude() < 62, "argument too large");
        let shift = (-self.exp).max(0) as u32;
        if shift >= 257 {
            return 0;
        }
        let (int, _) = shr_bits(self.m, shift);
        let mut v = int[0] as i64;
        if shift > 0 {
            let bit = |i: u32| (self.m[(i / 64) as usize] >> (i % 64)) & 1;
            let half = bit(shift - 1) == 1;
            let below = (0..shift - 1).any(|i| bit(i) == 1);
            if half && (below || v & 1 == 1) {
                v += 1;
            }
        }
        if self.neg { -v } else { v }
    }

    /// floor() for a non-negative value below 2^62.
    pub fn round_to_i64_floor(&self) -> i64 {
        if self.is_zero() {
            return 0;
        }
        let shift = (-self.exp).max(0) as u32;
        if shift >= 257 {
            return 0;
        }
        shr_bits(self.m, shift).0[0] as i64
    }

    /// Top 128 bits, the exponent of their lowest bit, and whether lower bits are set.
    pub fn top128(&self) -> (u128, i32, bool) {
        let hi = (self.m[3] as u128) << 64 | self.m[2] as u128;
        (hi, self.exp + 128, self.m[1] != 0 || self.m[0] != 0)
    }
}
