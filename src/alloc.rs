//! Running out of memory ends the run the way GNU sed does: `sed: memory exhausted` on standard
//! error and exit status 1, not the abort of Rust's default handler.
//!
//! `Guarded` wraps the system allocator; when an allocation fails it reports and exits at once,
//! without allocating (the message prefix was stored at start-up). Only the binary installs it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicPtr, Ordering};

/// The name this program was run as (a leaked string), the prefix of the message.
static PROGRAM: AtomicPtr<String> = AtomicPtr::new(std::ptr::null_mut());

/// Remembers the name the program was run as, for the out-of-memory message.
pub fn set_program_name(name: String) {
    PROGRAM.store(Box::into_raw(Box::new(name)), Ordering::Release);
}

#[cold]
fn exhausted() -> ! {
    let p = PROGRAM.load(Ordering::Acquire);
    // SAFETY: the pointer is null or a leaked `String` that is never freed or changed.
    let name = if p.is_null() { "sed" } else { unsafe { (*p).as_str() } };
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(name.as_bytes());
    let _ = err.write_all(b": memory exhausted\n");
    std::process::exit(1)
}

/// The system allocator, ending the process with GNU sed's message when it has no memory.
pub struct Guarded;

// SAFETY: every method forwards to `System` and only adds the exit on a null result.
unsafe impl GlobalAlloc for Guarded {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            exhausted();
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if p.is_null() {
            exhausted();
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if p.is_null() {
            exhausted();
        }
        p
    }
}
