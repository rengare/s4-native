//! What the original binkw32.dll (lifted, `lifted/binkw32`) imports beyond the common
//! kernel32/user32/gdi32 shims: its window/GDI display paths (the game draws the video itself
//! through `BinkCopyToBuffer`), its own waveOut sound (the game gives Bink the Miles sound
//! system instead, so waveOut is refused) and a few system queries.

use crate::Shim;

macro_rules! sh {
    ($dll:expr, $name:expr, $args:expr, $f:expr) => {
        Shim { dll: $dll, name: $name, args: $args, cdecl: false, f: $f }
    };
}

fn put_dir(c: &mut s4_x86::Cpu, buf: u32, size: u32, s: &str) -> u32 {
    let b = s.as_bytes();
    if (b.len() as u32) < size {
        c.mem.write_bytes(buf, b);
        c.mem.w8(buf + b.len() as u32, 0);
    }
    b.len() as u32
}

pub fn all() -> Vec<Shim> {
    vec![
        sh!("kernel32.dll", "GetWindowsDirectoryA", 2, |c, a| put_dir(c, a[0], a[1], "C:\\WINDOWS")),
        sh!("kernel32.dll", "GetSystemDirectoryA", 2, |c, a| put_dir(c, a[0], a[1], "C:\\WINDOWS\\SYSTEM")),
        sh!("kernel32.dll", "ReleaseMutex", 1, |_, a| match crate::handles::get(a[0]) {
            Some(crate::handles::Obj::Mutex(s)) => {
                let mut st = s.state.lock().unwrap();
                let mut o = s.owner.lock().unwrap();
                if o.1 == 0 {
                    return 0;
                }
                o.1 -= 1;
                if o.1 == 0 {
                    *o = (0, 0);
                    *st = true;
                    s.cond.notify_all();
                }
                1
            }
            _ => 0,
        }),
        sh!("kernel32.dll", "WaitForMultipleObjects", 4, |c, a| crate::kernel32::wait_multiple(c, a[0], a[1], a[2] != 0, a[3])),
        sh!("user32.dll", "BeginPaint", 2, |_, _| 1),
        sh!("user32.dll", "EndPaint", 2, |_, _| 1),
        sh!("user32.dll", "RegisterClassA", 1, |_, _| 0xc001),
        sh!("user32.dll", "UnregisterClassA", 2, |_, _| 1),
        sh!("user32.dll", "GetTopWindow", 1, |_, _| 0),
        sh!("user32.dll", "GetWindow", 2, |_, _| 0),
        sh!("user32.dll", "GetWindowThreadProcessId", 2, |_, _| 1),
        sh!("user32.dll", "GetClassLongA", 2, |_, _| 0),
        sh!("user32.dll", "ChangeDisplaySettingsA", 2, |_, _| 0),
        sh!("user32.dll", "IsWindowVisible", 1, |_, _| 1),
        sh!("gdi32.dll", "SetStretchBltMode", 2, |_, _| 1),
        sh!("gdi32.dll", "StretchBlt", 11, |_, _| 1),
        sh!("gdi32.dll", "SetPixel", 4, |_, _| 0),
        // MMSYSERR_NODRIVER: Bink goes on without its own sound.
        sh!("winmm.dll", "waveOutOpen", 6, |_, _| 4),
        sh!("winmm.dll", "waveOutClose", 1, |_, _| 0),
        sh!("winmm.dll", "waveOutPause", 1, |_, _| 0),
        sh!("winmm.dll", "waveOutRestart", 1, |_, _| 0),
        sh!("winmm.dll", "waveOutReset", 1, |_, _| 0),
        sh!("winmm.dll", "waveOutSetVolume", 2, |_, _| 0),
        sh!("winmm.dll", "waveOutWrite", 3, |_, _| 0),
        sh!("winmm.dll", "waveOutPrepareHeader", 3, |_, _| 0),
        sh!("winmm.dll", "waveOutUnprepareHeader", 3, |_, _| 0),
    ]
}
