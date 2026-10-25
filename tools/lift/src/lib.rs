//! Lifts the original binaries' functions to Rust, instruction by
//! instruction (see `s4/docs/lift.md`).

pub mod cases;
pub mod cfg;
pub mod eh;
pub mod emit;
pub mod pe;

use emit::{Ctx, Em, Unsupported};
use iced_x86::{ConditionCode, FlowControl, Formatter, Instruction, IntelFormatter, Mnemonic, OpKind, Register};
use std::collections::BTreeSet;
use std::fmt::Write;

pub fn text(ins: &Instruction) -> String {
    let mut f = IntelFormatter::new();
    let mut s = String::new();
    f.format(ins, &mut s);
    s
}

const HEADER: &str = "#![allow(unused_mut, unused_variables, unused_assignments, unused_parens, unreachable_code, unused_macros, unused_imports, unused_braces, non_snake_case, clippy::all)]\n\
use super::*;\n\
use s4_x86::{alu, Cpu, Fl};\n\
use s4_x86::fpu::Arith;\n\
use s4_x86::mmx;\n\
use s4_x86::string::{self, Rep};\n\
use s4_x87::{Constant, Value};\n\n";

pub fn header() -> &'static str {
    HEADER
}

pub fn fn_name(prefix: &str, addr: u32) -> String {
    format!("{prefix}{addr:08x}")
}

/// Rust source of one function.
pub fn lift_function(img: &pe::Image, fun: &cfg::Function, lifted: &BTreeSet<u32>, prefix: &str) -> Result<(String, Vec<String>), Unsupported> {
    if !fun.problems.is_empty() {
        return Err(Unsupported(fun.problems.join("; ")));
    }
    // Functions with labels are re-entered at them by computed jumps from deeper in the stack (GuestJump), like catch continuations.
    let wrapped = fun.has_seh() || !cfg::setjmp_returns(img, fun).is_empty() || !fun.labels.is_empty();
    let cx = Ctx { img, fun, lifted, prefix };
    let mut em = Em { cx: &cx, out: String::new(), disp: None };
    let name = fn_name(prefix, fun.entry);
    let mut body = String::new();
    let mut insns = fun.insns.values().peekable();
    let mut open = false;
    let mut gaps: Vec<String> = Vec::new();
    while let Some(ins) = insns.next() {
        let a = ins.ip() as u32;
        if fun.leaders.contains(&a) {
            if open {
                writeln!(em.out, "pc = {a:#x}; continue; }}").unwrap();
            }
            writeln!(em.out, "{a:#x} => {{").unwrap();
            open = true;
        } else if !open {
            // Code that nothing jumps to (an overlapping decoding): its own arm.
            writeln!(em.out, "{a:#x} => {{").unwrap();
            open = true;
        }
        let next = ins.next_ip() as u32;
        let mark = em.out.len();
        let terminated = match terminator(&mut em, ins, fun) {
            Ok(t) => t,
            Err(e) => {
                // An instruction we cannot lift becomes a trap that fires only if it is executed.
                em.out.truncate(mark);
                gaps.push(format!("{:08x} {}: {}", ins.ip(), text(ins), e.0));
                writeln!(em.out, "// {:08x} {}\npanic!(\"lift gap: {} at {:#x}\");", ins.ip(), text(ins), text(ins).replace('"', "'"), ins.ip()).unwrap();
                true
            }
        };
        if terminated {
            writeln!(em.out, "}}").unwrap();
            open = false;
        } else if insns.peek().map(|n| n.ip() as u32 != next).unwrap_or(true) {
            // The following code was never reached (or is another block
            // start that falls off the decoded part).
            if fun.insns.contains_key(&next) {
                // Decoded by another instruction stream that overlaps this one.
                writeln!(em.out, "pc = {next:#x}; continue; }}").unwrap();
            } else {
                writeln!(em.out, "panic!(\"fell off the decoded code at {next:#x}\"); }}").unwrap();
            }
            open = false;
        }
    }
    if open {
        writeln!(em.out, "panic!(\"fell off the end\"); }}").unwrap();
    }
    write!(
        body,
        "pub fn {name}(c: &mut Cpu) {{ {name}_at(c, {entry:#x}); }}\n\n\
         /// Runs the function from the block that starts at `pc0` (its entry, or any other block start).\n\
         pub fn {name}_at(c: &mut Cpu, pc0: u32) {{\n\
         let m = c.mem;\n\
         let (mut eax, mut ecx, mut edx, mut ebx, mut esp, mut ebp, mut esi, mut edi) = (c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi);\n\
         let mut fl = c.fl;\n\
         macro_rules! st {{ () => {{ c.eax = eax; c.ecx = ecx; c.edx = edx; c.ebx = ebx; c.esp = esp; c.ebp = ebp; c.esi = esi; c.edi = edi; c.fl = fl; }} }}\n\
         macro_rules! ld {{ () => {{ eax = c.eax; ecx = c.ecx; edx = c.edx; ebx = c.ebx; esp = c.esp; ebp = c.ebp; esi = c.esi; edi = c.edi; fl = c.fl; }} }}\n\
         let ra0 = m.r32(esp);\n\
         {trace_enter}\

         let mut pc: u32 = pc0;\n\
         {open}loop {{ match pc {{\n{blocks}\
         _ => unreachable!(\"pc {{pc:#x}}\"),\n\
         }} }}{close}\n}}\n\n",
        entry = fun.entry,
        open = if wrapped { "loop {\nlet r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {\n" } else { "" },
        close = if wrapped {
            let leaders: Vec<String> = fun.leaders.iter().map(|l| format!("{l:#x}")).collect();
            format!(
                "\n}}));\nmatch r {{\nOk(()) => return,\nErr(p) => match p.downcast::<s4_x86::GuestJump>() {{\nOk(j) if matches!(j.target, {}) => {{ ld!(); pc = j.target; }}\nOk(j) => std::panic::resume_unwind(j),\nErr(p) => std::panic::resume_unwind(p),\n}},\n}}\n}}",
                leaders.join(" | ")
            )
        } else {
            String::new()
        },
        trace_enter = if std::env::var_os("LIFT_TRACE_CALLS").is_some() { format!("eprintln!(\"enter {name} esp={{esp:#x}} ra={{ra0:#x}}\");\n") } else { String::new() },
        blocks = em.out
    )
    .unwrap();
    Ok((body, gaps))
}

/// A function body for one instruction, for the conformance tests: it loads
/// the state, runs the instruction's code and stores the state.
pub fn lift_single(name: &str, ins: &Instruction) -> Result<String, Unsupported> {
    let img = pe::Image::empty();
    let fun = cfg::Function { entry: 0, insns: Default::default(), leaders: Default::default(), tables: Default::default(), problems: Vec::new(), labels: Default::default() };
    let lifted = BTreeSet::new();
    let cx = Ctx { img: &img, fun: &fun, lifted: &lifted, prefix: "f_" };
    let mut em = Em { cx: &cx, out: String::new(), disp: Some("DISP.load(std::sync::atomic::Ordering::Relaxed)".into()) };
    em.insn(ins)?;
    Ok(format!(
        "pub fn {name}(c: &mut Cpu) {{\n\
         let m = c.mem;\n\
         let (mut eax, mut ecx, mut edx, mut ebx, mut esp, mut ebp, mut esi, mut edi) = (c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi);\n\
         let mut fl = c.fl;\n\
         macro_rules! st {{ () => {{ c.eax = eax; c.ecx = ecx; c.edx = edx; c.ebx = ebx; c.esp = esp; c.ebp = ebp; c.esi = esi; c.edi = edi; c.fl = fl; }} }}\n\
         macro_rules! ld {{ () => {{ eax = c.eax; ecx = c.ecx; edx = c.edx; ebx = c.ebx; esp = c.esp; ebp = c.ebp; esi = c.esi; edi = c.edi; fl = c.fl; }} }}\n\
         {}\
         st!();\n}}\n\n",
        em.out
    ))
}

/// Emits `ins`; returns whether it ends the block (control never falls
/// through to the next instruction).
fn terminator(em: &mut Em, ins: &Instruction, fun: &cfg::Function) -> Result<bool, Unsupported> {
    use Mnemonic as M;
    let mn = ins.mnemonic();
    let mut line = |em: &mut Em, s: String| {
        em.out.push_str(&s);
        em.out.push('\n');
    };
    match ins.flow_control() {
        FlowControl::Next => {
            em.insn(ins)?;
            Ok(false)
        }
        FlowControl::Call => {
            line(em, format!("// {:08x} {}", ins.ip(), text(ins)));
            let t = ins.near_branch_target() as u32;
            let next = ins.next_ip() as u32;
            if t == next {
                line(em, format!("esp = esp.wrapping_sub(4); m.w32(esp, {next:#x});"));
            } else {
                line(em, format!("esp = esp.wrapping_sub(4); m.w32(esp, {next:#x});"));
                em.call(t);
            }
            Ok(false)
        }
        FlowControl::IndirectCall => {
            line(em, format!("// {:08x} {}", ins.ip(), text(ins)));
            line(em, "{".into());
            em.indirect_target(ins)?;
            let next = ins.next_ip() as u32;
            line(em, format!("esp = esp.wrapping_sub(4); m.w32(esp, {next:#x});"));
            line(em, "st!(); c.call(t); ld!();".into());
            line(em, "}".into());
            Ok(false)
        }
        FlowControl::ConditionalBranch => {
            line(em, format!("// {:08x} {}", ins.ip(), text(ins)));
            let t = ins.near_branch_target() as u32;
            let cond = match mn {
                M::Jecxz => "ecx == 0".to_string(),
                M::Loop => {
                    line(em, "ecx = ecx.wrapping_sub(1);".into());
                    "ecx != 0".to_string()
                }
                M::Loope => {
                    line(em, "ecx = ecx.wrapping_sub(1);".into());
                    "ecx != 0 && fl.zf".to_string()
                }
                M::Loopne => {
                    line(em, "ecx = ecx.wrapping_sub(1);".into());
                    "ecx != 0 && !fl.zf".to_string()
                }
                M::Jcxz => "(ecx & 0xffff) == 0".to_string(),
                _ => emit::cond(ins.condition_code()).to_string(),
            };
            line(em, format!("if {cond} {{ pc = {t:#x}; continue; }}"));
            // Falls through (the next instruction is a leader).
            Ok(false)
        }
        FlowControl::UnconditionalBranch => {
            line(em, format!("// {:08x} {}", ins.ip(), text(ins)));
            let t = ins.near_branch_target() as u32;
            if fun.leaders.contains(&t) {
                line(em, format!("pc = {t:#x}; continue;"));
            } else {
                em.tail_call(t);
            }
            Ok(true)
        }
        FlowControl::IndirectBranch => {
            line(em, format!("// {:08x} {}", ins.ip(), text(ins)));
            line(em, "{".into());
            em.indirect_target(ins)?;
            if let Some(ts) = fun.tables.get(&(ins.ip() as u32)) {
                line(em, "match t {".into());
                let uniq: BTreeSet<u32> = ts.iter().copied().collect();
                for t in uniq {
                    line(em, format!("{t:#x} => {{ pc = {t:#x}; continue; }}"));
                }
                line(em, "_ => { st!(); c.tail_jump(t); return; }".into());
                line(em, "}".into());
            } else if !fun.labels.is_empty() {
                // A computed jump to one of the function's own labels stays inside it.
                line(em, "match t {".into());
                for l in &fun.labels {
                    line(em, format!("{l:#x} => {{ pc = {l:#x}; continue; }}"));
                }
                line(em, "_ => { st!(); c.tail_jump(t); return; }".into());
                line(em, "}".into());
            } else {
                line(em, "st!(); c.tail_jump(t); return;".into());
            }
            line(em, "}".into());
            Ok(true)
        }
        FlowControl::Return => {
            line(em, format!("// {:08x} {}", ins.ip(), text(ins)));
            let n = if ins.op_count() == 1 { ins.immediate(0) as u32 } else { 0 };
            line(em, format!("assert_eq!(m.r32(esp), ra0, \"return address changed in {} at {:#x} (esp {{esp:#x}}, expected return to {{ra0:#x}})\"); esp = esp.wrapping_add({});", fn_name(em.cx.prefix, fun.entry), ins.ip(), 4 + n));
            line(em, "st!(); return;".into());
            Ok(true)
        }
        FlowControl::Exception | FlowControl::Interrupt | FlowControl::XbeginXabortXend => {
            line(em, format!("// {:08x} {}", ins.ip(), text(ins)));
            if matches!(mn, M::Int3 | M::Ud2 | M::Hlt | M::Int | M::Int1 | M::Into | M::Ud0 | M::Ud1) {
                line(em, format!("panic!(\"{} reached at {:#x}\");", mn_name(mn), ins.ip()));
                Ok(true)
            } else {
                Err(Unsupported(text(ins)))
            }
        }
    }
}

fn mn_name(m: Mnemonic) -> String {
    format!("{m:?}").to_lowercase()
}

impl Em<'_> {
    pub fn call(&mut self, target: u32) {
        let name = fn_name(self.cx.prefix, target);
        let s = if self.cx.lifted.contains(&target) { format!("st!(); {name}(c); ld!();\n") } else { format!("st!(); c.call({target:#x}); ld!();\n") };
        self.out.push_str(&s);
    }

    pub fn tail_call(&mut self, target: u32) {
        let name = fn_name(self.cx.prefix, target);
        let s = if self.cx.lifted.contains(&target) { format!("st!(); {name}(c); return;\n") } else { format!("st!(); c.call({target:#x}); return;\n") };
        self.out.push_str(&s);
    }

    /// `let t = <target>;` of an indirect call or jump.
    pub fn indirect_target(&mut self, ins: &Instruction) -> Result<(), Unsupported> {
        self.ea_pub(ins)?;
        let t = self.read_pub(ins, 0)?;
        writeln!(self.out, "let t: u32 = {t};").unwrap();
        Ok(())
    }
}

const _: Option<ConditionCode> = None;
const _: Option<OpKind> = None;
const _: Option<Register> = None;
