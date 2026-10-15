//! Compilation of the tree into a program for the matchers in `exec`.

use super::ast::{Assert, Node, Unit};
use super::charset::ByteSet;
use super::CompileError;

/// One instruction. Alternatives of `Split` are ordered: the first one has
/// the higher priority (greedy repetition, left alternative first).
#[derive(Clone, Copy, Debug)]
pub enum Inst {
    /// Consume one unit (index into `Program::units`).
    Unit(u32),
    Split(u32, u32),
    Jmp(u32),
    Open(u32),
    /// Close a group. The first flag marks a group that is the whole body of an
    /// optional repetition (see the capture rules in `exec`). The second one marks
    /// a group closed in a middle copy of a bounded repetition `{m,n}`
    /// (`m < copy < n`): a back-reference does not see such a group. The last one is 1 or 2
    /// for a group that is the whole body of the loop of `*` (1) or of `+`/`{m,}` (2) and
    /// whose body is not itself an unbounded repetition: an empty last iteration of such a
    /// group is seen by a back-reference as an empty group (see `exec`). It is 3 for a group
    /// whose body is an unbounded repetition (`(a*)*`), else 0.
    Close(u32, bool, bool, u8),
    Assert(Assert),
    Backref(u32),
    Match,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub insts: Vec<Inst>,
    pub units: Vec<Unit>,
    pub ngroups: usize,
    /// Groups whose contents can change the course of a match: those named by
    /// a back-reference.
    pub referenced: Vec<bool>,
    pub has_backrefs: bool,
    /// Bytes that can start a match, when no match can be empty.
    pub first_bytes: Option<ByteSet>,
    /// Epsilon predecessors of every instruction (for backward reachability).
    pub preds: Vec<Vec<u32>>,
    /// UTF-8: matches start only at character boundaries (GNU sed does so
    /// for patterns with multibyte-aware parts, and under `I`); otherwise
    /// every byte offset is tried.
    pub char_starts: bool,
    /// The instruction index of every unit (units and `Inst::Unit` correspond one to one).
    pub unit_pcs: Vec<u32>,
    /// For every group a back-reference names that has more than one copy in the program (it
    /// lies in a repetition that was expanded): its copies, in program order. Empty for every
    /// other group. See `Backtracker::copy_ok`.
    pub copies: Vec<Vec<GroupCopy>>,
    /// Some group a back-reference names can be empty and is repeated (its copies, or an empty
    /// iteration of its loop): the registers of the glibc quirks are needed (see `Backtracker`).
    pub quirks: bool,
    /// With the quirks: a group a back-reference names can be empty (a back-reference to it that
    /// finds it unset, because another alternative was taken, is glibc's to refuse at the start).
    pub nullable_refs: Vec<bool>,
}

/// One copy of a group in the program.
#[derive(Clone, Copy, Debug)]
pub struct GroupCopy {
    pub open: u32,
    pub close: u32,
    /// The copy is not inside an optional copy of a repetition.
    pub mandatory: bool,
    /// The body of the group can match the empty string.
    pub nullable: bool,
}

/// Programs larger than this are refused, as an exhausted regcomp would.
const MAX_INSTS: usize = 1 << 23;

pub fn compile(
    node: &Node,
    ngroups: usize,
    referenced: Vec<bool>,
    char_starts: bool,
    utf8: bool,
    glibc: bool,
) -> Result<Program, CompileError> {
    let has_backrefs = referenced.iter().any(|&r| r);
    let mut c = Compiler {
        insts: Vec::new(),
        units: Vec::new(),
        optional: 0,
        groups: Vec::new(),
        loopk: 0,
        backrefs: has_backrefs,
        glibc,
    };
    c.emit(node, false, false, false)?;
    c.insts.push(Inst::Match);
    let mut preds = vec![Vec::new(); c.insts.len()];
    for (pc, inst) in c.insts.iter().enumerate() {
        let pc32 = pc as u32;
        match *inst {
            Inst::Split(a, b) => {
                preds[a as usize].push(pc32);
                preds[b as usize].push(pc32);
            }
            Inst::Jmp(a) => preds[a as usize].push(pc32),
            Inst::Open(_) | Inst::Close(..) | Inst::Assert(_) => preds[pc + 1].push(pc32),
            Inst::Unit(_) | Inst::Backref(_) | Inst::Match => {}
        }
    }
    // In UTF-8, GNU sed starts matches only at character boundaries when
    // the pattern uses a class-like set or a word assertion.
    let char_starts = char_starts
        || c.units.iter().any(|u| matches!(u, Unit::Chars(cs) if cs.mb))
        || (utf8
            && c.insts.iter().any(|i| {
                matches!(
                    i,
                    Inst::Assert(
                        Assert::WordBoundary | Assert::NotWordBoundary | Assert::WordStart | Assert::WordEnd
                    )
                )
            }));
    let mut unit_pcs = vec![0u32; c.units.len()];
    for (pc, inst) in c.insts.iter().enumerate() {
        if let Inst::Unit(k) = *inst {
            unit_pcs[k as usize] = pc as u32;
        }
    }
    let mut copies = vec![Vec::new(); ngroups + 1];
    for (g, slot) in copies.iter_mut().enumerate().skip(1) {
        if !referenced.get(g).copied().unwrap_or(false) {
            continue;
        }
        let list: Vec<GroupCopy> = c.groups.iter().filter(|e| e.0 == g).map(|e| e.1).collect();
        if glibc && list.len() > 1 {
            *slot = list;
        }
    }
    let nullable_refs: Vec<bool> = (0..=ngroups).map(|g| glibc && referenced.get(g).copied().unwrap_or(false) && c.groups.iter().any(|e| e.0 == g && e.1.nullable)).collect();
    let quirks = glibc
        && (copies.iter().any(|l| l.iter().any(|c| c.nullable)) || nullable_refs.iter().any(|&b| b))
        || c.insts.iter().any(|i| matches!(*i, Inst::Close(g, _, _, lk) if lk != 0 && referenced.get(g as usize) == Some(&true)));
    let mut prog = Program {
        quirks,
        nullable_refs,
        copies,
        insts: c.insts,
        units: c.units,
        ngroups,
        referenced,
        has_backrefs,
        first_bytes: None,
        preds,
        char_starts,
        unit_pcs,
    };
    prog.first_bytes = first_bytes(&prog);
    Ok(prog)
}

struct Compiler {
    insts: Vec<Inst>,
    units: Vec<Unit>,
    /// How many optional copies of repetitions enclose the node being emitted.
    optional: u32,
    /// Every group emitted: its number and its copy.
    groups: Vec<(usize, GroupCopy)>,
    /// 1 while emitting the body of the loop of `*`, 2 of `+`/`{m,}`, 0 elsewhere.
    loopk: u8,
    /// The pattern has back-references.
    backrefs: bool,
    /// Reproduce the C library's quirks: mark groups for them (see `Inst::Close`).
    glibc: bool,
}

impl Compiler {
    fn pc(&self) -> u32 {
        self.insts.len() as u32
    }

    fn push(&mut self, i: Inst) -> Result<u32, CompileError> {
        if self.insts.len() >= MAX_INSTS {
            return Err(CompileError { message: super::parse::E_ESIZE.to_string() });
        }
        self.insts.push(i);
        Ok(self.pc() - 1)
    }

    /// Emits `node`. `opt` marks a group that is the whole body of an
    /// optional repetition; `dup` is true inside a copy made by expanding a
    /// repetition (every copy but the first), where the marks of inner
    /// repetitions are not kept (GNU sed behaves so: in `((a?)*b){2}` the
    /// second copy of group 2 does not restore an earlier iteration).
    fn emit(&mut self, node: &Node, opt: bool, dup: bool, mid: bool) -> Result<(), CompileError> {
        match node {
            Node::Empty => {}
            Node::Unit(u) => {
                self.units.push(u.clone());
                let k = self.units.len() as u32 - 1;
                self.push(Inst::Unit(k))?;
            }
            Node::Assert(a) => {
                self.push(Inst::Assert(*a))?;
            }
            Node::Backref(g) => {
                self.push(Inst::Backref(*g as u32))?;
            }
            Node::Group(g, inner) => {
                let open = self.push(Inst::Open(*g as u32))?;
                let lk = std::mem::take(&mut self.loopk);
                // Body that is a repetition itself (`(a*)*`): with back-references in the
                // pattern, glibc reports its empty last iteration as it is.
                let lk = if !self.glibc || lk == 0 || !can_be_empty(inner) { 0 } else if is_loop(inner) { 3 } else { lk };
                let opt = opt && !(self.backrefs && lk == 3);
                self.emit(inner, false, dup, mid)?;
                let close = self.push(Inst::Close(*g as u32, opt, mid, lk))?;
                let copy = GroupCopy { open, close, mandatory: self.optional == 0, nullable: can_be_empty(inner) };
                self.groups.push((*g, copy));
            }
            Node::Concat(items) => {
                for it in items {
                    self.emit(it, false, dup, mid)?;
                }
            }
            Node::Alt(branches) => {
                // GNU sed nests alternatives to the left, ((b1|b2)|b3)|...,
                // and prefers the left side of each, except that an empty
                // first branch yields to a non-empty second one:
                // `(|b|c)` tries b, empty, c; `(a||b)` tries a, empty, b.
                let mut order: Vec<&Node> = branches.iter().collect();
                if order.len() >= 2 && nodeless(order[0]) && !nodeless(order[1]) {
                    order.swap(0, 1);
                }
                let mut jumps = Vec::new();
                for (k, b) in order.iter().enumerate() {
                    if k + 1 < order.len() {
                        let split = self.push(Inst::Split(0, 0))?;
                        self.emit(b, false, dup, mid)?;
                        jumps.push(self.push(Inst::Jmp(0))?);
                        let next = self.pc();
                        self.insts[split as usize] = Inst::Split(split + 1, next);
                    } else {
                        self.emit(b, false, dup, mid)?;
                    }
                }
                let end = self.pc();
                for j in jumps {
                    self.insts[j as usize] = Inst::Jmp(end);
                }
            }
            Node::Repeat(body, min, max) => {
                let mark = matches!(**body, Node::Group(..)) && !dup;
                let mut copy = 0u32;
                for _ in 0..*min {
                    self.emit(body, false, dup || copy > 0, mid)?;
                    copy += 1;
                }
                match max {
                    None => {
                        let l = self.push(Inst::Split(0, 0))?;
                        self.optional += 1;
                        self.loopk = if !matches!(**body, Node::Group(..)) { 0 } else if *min == 0 { 1 } else { 2 };
                        self.emit(body, mark, dup || copy > 0, mid)?;
                        self.loopk = 0;
                        self.optional -= 1;
                        self.push(Inst::Jmp(l))?;
                        let end = self.pc();
                        self.insts[l as usize] = Inst::Split(l + 1, end);
                    }
                    Some(max) => {
                        // With a minimum of 0 only the original copy is
                        // marked; copies made from it are not.
                        let mut holes = Vec::new();
                        for _ in *min..*max {
                            holes.push(self.push(Inst::Split(0, 0))?);
                            self.optional += 1;
                            self.emit(body, mark && (*min > 0 || copy == 0), dup || copy > 0, self.glibc && (mid || copy + 1 < *max))?;
                            self.optional -= 1;
                            copy += 1;
                        }
                        let end = self.pc();
                        for h in holes {
                            self.insts[h as usize] = Inst::Split(h + 1, end);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Whether a node can match the empty string (a back-reference is taken to).
fn can_be_empty(node: &Node) -> bool {
    match node {
        Node::Empty | Node::Assert(_) | Node::Backref(_) => true,
        Node::Unit(_) => false,
        Node::Group(_, inner) => can_be_empty(inner),
        Node::Concat(items) => items.iter().all(can_be_empty),
        Node::Alt(branches) => branches.iter().any(can_be_empty),
        Node::Repeat(body, min, _) => *min == 0 || can_be_empty(body),
    }
}

/// Whether a node is an unbounded repetition (possibly in groups).
fn is_loop(node: &Node) -> bool {
    match node {
        Node::Group(_, inner) => is_loop(inner),
        Node::Repeat(_, _, max) => max.is_none(),
        _ => false,
    }
}

/// Whether a node compiles to no instruction at all.
fn nodeless(node: &Node) -> bool {
    match node {
        Node::Empty => true,
        Node::Concat(items) => items.iter().all(nodeless),
        Node::Alt(branches) => branches.iter().all(nodeless),
        Node::Repeat(body, _, max) => *max == Some(0) || nodeless(body),
        Node::Unit(_) | Node::Assert(_) | Node::Group(..) | Node::Backref(_) => false,
    }
}

/// The set of bytes a match can start with, or `None` when a match may be
/// empty (or start with something we do not analyze).
fn first_bytes(prog: &Program) -> Option<ByteSet> {
    let mut set = ByteSet::empty();
    let mut seen = vec![false; prog.insts.len()];
    let mut stack = vec![0u32];
    while let Some(pc) = stack.pop() {
        let pc = pc as usize;
        if seen[pc] {
            continue;
        }
        seen[pc] = true;
        match prog.insts[pc] {
            Inst::Unit(u) => match &prog.units[u as usize] {
                Unit::Byte(b) => set.union_with(b),
                Unit::Any { .. } | Unit::Chars(_) => {
                    // Any byte that can begin a character (or a raw byte).
                    return None;
                }
            },
            Inst::Split(a, b) => {
                stack.push(a);
                stack.push(b);
            }
            Inst::Jmp(a) => stack.push(a),
            Inst::Open(_) | Inst::Close(..) | Inst::Assert(_) => stack.push(pc as u32 + 1),
            Inst::Backref(_) | Inst::Match => return None,
        }
    }
    if set.is_full() { None } else { Some(set) }
}
