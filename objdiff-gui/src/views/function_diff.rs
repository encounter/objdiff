use std::{cmp::Ordering, default::Default};

use egui::{text::LayoutJob, Label, Response, Sense, Widget};
use egui_extras::TableRow;
use objdiff_core::{
    diff::{
        display::{display_diff, DiffText, HighlightKind},
        ObjDiff, ObjInsDiff, ObjInsDiffKind,
    },
    obj::{ObjInfo, ObjIns, ObjInsArg, ObjInsArgValue, ObjSection, ObjSymbol, SymbolRef},
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

fn ins_hover_ui(
    ui: &mut egui::Ui,
    obj: &ObjInfo,
    section: &ObjSection,
    ins: &ObjIns,
    symbol: &ObjSymbol,
    appearance: &Appearance,
) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        let offset = ins.address - section.address;
        ui.label(format!(
            "{:02x?}",
            &section.data[offset as usize..(offset + ins.size as u64) as usize]
        ));

        if let Some(virtual_address) = symbol.virtual_address {
            let offset = ins.address - symbol.address;
            ui.colored_label(
                appearance.replace_color,
                format!("Virtual address: {:#x}", virtual_address + offset),
            );
        }

        if let Some(orig) = &ins.orig {
            ui.label(format!("Original: {}", orig));
        }

        for arg in &ins.args {
            if let ObjInsArg::Arg(arg) = arg {
                match arg {
                    ObjInsArgValue::Signed(v) => {
                        ui.label(format!("{arg} == {v}"));
                    }
                    ObjInsArgValue::Unsigned(v) => {
                        ui.label(format!("{arg} == {v}"));
                    }
                    _ => {}
                }
            }
        }

        if let Some(reloc) = &ins.reloc {
            ui.label(format!("Relocation type: {}", obj.arch.display_reloc(reloc.flags)));
            let addend_str = match reloc.addend.cmp(&0i64) {
                Ordering::Greater => format!("+{:x}", reloc.addend),
                Ordering::Less => format!("-{:x}", -reloc.addend),
                _ => "".to_string(),
            };
            ui.colored_label(
                appearance.highlight_color,
                format!("Name: {}{}", reloc.target.name, addend_str),
            );
            if let Some(orig_section_index) = reloc.target.orig_section_index {
                if let Some(section) =
                    obj.sections.iter().find(|s| s.orig_index == orig_section_index)
                {
                    ui.colored_label(
                        appearance.highlight_color,
                        format!("Section: {}", section.name),
                    );
                }
                ui.colored_label(
                    appearance.highlight_color,
                    format!("Address: {:x}{}", reloc.target.address, addend_str),
                );
                ui.colored_label(
                    appearance.highlight_color,
                    format!("Size: {:x}", reloc.target.size),
                );
                for label in obj.arch.display_ins_data_labels(ins) {
                    ui.colored_label(appearance.highlight_color, label);
                }
            } else {
                ui.colored_label(appearance.highlight_color, "Extern".to_string());
            }
        }

        if let Some(decoded) = rlwinmdec::decode(&ins.formatted) {
            ui.colored_label(appearance.highlight_color, decoded.trim());
        }
    });
}

fn ins_context_menu(
    ui: &mut egui::Ui,
    obj: &ObjInfo,
    section: &ObjSection,
    ins: &ObjIns,
    symbol: &ObjSymbol,
) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        if ui.button(format!("Copy \"{}\"", ins.formatted)).clicked() {
            ui.output_mut(|output| output.copied_text.clone_from(&ins.formatted));
            ui.close_menu();
        }

        let mut hex_string = "0x".to_string();
        for byte in &section.data[ins.address as usize..(ins.address + ins.size as u64) as usize] {
            hex_string.push_str(&format!("{:02x}", byte));
        }
        if ui.button(format!("Copy \"{hex_string}\" (instruction bytes)")).clicked() {
            ui.output_mut(|output| output.copied_text = hex_string);
            ui.close_menu();
        }

        if let Some(virtual_address) = symbol.virtual_address {
            let offset = ins.address - symbol.address;
            let offset_string = format!("{:#x}", virtual_address + offset);
            if ui.button(format!("Copy \"{offset_string}\" (virtual address)")).clicked() {
                ui.output_mut(|output| output.copied_text = offset_string);
                ui.close_menu();
            }
        }

        for arg in &ins.args {
            if let ObjInsArg::Arg(arg) = arg {
                match arg {
                    ObjInsArgValue::Signed(v) => {
                        if ui.button(format!("Copy \"{arg}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = arg.to_string());
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = v.to_string());
                            ui.close_menu();
                        }
                    }
                    ObjInsArgValue::Unsigned(v) => {
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
        if let Some(reloc) = &ins.reloc {
            for literal in obj.arch.display_ins_data_literals(ins) {
                if ui.button(format!("Copy \"{literal}\"")).clicked() {
                    ui.output_mut(|output| output.copied_text.clone_from(&literal));
                    ui.close_menu();
                }
            }
            if let Some(name) = &reloc.target.demangled_name {
                if ui.button(format!("Copy \"{name}\"")).clicked() {
                    ui.output_mut(|output| output.copied_text.clone_from(name));
                    ui.close_menu();
                }
            }
            if ui.button(format!("Copy \"{}\"", reloc.target.name)).clicked() {
                ui.output_mut(|output| output.copied_text.clone_from(&reloc.target.name));
                ui.close_menu();
            }
        }
    });
}

#[must_use]
#[expect(clippy::too_many_arguments)]
fn diff_text_ui(
    ui: &mut egui::Ui,
    text: DiffText<'_>,
    ins_diff: &ObjInsDiff,
    appearance: &Appearance,
    ins_view_state: &FunctionViewState,
    column: usize,
    space_width: f32,
    response_cb: impl Fn(Response) -> Response,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let label_text;
    let mut base_color = match ins_diff.kind {
        ObjInsDiffKind::None | ObjInsDiffKind::OpMismatch | ObjInsDiffKind::ArgMismatch => {
            appearance.text_color
        }
        ObjInsDiffKind::Replace => appearance.replace_color,
        ObjInsDiffKind::Delete => appearance.delete_color,
        ObjInsDiffKind::Insert => appearance.insert_color,
    };
    let mut pad_to = 0;
    match text {
        DiffText::Basic(text) => {
            label_text = text.to_string();
        }
        DiffText::BasicColor(s, idx) => {
            label_text = s.to_string();
            base_color = appearance.diff_colors[idx % appearance.diff_colors.len()];
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
            if ins_diff.kind == ObjInsDiffKind::OpMismatch {
                base_color = appearance.replace_color;
            }
            pad_to = 8;
        }
        DiffText::Argument(arg, diff) => {
            label_text = arg.to_string();
            if let Some(diff) = diff {
                base_color = appearance.diff_colors[diff.idx % appearance.diff_colors.len()]
            }
        }
        DiffText::BranchDest(addr, diff) => {
            label_text = format!("{addr:x}");
            if let Some(diff) = diff {
                base_color = appearance.diff_colors[diff.idx % appearance.diff_colors.len()]
            }
        }
        DiffText::Symbol(sym, diff) => {
            let name = sym.demangled_name.as_ref().unwrap_or(&sym.name);
            label_text = name.clone();
            if let Some(diff) = diff {
                base_color = appearance.diff_colors[diff.idx % appearance.diff_colors.len()]
            } else {
                base_color = appearance.emphasized_text_color;
            }
        }
        DiffText::Addend(addend, diff) => {
            label_text = match addend.cmp(&0i64) {
                Ordering::Greater => format!("+{:#x}", addend),
                Ordering::Less => format!("-{:#x}", -addend),
                _ => "".to_string(),
            };
            if let Some(diff) = diff {
                base_color = appearance.diff_colors[diff.idx % appearance.diff_colors.len()]
            } else {
                base_color = appearance.emphasized_text_color;
            }
        }
        DiffText::Spacing(n) => {
            ui.add_space(n as f32 * space_width);
            return ret;
        }
        DiffText::Eol => {
            label_text = "\n".to_string();
        }
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
    ins_diff: &ObjInsDiff,
    symbol: &ObjSymbol,
    appearance: &Appearance,
    ins_view_state: &FunctionViewState,
    column: usize,
    response_cb: impl Fn(Response) -> Response,
) -> Option<DiffViewAction> {
    let mut ret = None;
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    if ins_diff.kind != ObjInsDiffKind::None {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }
    let space_width = ui.fonts(|f| f.glyph_width(&appearance.code_font, ' '));
    display_diff(ins_diff, symbol.address, |text| {
        if let Some(action) = diff_text_ui(
            ui,
            text,
            ins_diff,
            appearance,
            ins_view_state,
            column,
            space_width,
            &response_cb,
        ) {
            ret = Some(action);
        }
        Ok::<_, ()>(())
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
    column: usize,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let symbol_ref = ctx.symbol_ref?;
    let (section, symbol) = ctx.obj.section_symbol(symbol_ref);
    let section = section?;
    let ins_diff = &ctx.diff.symbol_diff(symbol_ref).instructions[row.index()];
    let response_cb = |response: Response| {
        if let Some(ins) = &ins_diff.ins {
            response.context_menu(|ui| ins_context_menu(ui, ctx.obj, section, ins, symbol));
            response.on_hover_ui_at_pointer(|ui| {
                ins_hover_ui(ui, ctx.obj, section, ins, symbol, appearance)
            })
        } else {
            response
        }
    };
    let (_, response) = row.col(|ui| {
        if let Some(action) =
            asm_row_ui(ui, ins_diff, symbol, appearance, ins_view_state, column, response_cb)
        {
            ret = Some(action);
        }
    });
    response_cb(response);
    ret
}

#[derive(Clone, Copy)]
pub struct FunctionDiffContext<'a> {
    pub obj: &'a ObjInfo,
    pub diff: &'a ObjDiff,
    pub symbol_ref: Option<SymbolRef>,
}
