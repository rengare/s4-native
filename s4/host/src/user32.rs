//! USER32: windows, the message queue, input state, text formatting.

use crate::kernel32::{cstr, set_last_error};
use crate::Shim;
use s4_x86::Cpu;
use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex};

macro_rules! shim {
    ($name:expr, $args:expr, $f:expr) => {
        Shim { dll: "user32.dll", name: $name, args: $args, cdecl: false, f: $f }
    };
}

pub const WM_QUIT: u32 = 0x12;

#[derive(Clone, Debug, Default)]
pub struct Msg {
    /// 0: for any thread; else only for the thread with this id (PostThreadMessage).
    pub tid: u32,
    pub hwnd: u32,
    pub message: u32,
    pub wparam: u32,
    pub lparam: u32,
    pub time: u32,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Window {
    pub class: String,
    pub wndproc: u32,
    pub style: u32,
    pub ex_style: u32,
    pub rect: [i32; 4],
    pub parent: u32,
    pub title: String,
    pub user_data: u32,
    pub long: HashMap<i32, u32>,
    pub visible: bool,
    pub enabled: bool,
    /// Has been shown (and activated) once.
    pub activated: bool,
    /// The thread that created the window (its messages are read there).
    pub owner: u32,
}

pub struct User {
    classes: HashMap<String, u32>,
    windows: HashMap<u32, Window>,
    next_hwnd: u32,
    queue: VecDeque<Msg>,
    quit: Option<i32>,
    /// Keyboard and mouse state as the host last reported it.
    pub keys: [bool; 256],
    pub cursor: (i32, i32),
    pub capture: u32,
    pub screen: (i32, i32),
}

impl Default for User {
    fn default() -> User {
        User { classes: HashMap::new(), windows: HashMap::new(), next_hwnd: 0x10010, queue: VecDeque::new(), quit: None, keys: [false; 256], cursor: (0, 0), capture: 0, screen: (1920, 1080) }
    }
}

static ACTIVATION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Makes `ShowWindow` queue the activation messages Windows sends a newly shown window (off by default).
pub fn activation_messages(on: bool) {
    ACTIVATION.store(on, std::sync::atomic::Ordering::Relaxed);
}

static USER: Mutex<Option<User>> = Mutex::new(None);
static QUEUE_CV: Condvar = Condvar::new();

fn with<R>(f: impl FnOnce(&mut User) -> R) -> R {
    let mut g = USER.lock().unwrap();
    let u = g.get_or_insert_with(User::default);
    f(u)
}

/// Queues a window message from the host (input, close requests).
pub fn post_message(hwnd: u32, message: u32, wparam: u32, lparam: u32) {
    with(|u| {
        let (x, y) = u.cursor;
        // A window's messages are read by the thread that made it; with no window they belong to the posting thread.
        let tid = if hwnd == 0 { crate::kernel32::current_thread_id() } else { u.windows.get(&hwnd).map_or(0, |w| w.owner) };
        u.queue.push_back(Msg { tid, hwnd, message, wparam, lparam, time: crate::shims::tick(), x, y });
    });
    QUEUE_CV.notify_all();
}

/// `PostThreadMessage`: the message goes to the queue of thread `tid` only.
pub(crate) fn post_thread_message(tid: u32, message: u32, wparam: u32, lparam: u32) {
    with(|u| {
        let (x, y) = u.cursor;
        u.queue.push_back(Msg { tid, hwnd: 0, message, wparam, lparam, time: crate::shims::tick(), x, y });
    });
    QUEUE_CV.notify_all();
}

pub fn set_key(vk: u8, down: bool) {
    with(|u| u.keys[vk as usize] = down);
}

static WARP: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);

/// The last `SetCursorPos` target the front end has not seen yet.
pub fn take_warp() -> Option<(i32, i32)> {
    WARP.lock().unwrap().take()
}

pub fn set_cursor(x: i32, y: i32) {
    with(|u| u.cursor = (x, y));
}

static MOUSE_DELTA: std::sync::Mutex<(i32, i32)> = std::sync::Mutex::new((0, 0));

/// Mouse movement the cursor does not show (the front end freezes the cursor during a right-button drag and grabs the
/// pointer): added up here, taken by the DirectInput mouse as relative movement.
pub fn add_mouse_delta(dx: i32, dy: i32) {
    let mut d = MOUSE_DELTA.lock().unwrap();
    *d = (d.0 + dx, d.1 + dy);
}

pub fn take_mouse_delta() -> (i32, i32) {
    std::mem::take(&mut *MOUSE_DELTA.lock().unwrap())
}

/// The screen size the game sees (its display mode; the window shows it scaled).
pub fn set_screen(w: i32, h: i32) {
    with(|u| u.screen = (w, h));
}

/// Number of window messages waiting.
pub(crate) fn queue_len() -> usize {
    with(|u| u.queue.len())
}

pub(crate) fn key_down(vk: u8) -> bool {
    with(|u| u.keys[vk as usize])
}

pub(crate) fn cursor() -> (i32, i32) {
    with(|u| u.cursor)
}

/// Sets a window's rectangle (screen coordinates: left, top, right, bottom).
pub(crate) fn set_window_rect(hwnd: u32, rect: [i32; 4]) {
    with(|u| {
        if let Some(w) = u.windows.get_mut(&hwnd) {
            w.rect = rect;
        }
    });
}

pub fn main_window() -> u32 {
    with(|u| u.windows.keys().copied().min().unwrap_or(0))
}

fn write_msg(c: &Cpu, p: u32, m: &Msg) {
    for (i, v) in [m.hwnd, m.message, m.wparam, m.lparam, m.time, m.x as u32, m.y as u32].iter().enumerate() {
        c.mem.w32(p + 4 * i as u32, *v);
    }
}

fn read_msg(c: &Cpu, p: u32) -> Msg {
    let r = |i: u32| c.mem.r32(p + 4 * i);
    Msg { tid: 0, hwnd: r(0), message: r(1), wparam: r(2), lparam: r(3), time: r(4), x: r(5) as i32, y: r(6) as i32 }
}

fn pump(c: &mut Cpu, p: u32, hwnd: u32, remove: bool, block: bool) -> u32 {
    let _ = hwnd;
    let mut g = USER.lock().unwrap();
    loop {
        let u = g.get_or_insert_with(User::default);
        if let Some(code) = u.quit {
            if u.queue.iter().all(|m| m.tid != 0 && m.tid != crate::kernel32::current_thread_id()) {
                if remove {
                    u.quit = None;
                }
                write_msg(c, p, &Msg { message: WM_QUIT, wparam: code as u32, ..Default::default() });
                return if block { 0 } else { 1 };
            }
        }
        let me = crate::kernel32::current_thread_id();
        if let Some(at) = u.queue.iter().position(|m| m.tid == 0 || m.tid == me) {
            let m = if remove { u.queue.remove(at).unwrap() } else { u.queue[at].clone() };
            write_msg(c, p, &m);
            return 1;
        }
        if !block {
            // A game that polls back-to-back is idle: do not burn a core. One that did work
            // since the last poll is busy (loading): do not slow it down.
            drop(g);
            thread_local!(static LAST_POLL: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) });
            let now = std::time::Instant::now();
            let idle = LAST_POLL.with(|l| l.replace(Some(now))).is_some_and(|t| now.duration_since(t) < std::time::Duration::from_micros(60));
            if idle {
                // S4_IDLE_SLEEP_US: experiment knob (default 300).
                static US: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
                let us = *US.get_or_init(|| std::env::var("S4_IDLE_SLEEP_US").ok().and_then(|v| v.parse().ok()).unwrap_or(300));
                if us > 0 {
                    std::thread::sleep(std::time::Duration::from_micros(us));
                }
                LAST_POLL.with(|l| l.set(Some(std::time::Instant::now())));
            }
            return 0;
        }
        g = QUEUE_CV.wait(g).unwrap();
    }
}

/// The window procedure of `hwnd` (0 if none).
fn wndproc_of(hwnd: u32) -> u32 {
    with(|u| u.windows.get(&hwnd).map(|w| w.wndproc).unwrap_or(0))
}

pub fn send_message(c: &mut Cpu, hwnd: u32, msg: u32, w: u32, l: u32) -> u32 {
    if std::env::var_os("S4_MSG_TRACE").is_some() {
        eprintln!("msg hwnd={hwnd:#x} {msg:#x} w={w:#x} l={l:#x}");
    }
    let proc_ = wndproc_of(hwnd);
    if proc_ == 0 {
        return 0;
    }
    crate::shims::guest_call(c, proc_, &[hwnd, msg, w, l])
}

pub fn all() -> Vec<Shim> {
    vec![
        shim!("RegisterClassExA", 1, |c, a| {
            let p = a[0];
            let proc_ = c.mem.r32(p + 8);
            let name = cstr(c, c.mem.r32(p + 40));
            with(|u| {
                u.classes.insert(name.to_lowercase(), proc_);
            });
            0xc001
        }),
        // WNDCLASSA: style, lpfnWndProc, cbClsExtra, cbWndExtra, hInstance, hIcon, hCursor, hbrBackground, lpszMenuName, lpszClassName
        shim!("RegisterClassA", 1, |c, a| {
            let proc_ = c.mem.r32(a[0] + 4);
            let name = cstr(c, c.mem.r32(a[0] + 36));
            if std::env::var_os("S4_WIN_TRACE").is_some() {
                eprintln!("RegisterClass '{name}' window procedure {proc_:#x}");
            }
            with(|u| {
                u.classes.insert(name.to_lowercase(), proc_);
            });
            0xc001
        }),
        shim!("CreateWindowExA", 12, |c, a| {
            let class = if a[1] < 0x10000 { format!("#{}", a[1]) } else { cstr(c, a[1]) };
            let title = if a[2] != 0 { cstr(c, a[2]) } else { String::new() };
            let proc_ = with(|u| u.classes.get(&class.to_lowercase()).copied()).unwrap_or(0);
            let (w, h) = (a[6] as i32, a[7] as i32);
            let hwnd = with(|u| {
                let h_ = u.next_hwnd;
                u.next_hwnd += 0x10;
                u.windows.insert(
                    h_,
                    Window { class, wndproc: proc_, style: a[3], ex_style: a[0], rect: [a[4] as i32, a[5] as i32, a[4] as i32 + w, a[5] as i32 + h], parent: a[8], title, enabled: true, owner: crate::kernel32::current_thread_id(), ..Default::default() },
                );
                h_
            });
            // WM_NCCREATE and WM_CREATE carry the CREATESTRUCT.
            let cs = crate::heap::alloc_zeroed(c, 48);
            c.mem.w32(cs, a[11]); // lpCreateParams
            c.mem.w32(cs + 4, a[10]); // hInstance
            c.mem.w32(cs + 8, a[9]);
            c.mem.w32(cs + 12, a[8]);
            c.mem.w32(cs + 16, h as u32);
            c.mem.w32(cs + 20, w as u32);
            c.mem.w32(cs + 24, a[5]);
            c.mem.w32(cs + 28, a[4]);
            c.mem.w32(cs + 32, a[3]);
            c.mem.w32(cs + 36, a[2]);
            c.mem.w32(cs + 40, a[1]);
            c.mem.w32(cs + 44, a[0]);
            send_message(c, hwnd, 0x81, 0, cs);
            send_message(c, hwnd, 0x01, 0, cs);
            hwnd
        }),
        shim!("DestroyWindow", 1, |c, a| {
            send_message(c, a[0], 0x02, 0, 0);
            with(|u| u.windows.remove(&a[0]).is_some()) as u32
        }),
        shim!("DefWindowProcA", 4, |c, a| match a[1] {
            0x10 => {
                // WM_CLOSE: destroy the window.
                send_message(c, a[0], 0x02, 0, 0);
                with(|u| u.windows.remove(&a[0]));
                0
            }
            _ => 0,
        }),
        shim!("ShowWindow", 2, |_, a| {
            let first = with(|u| match u.windows.get_mut(&a[0]) {
                Some(w) => {
                    let first = a[1] != 0 && !w.visible && !w.activated;
                    w.visible = a[1] != 0;
                    w.activated |= a[1] != 0;
                    first
                }
                None => false,
            });
            // Showing a window activates it: Windows tells it so with these messages (WM_SIZE, WM_ACTIVATEAPP,
            // WM_ACTIVATE, WM_SETFOCUS, WM_PAINT). Only for programs that ask (`activation_messages`).
            if first && ACTIVATION.load(std::sync::atomic::Ordering::Relaxed) {
                let size = with(|u| u.windows.get(&a[0]).map_or(0, |w| ((w.rect[3] - w.rect[1]) as u32) << 16 | (w.rect[2] - w.rect[0]) as u32 & 0xffff));
                for (m, w, l) in [(0x05, 0, size), (0x1c, 1, 0), (0x06, 1, 0), (0x07, 0, 0), (0x0f, 0, 0)] {
                    post_message(a[0], m, w, l);
                }
            }
            0
        }),
        shim!("SetWindowPos", 7, |_, a| {
            with(|u| {
                if let Some(w) = u.windows.get_mut(&a[0]) {
                    if a[6] & 2 == 0 {
                        let (wd, ht) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
                        w.rect = [a[2] as i32, a[3] as i32, a[2] as i32 + wd, a[3] as i32 + ht];
                    }
                    if a[6] & 1 == 0 {
                        let (x, y) = (w.rect[0], w.rect[1]);
                        w.rect = [x, y, x + a[4] as i32, y + a[5] as i32];
                    }
                }
            });
            1
        }),
        shim!("EnableWindow", 2, |_, a| {
            with(|u| {
                if let Some(w) = u.windows.get_mut(&a[0]) {
                    w.enabled = a[1] != 0;
                }
            });
            0
        }),
        shim!("GetClientRect", 2, |c, a| {
            let r = with(|u| u.windows.get(&a[0]).map(|w| w.rect)).unwrap_or([0, 0, 800, 600]);
            for (i, v) in [0, 0, r[2] - r[0], r[3] - r[1]].iter().enumerate() {
                c.mem.w32(a[1] + 4 * i as u32, *v as u32);
            }
            1
        }),
        shim!("GetWindowRect", 2, |c, a| {
            let r = with(|u| u.windows.get(&a[0]).map(|w| w.rect)).unwrap_or([0, 0, 800, 600]);
            for (i, v) in r.iter().enumerate() {
                c.mem.w32(a[1] + 4 * i as u32, *v as u32);
            }
            1
        }),
        shim!("ClientToScreen", 2, |_, _| 1),
        shim!("ScreenToClient", 2, |_, _| 1),
        shim!("GetActiveWindow", 0, |_, _| main_window()),
        shim!("GetLastActivePopup", 1, |_, a| a[0]),
        shim!("GetWindowLongA", 2, |_, a| with(|u| u.windows.get(&a[0]).map(|w| if a[1] as i32 == -4 { w.wndproc } else if a[1] as i32 == -21 { w.user_data } else if a[1] as i32 == -16 { w.style } else { w.long.get(&(a[1] as i32)).copied().unwrap_or(0) }).unwrap_or(0))),
        shim!("SetWindowLongA", 3, |_, a| with(|u| match u.windows.get_mut(&a[0]) {
            Some(w) => {
                let slot = match a[1] as i32 {
                    -4 => &mut w.wndproc,
                    -21 => &mut w.user_data,
                    -16 => &mut w.style,
                    k => w.long.entry(k).or_insert(0),
                };
                std::mem::replace(slot, a[2])
            }
            None => 0,
        })),
        shim!("GetMessageA", 4, |c, a| pump(c, a[0], a[1], true, true)),
        shim!("PeekMessageA", 5, |c, a| pump(c, a[0], a[1], a[4] & 1 != 0, false)),
        shim!("TranslateMessage", 1, |_, _| 0),
        shim!("DispatchMessageA", 1, |c, a| {
            let m = read_msg(c, a[0]);
            send_message(c, m.hwnd, m.message, m.wparam, m.lparam)
        }),
        shim!("PostQuitMessage", 1, |_, a| {
            with(|u| u.quit = Some(a[0] as i32));
            QUEUE_CV.notify_all();
            0
        }),
        shim!("GetSystemMetrics", 1, |_, a| {
            let (w, h) = with(|u| u.screen);
            match a[0] {
                0 | 16 => w as u32,
                1 | 17 => h as u32,
                4 => 19,
                5 | 6 => 1,
                7 | 8 => 3,
                32 | 33 => 4,
                _ => 0,
            }
        }),
        shim!("GetAsyncKeyState", 1, |_, a| with(|u| (u.keys[(a[0] & 0xff) as usize] as u32) << 15)),
        shim!("GetKeyState", 1, |_, a| with(|u| (u.keys[(a[0] & 0xff) as usize] as u32) << 15)),
        shim!("GetCursorPos", 1, |c, a| {
            let (x, y) = with(|u| u.cursor);
            c.mem.w32(a[0], x as u32);
            c.mem.w32(a[0] + 4, y as u32);
            1
        }),
        shim!("SetCursorPos", 2, |_, a| {
            if std::env::var_os("S4_INPUT_TRACE").is_some() {
                eprintln!("SetCursorPos({}, {})", a[0] as i32, a[1] as i32);
            }
            set_cursor(a[0] as i32, a[1] as i32);
            *WARP.lock().unwrap() = Some((a[0] as i32, a[1] as i32));
            1
        }),
        shim!("SetCapture", 1, |_, a| with(|u| std::mem::replace(&mut u.capture, a[0]))),
        shim!("ReleaseCapture", 0, |_, _| {
            with(|u| u.capture = 0);
            1
        }),
        shim!("LoadIconA", 2, |_, _| 1),
        shim!("LoadImageA", 6, |_, _| 1),
        shim!("MessageBeep", 1, |_, _| 1),
        shim!("MessageBoxA", 4, |c, a| {
            eprintln!("MessageBox: {} -- {}", cstr(c, a[2]), cstr(c, a[1]));
            1
        }),
        shim!("DialogBoxParamA", 5, |_, _| 1),
        shim!("EndDialog", 2, |_, _| 1),
        shim!("GetDlgItemTextA", 4, |_, _| 0),
        shim!("SetDlgItemTextA", 3, |_, _| 1),
        shim!("IsDlgButtonChecked", 2, |_, _| 0),
        shim!("OpenClipboard", 1, |_, _| 0),
        shim!("CloseClipboard", 0, |_, _| 1),
        shim!("EmptyClipboard", 0, |_, _| 1),
        shim!("GetClipboardData", 1, |_, _| 0),
        shim!("SetClipboardData", 2, |_, _| 0),
    ]
}

/// Windows `wsprintf`: formats `fmt` with the 32-bit arguments `args`.
pub fn format_c(c: &Cpu, fmt: u32, args: &mut dyn FnMut() -> u32) -> String {
    let f = c.mem.cstr(fmt);
    let mut out = String::new();
    let mut i = 0;
    while i < f.len() {
        let ch = f[i] as char;
        i += 1;
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let (mut left, mut zero, mut plus, mut space, mut alt) = (false, false, false, false, false);
        while i < f.len() {
            match f[i] {
                b'-' => left = true,
                b'0' => zero = true,
                b'+' => plus = true,
                b' ' => space = true,
                b'#' => alt = true,
                _ => break,
            }
            i += 1;
        }
        let mut width = 0usize;
        if i < f.len() && f[i] == b'*' {
            width = args() as i32 as usize;
            i += 1;
        } else {
            while i < f.len() && f[i].is_ascii_digit() {
                width = width * 10 + (f[i] - b'0') as usize;
                i += 1;
            }
        }
        let mut prec: Option<usize> = None;
        if i < f.len() && f[i] == b'.' {
            i += 1;
            let mut p = 0usize;
            if i < f.len() && f[i] == b'*' {
                p = args() as usize;
                i += 1;
            } else {
                while i < f.len() && f[i].is_ascii_digit() {
                    p = p * 10 + (f[i] - b'0') as usize;
                    i += 1;
                }
            }
            prec = Some(p);
        }
        let (mut short, mut long) = (false, false);
        while i < f.len() && matches!(f[i], b'h' | b'l' | b'L' | b'I') {
            if f[i] == b'h' {
                short = true;
            } else {
                long = true;
            }
            i += 1;
        }
        if i >= f.len() {
            break;
        }
        let conv = f[i] as char;
        i += 1;
        let _ = long;
        let body = match conv {
            'd' | 'i' => {
                let v = args() as i32;
                let v = if short { v as i16 as i32 } else { v };
                let mut s = v.unsigned_abs().to_string();
                if let Some(p) = prec {
                    while s.len() < p {
                        s.insert(0, '0');
                    }
                }
                let sign = if v < 0 { "-" } else if plus { "+" } else if space { " " } else { "" };
                pad_num(sign, &s, width, left, zero && prec.is_none())
            }
            'u' => {
                let v = args();
                let v = if short { v as u16 as u32 } else { v };
                pad_num("", &v.to_string(), width, left, zero)
            }
            'x' | 'X' => {
                let v = args();
                let v = if short { v as u16 as u32 } else { v };
                let mut s = if conv == 'x' { format!("{v:x}") } else { format!("{v:X}") };
                if let Some(p) = prec {
                    while s.len() < p {
                        s.insert(0, '0');
                    }
                }
                pad_num(if alt && v != 0 { if conv == 'x' { "0x" } else { "0X" } } else { "" }, &s, width, left, zero)
            }
            'p' => format!("{:08X}", args()),
            'c' => {
                let v = args() as u8 as char;
                pad_num("", &v.to_string(), width, left, false)
            }
            's' | 'S' => {
                let p = args();
                let mut s: String = if p == 0 { "(null)".into() } else { c.mem.cstr(p).iter().map(|&b| b as char).collect() };
                if let Some(pr) = prec {
                    s.truncate(pr);
                }
                pad_num("", &s, width, left, false)
            }
            '%' => "%".to_string(),
            other => format!("%{other}"),
        };
        out.push_str(&body);
    }
    out
}

fn pad_num(sign: &str, digits: &str, width: usize, left: bool, zero: bool) -> String {
    let len = sign.len() + digits.len();
    if len >= width {
        return format!("{sign}{digits}");
    }
    let pad = width - len;
    if left {
        format!("{sign}{digits}{}", " ".repeat(pad))
    } else if zero {
        format!("{sign}{}{digits}", "0".repeat(pad))
    } else {
        format!("{}{sign}{digits}", " ".repeat(pad))
    }
}

pub fn cdecl() -> Vec<Shim> {
    vec![
        Shim {
            dll: "user32.dll",
            name: "wsprintfA",
            args: 0,
            cdecl: true,
            f: |c, _| {
                // wsprintfA(char *dst, const char *fmt, ...): arguments follow on the stack.
                let base = c.esp + 4;
                let dst = c.mem.r32(base);
                let fmt = c.mem.r32(base + 4);
                let mut next = base + 8;
                let s = format_c(c, fmt, &mut || {
                    let v = c.mem.r32(next);
                    next += 4;
                    v
                });
                let bytes: Vec<u8> = s.chars().map(|ch| ch as u32 as u8).collect();
                c.mem.write_bytes(dst, &bytes);
                c.mem.w8(dst + bytes.len() as u32, 0);
                bytes.len() as u32
            },
        },
        Shim {
            dll: "user32.dll",
            name: "wvsprintfA",
            args: 3,
            cdecl: false,
            f: |c, a| {
                let mut next = a[2];
                let s = format_c(c, a[1], &mut || {
                    let v = c.mem.r32(next);
                    next += 4;
                    v
                });
                let bytes: Vec<u8> = s.chars().map(|ch| ch as u32 as u8).collect();
                c.mem.write_bytes(a[0], &bytes);
                c.mem.w8(a[0] + bytes.len() as u32, 0);
                bytes.len() as u32
            },
        },
    ]
}

#[allow(dead_code)]
fn unused(_: u32) {
    set_last_error(0);
}
