//! Export functions from 32-bit Windows DLLs under the original MSVC-mangled names
//! (`?Init@IVideoEngine@@QAG_NI@Z`), which Rust's `export_name` cannot produce on
//! i686-pc-windows-gnu. Each export is a global-asm stub that jumps to the Rust
//! function; `.drectve` puts it in the export table like MinGW's `dllexport`.
//!
//! Calling conventions of MSVC members: `QAE` = `__thiscall` (`this` in ECX) →
//! `extern "thiscall"`; `QAG`/`YG` = `__stdcall` (`this` on the stack) →
//! `extern "system"`. A `jmp` keeps the caller's stack untouched, so the stub
//! works for any convention.
#![no_std]

/// Exports an asm stub under an original (MSVC-mangled) name. Global asm because
/// those names are only valid quoted; exported via `.drectve` like MinGW's
/// `dllexport` (ld resolves an i386 export `X` to the symbol `_X`).
#[macro_export]
macro_rules! export_stub {
    ($name:literal, $body:literal, $($op:tt)*) => {
        core::arch::global_asm!(
            concat!(
                ".text\n.globl \"_", $name, "\"\n.p2align 4\n\"_", $name, "\":\n  ", $body, "\n",
                ".section .drectve,\"yn\"\n.ascii \" -export:\\\"", $name, "\\\"\"\n.text\n",
            ),
            $($op)*
        );
    };
}

/// Export `name` as a jump to our Rust implementation `f`.
#[macro_export]
macro_rules! implement {
    ($name:literal, $f:path) => {
        $crate::export_stub!($name, "jmp {f}", f = sym $f);
    };
}


/// Export the static `sym` as data under an original name (vftables: the game
/// reads `??_7Class@@6B@` from its import table and stores it in objects).
#[macro_export]
macro_rules! export_data {
    ($name:literal, $sym:path) => {
        core::arch::global_asm!(
            concat!(
                ".globl \"_", $name, "\"\n.set \"_", $name, "\", {s}\n",
                ".section .drectve,\"yn\"\n.ascii \" -export:\\\"", $name, "\\\",DATA\"\n.text\n",
            ),
            s = sym $sym
        );
    };
}
