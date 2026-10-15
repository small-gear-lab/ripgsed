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

use std::collections::HashSet;

use super::ast::{Assert, Unit};
use super::charset::{decode_utf8, decode_utf8_before, is_word_byte, is_word_char, simple_lower};
use super::compile::{Inst, Program};

const NONE: usize = usize::MAX;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    Fail,
    Pass,
    NeedNewline,
}
const RING: usize = 5;

pub struct Searcher<'a> {
    pub prog: &'a Program,
    pub hay: &'a [u8],
    pub multiline: bool,
    pub utf8: bool,
    pub icase: bool,
}

impl<'a> Searcher<'a> {
    #[inline]
    fn unit_len(&self, u: u32, pos: usize) -> Option<usize> {
        let b = *self.hay.get(pos)?;
        match &self.prog.units[u as usize] {
            Unit::Byte(set) => set.contains(b).then_some(1),
            Unit::Any { newline } => {
                let (c, len) = decode_utf8(self.hay, pos)?;
                (*newline || c != '\n').then_some(len)
            }
            Unit::Chars(cs) => {
                if let Some((c, len)) = decode_utf8(self.hay, pos) {
                    return cs.matches_char(c).then_some(len);
                }
                // An invalid byte matches only a raw byte written in the pattern.
                (!cs.negated && cs.raw.contains(b)).then_some(1)
            }
        }
    }

    fn word_after(&self, pos: usize) -> bool {
        if pos >= self.hay.len() {
            return false;
        }
        if self.utf8 {
            match decode_utf8(self.hay, pos) {
                Some((c, _)) => is_word_char(c),
                None => false,
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
            match decode_utf8_before(self.hay, pos) {
                Some((c, _)) => is_word_char(c),
                None => false,
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
            Assert::LineStart => {
                hold(pos == 0 || (self.hay[pos - 1] == b'\n' && (self.multiline || pos > start)))
            }
            Assert::LineEnd => {
                if pos == len || (self.multiline && self.hay[pos] == b'\n') {
                    Check::Pass
                } else if self.hay[pos] == b'\n' {
                    Check::NeedNewline
                } else {
                    Check::Fail
                }
            }
            _ => hold(self.assert_ok(a, pos)),
        }
    }

    fn assert_ok(&self, a: Assert, pos: usize) -> bool {
        let len = self.hay.len();
        match a {
            Assert::LineStart => pos == 0 || (self.multiline && self.hay[pos - 1] == b'\n'),
            Assert::LineEnd => pos == len || (self.multiline && self.hay[pos] == b'\n'),
            Assert::BufStart => pos == 0,
            Assert::BufEnd => pos == len,
            Assert::WordBoundary => self.word_before(pos) != self.word_after(pos),
            Assert::NotWordBoundary => self.word_before(pos) == self.word_after(pos),
            Assert::WordStart => !self.word_before(pos) && self.word_after(pos),
            Assert::WordEnd => self.word_before(pos) && !self.word_after(pos),
        }
    }

    /// Length of the input unit at `pos` for stepping between candidate starts.
    fn step_len(&self, pos: usize) -> usize {
        if self.utf8 {
            decode_utf8(self.hay, pos).map_or(1, |(_, l)| l)
        } else {
            1
        }
    }

    fn may_start(&self, pos: usize) -> bool {
        match &self.prog.first_bytes {
            None => true,
            Some(set) => pos < self.hay.len() && set.contains(self.hay[pos]),
        }
    }

    /// Finds the leftmost-longest match starting at or after `start`.
    pub fn search(&self, start: usize) -> Option<(usize, Vec<Option<(usize, usize)>>)> {
        let len = self.hay.len();
        if start > len {
            return None;
        }
        let (s, e) = if self.prog.has_backrefs {
            let mut bt = Backtracker::new(self);
            let mut s = start;
            loop {
                if self.may_start(s) {
                    if let Some(e) = bt.run(s, None) {
                        break (s, e);
                    }
                }
                if s >= len {
                    return None;
                }
                s += self.step_len(s);
            }
        } else {
            self.longest(start)?
        };
        let mut groups = vec![None; self.prog.ngroups + 1];
        groups[0] = Some((s, e));
        if self.prog.ngroups > 0 {
            let mut bt = Backtracker::new(self);
            let found = bt.run(s, Some(e));
            debug_assert!(found.is_some(), "the capture pass must find the match");
            for g in 1..=self.prog.ngroups {
                let (a, b) = (bt.caps[2 * g], bt.caps[2 * g + 1]);
                if a != NONE && b != NONE {
                    groups[g] = Some((a, b));
                }
            }
        }
        Some((s, groups))
    }

    /// Breadth-first simulation for patterns without back-references.
    fn longest(&self, start: usize) -> Option<(usize, usize)> {
        let prog = self.prog;
        let m = prog.insts.len();
        let len = self.hay.len();
        let mut ring: Vec<Vec<(u32, usize)>> = (0..RING).map(|_| Vec::new()).collect();
        let mut slot = vec![u32::MAX; RING * m];
        let mut stamp = vec![0usize; 2 * m];
        let mut generation = 0usize;
        let mut pending = 0usize;
        let mut best: Option<(usize, usize)> = None;
        // Closure entries carry whether the path must consume a newline next.
        let mut stack: Vec<(u32, bool)> = Vec::new();
        let mut consuming: Vec<(u32, usize)> = Vec::new();
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
            generation += 1;
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
                        Inst::Backref(_) => {}
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
                for &(pc, st) in &consuming {
                    if let Some((bs, _)) = best {
                        if st > bs {
                            continue;
                        }
                    }
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
                            let e = &mut ring[q][slot[k] as usize];
                            if st < e.1 {
                                e.1 = st;
                            }
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
/// otherwise it records the empty match.
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
    base: usize,
    memo_bits: Vec<u64>,
    memo_set: HashSet<Vec<usize>>,
    keyed: Vec<usize>,
}

#[derive(Clone, Copy)]
enum Undo {
    Cap(u32, usize),
    Snap(u32, usize),
    Stamp(u32, u64),
}

#[derive(Clone, Copy)]
struct Choice {
    pc: u32,
    pos: usize,
    trail: usize,
    seg: u64,
    /// The path must consume the newline at `pos` next (see `assert_check`).
    nl: bool,
}

impl<'s, 'a> Backtracker<'s, 'a> {
    fn new(s: &'s Searcher<'a>) -> Self {
        let n = 2 * (s.prog.ngroups + 1);
        let keyed = (1..=s.prog.ngroups).filter(|&g| s.prog.referenced[g]).collect();
        Backtracker {
            s,
            caps: vec![NONE; n],
            snap: vec![NONE; n],
            trail: Vec::new(),
            stack: Vec::new(),
            stamp: vec![0; s.prog.insts.len()],
            seg: 1,
            next_seg: 2,
            viable: Vec::new(),
            base: 0,
            memo_bits: Vec::new(),
            memo_set: HashSet::new(),
            keyed,
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
    fn is_viable(&self, pc: u32, pos: usize) -> bool {
        if self.viable.is_empty() {
            return true;
        }
        let m = self.s.prog.insts.len();
        let k = (pos - self.base) * m + pc as usize;
        self.viable[k / 64] & (1 << (k % 64)) != 0
    }

    /// Backward reachability of `(Match, end)` for patterns without
    /// back-references.
    fn compute_viable(&mut self, start: usize, end: usize) {
        let prog = self.s.prog;
        let m = prog.insts.len();
        let rows = end - start + 1;
        self.viable.clear();
        self.viable.resize((rows * m).div_ceil(64), 0);
        self.base = start;
        let preds = &prog.preds;
        let mut work: Vec<u32> = Vec::new();
        for pos in (start..=end).rev() {
            let row = (pos - start) * m;
            work.clear();
            for pc in 0..m {
                let ok = match prog.insts[pc] {
                    Inst::Match => pos == end,
                    Inst::Unit(u) => match self.s.unit_len(u, pos) {
                        Some(l) if pos + l <= end => {
                            let k = (pos + l - start) * m + pc + 1;
                            self.viable[k / 64] & (1 << (k % 64)) != 0
                        }
                        _ => false,
                    },
                    _ => false,
                };
                if ok {
                    let k = row + pc;
                    self.viable[k / 64] |= 1 << (k % 64);
                    work.push(pc as u32);
                }
            }
            while let Some(q) = work.pop() {
                for &p in &preds[q as usize] {
                    let k = row + p as usize;
                    if self.viable[k / 64] & (1 << (k % 64)) != 0 {
                        continue;
                    }
                    if let Inst::Assert(a) = prog.insts[p as usize] {
                        match self.s.assert_check(a, pos, start) {
                            Check::Pass => {}
                            // The newline is consumed next unless the match ends here.
                            Check::NeedNewline if pos != end => {}
                            _ => continue,
                        }
                    }
                    self.viable[k / 64] |= 1 << (k % 64);
                    work.push(p);
                }
            }
        }
    }

    /// Marks a state reached right after consuming input; false if it was
    /// reached before.
    fn memo(&mut self, pc: u32, pos: usize) -> bool {
        if self.s.prog.has_backrefs {
            let mut key = Vec::with_capacity(3 + 4 * self.keyed.len());
            key.push(pc as usize);
            key.push(pos);
            for &g in &self.keyed {
                key.extend_from_slice(&self.caps[2 * g..2 * g + 2]);
                key.extend_from_slice(&self.snap[2 * g..2 * g + 2]);
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

    fn backref_len(&self, g: usize, pos: usize) -> Option<usize> {
        let (a, b) = (self.caps[2 * g], self.caps[2 * g + 1]);
        if a == NONE || b == NONE {
            return None;
        }
        let hay = self.s.hay;
        let sub = &hay[a..b];
        if !self.s.icase {
            return hay[pos..].starts_with(sub).then_some(sub.len());
        }
        if !self.s.utf8 {
            let end = pos + sub.len();
            return (end <= hay.len() && hay[pos..end].eq_ignore_ascii_case(sub)).then_some(sub.len());
        }
        let (mut i, mut j) = (a, pos);
        while i < b {
            match (decode_utf8(hay, i), decode_utf8(hay, j)) {
                (Some((c1, l1)), Some((c2, l2))) => {
                    if c1 != c2 && simple_lower(c1) != simple_lower(c2) {
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
        if !prog.has_backrefs {
            let e = target.expect("only searched with back-references");
            self.compute_viable(start, e);
            let nbits = (e - start + 1) * prog.insts.len();
            self.memo_bits.clear();
            self.memo_bits.resize(nbits.div_ceil(64), 0);
            self.base = start;
        }
        let limit = target.unwrap_or(self.s.hay.len());
        let mut best: Option<usize> = None;
        self.new_segment();
        self.memo(0, start);
        self.stack.push(Choice { pc: 0, pos: start, trail: 0, seg: self.seg, nl: false });
        while let Some(ch) = self.stack.pop() {
            self.undo(ch.trail);
            self.seg = ch.seg;
            let (mut pc, mut pos, mut nl) = (ch.pc, ch.pos, ch.nl);
            loop {
                if pos > limit || !self.is_viable(pc, pos) {
                    break;
                }
                self.mark(pc);
                match prog.insts[pc as usize] {
                    Inst::Unit(u) => match self.s.unit_len(u, pos) {
                        Some(l) => {
                            pos += l;
                            pc += 1;
                            nl = false;
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
                            self.stack.push(Choice { pc: b, pos, trail: self.trail.len(), seg: self.seg, nl });
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
                    Inst::Close(g, opt) => {
                        let g = g as usize;
                        if self.caps[2 * g] == pos {
                            if opt && self.snap[2 * g] != NONE {
                                for i in 0..self.caps.len() {
                                    let v = self.snap[i];
                                    self.set_cap(i, v);
                                }
                            } else {
                                self.set_cap(2 * g + 1, pos);
                            }
                        } else {
                            self.set_cap(2 * g + 1, pos);
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
                            Check::NeedNewline => nl = true,
                            Check::Fail => break,
                        }
                        pc += 1;
                    }
                    Inst::Backref(g) => match self.backref_len(g as usize, pos) {
                        Some(l) => {
                            pos += l;
                            pc += 1;
                            if l > 0 {
                                nl = false;
                                self.new_segment();
                                if !self.memo(pc, pos) {
                                    break;
                                }
                            }
                        }
                        None => break,
                    },
                    Inst::Match if nl => break,
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
