//! The matchers.
//!
//! * `Searcher::longest` finds the leftmost-longest match of a pattern
//!   without back-references by a breadth-first simulation of the program
//!   (each thread carries the earliest start that reaches it).
//! * `Backtracker` walks the program depth-first in priority order. It finds
//!   the capture assignment of a known match (the first path, in priority
//!   order, that ends exactly at the known end), and, for patterns with
//!   back-references, also the longest end. It memoizes visited states, so
//!   it never takes exponential time.

use std::collections::{HashMap, HashSet};

use super::ast::{Assert, Unit};
use super::charset::{decode_utf8, decode_utf8_before, is_word_byte, is_word_char, is_word_char_libc, simple_upper};
use super::compile::{GroupCopy, Inst, Program};

const NONE: usize = usize::MAX;

/// Group spans of a match (group 0 is the whole match).
pub type Groups = Vec<Option<(usize, usize)>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    Fail,
    Pass,
    NeedNewline,
}
const RING: usize = 5;
/// Above this many (instruction, position) pairs the capture pass uses no
/// bit tables.
const BIG_TABLE_BITS: usize = 1 << 27;

#[cfg(not(test))]
fn big_table_bits() -> usize {
    BIG_TABLE_BITS
}

#[cfg(test)]
thread_local! {
    /// Lets a test lower the limit to reach the large-program path with a small input.
    pub static TEST_BIG_TABLE_BITS: std::cell::Cell<usize> = const { std::cell::Cell::new(BIG_TABLE_BITS) };
}

#[cfg(test)]
fn big_table_bits() -> usize {
    TEST_BIG_TABLE_BITS.with(|c| c.get())
}

/// Working memory of `Searcher::longest`, kept between searches so that a search allocates
/// nothing once the buffers have grown to the size of the program.
#[derive(Clone, Debug, Default)]
pub struct Scratch {
    m: usize,
    ring: Vec<Vec<(u32, usize)>>,
    slot: Vec<u32>,
    stamp: Vec<usize>,
    generation: usize,
    stack: Vec<(u32, bool)>,
    consuming: Vec<(u32, usize)>,
    /// Buffers of `Backtracker`, handed back when it is dropped.
    bt_caps: Vec<usize>,
    bt_snap: Vec<usize>,
    bt_stamp: Vec<u64>,
    bt_trail: Vec<Undo>,
    bt_stack: Vec<Choice>,
}

impl Scratch {
    fn prepare(&mut self, m: usize) {
        if self.m != m {
            self.m = m;
            self.ring = (0..RING).map(|_| Vec::new()).collect();
            self.slot = vec![u32::MAX; RING * m];
            self.stamp = vec![0; 2 * m];
            self.generation = 0;
        }
    }

    /// Leaves the buffers ready for the next search: no seed is pending.
    fn reset(&mut self) {
        // A search consumes every seed it schedules (none goes past the end of the haystack), and
        // clears the `slot` of each as it does.
        debug_assert!(self.ring.iter().all(Vec::is_empty));
        self.stack.clear();
        self.consuming.clear();
    }
}

pub struct Searcher<'a> {
    pub scratch: &'a std::cell::RefCell<Scratch>,
    pub dfa: &'a std::cell::RefCell<super::dfa::DfaSlot>,
    pub node: &'a super::ast::Node,
    /// Offsets of the groups of a fixed-width pattern (see `dfa::fixed_groups`).
    pub fixed: Option<&'a (usize, Vec<(usize, usize)>)>,
    /// Leading repetitions before a `\B` (see `star_before_not_word_boundary`); 0 for none.
    pub quirk_stars: usize,
    /// `glibc_compat`: the matcher's quirks are reproduced.
    pub glibc: bool,
    /// Off in tests that compare the DFA with the simulation.
    pub use_dfa: bool,
    pub prog: &'a Program,
    pub hay: &'a [u8],
    pub multiline: bool,
    /// The line delimiter: `^` and `$` match next to it in `M` mode.
    pub delim: u8,
    /// Address semantics (`Regex::is_match`): in `M` mode `\`` and `\'`
    /// match at line boundaries like `^` and `$`.
    pub address: bool,
    /// Text is read as characters (UTF-8 or a multibyte character set), not as bytes.
    pub utf8: bool,
    /// The characters are those of a locale of the C library (`Locale::Libc`, `Locale::Mb`).
    pub libc: bool,
    /// The characters are those of a multibyte character set other than UTF-8 (`Locale::Mb`):
    /// they are read with `mbrtowc`, and every search starts at a character boundary.
    pub mb: bool,
    /// A single-byte locale of the C library (`Locale::Sb`).
    pub sb: bool,
    pub icase: bool,
    /// A position known to be a character boundary at or before the one asked about (`Mb`).
    pub bhint: std::cell::Cell<usize>,
}

impl<'a> Searcher<'a> {
    /// The character at `pos` and its length (UTF-8 or the character set of the locale).
    #[inline]
    fn dec(&self, pos: usize) -> Option<(char, usize)> {
        if self.mb { crate::locale::mb_decode(self.hay, pos) } else { decode_utf8(self.hay, pos) }
    }

    /// The character that ends exactly at `pos`, if there is one.
    fn dec_before(&self, pos: usize) -> Option<(char, usize)> {
        if !self.mb {
            return decode_utf8_before(self.hay, pos);
        }
        // The set can have second bytes that look like first bytes, so the line is read forward
        // from a position known to be a boundary.
        let mut p = if self.bhint.get() < pos { self.bhint.get() } else { 0 };
        let mut last = None;
        while p < pos {
            let (step, ch) = match crate::locale::mb_decode(self.hay, p) {
                Some((c, l)) => (l, Some((c, l))),
                None => (1, None),
            };
            last = Some((p, ch));
            p += step;
        }
        match last {
            Some((start, ch)) => {
                self.bhint.set(start);
                if p == pos { ch } else { None }
            }
            None => None,
        }
    }

    fn word_char(&self, c: char) -> bool {
        if self.libc { is_word_char_libc(c) } else { is_word_char(c) }
    }

    fn upper(&self, c: char) -> char {
        if self.libc { crate::locale::upper(c) } else { simple_upper(c) }
    }

    #[inline]
    fn unit_len(&self, u: u32, pos: usize) -> Option<usize> {
        let b = *self.hay.get(pos)?;
        match &self.prog.units[u as usize] {
            Unit::Byte(set) => set.contains(b).then_some(1),
            Unit::Any { exclude } => {
                let (c, len) = self.dec(pos)?;
                (exclude.is_none_or(|b| c as u32 != b as u32 && c != '\n')).then_some(len)
            }
            Unit::Chars(cs) => {
                // An invalid byte matches no set.
                let (c, len) = self.dec(pos)?;
                cs.matches_char(c).then_some(len)
            }
        }
    }

    fn word_after(&self, pos: usize) -> bool {
        if pos >= self.hay.len() {
            return false;
        }
        if self.utf8 {
            // An invalid byte counts as the character of the same number (glibc reads it
            // that way), though `\w` does not match it.
            match self.dec(pos) {
                Some((c, _)) => self.word_char(c),
                None => self.word_char(self.hay[pos] as char),
            }
        } else {
            is_word_byte(self.hay[pos])
        }
    }

    fn word_before(&self, pos: usize) -> bool {
        if pos == 0 {
            return false;
        }
        if self.utf8 {
            match self.dec_before(pos) {
                Some((c, _)) => self.word_char(c),
                None => self.word_char(self.hay[pos - 1] as char),
            }
        } else {
            is_word_byte(self.hay[pos - 1])
        }
    }

    /// Checks an assertion at `pos` for a match that started at `start`.
    ///
    /// Outside `M` mode GNU sed still lets `^` match after a newline that the
    /// match itself consumed, and `$` before a newline that the match goes
    /// on to consume (`a\n^b` and `a$\nb` match "a\nb"). `^` and `$` at the
    /// edges of a match only match at the buffer edges. `Pass` means the
    /// assertion holds; `NeedNewline` that it holds only if the next step
    /// consumes the newline at `pos`.
    fn assert_check(&self, a: Assert, pos: usize, start: usize) -> Check {
        let len = self.hay.len();
        let hold = |b: bool| if b { Check::Pass } else { Check::Fail };
        match a {
            Assert::LineStart => hold(
                pos == 0
                    || (self.multiline && self.hay[pos - 1] == self.delim)
                    || (self.delim == b'\n' && self.hay[pos - 1] == b'\n' && pos > start),
            ),
            Assert::LineEnd => {
                if pos == len || (self.multiline && self.hay[pos] == self.delim) {
                    Check::Pass
                } else if self.delim == b'\n' && self.hay[pos] == b'\n' {
                    Check::NeedNewline
                } else {
                    Check::Fail
                }
            }
            Assert::BufStart if self.address && self.multiline => {
                self.assert_check(Assert::LineStart, pos, start)
            }
            Assert::BufEnd if self.address && self.multiline => self.assert_check(Assert::LineEnd, pos, start),
            Assert::BufStart | Assert::BufEnd => hold(self.assert_ok(a, pos)),
            // Word assertions never hold inside a multibyte character.
            _ => hold(!self.inside_char(pos) && self.assert_ok(a, pos)),
        }
    }

    fn assert_ok(&self, a: Assert, pos: usize) -> bool {
        let len = self.hay.len();
        match a {
            Assert::LineStart | Assert::LineEnd => unreachable!("`assert_check` decides `^` and `$`"),
            Assert::BufStart => pos == 0,
            Assert::BufEnd => pos == len,
            Assert::WordBoundary => self.word_before(pos) != self.word_after(pos),
            Assert::NotWordBoundary => self.word_before(pos) == self.word_after(pos),
            Assert::WordStart => !self.word_before(pos) && self.word_after(pos),
            Assert::WordEnd => self.word_before(pos) && !self.word_after(pos),
        }
    }

    /// Distance to the next candidate start. GNU sed tries every byte
    /// offset, also inside a multibyte character in UTF-8 (`s/\xa9/x/`
    /// matches the second byte of `é`, `s/x*/-/g` splits it); units that
    /// need a whole character simply fail there. Some patterns only start
    /// at character boundaries (`Program::char_starts`).
    fn step_len(&self, pos: usize) -> usize {
        if self.prog.char_starts {
            self.dec(pos).map_or(1, |(_, l)| l)
        } else {
            1
        }
    }

    /// In UTF-8, whether `pos` falls inside a valid multibyte character.
    fn inside_char(&self, pos: usize) -> bool {
        if !self.utf8 || pos == 0 || pos >= self.hay.len() {
            return false;
        }
        if self.mb {
            // Searches start at boundaries and step by characters, so a position inside a
            // character is only asked about by the assertions after a byte unit; read the line.
            let mut p = if self.bhint.get() <= pos { self.bhint.get() } else { 0 };
            while p < pos {
                let step = self.dec(p).map_or(1, |(_, l)| l);
                if p + step > pos {
                    return true;
                }
                p += step;
            }
            return false;
        }
        (1..=3).any(|back| {
            pos >= back && decode_utf8(self.hay, pos - back).is_some_and(|(_, l)| l > back)
        })
    }

    fn may_start(&self, pos: usize) -> bool {
        if self.prog.char_starts && self.inside_char(pos) {
            return false;
        }
        match &self.prog.first_bytes {
            None => true,
            Some(set) => pos < self.hay.len() && set.contains(self.hay[pos]),
        }
    }

    /// Whether there is a match anywhere (address semantics, see `address`):
    /// only the overall match matters, so the capture pass, with its
    /// stricter reading of `$`, is skipped unless back-references need it.
    pub fn exists(&self) -> bool {
        if self.prog.has_backrefs {
            return self.search(0).is_some();
        }
        if self.use_dfa {
            let mut slot = self.dfa.borrow_mut();
            if let Some(set) = slot.get(self.prog, self.node, self.utf8, self.mb, self.delim) {
                if let Ok(found) = set.unanchored.is_match(self.hay) {
                    return found;
                }
            }
        }
        self.longest(0).is_some()
    }

    /// The leftmost-longest match span from `from` by the DFAs; `None` if they do not apply
    /// (the haystack has bytes they do not handle, or the program is out of their reach), and
    /// `Some(None)` for "no match".
    fn dfa_span(&self, from: usize) -> Option<Option<(usize, usize)>> {
        if !self.use_dfa {
            return None;
        }
        let mut slot = self.dfa.borrow_mut();
        let set = slot.get(self.prog, self.node, self.utf8, self.mb, self.delim)?;
        // The earliest end of any match, then the leftmost start of a match ending there. A match
        // that starts earlier must end later: try those starts one by one (within a budget).
        let hay = self.hay;
        let Ok(first) = set.unanchored.first_end(hay, from) else { return None };
        let Some(e1) = first else { return Some(None) };
        let Ok(Some(s1)) = set.reverse.leftmost_start(hay, from, e1) else { return None };
        let mut budget = 256 + 4 * (e1 - from);
        for s in from..s1 {
            if self.prog.first_bytes.as_ref().is_some_and(|fb| !fb.contains(hay[s])) {
                continue;
            }
            match set.forward.longest_from(hay, s, &mut budget) {
                Ok(Some(e)) => return Some(Some((s, e))),
                Ok(None) => {}
                Err(_) => return None,
            }
        }
        let mut unlimited = usize::MAX;
        match set.forward.longest_from(hay, s1, &mut unlimited) {
            Ok(Some(e)) => Some(Some((s1, e))),
            _ => None,
        }
    }

    /// The empty match glibc reports instead of one at `s` when a leading repetition could consume
    /// a character there: it is put after that character (see `star_before_not_word_boundary`).
    fn end_of_star_run(&self, s: usize) -> (usize, usize) {
        let l = self.unit_len(0, s).filter(|&l| l > 0).expect("the caller saw a character here");
        (s + l, s + l)
    }

    /// Finds the leftmost-longest match starting at or after `start`.
    pub fn search(&self, start: usize) -> Option<(usize, Groups)> {
        let len = self.hay.len();
        let mut from = start;
        loop {
            debug_assert!(from <= len, "a search starts inside the haystack and steps by whole characters");
            let (s, e) = if self.prog.has_backrefs {
                let mut bt = Backtracker::new(self, false);
                let mut s = from;
                loop {
                    if self.may_start(s) {
                        if let Some(e) = bt.run(s, None) {
                            break (s, e);
                        }
                        // glibc gives up the whole search when its attempt at the first position
                        // fails because it refuses an empty group there (`(a?){2}\1` on `baaa`).
                        if s == start && bt.hit {
                            return None;
                        }
                    }
                    if s >= len {
                        return None;
                    }
                    s += self.step_len(s);
                }
            } else {
                match self.dfa_span(from) {
                    Some(span) => span?,
                    None => self.longest(from)?,
                }
            };
            let shifted = self.quirk_stars > 0 && s == e && s > 0 && self.unit_len(0, s).is_some_and(|l| l > 0);
            let (s, e) = if shifted { self.end_of_star_run(s) } else { (s, e) };
            let mut groups = vec![None; self.prog.ngroups + 1];
            groups[0] = Some((s, e));
            if shifted {
                // The groups, if any, took no part in the match glibc reports.
                return Some((s, groups));
            }
            if self.prog.ngroups == 0 {
                return Some((s, groups));
            }
            if let Some((width, offsets)) = self.fixed {
                // Every unit of such a pattern takes exactly one byte.
                debug_assert_eq!(e - s, *width);
                for (slot, &(a, b)) in groups.iter_mut().zip(offsets).skip(1) {
                    *slot = Some((s + a, s + b));
                }
                return Some((s, groups));
            }
            // GNU sed assigns the subexpressions with the strict reading of
            // `$` (see `assert_check`); when no such path reaches the end,
            // it gives up this start and searches on.
            let mut bt = Backtracker::new(self, true);
            // The registers of a group like `(a*)*` come from the first path that reaches `e`
            // without an empty iteration after a non-empty one, if there is one; the empty
            // iteration only makes the back-reference match (`(a*)*\1b` on `ab`).
            bt.no_empty = self.prog.has_backrefs && self.glibc;
            let found = bt.run(s, Some(e)).is_some() || {
                bt.no_empty = false;
                bt.run(s, Some(e)).is_some()
            };
            if found {
                for (g, slot) in groups.iter_mut().enumerate().skip(1) {
                    let (a, b) = (bt.caps[2 * g], bt.caps[2 * g + 1]);
                    if a != NONE && b != NONE {
                        *slot = Some((a, b));
                    }
                }
                return Some((s, groups));
            }
            if s >= len {
                return None;
            }
            from = s + self.step_len(s);
        }
    }

    /// Breadth-first simulation for patterns without back-references.
    fn longest(&self, start: usize) -> Option<(usize, usize)> {
        let prog = self.prog;
        let m = prog.insts.len();
        let len = self.hay.len();
        let mut scratch = self.scratch.borrow_mut();
        scratch.prepare(m);
        scratch.reset();
        let Scratch { ring, slot, stamp, generation, stack, consuming, .. } = &mut *scratch;
        let mut pending = 0usize;
        let mut best: Option<(usize, usize)> = None;
        // Closure entries carry whether the path must consume a newline next (in `stack`).
        let mut next_cand = start;
        let mut p = start;
        loop {
            let r = p % RING;
            let mut seeds = std::mem::take(&mut ring[r]);
            for &(pc, _) in &seeds {
                slot[r * m + pc as usize] = u32::MAX;
            }
            pending -= seeds.len();
            if seeds.len() > 1 {
                seeds.sort_by_key(|&(_, st)| st);
            }
            let is_cand = p == next_cand;
            if is_cand && p < len {
                next_cand = p + self.step_len(p);
            }
            if best.is_none() && is_cand && self.may_start(p) {
                seeds.push((0, p));
            }
            if seeds.is_empty() {
                if p >= len || (best.is_some() && pending == 0) {
                    ring[r] = seeds;
                    break;
                }
                ring[r] = seeds;
                p += 1;
                continue;
            }
            *generation += 1;
            let generation = *generation;
            consuming.clear();
            for &(pc0, st) in &seeds {
                if let Some((bs, _)) = best {
                    if st > bs {
                        continue;
                    }
                }
                stack.push((pc0, false));
                while let Some((pc, nl)) = stack.pop() {
                    let pcu = pc as usize;
                    let si = pcu + if nl { m } else { 0 };
                    if stamp[si] == generation {
                        continue;
                    }
                    stamp[si] = generation;
                    match prog.insts[pcu] {
                        Inst::Unit(_) => consuming.push((pc, st)),
                        Inst::Split(a, b) => {
                            stack.push((b, nl));
                            stack.push((a, nl));
                        }
                        Inst::Jmp(a) => stack.push((a, nl)),
                        Inst::Open(_) | Inst::Close(..) => stack.push((pc + 1, nl)),
                        Inst::Assert(a) => match self.assert_check(a, p, st) {
                            Check::Pass => stack.push((pc + 1, nl)),
                            Check::NeedNewline => stack.push((pc + 1, true)),
                            Check::Fail => {}
                        },
                        Inst::Backref(_) => unreachable!("the simulation runs only without back-references"),
                        Inst::Match if nl => {}
                        Inst::Match => match best {
                            None => best = Some((st, p)),
                            Some((bs, be)) => {
                                if st < bs || (st == bs && p > be) {
                                    best = Some((st, p));
                                }
                            }
                        },
                    }
                }
            }
            seeds.clear();
            ring[r] = seeds;
            if p < len {
                for &(pc, st) in consuming.iter() {
                    // A seed that started after the best match was dropped above.
                    debug_assert!(best.is_none_or(|(bs, _)| st <= bs));
                    let Inst::Unit(u) = prog.insts[pc as usize] else { unreachable!() };
                    if let Some(l) = self.unit_len(u, p) {
                        let q = (p + l) % RING;
                        let npc = pc + 1;
                        let k = q * m + npc as usize;
                        if slot[k] == u32::MAX {
                            slot[k] = ring[q].len() as u32;
                            ring[q].push((npc, st));
                            pending += 1;
                        } else {
                            // The seeds are taken in order of their start, so the thread that is
                            // already there started no later.
                            debug_assert!(ring[q][slot[k] as usize].1 <= st);
                        }
                    }
                }
            }
            if p >= len {
                break;
            }
            p += 1;
        }
        best
    }
}

/// Depth-first matcher in priority order, with capture registers.
///
/// Capture rules (observed from GNU sed): an `Open` records the start and
/// clears the end. A `Close` that ends a non-empty match records the end and
/// takes a snapshot of all registers. A `Close` that ends an empty match of a
/// group that is the whole body of an optional repetition, when that group
/// already has a value in the snapshot, restores all registers from the
/// snapshot (so an empty last iteration does not erase the previous one);
/// otherwise it records the empty match. When it restores, and the group is the body of the
/// loop of `*`, `+` or `{m,}` and is not itself a repetition, a back-reference sees the group
/// as empty at that position (`empty_slot`) although the registers hold the earlier value.
///
/// Loops: between two consumed characters a path may come back to a `Split`
/// whose preferred alternative it already went through; it then takes the
/// other alternative (so an empty iteration ends the loop instead of
/// killing the path), and a `Split` whose two alternatives were both taken
/// ends the path.
///
/// Without back-references a table of viable states (those from which the
/// known match end is reachable) is computed first, backwards, so the walk
/// never strays; with back-references the walk backtracks, memoizing the
/// states reached right after consuming input together with the registers
/// that back-references can read.
struct Backtracker<'s, 'a> {
    s: &'s Searcher<'a>,
    caps: Vec<usize>,
    snap: Vec<usize>,
    trail: Vec<Undo>,
    stack: Vec<Choice>,
    stamp: Vec<u64>,
    seg: u64,
    next_seg: u64,
    viable: Vec<u64>,
    free_a: Vec<u64>,
    free_b: Vec<u64>,
    /// Keep to paths with no assertion after the last character.
    halt_free: bool,
    base: usize,
    memo_bits: Vec<u64>,
    memo_set: HashSet<Vec<usize>>,
    keyed: Vec<usize>,
    /// `$` only matches at a real line end (no newline consumed later).
    strict: bool,
    /// The match is too long for the bit tables (see `run`).
    big: bool,
    /// For the current start: the instructions reached at each position by a walk of the program
    /// that ignores back-references (see `copy_ok`), once needed.
    reach: Option<Vec<u64>>,
    /// The positions at which a copy of a group, entered at a position, can close (see `copy_ok`),
    /// by the closing instruction and the entry position.
    body_closes: HashMap<(u32, usize), Vec<usize>>,
    /// Paths that end an iteration of a group whose body is a repetition empty after a non-empty
    /// one are not taken (see `Searcher::search`).
    no_empty: bool,
    /// The last back-reference `copy_ok` refused was one to an empty group at the start of the
    /// match.
    refused: bool,
    /// Some path that `copy_ok` refused for that reason would have matched (see `Searcher::search`).
    hit: bool,
}

/// `copy_ok` looks at matches that are not longer than this many bytes, and at groups with at
/// most this many copies.
const COPY_SPAN_LIMIT: usize = 256;
const MAX_COPIES: usize = 16;

#[derive(Clone, Copy, Debug)]
enum Undo {
    Cap(u32, usize),
    Snap(u32, usize),
    Stamp(u32, u64),
}

#[derive(Clone, Copy, Debug)]
struct Choice {
    pc: u32,
    pos: usize,
    trail: usize,
    seg: u64,
    /// The path must consume the newline at `pos` next (see `assert_check`).
    nl: bool,
    /// An assertion was checked since the last consumed character.
    c: bool,
}

impl Drop for Backtracker<'_, '_> {
    fn drop(&mut self) {
        if let Ok(mut sc) = self.s.scratch.try_borrow_mut() {
            sc.bt_caps = std::mem::take(&mut self.caps);
            sc.bt_snap = std::mem::take(&mut self.snap);
            sc.bt_stamp = std::mem::take(&mut self.stamp);
            sc.bt_trail = std::mem::take(&mut self.trail);
            sc.bt_stack = std::mem::take(&mut self.stack);
        }
    }
}

impl<'s, 'a> Backtracker<'s, 'a> {
    fn new(s: &'s Searcher<'a>, strict: bool) -> Self {
        // Two registers per group, then four more per group: whether it was last closed in a
        // middle copy of a bounded repetition (`Inst::Close`), which instruction closed it, and
        // where and by which instruction an empty iteration closed it (`empty_slot`); and a last
        // register for the shadow mark (see the `Backref` case of `run`).
        let n = if s.prog.quirks { 6 * (s.prog.ngroups + 1) + 1 } else { 4 * (s.prog.ngroups + 1) };
        let keyed = (1..=s.prog.ngroups).filter(|&g| s.prog.referenced[g]).collect();
        let (mut caps, mut snap, mut stamp, mut trail, mut stack) = {
            let mut sc = s.scratch.borrow_mut();
            (
                std::mem::take(&mut sc.bt_caps),
                std::mem::take(&mut sc.bt_snap),
                std::mem::take(&mut sc.bt_stamp),
                std::mem::take(&mut sc.bt_trail),
                std::mem::take(&mut sc.bt_stack),
            )
        };
        caps.clear();
        caps.resize(n, NONE);
        snap.clear();
        snap.resize(n, NONE);
        stamp.clear();
        stamp.resize(s.prog.insts.len(), 0);
        trail.clear();
        stack.clear();
        Backtracker {
            s,
            caps,
            snap,
            trail,
            stack,
            stamp,
            seg: 1,
            next_seg: 2,
            viable: Vec::new(),
            free_a: Vec::new(),
            free_b: Vec::new(),
            halt_free: false,
            base: 0,
            memo_bits: Vec::new(),
            memo_set: HashSet::new(),
            keyed,
            strict,
            big: false,
            reach: None,
            body_closes: HashMap::new(),
            hit: false,
            no_empty: false,
            refused: false,
        }
    }

    #[inline]
    fn set_cap(&mut self, i: usize, v: usize) {
        if self.caps[i] != v {
            self.trail.push(Undo::Cap(i as u32, self.caps[i]));
            self.caps[i] = v;
        }
    }

    #[inline]
    fn set_snap(&mut self, i: usize, v: usize) {
        if self.snap[i] != v {
            self.trail.push(Undo::Snap(i as u32, self.snap[i]));
            self.snap[i] = v;
        }
    }

    #[inline]
    fn mark(&mut self, pc: u32) {
        let old = self.stamp[pc as usize];
        if old != self.seg {
            self.trail.push(Undo::Stamp(pc, old));
            self.stamp[pc as usize] = self.seg;
        }
    }

    #[inline]
    fn seen(&self, pc: u32) -> bool {
        self.stamp[pc as usize] == self.seg
    }

    fn undo(&mut self, to: usize) {
        while self.trail.len() > to {
            match self.trail.pop().unwrap() {
                Undo::Cap(i, v) => self.caps[i as usize] = v,
                Undo::Snap(i, v) => self.snap[i as usize] = v,
                Undo::Stamp(pc, v) => self.stamp[pc as usize] = v,
            }
        }
    }

    fn new_segment(&mut self) {
        self.seg = self.next_seg;
        self.next_seg += 1;
    }

    #[inline]
    fn bit(t: &[u64], k: usize) -> bool {
        t[k / 64] & (1 << (k % 64)) != 0
    }

    #[inline]
    fn set_bit(t: &mut [u64], k: usize) {
        t[k / 64] |= 1 << (k % 64);
    }

    /// Whether the walk may be in this state. `c` tells that an assertion
    /// was checked since the last consumed character.
    #[inline]
    fn is_viable(&self, pc: u32, pos: usize, c: bool) -> bool {
        if self.viable.is_empty() {
            return true;
        }
        let m = self.s.prog.insts.len();
        let k = (pos - self.base) * m + pc as usize;
        if !self.halt_free {
            return Self::bit(&self.viable, k);
        }
        Self::bit(&self.free_a, k) || (!c && Self::bit(&self.free_b, k))
    }

    /// Backward reachability of `(Match, end)` for patterns without
    /// back-references.
    ///
    /// GNU sed prefers a path that reaches the end without checking an
    /// assertion after its last character: `(ab|a)(\b|b)` on `ab` gives `a`
    /// and `b`. So besides the plain table (`viable`) two more are computed:
    /// `free_a`, states that reach the end through at least one more
    /// character with no assertion after the last one, and `free_b`, states
    /// at the end that reach `Match` without any assertion. When such a path
    /// exists from the start, the walk keeps to them.
    fn compute_viable(&mut self, start: usize, end: usize) {
        let prog = self.s.prog;
        let m = prog.insts.len();
        let rows = end - start + 1;
        let words = (rows * m).div_ceil(64);
        for t in [&mut self.viable, &mut self.free_a, &mut self.free_b] {
            t.clear();
            t.resize(words, 0);
        }
        self.base = start;
        let preds = &prog.preds;
        let mut work: Vec<u32> = Vec::new();
        let strict = self.strict;
        let s = self.s;
        let assert_passes = |a: Assert, pos: usize| match s.assert_check(a, pos, start) {
            Check::Pass => true,
            Check::NeedNewline => pos != end && !strict,
            Check::Fail => false,
        };
        for pos in (start..=end).rev() {
            let row = (pos - start) * m;
            // free_b: pure epsilon paths to Match without assertions.
            if pos == end {
                work.clear();
                for pc in 0..m {
                    if matches!(prog.insts[pc], Inst::Match) {
                        Self::set_bit(&mut self.free_b, row + pc);
                        work.push(pc as u32);
                    }
                }
                while let Some(q) = work.pop() {
                    for &p in &preds[q as usize] {
                        let k = row + p as usize;
                        if Self::bit(&self.free_b, k) || matches!(prog.insts[p as usize], Inst::Assert(_)) {
                            continue;
                        }
                        Self::set_bit(&mut self.free_b, k);
                        work.push(p);
                    }
                }
            }
            for (table, free) in [(0, false), (1, true)] {
                work.clear();
                for pc in 0..m {
                    let ok = match prog.insts[pc] {
                        Inst::Match => !free && pos == end,
                        Inst::Unit(u) => match s.unit_len(u, pos) {
                            Some(l) if pos + l <= end => {
                                let k = (pos + l - start) * m + pc + 1;
                                if free {
                                    Self::bit(&self.free_a, k) || Self::bit(&self.free_b, k)
                                } else {
                                    Self::bit(&self.viable, k)
                                }
                            }
                            _ => false,
                        },
                        _ => false,
                    };
                    if ok {
                        let t = if table == 0 { &mut self.viable } else { &mut self.free_a };
                        Self::set_bit(t, row + pc);
                        work.push(pc as u32);
                    }
                }
                while let Some(q) = work.pop() {
                    for &p in &preds[q as usize] {
                        let k = row + p as usize;
                        let t = if table == 0 { &self.viable } else { &self.free_a };
                        if Self::bit(t, k) {
                            continue;
                        }
                        if let Inst::Assert(a) = prog.insts[p as usize] {
                            if !assert_passes(a, pos) {
                                continue;
                            }
                        }
                        let t = if table == 0 { &mut self.viable } else { &mut self.free_a };
                        Self::set_bit(t, k);
                        work.push(p);
                    }
                }
            }
        }
        self.halt_free = Self::bit(&self.free_a, 0) || Self::bit(&self.free_b, 0);
    }

    /// Marks a state reached right after consuming input; false if it was
    /// reached before.
    fn memo(&mut self, pc: u32, pos: usize) -> bool {
        if self.big {
            return self.memo_set.insert(vec![pc as usize, pos]);
        }
        if self.s.prog.has_backrefs {
            let mut key = Vec::with_capacity(4 + 10 * self.keyed.len());
            key.push(pc as usize);
            key.push(pos);
            for &g in &self.keyed {
                key.extend_from_slice(&self.caps[2 * g..2 * g + 2]);
                key.extend_from_slice(&self.snap[2 * g..2 * g + 2]);
                key.push(self.caps[self.mid_slot(g)]);
                key.push(self.snap[self.mid_slot(g)]);
                key.push(self.caps[self.closer_slot(g)]);
                key.push(self.snap[self.closer_slot(g)]);
                if self.s.prog.quirks {
                    key.push(self.caps[self.empty_slot(g)]);
                    key.push(self.caps[self.empty_closer_slot(g)]);
                }
            }
            if self.s.prog.quirks {
                key.push(self.caps[6 * (self.s.prog.ngroups + 1)]);
            }
            let mut bitsv = 0usize;
            for g in 1..=self.s.prog.ngroups.min(63) {
                if self.snap[2 * g] != NONE {
                    bitsv |= 1 << g;
                }
            }
            key.push(bitsv);
            return self.memo_set.insert(key);
        }
        let m = self.s.prog.insts.len();
        let k = (pos - self.base) * m + pc as usize;
        if self.memo_bits[k / 64] & (1 << (k % 64)) != 0 {
            return false;
        }
        self.memo_bits[k / 64] |= 1 << (k % 64);
        true
    }

    /// The register of group `g` that tells whether it was last closed in a middle copy of a
    /// bounded repetition.
    #[inline]
    fn mid_slot(&self, g: usize) -> usize {
        2 * (self.s.prog.ngroups + 1) + g
    }

    /// Whether the value of group `g` may be used by a back-reference.
    ///
    /// GNU sed does not find every match that back-references allow when the group is repeated
    /// and its copies are told apart only by position: if another copy of the group could also
    /// end where the group did, the value is not usable when that copy is an earlier one that
    /// starts no later, or a later one that starts later. Exceptions: for a group whose body
    /// cannot be empty, an optional copy is not hindered by a mandatory one that starts where it
    /// does; for one whose body can be empty, a mandatory copy is not hindered by an optional one.
    /// The copies are those of the program (a repetition `{m,n}` is expanded to `m` mandatory and
    /// `n - m` optional copies; `+` and `*` end in one looping copy). Where a copy could start is
    /// found by walking the program from the start of the match, ignoring back-references.
    /// An empty last iteration of a loop that gave its earlier value back counts as a copy that
    /// ends where it is (see `empty_slot`).
    fn copy_ok(&mut self, g: usize, start: usize, pos: usize) -> bool {
        let prog = self.s.prog;
        let copies = &prog.copies[g];
        if copies.is_empty() || copies.len() > MAX_COPIES {
            return true;
        }
        let (mut s, mut e, mut closer) = (self.caps[2 * g], self.caps[2 * g + 1], self.caps[self.closer_slot(g)]);
        let ep = if prog.quirks { self.caps[self.empty_slot(g)] } else { NONE };
        if ep != NONE {
            (s, e, closer) = (ep, ep, self.caps[self.empty_closer_slot(g)]);
        }
        if s == NONE || e == NONE || closer == NONE || e > start + COPY_SPAN_LIMIT {
            return true;
        }
        let words = prog.insts.len().div_ceil(64);
        if self.reach.is_none() {
            let limit = (start + COPY_SPAN_LIMIT).min(self.s.hay.len());
            self.reach = Some(self.simulate(start, start, limit, 0, None, words));
        }
        let closer = closer as u32;
        let mandatory = |q: u32| copies.iter().find(|c| c.close == q).is_none_or(|c| c.mandatory);
        let nullable = copies[0].nullable;
        for c in copies {
            if c.close == closer {
                continue;
            }
            for open_pos in start..=e {
                let row = (open_pos - start) * words;
                let reach = self.reach.as_ref().unwrap();
                if reach[row + c.open as usize / 64] & (1 << (c.open % 64)) == 0 {
                    continue;
                }
                if !self.closes_at(c, open_pos, e) {
                    continue;
                }
                let conflict = if c.close < closer {
                    open_pos < s || (open_pos == s && !(!nullable && mandatory(c.close) && !mandatory(closer)))
                } else {
                    open_pos > s && !(nullable && mandatory(closer) && !mandatory(c.close))
                };
                if conflict {
                    self.refused = s == e && s == start && pos == start;
                    return false;
                }
            }
        }
        true
    }

    /// Whether the copy `c` of a group, entered at `open_pos`, can close at `e`.
    fn closes_at(&mut self, c: &GroupCopy, open_pos: usize, e: usize) -> bool {
        if !self.body_closes.contains_key(&(c.close, open_pos)) {
            let words = self.s.prog.insts.len().div_ceil(64);
            let limit = (open_pos + COPY_SPAN_LIMIT).min(self.s.hay.len());
            let table = self.simulate(open_pos, open_pos, limit, c.open, Some(c.close), words);
            let closes: Vec<usize> = (0..=limit - open_pos)
                .filter(|&p| table[p * words + c.close as usize / 64] & (1 << (c.close % 64)) != 0)
                .map(|p| open_pos + p)
                .collect();
            self.body_closes.insert((c.close, open_pos), closes);
        }
        self.body_closes[&(c.close, open_pos)].binary_search(&e).is_ok()
    }

    /// Which instructions can be reached from `entry` at `from`, at each position up to `limit`
    /// (row `p - from`, one bit per instruction). Back-references and `Match` lead nowhere, and
    /// a path does not go past `stop`.
    fn simulate(&self, start: usize, from: usize, limit: usize, entry: u32, stop: Option<u32>, words: usize) -> Vec<u64> {
        let s = self.s;
        let prog = s.prog;
        let rows = limit - from + 1;
        let mut table = vec![0u64; rows * words];
        let mut seeds: Vec<Vec<u32>> = vec![Vec::new(); rows];
        seeds[0].push(entry);
        let mut stack: Vec<u32> = Vec::new();
        for p in 0..rows {
            stack.clear();
            stack.append(&mut seeds[p]);
            let row = p * words;
            while let Some(pc) = stack.pop() {
                let (w, b) = (row + pc as usize / 64, 1u64 << (pc % 64));
                if table[w] & b != 0 {
                    continue;
                }
                table[w] |= b;
                match prog.insts[pc as usize] {
                    Inst::Split(a, b) => {
                        stack.push(b);
                        stack.push(a);
                    }
                    Inst::Jmp(a) => stack.push(a),
                    Inst::Open(_) => stack.push(pc + 1),
                    Inst::Close(..) => {
                        if stop != Some(pc) {
                            stack.push(pc + 1);
                        }
                    }
                    Inst::Assert(a) => {
                        if s.assert_check(a, from + p, start) != Check::Fail {
                            stack.push(pc + 1);
                        }
                    }
                    Inst::Unit(u) => {
                        if let Some(l) = s.unit_len(u, from + p) {
                            if p + l < rows {
                                seeds[p + l].push(pc + 1);
                            }
                        }
                    }
                    Inst::Backref(_) | Inst::Match => {}
                }
            }
        }
        table
    }

    /// The register that holds the instruction that closed group `g` last.
    #[inline]
    fn closer_slot(&self, g: usize) -> usize {
        3 * (self.s.prog.ngroups + 1) + g
    }

    /// The register that holds the position of the empty iteration that last closed group `g`
    /// and gave its earlier value back (else `NONE`): a back-reference then sees an empty group
    /// at that position, though the group is reported with the earlier value.
    #[inline]
    fn empty_slot(&self, g: usize) -> usize {
        4 * (self.s.prog.ngroups + 1) + g
    }

    /// The register that holds the instruction of that empty iteration.
    #[inline]
    fn empty_closer_slot(&self, g: usize) -> usize {
        5 * (self.s.prog.ngroups + 1) + g
    }

    fn backref_len(&self, g: usize, pos: usize) -> Option<usize> {
        let (a, b) = (self.caps[2 * g], self.caps[2 * g + 1]);
        if a == NONE || b == NONE || self.caps[self.mid_slot(g)] == 1 {
            return None;
        }
        if self.s.prog.quirks && self.caps[self.empty_slot(g)] != NONE {
            return Some(0);
        }
        let hay = self.s.hay;
        let sub = &hay[a..b];
        if !self.s.icase {
            return hay[pos..].starts_with(sub).then_some(sub.len());
        }
        if self.s.sb {
            let end = pos + sub.len();
            let same = |x: &u8, y: &u8| x == y || crate::locale::byte_upper(*x) == crate::locale::byte_upper(*y);
            return (end <= hay.len() && hay[pos..end].iter().zip(sub).all(|(x, y)| same(x, y))).then_some(sub.len());
        }
        if !self.s.utf8 {
            let end = pos + sub.len();
            return (end <= hay.len() && hay[pos..end].eq_ignore_ascii_case(sub)).then_some(sub.len());
        }
        let (mut i, mut j) = (a, pos);
        while i < b {
            match (self.s.dec(i), self.s.dec(j)) {
                (Some((c1, l1)), Some((c2, l2))) => {
                    if c1 != c2 && self.s.upper(c1) != self.s.upper(c2) {
                        return None;
                    }
                    i += l1;
                    j += l2;
                }
                _ => {
                    if j >= hay.len() || hay[i] != hay[j] {
                        return None;
                    }
                    i += 1;
                    j += 1;
                }
            }
        }
        Some(j - pos)
    }

    /// Runs from `start`. With `target = Some(e)`, returns as soon as the
    /// first path in priority order reaches `Match` at `e`, leaving its
    /// registers in `caps`. With `None`, explores every path and returns the
    /// longest end.
    fn run(&mut self, start: usize, target: Option<usize>) -> Option<usize> {
        let prog = self.s.prog;
        self.caps.fill(NONE);
        self.snap.fill(NONE);
        self.trail.clear();
        self.stack.clear();
        self.memo_set.clear();
        self.viable.clear();
        self.big = false;
        self.hit = false;
        self.reach = None;
        if !prog.has_backrefs {
            let e = target.expect("only searched with back-references");
            let nbits = (e - start + 1) * prog.insts.len();
            self.base = start;
            // Very long matches of large programs: no tables (they would
            // take too much memory) and a hashed memo instead of a bitmap.
            self.big = nbits > big_table_bits();
            if !self.big {
                self.compute_viable(start, e);
                self.memo_bits.clear();
                self.memo_bits.resize(nbits.div_ceil(64), 0);
            }
        }
        let limit = target.unwrap_or(self.s.hay.len());
        let mut best: Option<usize> = None;
        self.new_segment();
        self.memo(0, start);
        self.stack.push(Choice { pc: 0, pos: start, trail: 0, seg: self.seg, nl: false, c: false });
        while let Some(ch) = self.stack.pop() {
            self.undo(ch.trail);
            self.seg = ch.seg;
            let (mut pc, mut pos, mut nl, mut c) = (ch.pc, ch.pos, ch.nl, ch.c);
            loop {
                if pos > limit || !self.is_viable(pc, pos, c) {
                    break;
                }
                self.mark(pc);
                match prog.insts[pc as usize] {
                    Inst::Unit(u) => match self.s.unit_len(u, pos) {
                        Some(l) => {
                            pos += l;
                            pc += 1;
                            nl = false;
                            c = false;
                            self.new_segment();
                            if !self.memo(pc, pos) {
                                break;
                            }
                        }
                        None => break,
                    },
                    Inst::Split(a, b) => {
                        if self.seen(a) {
                            if self.seen(b) {
                                break;
                            }
                            pc = b;
                        } else {
                            self.stack.push(Choice { pc: b, pos, trail: self.trail.len(), seg: self.seg, nl, c });
                            pc = a;
                        }
                    }
                    Inst::Jmp(a) => pc = a,
                    Inst::Open(g) => {
                        let g = g as usize;
                        self.set_cap(2 * g, pos);
                        self.set_cap(2 * g + 1, NONE);
                        pc += 1;
                    }
                    Inst::Close(g, opt, mid, lk) => {
                        let g = g as usize;
                        let slot = self.mid_slot(g);
                        if self.no_empty && lk == 3 && self.caps[2 * g] == pos && self.snap[2 * g] != NONE {
                            break;
                        }
                        if self.caps[2 * g] == pos {
                            if opt && self.snap[2 * g] != NONE {
                                for i in 0..self.caps.len() {
                                    let v = self.snap[i];
                                    self.set_cap(i, v);
                                }
                                if lk != 0 && prog.quirks {
                                    self.set_cap(self.empty_slot(g), pos);
                                    self.set_cap(self.empty_closer_slot(g), pc as usize);
                                }
                            } else {
                                self.set_cap(2 * g + 1, pos);
                                if prog.quirks {
                                    self.set_cap(self.empty_slot(g), NONE);
                                }
                                self.set_cap(slot, mid as usize);
                                self.set_cap(self.closer_slot(g), pc as usize);
                            }
                        } else {
                            self.set_cap(2 * g + 1, pos);
                            if prog.quirks {
                                self.set_cap(self.empty_slot(g), NONE);
                            }
                            self.set_cap(slot, mid as usize);
                            self.set_cap(self.closer_slot(g), pc as usize);
                            for i in 0..self.caps.len() {
                                let v = self.caps[i];
                                self.set_snap(i, v);
                            }
                        }
                        pc += 1;
                    }
                    Inst::Assert(a) => {
                        match self.s.assert_check(a, pos, start) {
                            Check::Pass => {}
                            Check::NeedNewline if !self.strict => nl = true,
                            Check::NeedNewline | Check::Fail => break,
                        }
                        c = true;
                        pc += 1;
                    }
                    Inst::Backref(g) => {
                        if !self.copy_ok(g as usize, start, pos) {
                            // The path goes on in shadow: if it reaches a match, glibc's attempt
                            // at `start` fails although it would have found one.
                            let sh = 6 * (prog.ngroups + 1);
                            if !prog.quirks {
                                break;
                            }
                            if self.caps[sh] != 1 {
                                if !self.refused {
                                    break;
                                }
                                self.set_cap(sh, 1);
                            }
                        }
                        match self.backref_len(g as usize, pos) {
                            Some(l) => {
                                pos += l;
                                pc += 1;
                                if l > 0 {
                                    nl = false;
                                    c = false;
                                    self.new_segment();
                                    if !self.memo(pc, pos) {
                                        break;
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                    Inst::Match if nl => break,
                    Inst::Match if prog.quirks && self.caps[6 * (prog.ngroups + 1)] == 1 => {
                        self.hit = true;
                        break;
                    }
                    Inst::Match => {
                        match target {
                            Some(e) => {
                                if pos == e {
                                    return Some(e);
                                }
                            }
                            None => {
                                if best.is_none_or(|b| pos > b) {
                                    best = Some(pos);
                                }
                            }
                        }
                        break;
                    }
                }
            }
        }
        best
    }
}
