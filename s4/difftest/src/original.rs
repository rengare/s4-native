use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::Mutex;

unsafe extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn SetDllDirectoryA(path: *const u8) -> i32;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> usize;
}

/// Writable sections per loaded module as they were right after loading.
static PRISTINE: Mutex<Option<HashMap<usize, Vec<(usize, Vec<u8>)>>>> = Mutex::new(None);

/// A loaded original module.
pub struct Original {
    pub base: usize,
    /// Image base the addresses in Ghidra's export use.
    pub image_base: usize,
    /// (RVA, size) of the writable sections (.data, .bss and the like).
    pub writable: Vec<(usize, usize)>,
}

fn u16_at(p: usize) -> u16 {
    unsafe { (p as *const u16).read_unaligned() }
}

fn u32_at(p: usize) -> u32 {
    unsafe { (p as *const u32).read_unaligned() }
}

impl Original {
    /// Loads `dll` from the extracted game (`S4_EXE_DIR` overrides the
    /// directory) and resets its writable sections to their state at load,
    /// so every test starts from the same globals.
    pub fn load(dll: &str) -> Original {
        let dir = std::env::var("S4_EXE_DIR").unwrap_or_else(|_| "Z:\\home\\ren\\workspace\\decomp\\extracted\\s4\\Exe".into());
        let cdir = std::ffi::CString::new(dir.clone()).unwrap();
        let path = std::ffi::CString::new(format!("{dir}\\{dll}")).unwrap();
        let m = unsafe {
            SetDllDirectoryA(cdir.as_ptr().cast());
            LoadLibraryA(path.as_ptr().cast())
        };
        assert!(!m.is_null(), "cannot load {dir}\\{dll}");
        let base = m as usize;
        let pe = base + u32_at(base + 0x3c) as usize;
        let sections = u16_at(pe + 6) as usize;
        let optional = u16_at(pe + 20) as usize;
        let image_base = u32_at(pe + 24 + 28) as usize;
        let mut writable = Vec::new();
        for i in 0..sections {
            let s = pe + 24 + optional + 40 * i;
            let (vsize, rva, flags) = (u32_at(s + 8) as usize, u32_at(s + 12) as usize, u32_at(s + 36));
            if flags & 0x8000_0000 != 0 && flags & 0x2000_0000 == 0 {
                writable.push((rva, vsize));
            }
        }
        let o = Original { base, image_base, writable };
        let mut pristine = PRISTINE.lock().unwrap();
        let saved = pristine.get_or_insert_with(HashMap::new).entry(base).or_insert_with(|| {
            o.writable.iter().map(|&(rva, size)| (rva, unsafe { core::slice::from_raw_parts((base + rva) as *const u8, size) }.to_vec())).collect()
        });
        for (rva, bytes) in saved.iter() {
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), (base + rva) as *mut u8, bytes.len()) };
        }
        o
    }

    /// An export as function type `F`.
    pub fn get<F: Copy>(&self, name: &str) -> F {
        let c = std::ffi::CString::new(name).unwrap();
        let p = unsafe { GetProcAddress(self.base as *mut c_void, c.as_ptr().cast()) };
        assert_ne!(p, 0, "{name} missing");
        unsafe { core::mem::transmute_copy(&p) }
    }

    /// A function at its Ghidra address as function type `F`.
    pub fn function<F: Copy>(&self, addr: usize) -> F {
        let p = self.runtime(addr);
        unsafe { core::mem::transmute_copy(&p) }
    }

    /// Runtime address of a Ghidra address.
    pub fn runtime(&self, addr: usize) -> usize {
        self.base + addr - self.image_base
    }

    /// Ghidra address of a runtime address.
    pub fn ghidra(&self, p: usize) -> usize {
        p - self.base + self.image_base
    }

    pub fn at<T>(&self, addr: usize) -> *mut T {
        self.runtime(addr) as *mut T
    }

    pub fn read<T: Copy>(&self, addr: usize) -> T {
        unsafe { self.at::<T>(addr).read_unaligned() }
    }

    pub fn write<T>(&self, addr: usize, v: T) {
        unsafe { self.at::<T>(addr).write_unaligned(v) }
    }

    /// The writable sections now.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            image_base: self.image_base,
            sections: self
                .writable
                .iter()
                .map(|&(rva, size)| (rva, unsafe { core::slice::from_raw_parts((self.base + rva) as *const u8, size) }.to_vec()))
                .collect(),
        }
    }
}

/// Contents of a module's writable sections at one moment.
pub struct Snapshot {
    image_base: usize,
    sections: Vec<(usize, Vec<u8>)>,
}

impl Snapshot {
    /// Ghidra addresses of the bytes that differ from `later`.
    pub fn changes(&self, later: &Snapshot) -> Vec<usize> {
        let mut out = Vec::new();
        for ((rva, a), (_, b)) in self.sections.iter().zip(&later.sections) {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                if x != y {
                    out.push(self.image_base + rva + i);
                }
            }
        }
        out
    }
}
