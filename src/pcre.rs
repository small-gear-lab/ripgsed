//! `grep -P` and `sed -P`: the PCRE2 library, through the bindings of `pcre2-sys`.
//!
//! The bindings carry the source of the library and build it, so the program needs neither the
//! PCRE2 files of the system at run time nor a build that finds them (`PCRE2_SYS_STATIC=1` always
//! builds the carried copy, which is what the repository asks for in `.cargo/config.toml`).

use std::ffi::{c_int, c_void};

use pcre2_sys as sys;

pub const MULTILINE: u32 = sys::PCRE2_MULTILINE;
pub const CASELESS: u32 = sys::PCRE2_CASELESS;
pub const DOLLAR_ENDONLY: u32 = sys::PCRE2_DOLLAR_ENDONLY;
pub const UCP: u32 = sys::PCRE2_UCP;
pub const UTF: u32 = sys::PCRE2_UTF;
pub const MATCH_INVALID_UTF: u32 = sys::PCRE2_MATCH_INVALID_UTF;
const INFO_CAPTURECOUNT: u32 = 4;

/// The value of an offset of a group that took no part in the match.
const UNSET: usize = usize::MAX;

/// The library gave up on a match: a limit (the number of backtracking steps, the depth, the heap, the
/// stack of the JIT) or another error. It is never the same as "no match".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcreError {
    code: i32,
}

impl PcreError {
    /// The code of the library (a negative number).
    pub fn code(&self) -> i32 {
        self.code
    }

    /// The match limit: the pattern backtracks too much for this text.
    pub fn is_backtracking_limit(&self) -> bool {
        self.code == sys::PCRE2_ERROR_MATCHLIMIT
    }

    /// The text of the error. The limits have the words of GNU grep.
    pub fn message(&self) -> String {
        match self.code {
            sys::PCRE2_ERROR_MATCHLIMIT => "exceeded PCRE's backtracking limit".to_string(),
            sys::PCRE2_ERROR_DEPTHLIMIT => "exceeded PCRE's nested backtracking limit".to_string(),
            sys::PCRE2_ERROR_JIT_STACKLIMIT => "exhausted PCRE JIT stack".to_string(),
            sys::PCRE2_ERROR_NOMEMORY => "memory exhausted".to_string(),
            code => {
                let mut buffer = [0u8; 256];
                // SAFETY: the buffer is as long as it is declared to be.
                let n = unsafe {
                    sys::pcre2_get_error_message_8(code, buffer.as_mut_ptr(), buffer.len())
                };
                match usize::try_from(n) {
                    Ok(n) if n > 0 => {
                        String::from_utf8_lossy(&buffer[..n.min(buffer.len())]).into_owned()
                    }
                    _ => format!("internal PCRE error: {code}"),
                }
            }
        }
    }
}

impl std::fmt::Display for PcreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// A compiled pattern with its working memory.
pub struct Pcre {
    code: *mut sys::pcre2_code_8,
    data: *mut sys::pcre2_match_data_8,
    groups: usize,
}

impl std::fmt::Debug for Pcre {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pcre")
    }
}

impl Pcre {
    /// Compiles `pattern`; the error is the message of the library.
    pub fn new(pattern: &[u8], options: u32) -> Result<Pcre, String> {
        let mut code_error: c_int = 0;
        let mut offset: usize = 0;
        // SAFETY: the pointers are valid for the call; `pattern` is read for its length only.
        let code = unsafe {
            sys::pcre2_compile_8(
                pattern.as_ptr(),
                pattern.len(),
                options,
                &mut code_error,
                &mut offset,
                std::ptr::null_mut(),
            )
        };
        if code.is_null() {
            let mut buffer = [0u8; 256];
            // SAFETY: the buffer is as long as it is declared to be.
            let n = unsafe {
                sys::pcre2_get_error_message_8(code_error, buffer.as_mut_ptr(), buffer.len())
            };
            let n = usize::try_from(n).unwrap_or(0).min(buffer.len());
            return Err(String::from_utf8_lossy(&buffer[..n]).into_owned());
        }
        // (JIT compiling is an optimization: if it fails or is not built in, the interpreter runs.)
        // SAFETY: `code` is a compiled pattern.
        unsafe { sys::pcre2_jit_compile_8(code, 1) };
        // SAFETY: `code` is a compiled pattern.
        let data =
            unsafe { sys::pcre2_match_data_create_from_pattern_8(code, std::ptr::null_mut()) };
        if data.is_null() {
            // SAFETY: `code` was made above and is not used again.
            unsafe { sys::pcre2_code_free_8(code) };
            return Err("memory exhausted".to_string());
        }
        let mut groups: u32 = 0;
        // SAFETY: `code` is a compiled pattern and the answer for this item is one `u32`.
        unsafe {
            sys::pcre2_pattern_info_8(
                code,
                INFO_CAPTURECOUNT,
                (&mut groups as *mut u32).cast::<c_void>(),
            )
        };
        Ok(Pcre {
            code,
            data,
            groups: groups as usize,
        })
    }

    /// The number of parenthesized groups of the pattern.
    pub fn group_count(&self) -> usize {
        self.groups
    }

    /// The first match in `subject` that starts at or after `start`: its start and end. A match that
    /// the library gives up on is an error, not `None`.
    pub fn find(&self, subject: &[u8], start: usize) -> Result<Option<(usize, usize)>, PcreError> {
        // SAFETY: the subject is valid for its length; `data` belongs to `code`.
        let rc = unsafe {
            sys::pcre2_match_8(
                self.code,
                subject.as_ptr(),
                subject.len(),
                start,
                0,
                self.data,
                std::ptr::null_mut(),
            )
        };
        if rc == sys::PCRE2_ERROR_NOMATCH {
            return Ok(None);
        }
        if rc < 0 {
            return Err(PcreError { code: rc });
        }
        // SAFETY: after a match the vector holds at least the start and end of the whole match.
        let (s, e) = unsafe {
            let ov = sys::pcre2_get_ovector_pointer_8(self.data);
            (*ov, *ov.add(1))
        };
        Ok(Some((s, e)))
    }

    /// Like `find`, with the offsets of every group (`None` for a group that took no part).
    pub fn captures(
        &self,
        subject: &[u8],
        start: usize,
        out: &mut Vec<Option<(usize, usize)>>,
    ) -> Result<bool, PcreError> {
        let Some((s, e)) = self.find(subject, start)? else {
            return Ok(false);
        };
        out.clear();
        out.push(Some((s, e)));
        // SAFETY: the match data was made for this pattern, so its vector holds a pair for the
        // whole match and for each group.
        let ov = unsafe { sys::pcre2_get_ovector_pointer_8(self.data) };
        for g in 1..=self.groups {
            // SAFETY: as above.
            let (a, b) = unsafe { (*ov.add(2 * g), *ov.add(2 * g + 1)) };
            out.push((a != UNSET && b != UNSET).then_some((a, b)));
        }
        Ok(true)
    }
}

impl Drop for Pcre {
    fn drop(&mut self) {
        // SAFETY: both were made by the library and are not used again.
        unsafe {
            sys::pcre2_match_data_free_8(self.data);
            sys::pcre2_code_free_8(self.code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_match_the_library_gives_up_on_is_an_error_not_no_match() {
        let re = Pcre::new(b"^(a|aa)+$", DOLLAR_ENDONLY).unwrap();
        let subject = format!("{}!", "a".repeat(60));
        let error = re.find(subject.as_bytes(), 0).unwrap_err();
        assert!(error.is_backtracking_limit(), "{error:?}");
        assert_eq!(error.message(), "exceeded PCRE's backtracking limit");
        assert_eq!(re.find(b"aaaa", 0), Ok(Some((0, 4))));
        assert_eq!(re.find(b"b", 0), Ok(None));
        let mut groups = Vec::new();
        assert_eq!(re.captures(b"aa", 0, &mut groups), Ok(true));
        assert_eq!(re.captures(b"b", 0, &mut groups), Ok(false));
    }
}
