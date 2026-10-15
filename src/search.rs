//! Byte search primitives: `memchr`, `memrchr`, `count`, and `memmem`.
//!
//! Written for this project (no dependency, D8). On x86_64 they use SSE2, which every x86_64
//! CPU has, and AVX2 when the CPU reports it; elsewhere, and for short inputs, a portable
//! scalar loop. The results are always those of the obvious scalar definitions, which the
//! tests check against random inputs around every vector width.

/// The index of the first `needle` in `hay`.
pub fn memchr(needle: u8, hay: &[u8]) -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    {
        if hay.len() >= 16 {
            // SAFETY: the CPU features are checked or are part of the x86_64 baseline.
            return unsafe {
                if avx2() && hay.len() >= 32 { x86::memchr_avx2(needle, hay) } else { x86::memchr_sse2(needle, hay) }
            };
        }
    }
    hay.iter().position(|&b| b == needle)
}

/// The index of the last `needle` in `hay`.
pub fn memrchr(needle: u8, hay: &[u8]) -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    {
        if hay.len() >= 16 {
            // SAFETY: SSE2 is part of the x86_64 baseline.
            return unsafe { x86::memrchr_sse2(needle, hay) };
        }
    }
    hay.iter().rposition(|&b| b == needle)
}

/// The number of `needle` bytes in `hay`.
pub fn count(needle: u8, hay: &[u8]) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if hay.len() >= 16 {
            // SAFETY: SSE2 is part of the x86_64 baseline.
            return unsafe { x86::count_sse2(needle, hay) };
        }
    }
    hay.iter().filter(|&&b| b == needle).count()
}

/// The index of the first occurrence of `needle` in `hay`. An empty needle is found at 0.
///
/// It looks for the two bytes of the needle that are rarest in ordinary text, compares 16 or
/// 32 haystack bytes at a time against both, and checks the rest of the needle only where both
/// agree.
pub fn memmem(hay: &[u8], needle: &[u8]) -> Option<usize> {
    match needle.len() {
        0 => return Some(0),
        1 => return memchr(needle[0], hay),
        n if n > hay.len() => return None,
        _ => {}
    }
    let (i1, i2) = rare_pair(needle);
    #[cfg(target_arch = "x86_64")]
    {
        if hay.len() >= needle.len() + 16 {
            // SAFETY: the CPU features are checked or are part of the x86_64 baseline.
            return unsafe {
                if avx2() && hay.len() >= needle.len() + 32 {
                    x86::memmem_avx2(hay, needle, i1, i2)
                } else {
                    x86::memmem_sse2(hay, needle, i1, i2)
                }
            };
        }
    }
    memmem_scalar(hay, needle, 0)
}

/// The straightforward search from position `from`; also finishes what the vector loops leave.
fn memmem_scalar(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let n = needle.len();
    if hay.len() < n {
        return None;
    }
    let first = needle[0];
    let last_start = hay.len() - n;
    let mut i = from;
    while i <= last_start {
        match hay[i..=last_start].iter().position(|&b| b == first) {
            None => return None,
            Some(k) => {
                i += k;
                if &hay[i..i + n] == needle {
                    return Some(i);
                }
                i += 1;
            }
        }
    }
    None
}

#[cfg(target_arch = "x86_64")]
fn avx2() -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    // 0: unknown, 1: no, 2: yes.
    static STATE: AtomicU8 = AtomicU8::new(0);
    match STATE.load(Ordering::Relaxed) {
        0 => {
            let yes = std::arch::is_x86_feature_detected!("avx2");
            STATE.store(if yes { 2 } else { 1 }, Ordering::Relaxed);
            yes
        }
        s => s == 2,
    }
}

/// How common a byte is in ordinary text, from 0 (rare) to 255 (very common). Only the order
/// matters, and only to pick which two bytes of a needle to test.
fn commonness(b: u8) -> u8 {
    match b {
        b' ' => 255,
        b'e' => 250,
        b't' | b'a' | b'o' => 240,
        b'i' | b'n' | b's' | b'r' => 230,
        b'h' | b'l' | b'd' | b'c' | b'u' => 210,
        b'm' | b'f' | b'p' | b'g' | b'w' | b'y' | b'b' => 190,
        b'v' | b'k' => 150,
        b'\n' | b'.' | b',' | b'-' | b'_' | b'/' | b'=' | b':' => 170,
        b'0'..=b'9' => 160,
        b'a'..=b'z' => 120,
        b'A'..=b'Z' => 110,
        0x21..=0x7e => 60,
        0x80..=0xff => 40,
        _ => 10,
    }
}

/// The positions in `needle` (at least two bytes long) of its two rarest bytes, the first one
/// being the rarest. The positions differ.
fn rare_pair(needle: &[u8]) -> (usize, usize) {
    let mut best = 0;
    for (i, &b) in needle.iter().enumerate() {
        if commonness(b) < commonness(needle[best]) {
            best = i;
        }
    }
    let mut second = if best == 0 { 1 } else { 0 };
    for (i, &b) in needle.iter().enumerate() {
        if i != best && commonness(b) < commonness(needle[second]) {
            second = i;
        }
    }
    (best, second)
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use super::memmem_scalar;
    use std::arch::x86_64::*;

    #[target_feature(enable = "sse2")]
    pub unsafe fn memchr_sse2(needle: u8, hay: &[u8]) -> Option<usize> {
        unsafe {
            let n = _mm_set1_epi8(needle as i8);
            let p = hay.as_ptr();
            let len = hay.len();
            let mut i = 0;
            while i + 16 <= len {
                let v = _mm_loadu_si128(p.add(i) as *const __m128i);
                let m = _mm_movemask_epi8(_mm_cmpeq_epi8(v, n)) as u32;
                if m != 0 {
                    return Some(i + m.trailing_zeros() as usize);
                }
                i += 16;
            }
            if i < len {
                // The last, overlapping block: bytes before `i` held no match, so the first hit is right.
                let start = len - 16;
                let v = _mm_loadu_si128(p.add(start) as *const __m128i);
                let m = _mm_movemask_epi8(_mm_cmpeq_epi8(v, n)) as u32;
                if m != 0 {
                    return Some(start + m.trailing_zeros() as usize);
                }
            }
            None
        }
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn memchr_avx2(needle: u8, hay: &[u8]) -> Option<usize> {
        unsafe {
            let n = _mm256_set1_epi8(needle as i8);
            let p = hay.as_ptr();
            let len = hay.len();
            let mut i = 0;
            while i + 32 <= len {
                let v = _mm256_loadu_si256(p.add(i) as *const __m256i);
                let m = _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, n)) as u32;
                if m != 0 {
                    return Some(i + m.trailing_zeros() as usize);
                }
                i += 32;
            }
            if i < len {
                let start = len - 32;
                let v = _mm256_loadu_si256(p.add(start) as *const __m256i);
                let m = _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, n)) as u32;
                if m != 0 {
                    return Some(start + m.trailing_zeros() as usize);
                }
            }
            None
        }
    }

    #[target_feature(enable = "sse2")]
    pub unsafe fn memrchr_sse2(needle: u8, hay: &[u8]) -> Option<usize> {
        unsafe {
            let n = _mm_set1_epi8(needle as i8);
            let p = hay.as_ptr();
            let mut end = hay.len();
            while end >= 16 {
                let start = end - 16;
                let v = _mm_loadu_si128(p.add(start) as *const __m128i);
                let m = _mm_movemask_epi8(_mm_cmpeq_epi8(v, n)) as u32;
                if m != 0 {
                    return Some(start + 31 - m.leading_zeros() as usize);
                }
                end = start;
            }
            // The first `end` (< 16) bytes: one overlapping block at the start.
            if end > 0 {
                let v = _mm_loadu_si128(p as *const __m128i);
                let m = _mm_movemask_epi8(_mm_cmpeq_epi8(v, n)) as u32 & ((1u32 << end) - 1);
                if m != 0 {
                    return Some(31 - m.leading_zeros() as usize);
                }
            }
            None
        }
    }

    #[target_feature(enable = "sse2")]
    pub unsafe fn count_sse2(needle: u8, hay: &[u8]) -> usize {
        unsafe {
            let n = _mm_set1_epi8(needle as i8);
            let p = hay.as_ptr();
            let len = hay.len();
            let mut total = 0usize;
            let mut i = 0;
            // Each byte lane counts up to 255 blocks before it is summed.
            while i + 16 <= len {
                let mut acc = _mm_setzero_si128();
                let mut blocks = 0;
                while i + 16 <= len && blocks < 255 {
                    let v = _mm_loadu_si128(p.add(i) as *const __m128i);
                    acc = _mm_sub_epi8(acc, _mm_cmpeq_epi8(v, n));
                    i += 16;
                    blocks += 1;
                }
                let sums = _mm_sad_epu8(acc, _mm_setzero_si128());
                total += _mm_cvtsi128_si64(sums) as usize + _mm_cvtsi128_si64(_mm_srli_si128(sums, 8)) as usize;
            }
            total + hay[i..].iter().filter(|&&b| b == needle).count()
        }
    }

    #[target_feature(enable = "sse2")]
    pub unsafe fn memmem_sse2(hay: &[u8], needle: &[u8], i1: usize, i2: usize) -> Option<usize> {
        unsafe {
            let n = needle.len();
            let v1 = _mm_set1_epi8(needle[i1] as i8);
            let v2 = _mm_set1_epi8(needle[i2] as i8);
            let p = hay.as_ptr();
            let last_start = hay.len() - n;
            let mut j = 0;
            // A block covers candidate starts j..j+16; its loads end at j + max(i1, i2) + 16.
            while j + 16 <= last_start + 1 {
                let a = _mm_loadu_si128(p.add(j + i1) as *const __m128i);
                let b = _mm_loadu_si128(p.add(j + i2) as *const __m128i);
                let mut m = _mm_movemask_epi8(_mm_and_si128(_mm_cmpeq_epi8(a, v1), _mm_cmpeq_epi8(b, v2))) as u32;
                while m != 0 {
                    let k = m.trailing_zeros() as usize;
                    if &hay[j + k..j + k + n] == needle {
                        return Some(j + k);
                    }
                    m &= m - 1;
                }
                j += 16;
            }
            memmem_scalar(hay, needle, j)
        }
    }

    #[target_feature(enable = "avx2")]
    pub unsafe fn memmem_avx2(hay: &[u8], needle: &[u8], i1: usize, i2: usize) -> Option<usize> {
        unsafe {
            let n = needle.len();
            let v1 = _mm256_set1_epi8(needle[i1] as i8);
            let v2 = _mm256_set1_epi8(needle[i2] as i8);
            let p = hay.as_ptr();
            let last_start = hay.len() - n;
            let mut j = 0;
            while j + 32 <= last_start + 1 {
                let a = _mm256_loadu_si256(p.add(j + i1) as *const __m256i);
                let b = _mm256_loadu_si256(p.add(j + i2) as *const __m256i);
                let mut m =
                    _mm256_movemask_epi8(_mm256_and_si256(_mm256_cmpeq_epi8(a, v1), _mm256_cmpeq_epi8(b, v2))) as u32;
                while m != 0 {
                    let k = m.trailing_zeros() as usize;
                    if &hay[j + k..j + k + n] == needle {
                        return Some(j + k);
                    }
                    m &= m - 1;
                }
                j += 32;
            }
            memmem_scalar(hay, needle, j)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn naive_memmem(hay: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() {
            return Some(0);
        }
        hay.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn single_byte_searches_match_the_scalar_definitions() {
        let mut rng = Rng(7);
        for len in (0..140).chain([255, 256, 257, 4000, 4096, 4097]) {
            for _ in 0..40 {
                let alphabet = 1 + rng.below(4);
                let hay: Vec<u8> = (0..len).map(|_| b'a' + rng.below(alphabet) as u8).collect();
                let needle = b'a' + rng.below(5) as u8;
                assert_eq!(memchr(needle, &hay), hay.iter().position(|&b| b == needle), "memchr len {len}");
                assert_eq!(memrchr(needle, &hay), hay.iter().rposition(|&b| b == needle), "memrchr len {len}");
                assert_eq!(count(needle, &hay), hay.iter().filter(|&&b| b == needle).count(), "count len {len}");
            }
        }
    }

    #[test]
    fn count_handles_more_than_255_blocks() {
        let hay = vec![b'x'; 16 * 600 + 5];
        assert_eq!(count(b'x', &hay), hay.len());
        assert_eq!(count(b'y', &hay), 0);
    }

    #[test]
    fn memmem_matches_the_scalar_definition() {
        let mut rng = Rng(99);
        for len in (0..200).chain([300, 1000, 4099]) {
            for _ in 0..60 {
                let alphabet = 1 + rng.below(3);
                let hay: Vec<u8> = (0..len).map(|_| b'a' + rng.below(alphabet) as u8).collect();
                let nlen = rng.below(9);
                let needle: Vec<u8> = (0..nlen).map(|_| b'a' + rng.below(alphabet + 1) as u8).collect();
                assert_eq!(memmem(&hay, &needle), naive_memmem(&hay, &needle), "len {len} needle {needle:?}");
            }
        }
    }

    #[test]
    fn memmem_finds_a_needle_at_every_position() {
        let hay: Vec<u8> = (0..200u32).map(|i| b'a' + (i % 7) as u8).collect();
        for start in 0..150 {
            let needle = hay[start..start + 5].to_vec();
            let found = memmem(&hay, &needle);
            assert_eq!(found, naive_memmem(&hay, &needle));
            assert!(found.unwrap() <= start);
        }
    }
}
