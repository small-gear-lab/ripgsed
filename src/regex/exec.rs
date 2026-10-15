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
        let mut stamp = vec![0usize; m];
        let mut generation = 0usize;
        let mut pending = 0usize;
        let mut best: Option<(usize, usize)> = None;
        let mut stack: Vec<u32> = Vec::new();
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
                stack.push(pc0);
                while let Some(pc) = stack.pop() {
                    let pcu = pc as usize;
                    if stamp[pcu] == generation {
                        continue;
                    }
                    stamp[pcu] = generation;
                    match prog.insts[pcu] {
                        Inst::Unit(_) => consuming.push((pc, st)),
                        Inst::Split(a, b) => {
                            stack.push(b);
                            stack.push(a);
                        }
                        Inst::Jmp(a) => stack.push(a),
                        Inst::Open(_) | Inst::Close(..) => stack.push(pc + 1),
                        Inst::Assert(a) => {
                            if self.assert_ok(a, p) {
                                stack.push(pc + 1);
                            }
                        }
                        Inst::Backref(_) => {}
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
struct Backtracker<'s, 'a> {
    s: &'s Searcher<'a>,
    caps: Vec<usize>,
    snap: Vec<usize>,
    trail: Vec<(u32, usize)>,
    stack: Vec<(u32, usize, usize)>,
    bits: Vec<u64>,
    bits_base: usize,
    set: HashSet<Vec<usize>>,
    keyed: Vec<usize>,
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
            bits: Vec::new(),
            bits_base: 0,
            set: HashSet::new(),
            keyed,
        }
    }

    #[inline]
    fn set_cap(&mut self, i: usize, v: usize) {
        if self.caps[i] != v {
            self.trail.push((i as u32, self.caps[i]));
            self.caps[i] = v;
        }
    }

    #[inline]
    fn set_snap(&mut self, i: usize, v: usize) {
        if self.snap[i] != v {
            self.trail.push(((i + self.caps.len()) as u32, self.snap[i]));
            self.snap[i] = v;
        }
    }

    fn undo(&mut self, to: usize) {
        let n = self.caps.len();
        while self.trail.len() > to {
            let (i, v) = self.trail.pop().unwrap();
            let i = i as usize;
            if i < n {
                self.caps[i] = v;
            } else {
                self.snap[i - n] = v;
            }
        }
    }

    /// Marks the state visited; false if it was already.
    fn visit(&mut self, pc: u32, pos: usize) -> bool {
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
            return self.set.insert(key);
        }
        let m = self.s.prog.insts.len();
        let k = (pos - self.bits_base) * m + pc as usize;
        let (w, b) = (k / 64, k % 64);
        if self.bits[w] & (1 << b) != 0 {
            return false;
        }
        self.bits[w] |= 1 << b;
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
    /// registers in `caps`. With `None`, explores everything and returns the
    /// longest end.
    fn run(&mut self, start: usize, target: Option<usize>) -> Option<usize> {
        let prog = self.s.prog;
        self.caps.fill(NONE);
        self.snap.fill(NONE);
        self.trail.clear();
        self.stack.clear();
        self.set.clear();
        if !prog.has_backrefs {
            let e = target.expect("only searched with back-references");
            let nbits = (e - start + 1) * prog.insts.len();
            self.bits.clear();
            self.bits.resize(nbits.div_ceil(64), 0);
            self.bits_base = start;
        }
        let limit = target.unwrap_or(self.s.hay.len());
        let mut best: Option<usize> = None;
        self.stack.push((0, start, 0));
        while let Some((pc0, pos0, tl)) = self.stack.pop() {
            self.undo(tl);
            let (mut pc, mut pos) = (pc0, pos0);
            loop {
                if pos > limit || !self.visit(pc, pos) {
                    break;
                }
                match prog.insts[pc as usize] {
                    Inst::Unit(u) => match self.s.unit_len(u, pos) {
                        Some(l) => {
                            pos += l;
                            pc += 1;
                        }
                        None => break,
                    },
                    Inst::Split(a, b) => {
                        self.stack.push((b, pos, self.trail.len()));
                        pc = a;
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
                        if !self.s.assert_ok(a, pos) {
                            break;
                        }
                        pc += 1;
                    }
                    Inst::Backref(g) => match self.backref_len(g as usize, pos) {
                        Some(l) => {
                            pos += l;
                            pc += 1;
                        }
                        None => break,
                    },
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
