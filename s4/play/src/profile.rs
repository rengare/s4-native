//! `S4_PROFILE=<file>`: a sampling profiler for the guest thread (there is no perf here). A helper thread signals the
//! guest thread every millisecond; the handler records the interrupted instruction address. The addresses (relative to
//! the program's load address) are written to the file every few seconds, to be matched against `nm -n` output.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

const CAPACITY: usize = 1 << 20;
static SAMPLES: [AtomicU64; CAPACITY] = [const { AtomicU64::new(0) }; CAPACITY];
/// The return address on the stack top (x86-64) or the link register (aarch64) at each sample: for a sample inside a leaf
/// routine such as memmove it tells the caller.
static CALLERS: [AtomicU64; CAPACITY] = [const { AtomicU64::new(0) }; CAPACITY];
static COUNT: AtomicUsize = AtomicUsize::new(0);
static LAST_CPU_NS: AtomicU64 = AtomicU64::new(0);

/// Samples taken while the thread was not running (it used no CPU time since the previous sample) have this bit set.
const IDLE: u64 = 1 << 63;

extern "C" fn on_signal(_: libc::c_int, _: *mut libc::siginfo_t, uc: *mut libc::c_void) {
    let uc = uc as *mut libc::ucontext_t;
    #[cfg(target_arch = "x86_64")]
    let pc = unsafe { (*uc).uc_mcontext.gregs[libc::REG_RIP as usize] as u64 };
    #[cfg(target_arch = "aarch64")]
    let pc = unsafe { (*uc).uc_mcontext.pc };
    #[cfg(target_arch = "x86_64")]
    let caller = unsafe { *((*uc).uc_mcontext.gregs[libc::REG_RSP as usize] as *const u64) };
    #[cfg(target_arch = "aarch64")]
    let caller = unsafe { (*uc).uc_mcontext.regs[30] };
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    let now = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
    let idle = now.saturating_sub(LAST_CPU_NS.swap(now, Relaxed)) < 200_000;
    let i = COUNT.fetch_add(1, Relaxed);
    if i < CAPACITY {
        CALLERS[i].store(caller, Relaxed);
        SAMPLES[i].store(if idle { pc | IDLE } else { pc }, Relaxed);
    }
}

/// Starts sampling the calling thread when `S4_PROFILE` is set.
pub fn start_for_this_thread() {
    let Ok(path) = std::env::var("S4_PROFILE") else { return };
    let tid = unsafe { libc::syscall(libc::SYS_gettid) } as libc::pid_t;
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as usize;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
        libc::sigaction(libc::SIGUSR2, &sa, std::ptr::null_mut());
    }
    // Load address of the program: the lowest mapping of the executable.
    let exe = std::fs::read_link("/proc/self/exe").unwrap_or_default();
    let base = std::fs::read_to_string("/proc/self/maps")
        .unwrap_or_default()
        .lines()
        .filter(|l| l.ends_with(exe.to_string_lossy().as_ref()))
        .filter_map(|l| u64::from_str_radix(l.split('-').next()?, 16).ok())
        .min()
        .unwrap_or(0);
    let pid = std::process::id() as libc::pid_t;
    std::thread::spawn(move || {
        let mut since_write = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1));
            unsafe { libc::syscall(libc::SYS_tgkill, pid, tid, libc::SIGUSR2) };
            since_write += 1;
            if since_write >= 4000 {
                since_write = 0;
                let n = COUNT.load(Relaxed).min(CAPACITY);
                let mut out = String::with_capacity(n * 8);
                for s in &SAMPLES[..n] {
                    // Absolute addresses; the first line is the program's load address, `<file>.maps` the mappings.
                    out.push_str(&format!("{:x}\n", s.load(Relaxed)));
                }
                out.insert_str(0, &format!("{base:x}\n"));
                let _ = std::fs::copy("/proc/self/maps", format!("{path}.maps"));
                let _ = std::fs::write(&path, out);
                let callers: String = CALLERS[..n].iter().map(|s| format!("{:x}\n", s.load(Relaxed))).collect();
                let _ = std::fs::write(format!("{path}.callers"), callers);
            }
        }
    });
}

/// `S4_TICKSCAN=<first second>`: looks for the game's logic tick counter. Takes snapshots of the program's data (0x400000..0x1000000)
/// one second apart from the given wall-clock second and prints every 32-bit value that grew steadily, 2 to 200 per second.
pub fn scan_counters(mem: s4_x86::Mem) {
    let Some(start) = std::env::var("S4_TICKSCAN").ok().and_then(|v| v.parse::<u64>().ok()) else { return };
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        const LO: u32 = 0x400000;
        const HI: u32 = 0x1000000;
        let mut snaps: Vec<(f64, Vec<u32>)> = Vec::new();
        for k in 0..5u64 {
            while t0.elapsed().as_secs_f64() < (start + k) as f64 {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let at = t0.elapsed().as_secs_f64();
            snaps.push((at, (LO..HI).step_by(4).map(|a| mem.r32(a)).collect()));
        }
        let n = snaps[0].1.len();
        let mut found = 0;
        for i in 0..n {
            let v: Vec<f64> = snaps.iter().map(|s| s.1[i] as f64).collect();
            let rates: Vec<f64> = (1..v.len()).map(|k| (v[k] - v[k - 1]) / (snaps[k].0 - snaps[k - 1].0)).collect();
            let mean = rates.iter().sum::<f64>() / rates.len() as f64;
            if (2.0..=200.0).contains(&mean) && rates.iter().all(|r| (r - mean).abs() <= mean * 0.35 + 1.0) {
                eprintln!("tick counter candidate {:#x}: {:.1}/s (values {:?})", LO + 4 * i as u32, mean, v.iter().map(|x| *x as u64).collect::<Vec<_>>());
                found += 1;
            }
        }
        eprintln!("tick scan done: {found} candidates");
    });
}

/// `S4_PACE=1`: prints the rates of two counters the game advances on its own clock (found with `S4_TICKSCAN`), every 5 seconds.
/// They stay at 35.0/s (0x7c5408) and 14.0/s (0xa329fc) however fast the program draws: a change means the game's pace changed.
pub fn pace_monitor(mem: s4_x86::Mem) {
    if std::env::var_os("S4_PACE").is_none() {
        return;
    }
    std::thread::spawn(move || {
        let mut last = (std::time::Instant::now(), mem.r32(0x7c5408), mem.r32(0xa329fc));
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let now = (std::time::Instant::now(), mem.r32(0x7c5408), mem.r32(0xa329fc));
            let dt = now.0.duration_since(last.0).as_secs_f64();
            eprintln!("pace: {:.1}/s {:.1}/s", now.1.wrapping_sub(last.1) as f64 / dt, now.2.wrapping_sub(last.2) as f64 / dt);
            last = now;
        }
    });
}
