//! The locale of the run, decided the way the C library decides it, and the few C library
//! functions that the matcher asks about characters.
//!
//! GNU sed calls `setlocale(LC_ALL, "")` and then uses whatever the C library knows about
//! the locale. `from_env` does the same: the priority of `LC_ALL`, `LC_CTYPE`/`LC_COLLATE`
//! and `LANG`, the spelling of the names, the search path `LOCPATH`, and the failure of a locale
//! that is not installed (which leaves every category in the `C` locale, even those that
//! could have been loaded) are all the library's.
//!
//! What the run then does depends on the character set of `LC_CTYPE` and on the two categories
//! the matcher reads:
//!
//! * a byte-wise locale (`C`, `POSIX`, or a character set this program does not know yet) is
//!   `Locale::C`;
//! * UTF-8 with `LC_CTYPE` and `LC_COLLATE` as glibc's `C.UTF-8` (or `C`) is `Locale::Utf8`:
//!   the tables in `regex::classes_table`, no call into the library while matching;
//! * every other UTF-8 locale is `Locale::Libc`: the characters are the same, but what a class
//!   contains, what the upper-case form of a letter is, and which characters a range such as
//!   `[a-z]` takes are asked of the library (`towupper`, `iswalnum`, and `regcomp`/`regexec` for
//!   bracket expressions, the same engine that GNU sed uses, so that collation and the case
//!   rules of the locale are exactly its answers).
//!
//! Single-byte and non-UTF-8 multibyte character sets can be added as more `Locale` values that
//! decode through `mbrtowc`; every place that asks the library already goes through this module.
//! See docs/ARCHITECTURE.md.

use crate::regex::Locale;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

/// Facts about the character set of the run, set once by `from_env` before anything reads them.
///
/// `ASCII_IDENT`: the bytes 0..=127 are the characters of the same number (false for Shift_JIS,
/// where 0x5c and 0x7e are the yen sign and the overline). `HIGH_ONLY`: every byte of a
/// multibyte character is 0x80 or more (EUC-JP, EUC-KR, UTF-8; not Shift_JIS, GBK, Big5, whose
/// second bytes can be ASCII). With both, an ASCII byte is always a character of its own.
static MB: AtomicBool = AtomicBool::new(false);
static ASCII_IDENT: AtomicBool = AtomicBool::new(true);
static HIGH_ONLY: AtomicBool = AtomicBool::new(true);
/// The word characters among the bytes 0x80..=0xff of a single-byte locale (`\w`, `\b`).
/// Single bytes of 0x80 or more that `mbrtowc` accepts and the library's matcher does not take for
/// `.` (0x80 in GBK): they count as invalid bytes.
static BAD_SINGLE: [AtomicBool; 128] = [const { AtomicBool::new(false) }; 128];
static WORD_HIGH: [AtomicBool; 128] = [const { AtomicBool::new(false) }; 128];

/// Whether an ASCII byte is always a character of its own, so that automata and literal searches
/// on bytes agree with the characters (true in `C`, UTF-8, EUC-JP, EUC-KR and single-byte sets).
pub fn ascii_bytes_are_characters() -> bool {
    ASCII_IDENT.load(Relaxed) && HIGH_ONLY.load(Relaxed)
}

/// Whether the bytes 0..=127 are the ASCII characters (false for Shift_JIS).
pub fn ascii_is_identity() -> bool {
    ASCII_IDENT.load(Relaxed)
}

/// Whether the byte 0x80..=0xff is a word character of a single-byte locale.
#[inline]
pub fn word_byte_high(b: u8) -> bool {
    WORD_HIGH[(b & 0x7f) as usize].load(Relaxed)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod sys {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::ffi::{c_char, c_int, CStr};
    use std::hash::{BuildHasherDefault, Hasher};
    
    use super::{ASCII_IDENT, BAD_SINGLE, HIGH_ONLY, WORD_HIGH};
    use crate::regex::Locale;
    use std::sync::atomic::Ordering::Relaxed;

    const LC_CTYPE: c_int = 0;
    const LC_COLLATE: c_int = 3;
    const LC_ALL: c_int = 6;
    const CODESET: c_int = 14;
    const REG_ICASE: c_int = 1 << 1;
    const REG_STARTEND: c_int = 1 << 2;

    /// `regex_t` is 64 bytes in glibc; the room is generous because the library fills it in.
    #[repr(C, align(16))]
    struct RegexBuf([u64; 32]);

    #[repr(C)]
    struct RegMatch {
        so: c_int,
        eo: c_int,
    }

    unsafe extern "C" {
        fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
        fn nl_langinfo(item: c_int) -> *mut c_char;
        safe fn towupper(wc: u32) -> u32;
        safe fn towlower(wc: u32) -> u32;
        safe fn iswalnum(wc: u32) -> c_int;
        fn regcomp(preg: *mut RegexBuf, pattern: *const c_char, cflags: c_int) -> c_int;
        fn regexec(preg: *const RegexBuf, string: *const c_char, nmatch: usize, pmatch: *mut RegMatch, eflags: c_int) -> c_int;
        fn regfree(preg: *mut RegexBuf);
        fn __ctype_get_mb_cur_max() -> usize;
        fn mbrtowc(pwc: *mut u32, s: *const c_char, n: usize, ps: *mut MbState) -> usize;
        fn wcrtomb(s: *mut c_char, wc: u32, ps: *mut MbState) -> usize;
        safe fn btowc(c: c_int) -> u32;
        safe fn wctob(wc: u32) -> c_int;
    }

    /// `mbstate_t` is 8 bytes in glibc; the room is generous. All zeros is the initial state.
    #[repr(C)]
    struct MbState([u64; 2]);

    fn name_of(p: *const c_char) -> Option<String> {
        // SAFETY: a non-null result of `setlocale` is a NUL-terminated string.
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }

    fn is_c_utf8(name: &str) -> bool {
        name.eq_ignore_ascii_case("C.UTF-8") || name.eq_ignore_ascii_case("C.UTF8")
    }

    pub fn from_env() -> Locale {
        // SAFETY: called once, before any other thread exists; the strings are NUL-terminated.
        unsafe {
            if setlocale(LC_ALL, c"".as_ptr()).is_null() {
                setlocale(LC_ALL, c"C".as_ptr());
                return Locale::C;
            }
            let ctype = name_of(setlocale(LC_CTYPE, std::ptr::null()));
            let collate = name_of(setlocale(LC_COLLATE, std::ptr::null()));
            let codeset = name_of(nl_langinfo(CODESET)).unwrap_or_default();
            // Only the two categories the matcher reads stay switched on: the messages of the C
            // library (`strerror`) and the number formats must not change with the locale.
            setlocale(LC_ALL, c"C".as_ptr());
            let (Some(ctype), Some(collate)) = (ctype, collate) else { return Locale::C };
            let utf8 = codeset.eq_ignore_ascii_case("UTF-8");
            // `C` and `POSIX` (and a name that fails to load, handled above) have the codeset of ASCII.
            if codeset.is_empty() || codeset == "ANSI_X3.4-1968" {
                return Locale::C;
            }
            let builtin = utf8 && is_c_utf8(&ctype) && (is_c_utf8(&collate) || collate == "C" || collate == "POSIX");
            if builtin {
                return Locale::Utf8;
            }
            let (Ok(c1), Ok(c2)) = (std::ffi::CString::new(ctype), std::ffi::CString::new(collate)) else {
                return Locale::C;
            };
            if setlocale(LC_CTYPE, c1.as_ptr()).is_null() || setlocale(LC_COLLATE, c2.as_ptr()).is_null() {
                setlocale(LC_ALL, c"C".as_ptr());
                return Locale::C;
            }
            if utf8 {
                return Locale::Libc;
            }
            // Another character set: a single-byte one (ISO-8859-x, KOI8-R, Windows-125x) is read
            // byte by byte with the answers of the library; a multibyte one (EUC-JP, Shift_JIS,
            // GBK, Big5, EUC-KR) is decoded with `mbrtowc`.
            if __ctype_get_mb_cur_max() <= 1 {
                if let Ok(set) = LibSet::new(b"[_[:alnum:]]", false) {
                    for b in 0x80..=0xffu8 {
                        WORD_HIGH[(b & 0x7f) as usize].store(set.contains_byte(b), Relaxed);
                    }
                }
                return Locale::Sb;
            }
            super::MB.store(true, Relaxed);
            probe_multibyte();
            Locale::Mb
        }
    }

    /// Finds out whether ASCII bytes are characters of their own in this multibyte character set.
    fn probe_multibyte() {
        let ident = (0..0x80u32).all(|b| decode_raw(&[b as u8]).is_some_and(|(c, l)| c == b && l == 1));
        // A byte of 0x80 or more followed by an ASCII byte: the pair is never (the start of) a
        // character, the first byte stands alone or is invalid.
        let high_only = (0x80..=0xffu32).all(|b| {
            (0..0x80u32).all(|t| {
                let bytes = [b as u8, t as u8];
                let mut st = MbState([0; 2]);
                let mut wc = 0u32;
                // SAFETY: two readable bytes; `st` is a fresh initial state.
                let r = unsafe { mbrtowc(&mut wc, bytes.as_ptr().cast(), 2, &mut st) };
                r == usize::MAX || r == 1 || r == 0
            })
        });
        if let Ok(dot) = LibSet::new(b".", false) {
            for b in 0x80..=0xffu8 {
                if decode_raw(&[b]).is_some() && !dot.contains_byte(b) {
                    BAD_SINGLE[(b & 0x7f) as usize].store(true, Relaxed);
                }
            }
        }
        ASCII_IDENT.store(ident, Relaxed);
        HIGH_ONLY.store(high_only, Relaxed);
    }

    /// The character at the start of `bytes` and its length, or `None` for an invalid or
    /// incomplete sequence.
    fn decode_raw(bytes: &[u8]) -> Option<(u32, usize)> {
        if bytes.is_empty() {
            return None;
        }
        let mut st = MbState([0; 2]);
        let mut wc = 0u32;
        // SAFETY: `bytes` is readable for its length; `st` is a fresh initial state.
        let r = unsafe { mbrtowc(&mut wc, bytes.as_ptr().cast(), bytes.len().min(8), &mut st) };
        match r {
            0 => Some((0, 1)),
            r if r <= 8 => Some((wc, r)),
            _ => None,
        }
    }

    pub fn starts_multibyte(b: u8) -> bool {
        let mut st = MbState([0; 2]);
        let mut wc = 0u32;
        // SAFETY: one readable byte; `st` is a fresh initial state.
        unsafe { mbrtowc(&mut wc, [b].as_ptr().cast(), 1, &mut st) == usize::MAX - 1 }
    }

    /// One character of a non-UTF-8 multibyte locale at `pos`.
    pub fn mb_decode(hay: &[u8], pos: usize) -> Option<(char, usize)> {
        let b = *hay.get(pos)?;
        if b < 0x80 && ASCII_IDENT.load(Relaxed) {
            return Some((b as char, 1));
        }
        let (wc, len) = decode_raw(&hay[pos..])?;
        if len == 1 && BAD_SINGLE[(b & 0x7f) as usize].load(Relaxed) {
            return None;
        }
        Some((char::from_u32(wc)?, len))
    }

    /// The bytes of a character in the locale (`wcrtomb`), or `None` when it has none.
    pub fn mb_encode(c: char, out: &mut [u8; 16]) -> Option<usize> {
        let mut st = MbState([0; 2]);
        // SAFETY: `out` has room for MB_LEN_MAX bytes; `st` is a fresh initial state.
        let r = unsafe { wcrtomb(out.as_mut_ptr().cast(), c as u32, &mut st) };
        (r != usize::MAX).then_some(r)
    }

    /// GNU sed converts a byte through the wide character: `towupper` of `btowc`, then `wctob`;
    /// a result that is not a byte of the set becomes 0xFF.
    fn byte_via_wide(b: u8, f: fn(u32) -> u32) -> u8 {
        let wc = btowc(b as c_int);
        if wc == u32::MAX {
            return 0xff;
        }
        match wctob(f(wc)) {
            r if (0..=255).contains(&r) => r as u8,
            _ => 0xff,
        }
    }

    pub fn byte_upper(b: u8) -> u8 {
        byte_via_wide(b, |w| towupper(w))
    }

    pub fn byte_lower(b: u8) -> u8 {
        byte_via_wide(b, |w| towlower(w))
    }

    pub fn upper(c: char) -> char {
        char::from_u32(towupper(c as u32)).unwrap_or(c)
    }

    pub fn lower(c: char) -> char {
        char::from_u32(towlower(c as u32)).unwrap_or(c)
    }

    /// `\w` and the word assertions: an alphanumeric character or `_`.
    pub fn is_word(c: char) -> bool {
        c == '_' || iswalnum(c as u32) != 0
    }

    /// A multiplicative hash for the table of characters already asked about.
    #[derive(Default)]
    struct CharHasher(u64);

    impl Hasher for CharHasher {
        fn finish(&self) -> u64 {
            self.0
        }

        fn write(&mut self, bytes: &[u8]) {
            for &b in bytes {
                self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
        }

        fn write_u32(&mut self, v: u32) {
            self.0 = (v as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
    }

    /// A bracket expression (or `\w`) compiled by the C library, asked about one character at a
    /// time. The answers are remembered: ASCII in two bit masks, the rest in a table.
    pub struct LibSet {
        re: Box<RegexBuf>,
        known: Cell<u128>,
        member: Cell<u128>,
        others: RefCell<HashMap<u32, bool, BuildHasherDefault<CharHasher>>>,
    }

    /// Why the library refused a pattern.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LibError {
        Collate,
        CType,
        Bracket,
        Range,
        Other,
    }

    impl LibSet {
        /// Compiles `text`, a whole bracket expression, as the only thing in a line.
        pub fn new(text: &[u8], icase: bool) -> Result<LibSet, LibError> {
            if text.contains(&0) {
                return Err(LibError::Other);
            }
            let mut pat = Vec::with_capacity(text.len() + 3);
            pat.push(b'^');
            pat.extend_from_slice(text);
            pat.extend_from_slice(b"$\0");
            let mut re = Box::new(RegexBuf([0; 32]));
            // SAFETY: `re` is large enough for a `regex_t` and `pat` is NUL-terminated.
            let rc = unsafe { regcomp(&mut *re, pat.as_ptr().cast(), if icase { REG_ICASE } else { 0 }) };
            match rc {
                0 => Ok(LibSet { re, known: Cell::new(0), member: Cell::new(0), others: RefCell::default() }),
                3 => Err(LibError::Collate),
                4 => Err(LibError::CType),
                7 => Err(LibError::Bracket),
                11 => Err(LibError::Range),
                _ => Err(LibError::Other),
            }
        }

        pub fn contains(&self, c: char) -> bool {
            let u = c as u32;
            if u < 128 {
                let bit = 1u128 << u;
                if self.known.get() & bit != 0 {
                    return self.member.get() & bit != 0;
                }
                let hit = self.ask(c);
                self.known.set(self.known.get() | bit);
                if hit {
                    self.member.set(self.member.get() | bit);
                }
                return hit;
            }
            if let Some(&hit) = self.others.borrow().get(&u) {
                return hit;
            }
            let hit = self.ask(c);
            self.others.borrow_mut().insert(u, hit);
            hit
        }

        /// Whether the library's set holds the single byte `b` (a single-byte locale).
        pub fn contains_byte(&self, b: u8) -> bool {
            self.ask_bytes(&[b])
        }

        fn ask(&self, c: char) -> bool {
            if super::MB.load(Relaxed) {
                let mut buf = [0u8; 16];
                return match mb_encode(c, &mut buf) {
                    Some(n) if n > 0 => self.ask_bytes(&buf[..n]),
                    _ => false,
                };
            }
            let mut buf = [0u8; 4];
            self.ask_bytes(c.encode_utf8(&mut buf).as_bytes())
        }

        fn ask_bytes(&self, s: &[u8]) -> bool {
            let mut m = RegMatch { so: 0, eo: s.len() as c_int };
            // SAFETY: the regex was compiled by `regcomp`; with `REG_STARTEND` the string is
            // read from `so` to `eo` and needs no terminator.
            unsafe { regexec(&*self.re, s.as_ptr().cast(), 0, &mut m, REG_STARTEND) == 0 }
        }
    }

    impl Drop for LibSet {
        fn drop(&mut self) {
            // SAFETY: compiled by `regcomp` and freed once.
            unsafe { regfree(&mut *self.re) }
        }
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
mod sys {
    //! Without glibc there is no library to ask: the locale is read from the names in the
    //! environment as before, only `C` and UTF-8 exist, and the matcher never reaches `LibSet`.

    use crate::regex::Locale;

    pub fn from_env() -> Locale {
        let value = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .unwrap_or_default()
            .to_ascii_lowercase();
        if value.contains("utf-8") || value.contains("utf8") { Locale::Utf8 } else { Locale::C }
    }

    pub fn upper(c: char) -> char {
        crate::regex::simple_upper(c)
    }

    pub fn lower(c: char) -> char {
        crate::regex::simple_lower(c)
    }

    pub fn is_word(c: char) -> bool {
        c == '_' || c.is_alphanumeric()
    }

    pub fn starts_multibyte(_b: u8) -> bool {
        false
    }

    pub fn mb_decode(_hay: &[u8], _pos: usize) -> Option<(char, usize)> {
        None
    }

    pub fn mb_encode(_c: char, _out: &mut [u8; 16]) -> Option<usize> {
        None
    }

    pub fn byte_upper(b: u8) -> u8 {
        b.to_ascii_uppercase()
    }

    pub fn byte_lower(b: u8) -> u8 {
        b.to_ascii_lowercase()
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LibError {
        Collate,
        CType,
        Bracket,
        Range,
        Other,
    }

    pub struct LibSet;

    impl LibSet {
        pub fn new(_text: &[u8], _icase: bool) -> Result<LibSet, LibError> {
            Err(LibError::Other)
        }

        pub fn contains(&self, _c: char) -> bool {
            false
        }

        pub fn contains_byte(&self, _b: u8) -> bool {
            false
        }
    }
}

pub use sys::{LibError, LibSet};

/// The locale the environment selects, with the C library set up for it (call once, first).
pub fn from_env() -> Locale {
    sys::from_env()
}

/// Whether the byte, alone, is the start of a longer character of a non-UTF-8 multibyte locale
/// (`mbrtowc` asks for more bytes): it cannot delimit a regular expression.
pub fn starts_multibyte(b: u8) -> bool {
    sys::starts_multibyte(b)
}

/// The length of the multibyte character at the start of `text` in a non-UTF-8 multibyte
/// locale, and 1 anywhere else (also for a byte that is not part of a character). The scripts
/// of GNU sed are read character by character there, so the second byte of a character (`\`
/// in Shift_JIS, `[` or `]` in Big5) is never an escape or a bracket.
pub fn script_char_len(text: &[u8]) -> usize {
    if text.first().is_none_or(|&b| b < 0x80) || !MB.load(Relaxed) {
        return 1;
    }
    mb_decode(text, 0).map_or(1, |(_, l)| l)
}

/// One character of a non-UTF-8 multibyte locale at `pos` of `hay`, with its length in bytes;
/// `None` for an invalid or incomplete sequence.
pub fn mb_decode(hay: &[u8], pos: usize) -> Option<(char, usize)> {
    sys::mb_decode(hay, pos)
}

/// The bytes of a character in a non-UTF-8 multibyte locale; `None` if the set has no such character.
pub fn mb_encode(c: char, out: &mut [u8; 16]) -> Option<usize> {
    sys::mb_encode(c, out)
}

/// `toupper` of a single-byte locale.
pub fn byte_upper(b: u8) -> u8 {
    sys::byte_upper(b)
}

/// `tolower` of a single-byte locale.
pub fn byte_lower(b: u8) -> u8 {
    sys::byte_lower(b)
}

/// `towupper` of the current locale.
pub fn upper(c: char) -> char {
    sys::upper(c)
}

/// `towlower` of the current locale.
pub fn lower(c: char) -> char {
    sys::lower(c)
}

/// `\w` of the current locale.
pub fn is_word(c: char) -> bool {
    sys::is_word(c)
}
