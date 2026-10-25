//! GfxEngine.dll, lifted, running in the native host (no Wine, no original
//! code): the sequence S4_Main (and `tools/gfxref`) use to draw a map, and
//! the frame must equal the one the original drew under Wine
//! (`logs/reference/aeneas.rgb565`, made by `tools/gfxref`).
#![cfg(unix)]

use s4_host::{shims, Shim, World};

fn exe_dir() -> std::path::PathBuf {
    std::env::var("S4_EXE_DIR").map(Into::into).unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extracted/s4/Exe"))
}

fn game_dir() -> std::path::PathBuf {
    exe_dir().join("..")
}

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn put(w: &mut World, bytes: &[u8]) -> u32 {
    let p = s4_host::heap::alloc(&mut w.cpu, bytes.len() as u32 + 16);
    w.cpu.mem.write_bytes(p, bytes);
    p
}

thread_local! {
    static TEXTURE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[test]
fn draws_the_reference_frame() {
    let reference = root().join("logs/reference/aeneas.rgb565");
    let Ok(reference) = std::fs::read(&reference) else {
        eprintln!("no reference frame (run tools/gfxref first); skipping");
        return;
    };
    let mut w = World::new(&[lifted_gfxengine::dispatch]);
    let bytes = std::fs::read(exe_dir().join("GfxEngine.dll")).unwrap();
    let m = w.load("GfxEngine", &bytes);
    s4_host::fs::init(&game_dir(), &std::env::temp_dir().join("s4-host-test"));
    w.link();
    assert_eq!(w.attach(m) & 0xff, 1, "DllMain failed");
    let export = |w: &World, n: &str| w.export(m, n);

    let map_data = std::fs::read(game_dir().join("Map/Singleplayer/Aeneas.map")).unwrap();
    let map = s4_formats_clean::map::MapFile::parse(&map_data).unwrap();
    let side = map.size().unwrap() as u32;
    let layer = map.chunk(s4_formats_clean::map::LANDSCAPE).unwrap().data().unwrap();
    let objects = map.chunk(s4_formats_clean::map::OBJECTS).unwrap().data().unwrap();
    let count = (side * side) as usize;
    let mut words = Vec::new();
    for t in objects.chunks_exact(4) {
        words.extend_from_slice(&t[..2]);
    }
    for t in objects.chunks_exact(4) {
        words.extend_from_slice(&t[2..]);
    }
    let layer_p = put(&mut w, &layer);
    let words_p = put(&mut w, &words);

    // The texture set S4_Main hands over: file after its 20-byte header.
    let tex = std::fs::read(game_dir().join("Gfx/2.gh6")).unwrap();
    let tex_p = put(&mut w, &tex[20..]);
    TEXTURE.with(|t| t.set(tex_p));
    let reload = w.add_shim(Shim { dll: "host", name: "reload", args: 3, cdecl: false, f: |_, a| if a[0] == 1 { TEXTURE.with(|t| t.get()) } else { 0 } });
    let objects_cb = w.add_shim(Shim { dll: "host", name: "objects", args: 0, cdecl: false, f: |_, _| 0 });

    let engine = s4_host::heap::alloc(&mut w.cpu, 256);
    w.cpu.mem.fill(engine, 0, 256);
    w.cpu.ecx = engine;
    w.call(export(&w, "??0IGfxEngine@@QAE@XZ"), &[], true);
    w.call(export(&w, "?SetReloadCallback@IGfxEngine@@QAGXP6GPAXH_N0@Z@Z"), &[engine, reload], true);
    let outs = s4_host::heap::alloc(&mut w.cpu, 16);
    let cfg = [0x0000_0100u32, 0, 1, 0x1234, 800, 600, 0, 0, 0, 0];
    let mut args = vec![engine];
    args.extend_from_slice(&cfg);
    args.extend_from_slice(&[1, outs, outs + 4, outs + 8]);
    let ok = w.call(export(&w, "?InitEngine@IGfxEngine@@QAG_NUSGfxRenderConfiguration@@_NAAH22@Z"), &args, true);
    assert_eq!(ok & 0xff, 1, "InitEngine failed; unimplemented imports: {:?}", w.missing());
    w.call(export(&w, "?SetGfxLayerAccess@IGfxEngine@@QAGXHPAUT_GFX_MAP_ELEMENT@@@Z"), &[engine, side, layer_p], true);
    w.call(export(&w, "?SetObjectLayerAccess@IGfxEngine@@QAGXP6IPAUSGfxObjectInfo@@IH@ZPAG1@Z"), &[engine, objects_cb, words_p, words_p + 2 * count as u32], true);
    w.call(export(&w, "?SetTriangleSize@IGfxEngine@@QAGXH@Z"), &[engine, 0x18_0000], true);
    w.call(export(&w, "?SetCameraMode@IGfxEngine@@QAGXH@Z"), &[engine, 1], true);
    w.call(export(&w, "?SetScrollOffsets@IGfxEngine@@QAGXHH@Z"), &[engine, 0, 0], true);
    let render = export(&w, "?RenderFrame@IGfxEngine@@QAG_N_N0@Z");
    for i in 0..20 {
        shims::set_clock(Some(1000 + 100 * i));
        w.call(render, &[engine, 0, 0], true);
    }
    // The back buffer: renderer (renderer_object) +0x3c is the surface; vtable +0x1c is Lock.
    let m = w.mem();
    let renderer = m.r32(0x18c6_64f4);
    let surface = m.r32(renderer + 0x3c);
    let vtable = m.r32(surface);
    let (lock, unlock) = (m.r32(vtable + 0x1c), m.r32(vtable + 0x20));
    w.cpu.ecx = surface;
    let r = w.call(lock, &[outs, outs + 4, 1], true);
    assert_eq!(r, 0, "Lock failed");
    let (pitch, bits) = (m.r32(outs), m.r32(outs + 4));
    let stride = if pitch as usize >= 1600 { pitch as usize } else { pitch as usize * 2 };
    let mut frame = Vec::new();
    for y in 0..600 {
        frame.extend_from_slice(&m.read_bytes(bits + (y * stride) as u32, 1600));
    }
    w.cpu.ecx = surface;
    w.call(unlock, &[], true);
    for sf in s4_host::ddraw::debug_surfaces(&w.cpu) {
        println!("surface {:#x}: {}x{} {}bpp caps {:#x} nonzero {}", sf.0, sf.1, sf.2, sf.3, sf.4, sf.5);
    }
    let nonzero = frame.iter().filter(|&&b| b != 0).count();
    println!("frame bytes non-zero: {nonzero}; first pixels {:02x?} vs reference {:02x?}; stride {stride}", &frame[1600*300..1600*300+16], &reference[1600*300..1600*300+16]);
    let diff = frame.chunks_exact(2).zip(reference.chunks_exact(2)).filter(|(a, b)| a != b).count();
    println!("native host frame vs the original's: {diff} of 480000 pixels differ; unimplemented imports: {:?}", w.missing());
    assert_eq!(diff, 0);
    let _ = shims::all;
}
