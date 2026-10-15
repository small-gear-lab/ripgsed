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
    /// Close a group. The flag marks a group that is the whole body of an
    /// optional repetition (see the capture rules in `exec`).
    Close(u32, bool),
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
}

/// Programs larger than this are refused, as an exhausted regcomp would.
const MAX_INSTS: usize = 1 << 23;

pub fn compile(node: &Node, ngroups: usize, referenced: Vec<bool>) -> Result<Program, CompileError> {
    let mut c = Compiler { insts: Vec::new(), units: Vec::new() };
    c.emit(node, false, false)?;
    c.insts.push(Inst::Match);
    let has_backrefs = referenced.iter().any(|&r| r);
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
    let mut prog =
        Program { insts: c.insts, units: c.units, ngroups, referenced, has_backrefs, first_bytes: None, preds };
    prog.first_bytes = first_bytes(&prog);
    Ok(prog)
}

struct Compiler {
    insts: Vec<Inst>,
    units: Vec<Unit>,
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
    fn emit(&mut self, node: &Node, opt: bool, dup: bool) -> Result<(), CompileError> {
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
                self.push(Inst::Open(*g as u32))?;
                self.emit(inner, false, dup)?;
                self.push(Inst::Close(*g as u32, opt))?;
            }
            Node::Concat(items) => {
                for it in items {
                    self.emit(it, false, dup)?;
                }
            }
            Node::Alt(branches) => {
                // A branch without any instruction (an empty alternative) is
                // the least preferred one, wherever it is written.
                let mut order: Vec<&Node> = branches.iter().filter(|b| !nodeless(b)).collect();
                if order.len() < branches.len() {
                    order.push(&Node::Empty);
                }
                let mut jumps = Vec::new();
                for (k, b) in order.iter().enumerate() {
                    if k + 1 < order.len() {
                        let split = self.push(Inst::Split(0, 0))?;
                        self.emit(b, false, dup)?;
                        jumps.push(self.push(Inst::Jmp(0))?);
                        let next = self.pc();
                        self.insts[split as usize] = Inst::Split(split + 1, next);
                    } else {
                        self.emit(b, false, dup)?;
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
                    self.emit(body, false, dup || copy > 0)?;
                    copy += 1;
                }
                match max {
                    None => {
                        let l = self.push(Inst::Split(0, 0))?;
                        self.emit(body, mark, dup || copy > 0)?;
                        self.push(Inst::Jmp(l))?;
                        let end = self.pc();
                        self.insts[l as usize] = Inst::Split(l + 1, end);
                    }
                    Some(max) => {
                        let mut holes = Vec::new();
                        for _ in *min..*max {
                            holes.push(self.push(Inst::Split(0, 0))?);
                            self.emit(body, mark, dup || copy > 0)?;
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
