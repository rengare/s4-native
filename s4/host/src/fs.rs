//! The guest's view of the file system. The game thinks it lives in
//! `C:\S4` (current directory `C:\S4`, its modules in `C:\S4\Exe`); that maps to
//! the game directory on the host, any other absolute path to a private data
//! directory. Windows file names are case-insensitive, so every component is
//! matched against the real directory contents.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Fs {
    pub game_dir: PathBuf,
    pub data_dir: PathBuf,
    /// Current directory as the guest sees it (backslashes, no trailing slash).
    pub cwd: String,
}

static FS: Mutex<Option<Fs>> = Mutex::new(None);

pub const GUEST_ROOT: &str = "C:\\S4";

static ROOT: Mutex<Option<String>> = Mutex::new(None);

/// Changes the name the guest sees for the game folder (default [`GUEST_ROOT`]); call before [`init`].
pub fn set_root(root: &str) {
    *ROOT.lock().unwrap() = Some(root.to_string());
}

/// The guest name of the game folder.
pub fn root() -> String {
    ROOT.lock().unwrap().clone().unwrap_or_else(|| GUEST_ROOT.to_string())
}

pub fn init(game_dir: &Path, data_dir: &Path) {
    std::fs::create_dir_all(data_dir).ok();
    *FS.lock().unwrap() = Some(Fs { game_dir: game_dir.to_path_buf(), data_dir: data_dir.to_path_buf(), cwd: root() });
}

pub fn with<R>(f: impl FnOnce(&mut Fs) -> R) -> R {
    let mut g = FS.lock().unwrap();
    f(g.as_mut().expect("file system not initialised"))
}

/// A guest path made absolute and normalised: `C:\S4\Gfx\2.gh6`.
pub fn absolute(path: &str) -> String {
    let p = path.replace('/', "\\");
    let full = if p.len() >= 2 && p.as_bytes()[1] == b':' {
        if p.len() > 2 && p.as_bytes()[2] == b'\\' { p } else { format!("{}\\{}", with(|f| f.cwd.clone()), &p[2..]) }
    } else if let Some(rest) = p.strip_prefix('\\') {
        format!("C:\\{rest}")
    } else {
        format!("{}\\{}", with(|f| f.cwd.clone()), p)
    };
    let (drive, rest) = full.split_at(2);
    let mut parts: Vec<&str> = Vec::new();
    for comp in rest.split('\\') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("{}\\{}", drive.to_uppercase(), parts.join("\\"))
}

fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
            return Some(e.path());
        }
    }
    None
}

/// The host path of a guest path (components that do not exist yet are kept as given).
pub fn host_path(path: &str) -> PathBuf {
    let abs = absolute(path);
    let rest = &abs[3..];
    let (mut cur, comps): (PathBuf, Vec<&str>) = {
        let comps: Vec<&str> = if rest.is_empty() { Vec::new() } else { rest.split('\\').collect() };
        let root = root();
        let n = root.len();
        let in_game = abs.len() >= n && abs[..n].eq_ignore_ascii_case(&root) && (abs.len() == n || abs.as_bytes()[n] == b'\\');
        if in_game {
            (with(|f| f.game_dir.clone()), comps[1..].to_vec())
        } else {
            (with(|f| f.data_dir.join(abs[..1].to_lowercase())), comps)
        }
    };
    for c in comps {
        cur = find_ci(&cur, c).unwrap_or_else(|| cur.join(c));
    }
    cur
}

/// The guest name of a host path inside the game directory.
pub fn guest_name(host: &Path) -> String {
    with(|f| match host.strip_prefix(&f.game_dir) {
        Ok(rel) => format!("{}\\{}", root(), rel.to_string_lossy().replace('/', "\\")),
        Err(_) => host.to_string_lossy().replace('/', "\\"),
    })
}
