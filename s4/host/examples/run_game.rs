//! Runs the lifted game natively: `cargo run --release -p s4-host --example run_game -- <game dir>`.
use s4_host::World;

fn main() {
    let game = std::env::args().nth(1).unwrap_or_else(|| "extracted/s4".into());
    let game = std::fs::canonicalize(game).expect("game dir");
    let mut w = World::new(&s4_lifted::dispatchers());
    s4_host::fs::init(&game, &std::env::temp_dir().join("s4-host"));
    w.load_game(&game.join("Exe"));
    let missing = w.missing();
    eprintln!("{} imports are not implemented yet:", missing.len());
    for m in &missing {
        eprintln!("  {m}");
    }
    if std::env::var_os("S4_LIST_ONLY").is_some() {
        return;
    }
    w.attach_all();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.run_program())) {
        Ok(code) => eprintln!("program returned {code}"),
        Err(p) => {
            if let Some(j) = p.downcast_ref::<s4_x86::GuestJump>() {
                eprintln!("a jump to {:#x} found no function that owns it", j.target);
            } else {
                std::panic::resume_unwind(p);
            }
        }
    }
}
