//! One guest thread runs at a time. The lifted code is not atomic (`lock`-prefixed and `xchg`
//! instructions are plain read-modify-write, and the original relies on preemptive threads that
//! never interleave inside one instruction), so guest threads switch only where they call
//! into the host: every shim call releases the lock and takes it again afterwards (queueing in
//! arrival order). Off by default (nothing needed it so far); `S4_GIL=1` turns it on.

use std::cell::Cell;
use std::sync::{Condvar, Mutex};

struct Queue {
    next: u64,
    serving: u64,
}

static Q: Mutex<Queue> = Mutex::new(Queue { next: 0, serving: 0 });
static CV: Condvar = Condvar::new();

thread_local! {
    static HELD: Cell<bool> = const { Cell::new(false) };
    static EXIT: Exit = const { Exit };
}

struct Exit;

impl Drop for Exit {
    fn drop(&mut self) {
        release();
    }
}

fn disabled() -> bool {
    static D: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *D.get_or_init(|| std::env::var_os("S4_GIL").is_none())
}

pub fn acquire() {
    if disabled() || HELD.with(|h| h.get()) {
        return;
    }
    EXIT.with(|_| {});
    let mut q = Q.lock().unwrap();
    let ticket = q.next;
    q.next += 1;
    while q.serving != ticket {
        q = CV.wait(q).unwrap();
    }
    HELD.with(|h| h.set(true));
}

pub fn release() {
    if !HELD.with(|h| h.replace(false)) {
        return;
    }
    let mut q = Q.lock().unwrap();
    q.serving += 1;
    CV.notify_all();
}
