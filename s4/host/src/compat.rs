//! The Windows functions The Settlers III needs that the rest of the host does not provide yet: semaphores, a few
//! file, GDI, window and COM calls, and the multimedia timer.

use crate::handles::{self, Obj};
use crate::kernel32::{cstr, set_last_error};
use crate::{heap, Shim};
use s4_x86::Cpu;

macro_rules! sh {
    ($dll:expr, $name:expr, $args:expr, $f:expr) => {
        Shim { dll: $dll, name: $name, args: $args, cdecl: false, f: $f }
    };
}

fn rect(c: &Cpu, p: u32) -> [i32; 4] {
    [0, 4, 8, 12].map(|o| c.mem.r32(p + o) as i32)
}

fn put_rect(c: &Cpu, p: u32, r: [i32; 4]) {
    for (i, v) in r.iter().enumerate() {
        c.mem.w32(p + 4 * i as u32, *v as u32);
    }
}

/// FILETIME (100 ns since 1601) of a host time.
fn filetime(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64 / 100 + 116_444_736_000_000_000)
}

static TIMERS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
static NEXT_TIMER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

fn ret_const<const R: u32>(_: &mut Cpu, _: &[u32]) -> u32 {
    R
}

const E_NOTIMPL: u32 = 0x8000_4001;
const DPERR_NOTLOBBIED: u32 = 0x8877_042e;

/// IDirectPlayLobby3A: stack dwords (with `this`) of each method by slot, and the slots that answer DPERR_NOTLOBBIED
/// (the game was not started by a lobby). The rest fail with E_NOTIMPL.
const LOBBY_ARGS: [u8; 19] = [3, 1, 1, 4, 7, 5, 5, 4, 4, 6, 5, 5, 4, 3, 5, 3, 3, 2, 2];
const LOBBY_NOTLOBBIED: [usize; 3] = [8, 9, 17];

/// A COM object whose methods fail (QueryInterface answers with the object itself): `args` gives the stack dwords of
/// the first methods, `notlobbied` the slots with a different answer. `key` names the vtable (made once).
fn stub_vtable(c: &mut Cpu, key: &'static str, args: &'static [u8], notlobbied: &'static [usize]) -> u32 {
    static VTABLES: std::sync::Mutex<Vec<(&'static str, u32)>> = std::sync::Mutex::new(Vec::new());
    let mut g = VTABLES.lock().unwrap();
    if let Some((_, vt)) = g.iter().find(|(k, _)| *k == key) {
        return *vt;
    }
    let vt = heap::host_alloc(c, 4 * 123);
    for i in 0..123usize {
        let (name, n, f): (&'static str, u8, fn(&mut Cpu, &[u32]) -> u32) = match i {
            0 => ("QueryInterface", 3, |c, a| {
                if std::env::var_os("S4_HOST_TRACE").is_some() {
                    eprintln!("  stub QueryInterface IID {:02X?}", c.mem.read_bytes(a[1], 16));
                }
                c.mem.w32(a[2], a[0]);
                crate::ddraw::add_ref(c, a[0]);
                0
            }),
            1 => ("AddRef", 1, |c, a| crate::ddraw::add_ref(c, a[0])),
            2 => ("Release", 1, |c, a| crate::ddraw::release(c, a[0])),
            14 if key == "IDirectPlayLobby" => ("CreateCompoundAddress", 5, crate::dplay::create_compound_address),
            _ => (
                Box::leak(format!("{key} slot {i}").into_boxed_str()),
                args.get(i).copied().unwrap_or(1),
                if notlobbied.contains(&i) { ret_const::<DPERR_NOTLOBBIED> } else { ret_const::<E_NOTIMPL> },
            ),
        };
        let addr = crate::register_shim(Shim { dll: "com", name, args: n, cdecl: false, f });
        c.mem.w32(vt + 4 * i as u32, addr);
    }
    g.push((key, vt));
    vt
}

/// `mciSendCommand(device, message, flags, parameters)`. Opening a device succeeds (device id 1); a command asked
/// to notify (MCI_NOTIFY) posts MM_MCINOTIFY (successful) to the window named in its parameters at once.
pub fn mci_command(c: &mut Cpu, device: u32, msg: u32, flags: u32, parms: u32) -> u32 {
    if std::env::var_os("S4_MCI_TRACE").is_some() {
        eprintln!("mciSendCommand(device {device}, msg {msg:#x}, flags {flags:#x}, parms {parms:#x}: {:x?})", (0..6).map(|i| c.mem.r32(parms + 4 * i)).collect::<Vec<_>>());
    }
    const MCI_OPEN: u32 = 0x803;
    const MCI_NOTIFY: u32 = 1;
    if msg == MCI_OPEN {
        // MCI_OPEN_PARMS: callback, device id (out), type, element, alias. MCI_OPEN_TYPE_ID (0x1000): the type is a number
        // (MCI_DEVTYPE_CD_AUDIO is 516), else the address of its name.
        let ty = c.mem.r32(parms + 8);
        let is_cd = if flags & 0x1000 != 0 { ty & 0xffff == 516 } else { ty != 0 && cstr(c, ty).eq_ignore_ascii_case("cdaudio") };
        if is_cd {
            c.mem.w32(parms + 4, crate::cdaudio::DEVICE);
            crate::cdaudio::CD.lock().unwrap().opened = true;
            return 0;
        }
        c.mem.w32(parms + 4, 9);
    }
    if device == crate::cdaudio::DEVICE {
        return crate::cdaudio::command(c, msg, flags, parms);
    }
    if flags & MCI_NOTIFY != 0 && parms != 0 {
        let hwnd = c.mem.r32(parms);
        if hwnd != 0 {
            crate::user32::post_message(hwnd, 0x3b9, 1, if device == 0 { 1 } else { device });
        }
    }
    0
}

pub fn all() -> Vec<Shim> {
    vec![
        // ---- stubs the Indeo codec's imports need (its configuration dialogs, registry and COM registration are not used) ----
        sh!("advapi32.dll", "RegCreateKeyA", 3, |_, _| 5),
        sh!("advapi32.dll", "RegOpenKeyA", 3, |_, _| 2),
        sh!("advapi32.dll", "RegQueryValueA", 4, |_, _| 2),
        sh!("advapi32.dll", "RegSetValueA", 5, |_, _| 5),
        sh!("advapi32.dll", "RegDeleteValueA", 2, |_, _| 2),
        sh!("gdi32.dll", "GetObjectA", 3, |_, _| 0),
        sh!("gdi32.dll", "GetSystemPaletteEntries", 4, |_, _| 0),
        // Resources by number: the "handle" is the address of the data (a resource has no other state here).
        sh!("kernel32.dll", "FindResourceA", 3, |c, a| {
            if a[1] >= 0x10000 || a[2] >= 0x10000 {
                return 0;
            }
            let base = if a[0] == 0 { crate::main_module().0 } else { a[0] };
            let Some(rva) = crate::shims::rsrc_of(base) else { return 0 };
            crate::pe::resource(c.mem, base, rva, a[2], a[1]).map_or(0, |r| r.0)
        }),
        sh!("kernel32.dll", "LoadResource", 2, |_, a| a[1]),
        sh!("kernel32.dll", "LockResource", 1, |_, a| a[0]),
        sh!("kernel32.dll", "FreeResource", 1, |_, _| 0),
        sh!("kernel32.dll", "GetShortPathNameA", 3, |c, a| {
            let s = c.mem.cstr(a[0]);
            if a[2] as usize <= s.len() {
                return s.len() as u32 + 1;
            }
            c.mem.write_bytes(a[1], &s);
            c.mem.w8(a[1] + s.len() as u32, 0);
            s.len() as u32
        }),
        sh!("kernel32.dll", "GlobalHandle", 1, |_, a| a[0]),
        sh!("kernel32.dll", "GlobalReAlloc", 3, |c, a| heap::realloc(c, a[0], a[1])),
        sh!("kernel32.dll", "LocalHandle", 1, |_, a| a[0]),
        sh!("kernel32.dll", "LocalLock", 1, |_, a| a[0]),
        sh!("kernel32.dll", "LocalUnlock", 1, |_, _| 1),
        sh!("kernel32.dll", "WritePrivateProfileStringA", 4, |_, _| 1),
        sh!("oleaut32.dll", "#161", 5, |_, _| 0x8000_4005),
        sh!("oleaut32.dll", "#163", 3, |_, _| 0x8000_4005),
        sh!("user32.dll", "CheckDlgButton", 3, |_, _| 1),
        sh!("user32.dll", "CheckRadioButton", 4, |_, _| 1),
        sh!("user32.dll", "CreateDialogParamA", 5, |_, _| 0),
        sh!("user32.dll", "GetDesktopWindow", 0, |_, _| 0x10),
        sh!("user32.dll", "GetDlgCtrlID", 1, |_, _| 0),
        sh!("user32.dll", "GetDlgItem", 2, |_, _| 0),
        sh!("user32.dll", "GetDlgItemInt", 4, |_, _| 0),
        sh!("user32.dll", "GetFocus", 0, |_, _| 0),
        sh!("user32.dll", "InvalidateRect", 3, |_, _| 1),
        sh!("user32.dll", "LoadStringA", 4, |_, _| 0),
        sh!("user32.dll", "LoadStringW", 4, |_, _| 0),
        sh!("user32.dll", "MapWindowPoints", 4, |_, _| 0),
        sh!("user32.dll", "OffsetRect", 3, |c, a| {
            let r = rect(c, a[0]);
            put_rect(c, a[0], [r[0] + a[1] as i32, r[1] + a[2] as i32, r[2] + a[1] as i32, r[3] + a[2] as i32]);
            1
        }),
        sh!("user32.dll", "SetDlgItemInt", 4, |_, _| 1),
        sh!("user32.dll", "SetFocus", 1, |_, _| 0),
        sh!("user32.dll", "SetWindowTextA", 2, |_, _| 1),
        sh!("user32.dll", "WinHelpA", 4, |_, _| 1),
        sh!("winmm.dll", "DefDriverProc", 5, |_, _| 0),
        sh!("ole32.dll", "CoFreeUnusedLibraries", 0, |_, _| 0),
        sh!("ole32.dll", "CoTaskMemAlloc", 1, |c, a| heap::alloc(c, a[0])),
        sh!("ole32.dll", "CoTaskMemFree", 1, |c, a| {
            heap::free(c, a[0]);
            0
        }),
        sh!("ole32.dll", "StringFromGUID2", 3, |c, a| {
            let g = c.mem.view(a[0], 16).to_vec();
            let d1 = u32::from_le_bytes(g[0..4].try_into().unwrap());
            let d2 = u16::from_le_bytes(g[4..6].try_into().unwrap());
            let d3 = u16::from_le_bytes(g[6..8].try_into().unwrap());
            let s = format!("{{{d1:08X}-{d2:04X}-{d3:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}", g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]);
            if (a[2] as usize) < s.len() + 1 {
                return 0;
            }
            for (i, ch) in s.encode_utf16().chain([0]).enumerate() {
                c.mem.w16(a[1] + 2 * i as u32, ch);
            }
            s.len() as u32 + 1
        }),
        // ---- ddraw: one display driver, the primary ----
        sh!("ddraw.dll", "DirectDrawEnumerateA", 2, |c, a| {
            let desc = heap::alloc(c, 32);
            c.mem.write_bytes(desc, b"display\0");
            let name = heap::alloc(c, 32);
            c.mem.write_bytes(name, b"primary\0");
            crate::shims::guest_call(c, a[0], &[0, desc, name, a[1]]);
            0
        }),
        // ---- dinput ----
        sh!("dinput.dll", "DirectInputCreateA", 4, crate::dinput::direct_input_create),
        // ---- kernel32 ----
        sh!("kernel32.dll", "CreateSemaphoreA", 4, |_, a| {
            let s = crate::kernel32::new_signal(false, false);
            *s.owner.lock().unwrap() = (a[1], a[2]);
            handles::insert(Obj::Semaphore(s))
        }),
        sh!("kernel32.dll", "ReleaseSemaphore", 3, |c, a| {
            let Some(Obj::Semaphore(s)) = handles::get(a[0]) else {
                set_last_error(crate::kernel32::ERROR_INVALID_HANDLE);
                return 0;
            };
            let _st = s.state.lock().unwrap();
            let mut o = s.owner.lock().unwrap();
            if a[2] != 0 {
                c.mem.w32(a[2], o.0);
            }
            if o.0.saturating_add(a[1]) > o.1 {
                return 0;
            }
            o.0 += a[1];
            drop(o);
            s.cond.notify_all();
            1
        }),
        sh!("kernel32.dll", "PulseEvent", 1, |_, a| {
            let Some(Obj::Event(s)) = handles::get(a[0]) else { return 0 };
            *s.state.lock().unwrap() = true;
            s.cond.notify_all();
            std::thread::yield_now();
            *s.state.lock().unwrap() = false;
            1
        }),
        // (flags, source, message id, language, buffer, size, arguments)
        sh!("kernel32.dll", "FormatMessageA", 7, |c, a| {
            let text = format!("Windows error {}.\r\n\0", a[2]);
            if a[0] & 0x100 != 0 {
                let p = heap::alloc(c, text.len() as u32);
                c.mem.write_bytes(p, text.as_bytes());
                c.mem.w32(a[4], p);
            } else if a[5] as usize >= text.len() {
                c.mem.write_bytes(a[4], text.as_bytes());
            } else {
                return 0;
            }
            text.len() as u32 - 1
        }),
        sh!("kernel32.dll", "GetFileTime", 4, |c, a| {
            let Some(Obj::File(f)) = handles::get(a[0]) else { return 0 };
            let Ok(m) = f.lock().unwrap().file.metadata() else { return 0 };
            for (p, t) in [(a[1], m.created()), (a[2], m.accessed()), (a[3], m.modified())] {
                if p != 0 {
                    let ft = t.map_or(0, filetime);
                    c.mem.w32(p, ft as u32);
                    c.mem.w32(p + 4, (ft >> 32) as u32);
                }
            }
            1
        }),
        sh!("kernel32.dll", "FileTimeToDosDateTime", 3, |c, a| {
            let ft = c.mem.r32(a[0]) as u64 | (c.mem.r32(a[0] + 4) as u64) << 32;
            let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
            let days = secs.div_euclid(86400);
            let rem = secs.rem_euclid(86400);
            // Civil date from days since 1970-01-01.
            let z = days + 719_468;
            let era = z.div_euclid(146_097);
            let doe = z.rem_euclid(146_097);
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let d = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            let y = yoe + era * 400 + i64::from(m <= 2);
            if !(1980..=2107).contains(&y) {
                return 0;
            }
            c.mem.w16(a[1], (((y - 1980) << 9) | (m << 5) | d) as u16);
            c.mem.w16(a[2], (((rem / 3600) << 11) | ((rem % 3600) / 60 << 5) | (rem % 60 / 2)) as u16);
            1
        }),
        sh!("kernel32.dll", "GetTempPathA", 2, |c, a| {
            let p = b"C:\\TEMP\\\0";
            if (a[0] as usize) < p.len() {
                return p.len() as u32;
            }
            c.mem.write_bytes(a[1], p);
            p.len() as u32 - 1
        }),
        // (path, prefix, unique, result)
        sh!("kernel32.dll", "GetTempFileNameA", 4, |c, a| {
            let n = if a[2] != 0 { a[2] } else { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_micros() & 0xffff | 1) };
            let mut path = cstr(c, a[0]);
            if !path.ends_with('\\') {
                path.push('\\');
            }
            let prefix: String = cstr(c, a[1]).chars().take(3).collect();
            let name = format!("{path}{prefix}{n:X}.tmp\0");
            c.mem.write_bytes(a[3], name.as_bytes());
            n
        }),
        sh!("kernel32.dll", "VirtualProtect", 4, |c, a| {
            if a[3] != 0 {
                c.mem.w32(a[3], 0x40);
            }
            1
        }),
        sh!("kernel32.dll", "lstrcpyA", 2, |c, a| {
            let s = c.mem.cstr(a[1]);
            c.mem.write_bytes(a[0], &s);
            c.mem.w8(a[0] + s.len() as u32, 0);
            a[0]
        }),
        // ---- ole32, rpcrt4, shell32 ----
        sh!("ole32.dll", "CoInitialize", 1, |_, _| 0),
        sh!("ole32.dll", "CoUninitialize", 0, |_, _| 0),
        sh!("ole32.dll", "CoCreateGuid", 1, |c, a| {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let pid = std::process::id() as u128;
            let mut x = (t ^ (pid << 64)).wrapping_mul(0x2545_f491_4f6c_dd1d_9e37_79b9_7f4a_7c15);
            x ^= x >> 47;
            let mut b = x.to_le_bytes();
            b[7] = (b[7] & 0x0f) | 0x40;
            b[8] = (b[8] & 0x3f) | 0x80;
            c.mem.write_bytes(a[0], &b);
            0
        }),
        // No COM class is really available: the caller gets an object that answers QueryInterface with itself and
        // fails every other method with E_NOTIMPL.
        sh!("ole32.dll", "CoCreateInstance", 5, |c, a| {
            let clsid = c.mem.r32(a[0]);
            if std::env::var_os("S4_HOST_TRACE").is_some() {
                eprintln!("CoCreateInstance CLSID {clsid:08X}");
            }
            if clsid == 0xd7b7_0ee0 {
                // CLSID_DirectDraw: created the COM way instead of with DirectDrawCreate.
                let v1 = c.mem.r32(a[3]) == 0x6c14_db80;
                let o = crate::ddraw::co_create_direct_draw(c, v1);
                c.mem.w32(a[4], o);
                return 0;
            }
            // DirectPlay (D1EB6D20, 2FE8F810) is answered by a stub object.
            if clsid != 0xd1eb_6d20 && clsid != 0x2fe8_f810 {
                if a[4] != 0 {
                    c.mem.w32(a[4], 0);
                }
                return 0x8004_0154;
            }
            let o = match clsid {
                0x2fe8_f810 => {
                    let vt = stub_vtable(c, "IDirectPlayLobby", &LOBBY_ARGS, &LOBBY_NOTLOBBIED);
                    crate::ddraw::new_object(c, vt)
                }
                _ => crate::dplay::create(c),
            };
            if a[4] != 0 {
                c.mem.w32(a[4], o);
            }
            0
        }),
        sh!("rpcrt4.dll", "UuidToStringA", 2, |c, a| {
            let g = c.mem.view(a[0], 16).to_vec();
            let d1 = u32::from_le_bytes(g[0..4].try_into().unwrap());
            let d2 = u16::from_le_bytes(g[4..6].try_into().unwrap());
            let d3 = u16::from_le_bytes(g[6..8].try_into().unwrap());
            let s = format!("{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}\0", g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]);
            let p = heap::alloc(c, s.len() as u32);
            c.mem.write_bytes(p, s.as_bytes());
            c.mem.w32(a[1], p);
            0
        }),
        sh!("rpcrt4.dll", "RpcStringFreeA", 1, |c, a| {
            let p = c.mem.r32(a[0]);
            if p != 0 {
                heap::free(c, p);
                c.mem.w32(a[0], 0);
            }
            0
        }),
        sh!("shell32.dll", "ShellExecuteExA", 1, |_, _| 0),
        // ---- user32 ----
        sh!("user32.dll", "PostMessageA", 4, |_, a| {
            crate::user32::post_message(a[0], a[1], a[2], a[3]);
            1
        }),
        sh!("user32.dll", "PostThreadMessageA", 4, |_, a| {
            crate::user32::post_thread_message(a[0], a[1], a[2], a[3]);
            1
        }),
        sh!("user32.dll", "SendMessageA", 4, |c, a| crate::user32::send_message(c, a[0], a[1], a[2], a[3])),
        // (hwnd, msg, wparam, lparam, flags, timeout, result*)
        sh!("user32.dll", "SendMessageTimeoutA", 7, |c, a| {
            let r = crate::user32::send_message(c, a[0], a[1], a[2], a[3]);
            if a[6] != 0 {
                c.mem.w32(a[6], r);
            }
            1
        }),
        sh!("user32.dll", "GetMessagePos", 0, |_, _| {
            let (x, y) = crate::user32::cursor();
            (y as u32) << 16 | (x as u32 & 0xffff)
        }),
        // (hwnd, x, y, width, height, repaint)
        sh!("user32.dll", "MoveWindow", 6, |_, a| {
            crate::user32::set_window_rect(a[0], [a[1] as i32, a[2] as i32, (a[1] + a[3]) as i32, (a[2] + a[4]) as i32]);
            1
        }),
        // (count, handles, wait all, timeout, mask): returns count when a window message is waiting
        sh!("user32.dll", "MsgWaitForMultipleObjects", 5, |c, a| {
            let deadline = (a[3] != 0xffff_ffff).then(|| std::time::Instant::now() + std::time::Duration::from_millis(a[3] as u64));
            loop {
                if crate::user32::queue_len() > 0 {
                    return a[0];
                }
                if a[0] > 0 {
                    let r = crate::kernel32::wait_multiple(c, a[0], a[1], a[2] != 0, 0);
                    if r != crate::kernel32::WAIT_TIMEOUT {
                        return r;
                    }
                }
                if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                    return crate::kernel32::WAIT_TIMEOUT;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }),
        sh!("user32.dll", "SetRect", 5, |c, a| {
            put_rect(c, a[0], [a[1] as i32, a[2] as i32, a[3] as i32, a[4] as i32]);
            1
        }),
        sh!("user32.dll", "IsRectEmpty", 1, |c, a| {
            let r = rect(c, a[0]);
            (r[2] <= r[0] || r[3] <= r[1]) as u32
        }),
        // (rect, x, y)
        sh!("user32.dll", "PtInRect", 3, |c, a| {
            let r = rect(c, a[0]);
            let (x, y) = (a[1] as i32, a[2] as i32);
            (x >= r[0] && x < r[2] && y >= r[1] && y < r[3]) as u32
        }),
        sh!("user32.dll", "IntersectRect", 3, |c, a| {
            let (p, q) = (rect(c, a[1]), rect(c, a[2]));
            let r = [p[0].max(q[0]), p[1].max(q[1]), p[2].min(q[2]), p[3].min(q[3])];
            if r[2] <= r[0] || r[3] <= r[1] {
                put_rect(c, a[0], [0; 4]);
                0
            } else {
                put_rect(c, a[0], r);
                1
            }
        }),
        // (hdc, BITMAPINFO*, usage, void** bits, section, offset): a bitmap whose pixels are guest memory the caller draws in.
        sh!("gdi32.dll", "CreateDIBSection", 6, |c, a| {
            let (size, w, h, bpp, used) = (c.mem.r32(a[1]), c.mem.r32(a[1] + 4) as i32, (c.mem.r32(a[1] + 8) as i32).unsigned_abs(), c.mem.r16(a[1] + 14) as u32, c.mem.r32(a[1] + 32));
            if size < 40 || w <= 0 || h == 0 || bpp == 0 {
                return 0;
            }
            if std::env::var_os("S4_FONT_TRACE").is_some() {
                eprintln!("CreateDIBSection {w}x{} (height field {}) bpp {bpp} compression {}", h, c.mem.r32(a[1] + 8) as i32, c.mem.r32(a[1] + 16));
            }
            let colors = if bpp <= 8 { if used != 0 { used } else { 1 << bpp } } else { 0 };
            let pitch = ((w as u32 * bpp + 31) / 32) * 4;
            let total = size + 4 * colors + pitch * h;
            let addr = heap::alloc_zeroed(c, total);
            let hdr = c.mem.read_bytes(a[1], (size + 4 * colors) as usize);
            c.mem.write_bytes(addr, &hdr);
            if a[3] != 0 {
                c.mem.w32(a[3], addr + size + 4 * colors);
            }
            let b = crate::gdi::dib_bitmap(c, addr);
            crate::gdi::new_handle(crate::gdi::Object::Bitmap(b))
        }),
        // ---- gdi32: objects are opaque handles; drawing with pens and regions is not needed yet ----
        sh!("gdi32.dll", "GetStockObject", 1, |_, a| 0x7000 + a[0]),
        sh!("gdi32.dll", "CreatePen", 3, |_, _| crate::gdi::new_handle(crate::gdi::Object::Other)),
        sh!("gdi32.dll", "CreateRectRgn", 4, |_, _| crate::gdi::new_handle(crate::gdi::Object::Other)),
        sh!("gdi32.dll", "SelectClipRgn", 2, |_, _| 2),
        sh!("gdi32.dll", "GdiFlush", 0, |_, _| 1),
        sh!("gdi32.dll", "GetCurrentObject", 2, |_, _| 0),
        sh!("gdi32.dll", "SetBkColor", 2, |_, a| {
            let old = crate::gdi::dc_state(a[0]).bk_color;
            crate::gdi::update_state(a[0], |s| s.bk_color = a[1]);
            old
        }),
        sh!("gdi32.dll", "MoveToEx", 4, |_, _| 1),
        sh!("gdi32.dll", "LineTo", 3, |_, _| 1),
        // LOGFONT: height at +0, weight at +16, italic at +20, face name at +28
        sh!("gdi32.dll", "CreateFontIndirectA", 1, |c, a| {
            let face = cstr(c, a[0] + 28);
            if std::env::var_os("S4_FONT_TRACE").is_some() {
                eprintln!("CreateFontIndirect '{face}' height {} weight {}", c.mem.r32(a[0]) as i32, c.mem.r32(a[0] + 16));
            }
            crate::gdi::new_handle(crate::gdi::Object::Font { height: c.mem.r32(a[0]) as i32, weight: c.mem.r32(a[0] + 16) as i32, italic: c.mem.r8(a[0] + 20) != 0, face })
        }),
        // TEXTMETRICA for a fixed-size face
        sh!("gdi32.dll", "GetTextMetricsA", 2, |c, a| {
            for o in (0..56).step_by(4) {
                c.mem.w32(a[1] + o, 0);
            }
            c.mem.w32(a[1], 15); // height
            c.mem.w32(a[1] + 4, 12); // ascent
            c.mem.w32(a[1] + 8, 3); // descent
            c.mem.w32(a[1] + 20, 7); // average width
            c.mem.w32(a[1] + 24, 14); // maximum width
            1
        }),
        // (dc, string, count, max width, fit*, dx*, size*): every character 7 pixels wide
        sh!("gdi32.dll", "GetTextExtentExPointA", 7, |c, a| {
            let n = a[2];
            let mut fit = 0;
            for i in 0..n {
                if a[3] == 0 || (i + 1) * 7 <= a[3] {
                    fit = i + 1;
                }
                if a[5] != 0 {
                    c.mem.w32(a[5] + 4 * i, (i + 1) * 7);
                }
            }
            if a[4] != 0 {
                c.mem.w32(a[4], fit);
            }
            if a[6] != 0 {
                c.mem.w32(a[6], n * 7);
                c.mem.w32(a[6] + 4, 15);
            }
            1
        }),
        // ---- winmm: the timer calls the guest from its own thread ----
        // (delay ms, resolution, callback, user, flags: 0 once, 1 periodic)
        sh!("winmm.dll", "timeSetEvent", 5, |_, a| {
            let (delay, cb, user, periodic) = (a[0].max(1), a[2], a[3], a[4] & 1 != 0);
            let id = NEXT_TIMER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            TIMERS.lock().unwrap().push(id);
            std::thread::Builder::new()
                .name(format!("timer-{id}"))
                .stack_size(16 << 20)
                .spawn(move || {
                    crate::kernel32::register_host_thread();
                    let mut c = crate::new_cpu(0);
                    let stack = heap::vm_alloc(&mut c, 1 << 20);
                    c.esp = stack + (1 << 20) - 64;
                    let mut next = std::time::Instant::now();
                    loop {
                        next += std::time::Duration::from_millis(delay as u64);
                        let now = std::time::Instant::now();
                        if next > now {
                            std::thread::sleep(next - now);
                        }
                        if !TIMERS.lock().unwrap().contains(&id) {
                            return;
                        }
                        crate::gil::acquire();
                        crate::shims::guest_call(&mut c, cb, &[id, 0, user, 0, 0]);
                        crate::gil::release();
                        if !periodic {
                            TIMERS.lock().unwrap().retain(|t| *t != id);
                            return;
                        }
                    }
                })
                .expect("timer thread");
            id
        }),
        sh!("winmm.dll", "timeKillEvent", 1, |_, a| {
            let mut t = TIMERS.lock().unwrap();
            let had = t.contains(&a[0]);
            t.retain(|x| *x != a[0]);
            if had { 0 } else { 97 } // TIMERR_NOCANDO
        }),
        // MCI (multimedia command interface): a device that opens, answers every command and finishes at once. (The
        // game plays its videos and the CD audio through MCI; see `mci_command`.)
        sh!("winmm.dll", "mciSendCommandA", 4, |c, a| crate::compat::mci_command(c, a[0], a[1], a[2], a[3])),
        sh!("winmm.dll", "mciSendStringA", 4, |c, a| {
            let cmd = cstr(c, a[0]);
            if std::env::var_os("S4_MCI_TRACE").is_some() {
                eprintln!("mciSendString({cmd:?}, return buffer {:#x} size {}, window {:#x})", a[1], a[2], a[3]);
            }
            if let Some(r) = crate::avi::send_string(c, &cmd, a[1], a[2], a[3]) {
                return r;
            }
            // Status queries get "0" back; a command that asks to notify gets its notification at once.
            if a[1] != 0 && a[2] > 1 {
                c.mem.write_bytes(a[1], b"0\0");
            }
            if cmd.split_whitespace().any(|w| w.eq_ignore_ascii_case("notify")) && a[3] != 0 {
                crate::user32::post_message(a[3], 0x3b9, 1, 1);
            }
            0
        }),
        sh!("winmm.dll", "mciGetErrorStringA", 3, |c, a| {
            let m = b"No multimedia device.\0";
            if (a[2] as usize) < m.len() {
                return 0;
            }
            c.mem.write_bytes(a[1], m);
            1
        }),
    ]
}
