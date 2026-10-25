//! CD audio through MCI for The Settlers III. The game plays its music as tracks of an audio CD ("cdaudio" device); the
//! GOG edition has no CD, it has the tracks as `Music/TrackNN.ogg`. The tracks are decoded here and mixed by the
//! DirectSound mixer ([`crate::dsound`]). Track numbers are those of the disc (track 1 is data; audio starts at 2).

use s4_x86::Cpu;
use std::sync::{Arc, Mutex};

/// The device id the game gets for "cdaudio".
/// 1, as on Windows for the first device opened: the game's CD window passes the notification on with its two values swapped, which only works out for device 1.
pub const DEVICE: u32 = 1;

const MCI_MODE_STOP: u32 = 525;
const MCI_MODE_PLAY: u32 = 526;
const MCI_MODE_PAUSE: u32 = 529;
const MM_MCINOTIFY: u32 = 0x3b9;

pub struct Track {
    pub pcm: Vec<i16>,
    pub rate: u32,
    pub channels: u32,
}

pub struct Cd {
    pub opened: bool,
    /// MCI time format (0 milliseconds, 2 MSF, 10 TMSF).
    pub time_format: u32,
    pub playing: bool,
    pub paused: bool,
    pub track: u32,
    pub to_track: u32,
    /// Decoded audio of the current track (None while it is being decoded).
    pub data: Option<Arc<Track>>,
    pub fpos: f64,
    /// Window to notify when the play command ends (0: none) and the id to say it with.
    pub notify: u32,
    /// Bumped by every play/stop so a decoder thread knows whether its track is still wanted.
    pub generation: u32,
}

pub static CD: Mutex<Cd> = Mutex::new(Cd { opened: false, time_format: 0, playing: false, paused: false, track: 0, to_track: 0, data: None, fpos: 0.0, notify: 0, generation: 0 });

fn track_path(n: u32) -> std::path::PathBuf {
    crate::fs::host_path(&format!("{}\\Music\\Track{n:02}.ogg", crate::fs::root()))
}

/// Number of tracks of the "disc": the data track plus the audio tracks present.
pub fn track_count() -> u32 {
    let dir = crate::fs::host_path(&format!("{}\\Music", crate::fs::root()));
    let mut max = 1;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if let Some(n) = name.strip_prefix("track").and_then(|r| r.strip_suffix(".ogg")).and_then(|n| n.parse::<u32>().ok()) {
                max = max.max(n);
            }
        }
    }
    max
}

fn decode(path: &std::path::Path) -> Option<Track> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("ogg");
    let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()).ok()?;
    let mut format = probed.format;
    let track = format.default_track()?.clone();
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).ok()?;
    let (mut rate, mut channels) = (track.codec_params.sample_rate.unwrap_or(44100), track.codec_params.channels.map_or(2, |c| c.count() as u32));
    let mut pcm: Vec<i16> = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track.id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else { continue };
        rate = decoded.spec().rate;
        channels = decoded.spec().channels.count() as u32;
        let mut buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, *decoded.spec());
        buf.copy_interleaved_ref(decoded);
        pcm.extend_from_slice(buf.samples());
    }
    // S4_CD_SECONDS=n: play only the first n seconds of every track (to test what happens when one ends).
    if let Some(n) = std::env::var("S4_CD_SECONDS").ok().and_then(|v| v.parse::<usize>().ok()) {
        pcm.truncate(n * rate as usize * channels as usize);
    }
    Some(Track { pcm, rate, channels })
}

/// Starts track `n` (decoding on a thread; the mixer waits for the data).
fn start_track(cd: &mut Cd, n: u32) {
    cd.track = n;
    cd.fpos = 0.0;
    cd.data = None;
    cd.generation = cd.generation.wrapping_add(1);
    let generation = cd.generation;
    let path = track_path(n);
    std::thread::spawn(move || {
        let t = decode(&path);
        let mut cd = CD.lock().unwrap();
        if cd.generation == generation {
            match t {
                Some(t) => cd.data = Some(Arc::new(t)),
                None => {
                    eprintln!("cdaudio: cannot play {}", path.display());
                    cd.playing = false;
                }
            }
        }
    });
}

/// Called by the mixer when the current track has been played to its end.
pub fn track_ended() {
    let mut cd = CD.lock().unwrap();
    if std::env::var_os("S4_MCI_TRACE").is_some() {
        eprintln!("cdaudio: track {} ended (to {}, notify window {:#x})", cd.track, cd.to_track, cd.notify);
    }
    if !cd.playing {
        return;
    }
    if cd.track < cd.to_track && cd.track < track_count() {
        let next = cd.track + 1;
        start_track(&mut cd, next);
    } else {
        cd.playing = false;
        cd.data = None;
        let hwnd = std::mem::take(&mut cd.notify);
        drop(cd);
        if hwnd != 0 {
            crate::user32::post_message(hwnd, MM_MCINOTIFY, 1, DEVICE);
        }
    }
}

fn abort_notify(cd: &mut Cd) {
    let hwnd = std::mem::take(&mut cd.notify);
    if hwnd != 0 {
        crate::user32::post_message(hwnd, MM_MCINOTIFY, 4, DEVICE);
    }
}

/// MCI time value of a track length / position in the current time format.
fn time_value(cd: &Cd, ms: u64, track: u32) -> u32 {
    let (m, s, f) = ((ms / 60000) as u32, ((ms / 1000) % 60) as u32, ((ms % 1000) * 75 / 1000) as u32);
    match cd.time_format {
        2 => m | s << 8 | f << 16,
        10 => track | m << 8 | s << 16 | f << 24,
        _ => ms as u32,
    }
}

fn track_length_ms(n: u32) -> u64 {
    decode(&track_path(n)).map_or(0, |t| t.pcm.len() as u64 / t.channels.max(1) as u64 * 1000 / t.rate.max(1) as u64)
}

/// A command for the "cdaudio" device. Returns the MCI error code (0: success).
pub fn command(c: &mut Cpu, msg: u32, flags: u32, parms: u32) -> u32 {
    const MCI_NOTIFY: u32 = 1;
    let mut cd = CD.lock().unwrap();
    match msg {
        // MCI_CLOSE
        0x804 => {
            abort_notify(&mut cd);
            cd.playing = false;
            cd.opened = false;
        }
        // MCI_PLAY: parms {callback, from, to}; MCI_FROM 4, MCI_TO 8
        0x806 => {
            let track_of = |cd: &Cd, v: u32| match cd.time_format {
                10 => v & 0xff,
                _ => v,
            };
            let from = if flags & 4 != 0 { track_of(&cd, c.mem.r32(parms + 4)) } else if cd.track >= 2 { cd.track } else { 2 };
            let to = if flags & 8 != 0 { track_of(&cd, c.mem.r32(parms + 8)) } else { track_count() };
            abort_notify(&mut cd);
            cd.paused = false;
            cd.playing = true;
            // MCI_TO names the position to stop at: a track number in TMSF means "up to the start of that track".
            cd.to_track = if cd.time_format == 10 && to > from { to - 1 } else { to.max(from) };
            cd.notify = if flags & MCI_NOTIFY != 0 { c.mem.r32(parms) } else { 0 };
            start_track(&mut cd, from.clamp(2, track_count().max(2)));
        }
        // MCI_STOP
        0x808 => {
            abort_notify(&mut cd);
            cd.playing = false;
            cd.paused = false;
            cd.generation = cd.generation.wrapping_add(1);
        }
        // MCI_PAUSE
        0x809 => cd.paused = true,
        // MCI_RESUME
        0x855 => cd.paused = false,
        // MCI_SEEK
        0x807 => {
            cd.playing = false;
            cd.fpos = 0.0;
        }
        // MCI_SET: parms {callback, time format, audio}; MCI_SET_TIME_FORMAT 0x400
        0x80d => {
            if flags & 0x400 != 0 {
                cd.time_format = c.mem.r32(parms + 4);
            }
        }
        // MCI_STATUS: parms {callback, return, item, track}; MCI_STATUS_ITEM 0x100, MCI_TRACK 0x10
        0x814 => {
            let item = c.mem.r32(parms + 8);
            let track = if flags & 0x10 != 0 { c.mem.r32(parms + 12) } else { cd.track };
            let value = match item {
                1 => {
                    // length: of the disc or of a track
                    drop(cd);
                    let ms = if flags & 0x10 != 0 { track_length_ms(track) } else { (2..=track_count()).map(track_length_ms).sum() };
                    cd = CD.lock().unwrap();
                    time_value(&cd, ms, 0)
                }
                2 => {
                    let rate = cd.data.as_ref().map_or(44100, |d| d.rate) as f64;
                    time_value(&cd, (cd.fpos / rate * 1000.0) as u64, cd.track)
                }
                3 => track_count(),
                4 => if cd.paused { MCI_MODE_PAUSE } else if cd.playing { MCI_MODE_PLAY } else { MCI_MODE_STOP },
                5 | 7 => 1,
                6 => cd.time_format,
                8 => cd.track.max(1),
                0x4001 => 0x4001 + 1, // MCI_CDA_TRACK_AUDIO
                _ => 0,
            };
            c.mem.w32(parms + 4, value);
        }
        _ => {}
    }
    if std::env::var_os("S4_MCI_TRACE").is_some() {
        eprintln!("cdaudio: msg {msg:#x} -> track {} to {} playing {} paused {}", cd.track, cd.to_track, cd.playing, cd.paused);
    }
    0
}
