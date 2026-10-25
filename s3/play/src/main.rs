//! Plays the lifted Settlers III: `s3play [game dir] [--scale pixelart|linear|nearest|integer]`.
//!
//! The game runs on its own thread (the lifted code, with the Windows functions it imports implemented
//! by `s4-host`); this thread owns the SDL3 window: it shows the guest's screen, turns SDL's mouse,
//! keyboard and window events into the window messages the game's message loop reads, shows the cursor
//! the game selects, and opens the audio device the game's mixer plays into.

#[path = "../../../s4/play/src/audio.rs"]
mod audio;
#[path = "../../../s4/play/src/keys.rs"]
mod keys;

use s4_host::{present, user32};
use sdl3_sys::everything::*;
use std::ffi::{c_int, c_void, CStr, CString};
use std::ptr::{null, null_mut};

const WM_KEYDOWN: u32 = 0x100;
const WM_KEYUP: u32 = 0x101;
const WM_CHAR: u32 = 0x102;
const WM_MOUSEMOVE: u32 = 0x200;
const WM_LBUTTONDOWN: u32 = 0x201;
const WM_LBUTTONUP: u32 = 0x202;
const WM_RBUTTONDOWN: u32 = 0x204;
const WM_RBUTTONUP: u32 = 0x205;
const WM_MBUTTONDOWN: u32 = 0x207;
const WM_MBUTTONUP: u32 = 0x208;
const WM_MOUSEWHEEL: u32 = 0x20a;
const WM_CLOSE: u32 = 0x10;

struct App {
    window: *mut SDL_Window,
    renderer: *mut SDL_Renderer,
    texture: *mut SDL_Texture,
    /// The guest screen size the texture and the renderer's logical size currently have.
    shown: (u32, u32),
    /// How the game's picture is scaled to the window (`--scale`, `S4_SCALE`).
    scale: Scale,
    frame: Option<std::sync::Arc<present::Frame>>,
    seen: u64,
    seen_at_report: u64,
    /// S4_AUTODRAG="x0,y0,x1,y1@secs[:l|r|R]": press at (x0,y0), move to (x1,y1) over 2 s in steps, release. 'R': a right
    /// drag with the pointer grabbed, (x1-x0, y1-y0) of DirectInput movement every step.
    drags: Vec<(i32, i32, i32, i32, f32, char)>,
    /// S4_FPS=1: (draws, seconds spent drawing, game frames, last report) per second.
    stats: (u32, f64, u32, std::time::Instant),
    /// S4_FRAME_DUMP=<path.ppm> S4_FRAME_AT=<secs>: write the game's frame (for comparisons).
    dumped: bool,
    /// S4_FRAME_SEQ="t1,t2,..." S4_FRAME_DUMP=<path>: also write the frame at each of these seconds to <path>.<n>.ppm.
    seq_done: Vec<bool>,
    cursor_serial: u64,
    cursor: *mut SDL_Cursor,
    mouse: (i32, i32),
    /// The game warps the cursor (`SetCursorPos`) while the right button is held (the camera drag: it
    /// puts the cursor at the screen centre every step and reads how far it was moved). The real pointer
    /// is not moved meanwhile; real motion is applied relative to (real position at the warp, warp
    /// target). On release the real pointer is put where the game's cursor is.
    anchor: Option<((f32, f32), (i32, i32))>,
    last_real: (f32, f32),
    /// The right button is held and the pointer is grabbed (relative mode): the camera drag goes on beyond the window edge.
    relative: bool,
    rel_resid: (f32, f32),
    buttons: u32,
    shift: bool,
    ctrl: bool,
    alt: bool,
    fullscreen: bool,
    /// A printable key was pressed and no text event has followed yet (see `tick_text`).
    pending_char: Option<char>,
    /// Some text event has arrived: the system provides text input, so no fallback is needed.
    text_events: bool,
    redraw: bool,
    started: std::time::Instant,
    /// S4_AUTOCLICK="x,y@seconds[;x,y@seconds...]": synthetic clicks in game coordinates (for tests).
    clicks: Vec<(i32, i32, f32, bool)>,
    /// The scripted pointer arrives 0.3 s before its click (the game's buttons react to a prior hover).
    click_moved: Vec<bool>,
    /// Per click: 'l' left, 'r' right, 'm' move only (S4_AUTOCLICK "x,y@secs:r").
    click_kind: Vec<char>,
    /// S4_AUTOKEY="vk@seconds[;...]": key presses (virtual-key codes, decimal or 0x..), 1.5 s long.
    keys: Vec<(u8, f32, u8)>,
}

/// The character a key types: `key` is the SDL keycode (lower case / unshifted), `upper` is shift or
/// caps lock for letters, `shift` for the other keys.
fn printable(key: u32, upper: bool, shift: bool) -> Option<char> {
    let c = char::from_u32(key).filter(|c| (' '..='~').contains(c))?;
    if c.is_ascii_lowercase() {
        return Some(if upper { c.to_ascii_uppercase() } else { c });
    }
    if !shift {
        return Some(c);
    }
    const FROM: &str = "1234567890-=[]\\;',./`";
    const TO: &str = "!@#$%^&*()_+{}|:\"<>?~";
    Some(FROM.find(c).and_then(|i| TO.chars().nth(i)).unwrap_or(c))
}

fn sdl_error() -> String {
    unsafe { CStr::from_ptr(SDL_GetError()).to_string_lossy().into_owned() }
}

impl App {
    fn new(window_size: (i32, i32), scale: Scale) -> App {
        let title = CString::new(std::env::var("S4_TITLE").unwrap_or_else(|_| "The Settlers IV (native)".into())).unwrap();
        let (window, renderer) = unsafe {
            let window = SDL_CreateWindow(title.as_ptr(), window_size.0, window_size.1, SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY);
            assert!(!window.is_null(), "cannot create a window: {}", sdl_error());
            let renderer = SDL_CreateRenderer(window, null());
            assert!(!renderer.is_null(), "cannot create a renderer: {}", sdl_error());
            if std::env::var_os("S4_VSYNC").is_some() {
                SDL_SetRenderVSync(renderer, 1);
            }
            SDL_SetRenderLogicalPresentation(renderer, 800, 600, SDL_LOGICAL_PRESENTATION_LETTERBOX);
            let started = SDL_StartTextInput(window);
            if std::env::var_os("S4_INPUT_TRACE").is_some() {
                eprintln!("text input started: {started}, active: {}", SDL_TextInputActive(window));
            }
            (window, renderer)
        };
        App {
            window,
            renderer,
            texture: null_mut(),
            shown: (800, 600),
            scale,
            frame: None,
            seen: 0,
            seen_at_report: 0,
            drags: std::env::var("S4_AUTODRAG").unwrap_or_default().split(';').filter_map(|d| { let (c, rest) = d.split_once('@')?; let v: Vec<i32> = c.split(',').filter_map(|n| n.trim().parse().ok()).collect(); let (t, k) = rest.split_once(':').unwrap_or((rest, "r")); (v.len() == 4).then(|| (v[0], v[1], v[2], v[3], t.trim().parse().unwrap_or(0.0), k.chars().next().unwrap_or('r'))) }).collect(),
            stats: (0, 0.0, 0, std::time::Instant::now()),
            dumped: false,
            seq_done: Vec::new(),
            cursor_serial: u64::MAX,
            cursor: null_mut(),
            mouse: (0, 0),
            anchor: None,
            last_real: (0.0, 0.0),
            relative: false,
            rel_resid: (0.0, 0.0),
            buttons: 0,
            shift: false,
            ctrl: false,
            alt: false,
            fullscreen: false,
            pending_char: None,
            text_events: false,
            redraw: true,
            started: std::time::Instant::now(),
            clicks: parse_clicks().0,
            click_moved: Vec::new(),
            click_kind: parse_clicks().1,
            keys: std::env::var("S4_AUTOKEY").ok().map(|v| v.split(';').filter_map(|c| { let (k, t) = c.split_once('@')?; let k = k.trim(); let k = match k.strip_prefix("0x") { Some(h) => u8::from_str_radix(h, 16).ok()?, None => k.parse().ok()? }; Some((k, t.trim().parse().ok()?, 0u8)) }).collect()).unwrap_or_default(),
        }
    }

    /// Window pixels per guest pixel.
    fn scale(&self) -> f32 {
        let (mut w, mut h) = (0, 0);
        unsafe { SDL_GetWindowSizeInPixels(self.window, &mut w, &mut h) };
        (w as f32 / self.shown.0 as f32).min(h as f32 / self.shown.1 as f32).max(0.01)
    }

    fn flags(&self) -> u32 {
        self.buttons | if self.shift { 4 } else { 0 } | if self.ctrl { 8 } else { 0 }
    }

    fn post_mouse(&self, msg: u32) {
        let lparam = ((self.mouse.1 as u32 & 0xffff) << 16) | (self.mouse.0 as u32 & 0xffff);
        user32::post_message(user32::main_window(), msg, self.flags(), lparam);
    }

    /// Lets go of every held button and modifier (a release that happens while another window has
    /// focus never arrives).
    fn release_all(&mut self) {
        for (bit, vkey, up) in [(1, 0x01u8, WM_LBUTTONUP), (2, 0x02, WM_RBUTTONUP), (0x10, 0x04, WM_MBUTTONUP)] {
            if self.buttons & bit != 0 {
                self.buttons &= !bit;
                user32::set_key(vkey, false);
                self.post_mouse(up);
            }
        }
        self.anchor = None;
        self.set_relative(false);
        self.shift = false;
        self.ctrl = false;
        self.alt = false;
        for k in [0x10u8, 0x11, 0x12] {
            user32::set_key(k, false);
        }
    }

    /// Grabs the pointer for a right-button drag (the camera), or lets it go and puts it where the game's cursor is.
    fn set_relative(&mut self, on: bool) {
        if self.relative == on {
            return;
        }
        self.relative = on;
        self.rel_resid = (0.0, 0.0);
        unsafe {
            SDL_SetWindowRelativeMouseMode(self.window, on);
            if !on {
                let (mut wx, mut wy) = (0.0, 0.0);
                SDL_RenderCoordinatesToWindow(self.renderer, self.mouse.0 as f32, self.mouse.1 as f32, &mut wx, &mut wy);
                SDL_WarpMouseInWindow(self.window, wx, wy);
                self.last_real = (self.mouse.0 as f32, self.mouse.1 as f32);
            }
        }
    }

    /// Puts the real pointer where the game's cursor is (after a drag during which the game moved it).
    fn end_anchor(&mut self) {
        if self.anchor.take().is_none() {
            return;
        }
        let (mut wx, mut wy) = (0.0, 0.0);
        unsafe {
            SDL_RenderCoordinatesToWindow(self.renderer, self.mouse.0 as f32, self.mouse.1 as f32, &mut wx, &mut wy);
            SDL_WarpMouseInWindow(self.window, wx, wy);
        }
        self.last_real = (self.mouse.0 as f32, self.mouse.1 as f32);
        if std::env::var_os("S4_INPUT_TRACE").is_some() {
            eprintln!("pointer put back at game ({}, {}) = window ({wx:.0}, {wy:.0})", self.mouse.0, self.mouse.1);
        }
    }

    fn handle(&mut self, ev: &mut SDL_Event) {
        unsafe {
            let ty = SDL_EventType(ev.r#type);
            if std::env::var_os("S4_INPUT_TRACE").is_some() && ty != SDL_EVENT_MOUSE_MOTION {
                eprintln!("event {:#x}", ev.r#type);
            }
            match ty {
                SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED => {
                    user32::post_message(user32::main_window(), WM_CLOSE, 0, 0);
                }
                SDL_EVENT_WINDOW_FOCUS_LOST => self.release_all(),
                SDL_EVENT_WINDOW_EXPOSED | SDL_EVENT_WINDOW_RESIZED | SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED | SDL_EVENT_WINDOW_SHOWN => self.redraw = true,
                SDL_EVENT_MOUSE_MOTION => {
                    let (w, h) = (self.shown.0 as f32, self.shown.1 as f32);
                    // Window coordinates -> the renderer's logical (guest) coordinates, letterbox included.
                    SDL_ConvertEventToRenderCoordinates(self.renderer, ev);
                    if self.relative {
                        // The cursor stays; the movement goes to the DirectInput mouse (game pixels, fractions kept).
                        self.rel_resid.0 += ev.motion.xrel;
                        self.rel_resid.1 += ev.motion.yrel;
                        let (ix, iy) = (self.rel_resid.0 as i32, self.rel_resid.1 as i32);
                        self.rel_resid = (self.rel_resid.0 - ix as f32, self.rel_resid.1 - iy as f32);
                        user32::add_mouse_delta(ix, iy);
                        // A real mouse still sends WM_MOUSEMOVE; the game reads the DirectInput buffer when one arrives.
                        if ix != 0 || iy != 0 {
                            self.post_mouse(WM_MOUSEMOVE);
                        }
                        return;
                    }
                    let (px, py) = (ev.motion.x, ev.motion.y);
                    if let Some(t) = user32::take_warp() {
                        if self.buttons != 0 {
                            self.anchor = Some((self.last_real, t));
                        }
                    }
                    self.last_real = (px, py);
                    let (vx, vy) = match self.anchor {
                        Some(((rx, ry), (tx, ty))) => (tx as f32 + (px - rx), ty as f32 + (py - ry)),
                        None => (px, py),
                    };
                    self.mouse = (vx.clamp(0.0, w - 1.0) as i32, vy.clamp(0.0, h - 1.0) as i32);
                    if std::env::var_os("S4_INPUT_TRACE").is_some() {
                        eprintln!("motion: game cursor ({}, {}) anchored={}", self.mouse.0, self.mouse.1, self.anchor.is_some());
                    }
                    user32::set_cursor(self.mouse.0, self.mouse.1);
                    self.post_mouse(WM_MOUSEMOVE);
                }
                SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP => {
                    let down = ev.button.down;
                    let (bit, vkey, dn, up) = match ev.button.button as u32 {
                        1 => (1, 0x01, WM_LBUTTONDOWN, WM_LBUTTONUP),
                        3 => (2, 0x02, WM_RBUTTONDOWN, WM_RBUTTONUP),
                        2 => (0x10, 0x04, WM_MBUTTONDOWN, WM_MBUTTONUP),
                        _ => return,
                    };
                    if down {
                        self.buttons |= bit;
                    } else {
                        self.buttons &= !bit;
                    }
                    user32::set_key(vkey, down);
                    self.post_mouse(if down { dn } else { up });
                    if bit == 2 {
                        self.set_relative(down);
                    }
                    if !down && self.buttons == 0 {
                        self.end_anchor();
                    }
                }
                SDL_EVENT_MOUSE_WHEEL => {
                    let lines = ev.wheel.y;
                    let wparam = ((((lines * 120.0) as i16) as u16 as u32) << 16) | self.flags();
                    let lparam = ((self.mouse.1 as u32 & 0xffff) << 16) | (self.mouse.0 as u32 & 0xffff);
                    user32::post_message(user32::main_window(), WM_MOUSEWHEEL, wparam, lparam);
                }
                SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP => {
                    let down = ev.key.down;
                    let scancode = ev.key.scancode.0 as u32;
                    if std::env::var_os("S4_INPUT_TRACE").is_some() {
                        eprintln!("key: down={down} scancode={scancode} keycode={:#x} mod={:#x} repeat={}", ev.key.key.0, ev.key.r#mod.0, ev.key.repeat);
                    }
                    if let Some(k) = keys::vk(scancode) {
                        user32::set_key(k, down);
                        match k {
                            0x10 => self.shift = down,
                            0x11 => self.ctrl = down,
                            0x12 => self.alt = down,
                            _ => {}
                        }
                        // Alt+Enter: full screen on and off (the original has no such key).
                        if down && !ev.key.repeat && k == 0x0d && self.alt {
                            self.fullscreen = !self.fullscreen;
                            SDL_SetWindowFullscreen(self.window, self.fullscreen);
                        }
                        let lparam = s4_host::dinput::key_lparam(k, down);
                        user32::post_message(user32::main_window(), if down { WM_KEYDOWN } else { WM_KEYUP }, k as u32, lparam);
                        // Windows' TranslateMessage makes a WM_CHAR of Escape, Enter, Backspace and Tab too; the game reads them there.
                        if down && !self.ctrl && !self.alt && matches!(k, 0x1b | 0x0d | 0x08 | 0x09) {
                            user32::post_message(user32::main_window(), WM_CHAR, k as u32, lparam);
                        }
                        if down && !self.ctrl && !self.alt {
                            self.pending_char = printable(ev.key.key.0, self.shift || (ev.key.r#mod.0 & SDL_KMOD_CAPS.0) != 0, self.shift);
                        }
                    }
                }
                SDL_EVENT_TEXT_INPUT => {
                    self.text_events = true;
                    self.pending_char = None;
                    let text = ev.text.text;
                    if !text.is_null() {
                        for ch in CStr::from_ptr(text).to_string_lossy().chars() {
                            if (ch as u32) < 256 {
                                user32::post_message(user32::main_window(), WM_CHAR, ch as u32, 1);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Shows the cursor the game selected (scaled with the picture), or hides it.
    fn update_cursor(&mut self) {
        let (serial, shown, image) = s4_host::cursor::current();
        if serial == self.cursor_serial {
            return;
        }
        self.cursor_serial = serial;
        unsafe {
            if !shown {
                SDL_HideCursor();
                return;
            }
            SDL_ShowCursor();
            let Some(c) = image else {
                SDL_SetCursor(null_mut());
                return;
            };
            // SDL sizes a cursor in window points: with a high-density display (macOS Retina) the picture scale in pixels is that much too big.
            let k = (self.scale() / SDL_GetWindowPixelDensity(self.window).max(1.0)).max(1.0);
            let (nw, nh) = (((c.w as f32 * k).round() as u32).clamp(1, 255), ((c.h as f32 * k).round() as u32).clamp(1, 255));
            let mut rgba = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    let (sx, sy) = (x * c.w / nw, y * c.h / nh);
                    let (s, d) = (((sy * c.w + sx) * 4) as usize, ((y * nw + x) * 4) as usize);
                    rgba[d..d + 4].copy_from_slice(&c.rgba[s..s + 4]);
                }
            }
            let surface = SDL_CreateSurfaceFrom(nw as c_int, nh as c_int, SDL_PIXELFORMAT_RGBA32, rgba.as_mut_ptr() as *mut c_void, (nw * 4) as c_int);
            if surface.is_null() {
                return;
            }
            let (hx, hy) = (((c.hot.0 as f32 * k) as c_int).min(nw as c_int - 1), ((c.hot.1 as f32 * k) as c_int).min(nh as c_int - 1));
            let cursor = SDL_CreateColorCursor(surface, hx, hy);
            SDL_DestroySurface(surface); // the cursor keeps its own copy
            if !cursor.is_null() {
                SDL_SetCursor(cursor);
                if !self.cursor.is_null() {
                    SDL_DestroyCursor(self.cursor);
                }
                self.cursor = cursor;
            }
        }
    }

    /// Some systems (an X session without an input method) never send text events: then the typed
    /// character is worked out from the key itself (US punctuation).
    fn tick_text(&mut self) {
        if let Some(c) = self.pending_char.take() {
            if !self.text_events {
                if std::env::var_os("S4_INPUT_TRACE").is_some() {
                    eprintln!("text fallback: {c:?}");
                }
                user32::post_message(user32::main_window(), WM_CHAR, c as u32, 1);
            }
        }
    }

    /// Scripted input: S4_AUTOCLICK, S4_AUTOKEY, S4_AUTODRAG, S4_FRAME_DUMP, S4_FPS.
    fn tick_scripts(&mut self) {
        let now = self.started.elapsed().as_secs_f32();
        if std::env::var_os("S4_WIN_TRACE").is_some() && (now as u32) != (now - 0.01) as u32 {
            eprintln!("t={now:.1} main window {:#x}", user32::main_window());
        }
        self.click_moved.resize(self.clicks.len(), false);
        for i in 0..self.clicks.len() {
            let (x, y, t, done) = self.clicks[i];
            if !self.click_moved[i] && !done && now >= t - 0.3 {
                self.click_moved[i] = true;
                self.mouse = (x, y);
                user32::set_cursor(x, y);
                self.post_mouse(WM_MOUSEMOVE);
            }
            let kind = self.click_kind.get(i).copied().unwrap_or('l');
            let (bit, vkey, dn, up) = if kind == 'r' { (2, 2, WM_RBUTTONDOWN, WM_RBUTTONUP) } else { (1, 1, WM_LBUTTONDOWN, WM_LBUTTONUP) };
            if !done && now >= t {
                self.clicks[i].3 = true;
                eprintln!("autoclick {kind} at ({x}, {y}): main window {:#x}", user32::main_window());
                self.mouse = (x, y);
                user32::set_cursor(x, y);
                self.post_mouse(WM_MOUSEMOVE);
                if kind != 'm' {
                    self.buttons = bit;
                    user32::set_key(vkey, true);
                    self.post_mouse(dn);
                }
            } else if done && now >= t + 0.25 && self.buttons == bit && kind != 'm' {
                self.buttons = 0;
                user32::set_key(vkey, false);
                self.post_mouse(up);
            }
        }
        for i in 0..self.keys.len() {
            let (k, t, state) = self.keys[i];
            let main = user32::main_window();
            if state == 0 && now >= t {
                self.keys[i].2 = 1;
                user32::set_key(k, true);
                if std::env::var_os("S4_HOST_TRACE").is_some() {
                    eprintln!("=== scripted key {k:#x} down");
                }
                user32::post_message(main, WM_KEYDOWN, k as u32, s4_host::dinput::key_lparam(k, true));
                if matches!(k, 0x30..=0x39 | 0x20 | 0x1b | 0x0d | 0x08 | 0x09) {
                    user32::post_message(main, WM_CHAR, k as u32, 1);
                } else if matches!(k, 0x41..=0x5a) {
                    user32::post_message(main, WM_CHAR, k as u32 + 32, 1);
                }
            } else if state == 1 && now >= t + 1.5 {
                self.keys[i].2 = 2;
                user32::set_key(k, false);
                user32::post_message(main, WM_KEYUP, k as u32, s4_host::dinput::key_lparam(k, false));
            }
        }
        if let (Ok(path), Ok(seq), Some(f)) = (std::env::var("S4_FRAME_DUMP"), std::env::var("S4_FRAME_SEQ"), &self.frame) {
            let times: Vec<f32> = seq.split(',').filter_map(|t| t.trim().parse().ok()).collect();
            self.seq_done.resize(times.len(), false);
            for (i, &t) in times.iter().enumerate() {
                if !self.seq_done[i] && now >= t {
                    self.seq_done[i] = true;
                    let mut out = format!("P6\n{} {}\n255\n", f.w, f.h).into_bytes();
                    for p in &f.pixels {
                        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
                    }
                    let _ = std::fs::write(format!("{path}.{i}.ppm"), out);
                }
            }
        }
        // S4_PEEK="hexaddr@secs;...": print the guest dword at each address (debugging: what is a flag at a point in time).
        if let Ok(spec) = std::env::var("S4_PEEK") {
            static DONE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
            for item in spec.split(';') {
                let Some((a, t)) = item.split_once('@') else { continue };
                let (Ok(addr), Ok(t)) = (u32::from_str_radix(a.trim_start_matches("0x"), 16), t.parse::<f32>()) else { continue };
                let mut done = DONE.lock().unwrap();
                if now >= t && !done.iter().any(|d| d == item) {
                    done.push(item.to_string());
                    if let Some(p) = s4_host::PROCESS.get() {
                        eprintln!("peek {addr:#x} at {t}s: {:#x}", p.mem.r32(addr));
                    }
                }
            }
        }
        // S4_DUMP="hexaddr:hexlen@secs:file;...": write guest memory to a file (debugging: diff the game's state around an action).
        if let Ok(spec) = std::env::var("S4_DUMP") {
            static DONE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
            for item in spec.split(';') {
                let Some((range, rest)) = item.split_once('@') else { continue };
                let Some((t, file)) = rest.split_once(':') else { continue };
                let Some((a, l)) = range.split_once(':') else { continue };
                let (Ok(addr), Ok(len), Ok(t)) = (u32::from_str_radix(a, 16), usize::from_str_radix(l, 16), t.parse::<f32>()) else { continue };
                let mut done = DONE.lock().unwrap();
                if now >= t && !done.iter().any(|d| d == item) {
                    done.push(item.to_string());
                    if let Some(p) = s4_host::PROCESS.get() {
                        let _ = std::fs::write(file, p.mem.read_bytes(addr, len));
                    }
                }
            }
        }
        if let (false, Ok(path), Some(f)) = (self.dumped, std::env::var("S4_FRAME_DUMP"), &self.frame) {
            if now >= std::env::var("S4_FRAME_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0) {
                self.dumped = true;
                let mut out = format!("P6\n{} {}\n255\n", f.w, f.h).into_bytes();
                for p in &f.pixels {
                    out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
                }
                let _ = std::fs::write(path, out);
            }
        }
        if std::env::var_os("S4_FPS").is_some() && self.stats.3.elapsed().as_secs_f32() >= 1.0 {
            let (n, t, g, _) = self.stats;
            let (mut w, mut h) = (0, 0);
            unsafe { SDL_GetWindowSizeInPixels(self.window, &mut w, &mut h) };
            // The game's own rate: frames it published (the window draws only the newest of those).
            let published = self.seen - self.seen_at_report;
            self.seen_at_report = self.seen;
            eprintln!("fps: {n} draws ({:.1} ms each) of {g} game frames, {published} published, window {w}x{h}", if n > 0 { t * 1000.0 / n as f64 } else { 0.0 });
            self.stats = (0, 0.0, 0, std::time::Instant::now());
        }
        for i in 0..self.drags.len() {
            let (x0, y0, x1, y1, t, k) = self.drags[i];
            let (bit, vkey, dn, up) = if k == 'r' || k == 'R' { (2, 2, WM_RBUTTONDOWN, WM_RBUTTONUP) } else { (1, 1, WM_LBUTTONDOWN, WM_LBUTTONUP) };
            let dt = now - t;
            if dt < 0.0 || dt > 2.6 {
                continue;
            }
            let f = (dt / 2.0).clamp(0.0, 1.0);
            let pos = (x0 + ((x1 - x0) as f32 * f) as i32, y0 + ((y1 - y0) as f32 * f) as i32);
            if self.buttons & bit == 0 && dt < 0.2 {
                if k == 'R' {
                    // the button goes down as the pointer moves (one read sees both)
                    user32::set_key(vkey, true);
                }
                self.mouse = (x0, y0);
                user32::set_cursor(x0, y0);
                self.post_mouse(WM_MOUSEMOVE);
                self.buttons |= bit;
                user32::set_key(vkey, true);
                self.post_mouse(dn);
            } else if self.buttons & bit != 0 && dt <= 2.0 && k == 'R' {
                // the grabbed pointer of a real right-drag: movement only as DirectInput deltas
                user32::add_mouse_delta(x1 - x0, y1 - y0);
                self.post_mouse(WM_MOUSEMOVE);
            } else if self.buttons & bit != 0 && dt <= 2.0 && pos != self.mouse {
                self.mouse = pos;
                user32::set_cursor(pos.0, pos.1);
                self.post_mouse(WM_MOUSEMOVE);
            } else if self.buttons & bit != 0 && dt > 2.2 {
                if k == 'R' {
                    // still moving as the button goes up (a read then sees the movement and the release together)
                    user32::add_mouse_delta(x1 - x0, y1 - y0);
                }
                self.buttons &= !bit;
                user32::set_key(vkey, false);
                self.post_mouse(up);
            }
        }
    }

    fn frame_waiting(&self) -> bool {
        present::newer_than(self.seen).is_some()
    }

    /// Takes the game's newest frame, if any, and draws when something changed.
    fn tick_video(&mut self) {
        if let Some((seq, f)) = present::newer_than(self.seen) {
            self.seen = seq;
            self.stats.2 += 1;
            if self.frame.is_none() {
                eprintln!("first frame after {:.1} s", self.started.elapsed().as_secs_f32());
            }
            self.frame = Some(f);
            self.redraw = true;
        }
        // Until the game's first picture (it takes a few seconds to start) show that the program is
        // alive: a Wayland window is only mapped once it has drawn something.
        if self.frame.is_none() {
            self.redraw = true;
        }
        if self.redraw {
            self.redraw = false;
            let t = std::time::Instant::now();
            // S4_NODRAW=1: do not show the picture (to tell the game's speed from the display's).
            if std::env::var_os("S4_NODRAW").is_none() {
                self.draw();
            }
            self.stats.0 += 1;
            self.stats.1 += t.elapsed().as_secs_f64();
        }
    }

    fn draw(&mut self) {
        unsafe {
            let Some(frame) = &self.frame else {
                SDL_SetRenderDrawColor(self.renderer, 0x10, 0x14, 0x18, 255);
                SDL_RenderClear(self.renderer);
                let t = self.started.elapsed().as_secs_f64();
                let (bw, x) = (200.0f32, (((t * 0.6).sin() * 0.5 + 0.5) * 600.0) as f32);
                SDL_SetRenderDrawColor(self.renderer, 0xc8, 0xa8, 0x50, 255);
                SDL_RenderFillRect(self.renderer, &SDL_FRect { x, y: 296.0, w: bw, h: 8.0 });
                SDL_RenderPresent(self.renderer);
                return;
            };
            if self.texture.is_null() || self.shown != (frame.w, frame.h) {
                if !self.texture.is_null() {
                    SDL_DestroyTexture(self.texture);
                }
                self.texture = SDL_CreateTexture(self.renderer, SDL_PIXELFORMAT_XRGB8888, SDL_TEXTUREACCESS_STREAMING, frame.w as c_int, frame.h as c_int);
                assert!(!self.texture.is_null(), "cannot create the screen texture: {}", sdl_error());
                self.scale.apply(self.texture);
                self.shown = (frame.w, frame.h);
                SDL_SetRenderLogicalPresentation(self.renderer, frame.w as c_int, frame.h as c_int, self.scale.presentation());
            }
            let t0 = std::time::Instant::now();
            SDL_UpdateTexture(self.texture, null(), frame.pixels.as_ptr() as *const c_void, (frame.w * 4) as c_int);
            let t1 = std::time::Instant::now();
            SDL_SetRenderDrawColor(self.renderer, 0, 0, 0, 255);
            SDL_RenderClear(self.renderer);
            SDL_RenderTexture(self.renderer, self.texture, null(), null());
            let t2 = std::time::Instant::now();
            SDL_RenderPresent(self.renderer);
            if std::env::var_os("S4_DRAW_TRACE").is_some() {
                eprintln!("draw: update {:.1} ms, render {:.1} ms, present {:.1} ms", (t1 - t0).as_secs_f64() * 1e3, (t2 - t1).as_secs_f64() * 1e3, t2.elapsed().as_secs_f64() * 1e3);
            }
        }
    }
}

/// How the game's picture is scaled to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scale {
    /// Sharp pixels at integer scales, smooth edges in between (SDL 3.4; nearest on older SDL).
    PixelArt,
    /// Smooth (bilinear).
    Linear,
    /// Hard pixels at any scale.
    Nearest,
    /// Hard pixels, only whole-number scales (black borders around).
    Integer,
}

impl Scale {
    fn parse(s: &str) -> Result<Scale, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "pixelart" | "sharp" => Ok(Scale::PixelArt),
            "linear" | "smooth" => Ok(Scale::Linear),
            "nearest" => Ok(Scale::Nearest),
            "integer" => Ok(Scale::Integer),
            o => Err(format!("unknown scale {o:?}: use pixelart, linear, nearest or integer")),
        }
    }

    fn presentation(self) -> SDL_RendererLogicalPresentation {
        if self == Scale::Integer { SDL_LOGICAL_PRESENTATION_INTEGER_SCALE } else { SDL_LOGICAL_PRESENTATION_LETTERBOX }
    }

    unsafe fn apply(self, texture: *mut SDL_Texture) {
        unsafe {
            match self {
                Scale::PixelArt => {
                    if !SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_PIXELART) {
                        SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_NEAREST);
                    }
                }
                Scale::Linear => {
                    SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_LINEAR);
                }
                Scale::Nearest | Scale::Integer => {
                    SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_NEAREST);
                }
            }
        }
    }
}

/// `S4_AUTOCLICK="x,y@seconds[:l|r|m];..."`: (positions and times, kinds).
fn parse_clicks() -> (Vec<(i32, i32, f32, bool)>, Vec<char>) {
    let mut clicks = Vec::new();
    let mut kinds = Vec::new();
    for c in std::env::var("S4_AUTOCLICK").unwrap_or_default().split(';') {
        let Some((xy, rest)) = c.split_once('@') else { continue };
        let Some((x, y)) = xy.split_once(',') else { continue };
        let (t, kind) = rest.split_once(':').unwrap_or((rest, "l"));
        if let (Ok(x), Ok(y), Ok(t)) = (x.trim().parse(), y.trim().parse(), t.trim().parse()) {
            clicks.push((x, y, t, false));
            kinds.push(kind.chars().next().unwrap_or('l'));
        }
    }
    (clicks, kinds)
}

fn main() {
    let mut game = None;
    let mut resolution = std::env::var("S3_RESOLUTION").ok();
    let mut scale = std::env::var("S4_SCALE").ok();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--scale" => scale = args.next(),
            "--resolution" => resolution = args.next(),
            "-h" | "--help" => {
                println!("usage: s3play [game folder] [--resolution auto|keep|640x480|800x600|1024x768] [--scale pixelart|linear|nearest|integer]");
                println!("  --resolution  the game's screen mode (default auto: the biggest that fits the desktop; keep: as the game has it)");
                std::process::exit(0);
            }
            _ if a.starts_with("--") => {
                eprintln!("unknown option {a} (see --help)");
                std::process::exit(2);
            }
            _ => game = Some(a),
        }
    }
    let scale = Scale::parse(scale.as_deref().unwrap_or("")).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let game = std::fs::canonicalize(game.unwrap_or_else(|| "extracted/s3".into())).expect("game folder");

    unsafe {
        SDL_SetHint(SDL_HINT_AUDIO_DEVICE_SAMPLE_FRAMES, c"512".as_ptr());
        SDL_SetHint(SDL_HINT_APP_NAME, c"The Settlers III (native)".as_ptr());
        assert!(SDL_Init(SDL_INIT_VIDEO | SDL_INIT_AUDIO | SDL_INIT_EVENTS), "SDL_Init failed: {}", sdl_error());
    }
    audio::register();
    let usable = unsafe {
        let mut r = SDL_Rect { x: 0, y: 0, w: 1920, h: 1080 };
        if !SDL_GetDisplayUsableBounds(SDL_GetPrimaryDisplay(), &mut r) {
            r = SDL_Rect { x: 0, y: 0, w: 1920, h: 1080 };
        }
        (r.w, r.h)
    };
    // The game's own screen modes (its `Resolution` setting: 0, 1, 2); the window fits the desktop.
    let modes = [(640, 480), (800, 600), (1024, 768)];
    let mode: Option<usize> = match resolution.as_deref().unwrap_or("auto") {
        "keep" => None,
        "auto" => Some(modes.iter().rposition(|&(w, h)| w <= usable.0 && h * 100 <= usable.1 * 95).unwrap_or(0)),
        r => match modes.iter().position(|&(w, h)| format!("{w}x{h}") == r) {
            Some(m) => Some(m),
            None => {
                eprintln!("unknown resolution {r}: use 640x480, 800x600, 1024x768, auto or keep");
                std::process::exit(2)
            }
        },
    };
    let (ww, wh) = mode.map_or((800, 600), |m| modes[m]);
    let k = (usable.0 as f64 * 0.9 / ww as f64).min(usable.1 as f64 * 0.9 / wh as f64).min(1.0);
    let mut app = App::new(((ww as f64 * k).round() as i32, (wh as f64 * k).round() as i32), scale);
    std::thread::Builder::new()
        .name("guest-main".into())
        .stack_size(256 << 20)
        .spawn(move || {
            let mut w = s4_host::World::new(&[lifted_s3main::dispatch, lifted_ir50::dispatch]);
            let data = std::env::var("XDG_DATA_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".local/share")).join("s3-native");
            s4_host::user32::activation_messages(true);
            // The game asks for a bold "MS Sans Serif" (a thin bitmap font): the regular weight of a TrueType face is closer.
            s4_host::text::use_regular_face();
            s4_host::fs::set_root("C:\\S3");
            s4_host::fs::init(&game, &data);
            s4_host::registry::enable(data.join("registry.txt"), &game, "C:\\S3");
            if let Some(m) = mode {
                s4_host::registry::set_dword("HKLM\\SOFTWARE\\Bluebyte\\Siedler3\\1.0\\General", "Resolution", m as u32);
            }
            let path = game.join("S3.EXE");
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            w.load("S3.EXE", &bytes);
            // The Indeo 5 codec of the game's movies.
            match std::fs::read(game.join("ir50_32.dll")) {
                Ok(b) => {
                    w.load("ir50_32.dll", &b);
                }
                Err(e) => eprintln!("ir50_32.dll: {e} (movies are skipped)"),
            }
            w.link();
            if std::env::var_os("S4_MISSING").is_some() {
                for m in w.missing() {
                    eprintln!("missing import: {m}");
                }
            }
            w.attach_all();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.run_program()));
            match r {
                Ok(code) => eprintln!("the game exited with {code}"),
                Err(p) => {
                    if let Some(j) = p.downcast_ref::<s4_x86::GuestJump>() {
                        eprintln!("a jump to {:#x} found no owner", j.target);
                    } else if let Some(s) = p.downcast_ref::<String>() {
                        eprintln!("the game stopped: {s}");
                    } else if let Some(s) = p.downcast_ref::<&str>() {
                        eprintln!("the game stopped: {s}");
                    }
                }
            }
            std::process::exit(0);
        })
        .expect("guest thread");
    let mut ev = std::mem::MaybeUninit::<SDL_Event>::zeroed();
    loop {
        unsafe {
            let wait = if app.frame_waiting() { 0 } else { 4 };
            if SDL_WaitEventTimeout(ev.as_mut_ptr(), wait) {
                app.handle(&mut *ev.as_mut_ptr());
                while SDL_PollEvent(ev.as_mut_ptr()) {
                    app.handle(&mut *ev.as_mut_ptr());
                }
            }
        }
        app.tick_text();
        app.tick_scripts();
        app.update_cursor();
        app.tick_video();
    }
}
