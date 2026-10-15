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
}

/// Programs larger than this are refused, as an exhausted regcomp would.
const MAX_INSTS: usize = 1 << 23;

pub fn compile(node: &Node, ngroups: usize, referenced: Vec<bool>) -> Result<Program, CompileError> {
    let mut c = Compiler { insts: Vec::new(), units: Vec::new() };
    c.emit(node, false)?;
    c.insts.push(Inst::Match);
    let has_backrefs = referenced.iter().any(|&r| r);
    let mut prog = Program { insts: c.insts, units: c.units, ngroups, referenced, has_backrefs, first_bytes: None };
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

    fn emit(&mut self, node: &Node, opt: bool) -> Result<(), CompileError> {
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
                self.emit(inner, false)?;
                self.push(Inst::Close(*g as u32, opt))?;
            }
            Node::Concat(items) => {
                for it in items {
                    self.emit(it, false)?;
                }
            }
            Node::Alt(branches) => {
                let mut jumps = Vec::new();
                for (k, b) in branches.iter().enumerate() {
                    if k + 1 < branches.len() {
                        let split = self.push(Inst::Split(0, 0))?;
                        self.emit(b, false)?;
                        jumps.push(self.push(Inst::Jmp(0))?);
                        let next = self.pc();
                        self.insts[split as usize] = Inst::Split(split + 1, next);
                    } else {
                        self.emit(b, false)?;
                    }
                }
                let end = self.pc();
                for j in jumps {
                    self.insts[j as usize] = Inst::Jmp(end);
                }
            }
            Node::Repeat(body, min, max) => {
                let is_group = matches!(**body, Node::Group(..));
                for _ in 0..*min {
                    self.emit(body, false)?;
                }
                match max {
                    None => {
                        let l = self.push(Inst::Split(0, 0))?;
                        self.emit(body, is_group)?;
                        self.push(Inst::Jmp(l))?;
                        let end = self.pc();
                        self.insts[l as usize] = Inst::Split(l + 1, end);
                    }
                    Some(max) => {
                        let mut holes = Vec::new();
                        for _ in *min..*max {
                            holes.push(self.push(Inst::Split(0, 0))?);
                            self.emit(body, is_group)?;
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
