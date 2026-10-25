//! MCI "AVIVideo" and "waveaudio" for The Settlers III: the game plays its movies (Indeo 5 video, PCM sound in an
//! AVI file) and the voice-over of its briefings through `mciSendString`. A movie is decoded by the game's own
//! Indeo codec (the lifted `ir50_32.dll`, a Video for Windows driver called through its `DriverProc`) on a thread of
//! its own, shown scaled over the game's screen, and its sound goes to the mixer ([`crate::dsound`]).

use crate::{heap, shims::guest_call};
use s4_x86::Cpu;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const MCIERR_FILE_NOT_FOUND: u32 = 277;
const MCIERR_UNRECOGNIZED_COMMAND: u32 = 262;
const MM_MCINOTIFY: u32 = 0x3b9;

/// A parsed AVI file.
pub struct Avi {
    data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Microseconds per video frame.
    pub frame_us: u32,
    /// The stream's format (BITMAPINFOHEADER bytes) and its handler (fourcc of the codec).
    bih: Vec<u8>,
    /// The video stream's `strd` data (the codec's state, which the player hands to the driver).
    strd: Vec<u8>,
    /// (offset, size, key frame) of every video chunk.
    frames: Vec<(usize, usize, bool)>,
    /// Audio: 16-bit PCM samples, rate, channels (empty if the file has none or it is not PCM).
    pub pcm: Arc<Vec<i16>>,
    pub rate: u32,
    pub channels: u32,
}

fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn u16le(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(d[o..o + 2].try_into().unwrap())
}

impl Avi {
    pub fn parse(data: Vec<u8>) -> Option<Avi> {
        if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"AVI " {
            return None;
        }
        let mut avi = Avi { data: Vec::new(), width: 0, height: 0, frame_us: 40000, bih: Vec::new(), strd: Vec::new(), frames: Vec::new(), pcm: Arc::new(Vec::new()), rate: 22050, channels: 2 };
        let (mut video_stream, mut audio_stream) = (None, None);
        let mut stream_no = 0u32;
        let mut audio_fmt = (0u16, 0u32, 0u16);
        let mut audio_bytes: Vec<u8> = Vec::new();
        let mut stack: Vec<(usize, usize)> = vec![(12, data.len())];
        // Chunks and LISTs in file order (iteratively: a LIST is entered, not skipped).
        let mut cur_stream_is_video = false;
        while let Some(&(pos, end)) = stack.last() {
            if pos + 8 > end.min(data.len()) {
                stack.pop();
                continue;
            }
            let id = &data[pos..pos + 4];
            let size = u32le(&data, pos + 4) as usize;
            let body = pos + 8;
            let next = body + size + (size & 1);
            stack.last_mut().unwrap().0 = next;
            match id {
                b"LIST" => {
                    if body + 4 <= data.len() {
                        stack.push((body + 4, (body + size).min(data.len())));
                    }
                }
                b"avih" if size >= 40 => {
                    avi.frame_us = u32le(&data, body);
                    avi.width = u32le(&data, body + 32);
                    avi.height = u32le(&data, body + 36);
                }
                b"strh" if size >= 36 => {
                    let ty = &data[body..body + 4];
                    cur_stream_is_video = ty == b"vids";
                    if ty == b"vids" {
                        video_stream = Some(stream_no);
                        let (scale, rate) = (u32le(&data, body + 20), u32le(&data, body + 24));
                        if scale > 0 && rate > 0 {
                            avi.frame_us = (scale as u64 * 1_000_000 / rate as u64) as u32;
                        }
                    } else if ty == b"auds" {
                        audio_stream = Some(stream_no);
                    }
                    stream_no += 1;
                }
                b"strd" if cur_stream_is_video => {
                    avi.strd = data[body..(body + size).min(data.len())].to_vec();
                }
                b"strf" => {
                    if cur_stream_is_video && size >= 40 {
                        avi.bih = data[body..body + size.min(data.len() - body)].to_vec();
                        avi.width = u32le(&data, body + 4);
                        avi.height = u32le(&data, body + 8);
                    } else if !cur_stream_is_video && size >= 16 {
                        audio_fmt = (u16le(&data, body), u32le(&data, body + 4), u16le(&data, body + 14));
                        avi.channels = u16le(&data, body + 2) as u32;
                        avi.rate = audio_fmt.1;
                    }
                }
                _ if id.len() == 4 && id[0].is_ascii_digit() && id[1].is_ascii_digit() => {
                    let n = ((id[0] - b'0') * 10 + (id[1] - b'0')) as u32;
                    let kind = &id[2..4];
                    let body_end = (body + size).min(data.len());
                    if Some(n) == video_stream && (kind == b"dc" || kind == b"db") {
                        avi.frames.push((body, body_end - body, true));
                    } else if Some(n) == audio_stream && kind == b"wb" {
                        audio_bytes.extend_from_slice(&data[body..body_end]);
                    }
                }
                _ => {}
            }
        }
        // 16-bit PCM only (what the game's movies carry).
        if audio_fmt.0 == 1 && audio_fmt.2 == 16 {
            avi.pcm = Arc::new(audio_bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect());
        }
        avi.data = data;
        (avi.width > 0 && avi.height > 0 && !avi.frames.is_empty() && avi.bih.len() >= 40).then_some(avi)
    }
}

#[derive(Default)]
struct Device {
    avi: Option<Arc<Avi>>,
    wave: Option<(Arc<Vec<i16>>, u32, u32)>,
    hwnd: u32,
    stop: Option<Arc<AtomicBool>>,
    voice: Option<u32>,
    id: u32,
}

static DEVICES: Mutex<Option<HashMap<String, Device>>> = Mutex::new(None);
static NEXT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(10);

fn with<R>(f: impl FnOnce(&mut HashMap<String, Device>) -> R) -> R {
    f(DEVICES.lock().unwrap().get_or_insert_with(HashMap::new))
}

fn host_file(guest: &str) -> std::path::PathBuf {
    crate::fs::host_path(guest)
}

fn read_wav(path: &std::path::Path) -> Option<(Arc<Vec<i16>>, u32, u32)> {
    let d = std::fs::read(path).ok()?;
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return None;
    }
    let (mut rate, mut ch, mut bits) = (22050, 1, 16);
    let mut i = 12;
    while i + 8 <= d.len() {
        let size = u32le(&d, i + 4) as usize;
        let body = i + 8;
        match &d[i..i + 4] {
            b"fmt " if size >= 16 => {
                ch = u16le(&d, body + 2) as u32;
                rate = u32le(&d, body + 4);
                bits = u16le(&d, body + 14);
            }
            b"data" => {
                let end = (body + size).min(d.len());
                let pcm: Vec<i16> = match bits {
                    16 => d[body..end].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect(),
                    8 => d[body..end].iter().map(|&b| (b as i16 - 128) << 8).collect(),
                    _ => return None,
                };
                return Some((Arc::new(pcm), rate, ch));
            }
            _ => {}
        }
        i = body + size + (size & 1);
    }
    None
}

/// The text of the command after its first `n` words.
fn words(cmd: &str) -> Vec<&str> {
    cmd.split_whitespace().collect()
}

/// `mciSendString(command, return buffer, return size, callback window)`; `None` if the command is not for these devices.
pub fn send_string(c: &mut Cpu, cmd: &str, ret: u32, ret_len: u32, callback: u32) -> Option<u32> {
    let w = words(cmd);
    let verb = w.first()?.to_ascii_lowercase();
    let put_ret = |c: &Cpu, s: &str| {
        if ret != 0 && ret_len as usize > s.len() {
            c.mem.write_bytes(ret, s.as_bytes());
            c.mem.w8(ret + s.len() as u32, 0);
        }
    };
    if verb == "open" {
        let rest = cmd.trim_start()[4..].trim_start();
        let lower = rest.to_ascii_lowercase();
        let alias_at = lower.rfind(" alias ")?;
        let alias = rest[alias_at + 7..].trim().to_string();
        let target = rest[..alias_at].trim();
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        if let Some(file) = target.to_ascii_lowercase().strip_prefix("avivideo!").map(|_| &target[9..]) {
            let Ok(data) = std::fs::read(host_file(file.trim_matches('"'))) else { return Some(MCIERR_FILE_NOT_FOUND) };
            let Some(avi) = Avi::parse(data) else { return Some(MCIERR_FILE_NOT_FOUND) };
            with(|d| d.insert(alias.to_lowercase(), Device { avi: Some(Arc::new(avi)), id, ..Default::default() }));
            return Some(0);
        }
        // `open file.wav alias x` (the type follows from the extension) or `open file type waveaudio alias x`.
        let (file, typed) = match lower[..alias_at].find(" type ") {
            Some(t) => (rest[..t].trim().trim_matches('"'), true),
            None => (target.trim_matches('"'), false),
        };
        if typed || file.to_ascii_lowercase().ends_with(".wav") {
            let Some(wave) = read_wav(&host_file(file)) else { return Some(MCIERR_FILE_NOT_FOUND) };
            with(|d| d.insert(alias.to_lowercase(), Device { wave: Some(wave), id, ..Default::default() }));
            return Some(0);
        }
        return None;
    }
    let alias = w.get(1)?.to_lowercase();
    if !with(|d| d.contains_key(&alias)) {
        return None;
    }
    match verb.as_str() {
        "close" => {
            stop(&alias, true);
            with(|d| d.remove(&alias));
            Some(0)
        }
        "stop" => {
            stop(&alias, true);
            Some(0)
        }
        "window" => {
            if let Some(i) = w.iter().position(|x| x.eq_ignore_ascii_case("handle")) {
                if let Some(h) = w.get(i + 1).and_then(|h| h.parse::<u32>().ok()) {
                    with(|d| d.get_mut(&alias).map(|d| d.hwnd = h));
                }
            }
            Some(0)
        }
        "status" => {
            let what = w[2..].join(" ").to_ascii_lowercase();
            let (hwnd, playing, len_ms) = with(|d| {
                let dev = &d[&alias];
                let playing = dev.stop.as_ref().is_some_and(|s| !s.load(Ordering::SeqCst)) || dev.voice.is_some_and(|v| crate::dsound::voice_position(v).is_some());
                let len = match (&dev.avi, &dev.wave) {
                    (Some(a), _) => a.frames.len() as u64 * a.frame_us as u64 / 1000,
                    (_, Some((p, r, ch))) => p.len() as u64 / (*ch).max(1) as u64 * 1000 / (*r).max(1) as u64,
                    _ => 0,
                };
                (dev.hwnd, playing, len)
            });
            if what.contains("window handle") {
                put_ret(c, &hwnd.to_string());
            } else if what.starts_with("mode") {
                put_ret(c, if playing { "playing" } else { "stopped" });
            } else if what.starts_with("length") {
                put_ret(c, &len_ms.to_string());
            } else {
                put_ret(c, "0");
            }
            Some(0)
        }
        "play" => {
            let notify = w.iter().any(|x| x.eq_ignore_ascii_case("notify"));
            let wait = w.iter().any(|x| x.eq_ignore_ascii_case("wait"));
            stop(&alias, true);
            let done = play(&alias, if notify { callback } else { 0 });
            if wait {
                while !done.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            Some(0)
        }
        "put" | "set" | "seek" | "pause" | "resume" | "where" | "configure" => Some(0),
        _ => Some(MCIERR_UNRECOGNIZED_COMMAND),
    }
}

/// Stops a device's playback (a notification asked for is sent as aborted).
fn stop(alias: &str, abort: bool) {
    let (stop_flag, voice) = with(|d| d.get_mut(alias).map_or((None, None), |dev| (dev.stop.take(), dev.voice.take())));
    if let Some(s) = stop_flag {
        s.store(true, Ordering::SeqCst);
        let _ = abort;
    }
    if let Some(v) = voice {
        crate::dsound::stop_voice(v);
    }
}

/// Starts playing; the flag is set when playback has ended (or been stopped).
fn play(alias: &str, notify_window: u32) -> Arc<AtomicBool> {
    let done = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let (avi, wave, id) = with(|d| {
        let dev = d.get_mut(alias).unwrap();
        dev.stop = Some(stop_flag.clone());
        (dev.avi.clone(), dev.wave.clone(), dev.id)
    });
    let finish = {
        let done = done.clone();
        let stop_flag = stop_flag.clone();
        move |movie: bool| {
            if movie {
                crate::present::restore_game_frame();
            }
            let aborted = stop_flag.load(Ordering::SeqCst);
            done.store(true, Ordering::SeqCst);
            if notify_window != 0 {
                crate::user32::post_message(notify_window, MM_MCINOTIFY, if aborted { 4 } else { 1 }, id);
            }
        }
    };
    if let Some(avi) = avi {
        let alias = alias.to_string();
        let sf = stop_flag.clone();
        std::thread::Builder::new()
            .name("avi-player".into())
            .stack_size(32 << 20)
            .spawn(move || {
                let voice = (!avi.pcm.is_empty()).then(|| crate::dsound::play_pcm(avi.pcm.clone(), avi.rate, avi.channels));
                with(|d| d.get_mut(&alias).map(|dev| dev.voice = voice));
                play_video(&avi, &sf, voice);
                if let Some(v) = voice {
                    crate::dsound::stop_voice(v);
                }
                finish(true);
            })
            .expect("video thread");
    } else if let Some((pcm, rate, ch)) = wave {
        let voice = crate::dsound::play_pcm(pcm, rate, ch);
        with(|d| d.get_mut(alias).map(|dev| dev.voice = Some(voice)));
        std::thread::Builder::new()
            .name("wave-player".into())
            .spawn(move || {
                while crate::dsound::voice_position(voice).is_some() && !stop_flag.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                finish(false);
            })
            .expect("wave thread");
    } else {
        done.store(true, Ordering::SeqCst);
    }
    done
}

/// Decodes and shows the frames of a movie, in time.
fn play_video(avi: &Avi, stop: &AtomicBool, voice: Option<u32>) {
    let module = crate::module_base("ir50_32");
    let driver = crate::proc_address(module, "DriverProc");
    if driver == 0 {
        eprintln!("avi: the Indeo codec (ir50_32.dll) is not loaded; the movie is skipped");
        return;
    }
    crate::kernel32::register_host_thread();
    let mut c = crate::new_cpu(0);
    let stack = heap::vm_alloc(&mut c, 1 << 20);
    c.esp = stack + (1 << 20) - 64;
    let call = |c: &mut Cpu, id: u32, msg: u32, l1: u32, l2: u32| guest_call(c, driver, &[id, 0, msg, l1, l2]);
    // DRV_LOAD, DRV_ENABLE, DRV_OPEN with ICOPEN {size, 'vidc', handler, version, flags (ICMODE_DECOMPRESS), ...}
    call(&mut c, 0, 1, 0, 0);
    call(&mut c, 0, 2, 0, 0);
    let icopen = heap::alloc_zeroed(&mut c, 48);
    let handler = u32le(&avi.bih, 16);
    for (i, v) in [32, u32::from_le_bytes(*b"vidc"), handler, 0, 2].iter().enumerate() {
        c.mem.w32(icopen + 4 * i as u32, *v);
    }
    let inst = call(&mut c, 0, 3, 0, icopen);
    if inst == 0 {
        eprintln!("avi: the Indeo codec did not open");
        return;
    }
    let (w, h) = (avi.width, avi.height);
    let bih_in = heap::alloc_zeroed(&mut c, 64);
    c.mem.write_bytes(bih_in, &avi.bih[..40.min(avi.bih.len())]);
    let bih_out = heap::alloc_zeroed(&mut c, 64);
    for (i, v) in [40, w, h, 1 | 24 << 16, 0, w * h * 3].iter().enumerate() {
        c.mem.w32(bih_out + 4 * i as u32, *v);
    }
    let out_buf = heap::alloc_zeroed(&mut c, w * h * 4 + 64);
    let max_in = avi.frames.iter().map(|f| f.1).max().unwrap_or(0) as u32;
    let in_buf = heap::alloc_zeroed(&mut c, max_in + 64);
    if !avi.strd.is_empty() {
        // ICM_SETSTATE with the stream's driver data, as the AVI players do.
        let st = heap::alloc_zeroed(&mut c, avi.strd.len() as u32 + 16);
        c.mem.write_bytes(st, &avi.strd);
        let r = call(&mut c, inst, 0x5001, st, avi.strd.len() as u32);
        if std::env::var_os("S4_MCI_TRACE").is_some() {
            eprintln!("avi: ICM_SETSTATE({} bytes) returned {r:#x}", avi.strd.len());
        }
    }
    let q = call(&mut c, inst, 0x400b, bih_in, bih_out);
    let b = call(&mut c, inst, 0x400c, bih_in, bih_out);
    if std::env::var_os("S4_MCI_TRACE").is_some() {
        eprintln!("avi: {w}x{h}, {} frames, {} us each, codec query {q:#x} begin {b:#x}", avi.frames.len(), avi.frame_us);
    }
    let (sw, sh) = crate::present::game_frame_size();
    let start = std::time::Instant::now();
    let icd = heap::alloc_zeroed(&mut c, 32);
    for (n, &(off, size, key)) in avi.frames.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let due = std::time::Duration::from_micros(n as u64 * avi.frame_us as u64);
        c.mem.write_bytes(in_buf, &avi.data[off..off + size]);
        c.mem.w32(bih_in + 20, size as u32);
        // ICDECOMPRESS {flags, lpbiInput, lpInput, lpbiOutput, lpOutput, ckid}
        let flags = if key && n == 0 { 0 } else { 0 };
        for (i, v) in [flags, bih_in, in_buf, bih_out, out_buf, 0].iter().enumerate() {
            c.mem.w32(icd + 4 * i as u32, *v);
        }
        let r = call(&mut c, inst, 0x400d, icd, 24);
        if r != 0 && std::env::var_os("S4_MCI_TRACE").is_some() && n < 3 {
            eprintln!("avi: frame {n} ({size} bytes, key {key}: {:02x?}) decode returned {r:#x}", &avi.data[off..off + 24.min(size)]);
        }
        if std::env::var_os("S4_MCI_TRACE").is_some() && (n < 3 || n % 250 == 0) {
            let dib = c.mem.view(out_buf, (w * h * 3) as usize);
            eprintln!("avi: frame {n} decode returned {r:#x}, {} of {} output bytes are not zero", dib.iter().filter(|&&b| b != 0).count(), dib.len());
        }
        // Behind schedule: skip showing (the decoder still has to see every frame); the audio sets the pace.
        let late = start.elapsed() > due + std::time::Duration::from_micros(avi.frame_us as u64 * 2);
        if !late {
            let dib = c.mem.view(out_buf, (w * h * 3) as usize);
            crate::present::publish_overlay(scale_frame(dib, w, h, sw, sh));
        }
        let now = start.elapsed();
        if due > now {
            std::thread::sleep(due - now);
        }
        if let Some(v) = voice {
            if crate::dsound::voice_position(v).is_none() && n + 1 >= avi.frames.len() {
                break;
            }
        }
    }
    call(&mut c, inst, 0x400e, 0, 0);
    call(&mut c, inst, 4, 0, 0);
    let _ = crate::dsound::stop_voice;
}

/// A bottom-up 24-bit DIB shown letter-boxed in a `sw` x `sh` frame.
fn scale_frame(dib: &[u8], w: u32, h: u32, sw: u32, sh: u32) -> crate::present::Frame {
    let scale = (sw as f64 / w as f64).min(sh as f64 / h as f64);
    let (dw, dh) = (((w as f64 * scale) as u32).min(sw), ((h as f64 * scale) as u32).min(sh));
    let (ox, oy) = ((sw - dw) / 2, (sh - dh) / 2);
    let mut pixels = vec![0u32; (sw * sh) as usize];
    for y in 0..dh {
        let sy = ((y as f64 / scale) as u32).min(h - 1);
        let row = ((h - 1 - sy) * w * 3) as usize;
        for x in 0..dw {
            let sx = ((x as f64 / scale) as u32).min(w - 1) as usize;
            let p = &dib[row + sx * 3..row + sx * 3 + 3];
            pixels[((y + oy) * sw + x + ox) as usize] = (p[2] as u32) << 16 | (p[1] as u32) << 8 | p[0] as u32;
        }
    }
    crate::present::Frame { w: sw, h: sh, pixels }
}
