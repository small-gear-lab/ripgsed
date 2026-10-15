//! ripgsed: a stream editor that separates sed syntax, semantics, and execution strategy.
//!
//! See docs/ARCHITECTURE.md for the module layout.

pub mod cli;
pub mod io;
pub mod regex;
pub mod runtime;
pub mod syntax;

/// A fatal error: the message to print (after the program name prefix) and the exit status.
#[derive(Debug)]
pub struct Fatal {
    pub message: String,
    pub status: i32,
}

impl Fatal {
    pub fn new(message: impl Into<String>, status: i32) -> Self {
        Fatal { message: message.into(), status }
    }
}
