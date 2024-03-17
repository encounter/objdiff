pub mod code;
pub mod data;
pub mod display;

use anyhow::Result;

use crate::{
    diff::{
        code::{diff_code, find_section_and_symbol, no_diff_code},
        data::{diff_bss_symbols, diff_data, no_diff_data},
    },
    obj::{ObjInfo, ObjIns, ObjSectionKind},
};

#[derive(Debug, Copy, Clone, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum X86Formatter {
    #[default]
    Intel,
    Gas,
    Nasm,
    Masm,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct DiffObjConfig {
    pub relax_reloc_diffs: bool,
    pub space_between_args: bool,
    pub x86_formatter: X86Formatter,
}

pub struct ProcessCodeResult {
    pub ops: Vec<u16>,
    pub insts: Vec<ObjIns>,
}

pub fn diff_objs(
    config: &DiffObjConfig,
    mut left: Option<&mut ObjInfo>,
    mut right: Option<&mut ObjInfo>,
) -> Result<()> {
    if let Some(left) = left.as_mut() {
        for left_section in &mut left.sections {
            if left_section.kind == ObjSectionKind::Code {
                for left_symbol in &mut left_section.symbols {
                    if let Some((right, (right_section_idx, right_symbol_idx))) =
                        right.as_mut().and_then(|obj| {
                            find_section_and_symbol(obj, &left_symbol.name).map(|s| (obj, s))
                        })
                    {
                        let right_section = &mut right.sections[right_section_idx];
                        let right_symbol = &mut right_section.symbols[right_symbol_idx];
                        left_symbol.diff_symbol = Some(right_symbol.name.clone());
                        right_symbol.diff_symbol = Some(left_symbol.name.clone());
                        diff_code(
                            left.arch.as_ref(),
                            config,
                            &left_section.data,
                            &right_section.data,
                            left_symbol,
                            right_symbol,
                            &left_section.relocations,
                            &right_section.relocations,
                            &left.line_info,
                            &right.line_info,
                        )?;
                    } else {
                        no_diff_code(
                            left.arch.as_ref(),
                            config,
                            &left_section.data,
                            left_symbol,
                            &left_section.relocations,
                            &left.line_info,
                        )?;
                    }
                }
            } else if let Some(right_section) = right
                .as_mut()
                .and_then(|obj| obj.sections.iter_mut().find(|s| s.name == left_section.name))
            {
                if left_section.kind == ObjSectionKind::Data {
                    diff_data(left_section, right_section)?;
                } else if left_section.kind == ObjSectionKind::Bss {
                    diff_bss_symbols(&mut left_section.symbols, &mut right_section.symbols)?;
                }
            } else if left_section.kind == ObjSectionKind::Data {
                no_diff_data(left_section);
            }
        }
    }
    if let Some(right) = right.as_mut() {
        for right_section in right.sections.iter_mut() {
            if right_section.kind == ObjSectionKind::Code {
                for right_symbol in &mut right_section.symbols {
                    if right_symbol.instructions.is_empty() {
                        no_diff_code(
                            right.arch.as_ref(),
                            config,
                            &right_section.data,
                            right_symbol,
                            &right_section.relocations,
                            &right.line_info,
                        )?;
                    }
                }
            } else if right_section.kind == ObjSectionKind::Data
                && right_section.data_diff.is_empty()
            {
                no_diff_data(right_section);
            }
        }
    }
    if let (Some(left), Some(right)) = (left, right) {
        diff_bss_symbols(&mut left.common, &mut right.common)?;
    }
    Ok(())
}
