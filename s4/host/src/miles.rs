//! The Miles Sound System (mss32) as the game's SoundEngine uses it: digital
//! samples (WAV: PCM or IMA ADPCM from the sound bank) and MP3 streams for the
//! music, mixed here and played through PipeWire/ALSA's command line players.

use crate::heap;
use crate::kernel32::cstr;
use crate::{fs, Shim};
use s4_formats_clean::sound::{decode_wav, Pcm};
use s4_x86::Cpu;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::Mutex;

macro_rules! mss {
    ($name:expr, $args:expr, $f:expr) => {
        Shim { dll: "mss32.dll", name: $name, args: $args, cdecl: false, f: $f }
    };
}

const FREE: u32 = 1;
const DONE: u32 = 2;
const PLAYING: u32 = 4;
const STOPPED: u32 = 8;

struct Voice {
    pcm: Option<Pcm>,
    rate: u32,
    volume: u32,
    pan: u32,
    status: u32,
    /// Position in input frames.
    pos: f64,
    callback: u32,
    stream: bool,
    /// Double-buffered sample (Bink's audio): two PCM buffers the game refills.
    buffered: bool,
    bufs: [Vec<i16>; 2],
    /// The guest buffers (address, bytes) are read when played, not when loaded: the game may still be writing.
    src: [(u32, usize); 2],
    mem: Option<s4_x86::Mem>,
    loaded: [bool; 2],
    cur: usize,
    channels: usize,
    fmt16: bool,
    eob: u32,
    user: [u32; 8],
}

impl Voice {
    fn new(stream: bool) -> Voice {
        Voice { pcm: None, rate: 22050, volume: 127, pan: 64, status: DONE, pos: 0.0, callback: 0, stream, buffered: false, bufs: [Vec::new(), Vec::new()], src: [(0, 0); 2], mem: None, loaded: [false; 2], cur: 0, channels: 2, fmt16: true, eob: 0, user: [0; 8] }
    }
}

struct Mixer {
    rate: u32,
    voices: HashMap<u32, Voice>,
    next: u32,
    max_samples: usize,
    callbacks: VecDeque<(u32, u32)>,
    output: Option<Box<dyn std::io::Write + Send>>,
    started: bool,
}

static MIXER: Mutex<Option<Mixer>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Mixer) -> R) -> R {
    let mut g = MIXER.lock().unwrap();
    let m = g.get_or_insert_with(|| Mixer { rate: 22050, voices: HashMap::new(), next: 0x7001_0000, max_samples: 32, callbacks: VecDeque::new(), output: None, started: false });
    f(m)
}

fn decode_mp3(bytes: Vec<u8>) -> Option<Pcm> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()).ok()?;
    let mut format = probed.format;
    let track = format.default_track()?.clone();
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).ok()?;
    let (mut samples, mut rate, mut channels) = (Vec::new(), 44100, 2u16);
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track.id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else { continue };
        let spec = *decoded.spec();
        rate = spec.rate;
        channels = spec.channels.count() as u16;
        let mut buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        samples.extend_from_slice(buf.samples());
    }
    Some(Pcm { channels, rate, samples })
}

/// Mixes `frames` stereo frames at the mixer's rate.
fn mix(m: &mut Mixer, frames: usize) -> Vec<i16> {
    let mut acc = vec![0i32; frames * 2];
    let out_rate = m.rate as f64;
    for (&h, v) in m.voices.iter_mut() {
        if v.status != PLAYING {
            continue;
        }
        if v.buffered {
            if std::env::var_os("S4_MIX_TRACE2").is_some() {
                eprintln!("mix buffered {h:#x}: cur {} loaded {:?} pos {:.0} src {:?}", v.cur, v.loaded, v.pos, v.src);
            }
            let step = v.rate as f64 / out_rate;
            let gain = v.volume as f32 / 127.0;
            let (gl, gr) = ((127 - v.pan.min(127)) as f32 / 64.0, v.pan.min(127) as f32 / 64.0);
            let (gl, gr) = (gl.min(1.0) * gain, gr.min(1.0) * gain);
            let ch = v.channels;
            for i in 0..frames {
                if !v.loaded[v.cur] {
                    break; // underrun: silence until the game refills
                }
                let (base, bytes) = v.src[v.cur];
                let Some(mem) = v.mem else { break };
                let sample_bytes = if v.fmt16 { 2 } else { 1 };
                let total = bytes / (ch * sample_bytes);
                let rd = |i: usize, c: usize| -> f32 {
                    let a = base + ((i * ch + c) * sample_bytes) as u32;
                    if v.fmt16 { mem.r16(a) as i16 as f32 } else { ((mem.r8(a) as i16 - 128) << 8) as f32 }
                };
                let p = v.pos as usize;
                if p >= total {
                    if std::env::var_os("S4_MIX_TRACE").is_some() {
                        eprintln!("[{:.2}] consumed buffer {} (frames {total}, step {step})", crate::shims::micros() as f64 / 1e6, v.cur);
                    }
                    if let Some(path) = std::env::var_os("S4_BINK_DUMP") {
                        use std::io::Write;
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                            let _ = f.write_all(&mem.read_bytes(base, bytes));
                        }
                    }
                    v.loaded[v.cur] = false;
                    v.pos -= total as f64;
                    v.cur ^= 1;
                    if v.eob != 0 {
                        m.callbacks.push_back((v.eob, h));
                    }
                    continue;
                }
                // Box filter over the input frames this output frame covers (aliasing when
                // downsampling 44.1 kHz video sound to the mixer rate); linear when upsampling.
                let (l, r) = if step > 1.0 {
                    let n = (step.ceil() as usize).max(1);
                    let (mut sl, mut sr, mut k) = (0f32, 0f32, 0f32);
                    for j in 0..n {
                        let q = p + j;
                        if q >= total {
                            break;
                        }
                        let (a, c) = (rd(q, 0), rd(q, 1.min(ch - 1)));
                        sl += a;
                        sr += c;
                        k += 1.0;
                    }
                    (sl / k.max(1.0), sr / k.max(1.0))
                } else {
                    let f = (v.pos - p as f64) as f32;
                    let q = (p + 1).min(total - 1);
                    (rd(p, 0) * (1.0 - f) + rd(q, 0) * f, rd(p, 1.min(ch - 1)) * (1.0 - f) + rd(q, 1.min(ch - 1)) * f)
                };
                acc[2 * i] += (l * gl) as i32;
                acc[2 * i + 1] += (r * gr) as i32;
                v.pos += step;
            }
            continue;
        }
        let Some(pcm) = &v.pcm else { continue };
        let ch = pcm.channels.max(1) as usize;
        let total = pcm.samples.len() / ch;
        let step = v.rate as f64 / out_rate;
        let gain = v.volume as f32 / 127.0;
        let (gl, gr) = ((127 - v.pan.min(127)) as f32 / 64.0, v.pan.min(127) as f32 / 64.0);
        let (gl, gr) = (gl.min(1.0) * gain, gr.min(1.0) * gain);
        for i in 0..frames {
            let p = v.pos as usize;
            if p >= total {
                v.status = DONE;
                if v.callback != 0 {
                    m.callbacks.push_back((v.callback, h));
                }
                break;
            }
            let (l, r) = if ch == 1 { (pcm.samples[p] as f32, pcm.samples[p] as f32) } else { (pcm.samples[p * ch] as f32, pcm.samples[p * ch + 1] as f32) };
            acc[2 * i] += (l * gl) as i32;
            acc[2 * i + 1] += (r * gr) as i32;
            v.pos += step;
        }
    }
    acc.into_iter().map(|x| x.clamp(-32768, 32767) as i16).collect()
}

/// Where the mixed sound goes: the front end registers a function that opens a sink for a sample rate
/// (interleaved signed 16-bit stereo). Its `write` should block while the device has enough queued: that
/// is what paces the mixer thread and keeps the delay between a game event and its sound short.
pub type OutputFactory = fn(rate: u32) -> Option<Box<dyn Write + Send>>;

static OUTPUT_FACTORY: Mutex<Option<OutputFactory>> = Mutex::new(None);

pub fn set_output_factory(f: OutputFactory) {
    *OUTPUT_FACTORY.lock().unwrap() = Some(f);
}

pub(crate) fn open_output(rate: u32) -> Option<Box<dyn Write + Send>> {
    if std::env::var_os("S4_NO_AUDIO").is_some() {
        return None;
    }
    if let Some(path) = std::env::var_os("S4_AUDIO_DUMP") {
        // Raw S16 stereo, for checking the mixer's output without a speaker.
        return std::fs::File::create(path).ok().map(|f| Box::new(f) as Box<dyn Write + Send>);
    }
    let factory = *OUTPUT_FACTORY.lock().unwrap();
    match factory.and_then(|f| f(rate)) {
        Some(o) => Some(o),
        None => {
            eprintln!("audio: no output device; the game runs silently");
            None
        }
    }
}

fn start_thread() {
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(|| {
            let started = std::time::Instant::now();
            let mut produced: u64 = 0;
            loop {
                let (rate, pcm) = {
                    let mut g = MIXER.lock().unwrap();
                    let m = g.as_mut().unwrap();
                    if m.output.is_none() && !m.started {
                        m.output = open_output(m.rate);
                        m.started = true;
                    }
                    let frames = (m.rate / 50) as usize;
                    (m.rate, mix(m, frames))
                };
                produced += pcm.len() as u64 / 2;
                let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
                // The sink blocks when its buffer is full (that paces this thread), so write
                // without holding the mixer lock: the game calls into the mixer constantly.
                let out = MIXER.lock().unwrap().as_mut().unwrap().output.take();
                let (wrote, out) = match out {
                    Some(mut o) => {
                        let ok = o.write_all(&bytes).is_ok();
                        (ok, ok.then_some(o))
                    }
                    None => (false, None),
                };
                MIXER.lock().unwrap().as_mut().unwrap().output = out;
                if !wrote || std::env::var_os("S4_AUDIO_DUMP").is_some() {
                    // No sink: keep time by the clock so statuses still advance.
                    let due = std::time::Duration::from_secs_f64(produced as f64 / rate as f64);
                    if let Some(wait) = due.checked_sub(started.elapsed()) {
                        std::thread::sleep(wait);
                    }
                }
            }
        })
        .expect("audio thread");
}

fn run_callbacks(c: &mut Cpu) {
    loop {
        let next = with(|m| m.callbacks.pop_front());
        let Some((cb, arg)) = next else { return };
        crate::shims::guest_call(c, cb, &[arg]);
    }
}

fn new_voice(stream: bool) -> u32 {
    with(|m| {
        let h = m.next;
        m.next += 16;
        m.voices.insert(h, Voice::new(stream));
        h
    })
}

fn voice<R>(h: u32, f: impl FnOnce(&mut Voice) -> R) -> Option<R> {
    with(|m| m.voices.get_mut(&h).map(f))
}

fn read_host_file(c: &Cpu, name_ptr: u32) -> Option<Vec<u8>> {
    let name = cstr(c, name_ptr);
    std::fs::read(fs::host_path(&name)).ok()
}

pub fn all() -> Vec<Shim> {
    vec![
        mss!("_AIL_startup@0", 0, |_, _| 1),
        mss!("_AIL_shutdown@0", 0, |_, _| 0),
        mss!("_AIL_set_preference@8", 2, |_, a| {
            if a[0] == 1 {
                with(|m| m.max_samples = a[1] as usize);
            }
            0
        }),
        mss!("_AIL_set_redist_directory@4", 1, |_, _| 0),
        mss!("_AIL_waveOutOpen@16", 4, |c, a| {
            // (HDIGDRIVER *, HWAVEOUT *, device, WAVEFORMAT *): a mixer at the requested rate.
            let rate = if a[3] != 0 { c.mem.r32(a[3] + 4) } else { 0 };
            let rate = if (8000..=48000).contains(&rate) { rate } else { 22050 };
            let first = with(|m| {
                let first = !m.started && m.output.is_none() && m.rate == 22050 && m.voices.is_empty();
                m.rate = rate;
                first
            });
            if first {
                start_thread();
            }
            c.mem.w32(a[0], 0x6001);
            0
        }),
        mss!("_AIL_digital_handle_release@4", 1, |_, _| 0),
        mss!("_AIL_digital_handle_reacquire@4", 1, |_, _| 1),
        mss!("_AIL_digital_CPU_percent@4", 1, |_, _| 0),
        mss!("_AIL_allocate_sample_handle@4", 1, |_, _| {
            let used = with(|m| m.voices.values().filter(|v| !v.stream).count());
            if used >= with(|m| m.max_samples) { 0 } else { new_voice(false) }
        }),
        mss!("_AIL_release_sample_handle@4", 1, |_, a| {
            with(|m| m.voices.remove(&a[0]));
            0
        }),
        mss!("_AIL_init_sample@4", 1, |_, a| {
            voice(a[0], |v| *v = Voice { callback: 0, ..Voice::new(false) });
            0
        }),
        mss!("_AIL_set_sample_type@12", 3, |_, a| {
            // (S, DIG_F_ format: 0 mono 8, 1 mono 16, 2 stereo 8, 3 stereo 16, flags): the game feeds buffers.
            voice(a[0], |v| {
                v.buffered = true;
                v.channels = if a[1] >= 2 { 2 } else { 1 };
                v.fmt16 = a[1] & 1 != 0;
                v.status = STOPPED;
                v.pos = 0.0;
            });
            0
        }),
        mss!("_AIL_set_named_sample_file@20", 5, |c, a| {
            // (S, name, image, size, block)
            let image = c.mem.read_bytes(a[2], a[3] as usize);
            match decode_wav(&image) {
                Ok(pcm) => {
                    voice(a[0], |v| {
                        v.rate = pcm.rate;
                        v.pcm = Some(pcm);
                        v.status = STOPPED;
                        v.pos = 0.0;
                    });
                    1
                }
                Err(_) => 0,
            }
        }),
        mss!("_AIL_set_sample_volume@8", 2, |_, a| {
            voice(a[0], |v| v.volume = a[1].min(127));
            0
        }),
        mss!("_AIL_set_sample_pan@8", 2, |_, a| {
            voice(a[0], |v| v.pan = a[1].min(127));
            0
        }),
        mss!("_AIL_sample_playback_rate@4", 1, |_, a| voice(a[0], |v| v.rate).unwrap_or(0)),
        mss!("_AIL_set_sample_playback_rate@8", 2, |_, a| {
            voice(a[0], |v| v.rate = a[1].max(1));
            0
        }),
        mss!("_AIL_start_sample@4", 1, |_, a| {
            voice(a[0], |v| {
                if v.pcm.is_some() || v.buffered {
                    v.pos = 0.0;
                    v.status = PLAYING;
                }
            });
            0
        }),
        mss!("_AIL_stop_sample@4", 1, |c, a| {
            if std::env::var_os("S4_MIX_TRACE").is_some() {
                let st: Vec<String> = (0..40).map(|i| c.mem.r32(c.esp + 4 * i)).filter(|v| (0x3000_0000..0x3010_0000).contains(v) || (0x1840_0000..0x1950_0000).contains(v)).map(|v| format!("{v:#x}")).collect();
                eprintln!("stop_sample stack: {}", st.join(" "));
            }
            voice(a[0], |v| {
                if v.status == PLAYING {
                    v.status = STOPPED;
                }
            });
            0
        }),
        mss!("_AIL_resume_sample@4", 1, |_, a| {
            voice(a[0], |v| {
                if v.status == STOPPED && (v.pcm.is_some() || v.buffered) {
                    v.status = PLAYING;
                }
            });
            0
        }),
        mss!("_AIL_end_sample@4", 1, |_, a| {
            voice(a[0], |v| v.status = DONE);
            0
        }),
        mss!("_AIL_sample_status@4", 1, |c, a| {
            run_callbacks(c);
            voice(a[0], |v| v.status).unwrap_or(FREE)
        }),
        mss!("_AIL_register_EOS_callback@8", 2, |_, a| {
            voice(a[0], |v| v.callback = a[1]);
            0
        }),
        mss!("_AIL_sample_buffer_ready@4", 1, |c, a| {
            run_callbacks(c);
            voice(a[0], |v| (0..2).find(|&i| !v.loaded[i]).map(|i| i as u32).unwrap_or(0xffff_ffff)).unwrap_or(0xffff_ffff)
        }),
        mss!("_AIL_load_sample_buffer@16", 4, |c, a| {
            // (S, buffer 0 or 1, data, length in bytes)
            let bytes = c.mem.read_bytes(a[2], a[3] as usize);
            if std::env::var_os("S4_MIX_TRACE").is_some() {
                eprintln!("[{:.2}] load buffer {} len {} nonzero {}", crate::shims::micros() as f64 / 1e6, a[1], a[3], bytes.iter().filter(|&&b| b != 0).count());
                if bytes.iter().any(|&b| b != 0) {
                    let v: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                    let first = v.iter().position(|&x| x != 0).unwrap_or(0);
                    eprintln!("  first nonzero sample {first}: {:?}", &v[first..(first + 24).min(v.len())]);
                }
            }
            let mem = c.mem;
            voice(a[0], |v| {
                let n = (a[1] & 1) as usize;
                v.src[n] = (a[2], a[3] as usize);
                v.mem = Some(mem);
                v.loaded[n] = true;
                // Bink never calls start: the sample plays once both buffers are filled.
                if v.status == STOPPED && v.loaded == [true, true] {
                    v.status = PLAYING;
                }
            });
            0
        }),
        mss!("_AIL_minimum_sample_buffer_size@12", 3, |_, a| {
            // (driver, rate, format): about 100 ms of audio.
            let frame = [1u32, 2, 2, 4][(a[2] & 3) as usize];
            ((a[1] / 10) * frame + 15) & !15
        }),
        mss!("_AIL_sample_buffer_info@20", 5, |c, a| {
            // (S, pos0*, len0*, pos1*, len1*): buffer positions.
            for p in &a[1..5] {
                if *p != 0 {
                    c.mem.w32(*p, 0);
                }
            }
            // Nonzero would make Bink restart its audio at every frame (it treats it as 'just started').
            0
        }),
        mss!("_AIL_register_EOB_callback@8", 2, |_, a| {
            voice(a[0], |v| std::mem::replace(&mut v.eob, a[1])).unwrap_or(0)
        }),
        mss!("_AIL_set_sample_user_data@12", 3, |_, a| {
            voice(a[0], |v| v.user[(a[1] & 7) as usize] = a[2]);
            0
        }),
        mss!("_AIL_sample_user_data@8", 2, |_, a| voice(a[0], |v| v.user[(a[1] & 7) as usize]).unwrap_or(0)),
        mss!("_AIL_mem_alloc_lock@4", 1, |c, a| heap::alloc(c, a[0] + 16)),
        mss!("_AIL_digital_latency@4", 1, |_, _| 50),
        mss!("_AIL_get_preference@4", 1, |_, _| 0),
        mss!("_AIL_serve@0", 0, |c, _| {
            run_callbacks(c);
            0
        }),
        mss!("_AIL_file_size@4", 1, |c, a| read_host_file(c, a[0]).map(|d| d.len() as u32).unwrap_or(0)),
        mss!("_AIL_file_read@8", 2, |c, a| match read_host_file(c, a[0]) {
            Some(d) => {
                let p = heap::alloc(c, d.len() as u32 + 16);
                c.mem.write_bytes(p, &d);
                p
            }
            None => 0,
        }),
        mss!("_AIL_mem_free_lock@4", 1, |c, a| {
            heap::free(c, a[0]);
            0
        }),
        mss!("_AIL_open_stream@12", 3, |c, a| {
            // (driver, filename, memory?): decode the MP3 now.
            let Some(bytes) = read_host_file(c, a[1]) else { return 0 };
            let Some(pcm) = decode_mp3(bytes) else { return 0 };
            let h = new_voice(true);
            voice(h, |v| {
                v.rate = pcm.rate;
                v.pcm = Some(pcm);
                v.status = STOPPED;
            });
            h
        }),
        mss!("_AIL_close_stream@4", 1, |_, a| {
            with(|m| m.voices.remove(&a[0]));
            0
        }),
        mss!("_AIL_start_stream@4", 1, |_, a| {
            voice(a[0], |v| {
                v.pos = 0.0;
                v.status = PLAYING;
            });
            0
        }),
        mss!("_AIL_pause_stream@8", 2, |_, a| {
            voice(a[0], |v| {
                if a[1] != 0 {
                    if v.status == PLAYING {
                        v.status = STOPPED;
                    }
                } else if v.status == STOPPED {
                    v.status = PLAYING;
                }
            });
            0
        }),
        mss!("_AIL_stream_status@4", 1, |c, a| {
            run_callbacks(c);
            voice(a[0], |v| v.status).unwrap_or(FREE)
        }),
        mss!("_AIL_stream_position@4", 1, |_, a| voice(a[0], |v| v.pos as u32).unwrap_or(0)),
        mss!("_AIL_set_stream_position@8", 2, |_, a| {
            voice(a[0], |v| v.pos = a[1] as f64);
            0
        }),
        mss!("_AIL_set_stream_volume@8", 2, |_, a| {
            voice(a[0], |v| v.volume = a[1].min(127));
            0
        }),
        mss!("_AIL_set_stream_pan@8", 2, |_, a| {
            voice(a[0], |v| v.pan = a[1].min(127));
            0
        }),
        mss!("_AIL_stream_info@20", 5, |c, a| {
            // (stream, *datarate, *sndtype, *length, *memory)
            let (rate, len) = voice(a[0], |v| (v.rate, v.pcm.as_ref().map(|p| p.samples.len() as u32 * 2).unwrap_or(0))).unwrap_or((0, 0));
            for (p, val) in [(a[1], rate * 4), (a[2], 0), (a[3], len), (a[4], 0)] {
                if p != 0 {
                    c.mem.w32(p, val);
                }
            }
            0
        }),
        mss!("_AIL_register_stream_callback@8", 2, |_, a| {
            voice(a[0], |v| v.callback = a[1]);
            0
        }),
        mss!("_AIL_open_input@4", 1, |_, _| 0),
        mss!("_AIL_close_input@4", 1, |_, _| 0),
        mss!("_AIL_set_input_state@8", 2, |_, _| 0),
        mss!("_AIL_process_digital_audio@24", 6, |_, _| 0),
        mss!("_AIL_size_processed_digital_audio@16", 4, |_, _| 0),
        mss!("RIB_find_file_provider", 4, |_, _| 0),
        mss!("RIB_request_interface", 4, |_, _| 0),
    ]
}
