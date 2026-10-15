//! A lazily built DFA that finds the span of a match (not its groups).
//!
//! The breadth-first simulation in `exec` follows every thread of the program at every byte.
//! A DFA follows one state per byte, and builds a state the first time it is reached, so most
//! of a search is a table lookup. It only answers "where": the capture pass in `exec` still
//! assigns the groups, on the span found here.
//!
//! The leftmost-longest match is found in two scans, as in RE#: a backward scan with the
//! reversed pattern, trying every end, finds the leftmost start (the last position at which the
//! reversed program accepts); a forward scan from that start finds the longest end. A haystack
//! with no match costs one backward scan.
//!
//! To stay exactly equal to the simulation, the DFA is used only where the assertions reduce to
//! simple facts about the neighbouring bytes: the haystack must hold no line delimiter (so `^`,
//! `$`, and the `M` flag mean the buffer edges, and the newline rules of `assert_check` never
//! apply). In UTF-8 the characters of the pattern are spelled out as byte sequences, so the DFAs
//! run on bytes; patterns that cannot be (case folding, word assertions) stop at the first
//! non-ASCII byte instead. Programs with back-references never come here. See `Searcher` for the check and the fallback.

use std::collections::HashMap;

use super::ast::Assert;
use super::ast::Unit;
use super::charset::is_word_byte;
use super::compile::{Inst, Program};

/// What the byte next to a position is, for assertions.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Ctx {
    /// The edge of the haystack.
    Edge = 0,
    Word = 1,
    Other = 2,
}

impl Ctx {
    fn of(b: u8) -> Ctx {
        if is_word_byte(b) { Ctx::Word } else { Ctx::Other }
    }

    fn is_word(self) -> bool {
        self == Ctx::Word
    }
}

const UNKNOWN: u32 = u32::MAX;
const ACCEPT: u32 = 1 << 31;
/// More states than this and the search gives up (the caller uses the simulation).
const MAX_STATES: usize = 2000;

/// The DFA gave up (too many states): the caller falls back to the simulation.
#[derive(Debug)]
pub struct GiveUp;

struct State {
    kernel: Vec<u32>,
    prev: Ctx,
}

pub struct Dfa {
    /// The line delimiter, and whether input is UTF-8: the scans give up on such bytes (see the
    /// module documentation).
    delim: u8,
    utf8: bool,
    prog: Program,
    /// For every unit, which bytes it accepts.
    tables: Vec<[bool; 256]>,
    /// Add the start of the program at every position (a search that may begin anywhere).
    unanchored: bool,
    states: Vec<State>,
    index: HashMap<(Vec<u32>, Ctx), u32>,
    /// `states.len() * 256` entries: next state, with `ACCEPT` set if the program matches
    /// before consuming the byte.
    trans: Vec<u32>,
    /// Per state and next context: 0 unknown, 1 no match here, 2 match here.
    accept_at: Vec<[u8; 3]>,
    stamp: Vec<u32>,
    generation: u32,
    stack: Vec<u32>,
    starts: [u32; 3],
}

impl Dfa {
    /// `utf8`: units that match characters are tabulated on ASCII bytes only.
    pub fn new(prog: Program, utf8: bool, delim: u8, unanchored: bool) -> Dfa {
        let tables = prog.units.iter().map(|u| table(u, utf8)).collect();
        let n = prog.insts.len();
        Dfa {
            delim,
            utf8,
            prog,
            tables,
            unanchored,
            states: Vec::new(),
            index: HashMap::new(),
            trans: Vec::new(),
            accept_at: Vec::new(),
            stamp: vec![0; n],
            generation: 0,
            stack: Vec::new(),
            starts: [UNKNOWN; 3],
        }
    }

    /// Whether the program has anything the DFA cannot follow.
    pub fn supports(prog: &Program) -> bool {
        !prog.has_backrefs && !prog.insts.iter().any(|i| matches!(i, Inst::Backref(_)))
    }

    fn state_for(&mut self, kernel: Vec<u32>, prev: Ctx) -> Result<u32, GiveUp> {
        if let Some(&id) = self.index.get(&(kernel.clone(), prev)) {
            return Ok(id);
        }
        if self.states.len() >= MAX_STATES {
            return Err(GiveUp);
        }
        let id = self.states.len() as u32;
        self.index.insert((kernel.clone(), prev), id);
        self.states.push(State { kernel, prev });
        self.trans.extend(std::iter::repeat_n(UNKNOWN, 256));
        self.accept_at.push([0; 3]);
        Ok(id)
    }

    fn start(&mut self, prev: Ctx) -> Result<u32, GiveUp> {
        let slot = prev as usize;
        if self.starts[slot] == UNKNOWN {
            let kernel = if self.unanchored { Vec::new() } else { vec![0] };
            self.starts[slot] = self.state_for(kernel, prev)?;
        }
        Ok(self.starts[slot])
    }

    fn holds(a: Assert, prev: Ctx, next: Ctx) -> bool {
        match a {
            Assert::LineStart | Assert::BufStart => prev == Ctx::Edge,
            Assert::LineEnd | Assert::BufEnd => next == Ctx::Edge,
            Assert::WordBoundary => prev.is_word() != next.is_word(),
            Assert::NotWordBoundary => prev.is_word() == next.is_word(),
            Assert::WordStart => !prev.is_word() && next.is_word(),
            Assert::WordEnd => prev.is_word() && !next.is_word(),
        }
    }

    /// The closure of the state's threads at a position whose neighbours are `prev` and
    /// `next`: whether the program can match here, and the units that consume next.
    fn closure(&mut self, state: u32, next: Ctx, units: &mut Vec<u32>) -> bool {
        self.generation += 1;
        let generation = self.generation;
        units.clear();
        let prev = self.states[state as usize].prev;
        let mut matched = false;
        self.stack.clear();
        if self.unanchored {
            self.stack.push(0);
        }
        for &pc in &self.states[state as usize].kernel {
            self.stack.push(pc);
        }
        while let Some(pc) = self.stack.pop() {
            let i = pc as usize;
            if self.stamp[i] == generation {
                continue;
            }
            self.stamp[i] = generation;
            match self.prog.insts[i] {
                Inst::Unit(u) => units.push(u),
                Inst::Split(a, b) => {
                    self.stack.push(b);
                    self.stack.push(a);
                }
                Inst::Jmp(a) => self.stack.push(a),
                Inst::Open(_) | Inst::Close(..) => self.stack.push(pc + 1),
                Inst::Assert(a) => {
                    if Self::holds(a, prev, next) {
                        self.stack.push(pc + 1);
                    }
                }
                Inst::Backref(_) => unreachable!("`supports` rejects programs with back-references"),
                Inst::Match => matched = true,
            }
        }
        matched
    }

    /// The transition on byte `b`: the next state, and whether the program matched right
    /// before `b` (that is, at the position the scan was at).
    #[inline(always)]
    fn step(&mut self, state: u32, b: u8) -> Result<u32, GiveUp> {
        // Transitions on bytes the scans give up on are never filled in.
        let t = self.trans[state as usize * 256 + b as usize];
        if t != UNKNOWN {
            return Ok(t);
        }
        self.step_slow(state, b)
    }

    #[cold]
    #[inline(never)]
    fn step_slow(&mut self, state: u32, b: u8) -> Result<u32, GiveUp> {
        if self.unhandled(b) {
            return Err(GiveUp);
        }
        let ctx = Ctx::of(b);
        let mut units = Vec::new();
        let matched = self.closure(state, ctx, &mut units);
        let mut kernel: Vec<u32> = units
            .iter()
            .filter(|&&u| self.tables[u as usize][b as usize])
            .map(|&u| self.unit_pc(u) + 1)
            .collect();
        kernel.sort_unstable();
        kernel.dedup();
        let next = self.state_for(kernel, ctx)?;
        let t = next | if matched { ACCEPT } else { 0 };
        self.trans[state as usize * 256 + b as usize] = t;
        Ok(t)
    }

    /// The instruction index of a unit (units are numbered in instruction order).
    fn unit_pc(&self, u: u32) -> u32 {
        self.prog.unit_pcs[u as usize]
    }

    /// Whether the program matches at the end of the scan, given the context beyond it.
    fn accepts_at(&mut self, state: u32, next: Ctx) -> bool {
        let cached = self.accept_at[state as usize][next as usize];
        if cached != 0 {
            return cached == 2;
        }
        let mut units = Vec::new();
        let matched = self.closure(state, next, &mut units);
        self.accept_at[state as usize][next as usize] = if matched { 2 } else { 1 };
        matched
    }

    /// Anchored forward scan: the longest match that starts at `start` (the end offset), if any.
    /// Gives up when the scan would read more than `budget` bytes (which it uses up).
    pub fn longest_from(&mut self, hay: &[u8], start: usize, budget: &mut usize) -> Result<Option<usize>, GiveUp> {
        let prev = self.neighbour(hay, start)?;
        let mut state = self.start(prev)?;
        let mut last = None;
        let mut p = start;
        while p < hay.len() {
            if *budget == 0 {
                return Err(GiveUp);
            }
            *budget -= 1;
            let t = self.step(state, hay[p])?;
            if t & ACCEPT != 0 {
                last = Some(p);
            }
            state = t & !ACCEPT;
            if self.is_dead(state) {
                return Ok(last);
            }
            p += 1;
        }
        if self.accepts_at(state, Ctx::Edge) {
            last = Some(hay.len());
        }
        Ok(last)
    }

    /// Backward scan from `to` with the reversed program: the leftmost start `>= from` of a
    /// match that ends at or before `to`. `self` must be built from the reversed pattern,
    /// unanchored.
    pub fn leftmost_start(&mut self, hay: &[u8], from: usize, to: usize) -> Result<Option<usize>, GiveUp> {
        let right = if to == hay.len() {
            Ctx::Edge
        } else {
            // `first_end` stepped over this byte to find the match that ends before it.
            debug_assert!(!self.unhandled(hay[to]));
            Ctx::of(hay[to])
        };
        let mut state = self.start(right)?;
        let mut best = None;
        let mut p = to;
        while p > from {
            let t = self.step(state, hay[p - 1])?;
            if t & ACCEPT != 0 {
                best = Some(p);
            }
            state = t & !ACCEPT;
            p -= 1;
        }
        let beyond = self.neighbour(hay, from)?;
        if self.accepts_at(state, beyond) {
            best = Some(from);
        }
        Ok(best)
    }

    /// Forward unanchored scan from `from`: the smallest end offset of any match, if there is
    /// one. `self` must be unanchored.
    pub fn first_end(&mut self, hay: &[u8], from: usize) -> Result<Option<usize>, GiveUp> {
        let prev = self.neighbour(hay, from)?;
        let mut state = self.start(prev)?;
        for p in from..hay.len() {
            let t = self.step(state, hay[p])?;
            if t & ACCEPT != 0 {
                return Ok(Some(p));
            }
            state = t & !ACCEPT;
        }
        Ok(self.accepts_at(state, Ctx::Edge).then_some(hay.len()))
    }

    /// Forward unanchored scan: is there any match? `self` must be unanchored.
    pub fn is_match(&mut self, hay: &[u8]) -> Result<bool, GiveUp> {
        Ok(self.first_end(hay, 0)?.is_some())
    }

    /// The context left of `pos`: the edge, or the byte before it (which must be one the DFA
    /// handles).
    fn neighbour(&self, hay: &[u8], pos: usize) -> Result<Ctx, GiveUp> {
        if pos == 0 {
            return Ok(Ctx::Edge);
        }
        let b = hay[pos - 1];
        if self.unhandled(b) {
            return Err(GiveUp);
        }
        Ok(Ctx::of(b))
    }

    /// Bytes the scans give up on: the delimiter, newlines (`M` mode and `-z`), non-ASCII in UTF-8.
    #[inline]
    fn unhandled(&self, b: u8) -> bool {
        b == self.delim || b == b'\n' || (self.utf8 && b >= 0x80)
    }

    fn is_dead(&self, state: u32) -> bool {
        !self.unanchored && self.states[state as usize].kernel.is_empty()
    }
}

/// The bytes a unit accepts. Characters are only tabulated for ASCII: in UTF-8 the DFA runs on
/// ASCII haystacks only.
fn table(unit: &Unit, utf8: bool) -> [bool; 256] {
    let mut t = [false; 256];
    for b in 0..=255u8 {
        t[b as usize] = match unit {
            Unit::Byte(set) => set.contains(b),
            Unit::Any { exclude } => b < 0x80 && exclude.is_none_or(|e| b != e && b != b'\n'),
            Unit::Chars(cs) => b < 0x80 && cs.matches_char(b as char),
        };
    }
    let _ = utf8;
    t
}

/// The pattern read backwards: concatenations reversed, assertions mirrored (what is before
/// becomes what is after).
pub fn reverse(node: &super::ast::Node) -> super::ast::Node {
    use super::ast::Node;
    match node {
        Node::Concat(children) => Node::Concat(children.iter().rev().map(reverse).collect()),
        Node::Alt(children) => Node::Alt(children.iter().map(reverse).collect()),
        Node::Group(n, inner) => Node::Group(*n, Box::new(reverse(inner))),
        Node::Repeat(inner, min, max) => Node::Repeat(Box::new(reverse(inner)), *min, *max),
        Node::Assert(a) => Node::Assert(match a {
            Assert::LineStart => Assert::LineEnd,
            Assert::LineEnd => Assert::LineStart,
            Assert::BufStart => Assert::BufEnd,
            Assert::BufEnd => Assert::BufStart,
            Assert::WordStart => Assert::WordEnd,
            Assert::WordEnd => Assert::WordStart,
            other => *other,
        }),
        other => other.clone(),
    }
}

/// The three DFAs of a pattern: forward anchored (longest end from a start), reverse
/// unanchored (leftmost start), and forward unanchored (is there a match).
pub struct DfaSet {
    pub forward: Dfa,
    pub reverse: Dfa,
    pub unanchored: Dfa,
}

/// Built on first use; `Disabled` if the pattern is out of reach of the DFA.
#[derive(Default)]
pub enum DfaSlot {
    #[default]
    Unbuilt,
    Disabled,
    Ready(Box<DfaSet>),
}

/// A clone starts with no automata: they are caches, rebuilt on first use.
impl Clone for DfaSlot {
    fn clone(&self) -> DfaSlot {
        DfaSlot::Unbuilt
    }
}

impl std::fmt::Debug for DfaSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DfaSlot::Unbuilt => "DfaSlot::Unbuilt",
            DfaSlot::Disabled => "DfaSlot::Disabled",
            DfaSlot::Ready(_) => "DfaSlot::Ready",
        })
    }
}

impl DfaSlot {
    pub fn get(&mut self, prog: &Program, node: &super::ast::Node, utf8: bool, delim: u8) -> Option<&mut DfaSet> {
        if matches!(self, DfaSlot::Unbuilt) {
            *self = Self::build(prog, node, utf8, delim).map_or(DfaSlot::Disabled, |set| DfaSlot::Ready(Box::new(set)));
        }
        match self {
            DfaSlot::Ready(set) => Some(set),
            _ => None,
        }
    }

    fn build(prog: &Program, node: &super::ast::Node, utf8: bool, delim: u8) -> Option<DfaSet> {
        if !Dfa::supports(prog) {
            return None;
        }
        let compile = |n: &super::ast::Node, utf8: bool| {
            super::compile::compile(n, prog.ngroups, vec![false; prog.ngroups + 1], prog.char_starts && utf8, utf8, false).ok()
        };
        // In UTF-8, characters are spelled out as byte sequences when the pattern allows it; the
        // DFAs then run on bytes and can pass non-ASCII text. Otherwise they stop at the first
        // non-ASCII byte and the caller falls back.
        if utf8 {
            // A pattern that starts only at character boundaries could match empty in the middle
            // of a character here, so those are expanded only if no match can be empty.
            if let Some(bytes) = expand_utf8(node).filter(|b| !prog.char_starts || !nullable(b)) {
                if let (Some(fwd), Some(rev)) = (compile(&bytes, false), compile(&reverse(&bytes), false)) {
                    return Some(DfaSet {
                        unanchored: Dfa::new(fwd.clone(), false, delim, true),
                        forward: Dfa::new(fwd, false, delim, false),
                        reverse: Dfa::new(rev, false, delim, true),
                    });
                }
            }
        }
        let rev = compile(&reverse(node), utf8)?;
        Some(DfaSet {
            forward: Dfa::new(prog.clone(), utf8, delim, false),
            reverse: Dfa::new(rev, utf8, delim, true),
            unanchored: Dfa::new(prog.clone(), utf8, delim, true),
        })
    }
}

/// The pattern with every character unit written as the UTF-8 byte sequences it matches, or
/// `None` when it has something that cannot be (case folding, word assertions, which
/// depend on whole characters, or a set too large).
fn expand_utf8(node: &super::ast::Node) -> Option<super::ast::Node> {
    use super::ast::Node;
    let mut budget = 4000usize;
    fn walk(node: &Node, budget: &mut usize) -> Option<Node> {
        Some(match node {
            Node::Empty => Node::Empty,
            Node::Backref(_) => unreachable!("`supports` rejects programs with back-references"),
            Node::Assert(a) => match a {
                Assert::WordBoundary | Assert::NotWordBoundary | Assert::WordStart | Assert::WordEnd => return None,
                _ => Node::Assert(*a),
            },
            Node::Group(n, inner) => Node::Group(*n, Box::new(walk(inner, budget)?)),
            Node::Concat(items) => Node::Concat(items.iter().map(|i| walk(i, budget)).collect::<Option<_>>()?),
            Node::Alt(items) => Node::Alt(items.iter().map(|i| walk(i, budget)).collect::<Option<_>>()?),
            Node::Repeat(inner, min, max) => Node::Repeat(Box::new(walk(inner, budget)?), *min, *max),
            Node::Unit(Unit::Byte(set)) => {
                if (0x80..=0xffu8).any(|b| set.contains(b)) {
                    return None;
                }
                Node::Unit(Unit::Byte(*set))
            }
            Node::Unit(Unit::Any { exclude }) => {
                let mut ranges = normalize(vec![(0, 0x10ffff)]);
                if let Some(e) = exclude {
                    ranges = subtract(&ranges, &[(0xa, 0xa), (*e as u32, *e as u32)]);
                }
                sequences(&ranges, budget)?
            }
            Node::Unit(Unit::Chars(cs)) => {
                if cs.icase || cs.by_upper || cs.lib.is_some() {
                    return None;
                }
                let mut ranges = cs.ranges.clone();
                for class in &cs.classes {
                    ranges.extend_from_slice(class.ranges());
                }
                let mut ranges = normalize(ranges);
                if cs.negated {
                    ranges = normalize(subtract(&[(0, 0x10ffff)], &ranges));
                }
                if let Some(e) = cs.exclude {
                    ranges = subtract(&ranges, &[(0xa, 0xa), (e as u32, e as u32)]);
                }
                sequences(&ranges, budget)?
            }
        })
    }
    walk(node, &mut budget)
}

/// Sorted, merged, without surrogates.
fn normalize(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    ranges.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (lo, hi) in ranges {
        match out.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    subtract(&out, &[(0xd800, 0xdfff)])
}

/// `a` without `b` (both sorted and merged, or `b` small).
fn subtract(a: &[(u32, u32)], b: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut b = b.to_vec();
    b.sort_unstable();
    let mut out = Vec::new();
    for &(mut lo, hi) in a {
        let mut dead = false;
        for &(blo, bhi) in &b {
            if bhi < lo || blo > hi {
                continue;
            }
            if blo > lo {
                out.push((lo, blo - 1));
            }
            if bhi >= hi {
                dead = true;
                break;
            }
            lo = bhi + 1;
        }
        if !dead {
            out.push((lo, hi));
        }
    }
    out
}

/// The code point ranges as an alternation of UTF-8 byte sequences.
fn sequences(ranges: &[(u32, u32)], budget: &mut usize) -> Option<super::ast::Node> {
    use super::ast::Node;
    use super::charset::ByteSet;
    let mut seqs: Vec<Vec<(u8, u8)>> = Vec::new();
    for &(lo, hi) in ranges {
        split_utf8(lo, hi, &mut seqs);
    }
    if seqs.len() > *budget {
        return None;
    }
    *budget -= seqs.len();
    let branches: Vec<Node> = seqs
        .into_iter()
        .map(|seq| {
            let units: Vec<Node> = seq
                .into_iter()
                .map(|(lo, hi)| {
                    let mut set = ByteSet::empty();
                    set.insert_range(lo, hi);
                    Node::Unit(Unit::Byte(set))
                })
                .collect();
            if units.len() == 1 { units.into_iter().next().unwrap() } else { Node::Concat(units) }
        })
        .collect();
    Some(match branches.len() {
        0 => Node::Unit(Unit::Byte(super::charset::ByteSet::empty())),
        1 => branches.into_iter().next().unwrap(),
        _ => Node::Alt(branches),
    })
}

/// Splits a range of scalar values into runs whose encodings are products of byte ranges.
fn split_utf8(lo: u32, hi: u32, out: &mut Vec<Vec<(u8, u8)>>) {
    debug_assert!(lo <= hi);
    // One encoded length at a time.
    for &(top, _) in &[(0x7fu32, 1), (0x7ff, 2), (0xffff, 3)] {
        if lo <= top && hi > top {
            split_utf8(lo, top, out);
            split_utf8(top + 1, hi, out);
            return;
        }
    }
    if hi < 0x80 {
        out.push(vec![(lo as u8, hi as u8)]);
        return;
    }
    let n = if hi < 0x800 { 2 } else if hi < 0x10000 { 3 } else { 4 };
    for i in 1..n {
        let m = (1u32 << (6 * i)) - 1;
        if lo & !m != hi & !m {
            if lo & m != 0 {
                split_utf8(lo, lo | m, out);
                split_utf8((lo | m) + 1, hi, out);
                return;
            }
            if hi & m != m {
                split_utf8(lo, (hi & !m) - 1, out);
                split_utf8(hi & !m, hi, out);
                return;
            }
        }
    }
    let encode = |c: u32| {
        let mut buf = [0u8; 4];
        char::from_u32(c).expect("scalar value").encode_utf8(&mut buf).as_bytes().to_vec()
    };
    let (a, b) = (encode(lo), encode(hi));
    out.push(a.iter().zip(&b).map(|(&x, &y)| (x, y)).collect());
}

/// For a pattern that is a fixed sequence of single bytes, with groups around parts of it, the
/// offsets (from the match start) of every group: the match length and the sub-matches then need
/// no search. `None` for anything else.
pub fn fixed_groups(node: &super::ast::Node, ngroups: usize) -> Option<(usize, Vec<(usize, usize)>)> {
    use super::ast::{Node, Unit};
    fn walk(node: &Node, at: usize, groups: &mut [(usize, usize)]) -> Option<usize> {
        match node {
            Node::Empty => Some(at),
            Node::Unit(Unit::Byte(_)) => Some(at + 1),
            Node::Group(n, inner) => {
                let end = walk(inner, at, groups)?;
                *groups.get_mut(*n)? = (at, end);
                Some(end)
            }
            Node::Concat(children) => {
                let mut pos = at;
                for child in children {
                    pos = walk(child, pos, groups)?;
                }
                Some(pos)
            }
            _ => None,
        }
    }
    let mut groups = vec![(usize::MAX, usize::MAX); ngroups + 1];
    let width = walk(node, 0, &mut groups)?;
    // Every group must be placed (a group number the walk never met would be unset).
    groups.iter().skip(1).all(|g| g.0 != usize::MAX).then_some((width, groups))
}

/// Whether the pattern can match the empty string (assertions count as empty).
pub(super) fn nullable(node: &super::ast::Node) -> bool {
    use super::ast::Node;
    match node {
        Node::Empty | Node::Assert(_) | Node::Backref(_) => true,
        Node::Unit(_) => false,
        Node::Group(_, inner) => nullable(inner),
        Node::Concat(items) => items.iter().all(nullable),
        Node::Alt(items) => items.iter().any(nullable),
        Node::Repeat(inner, min, _) => *min == 0 || nullable(inner),
    }
}
