#![cfg(all(windows, target_arch = "x86"))]
use s4_x86::{native::execute, Cpu, Mem};

#[test]
fn executes_one_instruction() {
    let mut stack = vec![0u8; 4096];
    let top = stack.as_mut_ptr() as u32 + 2048;
    let mut c = Cpu::new(Mem::IDENTITY, top);
    c.eax = 5;
    // add eax, 7
    execute(&[0x83, 0xc0, 0x07], &mut c);
    assert_eq!(c.eax, 12);
    assert_eq!(c.esp, top);
}
