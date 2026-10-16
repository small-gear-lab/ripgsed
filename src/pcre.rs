//! `grep -P` and `sed -P`: the PCRE2 library of the system, loaded when it is first needed.
//!
//! The crate has no dependencies, and a build must not need the PCRE2 files, so the library is
//! opened with `dlopen` and the few functions that are used are looked up by name. If it is not
//! there, `-P` says so and everything else works.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::OnceLock;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;

pub const CASELESS: u32 = 0x0000_0008;
pub const MULTILINE: u32 = 0x0000_0400;
pub const DOLLAR_ENDONLY: u32 = 0x0000_0010;
pub const UCP: u32 = 0x0002_0000;
pub const UTF: u32 = 0x0008_0000;
pub const MATCH_INVALID_UTF: u32 = 0x0400_0000;
const ERROR_NOMATCH: c_int = -1;

type Compile = unsafe extern "C" fn(*const u8, usize, u32, *mut c_int, *mut usize, *mut c_void) -> *mut c_void;
type Match = unsafe extern "C" fn(*const c_void, *const u8, usize, usize, u32, *mut c_void, *mut c_void) -> c_int;
type MatchDataCreate = unsafe extern "C" fn(*const c_void, *mut c_void) -> *mut c_void;
type OvectorPointer = unsafe extern "C" fn(*mut c_void) -> *mut usize;
type ErrorMessage = unsafe extern "C" fn(c_int, *mut u8, usize) -> c_int;
type CodeFree = unsafe extern "C" fn(*mut c_void);
type MatchDataFree = unsafe extern "C" fn(*mut c_void);
type PatternInfo = unsafe extern "C" fn(*const c_void, u32, *mut c_void) -> c_int;
type JitCompile = unsafe extern "C" fn(*mut c_void, u32) -> c_int;

struct Lib {
    compile: Compile,
    match_: Match,
    match_data_create: MatchDataCreate,
    ovector_pointer: OvectorPointer,
    error_message: ErrorMessage,
    code_free: CodeFree,
    match_data_free: MatchDataFree,
    pattern_info: PatternInfo,
    jit_compile: Option<JitCompile>,
}

fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(load).as_ref()
}

fn load() -> Option<Lib> {
    // SAFETY: the names are NUL-terminated; a symbol that is found has the signature it is
    // declared with in `pcre2.h`, and the library stays loaded for the life of the process.
    unsafe {
        let mut handle = std::ptr::null_mut();
        for name in [c"libpcre2-8.so.0", c"libpcre2-8.so"] {
            handle = dlopen(name.as_ptr(), RTLD_NOW);
            if !handle.is_null() {
                break;
            }
        }
        if handle.is_null() {
            return None;
        }
        let sym = |name: &CStr| -> Option<*mut c_void> {
            let p = dlsym(handle, name.as_ptr());
            (!p.is_null()).then_some(p)
        };
        let jit = sym(c"pcre2_jit_compile_8").map(|p| std::mem::transmute::<*mut c_void, JitCompile>(p));
        Some(Lib {
            compile: std::mem::transmute::<*mut c_void, Compile>(sym(c"pcre2_compile_8")?),
            match_: std::mem::transmute::<*mut c_void, Match>(sym(c"pcre2_match_8")?),
            match_data_create: std::mem::transmute::<*mut c_void, MatchDataCreate>(sym(c"pcre2_match_data_create_from_pattern_8")?),
            ovector_pointer: std::mem::transmute::<*mut c_void, OvectorPointer>(sym(c"pcre2_get_ovector_pointer_8")?),
            error_message: std::mem::transmute::<*mut c_void, ErrorMessage>(sym(c"pcre2_get_error_message_8")?),
            code_free: std::mem::transmute::<*mut c_void, CodeFree>(sym(c"pcre2_code_free_8")?),
            match_data_free: std::mem::transmute::<*mut c_void, MatchDataFree>(sym(c"pcre2_match_data_free_8")?),
            pattern_info: std::mem::transmute::<*mut c_void, PatternInfo>(sym(c"pcre2_pattern_info_8")?),
            jit_compile: jit,
        })
    }
}

/// A compiled pattern with its working memory.
pub struct Pcre {
    lib: &'static Lib,
    code: *mut c_void,
    data: *mut c_void,
    groups: usize,
}

impl std::fmt::Debug for Pcre {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pcre")
    }
}

/// The value of an offset of a group that took no part in the match.
const UNSET: usize = usize::MAX;
const INFO_CAPTURECOUNT: u32 = 4;

impl Pcre {
    /// Compiles `pattern`; the error is the message of the library.
    pub fn new(pattern: &[u8], options: u32) -> Result<Pcre, String> {
        let Some(lib) = lib() else {
            return Err("Perl matching not supported in a --disable-perl-regexp build".to_string());
        };
        let mut code_error: c_int = 0;
        let mut offset: usize = 0;
        // SAFETY: the pointers are valid for the call; `pattern` is read for its length only.
        let code = unsafe { (lib.compile)(pattern.as_ptr(), pattern.len(), options, &mut code_error, &mut offset, std::ptr::null_mut()) };
        if code.is_null() {
            let mut buffer = [0u8; 256];
            // SAFETY: the buffer is as long as it is declared to be.
            let n = unsafe { (lib.error_message)(code_error, buffer.as_mut_ptr(), buffer.len()) };
            let n = usize::try_from(n).unwrap_or(0).min(buffer.len());
            return Err(String::from_utf8_lossy(&buffer[..n]).into_owned());
        }
        if let Some(jit) = lib.jit_compile {
            // (JIT compiling is an optimization: if it fails the interpreter runs.)
            // SAFETY: `code` is a compiled pattern.
            unsafe { jit(code, 1) };
        }
        // SAFETY: `code` is a compiled pattern.
        let data = unsafe { (lib.match_data_create)(code, std::ptr::null_mut()) };
        if data.is_null() {
            // SAFETY: `code` was made above and is not used again.
            unsafe { (lib.code_free)(code) };
            return Err("memory exhausted".to_string());
        }
        let mut groups: u32 = 0;
        // SAFETY: `code` is a compiled pattern and the answer for this item is one `u32`.
        unsafe { (lib.pattern_info)(code, INFO_CAPTURECOUNT, (&mut groups as *mut u32).cast()) };
        Ok(Pcre { lib, code, data, groups: groups as usize })
    }

    /// The number of parenthesized groups of the pattern.
    pub fn group_count(&self) -> usize {
        self.groups
    }

    /// Like `find`, with the offsets of every group (`None` for a group that took no part).
    pub fn captures(&self, subject: &[u8], start: usize, out: &mut Vec<Option<(usize, usize)>>) -> bool {
        let Some((s, e)) = self.find(subject, start) else {
            return false;
        };
        out.clear();
        out.push(Some((s, e)));
        // SAFETY: the match data was made for this pattern, so its vector holds a pair for the
        // whole match and for each group.
        let ov = unsafe { (self.lib.ovector_pointer)(self.data) };
        for g in 1..=self.groups {
            let (a, b) = unsafe { (*ov.add(2 * g), *ov.add(2 * g + 1)) };
            out.push((a != UNSET && b != UNSET).then_some((a, b)));
        }
        true
    }

    /// The first match in `subject` that starts at or after `start`: its start and end.
    pub fn find(&self, subject: &[u8], start: usize) -> Option<(usize, usize)> {
        // SAFETY: the subject is valid for its length; `data` belongs to `code`.
        let rc = unsafe { (self.lib.match_)(self.code, subject.as_ptr(), subject.len(), start, 0, self.data, std::ptr::null_mut()) };
        if rc < 0 {
            let _ = ERROR_NOMATCH;
            return None;
        }
        // SAFETY: after a match the vector holds at least the start and end of the whole match.
        let (s, e) = unsafe {
            let ov = (self.lib.ovector_pointer)(self.data);
            (*ov, *ov.add(1))
        };
        Some((s, e))
    }
}

impl Drop for Pcre {
    fn drop(&mut self) {
        // SAFETY: both were made by the library and are not used again.
        unsafe {
            (self.lib.match_data_free)(self.data);
            (self.lib.code_free)(self.code);
        }
    }
}
