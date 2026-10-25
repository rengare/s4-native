//! The remaining system DLLs: advapi32 (registry), winmm, version, oleaut32,
//! shell32, gdi32 additions, dsound, ws2_32 (Winsock on the host's sockets),
//! plus the third-party Miles (mss32) and Bink (binkw32) stubs.

use crate::kernel32::{cstr, set_last_error, wstr};
use crate::{heap, Shim};
use s4_x86::Cpu;

macro_rules! sh {
    ($dll:expr, $name:expr, $args:expr, $f:expr) => {
        Shim { dll: $dll, name: $name, args: $args, cdecl: false, f: $f }
    };
}

/// Names for imports by ordinal.
pub fn ordinal_name(dll: &str, name: &str) -> String {
    let Some(n) = name.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()) else { return name.to_string() };
    let table: &[(u32, &str)] = match dll {
        "ws2_32.dll" => &[
            (1, "accept"), (2, "bind"), (3, "closesocket"), (4, "connect"), (5, "getpeername"), (6, "getsockname"), (7, "getsockopt"), (8, "htonl"), (9, "htons"), (10, "ioctlsocket"), (11, "inet_addr"), (12, "inet_ntoa"),
            (13, "listen"), (14, "ntohl"), (15, "ntohs"), (16, "recv"), (17, "recvfrom"), (18, "select"), (19, "send"), (20, "sendto"), (21, "setsockopt"), (22, "shutdown"), (23, "socket"), (51, "gethostbyaddr"),
            (52, "gethostbyname"), (57, "gethostname"), (111, "WSAGetLastError"), (115, "WSAStartup"), (116, "WSACleanup"),
        ],
        "oleaut32.dll" => &[(2, "SysAllocString"), (6, "SysFreeString"), (7, "SysStringLen"), (8, "VariantInit"), (9, "VariantClear"), (12, "VariantChangeType"), (200, "SetErrorInfo"), (201, "GetErrorInfo"), (202, "CreateErrorInfo")],
        "dsound.dll" => &[(1, "DirectSoundCreate"), (2, "DirectSoundEnumerateA")],
        _ => &[],
    };
    table.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string()).unwrap_or_else(|| name.to_string())
}

pub fn all() -> Vec<Shim> {
    let mut v = vec![
        // advapi32: no registry values (the game falls back to defaults)
        sh!("advapi32.dll", "RegOpenKeyExA", 5, |_, _| 2),
        sh!("advapi32.dll", "RegCreateKeyExA", 9, |_, _| 5),
        sh!("advapi32.dll", "RegQueryValueExA", 6, |_, _| 2),
        sh!("advapi32.dll", "RegCloseKey", 1, |_, _| 0),
        sh!("advapi32.dll", "GetUserNameA", 2, |c, a| {
            c.mem.write_bytes(a[0], b"player\0");
            c.mem.w32(a[1], 7);
            1
        }),
        // winmm
        sh!("winmm.dll", "timeGetTime", 0, |_, _| crate::shims::tick()),
        sh!("winmm.dll", "timeBeginPeriod", 1, |_, _| 0),
        sh!("winmm.dll", "timeEndPeriod", 1, |_, _| 0),
        sh!("winmm.dll", "mixerGetNumDevs", 0, |_, _| 0),
        sh!("winmm.dll", "mixerOpen", 5, |_, _| 1),
        sh!("winmm.dll", "mixerClose", 1, |_, _| 0),
        sh!("winmm.dll", "mixerGetDevCapsA", 3, |_, _| 1),
        sh!("winmm.dll", "mixerGetLineInfoA", 3, |_, _| 1),
        sh!("winmm.dll", "mixerGetLineControlsA", 3, |_, _| 1),
        sh!("winmm.dll", "mixerGetControlDetailsA", 3, |_, _| 1),
        sh!("winmm.dll", "mixerSetControlDetails", 3, |_, _| 1),
        // version
        sh!("version.dll", "GetFileVersionInfoSizeA", 2, |_, _| 0),
        sh!("version.dll", "GetFileVersionInfoA", 4, |_, _| 0),
        sh!("version.dll", "VerQueryValueA", 4, |_, _| 0),
        // shell32
        sh!("shell32.dll", "ShellExecuteA", 6, |_, _| 2),
        // gdi32
        sh!("gdi32.dll", "GetDeviceCaps", 2, |_, a| match a[1] {
            12 => 16,  // BITSPIXEL
            14 => 1,   // PLANES
            8 => 1920, // HORZRES
            10 => 1080,
            88 | 90 => 96,
            _ => 0,
        }),
        sh!("gdi32.dll", "CreateCompatibleBitmap", 3, |_, _| crate::gdi::new_handle(crate::gdi::Object::Other)),
        sh!("gdi32.dll", "GetDIBits", 7, |_, _| 0),
        sh!("gdi32.dll", "GetTextExtentPoint32A", 4, |c, a| {
            let s: String = c.mem.read_bytes(a[1], a[2] as usize).iter().map(|&b| b as char).collect();
            let (w, h) = crate::text::extent(a[0], &s);
            c.mem.w32(a[3], w as u32);
            c.mem.w32(a[3] + 4, h as u32);
            1
        }),
        sh!("gdi32.dll", "SetBkMode", 2, |_, a| {
            let mut old = 2;
            crate::gdi::update_state(a[0], |s| old = std::mem::replace(&mut s.bk_mode, a[1]));
            old
        }),
        sh!("gdi32.dll", "SetTextAlign", 2, |_, a| {
            let mut old = 0;
            crate::gdi::update_state(a[0], |s| old = std::mem::replace(&mut s.align, a[1]));
            old
        }),
        sh!("gdi32.dll", "SetTextColor", 2, |_, a| {
            let mut old = 0;
            crate::gdi::update_state(a[0], |s| old = std::mem::replace(&mut s.text_color, a[1]));
            old
        }),
        sh!("gdi32.dll", "CreateFontA", 14, |c, a| {
            // (height, width, escapement, orientation, weight, italic, underline, strikeout, charset, outprec, clipprec, quality, pitch, face)
            let face = crate::kernel32::cstr(c, a[13]);
            crate::gdi::new_handle(crate::gdi::Object::Font { height: a[0] as i32, weight: a[4] as i32, italic: a[5] & 0xff != 0, face })
        }),
        sh!("gdi32.dll", "TextOutA", 5, |c, a| {
            let s: String = c.mem.read_bytes(a[3], a[4] as usize).iter().map(|&b| b as char).collect();
            crate::text::text_out(c, a[0], a[1] as i32, a[2] as i32, &s);
            1
        }),
        sh!("user32.dll", "DrawTextA", 5, |c, a| {
            let bytes = if a[2] as i32 == -1 { c.mem.cstr(a[1]) } else { c.mem.read_bytes(a[1], a[2] as usize) };
            let s: String = bytes.iter().map(|&b| b as char).collect();
            let mut r = [c.mem.r32(a[3]) as i32, c.mem.r32(a[3] + 4) as i32, c.mem.r32(a[3] + 8) as i32, c.mem.r32(a[3] + 12) as i32];
            if std::env::var_os("S4_FONT_TRACE").is_some() {
                eprintln!("DrawTextA dc={:#x} {:?} rect={:?} fmt={:#x}", a[0], s, r, a[4]);
            }
            let h = crate::text::draw_text(c, a[0], &s, &mut r, a[4]);
            for (i, v) in r.iter().enumerate() {
                c.mem.w32(a[3] + 4 * i as u32, *v as u32);
            }
            h as u32
        }),
        sh!("gdi32.dll", "LineDDA", 6, |_, _| 1),
        sh!("gdi32.dll", "BitBlt", 9, crate::gdi::bit_blt),
        // oleaut32
        sh!("oleaut32.dll", "SysAllocString", 1, |c, a| sys_alloc(c, a[0], None)),
        sh!("oleaut32.dll", "SysFreeString", 1, |c, a| {
            if a[0] != 0 {
                heap::free(c, a[0] - 4);
            }
            0
        }),
        sh!("oleaut32.dll", "SysStringLen", 1, |c, a| if a[0] == 0 { 0 } else { c.mem.r32(a[0] - 4) / 2 }),
        sh!("oleaut32.dll", "VariantInit", 1, |c, a| {
            c.mem.fill(a[0], 0, 16);
            0
        }),
        sh!("oleaut32.dll", "VariantClear", 1, |c, a| {
            if c.mem.r16(a[0]) == 8 {
                let b = c.mem.r32(a[0] + 8);
                if b != 0 {
                    heap::free(c, b - 4);
                }
            }
            c.mem.fill(a[0], 0, 16);
            0
        }),
        sh!("oleaut32.dll", "VariantChangeType", 4, |_, _| 0x8000_4001),
        sh!("oleaut32.dll", "SetErrorInfo", 2, |_, _| 0),
        sh!("oleaut32.dll", "GetErrorInfo", 2, |_, _| 1),
        sh!("oleaut32.dll", "CreateErrorInfo", 1, |_, _| 0x8000_4001),
        // dsound: no device (the game's own SoundEngine uses Miles, replaced below)
        sh!("dsound.dll", "DirectSoundCreate", 3, crate::dsound::direct_sound_create),
        sh!("dsound.dll", "DirectSoundEnumerateA", 2, |_, _| 0),
    ];
    v.extend(crate::winsock::all());
    v.extend(crate::audio::all());
    v.extend(crate::miles::all());
    v
}

fn sys_alloc(c: &mut Cpu, wide: u32, len: Option<u32>) -> u32 {
    if wide == 0 {
        return 0;
    }
    let n = len.unwrap_or_else(|| {
        let mut k = 0;
        while c.mem.r16(wide + 2 * k) != 0 {
            k += 1;
        }
        k
    });
    let p = heap::alloc(c, 8 + 2 * n);
    c.mem.w32(p, 2 * n);
    for i in 0..n {
        c.mem.w16(p + 4 + 2 * i, c.mem.r16(wide + 2 * i));
    }
    c.mem.w16(p + 4 + 2 * n, 0);
    p + 4
}

#[allow(dead_code)]
fn unused(c: &Cpu) {
    let _ = (cstr(c, 0), wstr(c, 0));
    set_last_error(0);
}
