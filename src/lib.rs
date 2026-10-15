//! ripgsed: a stream editor that separates sed syntax, semantics, and execution strategy.
//!
//! See docs/ARCHITECTURE.md for the module layout.

pub mod cli;
pub mod io;
pub mod regex;
pub mod runtime;
pub mod syntax;
