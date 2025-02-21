use std::{cmp::Ordering, default::Default};

use egui::{text::LayoutJob, Label, Response, Sense, Widget};
use egui_extras::TableRow;
use objdiff_core::{
    diff::{
        display::{display_row, DiffText, HighlightKind},
        DiffObjConfig, InstructionArgDiffIndex, InstructionDiffKind, InstructionDiffRow,
        ObjectDiff,
    },
    obj::{
        InstructionArg, InstructionArgValue, InstructionRef, Object, ParsedInstruction,
        ResolvedRelocation, Section, Symbol,
    },
};

use crate::views::{appearance::Appearance, symbol_diff::DiffViewAction};

#[derive(Default)]
pub struct FunctionViewState {
    left_highlight: HighlightKind,
    right_highlight: HighlightKind,
}

impl FunctionViewState {
    pub fn highlight(&self, column: usize) -> &HighlightKind {
        match column {
            0 => &self.left_highlight,
            1 => &self.right_highlight,
            _ => &HighlightKind::None,
        }
    }

    pub fn set_highlight(&mut self, column: usize, highlight: HighlightKind) {
        match column {
            0 => {
                if highlight == self.left_highlight {
                    if highlight == self.right_highlight {
                        self.left_highlight = HighlightKind::None;
                        self.right_highlight = HighlightKind::None;
                    } else {
                        self.right_highlight = self.left_highlight.clone();
                    }
                } else {
                    self.left_highlight = highlight;
                }
            }
            1 => {
                if highlight == self.right_highlight {
                    if highlight == self.left_highlight {
                        self.left_highlight = HighlightKind::None;
                        self.right_highlight = HighlightKind::None;
                    } else {
                        self.left_highlight = self.right_highlight.clone();
                    }
                } else {
                    self.right_highlight = highlight;
                }
            }
            _ => {}
        }
    }

    pub fn clear_highlight(&mut self) {
        self.left_highlight = HighlightKind::None;
        self.right_highlight = HighlightKind::None;
    }
}

#[expect(unused)]
#[derive(Clone, Copy)]
pub struct ResolvedInstructionRef<'obj> {
    pub symbol: &'obj Symbol,
    pub section_idx: usize,
    pub section: &'obj Section,
    pub data: &'obj [u8],
    pub relocation: Option<ResolvedRelocation<'obj>>,
}

fn resolve_instruction_ref(
    obj: &Object,
    symbol_idx: usize,
    ins_ref: InstructionRef,
) -> Option<ResolvedInstructionRef> {
    let symbol = &obj.symbols[symbol_idx];
    let section_idx = symbol.section?;
    let section = &obj.sections[section_idx];
    let offset = ins_ref.address.checked_sub(section.address)?;
    let data = section.data.get(offset as usize..offset as usize + ins_ref.size as usize)?;
    let relocation = section.relocation_at(ins_ref.address, obj);
    Some(ResolvedInstructionRef { symbol, section, section_idx, data, relocation })
}

fn resolve_instruction<'obj>(
    obj: &'obj Object,
    symbol_idx: usize,
    ins_ref: InstructionRef,
    diff_config: &DiffObjConfig,
) -> Option<(ResolvedInstructionRef<'obj>, ParsedInstruction)> {
    let resolved = resolve_instruction_ref(obj, symbol_idx, ins_ref)?;
    let ins = obj
        .arch
        .process_instruction(
            ins_ref,
            resolved.data,
            resolved.relocation,
            resolved.symbol.address..resolved.symbol.address + resolved.symbol.size,
            resolved.section_idx,
            diff_config,
        )
        .ok()?;
    Some((resolved, ins))
}

fn ins_hover_ui(
    ui: &mut egui::Ui,
    obj: &Object,
    symbol_idx: usize,
    ins_ref: InstructionRef,
    diff_config: &DiffObjConfig,
    appearance: &Appearance,
) {
    let Some((
        ResolvedInstructionRef { symbol, section_idx: _, section: _, data, relocation },
        ins,
    )) = resolve_instruction(obj, symbol_idx, ins_ref, diff_config)
    else {
        ui.colored_label(appearance.delete_color, "Failed to resolve instruction");
        return;
    };

    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        ui.label(format!("{:02x?}", data));

        if let Some(virtual_address) = symbol.virtual_address {
            let offset = ins_ref.address - symbol.address;
            ui.colored_label(
                appearance.replace_color,
                format!("Virtual address: {:#x}", virtual_address + offset),
            );
        }

        // TODO
        // if let Some(orig) = &ins.orig {
        //     ui.label(format!("Original: {}", orig));
        // }

        for arg in &ins.args {
            if let InstructionArg::Value(arg) = arg {
                match arg {
                    InstructionArgValue::Signed(v) => {
                        ui.label(format!("{arg} == {v}"));
                    }
                    InstructionArgValue::Unsigned(v) => {
                        ui.label(format!("{arg} == {v}"));
                    }
                    _ => {}
                }
            }
        }

        if let Some(resolved) = relocation {
            ui.label(format!(
                "Relocation type: {}",
                obj.arch.display_reloc(resolved.relocation.flags)
            ));
            let addend_str = match resolved.relocation.addend.cmp(&0i64) {
                Ordering::Greater => format!("+{:x}", resolved.relocation.addend),
                Ordering::Less => format!("-{:x}", -resolved.relocation.addend),
                _ => "".to_string(),
            };
            ui.colored_label(
                appearance.highlight_color,
                format!("Name: {}{}", resolved.symbol.name, addend_str),
            );
            if let Some(orig_section_index) = resolved.symbol.section {
                let section = &obj.sections[orig_section_index];
                ui.colored_label(appearance.highlight_color, format!("Section: {}", section.name));
                ui.colored_label(
                    appearance.highlight_color,
                    format!("Address: {:x}{}", resolved.symbol.address, addend_str),
                );
                ui.colored_label(
                    appearance.highlight_color,
                    format!("Size: {:x}", resolved.symbol.size),
                );
                // TODO
                // for label in obj.arch.display_ins_data_labels(ins) {
                //     ui.colored_label(appearance.highlight_color, label);
                // }
            } else {
                ui.colored_label(appearance.highlight_color, "Extern".to_string());
            }
        }

        // TODO
        // if let Some(decoded) = rlwinmdec::decode(&ins.formatted) {
        //     ui.colored_label(appearance.highlight_color, decoded.trim());
        // }
    });
}

fn ins_context_menu(
    ui: &mut egui::Ui,
    obj: &Object,
    symbol_idx: usize,
    ins_ref: InstructionRef,
    diff_config: &DiffObjConfig,
    appearance: &Appearance,
) {
    let Some((
        ResolvedInstructionRef { symbol, section_idx: _, section: _, data, relocation },
        ins,
    )) = resolve_instruction(obj, symbol_idx, ins_ref, diff_config)
    else {
        ui.colored_label(appearance.delete_color, "Failed to resolve instruction");
        return;
    };

    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        // TODO
        // if ui.button(format!("Copy \"{}\"", ins.formatted)).clicked() {
        //     ui.output_mut(|output| output.copied_text.clone_from(&ins.formatted));
        //     ui.close_menu();
        // }

        let mut hex_string = "0x".to_string();
        for byte in data {
            hex_string.push_str(&format!("{:02x}", byte));
        }
        if ui.button(format!("Copy \"{hex_string}\" (instruction bytes)")).clicked() {
            ui.output_mut(|output| output.copied_text = hex_string);
            ui.close_menu();
        }

        if let Some(virtual_address) = symbol.virtual_address {
            let offset = ins_ref.address - symbol.address;
            let offset_string = format!("{:#x}", virtual_address + offset);
            if ui.button(format!("Copy \"{offset_string}\" (virtual address)")).clicked() {
                ui.output_mut(|output| output.copied_text = offset_string);
                ui.close_menu();
            }
        }

        for arg in &ins.args {
            if let InstructionArg::Value(arg) = arg {
                match arg {
                    InstructionArgValue::Signed(v) => {
                        if ui.button(format!("Copy \"{arg}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = arg.to_string());
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = v.to_string());
                            ui.close_menu();
                        }
                    }
                    InstructionArgValue::Unsigned(v) => {
                        if ui.button(format!("Copy \"{arg}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = arg.to_string());
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = v.to_string());
                            ui.close_menu();
                        }
                    }
                    _ => {}
                }
            }
        }

        if let Some(resolved) = relocation {
            // TODO
            // for literal in obj.arch.display_ins_data_literals(ins) {
            //     if ui.button(format!("Copy \"{literal}\"")).clicked() {
            //         ui.output_mut(|output| output.copied_text.clone_from(&literal));
            //         ui.close_menu();
            //     }
            // }
            if let Some(name) = &resolved.symbol.demangled_name {
                if ui.button(format!("Copy \"{name}\"")).clicked() {
                    ui.output_mut(|output| output.copied_text.clone_from(name));
                    ui.close_menu();
                }
            }
            if ui.button(format!("Copy \"{}\"", resolved.symbol.name)).clicked() {
                ui.output_mut(|output| output.copied_text.clone_from(&resolved.symbol.name));
                ui.close_menu();
            }
        }
    });
}

#[must_use]
fn diff_text_ui(
    ui: &mut egui::Ui,
    text: DiffText<'_>,
    diff: InstructionArgDiffIndex,
    ins_diff: &InstructionDiffRow,
    appearance: &Appearance,
    ins_view_state: &FunctionViewState,
    column: usize,
    space_width: f32,
    response_cb: impl Fn(Response) -> Response,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let label_text;
    let mut base_color = match ins_diff.kind {
        InstructionDiffKind::None
        | InstructionDiffKind::OpMismatch
        | InstructionDiffKind::ArgMismatch => appearance.text_color,
        InstructionDiffKind::Replace => appearance.replace_color,
        InstructionDiffKind::Delete => appearance.delete_color,
        InstructionDiffKind::Insert => appearance.insert_color,
    };
    let mut pad_to = 0;
    match text {
        DiffText::Basic(text) => {
            label_text = text.to_string();
        }
        DiffText::Line(num) => {
            label_text = num.to_string();
            base_color = appearance.deemphasized_text_color;
            pad_to = 5;
        }
        DiffText::Address(addr) => {
            label_text = format!("{:x}:", addr);
            pad_to = 5;
        }
        DiffText::Opcode(mnemonic, _op) => {
            label_text = mnemonic.to_string();
            if ins_diff.kind == InstructionDiffKind::OpMismatch {
                base_color = appearance.replace_color;
            }
            pad_to = 8;
        }
        DiffText::Argument(arg) => {
            label_text = arg.to_string();
        }
        DiffText::BranchDest(addr) => {
            label_text = format!("{addr:x}");
        }
        DiffText::Symbol(sym) => {
            let name = sym.demangled_name.as_ref().unwrap_or(&sym.name);
            label_text = name.clone();
            base_color = appearance.emphasized_text_color;
        }
        DiffText::Addend(addend) => {
            label_text = match addend.cmp(&0i64) {
                Ordering::Greater => format!("+{:#x}", addend),
                Ordering::Less => format!("-{:#x}", -addend),
                _ => "".to_string(),
            };
            base_color = appearance.emphasized_text_color;
        }
        DiffText::Spacing(n) => {
            ui.add_space(n as f32 * space_width);
            return ret;
        }
        DiffText::Eol => {
            label_text = "\n".to_string();
        }
    }
    if let Some(diff_idx) = diff.get() {
        base_color = appearance.diff_colors[diff_idx as usize % appearance.diff_colors.len()];
    }

    let len = label_text.len();
    let highlight = *ins_view_state.highlight(column) == text;
    let mut response = Label::new(LayoutJob::single_section(
        label_text,
        appearance.code_text_format(base_color, highlight),
    ))
    .sense(Sense::click())
    .ui(ui);
    response = response_cb(response);
    if response.clicked() {
        ret = Some(DiffViewAction::SetDiffHighlight(column, text.into()));
    }
    if len < pad_to {
        ui.add_space((pad_to - len) as f32 * space_width);
    }
    ret
}

#[must_use]
fn asm_row_ui(
    ui: &mut egui::Ui,
    obj: &Object,
    ins_diff: &InstructionDiffRow,
    symbol_idx: usize,
    appearance: &Appearance,
    ins_view_state: &FunctionViewState,
    diff_config: &DiffObjConfig,
    column: usize,
    response_cb: impl Fn(Response) -> Response,
) -> Option<DiffViewAction> {
    let mut ret = None;
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    if ins_diff.kind != InstructionDiffKind::None {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }
    let space_width = ui.fonts(|f| f.glyph_width(&appearance.code_font, ' '));
    display_row(obj, symbol_idx, ins_diff, diff_config, |text, diff| {
        if let Some(action) = diff_text_ui(
            ui,
            text,
            diff,
            ins_diff,
            appearance,
            ins_view_state,
            column,
            space_width,
            &response_cb,
        ) {
            ret = Some(action);
        }
        Ok(())
    })
    .unwrap();
    ret
}

#[must_use]
pub(crate) fn asm_col_ui(
    row: &mut TableRow<'_, '_>,
    ctx: FunctionDiffContext<'_>,
    appearance: &Appearance,
    ins_view_state: &FunctionViewState,
    diff_config: &DiffObjConfig,
    column: usize,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let symbol_ref = ctx.symbol_ref?;
    let ins_row = &ctx.diff.symbols[symbol_ref].instruction_rows[row.index()];
    let response_cb = |response: Response| {
        if let Some(ins_ref) = ins_row.ins_ref {
            response.context_menu(|ui| {
                ins_context_menu(ui, ctx.obj, symbol_ref, ins_ref, diff_config, appearance)
            });
            response.on_hover_ui_at_pointer(|ui| {
                ins_hover_ui(ui, ctx.obj, symbol_ref, ins_ref, diff_config, appearance)
            })
        } else {
            response
        }
    };
    let (_, response) = row.col(|ui| {
        if let Some(action) = asm_row_ui(
            ui,
            ctx.obj,
            ins_row,
            symbol_ref,
            appearance,
            ins_view_state,
            diff_config,
            column,
            response_cb,
        ) {
            ret = Some(action);
        }
    });
    response_cb(response);
    ret
}

#[derive(Clone, Copy)]
pub struct FunctionDiffContext<'a> {
    pub obj: &'a Object,
    pub diff: &'a ObjectDiff,
    pub symbol_ref: Option<usize>,
}
