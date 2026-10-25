//! Instruction semantics as Rust statements. The generated function keeps the
//! general registers in locals (`eax`..`edi`, all `u32`), the flags in `fl`
//! (`s4_x86::Fl`), memory in `m`, and the x87/MMX/stack state in `c`.

use crate::cfg::Function;
use crate::pe::Image;
use iced_x86::{ConditionCode, Instruction, MemorySize, Mnemonic, OpKind, Register};
use std::fmt::Write;

#[derive(Debug)]
pub struct Unsupported(pub String);

pub type R = Result<(), Unsupported>;

const GPR: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];

pub struct Ctx<'a> {
    pub img: &'a Image,
    pub fun: &'a Function,
    /// Functions lifted in this run (direct calls to them are static).
    pub lifted: &'a std::collections::BTreeSet<u32>,
    /// Name prefix of lifted functions.
    pub prefix: &'a str,
}

fn gpr(r: Register) -> &'static str {
    GPR[r.full_register32().number()]
}

fn is_high8(r: Register) -> bool {
    matches!(r, Register::AH | Register::CH | Register::DH | Register::BH)
}

/// Value of a register operand as a zero-extended `u32`.
fn read_reg(r: Register) -> Result<String, Unsupported> {
    if r.is_gpr32() {
        Ok(gpr(r).to_string())
    } else if r.is_gpr16() {
        Ok(format!("({} & 0xffff)", gpr(r)))
    } else if r.is_gpr8() {
        if is_high8(r) { Ok(format!("(({} >> 8) & 0xff)", gpr(r))) } else { Ok(format!("({} & 0xff)", gpr(r))) }
    } else {
        Err(Unsupported(format!("register {r:?}")))
    }
}

fn write_reg(r: Register, v: &str) -> Result<String, Unsupported> {
    let n = gpr(r);
    if r.is_gpr32() {
        Ok(format!("{n} = {v};"))
    } else if r.is_gpr16() {
        Ok(format!("{n} = ({n} & 0xffff0000) | (({v}) & 0xffff);"))
    } else if r.is_gpr8() {
        if is_high8(r) { Ok(format!("{n} = ({n} & 0xffff00ff) | ((({v}) & 0xff) << 8);")) } else { Ok(format!("{n} = ({n} & 0xffffff00) | (({v}) & 0xff);")) }
    } else {
        Err(Unsupported(format!("register {r:?}")))
    }
}

fn reg_bits(r: Register) -> u32 {
    (r.size() * 8) as u32
}

fn mem_bits(ins: &Instruction) -> Result<u32, Unsupported> {
    Ok(match ins.memory_size() {
        MemorySize::UInt8 | MemorySize::Int8 => 8,
        MemorySize::UInt16 | MemorySize::Int16 | MemorySize::WordOffset => 16,
        MemorySize::UInt32 | MemorySize::Int32 | MemorySize::DwordOffset | MemorySize::SegPtr32 => 32,
        s => return Err(Unsupported(format!("memory size {s:?}"))),
    })
}

/// Effective address expression (u32).
fn ea_expr(ins: &Instruction, disp_override: Option<&str>) -> Result<String, Unsupported> {
    if ins.memory_base() == Register::EIP || ins.memory_index().is_vector_register() {
        return Err(Unsupported("address form".into()));
    }
    let mut parts: Vec<String> = Vec::new();
    if ins.memory_base() != Register::None {
        if !ins.memory_base().is_gpr32() {
            return Err(Unsupported("16-bit address".into()));
        }
        parts.push(gpr(ins.memory_base()).to_string());
    }
    if ins.memory_index() != Register::None {
        if !ins.memory_index().is_gpr32() {
            return Err(Unsupported("16-bit address".into()));
        }
        let idx = gpr(ins.memory_index());
        let sc = ins.memory_index_scale();
        parts.push(if sc == 1 { idx.to_string() } else { format!("{idx}.wrapping_mul({sc})") });
    }
    let disp = ins.memory_displacement32();
    if let Some(d) = disp_override {
        parts.push(d.to_string());
    } else if disp != 0 || parts.is_empty() {
        parts.push(format!("{disp:#x}u32"));
    }
    let mut it = parts.into_iter();
    let mut s = it.next().unwrap();
    for p in it {
        s = format!("{s}.wrapping_add({p})");
    }
    Ok(s)
}

/// Whether `ins` is one of the string instructions (not the SSE `movsd`/`cmpsd`).
pub fn is_string_op(ins: &Instruction) -> bool {
    use Mnemonic as M;
    matches!(ins.mnemonic(), M::Movsb | M::Movsw | M::Movsd | M::Cmpsb | M::Cmpsw | M::Cmpsd | M::Stosb | M::Stosw | M::Stosd | M::Lodsb | M::Lodsw | M::Lodsd | M::Scasb | M::Scasw | M::Scasd | M::Insb | M::Insw | M::Insd | M::Outsb | M::Outsw | M::Outsd) && is_string(ins)
}

fn is_string(ins: &Instruction) -> bool {
    match ins.mnemonic() {
        Mnemonic::Movsd | Mnemonic::Cmpsd => (0..ins.op_count()).all(|i| ins.op_kind(i) != OpKind::Register),
        _ => true,
    }
}

fn uses_fs(ins: &Instruction) -> bool {
    ins.segment_prefix() == Register::FS
}

pub fn cond(cc: ConditionCode) -> &'static str {
    match cc {
        ConditionCode::o => "fl.of",
        ConditionCode::no => "!fl.of",
        ConditionCode::b => "fl.cf",
        ConditionCode::ae => "!fl.cf",
        ConditionCode::e => "fl.zf",
        ConditionCode::ne => "!fl.zf",
        ConditionCode::be => "(fl.cf || fl.zf)",
        ConditionCode::a => "(!fl.cf && !fl.zf)",
        ConditionCode::s => "fl.sf",
        ConditionCode::ns => "!fl.sf",
        ConditionCode::p => "fl.pf",
        ConditionCode::np => "!fl.pf",
        ConditionCode::l => "(fl.sf != fl.of)",
        ConditionCode::ge => "(fl.sf == fl.of)",
        ConditionCode::le => "(fl.zf || fl.sf != fl.of)",
        ConditionCode::g => "(!fl.zf && fl.sf == fl.of)",
        ConditionCode::None => "true",
    }
}

pub struct Em<'a> {
    pub cx: &'a Ctx<'a>,
    pub out: String,
    /// Expression replacing the displacement of memory operands (the
    /// conformance tests vary it per run).
    pub disp: Option<String>,
}

impl<'a> Em<'a> {
    fn line(&mut self, s: impl AsRef<str>) {
        self.out.push_str(s.as_ref());
        self.out.push('\n');
    }

    /// Operand width in bits (register, memory, or the other operand's for
    /// immediates).
    fn bits(&self, ins: &Instruction, i: u32) -> Result<u32, Unsupported> {
        match ins.op_kind(i) {
            OpKind::Register => Ok(reg_bits(ins.op_register(i))),
            OpKind::Memory => mem_bits(ins),
            _ if i > 0 => {
                // Immediate: the width of operand 0.
                self.bits(ins, 0)
            }
            k => Err(Unsupported(format!("operand kind {k:?} has no width"))),
        }
    }

    fn mask(bits: u32) -> &'static str {
        match bits {
            8 => "0xff",
            16 => "0xffff",
            _ => "0xffffffff",
        }
    }

    /// Reads operand `i` (zero-extended u32 expression). Memory operands
    /// use the `ea` local.
    fn read(&self, ins: &Instruction, i: u32) -> Result<String, Unsupported> {
        match ins.op_kind(i) {
            OpKind::Register => read_reg(ins.op_register(i)),
            OpKind::Memory => {
                let bits = mem_bits(ins)?;
                if uses_fs(ins) {
                    if bits != 32 {
                        return Err(Unsupported("fs access width".into()));
                    }
                    return Ok("c.fs_r32(ea)".into());
                }
                Ok(match bits {
                    8 => "(m.r8(ea) as u32)".into(),
                    16 => "(m.r16(ea) as u32)".into(),
                    _ => "m.r32(ea)".into(),
                })
            }
            OpKind::Immediate8 | OpKind::Immediate8to16 | OpKind::Immediate8to32 | OpKind::Immediate16 | OpKind::Immediate32 => {
                let bits = self.bits(ins, 0)?;
                let v = ins.immediate(i) as u32 & if bits == 32 { u32::MAX } else { (1 << bits) - 1 };
                Ok(format!("{v:#x}u32"))
            }
            k => Err(Unsupported(format!("operand kind {k:?}"))),
        }
    }

    fn write(&self, ins: &Instruction, i: u32, v: &str) -> Result<String, Unsupported> {
        match ins.op_kind(i) {
            OpKind::Register => write_reg(ins.op_register(i), v),
            OpKind::Memory => {
                let bits = mem_bits(ins)?;
                if uses_fs(ins) {
                    if bits != 32 {
                        return Err(Unsupported("fs access width".into()));
                    }
                    return Ok(format!("c.fs_w32(ea, {v});"));
                }
                Ok(match bits {
                    8 => format!("m.w8(ea, ({v}) as u8);"),
                    16 => format!("m.w16(ea, ({v}) as u16);"),
                    _ => format!("m.w32(ea, {v});"),
                })
            }
            k => Err(Unsupported(format!("write to operand kind {k:?}"))),
        }
    }

    /// Emits `let ea = ...;` when the instruction has a memory operand.
    pub fn ea_pub(&mut self, ins: &Instruction) -> R {
        self.ea(ins)
    }

    pub fn read_pub(&self, ins: &Instruction, i: u32) -> Result<String, Unsupported> {
        self.read(ins, i)
    }

    fn ea(&mut self, ins: &Instruction) -> R {
        for i in 0..ins.op_count() {
            if ins.op_kind(i) == OpKind::Memory {
                let e = ea_expr(ins, self.disp.as_deref())?;
                self.line(format!("let ea: u32 = {e};"));
                break;
            }
        }
        Ok(())
    }

    fn push(&mut self, v: &str) {
        self.line(format!("esp = esp.wrapping_sub(4); m.w32(esp, {v});"));
    }

    fn pop_into(&mut self, name: &str) {
        self.line(format!("{name} = m.r32(esp); esp = esp.wrapping_add(4);"));
    }

    fn call_stmt(&mut self, target: u32) {
        let name = format!("{}{target:08x}", self.cx.prefix);
        if self.cx.lifted.contains(&target) {
            self.line(format!("st!(); {name}(c); ld!();"));
        } else {
            self.line(format!("st!(); c.call({target:#x}); ld!();"));
        }
    }

    /// One non-terminating instruction.
    pub fn insn(&mut self, ins: &Instruction) -> R {
        use Mnemonic as M;
        let mn = ins.mnemonic();
        self.line(format!("// {:08x} {}", ins.ip(), crate::text(ins)));
        self.line("{");
        let r = self.insn_body(ins, mn);
        self.line("}");
        r
    }

    fn insn_body(&mut self, ins: &Instruction, mn: Mnemonic) -> R {
        use Mnemonic as M;
        match mn {
            M::Nop | M::Fnop | M::Wait | M::Int3 | M::Pause | M::Prefetchnta | M::Prefetcht0 | M::Prefetcht1 | M::Prefetcht2 => {}
            M::Mov => {
                self.ea(ins)?;
                if ins.op0_kind() == OpKind::Register && ins.op0_register().is_segment_register() || ins.op1_kind() == OpKind::Register && ins.op1_register().is_segment_register() {
                    return Err(Unsupported("segment register move".into()));
                }
                let v = self.read(ins, 1)?;
                let w = self.write(ins, 0, &v)?;
                self.line(w);
            }
            M::Movzx => {
                self.ea(ins)?;
                let v = self.read(ins, 1)?;
                let w = self.write(ins, 0, &v)?;
                self.line(w);
            }
            M::Movsx => {
                self.ea(ins)?;
                let sb = self.bits(ins, 1)?;
                let v = self.read(ins, 1)?;
                let v = format!("alu::sext({sb}, {v}) as u32");
                let w = self.write(ins, 0, &v)?;
                self.line(w);
            }
            M::Lea => {
                let e = ea_expr(ins, self.disp.as_deref())?;
                let w = write_reg(ins.op0_register(), &e)?;
                self.line(w);
            }
            M::Xchg => {
                self.ea(ins)?;
                let a = self.read(ins, 0)?;
                let b = self.read(ins, 1)?;
                self.line(format!("let a = {a}; let b = {b};"));
                let w0 = self.write(ins, 0, "b")?;
                let w1 = self.write(ins, 1, "a")?;
                self.line(w0);
                self.line(w1);
            }
            M::Add | M::Adc | M::Sub | M::Sbb | M::Cmp | M::And | M::Or | M::Xor | M::Test => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0)?;
                let a = self.read(ins, 0)?;
                let b = self.read(ins, 1)?;
                let e = match mn {
                    M::Add => format!("alu::add({bits}, a, b, 0, &mut fl)"),
                    M::Adc => format!("alu::add({bits}, a, b, fl.cf as u32, &mut fl)"),
                    M::Sub | M::Cmp => format!("alu::sub({bits}, a, b, 0, &mut fl)"),
                    M::Sbb => format!("alu::sub({bits}, a, b, fl.cf as u32, &mut fl)"),
                    M::And | M::Test => format!("alu::logic({bits}, a & b, &mut fl)"),
                    M::Or => format!("alu::logic({bits}, a | b, &mut fl)"),
                    _ => format!("alu::logic({bits}, a ^ b, &mut fl)"),
                };
                self.line(format!("let a = {a}; let b = {b}; let r = {e};"));
                if !matches!(mn, M::Cmp | M::Test) {
                    let w = self.write(ins, 0, "r")?;
                    self.line(w);
                }
            }
            M::Inc | M::Dec | M::Neg | M::Not => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0)?;
                let a = self.read(ins, 0)?;
                let e = match mn {
                    M::Inc => format!("alu::inc({bits}, a, &mut fl)"),
                    M::Dec => format!("alu::dec({bits}, a, &mut fl)"),
                    M::Neg => format!("alu::neg({bits}, a, &mut fl)"),
                    _ => format!("alu::not({bits}, a)"),
                };
                self.line(format!("let a = {a}; let r = {e};"));
                let w = self.write(ins, 0, "r")?;
                self.line(w);
            }
            M::Shl | M::Sal | M::Shr | M::Sar | M::Rol | M::Ror | M::Rcl | M::Rcr => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0)?;
                let a = self.read(ins, 0)?;
                let n = if ins.op_count() == 1 {
                    "1u32".to_string()
                } else if ins.op1_kind() == OpKind::Register {
                    read_reg(ins.op1_register())?
                } else {
                    format!("{:#x}u32", ins.immediate(1) as u32 & 0xff)
                };
                let f = match mn {
                    M::Shl | M::Sal => "shl",
                    M::Shr => "shr",
                    M::Sar => "sar",
                    M::Rol => "rol",
                    M::Ror => "ror",
                    M::Rcl => "rcl",
                    _ => "rcr",
                };
                self.line(format!("let a = {a}; let r = alu::{f}({bits}, a, {n}, &mut fl);"));
                let w = self.write(ins, 0, "r")?;
                self.line(w);
            }
            M::Shld | M::Shrd => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0)?;
                let a = self.read(ins, 0)?;
                let b = self.read(ins, 1)?;
                let n = if ins.op2_kind() == OpKind::Register { read_reg(ins.op2_register())? } else { format!("{:#x}u32", ins.immediate(2) as u32 & 0xff) };
                let f = if mn == M::Shld { "shld" } else { "shrd" };
                self.line(format!("let a = {a}; let b = {b}; let r = alu::{f}({bits}, a, b, {n}, &mut fl);"));
                let w = self.write(ins, 0, "r")?;
                self.line(w);
            }
            M::Mul | M::Imul if ins.op_count() == 1 => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0)?;
                let b = self.read(ins, 0)?;
                let f = if mn == M::Mul { "mul" } else { "imul1" };
                match bits {
                    8 => self.line(format!("let b = {b}; let (lo, hi) = alu::{f}(8, eax & 0xff, b, &mut fl); eax = (eax & 0xffff0000) | (hi << 8) | lo;")),
                    16 => self.line(format!("let b = {b}; let (lo, hi) = alu::{f}(16, eax & 0xffff, b, &mut fl); eax = (eax & 0xffff0000) | lo; edx = (edx & 0xffff0000) | hi;")),
                    _ => self.line(format!("let b = {b}; let (lo, hi) = alu::{f}(32, eax, b, &mut fl); eax = lo; edx = hi;")),
                }
            }
            M::Imul => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0)?;
                let (x, y) = if ins.op_count() == 2 { (self.read(ins, 0)?, self.read(ins, 1)?) } else { (self.read(ins, 1)?, format!("{:#x}u32", ins.immediate(2) as u32 & if bits == 32 { u32::MAX } else { (1 << bits) - 1 })) };
                self.line(format!("let a = {x}; let b = {y}; let r = alu::imul({bits}, a, b, &mut fl);"));
                let w = self.write(ins, 0, "r")?;
                self.line(w);
            }
            M::Div | M::Idiv => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0)?;
                let d = self.read(ins, 0)?;
                let f = if mn == M::Div { "div" } else { "idiv" };
                match bits {
                    8 => self.line(format!("let d = {d}; let (q, r) = alu::{f}(8, eax & 0xffff, 0, d); eax = (eax & 0xffff0000) | (r << 8) | q;")),
                    16 => self.line(format!("let d = {d}; let (q, r) = alu::{f}(16, eax & 0xffff, edx & 0xffff, d); eax = (eax & 0xffff0000) | q; edx = (edx & 0xffff0000) | r;")),
                    _ => self.line(format!("let d = {d}; let (q, r) = alu::{f}(32, eax, edx, d); eax = q; edx = r;")),
                }
            }
            M::Cdq => self.line("edx = ((eax as i32) >> 31) as u32;"),
            M::Cwde => self.line("eax = (eax as u16 as i16 as i32) as u32;"),
            M::Cwd => self.line("edx = (edx & 0xffff0000) | ((((eax as u16 as i16) >> 15) as u16) as u32);"),
            M::Cbw => self.line("eax = (eax & 0xffff0000) | (((eax as u8 as i8 as i16) as u16) as u32);"),
            M::Bswap => {
                let r = ins.op0_register();
                self.line(format!("{n} = {n}.swap_bytes();", n = gpr(r)));
            }
            _ => return self.insn_more(ins, mn),
        }
        Ok(())
    }

    fn insn_more(&mut self, ins: &Instruction, mn: Mnemonic) -> R {
        use Mnemonic as M;
        let name = format!("{mn:?}");
        if let Some(_) = name.strip_prefix("Set").filter(|_| mn != M::Sahf && ins.op_count() == 1) {
            self.ea(ins)?;
            let w = self.write(ins, 0, &format!("({}) as u32", cond(ins.condition_code())))?;
            self.line(w);
            return Ok(());
        }
        if name.starts_with("Cmov") {
            self.ea(ins)?;
            let v = self.read(ins, 1)?;
            let w = self.write(ins, 0, "v")?;
            self.line(format!("let v = {v};"));
            self.line(format!("if {} {{ {w} }}", cond(ins.condition_code())));
            // A 32-bit cmov with a false condition still writes (zero
            // extension) only in 64-bit mode; nothing to do here.
            return Ok(());
        }
        match mn {
            M::Push if ins.op0_kind() == OpKind::Register && ins.op0_register().is_segment_register() => {
                // Win32 user-mode selectors.
                let sel = if ins.op0_register() == Register::CS { 0x1bu32 } else { 0x23 };
                self.push(&format!("{sel:#x}u32"));
            }
            M::Push => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0).unwrap_or(32);
                if bits == 16 {
                    let v = match ins.op0_kind() {
                        OpKind::Immediate16 | OpKind::Immediate8to16 => format!("{:#x}u32", ins.immediate(0) as u32 & 0xffff),
                        _ => self.read(ins, 0)?,
                    };
                    self.line(format!("let v = {v}; esp = esp.wrapping_sub(2); m.w16(esp, v as u16);"));
                    return Ok(());
                }
                if bits != 32 {
                    return Err(Unsupported("odd push".into()));
                }
                let v = match ins.op0_kind() {
                    OpKind::Immediate8to32 | OpKind::Immediate32 => format!("{:#x}u32", ins.immediate(0) as u32),
                    OpKind::Register if ins.op0_register().is_segment_register() => return Err(Unsupported("push segment".into())),
                    _ => self.read(ins, 0)?,
                };
                self.line(format!("let v = {v};"));
                self.push("v");
            }
            M::Pop => {
                if ins.op0_kind() == OpKind::Memory {
                    // The address is computed with ESP after the pop.
                    self.line("let v = m.r32(esp); esp = esp.wrapping_add(4);");
                    self.ea(ins)?;
                    let w = self.write(ins, 0, "v")?;
                    self.line(w);
                } else if ins.op0_register().is_gpr16() {
                    self.line("let v = m.r16(esp) as u32; esp = esp.wrapping_add(2);");
                    let w = self.write(ins, 0, "v")?;
                    self.line(w);
                } else if ins.op0_register().is_segment_register() {
                    // Segment registers are not modelled (flat memory): the selector is discarded.
                    self.line("esp = esp.wrapping_add(4);");
                } else {
                    self.line("let v = m.r32(esp); esp = esp.wrapping_add(4);");
                    let w = self.write(ins, 0, "v")?;
                    self.line(w);
                }
            }
            M::Pushfd => self.push("fl.to_u32()"),
            M::Pushf => self.line("esp = esp.wrapping_sub(2); m.w16(esp, fl.to_u32() as u16);"),
            M::Popf => self.line("let v = m.r16(esp) as u32; esp = esp.wrapping_add(2); fl = Fl::from_u32(v);"),
            M::Pusha => {
                self.line("let t = esp;");
                for r in ["eax", "ecx", "edx", "ebx"] {
                    self.line(format!("esp = esp.wrapping_sub(2); m.w16(esp, {r} as u16);"));
                }
                self.line("esp = esp.wrapping_sub(2); m.w16(esp, t as u16);");
                for r in ["ebp", "esi", "edi"] {
                    self.line(format!("esp = esp.wrapping_sub(2); m.w16(esp, {r} as u16);"));
                }
            }
            M::Popa => {
                for r in ["edi", "esi", "ebp"] {
                    self.line(format!("{r} = ({r} & 0xffff0000) | m.r16(esp) as u32; esp = esp.wrapping_add(2);"));
                }
                self.line("esp = esp.wrapping_add(2);");
                for r in ["ebx", "edx", "ecx", "eax"] {
                    self.line(format!("{r} = ({r} & 0xffff0000) | m.r16(esp) as u32; esp = esp.wrapping_add(2);"));
                }
            }
            M::Rdtsc => self.line("let t = c.rdtsc(); eax = t as u32; edx = (t >> 32) as u32;"),
            M::Cpuid => self.line("let (a, b, cc, d) = c.cpuid(eax, ecx); eax = a; ebx = b; ecx = cc; edx = d;"),
            // Port I/O: nothing is attached.
            M::Out | M::Outsb | M::Outsw | M::Outsd => {}
            M::In => {
                let w = write_reg(ins.op0_register(), "0")?;
                self.line(w);
            }
            M::Popfd => {
                self.line("let v = m.r32(esp); esp = esp.wrapping_add(4); fl = Fl::from_u32(v);");
            }
            M::Pushad => {
                self.line("let t = esp;");
                for r in ["eax", "ecx", "edx", "ebx"] {
                    self.push(r);
                }
                self.push("t");
                for r in ["ebp", "esi", "edi"] {
                    self.push(r);
                }
            }
            M::Popad => {
                self.pop_into("edi");
                self.pop_into("esi");
                self.pop_into("ebp");
                self.line("esp = esp.wrapping_add(4);");
                self.pop_into("ebx");
                self.pop_into("edx");
                self.pop_into("ecx");
                self.pop_into("eax");
            }
            M::Leave => {
                self.line("esp = ebp;");
                self.pop_into("ebp");
            }
            M::Cld => self.line("fl.df = false;"),
            M::Std => self.line("fl.df = true;"),
            M::Clc => self.line("fl.cf = false;"),
            M::Stc => self.line("fl.cf = true;"),
            M::Cmc => self.line("fl.cf = !fl.cf;"),
            M::Sahf => self.line("fl.set_ah((eax >> 8) & 0xff);"),
            M::Lahf => self.line("eax = (eax & 0xffff00ff) | (fl.to_ah() << 8);"),
            M::Xlatb => self.line("eax = (eax & 0xffffff00) | (m.r8(ebx.wrapping_add(eax & 0xff)) as u32);"),
            M::Bsf | M::Bsr => {
                self.ea(ins)?;
                let v = self.read(ins, 1)?;
                let f = if mn == M::Bsf { "bsf" } else { "bsr" };
                let w = self.write(ins, 0, "i")?;
                self.line(format!("let v = {v}; if let Some(i) = alu::{f}(v, &mut fl) {{ {w} }}"));
            }
            M::Bt | M::Bts | M::Btr | M::Btc => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0)?;
                let off = if ins.op1_kind() == OpKind::Register { read_reg(ins.op1_register())? } else { format!("{:#x}u32", ins.immediate(1) as u32 & 0xff) };
                if ins.op0_kind() == OpKind::Memory && ins.op1_kind() == OpKind::Register {
                    // Bit string: the offset reaches beyond the operand.
                    if bits != 32 {
                        return Err(Unsupported("bt m16 reg".into()));
                    }
                    self.line(format!("let off = {off}; let ea = ea.wrapping_add((((off as i32) >> 5) as u32).wrapping_mul(4)); let bit = off & 31;"));
                } else {
                    self.line(format!("let bit = {off} & {};", bits - 1));
                }
                let a = self.read(ins, 0)?;
                self.line(format!("let a = {a}; fl.cf = (a >> bit) & 1 != 0;"));
                let upd = match mn {
                    M::Bts => Some("a | (1 << bit)"),
                    M::Btr => Some("a & !(1 << bit)"),
                    M::Btc => Some("a ^ (1 << bit)"),
                    _ => None,
                };
                if let Some(u) = upd {
                    let w = self.write(ins, 0, &format!("({u})"))?;
                    self.line(w);
                }
            }
            M::Cmpxchg => {
                self.ea(ins)?;
                let bits = self.bits(ins, 0)?;
                let a = self.read(ins, 0)?;
                let src = self.read(ins, 1)?;
                let acc = match bits {
                    8 => "(eax & 0xff)",
                    16 => "(eax & 0xffff)",
                    _ => "eax",
                };
                self.line(format!("let a = {a}; let s = {src}; alu::sub({bits}, {acc}, a, 0, &mut fl);"));
                let w = self.write(ins, 0, "s")?;
                let wacc = write_reg(match bits { 8 => Register::AL, 16 => Register::AX, _ => Register::EAX }, "a")?;
                self.line(format!("if fl.zf {{ {w} }} else {{ {wacc} }}"));
            }
            M::Movsb | M::Movsw | M::Movsd | M::Stosb | M::Stosw | M::Stosd | M::Lodsb | M::Lodsw | M::Lodsd | M::Scasb | M::Scasw | M::Scasd | M::Cmpsb | M::Cmpsw | M::Cmpsd if is_string(ins) => {
                self.string_op(ins, mn)?;
            }
            M::Ret | M::Call | M::Jmp | M::Retf => unreachable!("terminators are handled by the block emitter"),
            _ => return self.insn_fpu(ins, mn),
        }
        Ok(())
    }

    fn string_op(&mut self, ins: &Instruction, mn: Mnemonic) -> R {
        use Mnemonic as M;
        let bits = match mn {
            M::Movsb | M::Stosb | M::Lodsb | M::Scasb | M::Cmpsb => 8,
            M::Movsw | M::Stosw | M::Lodsw | M::Scasw | M::Cmpsw => 16,
            _ => 32,
        };
        if ins.segment_prefix() != Register::None {
            return Err(Unsupported("segment override on string op".into()));
        }
        let rep = if ins.has_repe_prefix() { "Rep::E" } else if ins.has_repne_prefix() { "Rep::Ne" } else { "Rep::None" };
        match mn {
            M::Movsb | M::Movsw | M::Movsd => self.line(format!("string::movs(m, {bits}, {rep}, &mut esi, &mut edi, &mut ecx, fl.df);")),
            M::Stosb | M::Stosw | M::Stosd => {
                let acc = match bits { 8 => "(eax & 0xff)", 16 => "(eax & 0xffff)", _ => "eax" };
                self.line(format!("string::stos(m, {bits}, {rep}, {acc}, &mut edi, &mut ecx, fl.df);"));
            }
            M::Lodsb | M::Lodsw | M::Lodsd => {
                let acc = match bits { 8 => "(eax & 0xff)", 16 => "(eax & 0xffff)", _ => "eax" };
                let w = write_reg(match bits { 8 => Register::AL, 16 => Register::AX, _ => Register::EAX }, "v")?;
                self.line(format!("let v = string::lods(m, {bits}, {rep}, {acc}, &mut esi, &mut ecx, fl.df); {w}"));
            }
            M::Scasb | M::Scasw | M::Scasd => {
                let acc = match bits { 8 => "(eax & 0xff)", 16 => "(eax & 0xffff)", _ => "eax" };
                self.line(format!("string::scas(m, {bits}, {rep}, {acc}, &mut edi, &mut ecx, &mut fl);"));
            }
            _ => self.line(format!("string::cmps(m, {bits}, {rep}, &mut esi, &mut edi, &mut ecx, &mut fl);")),
        }
        Ok(())
    }

    /// A memory operand of an x87 instruction as a `Value`.
    fn fpu_mem_value(ins: &Instruction) -> Result<String, Unsupported> {
        Ok(match ins.memory_size() {
            MemorySize::Float32 => "Value::from_f32(f32::from_bits(m.r32(ea)))",
            MemorySize::Float64 => "Value::from_f64(f64::from_bits(m.r64(ea)))",
            MemorySize::Float80 => "Value::from_tbyte(m.r80(ea))",
            MemorySize::Int16 => "Value::from_i32(m.r16(ea) as i16 as i32)",
            MemorySize::Int32 => "Value::from_i32(m.r32(ea) as i32)",
            MemorySize::Int64 => "Value::from_i64(m.r64(ea) as i64)",
            s => return Err(Unsupported(format!("x87 memory size {s:?}"))),
        }
        .to_string())
    }

    fn st_index(r: Register) -> usize {
        r as usize - Register::ST0 as usize
    }

    fn insn_fpu(&mut self, ins: &Instruction, mn: Mnemonic) -> R {
        use Mnemonic as M;
        let name = format!("{mn:?}");
        // Arithmetic family.
        let arith = |base: &str| -> Option<&'static str> {
            Some(match base {
                "add" => "Arith::Add",
                "mul" => "Arith::Mul",
                "sub" => "Arith::Sub",
                "subr" => "Arith::SubR",
                "div" => "Arith::Div",
                "divr" => "Arith::DivR",
                _ => return None,
            })
        };
        let lower = name.to_lowercase();
        if lower.starts_with('f') {
            let rest = &lower[1..];
            let (is_int, rest2) = match rest.strip_prefix('i') {
                Some(r) if arith(r).is_some() => (true, r),
                _ => (false, rest),
            };
            let (pop, base) = match rest2.strip_suffix('p') {
                Some(b) if arith(b).is_some() => (true, b),
                _ => (false, rest2),
            };
            if let Some(op) = arith(base) {
                if ins.op0_kind() == OpKind::Memory {
                    self.ea(ins)?;
                    let v = Em::fpu_mem_value(ins)?;
                    self.line(format!("c.fpu.op_val({op}, {v});"));
                } else {
                    let d = Em::st_index(ins.op0_register());
                    let s = if ins.op_count() > 1 { Em::st_index(ins.op1_register()) } else { 0 };
                    self.line(format!("c.fpu.op_st({op}, {d}, {s}, {pop});"));
                }
                let _ = is_int;
                return Ok(());
            }
        }
        match mn {
            M::Fld => {
                if ins.op0_kind() == OpKind::Memory {
                    self.ea(ins)?;
                    self.line(match ins.memory_size() {
                        MemorySize::Float32 => "c.fpu.fld_f32(m.r32(ea));".to_string(),
                        MemorySize::Float64 => "c.fpu.fld_f64(m.r64(ea));".to_string(),
                        MemorySize::Float80 => "c.fpu.fld_f80(m.r80(ea));".to_string(),
                        s => return Err(Unsupported(format!("fld {s:?}"))),
                    });
                } else {
                    self.line(format!("c.fpu.fld_st({});", Em::st_index(ins.op0_register())));
                }
            }
            M::Fild => {
                self.ea(ins)?;
                let v = match ins.memory_size() {
                    MemorySize::Int16 => "m.r16(ea) as i16 as i64",
                    MemorySize::Int32 => "m.r32(ea) as i32 as i64",
                    _ => "m.r64(ea) as i64",
                };
                self.line(format!("c.fpu.fild({v});"));
            }
            M::Fst | M::Fstp => {
                let pop = mn == M::Fstp;
                if ins.op0_kind() == OpKind::Memory {
                    self.ea(ins)?;
                    self.line(match ins.memory_size() {
                        MemorySize::Float32 => format!("let v = c.fpu.fst_f32({pop}); m.w32(ea, v);"),
                        MemorySize::Float64 => format!("let v = c.fpu.fst_f64({pop}); m.w64(ea, v);"),
                        MemorySize::Float80 => format!("let v = c.fpu.fst_f80({pop}); m.w80(ea, v);"),
                        s => return Err(Unsupported(format!("fst {s:?}"))),
                    });
                } else {
                    self.line(format!("c.fpu.fst_st({}, {pop});", Em::st_index(ins.op0_register())));
                }
            }
            M::Fist | M::Fistp | M::Fisttp => {
                self.ea(ins)?;
                let pop = mn != M::Fist;
                let f = if mn == M::Fisttp { "fisttp" } else { "fist" };
                match ins.memory_size() {
                    MemorySize::Int16 => self.line(format!("let v = c.fpu.{f}(16, {pop}); m.w16(ea, v as u16);")),
                    MemorySize::Int32 => self.line(format!("let v = c.fpu.{f}(32, {pop}); m.w32(ea, v as u32);")),
                    _ => self.line(format!("let v = c.fpu.{f}(64, {pop}); m.w64(ea, v as u64);")),
                }
            }
            M::Fxch => {
                let i = match ins.op_count() {
                    0 => 1,
                    1 => Em::st_index(ins.op0_register()),
                    _ => Em::st_index(ins.op1_register()),
                };
                self.line(format!("c.fpu.xch({i});"));
            }
            M::Fchs => self.line("c.fpu.chs();"),
            M::Fsqrt => self.line("c.fpu.sqrt();"),
            M::Fyl2x => self.line("c.fpu.fyl2x(false);"),
            M::Fyl2xp1 => self.line("c.fpu.fyl2x(true);"),
            M::F2xm1 => self.line("c.fpu.f2xm1();"),
            M::Fsin => self.line("c.fpu.sincos(0);"),
            M::Fcos => self.line("c.fpu.sincos(1);"),
            M::Fptan => self.line("c.fpu.sincos(2);"),
            M::Fpatan => self.line("c.fpu.fpatan();"),
            M::Frndint => self.line("c.fpu.frndint();"),
            M::Fscale => self.line("c.fpu.fscale();"),
            M::Fprem => self.line("c.fpu.fprem(false);"),
            M::Fprem1 => self.line("c.fpu.fprem(true);"),
            M::Fabs => self.line("c.fpu.abs();"),
            M::Fld1 => self.line("c.fpu.fld_const(Constant::One);"),
            M::Fldz => self.line("c.fpu.fld_const(Constant::Zero);"),
            M::Fldpi => self.line("c.fpu.fld_const(Constant::Pi);"),
            M::Fldl2e => self.line("c.fpu.fld_const(Constant::L2E);"),
            M::Fldl2t => self.line("c.fpu.fld_const(Constant::L2T);"),
            M::Fldln2 => self.line("c.fpu.fld_const(Constant::Ln2);"),
            M::Fldlg2 => self.line("c.fpu.fld_const(Constant::Lg2);"),
            M::Fcom | M::Fcomp | M::Fucom | M::Fucomp | M::Ficom | M::Ficomp => {
                let ordered = matches!(mn, M::Fcom | M::Fcomp | M::Ficom | M::Ficomp);
                let pops = matches!(mn, M::Fcomp | M::Fucomp | M::Ficomp) as u32;
                if ins.op_count() > 0 && ins.op0_kind() == OpKind::Memory {
                    self.ea(ins)?;
                    let v = Em::fpu_mem_value(ins)?;
                    self.line(format!("c.fpu.compare({v}, {ordered}); for _ in 0..{pops} {{ c.fpu.pop(); }}"));
                } else {
                    let i = match ins.op_count() {
                        0 => 1,
                        1 => Em::st_index(ins.op0_register()),
                        _ => Em::st_index(ins.op1_register()),
                    };
                    self.line(format!("c.fpu.compare_st({i}, {ordered}, {pops});"));
                }
            }
            M::Fcompp => self.line("c.fpu.compare_st(1, true, 2);"),
            M::Fucompp => self.line("c.fpu.compare_st(1, false, 2);"),
            M::Ftst => self.line("c.fpu.test();"),
            M::Fxam => self.line("c.fpu.examine();"),
            M::Fnstsw | M::Fstsw => {
                if ins.op0_kind() == OpKind::Memory {
                    self.ea(ins)?;
                    self.line("m.w16(ea, c.fpu.status());");
                } else {
                    self.line("eax = (eax & 0xffff0000) | (c.fpu.status() as u32);");
                }
            }
            M::Fnstcw | M::Fstcw => {
                self.ea(ins)?;
                self.line("m.w16(ea, c.fpu.cw);");
            }
            M::Fldcw => {
                self.ea(ins)?;
                self.line("c.fpu.load_cw(m.r16(ea));");
            }
            M::Fnstenv | M::Fstenv => {
                self.ea(ins)?;
                self.line("c.fpu.store_env(m, ea);");
            }
            M::Fldenv => {
                self.ea(ins)?;
                self.line("c.fpu.load_env(m, ea);");
            }
            M::Fnsave | M::Fsave => {
                self.ea(ins)?;
                self.line("c.fpu.save_state(m, ea);");
            }
            M::Frstor => {
                self.ea(ins)?;
                self.line("c.fpu.restore_state(m, ea);");
            }
            M::Fnclex | M::Fclex => self.line("c.fpu.clear_exceptions();"),
            M::Fninit | M::Finit => self.line("c.fpu.init();"),
            M::Ffree => self.line(format!("c.fpu.free({});", Em::st_index(ins.op0_register()))),
            M::Fdecstp => self.line("c.fpu.decstp();"),
            M::Fincstp => self.line("c.fpu.incstp();"),
            _ => return self.insn_mmx(ins, mn),
        }
        Ok(())
    }

    fn mm_operand(&self, ins: &Instruction, i: u32) -> Result<String, Unsupported> {
        match ins.op_kind(i) {
            OpKind::Register if ins.op_register(i).is_mm() => Ok(format!("c.fpu.mm_read({})", ins.op_register(i) as usize - Register::MM0 as usize)),
            OpKind::Register => Ok(format!("({} as u64)", read_reg(ins.op_register(i))?)),
            OpKind::Memory => Ok(match ins.memory_size() {
                MemorySize::UInt32 | MemorySize::Int32 | MemorySize::Packed32_UInt16 => "(m.r32(ea) as u64)".into(),
                _ => "m.r64(ea)".into(),
            }),
            OpKind::Immediate8 => Ok(format!("{}u64", ins.immediate(i) as u8)),
            k => Err(Unsupported(format!("mmx operand {k:?}"))),
        }
    }

    fn insn_mmx(&mut self, ins: &Instruction, mn: Mnemonic) -> R {
        use Mnemonic as M;
        let name = format!("{mn:?}").to_lowercase();
        const BINARY: &[&str] = &[
            "paddb", "paddw", "paddd", "psubb", "psubw", "psubd", "paddusb", "paddusw", "psubusb", "psubusw", "paddsb", "paddsw", "psubsb", "psubsw", "pmullw", "pmulhw", "pcmpeqb", "pcmpeqw", "pcmpeqd", "pcmpgtb", "pcmpgtw", "pcmpgtd", "pand", "pandn", "por", "pxor", "pmaddwd", "psllw", "pslld", "psllq", "psrlw", "psrld", "psrlq", "psraw", "psrad", "punpcklbw", "punpckhbw", "punpcklwd", "punpckhwd", "punpckldq", "punpckhdq", "packuswb", "packsswb", "packssdw",
        ];
        if mn == M::Emms {
            self.line("c.fpu.emms();");
            return Ok(());
        }
        if BINARY.contains(&name.as_str()) {
            self.ea(ins)?;
            let d = ins.op0_register() as usize - Register::MM0 as usize;
            let src = self.mm_operand(ins, 1)?;
            self.line(format!("let a = c.fpu.mm_read({d}); let b = {src}; c.fpu.mm_write({d}, mmx::{name}(a, b));"));
            return Ok(());
        }
        match mn {
            M::Movq => {
                self.ea(ins)?;
                if ins.op0_kind() == OpKind::Register {
                    let d = ins.op0_register() as usize - Register::MM0 as usize;
                    let src = self.mm_operand(ins, 1)?;
                    self.line(format!("let v = {src}; c.fpu.mm_write({d}, v);"));
                } else {
                    let s = self.mm_operand(ins, 1)?;
                    self.line(format!("let v = {s}; m.w64(ea, v);"));
                }
            }
            M::Movd => {
                self.ea(ins)?;
                if ins.op0_register().is_mm() {
                    let d = ins.op0_register() as usize - Register::MM0 as usize;
                    let src = self.mm_operand(ins, 1)?;
                    self.line(format!("let v = {src} & 0xffffffff; c.fpu.mm_write({d}, v);"));
                } else {
                    let s = self.mm_operand(ins, 1)?;
                    self.line(format!("let v = {s} as u32;"));
                    let w = self.write(ins, 0, "v")?;
                    self.line(w);
                }
            }
            _ => return Err(Unsupported(format!("{}", crate::text(ins)))),
        }
        Ok(())
    }
}
