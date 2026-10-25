//! What the x87 does under Wine for the original's `_controlfp(0x300, 0x300)`
//! in `SetGfxLayerAccess` (verified 2026-10-18): it selects round-toward-zero
//! (0x300 is `_MCW_RC`/`_RC_CHOP`, not a precision field), and the matching
//! "restore" call passes the new value back, so the FPU stays in that mode.
#![cfg(windows)]
use core::arch::asm;
use difftest::Original;

unsafe extern "C" {
    fn _controlfp(new: u32, mask: u32) -> u32;
}

fn control_word() -> u16 {
    let mut cw = 0u16;
    unsafe { asm!("fnstcw [{}]", in(reg) &mut cw, options(nostack)) };
    cw
}

fn fistp(x: f64) -> i32 {
    let mut out = 0i32;
    unsafe { asm!("fld qword ptr [{x}]", "fistp dword ptr [{o}]", x = in(reg) &x, o = in(reg) &mut out, options(nostack)) };
    out
}

#[test]
fn controlfp_0x300_is_round_toward_zero_and_sticks() {
    assert_eq!(control_word(), 0x027f, "a new thread starts in round-to-nearest, 53-bit precision");
    let new = unsafe { _controlfp(0x300, 0x300) };
    assert_eq!(control_word(), 0x0e7f);
    assert_eq!((fistp(3.83), fistp(2.5), fistp(-1.5)), (3, 2, -1));
    // _controlfp returns the NEW word, and passing it back keeps chop mode.
    unsafe { _controlfp(new, 0x300) };
    assert_eq!(control_word(), 0x0e7f);
}

#[test]
fn set_gfx_layer_access_leaves_the_fpu_in_chop_mode() {
    let o = Original::load("GfxEngine.dll");
    let new: extern "thiscall" fn(*mut [i32; 64]) -> *mut [i32; 64] = o.get("??0IGfxEngine@@QAE@XZ");
    let set_layer: extern "system" fn(*mut [i32; 64], i32, *const u8) = o.get("?SetGfxLayerAccess@IGfxEngine@@QAGXHPAUT_GFX_MAP_ELEMENT@@@Z");
    let engine = Box::into_raw(Box::new([0i32; 64]));
    new(engine);
    assert_eq!(control_word(), 0x027f);
    let data = vec![1u8; 16 * 16 * 4];
    set_layer(engine, 16, data.as_ptr());
    assert_eq!(control_word(), 0x0e7f);
}
