//! The `l` command: the pattern space in an unambiguous form.

use crate::regex::Locale;

/// `width` is the wrap length: output lines are `width - 1` characters followed
/// by a backslash. 0 and 1 mean no wrapping.
pub fn list(pattern: &[u8], width: usize, _locale: Locale) -> Vec<u8> {
    let limit = width.saturating_sub(1);
    let mut out = Vec::new();
    let mut column = 0;
    for &b in pattern {
        let escape: Vec<u8> = match b {
            b'\\' => b"\\\\".to_vec(),
            0x07 => b"\\a".to_vec(),
            0x08 => b"\\b".to_vec(),
            0x0c => b"\\f".to_vec(),
            b'\n' => b"\\n".to_vec(),
            b'\r' => b"\\r".to_vec(),
            b'\t' => b"\\t".to_vec(),
            0x0b => b"\\v".to_vec(),
            0x20..=0x7e => vec![b],
            other => format!("\\{other:03o}").into_bytes(),
        };
        if width > 1 && column + escape.len() > limit {
            out.extend_from_slice(b"\\\n");
            column = 0;
        }
        column += escape.len();
        out.extend_from_slice(&escape);
    }
    out.extend_from_slice(b"$\n");
    out
}
