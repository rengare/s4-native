//! Mirrors the interface discovery of Settlers 4 `SimpleNet.cpp` (S4_Main `discover_network_interfaces`):
//! every IPv4 address of `gethostbyname(gethostname())` except 127.0.0.1 becomes a
//! receiver socket; the sender binds to the last one. Run under the game's Wine.

use core::ffi::{c_char, c_void};

#[repr(C)]
struct HostEnt {
    name: *const c_char,
    aliases: *const *const c_char,
    addrtype: i16,
    length: i16,
    addr_list: *const *const [u8; 4],
}

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut c_void) -> i32;
    fn gethostname(name: *mut c_char, len: i32) -> i32;
    fn gethostbyname(name: *const c_char) -> *const HostEnt;
}

fn main() {
    let mut wsadata = [0u8; 512];
    let mut name = [0 as c_char; 256];
    unsafe {
        assert_eq!(WSAStartup(0x0202, wsadata.as_mut_ptr().cast()), 0);
        assert_eq!(gethostname(name.as_mut_ptr(), 256), 0);
        let host = core::ffi::CStr::from_ptr(name.as_ptr());
        println!("gethostname: {}", host.to_string_lossy());
        let he = gethostbyname(name.as_ptr());
        if he.is_null() {
            println!("gethostbyname failed");
            return;
        }
        let mut p = (*he).addr_list;
        while !(*p).is_null() {
            let [a, b, c, d] = **p;
            let skipped = [a, b, c, d] == [127, 0, 0, 1];
            println!("address {a}.{b}.{c}.{d}{}", if skipped { " (skipped by the game)" } else { "" });
            p = p.add(1);
        }
    }
}
