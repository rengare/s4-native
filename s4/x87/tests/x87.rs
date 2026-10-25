//! The software x87 against the real FPU (inline assembly, run under Wine on
//! an x86 host): add, subtract, multiply, divide, float and integer stores,
//! for all four rounding modes and all three precisions, on random operands
//! (including exact ties and operands with few significant bits) and on
//! hand-picked edge cases.
#![cfg(all(windows, target_arch = "x86"))]

use core::arch::asm;
use s4_x87::{Control, Precision, Rounding, Value};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// A random extended value: random exponent in a window where the sums and
/// products stay normal, a significand with a random number of significant
/// bits (few bits make exact ties and exact results likely).
fn random(rng: &mut Rng, window: i32) -> Value {
    if rng.next() % 40 == 0 {
        return Value::from_f32(0.0);
    }
    let bits = [1, 2, 3, 8, 24, 25, 53, 54, 64, 64, 64][(rng.next() % 11) as usize];
    let mant = (rng.next() | 1 << 63) & (u64::MAX << (64 - bits));
    let exp = 16383 + (rng.next() % (2 * window as u64 + 1)) as i32 - window;
    let sign = (rng.next() % 2) as u16;
    let mut bytes = [0u8; 10];
    bytes[..8].copy_from_slice(&mant.to_le_bytes());
    bytes[8..].copy_from_slice(&((sign << 15) | exp as u16).to_le_bytes());
    Value::from_tbyte(bytes)
}

fn controls() -> Vec<Control> {
    let mut all = Vec::new();
    for rounding in [Rounding::Nearest, Rounding::Down, Rounding::Up, Rounding::Chop] {
        for precision in [Precision::P24, Precision::P53, Precision::P64] {
            all.push(Control { rounding, precision });
        }
    }
    all
}

/// The FPU result of `a op b` as 80-bit memory.
fn hardware(op: &str, a: Value, b: Value, c: Control) -> [u8; 10] {
    let (ta, tb, mut out, cw) = (a.to_tbyte(), b.to_tbyte(), [0u8; 10], c.word());
    unsafe {
        match op {
            "add" => asm!("fldcw [{cw}]", "fld tbyte ptr [{a}]", "fld tbyte ptr [{b}]", "faddp st(1), st", "fstp tbyte ptr [{o}]", cw = in(reg) &cw, a = in(reg) &ta, b = in(reg) &tb, o = in(reg) &mut out, options(nostack)),
            "sub" => asm!("fldcw [{cw}]", "fld tbyte ptr [{a}]", "fld tbyte ptr [{b}]", "fsubp st(1), st", "fstp tbyte ptr [{o}]", cw = in(reg) &cw, a = in(reg) &ta, b = in(reg) &tb, o = in(reg) &mut out, options(nostack)),
            "mul" => asm!("fldcw [{cw}]", "fld tbyte ptr [{a}]", "fld tbyte ptr [{b}]", "fmulp st(1), st", "fstp tbyte ptr [{o}]", cw = in(reg) &cw, a = in(reg) &ta, b = in(reg) &tb, o = in(reg) &mut out, options(nostack)),
            "div" => asm!("fldcw [{cw}]", "fld tbyte ptr [{a}]", "fld tbyte ptr [{b}]", "fdivp st(1), st", "fstp tbyte ptr [{o}]", cw = in(reg) &cw, a = in(reg) &ta, b = in(reg) &tb, o = in(reg) &mut out, options(nostack)),
            _ => unreachable!(),
        }
        asm!("fldcw [{cw}]", cw = in(reg) &0x027fu16, options(nostack));
    }
    out
}

fn software(op: &str, a: Value, b: Value, c: Control) -> [u8; 10] {
    match op {
        "add" => a.add(b, c),
        "sub" => a.sub(b, c),
        "mul" => a.mul(b, c),
        "div" => a.div(b, c),
        _ => unreachable!(),
    }
    .to_tbyte()
}

/// Both zeros compare equal as values; the sign of a zero result matters
/// and is compared too.
fn check_arithmetic(rng: &mut Rng, window: i32, rounds: usize) {
    for c in controls() {
        for op in ["add", "sub", "mul", "div"] {
            for _ in 0..rounds {
                let (a, mut b) = (random(rng, window), random(rng, window));
                if op == "div" && b == Value::from_f32(0.0) || (op == "div" && b.to_tbyte()[..8] == [0; 8]) {
                    b = Value::from_i32(3);
                }
                // Operands close to each other exercise cancellation.
                let b = if rng.next() % 5 == 0 && op != "mul" && op != "div" { a.neg().add(Value::from_i32(0), Control { rounding: Rounding::Nearest, precision: Precision::P64 }) } else { b };
                let (want, got) = (hardware(op, a, b, c), software(op, a, b, c));
                assert_eq!(got, want, "{op} {:02x?} {:02x?} under {:#06x}", a.to_tbyte(), b.to_tbyte(), c.word());
            }
        }
    }
}

#[test]
fn arithmetic_matches_the_fpu() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    check_arithmetic(&mut rng, 30, 6000);
    check_arithmetic(&mut rng, 3, 6000);
    check_arithmetic(&mut rng, 300, 2000);
}

#[test]
fn cancellation_matches_the_fpu() {
    // a + (-a + small) and nearly equal operands.
    let mut rng = Rng(0x0dd_ba11);
    let ext = Control { rounding: Rounding::Nearest, precision: Precision::P64 };
    for c in controls() {
        for _ in 0..4000 {
            let a = random(&mut rng, 20);
            let delta = Value::from_i32((rng.next() % 5) as i32 - 2).mul(Value::from_f64(f64::powi(2.0, -((rng.next() % 70) as i32))), ext);
            let b = a.add(delta, ext).neg();
            for op in ["add", "sub"] {
                let (want, got) = (hardware(op, a, b, c), software(op, a, b, c));
                assert_eq!(got, want, "{op} {:02x?} {:02x?} under {:#06x}", a.to_tbyte(), b.to_tbyte(), c.word());
            }
        }
    }
}

fn fistp32(a: Value, c: Control) -> i32 {
    let (t, mut out, cw) = (a.to_tbyte(), 0i32, c.word());
    unsafe {
        asm!("fldcw [{cw}]", "fld tbyte ptr [{a}]", "fistp dword ptr [{o}]", "fldcw [{d}]", cw = in(reg) &cw, a = in(reg) &t, o = in(reg) &mut out, d = in(reg) &0x027fu16, options(nostack));
    }
    out
}

fn fistp64(a: Value, c: Control) -> i64 {
    let (t, mut out, cw) = (a.to_tbyte(), 0i64, c.word());
    unsafe {
        asm!("fldcw [{cw}]", "fld tbyte ptr [{a}]", "fistp qword ptr [{o}]", "fldcw [{d}]", cw = in(reg) &cw, a = in(reg) &t, o = in(reg) &mut out, d = in(reg) &0x027fu16, options(nostack));
    }
    out
}

fn fstp32(a: Value, c: Control) -> u32 {
    let (t, mut out, cw) = (a.to_tbyte(), 0u32, c.word());
    unsafe {
        asm!("fldcw [{cw}]", "fld tbyte ptr [{a}]", "fstp dword ptr [{o}]", "fldcw [{d}]", cw = in(reg) &cw, a = in(reg) &t, o = in(reg) &mut out, d = in(reg) &0x027fu16, options(nostack));
    }
    out
}

fn fstp64(a: Value, c: Control) -> u64 {
    let (t, mut out, cw) = (a.to_tbyte(), 0u64, c.word());
    unsafe {
        asm!("fldcw [{cw}]", "fld tbyte ptr [{a}]", "fstp qword ptr [{o}]", "fldcw [{d}]", cw = in(reg) &cw, a = in(reg) &t, o = in(reg) &mut out, d = in(reg) &0x027fu16, options(nostack));
    }
    out
}

#[test]
fn stores_match_the_fpu() {
    let mut rng = Rng(0xfeed_f00d_1234);
    for c in controls() {
        for window in [3, 40, 140, 1100] {
            for _ in 0..5000 {
                let a = random(&mut rng, window);
                assert_eq!(a.to_i32(c.rounding), fistp32(a, c), "fistp dword {:02x?} {:#06x}", a.to_tbyte(), c.word());
                assert_eq!(a.to_i64(c.rounding), fistp64(a, c), "fistp qword {:02x?} {:#06x}", a.to_tbyte(), c.word());
                assert_eq!(a.to_f32(c.rounding).to_bits(), fstp32(a, c), "fstp dword {:02x?} {:#06x}", a.to_tbyte(), c.word());
                assert_eq!(a.to_f64(c.rounding).to_bits(), fstp64(a, c), "fstp qword {:02x?} {:#06x}", a.to_tbyte(), c.word());
            }
        }
    }
}

#[test]
fn loads_and_edge_values() {
    // Exact loads of floats, integers and a few edge values through stores.
    let c = Control::DEFAULT;
    for f in [0.0f32, -0.0, 1.0, -1.5, 0.1, 3.4028235e38, 1.17549435e-38, 1.0e-45, 16777217.0, 2147483648.0] {
        assert_eq!(Value::from_f32(f).to_f32(c.rounding).to_bits(), f.to_bits());
    }
    for f in [0.0f64, -0.0, 1.0, 0.1, 1.7976931348623157e308, 2.2250738585072014e-308, 5e-324, 9007199254740993.0] {
        assert_eq!(Value::from_f64(f).to_f64(c.rounding).to_bits(), f.to_bits());
    }
    for i in [0i32, 1, -1, i32::MAX, i32::MIN, 123456789] {
        assert_eq!(Value::from_i32(i).to_i32(Rounding::Chop), i);
    }
    // The values the game's rounding hinges on: 2.5 and 3.83 in each mode.
    for (x, nearest, chop, down, up) in [(2.5f64, 2, 2, 2, 3), (3.83, 4, 3, 3, 4), (-1.5, -2, -1, -2, -1), (0.5, 0, 0, 0, 1)] {
        let v = Value::from_f64(x);
        assert_eq!((v.to_i32(Rounding::Nearest), v.to_i32(Rounding::Chop), v.to_i32(Rounding::Down), v.to_i32(Rounding::Up)), (nearest, chop, down, up));
    }
    // Overflow of the integer store: "integer indefinite".
    assert_eq!(Value::from_f64(3.0e9).to_i32(Rounding::Chop), i32::MIN);
    assert_eq!(Value::from_f64(-2147483648.0).to_i32(Rounding::Chop), i32::MIN);
    assert_eq!(Value::from_f64(2147483647.5).to_i32(Rounding::Chop), 2147483647);
}

#[test]
fn control_words() {
    for c in controls() {
        assert_eq!(Control::from_word(c.word()), c);
    }
    assert_eq!(Control::DEFAULT.word(), 0x027f);
    assert_eq!(Control::CHOP.word(), 0x0e7f);
}

fn specials() -> Vec<Value> {
    let tb = |mant: u64, se: u16| {
        let mut b = [0u8; 10];
        b[..8].copy_from_slice(&mant.to_le_bytes());
        b[8..].copy_from_slice(&se.to_le_bytes());
        Value::from_tbyte(b)
    };
    vec![
        tb(0, 0),
        tb(0, 0x8000),
        tb(1 << 63, 0x7fff),
        tb(1 << 63, 0xffff),
        tb(3 << 62, 0xffff), // the real indefinite
        Value::from_i32(0),
        Value::from_i64(0),
        Value::from_i32(1),
        Value::from_i32(-2),
        Value::from_f64(3.5),
        Value::from_f64(-1.0e-30),
    ]
}

#[test]
fn infinities_nan_and_zeros_match_the_fpu() {
    for c in controls() {
        for a in specials() {
            for b in specials() {
                for op in ["add", "sub", "mul", "div"] {
                    let (want, got) = (hardware(op, a, b, c), software(op, a, b, c));
                    assert_eq!(got, want, "{op} {:02x?} {:02x?} under {:#06x}", a.to_tbyte(), b.to_tbyte(), c.word());
                }
            }
            assert_eq!(a.to_i32(c.rounding), fistp32(a, c), "fistp dword {:02x?}", a.to_tbyte());
            assert_eq!(a.to_i64(c.rounding), fistp64(a, c), "fistp qword {:02x?}", a.to_tbyte());
            assert_eq!(a.to_f32(c.rounding).to_bits(), fstp32(a, c), "fstp dword {:02x?} {:#06x}", a.to_tbyte(), c.word());
            assert_eq!(a.to_f64(c.rounding).to_bits(), fstp64(a, c), "fstp qword {:02x?} {:#06x}", a.to_tbyte(), c.word());
        }
    }
}

fn hardware_load_f32(v: f32) -> [u8; 10] {
    let mut out = [0u8; 10];
    unsafe { asm!("fld dword ptr [{v}]", "fstp tbyte ptr [{o}]", v = in(reg) &v, o = in(reg) &mut out, options(nostack)) };
    out
}

fn hardware_load_f64(v: f64) -> [u8; 10] {
    let mut out = [0u8; 10];
    unsafe { asm!("fld qword ptr [{v}]", "fstp tbyte ptr [{o}]", v = in(reg) &v, o = in(reg) &mut out, options(nostack)) };
    out
}

fn hardware_load_i32(v: i32) -> [u8; 10] {
    let mut out = [0u8; 10];
    unsafe { asm!("fild dword ptr [{v}]", "fstp tbyte ptr [{o}]", v = in(reg) &v, o = in(reg) &mut out, options(nostack)) };
    out
}

fn hardware_load_i64(v: i64) -> [u8; 10] {
    let mut out = [0u8; 10];
    unsafe { asm!("fild qword ptr [{v}]", "fstp tbyte ptr [{o}]", v = in(reg) &v, o = in(reg) &mut out, options(nostack)) };
    out
}

#[test]
fn loads_match_the_fpu() {
    let mut rng = Rng(0xabcd_ef01_2345);
    for v in [0i32, 1, -1, 2, i32::MAX, i32::MIN, 123456789, -987654321] {
        assert_eq!(Value::from_i32(v).to_tbyte(), hardware_load_i32(v), "fild dword {v}");
    }
    for v in [0i64, 1, -1, i64::MAX, i64::MIN, 1 << 40, -(1 << 62) - 12345] {
        assert_eq!(Value::from_i64(v).to_tbyte(), hardware_load_i64(v), "fild qword {v}");
    }
    for _ in 0..20000 {
        let r = rng.next();
        let (i, j) = (r as i32, (r >> 7) as i64 ^ ((r as i64) << 20));
        assert_eq!(Value::from_i32(i >> (r % 31) as u32).to_tbyte(), hardware_load_i32(i >> (r % 31) as u32));
        assert_eq!(Value::from_i64(j >> (r % 63) as u32).to_tbyte(), hardware_load_i64(j >> (r % 63) as u32));
        // Finite floats of every exponent, denormals included.
        let f = f32::from_bits(rng.next() as u32);
        if f.is_finite() {
            assert_eq!(Value::from_f32(f).to_tbyte(), hardware_load_f32(f), "fld dword {:#x}", f.to_bits());
        }
        let d = f64::from_bits(rng.next());
        if d.is_finite() {
            assert_eq!(Value::from_f64(d).to_tbyte(), hardware_load_f64(d), "fld qword {:#x}", d.to_bits());
        }
        // Denormals and the smallest normals.
        let f = f32::from_bits((rng.next() as u32) & 0x00ff_ffff | ((rng.next() as u32) & 0x8000_0000));
        assert_eq!(Value::from_f32(f).to_tbyte(), hardware_load_f32(f));
        let d = f64::from_bits(rng.next() & 0x001f_ffff_ffff_ffff | (rng.next() & 0x8000_0000_0000_0000));
        assert_eq!(Value::from_f64(d).to_tbyte(), hardware_load_f64(d));
    }
    for v in [0.0f32, -0.0, 1.0, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(Value::from_f32(v).to_tbyte(), hardware_load_f32(v));
    }
    for v in [0.0f64, -0.0, 1.0, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(Value::from_f64(v).to_tbyte(), hardware_load_f64(v));
    }
}

#[test]
fn far_apart_operands_match_the_fpu() {
    // The smaller operand lies below or around the end of the larger one's
    // significand: sticky bits decide ties.
    let mut rng = Rng(0x5eed_0f_f1a5);
    let ext = Control { rounding: Rounding::Nearest, precision: Precision::P64 };
    for c in controls() {
        for _ in 0..8000 {
            let a = random(&mut rng, 4);
            let k = 20 + (rng.next() % 130) as i32;
            let b = random(&mut rng, 4).mul(Value::from_f64(f64::powi(2.0, -k.min(1000))).mul(Value::from_f64(f64::powi(2.0, -(k - k.min(1000)))), ext), ext);
            for op in ["add", "sub"] {
                let (want, got) = (hardware(op, a, b, c), software(op, a, b, c));
                assert_eq!(got, want, "{op} {:02x?} {:02x?} under {:#06x}", a.to_tbyte(), b.to_tbyte(), c.word());
                let (want, got) = (hardware(op, b, a, c), software(op, b, a, c));
                assert_eq!(got, want, "{op} {:02x?} {:02x?} under {:#06x}", b.to_tbyte(), a.to_tbyte(), c.word());
            }
        }
    }
}

#[test]
fn stores_near_float_boundaries_match_the_fpu() {
    // Values around the smallest normal, the denormal range and the largest
    // finite float, with full and rounded-up significands.
    let mut rng = Rng(0xb0_0d_a5);
    for c in controls() {
        for (centre, spread) in [(-126i32, 30i32), (-1022, 60), (127, 3), (1023, 3), (-149, 4), (-1074, 4)] {
            for _ in 0..3000 {
                let bits = [1, 2, 22, 23, 24, 25, 26, 52, 53, 54, 64][(rng.next() % 11) as usize];
                let mant = (rng.next() | 1 << 63) & (u64::MAX << (64 - bits)) | if rng.next() % 3 == 0 { (1u64 << (64 - bits).min(63)) - 1 & !(1 << 63) } else { 0 };
                let e = 16383 + centre + (rng.next() % (2 * spread as u64 + 1)) as i32 - spread;
                let mut b = [0u8; 10];
                b[..8].copy_from_slice(&mant.to_le_bytes());
                b[8..].copy_from_slice(&(((rng.next() % 2) as u16) << 15 | e as u16).to_le_bytes());
                let a = Value::from_tbyte(b);
                assert_eq!(a.to_f32(c.rounding).to_bits(), fstp32(a, c), "fstp dword {:02x?} {:#06x}", a.to_tbyte(), c.word());
                assert_eq!(a.to_f64(c.rounding).to_bits(), fstp64(a, c), "fstp qword {:02x?} {:#06x}", a.to_tbyte(), c.word());
            }
        }
    }
}

#[test]
fn memory_format_round_trips() {
    let mut rng = Rng(0x7b17e5);
    for _ in 0..20000 {
        let v = random(&mut rng, 16000);
        assert_eq!(Value::from_tbyte(v.to_tbyte()).to_tbyte(), v.to_tbyte());
    }
    for v in specials() {
        assert_eq!(Value::from_tbyte(v.to_tbyte()), v);
    }
    // The special values in their memory format, written out: infinity,
    // negative infinity, the real indefinite.
    for bytes in [[0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f], [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0xff], [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0xff]] {
        assert_eq!(Value::from_tbyte(bytes).to_tbyte(), bytes);
    }
    // 1.0 and -2.5 by hand: significand, then sign and biased exponent.
    let mut one = [0u8; 10];
    one[7] = 0x80;
    one[8] = 0xff;
    one[9] = 0x3f;
    assert_eq!(Value::from_tbyte(one), Value::from_i32(1));
    assert_eq!(Value::from_i32(-2).to_tbyte(), [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0xc0]);
}

#[test]
fn nan_and_infinity_loads_match_the_fpu() {
    for bits in [0x7fc0_0000u32, 0xffc0_0000, 0x7f80_0000, 0xff80_0000] {
        let v = f32::from_bits(bits);
        assert_eq!(Value::from_f32(v).to_tbyte(), hardware_load_f32(v), "fld dword {bits:#x}");
    }
    for bits in [0x7ff8_0000_0000_0000u64, 0xfff8_0000_0000_0000, 0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000] {
        let v = f64::from_bits(bits);
        assert_eq!(Value::from_f64(v).to_tbyte(), hardware_load_f64(v), "fld qword {bits:#x}");
    }
}

#[test]
fn half_ulp_ties_with_far_away_sticky_bits_match_the_fpu() {
    // The smaller operand starts exactly at the half-way point of the larger
    // one's 64-bit significand (distance 64) or lower, with one more bit far
    // below: only the sticky bit decides the rounding.
    let tb = |mant: u64, exp: i32, neg: bool| {
        let mut b = [0u8; 10];
        b[..8].copy_from_slice(&mant.to_le_bytes());
        b[8..].copy_from_slice(&(((neg as u16) << 15) | (16383 + exp) as u16).to_le_bytes());
        Value::from_tbyte(b)
    };
    for c in controls() {
        for a_mant in [1u64 << 63, (1 << 63) | 1, (1 << 63) | 2, 0xffff_ffff_ffff_ffff, 0xdead_beef_0000_0001, 0xdead_beef_0000_0000] {
            for d in 56..=130 {
                for b_mant in [1u64 << 63, (1 << 63) | 1, (1 << 63) | (1 << 32), 0xc000_0000_0000_0001, u64::MAX] {
                    for (neg_a, neg_b) in [(false, false), (false, true), (true, false)] {
                        let (a, b) = (tb(a_mant, 0, neg_a), tb(b_mant, -d, neg_b));
                        for op in ["add", "sub"] {
                            let (want, got) = (hardware(op, a, b, c), software(op, a, b, c));
                            assert_eq!(got, want, "{op} {:02x?} {:02x?} under {:#06x}", a.to_tbyte(), b.to_tbyte(), c.word());
                        }
                    }
                }
            }
        }
    }
}


/// Runs `body` (x87 instructions on ST(0), ST(1)) with `a` as ST(1) and `b` as ST(0) and returns
/// (ST(0), ST(1), status word). `ops` is the instruction text.
macro_rules! run_hw {
    ($a:expr, $b:expr, $c:expr, $($ins:literal),+) => {{
        let mut buf = [0u8; 80];
        buf[..10].copy_from_slice(&$a.to_tbyte());
        buf[16..26].copy_from_slice(&$b.to_tbyte());
        buf[64..66].copy_from_slice(&$c.word().to_le_bytes());
        buf[66..68].copy_from_slice(&0x027fu16.to_le_bytes());
        unsafe {
            asm!("fnclex", "fldcw [{p} + 64]", "fld tbyte ptr [{p}]", "fld tbyte ptr [{p} + 16]", $($ins,)+ "fnstsw [{p} + 68]", "fstp tbyte ptr [{p} + 32]", "fstp tbyte ptr [{p} + 48]", "fldcw [{p} + 66]", p = in(reg) buf.as_mut_ptr(), options(nostack));
        }
        let (mut r0, mut r1) = ([0u8; 10], [0u8; 10]);
        r0.copy_from_slice(&buf[32..42]);
        r1.copy_from_slice(&buf[48..58]);
        (r0, r1, u16::from_le_bytes([buf[68], buf[69]]))
    }};
}

fn small_value(rng: &mut Rng, lo: i32, hi: i32) -> Value {
    let mant = rng.next() | 1 << 63;
    let exp = 16383 + lo + (rng.next() % (hi - lo + 1) as u64) as i32;
    let mut bytes = [0u8; 10];
    bytes[..8].copy_from_slice(&mant.to_le_bytes());
    bytes[8..].copy_from_slice(&(exp as u16 | ((rng.next() % 2) as u16) << 15).to_le_bytes());
    Value::from_tbyte(bytes)
}

fn check_exact(name: &str, ok: usize, total: usize) {
    println!("{name}: {ok} of {total} equal to the hardware");
}

#[test]
fn rounding_scaling_and_remainder_match_the_fpu() {
    let mut rng = Rng(0x7a11_0c47);
    for c in controls() {
        for _ in 0..3000 {
            let a = small_value(&mut rng, -6, 70);
            let b = small_value(&mut rng, -6, 8);
            // frndint
            let (hw, _, _) = run_hw!(Value::ZERO, a, c, "frndint");
            assert_eq!(a.rndint(c.rounding).to_tbyte(), hw, "frndint {:02x?} {:#06x}", a.to_tbyte(), c.word());
            // fscale: ST(0) * 2^trunc(ST(1)); st1 = integer-valued small
            let n = Value::from_i32((rng.next() % 200) as i32 - 100);
            let (hw, _, _) = run_hw!(n, a, c, "fscale");
            assert_eq!(a.scale(n).to_tbyte(), hw, "fscale {:02x?} {n:?}", a.to_tbyte());
            // fprem / fprem1: ST(0) = a, ST(1) = b
            for ieee in [false, true] {
                let (r, q, complete) = a.prem(b, ieee);
                let (hw, _, sw) = if ieee { run_hw!(b, a, c, "fprem1") } else { run_hw!(b, a, c, "fprem") };
                let c2 = sw & 0x400 != 0;
                assert_eq!(c2, !complete, "fprem C2 {:02x?} {:02x?} ieee {ieee}", a.to_tbyte(), b.to_tbyte());
                if complete {
                    assert_eq!(r.to_tbyte(), hw, "fprem {:02x?} {:02x?} ieee {ieee}", a.to_tbyte(), b.to_tbyte());
                    let hw_q = (sw >> 6 & 4) | (sw >> 9 & 1) | (sw >> 13 & 2);
                    // C0 = Q2, C3 = Q1, C1 = Q0
                    let q_hw = ((sw >> 8) & 1) << 2 | ((sw >> 14) & 1) << 1 | ((sw >> 9) & 1);
                    let _ = hw_q;
                    assert_eq!(q as u16, q_hw, "fprem quotient bits {:02x?} {:02x?} ieee {ieee}", a.to_tbyte(), b.to_tbyte());
                }
            }
        }
    }
}

#[test]
fn transcendentals_agree_with_the_fpu() {
    let mut rng = Rng(0x51de_ca11);
    let c = Control::DEFAULT;
    let ext = Control { rounding: Rounding::Nearest, precision: Precision::P64 };
    let _ = c;
    let (mut n_log, mut ok_log) = (0, 0);
    let (mut n_exp, mut ok_exp) = (0, 0);
    let (mut n_sin, mut ok_sin) = (0, 0);
    let (mut n_cos, mut ok_cos) = (0, 0);
    let (mut n_tan, mut ok_tan) = (0, 0);
    let (mut n_atan, mut ok_atan) = (0, 0);
    for _ in 0..4000 {
        // fyl2x: ST(0)=x>0, ST(1)=y
        let mut x = small_value(&mut rng, -20, 20);
        x = x.abs();
        let y = small_value(&mut rng, -4, 4);
        let (hw, _, _) = run_hw!(y, x, ext, "fyl2x");
        n_log += 1;
        ok_log += (Value::fyl2x(x, y, ext).to_tbyte() == hw) as usize;
        // f2xm1: |x| <= 1
        let t = small_value(&mut rng, -30, -1);
        let (hw, _, _) = run_hw!(Value::ZERO, t, ext, "f2xm1");
        n_exp += 1;
        ok_exp += (t.f2xm1(ext).to_tbyte() == hw) as usize;
        // sin, cos, tan
        let s = small_value(&mut rng, -10, 4);
        let (hw, _, _) = run_hw!(Value::ZERO, s, ext, "fsin");
        n_sin += 1;
        ok_sin += (s.sincos(0, ext).unwrap().to_tbyte() == hw) as usize;
        let (hw, _, _) = run_hw!(Value::ZERO, s, ext, "fcos");
        n_cos += 1;
        ok_cos += (s.sincos(1, ext).unwrap().to_tbyte() == hw) as usize;
        let (_, hw1, _) = run_hw!(Value::ZERO, s, ext, "fptan");
        let _ = hw1;
        let (hw0, _, _) = run_hw!(Value::ZERO, s, ext, "fptan", "fstp st(0)");
        n_tan += 1;
        ok_tan += (s.sincos(2, ext).unwrap().to_tbyte() == hw0) as usize;
        // fpatan: ST(1)=y, ST(0)=x
        let ay = small_value(&mut rng, -8, 8);
        let ax = small_value(&mut rng, -8, 8);
        let (hw, _, _) = run_hw!(ay, ax, ext, "fpatan");
        n_atan += 1;
        ok_atan += (Value::fpatan(ay, ax, ext).to_tbyte() == hw) as usize;
    }
    check_exact("fyl2x", ok_log, n_log);
    check_exact("f2xm1", ok_exp, n_exp);
    check_exact("fsin", ok_sin, n_sin);
    check_exact("fcos", ok_cos, n_cos);
    check_exact("fptan", ok_tan, n_tan);
    check_exact("fpatan", ok_atan, n_atan);
    // The correctly rounded value and the hardware agree on all but rare last-bit cases.
    for (ok, n) in [(ok_log, n_log), (ok_exp, n_exp), (ok_sin, n_sin), (ok_cos, n_cos), (ok_tan, n_tan), (ok_atan, n_atan)] {
        assert!(ok * 100 >= n * 70, "fewer than 70% of results equal the hardware");
    }
}
