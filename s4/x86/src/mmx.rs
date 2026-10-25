//! MMX instructions on 64-bit values. The registers are the low halves of the
//! x87 registers (`Fpu::reg[n]`); any MMX instruction other than `emms`
//! resets TOP and the tags, and writing a register sets its exponent field
//! to all ones.

use crate::fpu::Fpu;

impl Fpu {
    /// The effect every MMX instruction has on the x87 state.
    pub fn mmx_enter(&mut self) {
        self.top = 0;
        self.empty = 0;
    }

    pub fn mm_read(&mut self, n: usize) -> u64 {
        self.mmx_enter();
        u64::from_le_bytes(self.reg_bytes(n)[..8].try_into().unwrap())
    }

    pub fn mm_write(&mut self, n: usize, v: u64) {
        self.mmx_enter();
        let mut b = [0xffu8; 10];
        b[..8].copy_from_slice(&v.to_le_bytes());
        self.set_reg_bytes(n, b);
    }

    /// `emms`: all registers empty.
    pub fn emms(&mut self) {
        self.empty = 0xff;
        self.top = 0;
    }
}

fn b(a: u64) -> [u8; 8] {
    a.to_le_bytes()
}

fn w(a: u64) -> [u16; 4] {
    [a as u16, (a >> 16) as u16, (a >> 32) as u16, (a >> 48) as u16]
}

fn d(a: u64) -> [u32; 2] {
    [a as u32, (a >> 32) as u32]
}

fn from_b(v: [u8; 8]) -> u64 {
    u64::from_le_bytes(v)
}

fn from_w(v: [u16; 4]) -> u64 {
    v[0] as u64 | (v[1] as u64) << 16 | (v[2] as u64) << 32 | (v[3] as u64) << 48
}

fn from_d(v: [u32; 2]) -> u64 {
    v[0] as u64 | (v[1] as u64) << 32
}

macro_rules! lanewise {
    ($name:ident, $to:ident, $from:ident, $n:expr, |$x:ident, $y:ident| $e:expr) => {
        pub fn $name(a: u64, bb: u64) -> u64 {
            let (xa, xb) = ($to(a), $to(bb));
            let mut r = xa;
            for i in 0..$n {
                let ($x, $y) = (xa[i], xb[i]);
                r[i] = $e;
            }
            $from(r)
        }
    };
}

lanewise!(paddb, b, from_b, 8, |x, y| x.wrapping_add(y));
lanewise!(paddw, w, from_w, 4, |x, y| x.wrapping_add(y));
lanewise!(paddd, d, from_d, 2, |x, y| x.wrapping_add(y));
lanewise!(psubb, b, from_b, 8, |x, y| x.wrapping_sub(y));
lanewise!(psubw, w, from_w, 4, |x, y| x.wrapping_sub(y));
lanewise!(psubd, d, from_d, 2, |x, y| x.wrapping_sub(y));
lanewise!(paddusb, b, from_b, 8, |x, y| x.saturating_add(y));
lanewise!(paddusw, w, from_w, 4, |x, y| x.saturating_add(y));
lanewise!(psubusb, b, from_b, 8, |x, y| x.saturating_sub(y));
lanewise!(psubusw, w, from_w, 4, |x, y| x.saturating_sub(y));
lanewise!(paddsb, b, from_b, 8, |x, y| (x as i8).saturating_add(y as i8) as u8);
lanewise!(paddsw, w, from_w, 4, |x, y| (x as i16).saturating_add(y as i16) as u16);
lanewise!(psubsb, b, from_b, 8, |x, y| (x as i8).saturating_sub(y as i8) as u8);
lanewise!(psubsw, w, from_w, 4, |x, y| (x as i16).saturating_sub(y as i16) as u16);
lanewise!(pmullw, w, from_w, 4, |x, y| (x as i16 as i32).wrapping_mul(y as i16 as i32) as u16);
lanewise!(pmulhw, w, from_w, 4, |x, y| ((x as i16 as i32).wrapping_mul(y as i16 as i32) >> 16) as u16);
lanewise!(pcmpeqb, b, from_b, 8, |x, y| if x == y { 0xff } else { 0 });
lanewise!(pcmpeqw, w, from_w, 4, |x, y| if x == y { 0xffff } else { 0 });
lanewise!(pcmpeqd, d, from_d, 2, |x, y| if x == y { u32::MAX } else { 0 });
lanewise!(pcmpgtb, b, from_b, 8, |x, y| if (x as i8) > (y as i8) { 0xff } else { 0 });
lanewise!(pcmpgtw, w, from_w, 4, |x, y| if (x as i16) > (y as i16) { 0xffff } else { 0 });
lanewise!(pcmpgtd, d, from_d, 2, |x, y| if (x as i32) > (y as i32) { u32::MAX } else { 0 });

pub fn pand(a: u64, bb: u64) -> u64 {
    a & bb
}

/// `pandn`: (not a) and b.
pub fn pandn(a: u64, bb: u64) -> u64 {
    !a & bb
}

pub fn por(a: u64, bb: u64) -> u64 {
    a | bb
}

pub fn pxor(a: u64, bb: u64) -> u64 {
    a ^ bb
}

pub fn pmaddwd(a: u64, bb: u64) -> u64 {
    let (x, y) = (w(a), w(bb));
    let p = |i: usize| (x[i] as i16 as i32).wrapping_mul(y[i] as i16 as i32);
    from_d([p(0).wrapping_add(p(1)) as u32, p(2).wrapping_add(p(3)) as u32])
}

pub fn psllw(a: u64, n: u64) -> u64 {
    if n > 15 { 0 } else { from_w(w(a).map(|x| x << n)) }
}

pub fn pslld(a: u64, n: u64) -> u64 {
    if n > 31 { 0 } else { from_d(d(a).map(|x| x << n)) }
}

pub fn psllq(a: u64, n: u64) -> u64 {
    if n > 63 { 0 } else { a << n }
}

pub fn psrlw(a: u64, n: u64) -> u64 {
    if n > 15 { 0 } else { from_w(w(a).map(|x| x >> n)) }
}

pub fn psrld(a: u64, n: u64) -> u64 {
    if n > 31 { 0 } else { from_d(d(a).map(|x| x >> n)) }
}

pub fn psrlq(a: u64, n: u64) -> u64 {
    if n > 63 { 0 } else { a >> n }
}

pub fn psraw(a: u64, n: u64) -> u64 {
    let n = n.min(15) as u32;
    from_w(w(a).map(|x| ((x as i16) >> n) as u16))
}

pub fn psrad(a: u64, n: u64) -> u64 {
    let n = n.min(31) as u32;
    from_d(d(a).map(|x| ((x as i32) >> n) as u32))
}

pub fn punpcklbw(a: u64, bb: u64) -> u64 {
    let (x, y) = (b(a), b(bb));
    from_b([x[0], y[0], x[1], y[1], x[2], y[2], x[3], y[3]])
}

pub fn punpckhbw(a: u64, bb: u64) -> u64 {
    let (x, y) = (b(a), b(bb));
    from_b([x[4], y[4], x[5], y[5], x[6], y[6], x[7], y[7]])
}

pub fn punpcklwd(a: u64, bb: u64) -> u64 {
    let (x, y) = (w(a), w(bb));
    from_w([x[0], y[0], x[1], y[1]])
}

pub fn punpckhwd(a: u64, bb: u64) -> u64 {
    let (x, y) = (w(a), w(bb));
    from_w([x[2], y[2], x[3], y[3]])
}

pub fn punpckldq(a: u64, bb: u64) -> u64 {
    from_d([d(a)[0], d(bb)[0]])
}

pub fn punpckhdq(a: u64, bb: u64) -> u64 {
    from_d([d(a)[1], d(bb)[1]])
}

fn sat_u8(v: i16) -> u8 {
    v.clamp(0, 255) as u8
}

fn sat_i8(v: i16) -> u8 {
    v.clamp(-128, 127) as i8 as u8
}

pub fn packuswb(a: u64, bb: u64) -> u64 {
    let (x, y) = (w(a), w(bb));
    from_b([sat_u8(x[0] as i16), sat_u8(x[1] as i16), sat_u8(x[2] as i16), sat_u8(x[3] as i16), sat_u8(y[0] as i16), sat_u8(y[1] as i16), sat_u8(y[2] as i16), sat_u8(y[3] as i16)])
}

pub fn packsswb(a: u64, bb: u64) -> u64 {
    let (x, y) = (w(a), w(bb));
    from_b([sat_i8(x[0] as i16), sat_i8(x[1] as i16), sat_i8(x[2] as i16), sat_i8(x[3] as i16), sat_i8(y[0] as i16), sat_i8(y[1] as i16), sat_i8(y[2] as i16), sat_i8(y[3] as i16)])
}

pub fn packssdw(a: u64, bb: u64) -> u64 {
    let (x, y) = (d(a), d(bb));
    let s = |v: u32| (v as i32).clamp(-32768, 32767) as i16 as u16;
    from_w([s(x[0]), s(x[1]), s(y[0]), s(y[1])])
}
