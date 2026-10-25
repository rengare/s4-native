//! Loads the original GfxEngine.dll (its static constructors run) and prints
//! the tables they build, as Rust source.
//!
//! Usage (Wine): gfxdump <dir containing GfxEngine.dll and its dependencies>
//! - minimap colour per terrain type: 256 x RGB at RVA 0x667c8.
//! gfxdump <dir> <rva hex> <length> <out file>: writes that memory raw.

use core::ffi::c_void;

unsafe extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn SetDllDirectoryA(path: *const u8) -> i32;
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: gfxdump <Exe dir>");
    let cdir = std::ffi::CString::new(dir.clone()).unwrap();
    let path = std::ffi::CString::new(format!("{dir}\\GfxEngine.dll")).unwrap();
    let base = unsafe {
        SetDllDirectoryA(cdir.as_ptr().cast());
        LoadLibraryA(path.as_ptr().cast())
    } as usize;
    assert_ne!(base, 0, "cannot load {dir}\\GfxEngine.dll");
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 5 {
        let rva = usize::from_str_radix(args[2].trim_start_matches("0x"), 16).expect("rva");
        let len: usize = args[3].parse().expect("length");
        let bytes = unsafe { core::slice::from_raw_parts((base + rva) as *const u8, len) };
        std::fs::write(&args[4], bytes).expect("write");
        return;
    }
    let table = unsafe { core::slice::from_raw_parts((base + 0x667c8) as *const u8, 768) };
    println!("/// Minimap colour (RGB) per terrain type, built by GfxEngine's static");
    println!("/// constructor `init_minimap_color_table` (dumped from the original DLL by tools/gfxdump).");
    println!("pub const TERRAIN_COLORS: [[u8; 3]; 256] = [");
    for row in table.chunks(3 * 8) {
        let items: Vec<String> = row.chunks(3).map(|c| format!("[{}, {}, {}]", c[0], c[1], c[2])).collect();
        println!("    {},", items.join(", "));
    }
    println!("];");
}
