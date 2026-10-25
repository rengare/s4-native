//! Differential testing against the original game binaries (32-bit Windows
//! code, run under Wine):
//! - [`Original`]: load a DLL, address its globals as Ghidra shows them,
//!   restore its writable sections between tests, list the memory a call
//!   changed ([`Original::snapshot`], [`Snapshot::changes`]).
//! - [`Coverage`]: which conditional branches of chosen original functions
//!   the tests drove both ways (breakpoints + single steps, see `coverage`).
#![cfg(windows)]

pub mod coverage;
mod original;

pub use coverage::{Coverage, Writes};
pub use original::{Original, Snapshot};
