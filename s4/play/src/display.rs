//! Screen resolution: which of the game's screen modes to use, and how big the window starts.
//!
//! The game knows four screen modes, chosen by `Screenmode` in `Config/GameSettings.cfg`:
//! 1 = 800x600, 2 = 1024x768, 3 = 1280x1024, and 4 = 1600x1200 in the original, 1920x1080 here: `tools/lift_all.sh`
//! patches the size in the translated GfxEngine (`tools/lift/patches`). (`WindowWidth`/`WindowHeight` there only matter for the
//! game's own windowed mode, which this program does not use.) The game reads the file at start-up,
//! so the choice is written to the file before the game is loaded; the game rewrites the file itself
//! when it saves its settings.
//!
//! The game also keeps the mode sizes in a table in its data; entry 4 is changed to 1920x1080 in memory after the
//! game is loaded (nothing on disk changes), so that both agree. The engine renders any size with the layout of
//! the 1280x1024 mode, so the map view simply gets bigger.

use std::path::Path;

/// (mode number, width, height)
pub const MODES: [(u8, u32, u32); 4] = [(1, 800, 600), (2, 1024, 768), (3, 1280, 1024), (4, 1920, 1080)];

/// Guest address of the game's table of screen sizes (four pairs of 32-bit width, height), in `S4_Main.exe` of
/// GOG 2.50.1516. Checked before it is changed.
pub const SIZE_TABLE: u32 = 0x665b48;


#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    /// The largest mode that suits the desktop.
    Auto,
    /// Leave the setting of the game folder alone.
    Keep,
    Mode(u8),
}

/// `auto`, `keep`, `800x600`, `1024x768`, `1280x1024`, `1920x1080` (or `hd`; or the mode number 1 to 4).
pub fn parse_choice(s: &str) -> Result<Choice, String> {
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "auto" | "" => return Ok(Choice::Auto),
        "keep" => return Ok(Choice::Keep),
        "hd" | "fhd" | "1080p" => return Ok(Choice::Mode(4)),
        _ => {}
    }
    for (n, w, h) in MODES {
        if s == format!("{w}x{h}") || s == n.to_string() {
            return Ok(Choice::Mode(n));
        }
    }
    Err(format!("unknown resolution {s:?}: use auto, keep, 800x600, 1024x768, 1280x1024 or 1920x1080"))
}

/// The choice for a desktop whose usable area is `usable` (width, height): the biggest mode that fits, so a
/// full-screen window shows it at its own size or a little bigger (1920x1080 on a full-HD desktop or bigger).
pub fn auto_choice(usable: (i32, i32)) -> Choice {
    let (w, h) = usable;
    if w >= 1900 && h >= 1000 {
        Choice::Mode(4)
    } else if h >= 1000 {
        Choice::Mode(3)
    } else if h >= 740 {
        Choice::Mode(2)
    } else {
        Choice::Mode(1)
    }
}

/// The game mode (value of `Screenmode`) a choice needs, if it names one.
pub fn mode_of(c: &Choice) -> Option<u8> {
    match c {
        Choice::Mode(m) => Some(*m),
        _ => None,
    }
}

pub fn size_of(mode: u8) -> (u32, u32) {
    MODES.iter().find(|m| m.0 == mode).map(|m| (m.1, m.2)).unwrap_or((800, 600))
}

/// The screen size a choice gives (`kept` is the mode stored in the game folder, for `Keep`).
pub fn size_of_choice(c: &Choice, kept: Option<u8>) -> (u32, u32) {
    match c {
        Choice::Mode(m) => size_of(*m),
        _ => size_of(kept.unwrap_or(1)),
    }
}

/// Changes entry 4 of the game's table of screen sizes (1600x1200 in the original) to `size`, when the table is
/// the expected one. Returns whether it did. `mem` is the loaded game's address space.
pub fn patch_size_table(mem: &s4_x86::Mem, size: (u32, u32)) -> bool {
    let at = SIZE_TABLE + 3 * 8;
    if mem.r32(at) != 1600 || mem.r32(at + 4) != 1200 || mem.r32(SIZE_TABLE) != 800 || mem.r32(SIZE_TABLE + 4) != 600 {
        return false;
    }
    mem.w32(at, size.0);
    mem.w32(at + 4, size.1);
    true
}

/// The `Screenmode` currently stored in the file, if any.
pub fn read_mode(game: &Path) -> Option<u8> {
    let text = std::fs::read_to_string(game.join("Config/GameSettings.cfg")).ok()?;
    text.lines().find_map(|l| l.trim().strip_prefix("Screenmode")?.trim_start().strip_prefix('=')?.trim().parse().ok())
}

/// Stores `mode` in `Config/GameSettings.cfg` (keeping the file's line endings and everything else). Returns
/// whether the file changed. A missing file or key is left alone: the game then uses its default.
pub fn write_mode(game: &Path, mode: u8) -> std::io::Result<bool> {
    write_keys(game, &[("Screenmode", mode as u32)])
}

/// Sets the game's two hardware-rendering switches (terrain and objects through Direct3D) on or off.
pub fn write_hardware(game: &Path, on: bool) -> std::io::Result<bool> {
    write_keys(game, &[("LandscapeHardware", on as u32), ("ObjectHardware", on as u32)])
}

fn write_keys(game: &Path, keys: &[(&str, u32)]) -> std::io::Result<bool> {
    let path = game.join("Config/GameSettings.cfg");
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(false) };
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        let key = keys.iter().find(|(k, _)| t.strip_prefix(k).is_some_and(|r| r.trim_start().starts_with('=')));
        if let Some((k, v)) = key {
            let indent = &line[..line.len() - t.len()];
            let eol = &line[line.trim_end_matches(['\r', '\n']).len()..];
            let new = format!("{indent}{k} = {v}{eol}");
            changed |= new != line;
            out.push_str(&new);
        } else {
            out.push_str(line);
        }
    }
    if changed {
        std::fs::write(&path, out)?;
    }
    Ok(changed)
}

/// The window's starting size: the mode's size, reduced to fit in 90% of the desktop's usable area.
pub fn window_size(frame: (u32, u32), usable: (i32, i32)) -> (i32, i32) {
    let (fw, fh) = (frame.0 as f64, frame.1 as f64);
    let k = (usable.0 as f64 * 0.9 / fw).min(usable.1 as f64 * 0.9 / fh).min(1.0);
    ((fw * k).round() as i32, (fh * k).round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices() {
        assert_eq!(parse_choice("auto"), Ok(Choice::Auto));
        assert_eq!(parse_choice("1280X1024"), Ok(Choice::Mode(3)));
        assert_eq!(parse_choice("1920x1080"), Ok(Choice::Mode(4)));
        assert_eq!(parse_choice("HD"), Ok(Choice::Mode(4)));
        assert_eq!(parse_choice("2"), Ok(Choice::Mode(2)));
        assert_eq!(parse_choice("keep"), Ok(Choice::Keep));
        assert!(parse_choice("1920x1200").is_err());
    }

    #[test]
    fn auto_follows_the_desktop() {
        assert_eq!(auto_choice((1920, 1050)), Choice::Mode(4));
        assert_eq!(auto_choice((2560, 1400)), Choice::Mode(4));
        assert_eq!(auto_choice((1600, 1170)), Choice::Mode(3));
        assert_eq!(auto_choice((1280, 1024)), Choice::Mode(3));
        assert_eq!(auto_choice((1366, 768)), Choice::Mode(2));
        assert_eq!(auto_choice((1024, 600)), Choice::Mode(1));
    }

    #[test]
    fn window_fits_the_desktop() {
        assert_eq!(window_size((800, 600), (1920, 1050)), (800, 600));
        let (w, h) = window_size((1280, 1024), (1920, 1050));
        assert!(h <= 945 && w * 1024 == h * 1280 || (w * 1024 - h * 1280).abs() < 1024, "{w}x{h}");
    }

    #[test]
    fn the_file_keeps_its_line_endings() {
        let dir = std::env::temp_dir().join(format!("s4disp-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Config")).unwrap();
        let f = dir.join("Config/GameSettings.cfg");
        std::fs::write(&f, "[GAMESETTINGS]\r\n{\r\n    Screenmode = 1\r\n    Other = 5\r\n}").unwrap();
        assert_eq!(read_mode(&dir), Some(1));
        assert!(write_mode(&dir, 3).unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "[GAMESETTINGS]\r\n{\r\n    Screenmode = 3\r\n    Other = 5\r\n}");
        assert!(!write_mode(&dir, 3).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
