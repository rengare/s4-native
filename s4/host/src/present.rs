//! The screen: the last image the guest showed (a flip of, or a blit to, the
//! primary DirectDraw surface), for the window front end to display.

use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    /// 0x00RRGGBB.
    pub pixels: Vec<u32>,
}

static FRAME: Mutex<Option<(u64, Arc<Frame>)>> = Mutex::new(None);

/// The last frame the guest itself showed (a video shown over it does not replace it).
static GAME: Mutex<Option<Arc<Frame>>> = Mutex::new(None);

pub fn publish(frame: Frame) {
    let frame = Arc::new(frame);
    *GAME.lock().unwrap() = Some(frame.clone());
    let mut g = FRAME.lock().unwrap();
    let seq = g.as_ref().map(|f| f.0 + 1).unwrap_or(1);
    *g = Some((seq, frame));
}

/// Shows a frame that is not the guest's own (a movie): the guest's last frame stays remembered.
pub fn publish_overlay(frame: Frame) {
    let mut g = FRAME.lock().unwrap();
    let seq = g.as_ref().map(|f| f.0 + 1).unwrap_or(1);
    *g = Some((seq, Arc::new(frame)));
}

/// Shows the guest's own last frame again (after a movie).
pub fn restore_game_frame() {
    if let Some(f) = GAME.lock().unwrap().clone() {
        let mut g = FRAME.lock().unwrap();
        let seq = g.as_ref().map(|f| f.0 + 1).unwrap_or(1);
        *g = Some((seq, f));
    }
}

/// Size of the guest's own last frame (800x600 before it has shown one).
pub fn game_frame_size() -> (u32, u32) {
    GAME.lock().unwrap().as_ref().map_or((800, 600), |f| (f.w, f.h))
}

/// The frame newer than `seen` (its sequence number), if any.
pub fn newer_than(seen: u64) -> Option<(u64, Arc<Frame>)> {
    let g = FRAME.lock().unwrap();
    match g.as_ref() {
        Some((seq, f)) if *seq > seen => Some((*seq, Arc::clone(f))),
        _ => None,
    }
}
