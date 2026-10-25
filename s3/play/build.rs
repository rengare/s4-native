//! Links SDL3 when only its runtime library is installed (Debian/Ubuntu `libsdl3-0` without
//! `libsdl3-dev`): the bindings ask the linker for `-lSDL3`, which needs a `libSDL3.so` name that only
//! the development package provides. If the linker cannot find one but `libSDL3.so.0` exists, a symlink
//! to it in the build directory is enough. (With `libsdl3-dev`, pkg-config, or the `sdl3-from-source`
//! feature this does nothing.)

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_SDL3_FROM_SOURCE").is_some() {
        return;
    }
    let found_dev = Command::new("pkg-config").args(["--exists", "sdl3"]).status().map(|s| s.success()).unwrap_or(false);
    if found_dev {
        return;
    }
    let out = String::from_utf8(Command::new("ldconfig").arg("-p").output().map(|o| o.stdout).unwrap_or_default()).unwrap_or_default();
    // "\tlibSDL3.so.0 (libc6,x86-64) => /usr/lib/x86_64-linux-gnu/libSDL3.so.0"
    let runtime = out.lines().filter(|l| l.contains("libSDL3.so.0")).filter_map(|l| l.split("=>").nth(1)).map(|p| PathBuf::from(p.trim())).find(|p| p.exists());
    let Some(runtime) = runtime else { return };
    if let Some(dir) = runtime.parent() {
        if dir.join("libSDL3.so").exists() {
            return;
        }
    }
    let dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let link = dir.join("libSDL3.so");
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    if std::os::unix::fs::symlink(&runtime, &link).is_ok() {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
}
