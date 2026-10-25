//! The lifted GfxEngine against the original, under Wine: the same calls,
//! results and memory must agree. `tools/test.sh -p lifted-gfxengine`.
#![cfg(all(windows, target_arch = "x86"))]

use difftest::Original;
use s4_x86::native::Machine;
use std::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn load() -> Original {
    let o = Original::load("GfxEngine.dll");
    assert_eq!(o.base, o.image_base, "the DLL must load at its preferred base for the lifted code's absolute addresses");
    o
}

#[test]
fn triangle_selector() {
    let _guard = serial();
    let o = load();
    let selector: extern "C" fn(*mut i32, *mut i32, *mut i32, *mut u8, u32, *mut u32) = o.function(0x18c2_be80);
    let mut machine = Machine::new(&[lifted_gfxengine::dispatch]);
    let alphabet: [i32; 21] = [0, 1, 2, 0x0f, 0x10, 0x11, 0x2f, 0x30, 0x31, 0x5f, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x80, 0xff, 7, 20, 99 + 256];
    let mut rng = Rng(0x600d_f00d);
    for n in 0..20000 {
        let pick = |rng: &mut Rng| if rng.next() % 3 == 0 { (rng.next() % 256) as i32 } else { alphabet[rng.next() as usize % alphabet.len()] };
        let (a, b, c) = (pick(&mut rng), pick(&mut rng), pick(&mut rng));
        let flag = [0u32, 1, 2, 255][n % 4];
        let (mut oa, mut ob, mut oc, mut special, mut ptr) = (a, b, c, 0x5au8, 0xdead_beefu32);
        selector(&mut oa, &mut ob, &mut oc, &mut special, flag, &mut ptr);
        let (mut la, mut lb, mut lc, mut lspecial, mut lptr) = (a, b, c, 0x5au8, 0xdead_beefu32);
        machine.call_cdecl(
            lifted_gfxengine::f_18c2be80,
            &[&mut la as *mut i32 as u32, &mut lb as *mut i32 as u32, &mut lc as *mut i32 as u32, &mut lspecial as *mut u8 as u32, flag, &mut lptr as *mut u32 as u32],
        );
        assert_eq!((oa, ob, oc, special, ptr), (la, lb, lc, lspecial, lptr), "({a:#x}, {b:#x}, {c:#x}) flag {flag}");
    }
}
