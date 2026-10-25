//! Sound output through SDL3: an audio stream on the default playback device, fed by the game's
//! mixer thread (`s4_host::miles`) with interleaved signed 16-bit stereo.

use sdl3_sys::everything::*;
use std::ffi::c_int;
use std::io::Write;
use std::ptr::null_mut;

struct Sink {
    stream: *mut SDL_AudioStream,
    /// Stop putting data in while this many bytes are queued: it bounds the delay between a game
    /// event and its sound, and (because `write` blocks) it paces the mixer.
    limit: i32,
}

// SDL audio streams are thread-safe; the pointer is only used from the mixer thread.
unsafe impl Send for Sink {}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        unsafe {
            while SDL_GetAudioStreamQueued(self.stream) > self.limit {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            if !SDL_PutAudioStreamData(self.stream, bytes.as_ptr() as *const _, bytes.len() as c_int) {
                return Err(std::io::Error::other("audio stream closed"));
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn open(rate: u32) -> Option<Box<dyn Write + Send>> {
    unsafe {
        let spec = SDL_AudioSpec { format: SDL_AUDIO_S16, channels: 2, freq: rate as c_int };
        let stream = SDL_OpenAudioDeviceStream(SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK, &spec, None, null_mut());
        if stream.is_null() {
            eprintln!("audio: cannot open the playback device: {}", crate::sdl_error());
            return None;
        }
        SDL_ResumeAudioStreamDevice(stream);
        eprintln!("audio: playing through SDL3 ({rate} Hz stereo)");
        Some(Box::new(Sink { stream, limit: (rate / 25 * 4) as i32 }))
    }
}

/// Tells the game's mixer to play through SDL.
pub fn register() {
    s4_host::miles::set_output_factory(open);
}
