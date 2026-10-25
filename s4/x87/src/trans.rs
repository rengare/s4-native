//! Transcendental x87 instructions (`fyl2x`, `fyl2xp1`, `f2xm1`, `fsin`,
//! `fcos`, `fptan`, `fpatan`) computed with 256-bit arithmetic and rounded
//! once to 64 bits, plus the exact `frndint`, `fscale` and `fprem`/`fprem1`.
//! The hardware's own results of the transcendental instructions differ
//! between CPU makers in the last bit now and then; these are the correctly
//! rounded values (`tests/x87.rs` measures the agreement with the hardware).

use crate::big::Big;
use crate::{Control, Precision, Rounding, Value, INF, NAN};
use std::sync::OnceLock;

fn big_of(v: Value) -> Big {
    Big::from_parts(v.neg, v.mant, v.exp)
}

fn small(n: u64) -> Big {
    Big::from_u64(n)
}

fn finish(b: Big, c: Control) -> Value {
    if b.is_zero() {
        return Value::ZERO;
    }
    let (m, e, sticky) = b.top128();
    Value::round(b.neg, e, m, sticky, Control { rounding: c.rounding, precision: Precision::P64 })
}

/// Series loop: adds terms until they are 2^-262 of the sum.
fn series(first: Big, mut next: impl FnMut(&Big, u64) -> Big) -> Big {
    let mut sum = first;
    let mut term = first;
    for k in 1..400u64 {
        term = next(&term, k);
        if term.is_zero() {
            break;
        }
        sum = sum.add(term);
        if sum.is_zero() || term.magnitude() < sum.magnitude() - 262 {
            break;
        }
    }
    sum
}

/// Σ z^(2k+1)/(2k+1) (atanh), or with alternating signs (atan).
fn odd_series(z: Big, alternate: bool) -> Big {
    if z.is_zero() {
        return Big::ZERO;
    }
    let z2 = z.mul(z);
    let mut power = z;
    let mut sum = z;
    let mut sign_flip = false;
    for k in 1..600u64 {
        power = power.mul(z2);
        sign_flip ^= alternate;
        let mut t = power.div(small(2 * k + 1));
        if sign_flip {
            t = t.neg();
        }
        sum = sum.add(t);
        if t.magnitude() < sum.magnitude() - 262 {
            break;
        }
    }
    sum
}

fn ln2() -> Big {
    static V: OnceLock<Big> = OnceLock::new();
    *V.get_or_init(|| odd_series(small(1).div(small(3)), false).scale(1))
}

fn pi() -> Big {
    static V: OnceLock<Big> = OnceLock::new();
    *V.get_or_init(|| {
        // Machin: pi = 16 atan(1/5) - 4 atan(1/239).
        let a = odd_series(small(1).div(small(5)), true).scale(4);
        let b = odd_series(small(1).div(small(239)), true).scale(2);
        a.sub(b)
    })
}

/// atan(k/4) for k = 0..=4.
fn atan_table() -> &'static [Big; 5] {
    static V: OnceLock<[Big; 5]> = OnceLock::new();
    V.get_or_init(|| {
        let quarter_pi = pi().scale(-2);
        let a1 = odd_series(small(1).div(small(4)), true);
        let a2 = odd_series(small(1).div(small(2)), true);
        // atan(3/4) = pi/4 - atan(1/7).
        let a3 = quarter_pi.sub(odd_series(small(1).div(small(7)), true));
        [Big::ZERO, a1, a2, a3, quarter_pi]
    })
}

/// atan(t) for 0 <= t <= 1.
fn atan_unit(t: Big) -> Big {
    if t.is_zero() {
        return Big::ZERO;
    }
    // Nearest k/4.
    let k = t.mul(small(4)).add(Big::from_u64(1).scale(-1)); // 4t + 1/2
    let k = if k.is_zero() || k.neg { 0 } else { k.round_to_i64_floor().min(4) as usize };
    let c = Big::from_u64(k as u64).div(small(4));
    let u = t.sub(c).div(small(1).add(t.mul(c)));
    atan_table()[k].add(odd_series(u, true))
}

/// log2 of a positive finite value, as a Big (|x| must not be 1: callers handle that).
fn log2_big(x: Big) -> Big {
    // x = f * 2^k with f in [1, 2); recentre f to [0.707, 1.414].
    let mut k = x.exp + 255;
    let mut f = Big { neg: false, exp: -255, m: x.m };
    if f.cmp_abs(&Big::from_parts(false, 0xb504_f333_f9de_6484, -63)) == core::cmp::Ordering::Greater {
        f = f.scale(-1);
        k += 1;
    }
    let one = small(1);
    let z = f.sub(one).div(f.add(one));
    let ln_f = odd_series(z, false).scale(1);
    Big::from_i64(k as i64).add(ln_f.div(ln2()))
}

fn indefinite() -> Value {
    crate::record_invalid();
    Value::INDEFINITE
}

impl Value {
    fn is_finite_nonzero(self) -> bool {
        self.exp != INF && self.exp != NAN && !(self.mant == 0 && self.exp == 0)
    }

    /// `fyl2x`: `y * log2(x)` (x = ST(0), y = ST(1)).
    pub fn fyl2x(x: Value, y: Value, c: Control) -> Value {
        if x.is_nan() || y.is_nan() {
            return Value::nan_pick(x, y);
        }
        if x.neg && !x.is_zero() {
            return indefinite();
        }
        if x.is_zero() {
            // log2(0) = -inf.
            return if y.is_zero() { indefinite() } else { Value::inf(!y.neg) };
        }
        if x.is_inf() {
            return if y.is_zero() { indefinite() } else { Value::inf(y.neg) };
        }
        if y.is_inf() {
            // log2(1) = 0: inf * 0 is invalid.
            return if x.compare(Value::constant(crate::Constant::One, Rounding::Nearest)) == crate::Cmp::Equal { indefinite() } else { Value::inf(y.neg != (x.magnitude() < 0)) };
        }
        if y.is_zero() {
            return Value::zero(y.neg);
        }
        if x.compare(Value::constant(crate::Constant::One, Rounding::Nearest)) == crate::Cmp::Equal {
            return Value::zero(y.neg);
        }
        finish(big_of(y).mul(log2_big(big_of(x))), c)
    }

    /// `fyl2xp1`: `y * log2(x + 1)`, |x| < 1 - sqrt(2)/2.
    pub fn fyl2xp1(x: Value, y: Value, c: Control) -> Value {
        if x.is_nan() || y.is_nan() {
            return Value::nan_pick(x, y);
        }
        if x.is_inf() || y.is_inf() {
            return indefinite();
        }
        if x.is_zero() || y.is_zero() {
            return Value::zero(x.neg != y.neg);
        }
        let xb = big_of(x);
        let one = small(1);
        // log2(1+x) = 2 atanh(x / (2 + x)) / ln 2
        let z = xb.div(small(2).add(xb));
        let log = odd_series(z, false).scale(1).div(ln2());
        let _ = one;
        finish(big_of(y).mul(log), c)
    }

    /// `f2xm1`: `2^x - 1` for |x| <= 1.
    pub fn f2xm1(self, c: Control) -> Value {
        if self.is_nan() {
            return Value::nan_pick(self, self);
        }
        if self.is_zero() {
            return self;
        }
        if self.is_inf() {
            return if self.neg { Value::constant(crate::Constant::One, Rounding::Nearest).neg() } else { self };
        }
        let t = big_of(self).mul(ln2());
        // e^t - 1 = Σ_{k>=1} t^k / k!
        let sum = series(t, |term, k| term.mul(t).div(small(k + 1)));
        finish(sum, c)
    }

    /// sin, cos or tan of `self` (`which` 0, 1, 2); `None` outside |x| < 2^62.
    pub fn sincos(self, which: u8, c: Control) -> Option<Value> {
        if self.is_nan() {
            return Some(Value::nan_pick(self, self));
        }
        if self.is_inf() {
            return Some(indefinite());
        }
        if self.is_zero() {
            return Some(match which {
                0 | 2 => self,
                _ => Value::constant(crate::Constant::One, Rounding::Nearest),
            });
        }
        if self.magnitude() >= 62 {
            return None;
        }
        let x = big_of(self);
        let half_pi = pi().scale(-1);
        let n = x.div(half_pi).round_to_i64();
        let r = x.sub(Big::from_i64(n).mul(half_pi));
        let r2 = r.mul(r);
        // sin r = r - r^3/3! + ..., cos r = 1 - r^2/2! + ...
        let sin_r = if r.is_zero() { Big::ZERO } else { series(r, |term, k| term.mul(r2).div(small((2 * k) * (2 * k + 1))).neg()) };
        let cos_r = series(small(1), |term, k| term.mul(r2).div(small((2 * k - 1) * (2 * k))).neg());
        let (s, co) = match n.rem_euclid(4) {
            0 => (sin_r, cos_r),
            1 => (cos_r, sin_r.neg()),
            2 => (sin_r.neg(), cos_r.neg()),
            _ => (cos_r.neg(), sin_r),
        };
        Some(match which {
            0 => finish(s, c),
            1 => finish(co, c),
            _ => finish(s.div(co), c),
        })
    }

    /// `fpatan`: atan2(y, x) with y = ST(1), x = ST(0).
    pub fn fpatan(y: Value, x: Value, c: Control) -> Value {
        if x.is_nan() || y.is_nan() {
            return Value::nan_pick(x, y);
        }
        let pi_v = |num: u64, den: u64, neg: bool| -> Value {
            let b = pi().mul(small(num)).div(small(den));
            let mut v = finish(b, c);
            v.neg = neg;
            v
        };
        if y.is_zero() {
            return if x.neg { pi_v(1, 1, y.neg) } else { Value::zero(y.neg) };
        }
        if x.is_zero() {
            return pi_v(1, 2, y.neg);
        }
        if y.is_inf() {
            return match (x.is_inf(), x.neg) {
                (false, _) => pi_v(1, 2, y.neg),
                (true, false) => pi_v(1, 4, y.neg),
                (true, true) => pi_v(3, 4, y.neg),
            };
        }
        if x.is_inf() {
            return if x.neg { pi_v(1, 1, y.neg) } else { Value::zero(y.neg) };
        }
        let (ay, ax) = (big_of(y).abs(), big_of(x).abs());
        let swap = ay.cmp_abs(&ax) == core::cmp::Ordering::Greater;
        let t = if swap { ax.div(ay) } else { ay.div(ax) };
        let mut a = atan_unit(t);
        if swap {
            a = pi().scale(-1).sub(a);
        }
        if x.neg {
            a = pi().sub(a);
        }
        a.neg = y.neg;
        finish(a, c)
    }

    /// `frndint`.
    pub fn rndint(self, mode: Rounding) -> Value {
        if !self.is_finite_nonzero() || self.exp >= 0 {
            return if self.is_nan() { Value::nan_pick(self, self) } else { self };
        }
        let k = crate::shift_round(self.mant as u128, (-self.exp) as u32, false, self.neg, mode);
        if k == 0 { Value::zero(self.neg) } else { Value::normalized(self.neg, k as u64, 0) }
    }

    /// `fscale`: `self * 2^trunc(n)`.
    pub fn scale(self, n: Value) -> Value {
        if self.is_nan() || n.is_nan() {
            return Value::nan_pick(self, n);
        }
        if n.is_inf() {
            return match (n.neg, self.is_zero(), self.is_inf()) {
                (false, true, _) | (true, _, true) => indefinite(),
                (false, _, _) => if self.is_inf() { self } else { Value::inf(self.neg) },
                (true, _, _) => Value::zero(self.neg),
            };
        }
        if self.is_zero() || self.is_inf() || n.is_zero() {
            return self;
        }
        let k = match n.to_integer(Rounding::Chop) {
            Some(m) if m < 1 << 40 => if n.neg { -(m as i64) } else { m as i64 },
            _ => panic!("x87: fscale by a huge exponent is not modelled"),
        };
        let e = self.exp as i64 + k;
        assert!((-16382 - 63..=16383 - 63).contains(&e), "x87: result outside the normal extended exponent range");
        Value { exp: e as i32, ..self }
    }

    /// `fprem` (`ieee` false, quotient truncated) or `fprem1` (nearest): the
    /// remainder, the low three quotient bits, and whether the reduction is
    /// complete (else it was partial and C2 is set).
    pub fn prem(self, y: Value, ieee: bool) -> (Value, u32, bool) {
        if self.is_nan() || y.is_nan() {
            return (Value::nan_pick(self, y), 0, true);
        }
        if self.is_inf() || y.is_zero() {
            return (indefinite(), 0, true);
        }
        if y.is_inf() || self.is_zero() {
            return (self, 0, true);
        }
        let d = self.exp - y.exp;
        if d < 0 {
            // |x| < |y| (or within a factor of 2: exponents equal handled below).
        }
        if d >= 64 {
            // Partial reduction by 2^(d - 32)... of the quotient: reduce the exponent difference by N = 32.
            let reduce = d - 32;
            let (xm, ym) = (self.mant as u128, y.mant as u128);
            // x = xm * 2^ex ; y' = ym * 2^(ey + reduce); q = floor(x / y').
            let num = xm << 32; // xm * 2^(ex - ey - reduce) with ex - ey - reduce = 32
            let q = num / ym;
            let r = num % ym;
            let result = if r == 0 { Value::zero(self.neg) } else { Value::normalized_u128(self.neg, r, y.exp + reduce) };
            return (result, (q & 7) as u32, false);
        }
        // Complete: align exactly.
        let (xm, ym) = (self.mant as u128, y.mant as u128);
        let (num, den, base) = if d >= 0 { (xm << d, ym, y.exp) } else { (xm, ym << (-d), self.exp) };
        let mut q = num / den;
        let mut r = num % den;
        let mut neg = self.neg;
        if ieee && (2 * r > den || (2 * r == den && q & 1 == 1)) {
            r = den - r;
            q += 1;
            neg = !neg;
        }
        let result = if r == 0 { Value::zero(self.neg) } else { Value::normalized_u128(neg, r, base) };
        (result, (q & 7) as u32, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(b: Big) -> f64 {
        let (m, e, _) = b.top128();
        (m as f64) * (2.0f64).powi(e) * if b.neg { -1.0 } else { 1.0 }
    }

    #[test]
    fn constants() {
        assert!((f(ln2()) - core::f64::consts::LN_2).abs() < 1e-15, "{}", f(ln2()));
        assert!((f(pi()) - core::f64::consts::PI).abs() < 1e-15, "{}", f(pi()));
        let q = small(1).div(small(3));
        assert!((f(q) - 1.0 / 3.0).abs() < 1e-15);
        assert!((f(small(3).mul(small(5))) - 15.0).abs() < 1e-12);
        assert!((f(small(3).add(small(5))) - 8.0).abs() < 1e-12);
    }
}


#[cfg(test)]
mod sincos_regression {
    use super::*;

    /// `Big::add` once picked the larger operand by comparing limbs from the wrong end, which
    /// broke fsin/fcos for arguments just above a multiple of pi/2 (found through Bink's FFT tables).
    #[test]
    fn sincos_past_half_pi() {
        let c = Control::from_word(0x37f);
        for x in [1.58f64, 1.9, 4.8, 5.0, 7.0, -2.2] {
            let v = Value::from_f64(x);
            let s = v.sincos(0, c).unwrap().to_f64(Rounding::Nearest);
            let co = v.sincos(1, c).unwrap().to_f64(Rounding::Nearest);
            assert!((s - x.sin()).abs() < 1e-14, "sin({x}) = {s}");
            assert!((co - x.cos()).abs() < 1e-14, "cos({x}) = {co}");
        }
    }
}
