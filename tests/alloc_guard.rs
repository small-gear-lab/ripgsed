//! A failed allocation ends the process with GNU sed's message and status 1 (see `alloc`).
//! The check runs in a child process, as the guard exits the process it is in.

use std::alloc::{GlobalAlloc, Layout};
use std::process::Command;

use ripgsed::alloc::{set_program_name, Guarded};

/// More than any system gives: the allocation must fail.
fn huge() -> Layout {
    Layout::from_size_align(usize::MAX / 2, 1).unwrap()
}

fn child(kind: &str) {
    set_program_name("sed".to_string());
    // SAFETY: the layout is valid; the guard exits the process when the allocation fails.
    unsafe {
        match kind {
            "alloc" => drop(Guarded.alloc(huge())),
            "zeroed" => drop(Guarded.alloc_zeroed(huge())),
            _ => {
                let small = Layout::from_size_align(16, 1).unwrap();
                let p = Guarded.alloc(small);
                drop(Guarded.realloc(p, small, usize::MAX / 2));
            }
        }
    }
    // Reached only if the guard did not exit.
    std::process::exit(99);
}

#[test]
fn a_failed_allocation_says_memory_exhausted() {
    if let Ok(kind) = std::env::var("RIPGSED_ALLOC_GUARD_CHILD") {
        child(&kind);
    }
    for kind in ["alloc", "zeroed", "realloc"] {
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["a_failed_allocation_says_memory_exhausted", "--exact", "--nocapture"])
            .env("RIPGSED_ALLOC_GUARD_CHILD", kind)
            .output()
            .expect("run the test binary");
        assert_eq!(out.status.code(), Some(1), "{kind}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains("sed: memory exhausted"), "{kind}");
    }
}
