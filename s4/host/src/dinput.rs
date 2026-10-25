//! DirectInput (versions 3 to 7, ANSI) for the games that read the mouse and keyboard through it. The state comes
//! from the same key and cursor tables the window procedure's messages are made from.

use crate::ddraw::{add_ref, make_vtable, new_object, release};
use s4_x86::Cpu;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

const DI: &[&str] = &["QueryInterface", "AddRef", "Release", "CreateDevice", "EnumDevices", "GetDeviceStatus", "RunControlPanel", "Initialize", "FindDevice"];
const DEV: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "GetCapabilities", "EnumObjects", "GetProperty", "SetProperty", "Acquire", "Unacquire", "GetDeviceState", "GetDeviceData", "SetDataFormat", "SetEventNotification",
    "SetCooperativeLevel", "GetObjectInfo", "GetDeviceInfo", "RunControlPanel", "Initialize",
];

const DIERR_INVALIDPARAM: u32 = 0x8007_0057;
const DIERR_DEVICENOTREG: u32 = 0x8007_0002;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Mouse,
    Keyboard,
    Other,
}

struct Device {
    kind: Kind,
    last: (i32, i32),
    /// Mouse buttons (bit 0 left, 1 right, 2 middle) as the last buffered read saw them, and the event counter.
    buttons: u8,
    seq: u32,
    /// Keyboard: the keys (by DirectInput scan code) as the last buffered read saw them.
    keys: Vec<u8>,
    /// Buffered data not handed out yet: (offset, value).
    pending: VecDeque<(u32, u32)>,
}

static DEVICES: Mutex<Option<HashMap<u32, Device>>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut HashMap<u32, Device>) -> R) -> R {
    f(DEVICES.lock().unwrap().get_or_insert_with(HashMap::new))
}

/// The scan code (set 1, as the keyboard sends it) of a virtual key; 0 if there is none.
pub fn scan_code(vk: u8) -> u8 {
    match vk {
        0x1b => 1,
        b'1'..=b'9' => vk - b'1' + 2,
        b'0' => 11,
        0xbd => 12,
        0xbb => 13,
        0x08 => 14,
        0x09 => 15,
        b'Q' => 16, b'W' => 17, b'E' => 18, b'R' => 19, b'T' => 20, b'Y' => 21, b'U' => 22, b'I' => 23, b'O' => 24, b'P' => 25,
        0xdb => 26,
        0xdd => 27,
        0x0d => 28,
        0x11 | 0xa2 => 29,
        b'A' => 30, b'S' => 31, b'D' => 32, b'F' => 33, b'G' => 34, b'H' => 35, b'J' => 36, b'K' => 37, b'L' => 38,
        0xba => 39,
        0xde => 40,
        0xc0 => 41,
        0x10 | 0xa0 => 42,
        0xdc => 43,
        b'Z' => 44, b'X' => 45, b'C' => 46, b'V' => 47, b'B' => 48, b'N' => 49, b'M' => 50,
        0xbc => 51,
        0xbe => 52,
        0xbf => 53,
        0xa1 => 54,
        0x12 | 0xa4 => 56,
        0x20 => 57,
        0x14 => 58,
        0x70..=0x79 => vk - 0x70 + 59,
        0x7a => 87,
        0x7b => 88,
        0x26 => 200,
        0x25 => 203,
        0x27 => 205,
        0x28 => 208,
        0x24 => 199,
        0x23 => 207,
        0x21 => 201,
        0x22 => 209,
        0x2d => 210,
        0x2e => 211,
        _ => 0,
    }
}

fn device_vtable(c: &mut Cpu) -> u32 {
    static VTABLE: Mutex<u32> = Mutex::new(0);
    let mut g = VTABLE.lock().unwrap();
    if *g != 0 {
        return *g;
    }
    *g = make_vtable(
        c,
        "IDirectInputDevice",
        DEV,
        &[
            ("QueryInterface", 3, |c, a| {
                c.mem.w32(a[2], a[0]);
                add_ref(c, a[0]);
                0
            }),
            ("AddRef", 1, |c, a| add_ref(c, a[0])),
            ("Release", 1, |c, a| {
                let n = release(c, a[0]);
                if n == 0 {
                    with(|d| d.remove(&a[0]));
                }
                n
            }),
            ("GetCapabilities", 2, |c, a| {
                let size = c.mem.r32(a[1]);
                for o in (4..size.min(44)).step_by(4) {
                    c.mem.w32(a[1] + o, 0);
                }
                0
            }),
            ("EnumObjects", 4, |_, _| 0),
            ("GetProperty", 3, |_, _| 0),
            ("SetProperty", 3, |_, _| 0),
            ("Acquire", 1, |_, _| 0),
            ("Unacquire", 1, |_, _| 0),
            // (this, size, data*)
            ("GetDeviceState", 3, |c, a| {
                let Some((kind, last)) = with(|d| d.get(&a[0]).map(|d| (d.kind, d.last))) else { return DIERR_INVALIDPARAM };
                for o in 0..a[1] {
                    c.mem.w8(a[2] + o, 0);
                }
                match kind {
                    Kind::Keyboard => {
                        for vk in 1..=255u8 {
                            if crate::user32::key_down(vk) {
                                let sc = scan_code(vk);
                                if sc != 0 && (sc as u32) < a[1] {
                                    c.mem.w8(a[2] + sc as u32, 0x80);
                                }
                            }
                        }
                    }
                    Kind::Mouse => {
                        let now = crate::user32::cursor();
                        if a[1] >= 12 {
                            c.mem.w32(a[2], (now.0 - last.0) as u32);
                            c.mem.w32(a[2] + 4, (now.1 - last.1) as u32);
                        }
                        for (i, vk) in [1u8, 2, 4].iter().enumerate() {
                            if a[1] > 12 + i as u32 && crate::user32::key_down(*vk) {
                                c.mem.w8(a[2] + 12 + i as u32, 0x80);
                            }
                        }
                        with(|d| d.get_mut(&a[0]).map(|d| d.last = now));
                    }
                    Kind::Other => {}
                }
                0
            }),
            // (this, object size, data*, count*, flags): the buffered data is made from the cursor, button and key state:
            // each read appends what changed since the one before to the device's buffer (mouse: one X and one Y event for
            // the movement, relative like the device, and one per button, DIMOFS_BUTTON0.. at 12; keyboard: one per key,
            // by scan code) and hands out as many as there is room for. The right-drag camera of The Settlers III reads
            // this two events at a time, so a button change during a movement (three events) spans two reads.
            ("GetDeviceData", 5, |c, a| {
                let (size, data, count_p, flags) = (a[1], a[2], a[3], a[4]);
                let max = if count_p != 0 { c.mem.r32(count_p) } else { 0 };
                let Some(kind) = with(|d| d.get(&a[0]).map(|d| d.kind)) else { return DIERR_INVALIDPARAM };
                let now = crate::user32::cursor();
                let extra = if kind == Kind::Mouse { crate::user32::take_mouse_delta() } else { (0, 0) };
                let held: Vec<bool> = match kind {
                    Kind::Mouse => [1u8, 2, 4].iter().map(|&vk| crate::user32::key_down(vk)).collect(),
                    Kind::Keyboard => (1..=255u8).map(crate::user32::key_down).collect(),
                    Kind::Other => Vec::new(),
                };
                let (out, seq, added) = with(|d| {
                    let dev = d.get_mut(&a[0]).unwrap();
                    let before = dev.pending.len();
                    if kind == Kind::Mouse {
                        let (dx, dy) = (now.0 - dev.last.0 + extra.0, now.1 - dev.last.1 + extra.1);
                        dev.last = now;
                        if dx != 0 {
                            dev.pending.push_back((0, dx as u32));
                        }
                        if dy != 0 {
                            dev.pending.push_back((4, dy as u32));
                        }
                        for (i, &down) in held.iter().enumerate() {
                            if down != (dev.buttons >> i & 1 != 0) {
                                dev.buttons ^= 1 << i;
                                dev.pending.push_back((12 + i as u32, if down { 0x80 } else { 0 }));
                            }
                        }
                    }
                    if kind == Kind::Keyboard {
                        let mut keys = vec![0u8; 256];
                        for (vk, &down) in (1..=255u8).zip(&held) {
                            let sc = scan_code(vk) as usize;
                            if sc != 0 && down {
                                keys[sc] = 0x80;
                            }
                        }
                        for sc in 1..256usize {
                            if keys[sc] != dev.keys[sc] {
                                dev.pending.push_back((sc as u32, keys[sc] as u32));
                            }
                        }
                        dev.keys = keys;
                    }
                    let n = dev.pending.len().min(max as usize);
                    let out: Vec<(u32, u32)> = dev.pending.iter().take(n).copied().collect();
                    let seq = dev.seq;
                    // DIGDD_PEEK (1) leaves the data in the buffer; otherwise what was handed out is consumed. (No data
                    // pointer and an INFINITE count: the buffer is flushed.)
                    if flags & 1 == 0 {
                        dev.pending.drain(..n);
                        dev.seq += n as u32;
                    }
                    (out, seq, dev.pending.len() + n - before)
                });
                if (added > 0 || !out.is_empty()) && std::env::var_os("S4_DI_TRACE").is_some() {
                    eprintln!("dinput {kind:?}: {} events handed out (room for {max}, flags {flags:#x}, data {data:#x}): {:x?}", out.len(), out);
                }
                if data != 0 && size >= 16 {
                    let stamp = crate::shims::tick();
                    for (i, (ofs, val)) in out.iter().enumerate() {
                        let at = data + size * i as u32;
                        c.mem.w32(at, *ofs);
                        c.mem.w32(at + 4, *val);
                        c.mem.w32(at + 8, stamp);
                        c.mem.w32(at + 12, seq + i as u32);
                        for o in (16..size).step_by(4) {
                            c.mem.w32(at + o, 0);
                        }
                    }
                }
                if count_p != 0 {
                    c.mem.w32(count_p, out.len() as u32);
                }
                0
            }),
            ("SetDataFormat", 2, |_, _| 0),
            ("SetEventNotification", 2, |_, _| 0),
            ("SetCooperativeLevel", 3, |_, _| 0),
            ("GetObjectInfo", 4, |_, _| DIERR_INVALIDPARAM),
            ("GetDeviceInfo", 2, |_, _| DIERR_INVALIDPARAM),
            ("RunControlPanel", 2, |_, _| 0),
            ("Initialize", 3, |_, _| 0),
        ],
    );
    *g
}

fn input_vtable(c: &mut Cpu) -> u32 {
    static VTABLE: Mutex<u32> = Mutex::new(0);
    let mut g = VTABLE.lock().unwrap();
    if *g != 0 {
        return *g;
    }
    *g = make_vtable(
        c,
        "IDirectInput",
        DI,
        &[
            ("QueryInterface", 3, |c, a| {
                c.mem.w32(a[2], a[0]);
                add_ref(c, a[0]);
                0
            }),
            ("AddRef", 1, |c, a| add_ref(c, a[0])),
            ("Release", 1, |c, a| release(c, a[0])),
            // (this, GUID*, IDirectInputDevice**, outer)
            ("CreateDevice", 4, |c, a| {
                let g = c.mem.read_bytes(a[1], 16);
                // GUID_SysMouse {6F1D2B60-...} and GUID_SysKeyboard {6F1D2B61-...}.
                let kind = if g[0..4] == [0x60, 0x2b, 0x1d, 0x6f] {
                    Kind::Mouse
                } else if g[0..4] == [0x61, 0x2b, 0x1d, 0x6f] {
                    Kind::Keyboard
                } else {
                    return DIERR_DEVICENOTREG;
                };
                let vt = device_vtable(c);
                let o = new_object(c, vt);
                with(|d| d.insert(o, Device { kind, last: crate::user32::cursor(), buttons: 0, seq: 0, keys: vec![0; 256], pending: VecDeque::new() }));
                c.mem.w32(a[2], o);
                0
            }),
            ("EnumDevices", 5, |_, _| 0),
            ("GetDeviceStatus", 2, |_, _| 1), // DI_NOTATTACHED... not queried for the system devices
            ("RunControlPanel", 3, |_, _| 0),
            ("Initialize", 3, |_, _| 0),
            ("FindDevice", 4, |_, _| DIERR_DEVICENOTREG),
        ],
    );
    *g
}

/// `DirectInputCreateA(hinst, version, IDirectInput **, outer)`.
pub fn direct_input_create(c: &mut Cpu, a: &[u32]) -> u32 {
    let vt = input_vtable(c);
    let o = new_object(c, vt);
    c.mem.w32(a[2], o);
    0
}

/// The `lParam` of a WM_KEYDOWN / WM_KEYUP: repeat count, scan code, "extended key" flag, and the previous state and
/// transition bits of a release.
pub fn key_lparam(vk: u8, down: bool) -> u32 {
    let sc = scan_code(vk) as u32;
    // extended keys: the cursor block, right Ctrl/Alt, numpad Enter and Divide (DirectInput numbers them above 0x7f)
    let ext = (matches!(vk, 0x21..=0x28 | 0x2d | 0x2e | 0xa3 | 0xa5) || sc >= 0x80) as u32;
    1 | sc << 16 | ext << 24 | if down { 0 } else { 0xc000_0000 }
}
