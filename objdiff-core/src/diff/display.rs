use std::cmp::Ordering;

use crate::{
    diff::{ObjInsArgDiff, ObjInsDiff},
    obj::{ObjInsArg, ObjInsArgValue, ObjReloc, ObjSymbol},
};

#[derive(Debug, Copy, Clone)]
pub enum DiffText<'a> {
    /// Basic text
    Basic(&'a str),
    /// Colored text
    BasicColor(&'a str, usize),
    /// Line number
    Line(usize),
    /// Instruction address
    Address(u64),
    /// Instruction mnemonic
    Opcode(&'a str, u16),
    /// Instruction argument
    Argument(&'a ObjInsArgValue, Option<&'a ObjInsArgDiff>),
    /// Branch destination
    BranchDest(u64),
    /// Symbol name
    Symbol(&'a ObjSymbol),
    /// Number of spaces
    Spacing(usize),
    /// End of line
    Eol,
}

#[derive(Default, Clone, PartialEq, Eq)]
pub enum HighlightKind {
    #[default]
    None,
    Opcode(u16),
    Arg(ObjInsArgValue),
    Symbol(String),
    Address(u64),
}

pub fn display_diff<E>(
    ins_diff: &ObjInsDiff,
    base_addr: u64,
    mut cb: impl FnMut(DiffText) -> Result<(), E>,
) -> Result<(), E> {
    let Some(ins) = &ins_diff.ins else {
        cb(DiffText::Eol)?;
        return Ok(());
    };
    if let Some(line) = ins.line {
        cb(DiffText::Line(line as usize))?;
    }
    cb(DiffText::Address(ins.address - base_addr))?;
    if let Some(branch) = &ins_diff.branch_from {
        cb(DiffText::BasicColor(" ~> ", branch.branch_idx))?;
    } else {
        cb(DiffText::Spacing(4))?;
    }
    cb(DiffText::Opcode(&ins.mnemonic, ins.op))?;
    for (i, arg) in ins.args.iter().enumerate() {
        if i == 0 {
            cb(DiffText::Spacing(1))?;
        }
        match arg {
            ObjInsArg::PlainText(s) => {
                cb(DiffText::Basic(s))?;
            }
            ObjInsArg::Arg(v) => {
                let diff = ins_diff.arg_diff.get(i).and_then(|o| o.as_ref());
                cb(DiffText::Argument(v, diff))?;
            }
            ObjInsArg::Reloc => {
                display_reloc_name(ins.reloc.as_ref().unwrap(), &mut cb)?;
            }
            ObjInsArg::BranchDest(dest) => {
                if let Some(dest) = dest.checked_sub(base_addr) {
                    cb(DiffText::BranchDest(dest))?;
                } else {
                    cb(DiffText::Basic("<unknown>"))?;
                }
            }
        }
    }
    if let Some(branch) = &ins_diff.branch_to {
        cb(DiffText::BasicColor(" ~>", branch.branch_idx))?;
    }
    cb(DiffText::Eol)?;
    Ok(())
}

fn display_reloc_name<E>(
    reloc: &ObjReloc,
    mut cb: impl FnMut(DiffText) -> Result<(), E>,
) -> Result<(), E> {
    cb(DiffText::Symbol(&reloc.target))?;
    match reloc.target.addend.cmp(&0i64) {
        Ordering::Greater => cb(DiffText::Basic(&format!("+{:#x}", reloc.target.addend))),
        Ordering::Less => cb(DiffText::Basic(&format!("-{:#x}", -reloc.target.addend))),
        _ => Ok(()),
    }
}

impl PartialEq<DiffText<'_>> for HighlightKind {
    fn eq(&self, other: &DiffText) -> bool {
        match (self, other) {
            (HighlightKind::Opcode(a), DiffText::Opcode(_, b)) => a == b,
            (HighlightKind::Arg(a), DiffText::Argument(b, _)) => a.loose_eq(b),
            (HighlightKind::Symbol(a), DiffText::Symbol(b)) => a == &b.name,
            (HighlightKind::Address(a), DiffText::Address(b) | DiffText::BranchDest(b)) => a == b,
            _ => false,
        }
    }
}

impl PartialEq<HighlightKind> for DiffText<'_> {
    fn eq(&self, other: &HighlightKind) -> bool { other.eq(self) }
}

impl From<DiffText<'_>> for HighlightKind {
    fn from(value: DiffText<'_>) -> Self {
        match value {
            DiffText::Opcode(_, op) => HighlightKind::Opcode(op),
            DiffText::Argument(arg, _) => HighlightKind::Arg(arg.clone()),
            DiffText::Symbol(sym) => HighlightKind::Symbol(sym.name.to_string()),
            DiffText::Address(addr) | DiffText::BranchDest(addr) => HighlightKind::Address(addr),
            _ => HighlightKind::None,
        }
    }
}
