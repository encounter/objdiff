//! Core diffing helpers built directly on `objdiff-core`.
//!
//! These are UI-free and synchronous; the MCP server layer wraps them in
//! `spawn_blocking` and turns the results into tool responses.

use std::{collections::BTreeMap, fmt::Write as _, path::Path};

use anyhow::{Context, Result};
use objdiff_core::{
    bindings::diff::{DiffKind, DiffObject, DiffResult, DiffSymbol, DiffSymbolKind},
    diff::{DiffObjConfig, DiffSide, MappingConfig, diff_objs},
    obj::read,
};

/// Load target + base objects from disk and produce a full serializable diff.
///
/// `target` is the expected/baseline object (left); `base` is your current
/// build (right).
pub fn run_diff(
    target: &Path,
    base: &Path,
    config: &DiffObjConfig,
    mappings: &BTreeMap<String, String>,
) -> Result<DiffResult> {
    let target_obj = read::read(target, config, DiffSide::Target)
        .with_context(|| format!("Failed to read target object {}", target.display()))?;
    let base_obj = read::read(base, config, DiffSide::Base)
        .with_context(|| format!("Failed to read base object {}", base.display()))?;
    let mapping_config =
        MappingConfig { mappings: mappings.clone(), selecting_left: None, selecting_right: None };
    let result = diff_objs(Some(&target_obj), Some(&base_obj), None, config, &mapping_config)
        .context("Failed to diff objects")?;
    DiffResult::new(
        result.left.as_ref().map(|d| (&target_obj, d)),
        result.right.as_ref().map(|d| (&base_obj, d)),
        config,
    )
    .context("Failed to build diff result")
}

fn kind_marker(kind: DiffKind) -> &'static str {
    match kind {
        DiffKind::DiffNone => " ",
        DiffKind::DiffReplace => "~",
        DiffKind::DiffDelete => "-",
        DiffKind::DiffInsert => "+",
        DiffKind::DiffOpMismatch => "o",
        DiffKind::DiffArgMismatch => "a",
    }
}

fn is_function(sym: &DiffSymbol) -> bool {
    DiffSymbolKind::try_from(sym.kind).unwrap_or(DiffSymbolKind::SymbolUnknown)
        == DiffSymbolKind::SymbolFunction
}

/// A compact, token-efficient overview of every code symbol and its match %.
pub fn overview(diff: &DiffResult, min_only_mismatches: bool, limit: usize) -> String {
    let Some(right) = diff.right.as_ref() else {
        return "No base object in diff result.".to_string();
    };
    let mut rows: Vec<(&str, f32, u64)> = right
        .symbols
        .iter()
        .filter(|s| is_function(s))
        .map(|s| (s.name.as_str(), s.match_percent.unwrap_or(0.0), s.size))
        .collect();
    // Worst matches first — that's what you want to work on.
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    if min_only_mismatches {
        rows.retain(|r| r.1 < 100.0);
    }

    let mut out = String::new();
    let total = rows.len();
    let _ = writeln!(out, "{total} function(s) (worst match first):");
    for (name, pct, size) in rows.into_iter().take(limit) {
        let _ = writeln!(out, "  {pct:6.2}%  {size:>6}  {name}");
    }
    out
}

fn instr_text(sym: Option<&DiffSymbol>, i: usize) -> (&str, Option<u64>) {
    match sym.and_then(|s| s.instructions.get(i)) {
        Some(row) => match row.instruction.as_ref() {
            Some(ins) => (ins.formatted.as_str(), Some(ins.address)),
            None => ("", None),
        },
        None => ("", None),
    }
}

/// Render a single function's diff as a side-by-side (target vs current) table
/// with per-row mismatch markers — the primary signal for iterative matching.
pub fn function_diff(diff: &DiffResult, symbol: &str) -> Result<String> {
    let right = diff.right.as_ref().context("No base object in diff result")?;
    let left = diff.left.as_ref();

    // Look the symbol up by its base (current) name first; fall back to the
    // target-side name and follow its pairing back to the base symbol, so
    // mapped symbols (e.g. statics renamed between objects) resolve either way.
    let base_sym = right
        .symbols
        .iter()
        .find(|s| s.name == symbol)
        .or_else(|| {
            left.and_then(|l| l.symbols.iter().find(|s| s.name == symbol))
                .and_then(|ls| ls.target_symbol)
                .and_then(|bi| right.symbols.get(bi as usize))
        })
        .with_context(|| {
            format!("Symbol `{symbol}` not found in base (current) or target object")
        })?;

    let target_sym: Option<&DiffSymbol> = base_sym
        .target_symbol
        .and_then(|ti| left.and_then(|l: &DiffObject| l.symbols.get(ti as usize)));

    let pct = base_sym.match_percent.unwrap_or(0.0);
    let mut out = String::new();
    let _ = writeln!(out, "symbol : {symbol}");
    if let Some(dm) = &base_sym.demangled_name {
        let _ = writeln!(out, "demangled: {dm}");
    }
    let _ = writeln!(out, "match  : {pct:.2}%");
    match target_sym {
        Some(_) => {}
        None => {
            let _ = writeln!(
                out,
                "note   : no matched symbol in target object (unmatched — nothing to compare against)"
            );
        }
    }
    let _ = writeln!(
        out,
        "legend : ' '=equal ~=replace o=opcode-mismatch a=arg-mismatch +=insert -=delete"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {:<8}  {:1}  {:<38}  {:<38}",
        "addr", "", "target (expected)", "current (yours)"
    );

    let n = base_sym.instructions.len().max(target_sym.map(|s| s.instructions.len()).unwrap_or(0));
    for i in 0..n {
        let (rtext, raddr) = instr_text(Some(base_sym), i);
        let (ltext, laddr) = instr_text(target_sym, i);
        let kind = base_sym
            .instructions
            .get(i)
            .map(|r| DiffKind::try_from(r.diff_kind).unwrap_or(DiffKind::DiffNone))
            .unwrap_or(DiffKind::DiffInsert);
        let addr = raddr.or(laddr).unwrap_or(0);
        let _ =
            writeln!(out, "  {:08x}  {:1}  {:<38}  {:<38}", addr, kind_marker(kind), ltext, rtext);
    }
    Ok(out)
}
