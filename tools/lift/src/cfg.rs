//! Finding a function's instructions: everything reachable from its entry
//! through jumps and fall-through (calls are not followed).

use crate::pe::Image;
use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register};
use std::collections::{BTreeMap, BTreeSet};

pub struct Function {
    pub entry: u32,
    pub insns: BTreeMap<u32, Instruction>,
    /// Addresses that start a block (jump targets, entry, after branches).
    pub leaders: BTreeSet<u32>,
    /// Jump table targets of the indirect jump at an address.
    pub tables: BTreeMap<u32, Vec<u32>>,
    pub problems: Vec<String>,
    /// Code addresses inside the function that its own code uses as data (a table of labels for computed jumps).
    pub labels: BTreeSet<u32>,
}

fn decode_at(img: &Image, addr: u32) -> Option<Instruction> {
    let code = img.code_from(addr)?;
    let mut d = Decoder::with_ip(32, &code[..code.len().min(15)], addr as u64, DecoderOptions::NONE);
    let ins = d.decode();
    if ins.is_invalid() { None } else { Some(ins) }
}

fn valid_target(img: &Image, f: &Function, t: u32) -> bool {
    let section = img.section_range(f.entry);
    section.map(|(lo, hi)| t >= lo && t < hi).unwrap_or(false) && img.is_code(t) && decode_at(img, t).is_some()
}

/// Entries of a jump table `jmp [table + index*scale]`: the code addresses
/// found from the displacement on, allowing holes (an entry nothing uses may
/// be junk) until four invalid dwords in a row. Over-reading into the next
/// table only adds unreachable blocks; stopping early would fail at run time.
fn table_targets(img: &Image, f: &Function, jmp: &Instruction) -> Vec<u32> {
    let table = jmp.memory_displacement32();
    let mut out = Vec::new();
    // Negative indices reach the dwords before the displacement (overlapping tables).
    let mut bad = 0;
    for i in 1..1024u32 {
        match img.read32(table.wrapping_sub(4 * i)) {
            Some(t) if valid_target(img, f, t) => {
                out.push(t);
                bad = 0;
            }
            _ => {
                bad += 1;
                if bad >= 4 {
                    break;
                }
            }
        }
    }
    let mut bad = 0;
    for i in 0..4096 {
        match img.read32(table + 4 * i) {
            Some(t) if valid_target(img, f, t) => {
                out.push(t);
                bad = 0;
            }
            _ => {
                bad += 1;
                if bad >= 4 {
                    break;
                }
            }
        }
    }
    out
}

/// Whether the indirect jump reads a table of code addresses of this function
/// (at least two of the first eight dwords are code addresses), not an import slot.
fn is_table(img: &Image, f: &Function, jmp: &Instruction) -> bool {
    let table = jmp.memory_displacement32();
    (-8i32..8).filter(|&i| img.read32(table.wrapping_add((4 * i) as u32)).map(|t| valid_target(img, f, t)).unwrap_or(false)).count() >= 2
}

/// Like [`discover`] with extra block starts (catch continuations found elsewhere).
pub fn discover_extra(img: &Image, entry: u32, other_entries: &BTreeSet<u32>, extra: &[u32]) -> Function {
    let mut roots = vec![entry];
    roots.extend_from_slice(extra);
    let f = discover_roots(img, entry, other_entries, &roots);
    finish(img, entry, other_entries, f)
}

/// A catch or unwind funclet: it is not re-entered at continuations of its own (the continuations it names belong to the
/// function that installed the exception frame), so the frame-linking heuristics of [`discover`] do not apply.
pub fn discover_funclet(img: &Image, entry: u32, other_entries: &BTreeSet<u32>) -> Function {
    discover_roots(img, entry, other_entries, &[entry])
}

pub fn discover(img: &Image, entry: u32, other_entries: &BTreeSet<u32>) -> Function {
    let f = discover_roots(img, entry, other_entries, &[entry]);
    finish(img, entry, other_entries, f)
}

fn discover_roots(img: &Image, entry: u32, other_entries: &BTreeSet<u32>, roots: &[u32]) -> Function {
    let mut f = Function { entry, insns: BTreeMap::new(), leaders: BTreeSet::new(), tables: BTreeMap::new(), problems: Vec::new(), labels: BTreeSet::new() };
    f.leaders.insert(entry);
    for &r in roots {
        f.leaders.insert(r);
    }
    let mut work: Vec<u32> = roots.to_vec();
    while let Some(start) = work.pop() {
        let mut a = start;
        loop {
            if f.insns.contains_key(&a) {
                // Falling into code another path already decoded (a different instruction
                // stream resynchronising, as after an over-wide jump table): a join point.
                f.leaders.insert(a);
                break;
            }
            let Some(ins) = decode_at(img, a) else {
                // Garbage reached through an over-wide jump table is harmless; only the entry matters.
                if a == entry {
                    f.problems.push(format!("cannot decode at {a:#x}"));
                }
                break;
            };
            f.insns.insert(a, ins);
            let next = ins.next_ip() as u32;
            match ins.flow_control() {
                FlowControl::Next => {}
                FlowControl::Call | FlowControl::IndirectCall => {}
                FlowControl::ConditionalBranch => {
                    let t = ins.near_branch_target() as u32;
                    f.leaders.insert(t);
                    f.leaders.insert(next);
                    work.push(t);
                    work.push(next);
                    break;
                }
                FlowControl::UnconditionalBranch => {
                    let t = ins.near_branch_target() as u32;
                    if t != entry && other_entries.contains(&t) && a < entry + 16 {
                        // Tail call into another function.
                    } else {
                        f.leaders.insert(t);
                        work.push(t);
                    }
                    break;
                }
                FlowControl::IndirectBranch => {
                    if ins.op0_kind() == OpKind::Memory && (ins.memory_index() != Register::None || ins.memory_base() != Register::None) && is_table(img, &f, &ins) {
                        let ts = table_targets(img, &f, &ins);
                        for &t in &ts {
                            f.leaders.insert(t);
                            work.push(t);
                        }
                        f.tables.insert(a, ts);
                    }
                    break;
                }
                FlowControl::Return | FlowControl::Exception | FlowControl::Interrupt | FlowControl::XbeginXabortXend => break,
            }
            if ins.mnemonic() == Mnemonic::Call && ins.near_branch_target() as u32 == next {
                // call $+5; pop reg: not a call.
            }
            a = next;
            if f.leaders.contains(&a) {
                // Falling into an already-known block start: keep decoding
                // only if not yet seen (handled by the contains check).
            }
        }
    }
    f
}

fn finish(img: &Image, entry: u32, other_entries: &BTreeSet<u32>, f: Function) -> Function {
    let mut f = finish_seh(img, entry, other_entries, f);
    // After `call _setjmp3` the code can be re-entered by a `longjmp` from deeper in the stack.
    for r in setjmp_returns(img, &f) {
        f.leaders.insert(r);
    }
    f
}

/// Addresses after the calls the function makes to `setjmp` (directly or through an import thunk).
pub fn setjmp_returns(img: &Image, f: &Function) -> Vec<u32> {
    let is_setjmp = |slot: u32| img.imports.get(&slot).is_some_and(|l| l.to_ascii_lowercase().contains("setjmp"));
    let mut out = Vec::new();
    for ins in f.insns.values() {
        if ins.mnemonic() != Mnemonic::Call {
            continue;
        }
        let hit = match ins.op0_kind() {
            OpKind::NearBranch32 => decode_at(img, ins.near_branch_target() as u32).is_some_and(|t| {
                t.mnemonic() == Mnemonic::Jmp && t.op0_kind() == OpKind::Memory && t.memory_base() == Register::None && t.memory_index() == Register::None && is_setjmp(t.memory_displacement32())
            }),
            OpKind::Memory => ins.memory_base() == Register::None && ins.memory_index() == Register::None && is_setjmp(ins.memory_displacement32()),
            _ => false,
        };
        if hit {
            out.push(ins.next_ip() as u32);
        }
    }
    out
}

fn finish_seh(img: &Image, entry: u32, other_entries: &BTreeSet<u32>, f: Function) -> Function {
    // Functions that install an exception frame can be re-entered at a catch
    // continuation: code addresses used as immediates nearby are blocks too.
    if f.has_seh() {
        let mut extra = Vec::new();
        for ins in f.insns.values() {
            for k in 0..ins.op_count() {
                if ins.op_kind(k) == OpKind::Immediate32 {
                    let v = ins.immediate(k) as u32;
                    if v >= entry && v < entry + 0x10000 && img.is_code(v) && !f.insns.contains_key(&v) && !other_entries.contains(&v) {
                        extra.push(v);
                    }
                }
            }
        }
        if !extra.is_empty() {
            let more = discover_from(img, entry, other_entries, &extra, f);
            return more;
        }
    }
    f
}

impl Function {
    /// Whether the function writes `fs:[0]` (links an exception registration).
    pub fn has_seh(&self) -> bool {
        self.insns.values().any(|i| i.segment_prefix() == Register::FS && i.mnemonic() == Mnemonic::Mov && i.op0_kind() == OpKind::Memory && i.memory_displacement32() == 0 && i.memory_base() == Register::None)
    }
}

/// Continues the discovery of `f` from additional block starts.
fn discover_from(img: &Image, entry: u32, other_entries: &BTreeSet<u32>, starts: &[u32], mut f: Function) -> Function {
    let mut seeds: BTreeSet<u32> = starts.iter().copied().collect();
    for &a in &seeds {
        f.leaders.insert(a);
    }
    // Reuse the walker: run it with the extra starts as roots, keeping what is known.
    let mut g = discover_roots(img, entry, other_entries, &seeds.iter().copied().collect::<Vec<_>>());
    for (a, i) in std::mem::take(&mut g.insns) {
        f.insns.entry(a).or_insert(i);
    }
    f.leaders.extend(g.leaders);
    f.tables.extend(g.tables);
    seeds.clear();
    f
}
