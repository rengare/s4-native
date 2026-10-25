//! DirectSound for the games that use it directly (The Settlers III). Buffers live in guest memory so `Lock`
//! and `Unlock` work as the game expects. A mixer thread plays every playing buffer (volume, pan, frequency,
//! looping) into the output the front end registered for [`crate::miles`]; the play cursors are the mixer's
//! positions. Without an output device the mixer still runs on the clock, so the game's streaming works.

use crate::ddraw::{add_ref, make_vtable, new_object, release};
use crate::heap;
use s4_x86::Cpu;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;

const DS: &[&str] = &["QueryInterface", "AddRef", "Release", "CreateSoundBuffer", "GetCaps", "DuplicateSoundBuffer", "SetCooperativeLevel", "Compact", "GetSpeakerConfig", "SetSpeakerConfig", "Initialize"];
const DSB: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "GetCaps", "GetCurrentPosition", "GetFormat", "GetVolume", "GetPan", "GetFrequency", "GetStatus", "Initialize", "Lock", "Play", "SetCurrentPosition", "SetFormat", "SetVolume",
    "SetPan", "SetFrequency", "Stop", "Unlock", "Restore",
];

#[derive(Clone)]
pub struct Buffer {
    pub ptr: u32,
    pub size: u32,
    pub primary: bool,
    /// WAVEFORMATEX fields.
    pub channels: u32,
    pub rate: u32,
    pub bits: u32,
    pub playing: bool,
    pub looping: bool,
    /// Play cursor in sample frames (fractional while the mixer resamples).
    pub fpos: f64,
    pub volume: i32,
    pub pan: i32,
    pub freq: u32,
}

impl Buffer {
    fn frame_bytes(&self) -> u32 {
        (self.channels.max(1) * (self.bits.max(8) / 8)).max(1)
    }

    fn bytes_per_sec(&self) -> u64 {
        self.rate.max(1) as u64 * self.frame_bytes() as u64
    }

    /// The play cursor (bytes).
    fn cursor(&self) -> u32 {
        (self.fpos as u32).saturating_mul(self.frame_bytes()).min(self.size)
    }
}

static BUFFERS: Mutex<Option<HashMap<u32, Buffer>>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut HashMap<u32, Buffer>) -> R) -> R {
    f(BUFFERS.lock().unwrap().get_or_insert_with(HashMap::new))
}

fn buffer_vtable(c: &mut Cpu) -> u32 {
    static VTABLE: Mutex<u32> = Mutex::new(0);
    let mut g = VTABLE.lock().unwrap();
    if *g != 0 {
        return *g;
    }
    *g = make_vtable(
        c,
        "IDirectSoundBuffer",
        DSB,
        &[
            ("QueryInterface", 3, |c, a| {
                // The same object answers for every buffer interface.
                c.mem.w32(a[2], a[0]);
                add_ref(c, a[0]);
                0
            }),
            ("AddRef", 1, |c, a| add_ref(c, a[0])),
            ("Release", 1, |c, a| {
                let n = release(c, a[0]);
                if n == 0 {
                    with(|b| b.remove(&a[0]));
                }
                n
            }),
            // DSBCAPS: size, flags, buffer bytes, unlock transfer rate, cpu overhead
            ("GetCaps", 2, |c, a| {
                let b = with(|b| b.get(&a[0]).cloned());
                let Some(b) = b else { return 0x8878_0000 + 0x32 };
                c.mem.w32(a[1] + 4, if b.primary { 1 } else { 0 });
                c.mem.w32(a[1] + 8, b.size);
                c.mem.w32(a[1] + 12, 0);
                c.mem.w32(a[1] + 16, 0);
                0
            }),
            ("GetCurrentPosition", 3, |c, a| {
                let Some(b) = with(|b| b.get(&a[0]).cloned()) else { return 0x8878_0000 + 0x32 };
                let play = b.cursor();
                if a[1] != 0 {
                    c.mem.w32(a[1], play);
                }
                if a[2] != 0 {
                    // The write cursor runs ahead of the play cursor by about 20 ms of data.
                    let lead = (b.bytes_per_sec() / 50) as u32;
                    let w = if b.size == 0 { 0 } else { (play + lead) % b.size };
                    c.mem.w32(a[2], w);
                }
                0
            }),
            // (this, WAVEFORMATEX*, size, written*)
            ("GetFormat", 4, |c, a| {
                let Some(b) = with(|b| b.get(&a[0]).cloned()) else { return 0x8878_0000 + 0x32 };
                if a[1] != 0 && a[2] >= 16 {
                    c.mem.w16(a[1], 1);
                    c.mem.w16(a[1] + 2, b.channels as u16);
                    c.mem.w32(a[1] + 4, b.rate);
                    c.mem.w32(a[1] + 8, b.rate * b.channels * b.bits / 8);
                    c.mem.w16(a[1] + 12, (b.channels * b.bits / 8) as u16);
                    c.mem.w16(a[1] + 14, b.bits as u16);
                    if a[2] >= 18 {
                        c.mem.w16(a[1] + 16, 0);
                    }
                }
                if a[3] != 0 {
                    c.mem.w32(a[3], 18);
                }
                0
            }),
            ("GetVolume", 2, |c, a| {
                let v = with(|b| b.get(&a[0]).map_or(0, |b| b.volume));
                c.mem.w32(a[1], v as u32);
                0
            }),
            ("GetPan", 2, |c, a| {
                let v = with(|b| b.get(&a[0]).map_or(0, |b| b.pan));
                c.mem.w32(a[1], v as u32);
                0
            }),
            ("GetFrequency", 2, |c, a| {
                let v = with(|b| b.get(&a[0]).map_or(0, |b| if b.freq != 0 { b.freq } else { b.rate }));
                c.mem.w32(a[1], v);
                0
            }),
            ("GetStatus", 2, |c, a| {
                let s = with(|b| b.get(&a[0]).map_or(0, |b| if b.playing { 1 | if b.looping { 4 } else { 0 } } else { 0 }));
                // A buffer that is not looping stops when it reaches its end.
                c.mem.w32(a[1], s);
                0
            }),
            ("Initialize", 3, |_, _| 0),
            // (this, offset, bytes, ptr1**, bytes1*, ptr2**, bytes2*, flags)
            ("Lock", 8, |c, a| {
                let Some(b) = with(|b| b.get(&a[0]).cloned()) else { return 0x8878_0000 + 0x32 };
                let (mut offset, mut bytes) = (a[1], a[2]);
                if a[7] & 1 != 0 {
                    offset = b.cursor();
                }
                if a[7] & 2 != 0 {
                    offset = 0;
                    bytes = b.size;
                }
                if b.size == 0 || offset >= b.size || bytes > b.size {
                    return 0x8007_0057; // E_INVALIDARG
                }
                let first = bytes.min(b.size - offset);
                c.mem.w32(a[3], b.ptr + offset);
                c.mem.w32(a[4], first);
                if a[5] != 0 {
                    c.mem.w32(a[5], if first < bytes { b.ptr } else { 0 });
                }
                if a[6] != 0 {
                    c.mem.w32(a[6], bytes - first);
                }
                0
            }),
            // (this, reserved, priority, flags)
            ("Play", 4, |_, a| {
                with(|b| {
                    if let Some(b) = b.get_mut(&a[0]) {
                        b.playing = true;
                        b.looping = a[3] & 1 != 0;
                    }
                });
                0
            }),
            ("SetCurrentPosition", 2, |_, a| {
                with(|b| {
                    if let Some(b) = b.get_mut(&a[0]) {
                        b.fpos = (a[1].min(b.size) / b.frame_bytes()) as f64;
                    }
                });
                0
            }),
            // (this, WAVEFORMATEX*)
            ("SetFormat", 2, |c, a| {
                let (ch, rate, bits) = (c.mem.r16(a[1] + 2) as u32, c.mem.r32(a[1] + 4), c.mem.r16(a[1] + 14) as u32);
                with(|b| {
                    if let Some(b) = b.get_mut(&a[0]) {
                        (b.channels, b.rate, b.bits) = (ch, rate, bits);
                    }
                });
                0
            }),
            ("SetVolume", 2, |_, a| {
                with(|b| b.get_mut(&a[0]).map(|b| b.volume = a[1] as i32));
                0
            }),
            ("SetPan", 2, |_, a| {
                with(|b| b.get_mut(&a[0]).map(|b| b.pan = a[1] as i32));
                0
            }),
            ("SetFrequency", 2, |_, a| {
                with(|b| b.get_mut(&a[0]).map(|b| b.freq = if a[1] == 0 { 0 } else { a[1] }));
                0
            }),
            ("Stop", 1, |_, a| {
                with(|b| {
                    if let Some(b) = b.get_mut(&a[0]) {
                        b.playing = false;
                    }
                });
                0
            }),
            ("Unlock", 5, |_, _| 0),
            ("Restore", 1, |_, _| 0),
        ],
    );
    *g
}

fn new_buffer(c: &mut Cpu, desc: u32, out: u32) -> u32 {
    let flags = c.mem.r32(desc + 4);
    let size = c.mem.r32(desc + 8);
    let wfx = c.mem.r32(desc + 16);
    let primary = flags & 1 != 0;
    let (channels, rate, bits) = if wfx != 0 { (c.mem.r16(wfx + 2) as u32, c.mem.r32(wfx + 4), c.mem.r16(wfx + 14) as u32) } else { (2, 22050, 16) };
    let size = if primary { 0 } else { size };
    let ptr = if size > 0 { heap::alloc_zeroed(c, size) } else { 0 };
    // 8-bit sound is unsigned: silence is 0x80.
    if bits == 8 && size > 0 {
        let silence = vec![0x80u8; size as usize];
        c.mem.write_bytes(ptr, &silence);
    }
    let vt = buffer_vtable(c);
    let o = new_object(c, vt);
    with(|b| b.insert(o, Buffer { ptr, size, primary, channels, rate, bits, playing: false, looping: false, fpos: 0.0, volume: 0, pan: 0, freq: 0 }));
    c.mem.w32(out, o);
    0
}

fn sound_vtable(c: &mut Cpu) -> u32 {
    static VTABLE: Mutex<u32> = Mutex::new(0);
    let mut g = VTABLE.lock().unwrap();
    if *g != 0 {
        return *g;
    }
    *g = make_vtable(
        c,
        "IDirectSound",
        DS,
        &[
            ("QueryInterface", 3, |c, a| {
                c.mem.w32(a[2], a[0]);
                add_ref(c, a[0]);
                0
            }),
            ("AddRef", 1, |c, a| add_ref(c, a[0])),
            ("Release", 1, |c, a| release(c, a[0])),
            // (this, DSBUFFERDESC*, IDirectSoundBuffer**, outer)
            ("CreateSoundBuffer", 4, |c, a| new_buffer(c, a[1], a[2])),
            ("GetCaps", 2, |c, a| {
                let size = c.mem.r32(a[1]);
                for o in (4..size.min(96)).step_by(4) {
                    c.mem.w32(a[1] + o, 0);
                }
                c.mem.w32(a[1] + 4, 0x10 | 0x20 | 0xf | 0xf00); // flags
                c.mem.w32(a[1] + 8, 100); // min secondary rate
                c.mem.w32(a[1] + 12, 100_000); // max secondary rate
                0
            }),
            // (this, source buffer, IDirectSoundBuffer**): shares the data
            ("DuplicateSoundBuffer", 3, |c, a| {
                let Some(src) = with(|b| b.get(&a[1]).cloned()) else { return 0x8878_0000 + 0x32 };
                let vt = buffer_vtable(c);
                let o = new_object(c, vt);
                with(|b| b.insert(o, Buffer { playing: false, looping: false, fpos: 0.0, ..src }));
                c.mem.w32(a[2], o);
                0
            }),
            ("SetCooperativeLevel", 3, |_, _| 0),
            ("Compact", 1, |_, _| 0),
            ("GetSpeakerConfig", 2, |c, a| {
                c.mem.w32(a[1], 4); // DSSPEAKER_STEREO
                0
            }),
            ("SetSpeakerConfig", 2, |_, _| 0),
            ("Initialize", 2, |_, _| 0),
        ],
    );
    *g
}

/// `DirectSoundCreate(guid, IDirectSound **ds, outer)`.
pub fn direct_sound_create(c: &mut Cpu, a: &[u32]) -> u32 {
    start_mixer();
    let vt = sound_vtable(c);
    let o = new_object(c, vt);
    c.mem.w32(a[1], o);
    0
}

/// A sound the host plays by itself (a movie's audio track, a wave file played through MCI).
pub struct Voice {
    pub id: u32,
    pub pcm: Arc<Vec<i16>>,
    pub rate: u32,
    pub channels: u32,
    pub fpos: f64,
    pub looping: bool,
}

static VOICES: Mutex<Vec<Voice>> = Mutex::new(Vec::new());
static NEXT_VOICE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Starts playing interleaved signed 16-bit samples; returns the voice id (see [`stop_voice`], [`voice_position`]).
pub fn play_pcm(pcm: Arc<Vec<i16>>, rate: u32, channels: u32) -> u32 {
    start_mixer();
    let id = NEXT_VOICE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    VOICES.lock().unwrap().push(Voice { id, pcm, rate, channels, fpos: 0.0, looping: false });
    id
}

pub fn stop_voice(id: u32) {
    VOICES.lock().unwrap().retain(|v| v.id != id);
}

/// Whether the voice still plays, and where (seconds).
pub fn voice_position(id: u32) -> Option<f64> {
    VOICES.lock().unwrap().iter().find(|v| v.id == id).map(|v| v.fpos / v.rate.max(1) as f64)
}

fn mix_voices(acc: &mut [f32]) {
    let mut voices = VOICES.lock().unwrap();
    voices.retain_mut(|v| {
        let ch = v.channels.max(1) as usize;
        let frames = v.pcm.len() / ch;
        let step = v.rate as f64 / MIX_RATE as f64;
        for i in 0..CHUNK {
            let idx = v.fpos as usize;
            if idx + 1 >= frames {
                return false;
            }
            let frac = (v.fpos - idx as f64) as f32;
            for k in 0..2 {
                let c0 = k.min(ch - 1);
                let a = v.pcm[idx * ch + c0] as f32;
                let b = v.pcm[(idx + 1) * ch + c0] as f32;
                acc[i * 2 + k] += a + (b - a) * frac;
            }
            v.fpos += step;
        }
        true
    });
}

const MIX_RATE: u32 = 44100;
/// Frames mixed per step (10 ms).
const CHUNK: usize = 441;

fn start_mixer() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::Builder::new().name("dsound-mixer".into()).spawn(mixer).expect("mixer thread");
    });
}

/// Gain of a DirectSound attenuation in hundredths of a decibel (0 is full, -10000 is silence).
fn gain(hundredths_db: i32) -> f32 {
    if hundredths_db <= -10000 { 0.0 } else { 10f32.powf(hundredths_db.min(0) as f32 / 2000.0) }
}

fn sample(m: &s4_x86::Mem, b: &Buffer, frame: u32, ch: u32) -> f32 {
    let fb = b.frame_bytes();
    let c = ch.min(b.channels.max(1) - 1);
    if b.bits == 8 {
        (m.r8(b.ptr + frame * fb + c) as i32 - 128) as f32 * 256.0
    } else {
        m.r16(b.ptr + frame * fb + c * 2) as i16 as f32
    }
}

fn mixer() {
    let mut sink = crate::miles::open_output(MIX_RATE);
    let m = crate::PROCESS.get().expect("no process").mem;
    let mut acc = vec![0f32; CHUNK * 2];
    let mut next = std::time::Instant::now();
    loop {
        acc.iter_mut().for_each(|x| *x = 0.0);
        with(|bufs| {
            for b in bufs.values_mut().filter(|b| b.playing && !b.primary && b.size > 0) {
                let total = b.size / b.frame_bytes();
                let step = if b.freq != 0 { b.freq } else { b.rate } as f64 / MIX_RATE as f64;
                let (g, pan) = (gain(b.volume), b.pan);
                let (gl, gr) = (g * if pan > 0 { gain(-pan) } else { 1.0 }, g * if pan < 0 { gain(pan) } else { 1.0 });
                for i in 0..CHUNK {
                    let idx = b.fpos as u32;
                    if idx >= total {
                        if b.looping {
                            b.fpos -= total as f64;
                            continue;
                        }
                        b.playing = false;
                        b.fpos = total as f64;
                        break;
                    }
                    let frac = (b.fpos - idx as f64) as f32;
                    let next_idx = if idx + 1 < total { idx + 1 } else if b.looping { 0 } else { idx };
                    let l = sample(&m, b, idx, 0) * (1.0 - frac) + sample(&m, b, next_idx, 0) * frac;
                    let r = if b.channels > 1 { sample(&m, b, idx, 1) * (1.0 - frac) + sample(&m, b, next_idx, 1) * frac } else { l };
                    acc[i * 2] += l * gl;
                    acc[i * 2 + 1] += r * gr;
                    b.fpos += step;
                }
            }
        });
        mix_music(&mut acc);
        mix_voices(&mut acc);
        let bytes: Vec<u8> = acc.iter().flat_map(|x| (x.clamp(-32768.0, 32767.0) as i16).to_le_bytes()).collect();
        if let Some(s) = &mut sink {
            if s.write_all(&bytes).is_err() {
                sink = None;
            }
        }
        // Keep real time even when the sink does not block (no device, a file): one chunk per 10 ms.
        next += std::time::Duration::from_millis(10);
        let now = std::time::Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}

/// Mixes the CD audio track that is playing into `acc` (stereo, [`MIX_RATE`]).
fn mix_music(acc: &mut [f32]) {
    let ended = {
        let mut cd = crate::cdaudio::CD.lock().unwrap();
        if !cd.playing || cd.paused {
            return;
        }
        let Some(t) = cd.data.clone() else { return };
        let ch = t.channels.max(1) as usize;
        let frames = t.pcm.len() / ch;
        let step = t.rate as f64 / MIX_RATE as f64;
        let mut ended = false;
        for i in 0..CHUNK {
            let idx = cd.fpos as usize;
            if idx + 1 >= frames {
                ended = true;
                break;
            }
            let frac = (cd.fpos - idx as f64) as f32;
            for k in 0..2 {
                let c0 = k.min(ch - 1);
                let a = t.pcm[idx * ch + c0] as f32;
                let b = t.pcm[(idx + 1) * ch + c0] as f32;
                acc[i * 2 + k] += a + (b - a) * frac;
            }
            cd.fpos += step;
        }
        ended
    };
    if ended {
        crate::cdaudio::track_ended();
    }
}
