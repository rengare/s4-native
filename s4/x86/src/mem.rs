//! Memory as the original sees it: a flat 32-bit address space at `base`
//! in this process (0 when the original runs inside this process, i.e. under
//! Wine; the start of a 4 GiB reservation natively).

#[derive(Clone, Copy, Debug)]
pub struct Mem {
    pub base: usize,
}

impl Mem {
    /// Addresses are the process's own (the original loaded in this process).
    pub const IDENTITY: Mem = Mem { base: 0 };

    #[inline(always)]
    fn p(self, a: u32) -> usize {
        self.base.wrapping_add(a as usize)
    }

    #[inline(always)]
    pub fn r8(self, a: u32) -> u8 {
        unsafe { (self.p(a) as *const u8).read() }
    }

    #[inline(always)]
    pub fn r16(self, a: u32) -> u16 {
        unsafe { (self.p(a) as *const u16).read_unaligned() }
    }

    #[inline(always)]
    pub fn r32(self, a: u32) -> u32 {
        unsafe { (self.p(a) as *const u32).read_unaligned() }
    }

    #[inline(always)]
    pub fn r64(self, a: u32) -> u64 {
        unsafe { (self.p(a) as *const u64).read_unaligned() }
    }

    #[inline(always)]
    pub fn r80(self, a: u32) -> [u8; 10] {
        unsafe { (self.p(a) as *const [u8; 10]).read_unaligned() }
    }

    #[inline(always)]
    pub fn r128(self, a: u32) -> u128 {
        unsafe { (self.p(a) as *const u128).read_unaligned() }
    }

    #[inline(always)]
    pub fn w8(self, a: u32, v: u8) {
        unsafe { (self.p(a) as *mut u8).write(v) }
    }

    #[inline(always)]
    pub fn w16(self, a: u32, v: u16) {
        unsafe { (self.p(a) as *mut u16).write_unaligned(v) }
    }

    #[inline(always)]
    pub fn w32(self, a: u32, v: u32) {
        unsafe { (self.p(a) as *mut u32).write_unaligned(v) }
    }

    #[inline(always)]
    pub fn w64(self, a: u32, v: u64) {
        unsafe { (self.p(a) as *mut u64).write_unaligned(v) }
    }

    #[inline(always)]
    pub fn w80(self, a: u32, v: [u8; 10]) {
        unsafe { (self.p(a) as *mut [u8; 10]).write_unaligned(v) }
    }

    #[inline(always)]
    pub fn w128(self, a: u32, v: u128) {
        unsafe { (self.p(a) as *mut u128).write_unaligned(v) }
    }

    /// Reads of 1, 2 or 4 bytes as a zero-extended value (`bits` is a
    /// constant at the call sites).
    #[inline(always)]
    pub fn r(self, bits: u32, a: u32) -> u32 {
        match bits {
            8 => self.r8(a) as u32,
            16 => self.r16(a) as u32,
            _ => self.r32(a),
        }
    }

    #[inline(always)]
    pub fn w(self, bits: u32, a: u32, v: u32) {
        match bits {
            8 => self.w8(a, v as u8),
            16 => self.w16(a, v as u16),
            _ => self.w32(a, v),
        }
    }
}

impl Mem {
    pub fn write_bytes(self, a: u32, bytes: &[u8]) {
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.p(a) as *mut u8, bytes.len()) }
    }

    /// Copies `len` bytes inside guest memory (the ranges may overlap, as with `memmove`).
    pub fn copy(self, dst: u32, src: u32, len: usize) {
        unsafe { core::ptr::copy(self.p(src) as *const u8, self.p(dst) as *mut u8, len) }
    }

    /// A view of guest memory. Valid only while the guest does not change it: use it on the guest's own thread.
    pub fn view(self, a: u32, len: usize) -> &'static [u8] {
        unsafe { core::slice::from_raw_parts(self.p(a) as *const u8, len) }
    }

    pub fn read_bytes(self, a: u32, len: usize) -> Vec<u8> {
        unsafe { core::slice::from_raw_parts(self.p(a) as *const u8, len) }.to_vec()
    }

    pub fn fill(self, a: u32, v: u8, len: usize) {
        unsafe { core::ptr::write_bytes(self.p(a) as *mut u8, v, len) }
    }

    /// A NUL-terminated string.
    pub fn cstr(self, a: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut p = a;
        loop {
            let c = self.r8(p);
            if c == 0 {
                return out;
            }
            out.push(c);
            p = p.wrapping_add(1);
        }
    }
}
