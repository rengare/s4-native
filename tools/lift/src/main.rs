//! lift <binary> <functions.tsv> <out_dir> [--prefix f_] [--only addr,addr,...] [--module name] [--patch file]
//!
//! `--patch file`: lines `<address hex><TAB><old bytes hex><TAB><new bytes hex>` (`#` starts a comment) are applied to
//! the image before it is translated. Every old byte string is checked; any mismatch skips the whole file.
//!
//! Writes `<out_dir>/lifted_NNN.rs` (the functions), `<out_dir>/dispatch.rs`
//! (address -> function) and `<out_dir>/unsupported.txt`.

use lift::{cfg, eh, fn_name, header, lift_function, pe};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "cases").unwrap_or(false) {
        lift::cases::generate(&args[2..], &args[1]);
        return;
    }
    if args.len() < 3 {
        eprintln!("usage: lift <binary> <functions.tsv> <out_dir> [--prefix f_] [--only a,b]");
        std::process::exit(2);
    }
    let (binary, tsv, out_dir) = (&args[0], &args[1], &args[2]);
    let mut prefix = "f_".to_string();
    let mut only: Option<BTreeSet<u32>> = None;
    let mut patch: Option<String> = None;
    // `--parts <dir prefix> <n>`: the functions go to n crates (`<prefix>00` ..) that rustc compiles in parallel.
    let mut parts: Option<(String, usize)> = None;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--prefix" => {
                prefix = args[i + 1].clone();
                i += 2;
            }
            "--patch" => {
                patch = Some(args[i + 1].clone());
                i += 2;
            }
            "--parts" => {
                parts = Some((args[i + 1].clone(), args[i + 2].parse().unwrap()));
                i += 3;
            }
            "--only" => {
                only = Some(args[i + 1].split(',').map(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap()).collect());
                i += 2;
            }
            a => panic!("unknown argument {a}"),
        }
    }
    let mut img = pe::Image::load(binary);
    if let Some(file) = &patch {
        apply_patches(&mut img, file);
    }
    let mut entries = BTreeSet::new();
    let mut catch_funclets: BTreeSet<u32> = BTreeSet::new();
    let have_list = std::fs::metadata(tsv).is_ok();
    if let Ok(text) = std::fs::read_to_string(tsv) {
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            let a = u32::from_str_radix(f[0], 16).unwrap();
            entries.insert(a);
            // The Ghidra export has the name in column 3; the committed lists have a kind in column 2.
            if f.get(2).or(f.get(1)).map(|n| n.starts_with("Catch@")).unwrap_or(false) {
                catch_funclets.insert(a);
            }
        }
    }
    // Without Ghidra's list the exports and the entry point seed the search.
    for (_, a) in &img.exports {
        if img.is_code(*a) {
            entries.insert(*a);
        }
    }
    if img.entry != 0 && img.is_code(img.entry) {
        entries.insert(img.entry);
    }
    // The exception-handling tables name funclets and handler stubs that nothing calls directly.
    let ehi = eh::scan(&img);
    for &a in ehi.unwind_actions.iter().chain(&ehi.catch_handlers).chain(&ehi.stubs) {
        entries.insert(a);
    }
    if !have_list {
        // Unwind actions are funclets too (some end by returning the address to resume at, like catch handlers).
        catch_funclets.extend(ehi.catch_handlers.iter().chain(&ehi.unwind_actions).copied());
    }
    // Entry points: the function list's functions, plus what they call directly and every code
    // address the image's relocations point at, that no known function contains.
    let mut base_entries = entries.clone();
    let mut hard: BTreeSet<u32> = entries.clone();
    let mut entries = entries;
    let mut covered: BTreeSet<u32> = BTreeSet::new();
    let mut table_targets: BTreeSet<u32> = BTreeSet::new();
    let mut seh_entries: BTreeSet<u32> = BTreeSet::new();
    let mut seh_handlers: Vec<(u32, u32)> = Vec::new();
    // Which function's flow each instruction address was found in (the first), for telling labels from functions.
    let mut insn_owner: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    // (function, address): the function's code uses a code address as an immediate.
    let mut imm_refs: Vec<(u32, u32)> = Vec::new();
    // The address span (first and last instruction) of each function found.
    let mut spans: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    // Functions with a computed jump whose targets are unknown (no jump table found): code only that jump reaches may lie in them.
    let mut computed_jumps: BTreeSet<u32> = BTreeSet::new();
    let mut immediates: BTreeSet<u32> = BTreeSet::new();
    // Code addresses used as immediates inside catch handlers: where the handler resumes.
    let mut resume_points: BTreeSet<u32> = BTreeSet::new();
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut pending: Vec<u32> = entries.iter().copied().collect();
    let mut candidates: BTreeSet<u32> = img.relocs.iter().filter_map(|&r| img.read32(r)).filter(|&t| img.is_code(t)).collect();
    // Relocated pointers in data name functions (vtables, callbacks); those in code are immediates, which may be labels.
    let data_candidates: BTreeSet<u32> = img.relocs.iter().filter(|&&r| !img.is_code(r)).filter_map(|&r| img.read32(r)).filter(|&t| img.is_code(t)).collect();
    if img.relocs.is_empty() {
        // No relocation table (the program): every aligned dword in the data sections that points into code.
        for sec in img.sections.iter().filter(|s| s.flags & 0x2000_0000 == 0) {
            let at = img.base + sec.rva;
            for off in (0..sec.data.len().saturating_sub(3)).step_by(4) {
                let v = u32::from_le_bytes(sec.data[off..off + 4].try_into().unwrap());
                if img.is_code(v) {
                    candidates.insert(v);
                }
            }
            let _ = at;
        }
    }
    // Instruction-pointer-to-state maps hold addresses inside functions.
    candidates.retain(|a| !ehi.ip_map.contains(a));
    let mut cand_pending: Vec<u32> = candidates.iter().copied().collect();
    if img.relocs.is_empty() {
        hard.extend(candidates.iter().copied());
    } else {
        hard.extend(data_candidates.iter().copied());
    }
    loop {
        while let Some(a) = pending.pop() {
            if !seen.insert(a) {
                continue;
            }
            let f = cfg::discover(&img, a, &entries);
            if f.has_seh() {
                seh_entries.insert(a);
            }
            for ts in f.tables.values() {
                table_targets.extend(ts.iter().copied());
            }
            // SEH prologue: push <scope table>; push <handler>; mov eax, fs:[0]. The table's __except
            // handlers are blocks of this function (the exception dispatcher jumps into them).
            {
                let v: Vec<&iced_x86::Instruction> = f.insns.values().collect();
                for w in v.windows(3) {
                    let imm = |i: &iced_x86::Instruction| (i.mnemonic() == iced_x86::Mnemonic::Push && i.op0_kind() == iced_x86::OpKind::Immediate32).then(|| i.immediate(0) as u32);
                    let (Some(table), Some(h)) = (imm(w[0]), imm(w[1])) else { continue };
                    if have_list {
                        continue;
                    }
                    if !img.is_code(h) || img.is_code(table) || !img.in_image(table) || w[2].mnemonic() != iced_x86::Mnemonic::Mov {
                        continue;
                    }
                    for i in 0..64u32 {
                        let at = table + 12 * i;
                        let (Some(prev), Some(filter), Some(handler)) = (img.read32(at), img.read32(at + 4), img.read32(at + 8)) else { break };
                        if !(prev == 0xffff_ffff || prev < i) || !img.is_code(handler) || !(filter == 0 || filter == 1 || img.is_code(filter)) {
                            break;
                        }
                        seh_handlers.push((a, handler));
                    }
                }
            }
            if let (Some((&lo, _)), Some((&hi, _))) = (f.insns.iter().next(), f.insns.iter().next_back()) {
                spans.insert(a, (lo, hi));
            }
            if f.insns.values().any(|i| i.flow_control() == iced_x86::FlowControl::IndirectBranch && i.mnemonic() == iced_x86::Mnemonic::Jmp && !f.tables.contains_key(&(i.ip() as u32)) && !(i.op0_kind() == iced_x86::OpKind::Memory && i.memory_base() == iced_x86::Register::None && i.memory_index() == iced_x86::Register::None)) {
                computed_jumps.insert(a);
            }
            for (&ia, ins) in &f.insns {
                covered.insert(ia);
                insn_owner.entry(ia).or_default().push(a);
                for k in 0..ins.op_count() {
                    if ins.op_kind(k) == iced_x86::OpKind::Immediate32 {
                        let v = ins.immediate(k) as u32;
                        if img.is_code(v) {
                            imm_refs.push((a, v));
                            cand_pending.push(v);
                            immediates.insert(v);
                            if catch_funclets.contains(&a) {
                                resume_points.insert(v);
                            }
                        }
                    }
                }
                if ins.mnemonic() == iced_x86::Mnemonic::Call && ins.near_branch_target() as u32 != ins.next_ip() as u32 && ins.op0_kind() != iced_x86::OpKind::Memory && ins.op0_kind() != iced_x86::OpKind::Register {
                    let t = ins.near_branch_target() as u32;
                    if img.is_code(t) {
                        hard.insert(t);
                    }
                    if img.is_code(t) && entries.insert(t) {
                        pending.push(t);
                    }
                }
            }
        }
        // Relocated code pointers that are not inside any function found so far.
        let mut added = false;
        table_targets.extend(seh_handlers.iter().map(|&(_, h)| h));
        while let Some(t) = cand_pending.pop() {
            if !table_targets.contains(&t) && !entries.contains(&t) {
                entries.insert(t);
                pending.push(t);
                added = true;
            }
        }
        if !added && pending.is_empty() {
            break;
        }
    }
    // Without a function list the boundaries are what the image itself shows: exports, direct
    // calls, code pointers in data (minus jump-table case labels) and the exception tables' funclets.
    if !have_list {
        base_entries = hard.difference(&table_targets).copied().collect();
    }
    // A code address used as an immediate inside the range of a function with an exception
    // frame is a catch continuation: a block of that function, not a function.
    let mut extras: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    // With a function list any such immediate inside a function with an exception frame is taken as a
    // continuation (the list tells where functions start); without one only the resume points of
    // catch handlers are, which the exception tables identify exactly.
    let continuation_candidates = if have_list { immediates.clone() } else { resume_points.clone() };
    let debug: Option<u32> = std::env::var("LIFT_DEBUG").ok().and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok());
    if let Some(d) = debug {
        eprintln!("debug {d:#x}: candidate {} immediate {} resume {} catch_funclet {} in base_entries {} seh_entries near: {:?}", continuation_candidates.contains(&d), immediates.contains(&d), resume_points.contains(&d), catch_funclets.contains(&d), base_entries.contains(&d), base_entries.range(..=d).rev().take(3).map(|p| (*p, seh_entries.contains(p), catch_funclets.contains(p))).collect::<Vec<_>>());
    }
    for &t in &continuation_candidates {
        // Function starts follow alignment padding; continuations follow code.
        if matches!(img.read8(t.wrapping_sub(1)), Some(0x90) | Some(0xcc)) {
            continue;
        }
        // The parent: the nearest earlier function that is not a catch funclet; the
        // address must lie before the next ordinary function.
        let mut it = base_entries.range(..=t).rev().filter(|e| !catch_funclets.contains(e));
        if let Some(&p) = it.next() {
            let next = base_entries.range(t + 1..).find(|e| !catch_funclets.contains(e)).copied().unwrap_or(u32::MAX);
            if p != t && seh_entries.contains(&p) && t < next {
                extras.entry(p).or_default().push(t);
            }
        }
    }
    // A code address used as an immediate (no call, export or data pointer names it a function) that lies inside the code
    // of another function is a label of that function: a computed jump goes there.
    let mut labels_of: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    // Only for DLLs (an image with a relocation table: hand-written assembly with computed jumps, like a video codec).
    // In a program, code addresses used as immediates are callbacks and the like that really are functions.
    let find_labels = !have_list && !img.relocs.is_empty();
    if let Some(d) = std::env::var("LIFT_DEBUG").ok().and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()) {
        eprintln!("label debug {d:#x}: immediate {} hard {} table_target {} entry {} owner {:?} have_list {have_list}", immediates.contains(&d), hard.contains(&d), table_targets.contains(&d), entries.contains(&d), insn_owner.get(&d));
    }
    if find_labels {
        let mut owner_of: BTreeMap<u32, u32> = BTreeMap::new();
        for &v in &immediates {
            if hard.contains(&v) || table_targets.contains(&v) || !entries.contains(&v) {
                continue;
            }
            // Inside the flow of another function, or between its first and last instruction (code that only a computed
            // jump reaches lies there): a label of that function.
            // The function that names it and spans it (it keeps the label for the jump that comes back, from deeper in the stack).
            let by_ref = imm_refs.iter().filter(|&&(a, w)| w == v && a != v && entries.contains(&a)).map(|&(a, _)| a).find(|a| spans.get(a).is_some_and(|&(lo, hi)| lo < v && v < hi));
            // Or that names it and has a computed jump, the code past its end (a codec that fills a table of its own entry points
            // and jumps through it).
            let by_ref_jump = || imm_refs.iter().filter(|&&(a, w)| w == v && a != v && entries.contains(&a) && computed_jumps.contains(&a)).map(|&(a, _)| a).min();
            let by_flow = || insn_owner.get(&v).and_then(|os| os.iter().copied().find(|&o| o != v && entries.contains(&o)));
            let by_span = || spans.iter().filter(|&(&o, &(lo, hi))| o != v && computed_jumps.contains(&o) && entries.contains(&o) && lo < v && v < hi).min_by_key(|&(_, &(lo, hi))| hi - lo).map(|(&o, _)| o);
            if let Some(o) = by_ref.or_else(by_ref_jump).or_else(by_flow).or_else(by_span) {
                if std::env::var("LIFT_DEBUG").ok().and_then(|d| u32::from_str_radix(d.trim_start_matches("0x"), 16).ok()) == Some(v) {
                    eprintln!("label {v:#x} absorbed by {o:#x} (span {:?}, computed jumps {})", spans.get(&o), computed_jumps.contains(&o));
                }
                owner_of.insert(v, o);
            }
        }
        // The owner may itself be a label of another function (a codec's table of entry points: each one's flow runs into
        // the next): the label goes to the end of that chain, the function that stays one (in a cycle, its lowest address).
        for &v in owner_of.keys() {
            let mut chain = vec![v];
            let mut o = owner_of[&v];
            while let Some(&next) = owner_of.get(&o) {
                if chain.contains(&o) {
                    break;
                }
                chain.push(o);
                o = next;
            }
            let root = if owner_of.contains_key(&o) { chain.iter().copied().skip_while(|&a| a != o).min().unwrap_or(o) } else { o };
            if debug == Some(v) {
                eprintln!("label {v:#x}: chain {chain:x?}, root {root:#x}");
            }
            if root != v {
                extras.entry(root).or_default().push(v);
                labels_of.entry(root).or_default().insert(v);
            }
        }
    }
    // The __except handlers of SEH scope tables are blocks of the function that installs the frame.
    for &(p, h) in seh_handlers.iter().filter(|_| !have_list) {
        if h != p && !extras.get(&p).is_some_and(|l| l.contains(&h)) {
            extras.entry(p).or_default().push(h);
        }
    }
    for list in extras.values() {
        for t in list {
            entries.remove(t);
        }
    }
    let targets: Vec<u32> = entries.iter().copied().filter(|a| only.as_ref().map(|o| o.contains(a)).unwrap_or(true)).collect();
    let all: BTreeSet<u32> = targets.iter().copied().collect();
    // Pass 1: which functions lift at all.
    let mut funs = BTreeMap::new();
    let mut failed: BTreeMap<u32, String> = BTreeMap::new();
    let mut gaps: Vec<String> = Vec::new();
    for &a in &targets {
        if std::env::var("LIFT_TRACE").is_ok() {
            eprintln!("{a:08x}");
        }
        let f = if !have_list && catch_funclets.contains(&a) {
            cfg::discover_funclet(&img, a, &entries)
        } else {
            cfg::discover_extra(&img, a, &entries, extras.get(&a).map(|v| v.as_slice()).unwrap_or(&[]))
        };
        if std::env::var("LIFT_TRACE").is_ok() {
            eprintln!("discovered {} insns", f.insns.len());
        }
        let mut f = f;
        if let Some(l) = labels_of.get(&a) {
            f.labels = l.clone();
        }
        match lift_function(&img, &f, &all, &prefix) {
            Ok((_, g)) => {
                for line in g {
                    gaps.push(format!("{a:08x}\t{line}"));
                }
                funs.insert(a, f);
            }
            Err(e) => {
                failed.insert(a, e.0);
            }
        }
    }
    let ok: BTreeSet<u32> = funs.keys().copied().collect();
    // Pass 2: with direct calls only to what exists.
    std::fs::create_dir_all(out_dir).unwrap();
    let mut items: Vec<Item> = Vec::new();
    // Block starts of lifted functions that are not function entries: the original calls some of them
    // as functions (a decompiler's list knows them; the image alone does not), so they dispatch into
    // the function that contains them.
    let mut block_entries: BTreeSet<u32> = BTreeSet::new();
    for (&a, f) in &funs {
        let text = lift_function(&img, f, &ok, &prefix).unwrap().0;
        let mut dispatch = String::new();
        writeln!(dispatch, "        {a:#x} => {{ {}(c); true }}", fn_name(&prefix, a)).unwrap();
        // Only blocks that start right after a return or an unconditional jump (or padding) look like
        // function starts.
        let after_exit: BTreeSet<u32> = f
            .insns
            .values()
            .filter(|i| matches!(i.flow_control(), iced_x86::FlowControl::Return | iced_x86::FlowControl::UnconditionalBranch | iced_x86::FlowControl::IndirectBranch | iced_x86::FlowControl::Exception))
            .map(|i| i.next_ip() as u32)
            .collect();
        for &l in &f.leaders {
            let function_like = after_exit.contains(&l) || matches!(img.read8(l.wrapping_sub(1)), Some(0x90) | Some(0xcc));
            if l != a && function_like && !f.labels.contains(&l) && !ok.contains(&l) && f.insns.contains_key(&l) && block_entries.insert(l) {
                writeln!(dispatch, "        {l:#x} => {{ {}_at(c, {l:#x}); true }}", fn_name(&prefix, a)).unwrap();
            }
        }
        items.push(Item { addr: a, text, dispatch });
    }
    match &parts {
        None => write_crate(out_dir, &items, &[]),
        Some((dir_prefix, n)) => write_parts(out_dir, dir_prefix, *n, &items, &prefix, &ok),
    }
    let mut report = String::new();
    for (a, why) in &failed {
        writeln!(report, "{a:08x}\t{why}").unwrap();
    }
    std::fs::write(format!("{out_dir}/unsupported.txt"), report).unwrap();
    std::fs::write(format!("{out_dir}/gaps.txt"), gaps.join("\n")).unwrap();
    eprintln!("lifted {} of {} functions ({} unsupported)", ok.len(), targets.len(), failed.len());
}

/// One lifted function: its source and its lines of the dispatch table.
struct Item {
    addr: u32,
    text: String,
    dispatch: String,
}

/// Writes the modules of one crate (`lifted_NNN.rs`, 100 functions each) and its `lib.rs`: `extra` goes in front
/// of the modules (declarations of functions in other crates), then the dispatch function.
fn write_crate(out_dir: &str, items: &[Item], extra: &[String]) {
    std::fs::create_dir_all(out_dir).unwrap();
    let mut lib = String::from("// Generated by tools/lift; do not edit.\n");
    for l in extra {
        lib.push_str(l);
        lib.push('\n');
    }
    let mut dispatch = String::new();
    for (n, chunk) in items.chunks(100).enumerate() {
        let body: String = chunk.iter().map(|i| i.text.as_str()).collect();
        std::fs::write(format!("{out_dir}/lifted_{n:03}.rs"), format!("// Generated by tools/lift; do not edit.\n{}{}", header(), body)).unwrap();
        writeln!(lib, "mod lifted_{n:03};\npub use lifted_{n:03}::*;").unwrap();
    }
    for i in items {
        dispatch.push_str(&i.dispatch);
    }
    writeln!(lib, "\n/// Calls the lifted function at `addr`, if there is one.\npub fn dispatch(c: &mut s4_x86::Cpu, addr: u32) -> bool {{\n    match addr {{\n{dispatch}        _ => false,\n    }}\n}}").unwrap();
    std::fs::write(format!("{out_dir}/lib.rs"), lib).unwrap();
}

/// The addresses of the lifted functions that `text` calls directly.
fn callees(text: &str, prefix: &str) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    let mut at = 0;
    while let Some(p) = text[at..].find(prefix) {
        let s = at + p + prefix.len();
        at = s;
        if let (Some(h), Some(rest)) = (text.get(s..s + 8), text.get(s + 8..s + 11)) {
            if rest == "(c)" {
                if let Ok(a) = u32::from_str_radix(h, 16) {
                    out.insert(a);
                }
            }
        }
    }
    out
}

/// Splits the functions into `n` crates of about the same size (in address order). Calls between crates go through
/// `extern "Rust"` declarations of the exported functions, which the linker resolves (crates cannot depend on each
/// other in a circle). `out_dir` gets the crate with the `dispatch` function that picks the part by address.
fn write_parts(out_dir: &str, dir_prefix: &str, n: usize, items: &[Item], prefix: &str, ok: &BTreeSet<u32>) {
    let base = dir_prefix.rsplit('/').next().unwrap();
    let total: usize = items.iter().map(|i| i.text.len()).sum();
    let per = total / n + 1;
    let mut groups: Vec<Vec<usize>> = vec![Vec::new()];
    let mut size = 0;
    for (k, i) in items.iter().enumerate() {
        if size >= per && groups.len() < n {
            groups.push(Vec::new());
            size = 0;
        }
        groups.last_mut().unwrap().push(k);
        size += i.text.len();
    }
    let mut ranges = Vec::new();
    let owner: BTreeMap<u32, usize> = groups.iter().enumerate().flat_map(|(p, g)| g.iter().map(move |&k| (items[k].addr, p))).collect();
    for p in 0..n {
        let dir = format!("{dir_prefix}{p:02}");
        let src = format!("{dir}/src");
        let _ = std::fs::remove_dir_all(&src);
        let manifest = format!("{dir}/Cargo.toml");
        if std::fs::metadata(&manifest).is_err() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&manifest, part_manifest(base, p)).unwrap();
        }
        let Some(g) = groups.get(p) else {
            write_crate(&src, &[], &[]);
            continue;
        };
        let part_items: Vec<Item> = g
            .iter()
            .map(|&k| {
                let it = &items[k];
                let pat = format!("pub fn {}(c: &mut Cpu) {{ ", fn_name(prefix, it.addr));
                Item { addr: it.addr, text: it.text.replacen(&pat, &format!("#[unsafe(no_mangle)] {pat}"), 1), dispatch: it.dispatch.clone() }
            })
            .collect();
        let mut outside = BTreeSet::new();
        for it in &part_items {
            for a in callees(&it.text, prefix) {
                if ok.contains(&a) && owner.get(&a) != Some(&p) {
                    outside.insert(a);
                }
            }
        }
        let mut ext = vec!["unsafe extern \"Rust\" {".to_string()];
        ext.extend(outside.iter().map(|&a| format!("    pub safe fn {}(c: &mut s4_x86::Cpu);", fn_name(prefix, a))));
        ext.push("}".to_string());
        write_crate(&src, &part_items, &ext);
        let first = part_items[0].addr;
        let end = groups.get(p + 1).map_or(u32::MAX, |g| items[g[0]].addr - 1);
        ranges.push((p, first, end));
    }
    let mut lib = String::from("// Generated by tools/lift; do not edit.\n\n/// Calls the lifted function at `addr`, if there is one: the part that holds the address does it.\npub fn dispatch(c: &mut s4_x86::Cpu, addr: u32) -> bool {\n    match addr {\n");
    for (p, lo, hi) in ranges {
        writeln!(lib, "        {lo:#x}..={hi:#x} => lifted_{}_p{p:02}::dispatch(c, addr),", base.trim_end_matches("-p").replace('-', "_")).unwrap();
    }
    lib.push_str("        _ => false,\n    }\n}\n");
    std::fs::create_dir_all(out_dir).unwrap();
    std::fs::write(format!("{out_dir}/lib.rs"), lib).unwrap();
}

fn part_manifest(base: &str, p: usize) -> String {
    let name = base.trim_end_matches("-p");
    format!(
        "[package]\nname = \"lifted-{name}-p{p:02}\"\nversion = \"0.1.0\"\nedition = \"2024\"\ndescription = \"A part of the lifted {name} (generated from the user's copy of the game)\"\n\n[lib]\npath = \"src/lib.rs\"\n\n[dependencies]\ns4-x86 = {{ path = \"../../../s4/x86\" }}\ns4-x87 = {{ path = \"../../../s4/x87\" }}\n"
    )
}

/// Applies a patch file (see the header). All or nothing: the original image is kept on any error.
fn apply_patches(img: &mut pe::Image, file: &str) {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{file}: {e}"));
    let hex = |s: &str| -> Vec<u8> { (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap_or_else(|_| panic!("{file}: bad hex {s:?}"))).collect() };
    let mut list = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        assert!(f.len() >= 3, "{file}: a patch line needs address, old bytes, new bytes");
        list.push((u32::from_str_radix(f[0].trim_start_matches("0x"), 16).unwrap(), hex(f[1].trim()), hex(f[2].trim())));
    }
    let mut done: Vec<&(u32, Vec<u8>, Vec<u8>)> = Vec::new();
    for p in &list {
        match img.patch(p.0, &p.1, &p.2) {
            Ok(()) => done.push(p),
            Err(e) => {
                // Undo what was applied: the image stays as loaded.
                for (a, old, new) in done.iter().rev().map(|p| (p.0, &p.1, &p.2)) {
                    let _ = img.patch(a, new, old);
                }
                eprintln!("patch file {file} not applied: {e}");
                return;
            }
        }
    }
    eprintln!("applied {} patches from {file}", done.len());
}
