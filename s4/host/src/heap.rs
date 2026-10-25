//! The guest heap (`malloc`, `operator new`, `HeapAlloc`, `VirtualAlloc`):
//! 16-byte aligned blocks with an 8-byte size header, free lists by size class.

use s4_x86::Cpu;
use std::collections::BTreeMap;
use std::sync::Mutex;

pub const HEAP_START: u32 = 0x4000_0000;
pub const HEAP_END: u32 = 0x6000_0000;
/// Page-granular allocations (`VirtualAlloc`, thread stacks).
pub const VM_START: u32 = 0x6000_0000;
/// Page-granular allocations continue above the main thread's stack area when this range is full.
pub const VM_END: u32 = 0x7000_0000;
const VM_HIGH_START: u32 = 0x8000_0000;
const VM_HIGH_END: u32 = 0xf000_0000;

struct Heap {
    next: u32,
    free: BTreeMap<u32, Vec<u32>>,
    vm_next: u32,
    /// Blocks on the free lists (a block freed twice must not be handed out twice).
    freed: std::collections::BTreeSet<u32>,
    /// Size of each live page-granular block, and freed blocks by size (reused as they are).
    vm_sizes: BTreeMap<u32, u32>,
    vm_free: BTreeMap<u32, Vec<u32>>,
}

static HEAP: Mutex<Heap> = Mutex::new(Heap { next: HEAP_START, free: BTreeMap::new(), vm_next: VM_START, freed: std::collections::BTreeSet::new(), vm_sizes: BTreeMap::new(), vm_free: BTreeMap::new() });

pub fn init() {
    *HEAP.lock().unwrap() = Heap { next: HEAP_START, free: BTreeMap::new(), vm_next: VM_START, freed: std::collections::BTreeSet::new(), vm_sizes: BTreeMap::new(), vm_free: BTreeMap::new() };
}

fn class(size: u32) -> u32 {
    (size.max(16) + 15) & !15
}

/// Allocates `size` bytes (contents unspecified unless fresh).
pub fn alloc(c: &mut Cpu, size: u32) -> u32 {
    let sz = class(size);
    let mut h = HEAP.lock().unwrap();
    if let Some(list) = h.free.get_mut(&sz) {
        if let Some(p) = list.pop() {
            h.freed.remove(&p);
            if std::env::var_os("S4_ZERO_HEAP").is_some() {
                c.mem.fill(p, 0, sz as usize);
            }
            return p;
        }
    }
    let p = h.next + 16;
    h.next += 16 + sz;
    assert!(h.next < HEAP_END, "guest heap exhausted");
    c.mem.w32(p - 8, sz);
    p
}

pub fn alloc_zeroed(c: &mut Cpu, size: u32) -> u32 {
    let p = alloc(c, size);
    c.mem.fill(p, 0, size as usize);
    p
}

pub fn free(c: &mut Cpu, p: u32) {
    if p == 0 {
        return;
    }
    let sz = c.mem.r32(p - 8);
    let mut h = HEAP.lock().unwrap();
    if !h.freed.insert(p) {
        if std::env::var_os("S4_HEAP_TRACE").is_some() {
            eprintln!("heap: block {p:#x} freed twice (ignored)");
        }
        return;
    }
    h.free.entry(sz).or_default().push(p);
}

pub fn size(c: &Cpu, p: u32) -> u32 {
    c.mem.r32(p - 8)
}

pub fn realloc(c: &mut Cpu, p: u32, size: u32) -> u32 {
    if p == 0 {
        return alloc(c, size);
    }
    let old = self::size(c, p);
    if class(size) == old {
        return p;
    }
    let n = alloc(c, size);
    let bytes = c.mem.read_bytes(p, old.min(size) as usize);
    c.mem.write_bytes(n, &bytes);
    free(c, p);
    n
}

/// Page-aligned zeroed memory (`VirtualAlloc`).
pub fn vm_alloc(c: &mut Cpu, size: u32) -> u32 {
    let sz = (size + 0xfff) & !0xfff;
    let mut h = HEAP.lock().unwrap();
    let reused = h.vm_free.get_mut(&sz).and_then(|l| l.pop());
    let p = match reused {
        Some(p) => p,
        None => {
            if h.vm_next + sz > VM_END && h.vm_next < VM_HIGH_START {
                h.vm_next = VM_HIGH_START;
            }
            let p = h.vm_next;
            h.vm_next += sz;
            let end = if p >= VM_HIGH_START { VM_HIGH_END } else { VM_END };
            assert!(h.vm_next <= end, "guest virtual memory exhausted");
            p
        }
    };
    h.vm_sizes.insert(p, sz);
    c.mem.fill(p, 0, sz as usize);
    p
}

/// Gives a page-granular block back (`UnmapViewOfFile`, `VirtualFree`); the address is reused for a block of the same size.
pub fn vm_free(p: u32) {
    let mut h = HEAP.lock().unwrap();
    if let Some(sz) = h.vm_sizes.remove(&p) {
        h.vm_free.entry(sz).or_default().push(p);
    }
}

/// Memory for the host's own structures that live in guest memory (COM objects and their method tables): taken from
/// pages of their own, away from the guest's heap, so that a guest overrunning one of its blocks cannot reach them.
pub fn host_alloc(c: &mut Cpu, size: u32) -> u32 {
    static ARENA: Mutex<(u32, u32)> = Mutex::new((0, 0));
    let size = (size + 15) & !15;
    let mut a = ARENA.lock().unwrap();
    if a.0 + size > a.1 {
        let chunk = size.max(1 << 20);
        let p = vm_alloc(c, chunk);
        *a = (p, p + chunk);
    }
    let p = a.0;
    a.0 += size;
    p
}
