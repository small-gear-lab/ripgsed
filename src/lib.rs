//! A GNU sed 4.9 compatible stream editor, as a library and as the command `ripgsed`.
//!
//! A script is a list of sed commands. Make one by writing it as sed text, or by calling
//! methods; both give the same [`Script`], which a [`Run`] applies to text.
//!
//! ```
//! use ripgsed::{Address, Edit, Pattern, Run, Script};
//!
//! // As sed text ...
//! let script = Script::parse("2,3d;s/foo/bar/g")?;
//! // ... or with methods, which run the same commands at the same speed.
//! let same = Edit::new()
//!     .when(Address::lines(2..=3)).delete()
//!     .substitute(Pattern::basic("foo"), "bar").global()
//!     .build()?;
//! assert_eq!(script.to_string(), "2,3d;s/foo/bar/g");
//! assert_eq!(same.to_string(), "2,3d\ns/foo/bar/g");
//!
//! let out = Run::new().apply(&same, b"foo\nfoo\nfoo\nfoo foo\n")?;
//! assert_eq!(out, b"bar\nbar bar\n");
//! # Ok::<(), ripgsed::Error>(())
//! ```
//!
//! Every command is an address and an operation; no address means every line. The builder
//! writes the commands as sed text and the text goes through the parser of the command line,
//! so a mistake (a range for `q`, an unknown label) gets the message of GNU sed, and a script
//! built with methods is the script one would write by hand. Commands that have no method yet
//! (the hold space, `n` `N` `D` `P`, `r` `w`, labels and branches) go in with [`Edit::raw`].
//!
//! # What a run does
//!
//! [`Run::run`] reads a [`Read`](std::io::Read) and writes a [`Write`](std::io::Write) as it
//! goes; [`Run::apply`] works on bytes in memory. With [`Run::jobs`] a script whose lines are
//! independent of each other (most `s`, `d`, `p`, `y` scripts) is spread over threads, and the
//! output stays byte for byte what one thread gives.
//!
//! Patterns mean what the GNU sed manual says. [`Run::glibc_compat`] also reproduces the
//! quirks of the matcher of the C library, which GNU sed inherits; see `docs/` for the list.
//!
//! The modules below the documented items are for the command and the tests of the project:
//! they are hidden and may change in any release.

mod api;
mod edit;

pub use api::{Dialect, Encoding, Error, ErrorKind, Outcome, Run, Script};
pub use edit::{Address, Edit, Pattern};

// The parts the command `ripgsed` and the tests of the project use. They are not part of the
// interface of the crate and may change in any release.
#[doc(hidden)]
pub mod alloc;
#[doc(hidden)]
pub mod analysis;
#[doc(hidden)]
pub mod cli;
#[doc(hidden)]
pub mod debug;
#[doc(hidden)]
pub mod grep;
#[doc(hidden)]
pub mod io;
#[doc(hidden)]
pub mod locale;
#[doc(hidden)]
pub mod parallel;
#[doc(hidden)]
pub mod pcre;
#[doc(hidden)]
pub mod regex;
#[doc(hidden)]
pub mod runtime;
#[doc(hidden)]
pub mod search;
#[doc(hidden)]
pub mod stream;
#[doc(hidden)]
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
