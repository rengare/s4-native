//! Kernel objects behind guest handles: files, events, threads, mutexes,
//! directory searches, file mappings.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

pub struct FileObj {
    pub file: std::fs::File,
    pub path: std::path::PathBuf,
    pub append: bool,
}

pub struct Signal {
    pub state: Mutex<bool>,
    pub cond: Condvar,
    pub manual: bool,
    /// Mutexes: (owner thread id, recursion count).
    pub owner: Mutex<(u32, u32)>,
}

pub struct ThreadObj {
    pub exit: Mutex<Option<u32>>,
    pub cond: Condvar,
    pub id: u32,
    /// Open once the thread may run (CREATE_SUSPENDED threads wait for `ResumeThread`).
    pub gate: Mutex<bool>,
    pub gate_cond: Condvar,
}

/// A mapping backed by memory, not a file (shared memory by name): its view is made once.
pub struct AnonMap {
    pub size: u32,
    pub addr: Mutex<u32>,
}

pub struct FindObj {
    pub entries: Vec<(String, std::fs::Metadata)>,
    pub next: usize,
}

pub enum Obj {
    File(Arc<Mutex<FileObj>>),
    Event(Arc<Signal>),
    Thread(Arc<ThreadObj>),
    Mutex(Arc<Signal>),
    /// Count and maximum are kept in `owner`.
    Semaphore(Arc<Signal>),
    Find(Arc<Mutex<FindObj>>),
    Std(u32),
    Mapping(Arc<Mutex<FileObj>>),
    Anon(Arc<AnonMap>),
}

impl Clone for Obj {
    fn clone(&self) -> Obj {
        match self {
            Obj::File(a) => Obj::File(a.clone()),
            Obj::Event(a) => Obj::Event(a.clone()),
            Obj::Thread(a) => Obj::Thread(a.clone()),
            Obj::Mutex(a) => Obj::Mutex(a.clone()),
            Obj::Semaphore(a) => Obj::Semaphore(a.clone()),
            Obj::Find(a) => Obj::Find(a.clone()),
            Obj::Std(n) => Obj::Std(*n),
            Obj::Mapping(a) => Obj::Mapping(a.clone()),
            Obj::Anon(a) => Obj::Anon(a.clone()),
        }
    }
}

struct Table {
    next: u32,
    objects: HashMap<u32, Obj>,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    let mut g = TABLE.lock().unwrap();
    let t = g.get_or_insert_with(|| Table { next: 0x100, objects: HashMap::new() });
    f(t)
}

pub fn insert(o: Obj) -> u32 {
    with(|t| {
        let h = t.next;
        t.next += 4;
        t.objects.insert(h, o);
        h
    })
}

pub fn get(h: u32) -> Option<Obj> {
    with(|t| t.objects.get(&h).cloned())
}

pub fn close(h: u32) -> bool {
    with(|t| t.objects.remove(&h).is_some())
}

/// Standard handles: stdin, stdout, stderr.
pub fn std_handle(which: u32) -> u32 {
    // -10, -11, -12 as unsigned.
    let n = match which {
        0xffff_fff6 => 0,
        0xffff_fff5 => 1,
        _ => 2,
    };
    with(|t| {
        for (&h, o) in t.objects.iter() {
            if let Obj::Std(k) = o {
                if *k == n {
                    return h;
                }
            }
        }
        let h = t.next;
        t.next += 4;
        t.objects.insert(h, Obj::Std(n));
        h
    })
}
