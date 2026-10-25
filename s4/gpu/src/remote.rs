//! [`RemoteGpu`]: the renderer on a thread of its own. The guest thread (which runs the translated game) only queues commands;
//! the renderer thread encodes, uploads, converts texture pixels and submits. Commands run in the order they were queued, so
//! the picture is the same as with a [`Gpu`] called directly. Ids are made on the calling side ([`alloc_id`]).

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering::*};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::Arc;

/// Queued pixel data above this many bytes makes the caller wait (a bound on memory, not a pacing device).
const QUEUED_BYTES_LIMIT: usize = 96 << 20;

enum Cmd {
    CreateTexture(TextureId, u32, u32),
    DestroyTexture(TextureId),
    WriteTexture(TextureId, Vec<u8>),
    WriteTexture16 { id: TextureId, rect: (u32, u32, u32, u32), fmt: u32, data: Vec<u8> },
    CreateTarget(TargetId, u32, u32, u32),
    DestroyTarget(TargetId),
    TargetTexture(TargetId, TextureId),
    Blit { target: TargetId, texture: TextureId, src: (u32, u32, u32, u32), dst: (u32, u32), blend: bool },
    BeginScene(TargetId),
    Draw(TargetId, DrawState, Vec<Vertex>),
    EndScene(TargetId),
    WriteTarget(TargetId, (u32, u32, u32, u32), Vec<u8>),
    Present(TargetId, PresentSink),
}

pub struct RemoteGpu {
    tx: SyncSender<Vec<Cmd>>,
    queued: Arc<AtomicUsize>,
    pub adapter_name: String,
    /// Aliases made so far: target -> texture that shows its picture.
    aliases: HashMap<TargetId, TextureId>,
    /// Draws not sent yet: consecutive draws with the same target and state travel as one command.
    pending: Option<(TargetId, DrawState, Vec<Vertex>)>,
    /// Commands not sent yet; they travel together (waking the renderer thread costs a system call per send).
    batch: Vec<Cmd>,
}

/// A batch goes when it holds this many commands, or when the scene ends, or a picture is presented.
const BATCH: usize = 48;

impl RemoteGpu {
    /// Opens the renderer on its own thread. Errors as [`Gpu::new`].
    pub fn new() -> Result<RemoteGpu, String> {
        let (tx, rx) = sync_channel::<Vec<Cmd>>(256);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<String, String>>();
        let queued = Arc::new(AtomicUsize::new(0));
        let q = queued.clone();
        std::thread::Builder::new()
            .name("gpu-render".into())
            .spawn(move || match Gpu::new() {
                Ok(gpu) => {
                    let _ = ready_tx.send(Ok(gpu.adapter_name.clone()));
                    run(gpu, rx, q);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| e.to_string())?;
        let adapter_name = ready_rx.recv().map_err(|e| e.to_string())??;
        Ok(RemoteGpu { tx, queued, adapter_name, aliases: HashMap::new(), pending: None, batch: Vec::new() })
    }

    fn send(&mut self, c: Cmd) {
        self.flush_draws();
        self.batch.push(c);
        if self.batch.len() >= BATCH {
            self.send_batch();
        }
    }

    fn send_batch(&mut self) {
        if !self.batch.is_empty() {
            let _ = self.tx.send(std::mem::take(&mut self.batch));
        }
    }

    fn send_bytes(&mut self, bytes: usize, c: Cmd) {
        if self.queued.load(Acquire) > QUEUED_BYTES_LIMIT {
            self.flush_draws();
            self.send_batch();
            while self.queued.load(Acquire) > QUEUED_BYTES_LIMIT {
                std::thread::sleep(std::time::Duration::from_micros(100));
            }
        }
        self.queued.fetch_add(bytes, AcqRel);
        self.send(c);
    }

    fn flush_draws(&mut self) {
        if let Some((t, s, v)) = self.pending.take() {
            let bytes = v.len() * std::mem::size_of::<Vertex>();
            self.queued.fetch_add(bytes, AcqRel);
            self.batch.push(Cmd::Draw(t, s, v));
        }
    }

    pub fn create_texture(&mut self, w: u32, h: u32) -> TextureId {
        let id = alloc_id();
        self.send(Cmd::CreateTexture(id, w, h));
        id
    }

    pub fn destroy_texture(&mut self, id: TextureId) {
        self.send(Cmd::DestroyTexture(id));
    }

    /// Replaces the whole texture with RGBA8 pixels.
    pub fn write_texture(&mut self, id: TextureId, rgba: Vec<u8>) {
        self.send_bytes(rgba.len(), Cmd::WriteTexture(id, rgba));
    }

    /// Replaces a rectangle of the texture with the pixels of a 16-bit surface (rows packed, `w * 2` bytes each); `fmt` is the
    /// surface format (0: 5-6-5, 1: 1-5-5-5, 2: 4-4-4-4). The conversion to RGBA8 happens on the renderer thread.
    pub fn write_texture_region16(&mut self, id: TextureId, rect: (u32, u32, u32, u32), fmt: u32, data: Vec<u8>) {
        self.send_bytes(data.len() * 2, Cmd::WriteTexture16 { id, rect, fmt, data });
    }

    pub fn create_target(&mut self, w: u32, h: u32, scale: u32) -> TargetId {
        let id = alloc_id();
        self.send(Cmd::CreateTarget(id, w, h, scale));
        id
    }

    pub fn destroy_target(&mut self, id: TargetId) {
        self.aliases.remove(&id);
        self.send(Cmd::DestroyTarget(id));
    }

    pub fn target_texture(&mut self, id: TargetId) -> Option<TextureId> {
        if let Some(&a) = self.aliases.get(&id) {
            return Some(a);
        }
        let a = alloc_id();
        self.aliases.insert(id, a);
        self.send(Cmd::TargetTexture(id, a));
        Some(a)
    }

    pub fn blit(&mut self, target: TargetId, texture: TextureId, src: (u32, u32, u32, u32), dst: (u32, u32), blend: bool) {
        self.send(Cmd::Blit { target, texture, src, dst, blend });
    }

    pub fn begin_scene(&mut self, id: TargetId) {
        self.send(Cmd::BeginScene(id));
    }

    pub fn draw(&mut self, id: TargetId, state: DrawState, vertices: &[Vertex]) {
        self.draw_fill(id, state, vertices.len(), |v| v.copy_from_slice(vertices));
    }

    /// Adds `count` vertices that `fill` writes straight into the queued batch (no copy in between).
    pub fn draw_fill(&mut self, id: TargetId, state: DrawState, count: usize, fill: impl FnOnce(&mut [Vertex])) {
        match &mut self.pending {
            Some((t, s, v)) if *t == id && *s == state => {
                let at = v.len();
                v.resize(at + count, Vertex::zeroed());
                fill(&mut v[at..]);
            }
            _ => {
                self.flush_draws();
                let mut v = vec![Vertex::zeroed(); count];
                fill(&mut v);
                self.pending = Some((id, state, v));
            }
        }
    }

    pub fn end_scene_lazy(&mut self, id: TargetId) {
        self.send(Cmd::EndScene(id));
        self.send_batch();
    }

    pub fn write_target(&mut self, id: TargetId, rect: (u32, u32, u32, u32), rgba: Vec<u8>) {
        self.send_bytes(rgba.len(), Cmd::WriteTarget(id, rect, rgba));
    }

    /// Queues the target's picture to go to `sink` (on another thread, when it is read back). Everything queued before runs first.
    pub fn present_async(&mut self, id: TargetId, sink: &PresentSink) {
        self.send(Cmd::Present(id, sink.clone()));
        self.send_batch();
    }
}

fn run(mut gpu: Gpu, rx: Receiver<Vec<Cmd>>, queued: Arc<AtomicUsize>) {
    for cmd in rx.into_iter().flatten() {
        match cmd {
            Cmd::CreateTexture(id, w, h) => gpu.create_texture_as(id, w, h),
            Cmd::DestroyTexture(id) => gpu.destroy_texture(id),
            Cmd::WriteTexture(id, rgba) => {
                gpu.write_texture(id, &rgba);
                queued.fetch_sub(rgba.len(), AcqRel);
            }
            Cmd::WriteTexture16 { id, rect, fmt, data } => {
                let rgba = convert_packed(fmt, &data, rect.2);
                gpu.write_texture_region(id, rect, &rgba);
                queued.fetch_sub(data.len() * 2, AcqRel);
            }
            Cmd::CreateTarget(id, w, h, s) => gpu.create_target_as(id, w, h, s),
            Cmd::DestroyTarget(id) => gpu.destroy_target(id),
            Cmd::TargetTexture(id, alias) => {
                gpu.target_texture_as(id, alias);
            }
            Cmd::Blit { target, texture, src, dst, blend } => gpu.blit(target, texture, src, dst, blend),
            Cmd::BeginScene(id) => gpu.begin_scene(id, None),
            Cmd::Draw(id, state, vertices) => {
                gpu.draw(id, state, &vertices);
                queued.fetch_sub(vertices.len() * std::mem::size_of::<Vertex>(), AcqRel);
            }
            Cmd::EndScene(id) => gpu.end_scene_lazy(id),
            Cmd::WriteTarget(id, rect, rgba) => {
                gpu.write_target(id, rect, &rgba);
                queued.fetch_sub(rgba.len(), AcqRel);
            }
            Cmd::Present(id, sink) => {
                // Every staging buffer may still be in use by the reader thread: wait for one.
                let mut tries = 0;
                while !gpu.present_async(id, &sink) {
                    tries += 1;
                    if tries > 5000 {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_micros(200));
                }
            }
        }
    }
}
