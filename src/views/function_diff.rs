use std::{
    cmp::{max, Ordering},
    default::Default,
};

use cwdemangle::demangle;
use eframe::emath::Align;
use egui::{text::LayoutJob, Color32, Label, Layout, RichText, Sense, TextFormat, Vec2};
use egui_extras::{Column, TableBuilder, TableRow};
use ppc750cl::Argument;
use time::format_description;

use crate::{
    obj::{
        ObjInfo, ObjIns, ObjInsArg, ObjInsArgDiff, ObjInsDiff, ObjInsDiffKind, ObjReloc,
        ObjRelocKind, ObjSymbol,
    },
    views::{
        appearance::Appearance,
        symbol_diff::{match_color_for_symbol, DiffViewState, SymbolReference, View},
        write_text,
    },
};

#[derive(Default)]
pub enum HighlightKind {
    #[default]
    None,
    Opcode(u8),
    Arg(ObjInsArg),
    Symbol(String),
    Address(u32),
}

#[derive(Default)]
pub struct FunctionViewState {
    pub highlight: HighlightKind,
}

fn write_reloc_name(
    reloc: &ObjReloc,
    color: Color32,
    background_color: Color32,
    job: &mut LayoutJob,
    appearance: &Appearance,
) {
    let name = reloc.target.demangled_name.as_ref().unwrap_or(&reloc.target.name);
    job.append(name, 0.0, TextFormat {
        font_id: appearance.code_font.clone(),
        color: appearance.emphasized_text_color,
        background: background_color,
        ..Default::default()
    });
    match reloc.target.addend.cmp(&0i64) {
        Ordering::Greater => write_text(
            &format!("+{:#X}", reloc.target.addend),
            color,
            job,
            appearance.code_font.clone(),
        ),
        Ordering::Less => {
            write_text(
                &format!("-{:#X}", -reloc.target.addend),
                color,
                job,
                appearance.code_font.clone(),
            );
        }
        _ => {}
    }
}

fn write_reloc(
    reloc: &ObjReloc,
    color: Color32,
    background_color: Color32,
    job: &mut LayoutJob,
    appearance: &Appearance,
) {
    match reloc.kind {
        ObjRelocKind::PpcAddr16Lo => {
            write_reloc_name(reloc, color, background_color, job, appearance);
            write_text("@l", color, job, appearance.code_font.clone());
        }
        ObjRelocKind::PpcAddr16Hi => {
            write_reloc_name(reloc, color, background_color, job, appearance);
            write_text("@h", color, job, appearance.code_font.clone());
        }
        ObjRelocKind::PpcAddr16Ha => {
            write_reloc_name(reloc, color, background_color, job, appearance);
            write_text("@ha", color, job, appearance.code_font.clone());
        }
        ObjRelocKind::PpcEmbSda21 => {
            write_reloc_name(reloc, color, background_color, job, appearance);
            write_text("@sda21", color, job, appearance.code_font.clone());
        }
        ObjRelocKind::MipsHi16 => {
            write_text("%hi(", color, job, appearance.code_font.clone());
            write_reloc_name(reloc, color, background_color, job, appearance);
            write_text(")", color, job, appearance.code_font.clone());
        }
        ObjRelocKind::MipsLo16 => {
            write_text("%lo(", color, job, appearance.code_font.clone());
            write_reloc_name(reloc, color, background_color, job, appearance);
            write_text(")", color, job, appearance.code_font.clone());
        }
        ObjRelocKind::MipsGot16 => {
            write_text("%got(", color, job, appearance.code_font.clone());
            write_reloc_name(reloc, color, background_color, job, appearance);
            write_text(")", color, job, appearance.code_font.clone());
        }
        ObjRelocKind::MipsCall16 => {
            write_text("%call16(", color, job, appearance.code_font.clone());
            write_reloc_name(reloc, color, background_color, job, appearance);
            write_text(")", color, job, appearance.code_font.clone());
        }
        ObjRelocKind::MipsGpRel16 => {
            write_text("%gp_rel(", color, job, appearance.code_font.clone());
            write_reloc_name(reloc, color, background_color, job, appearance);
            write_text(")", color, job, appearance.code_font.clone());
        }
        ObjRelocKind::PpcRel24 | ObjRelocKind::PpcRel14 | ObjRelocKind::Mips26 => {
            write_reloc_name(reloc, color, background_color, job, appearance);
        }
        ObjRelocKind::Absolute | ObjRelocKind::MipsGpRel32 => {
            write_text("[INVALID]", color, job, appearance.code_font.clone());
        }
    };
}

fn write_ins(
    ins: &ObjIns,
    diff_kind: &ObjInsDiffKind,
    args: &[Option<ObjInsArgDiff>],
    base_addr: u32,
    ui: &mut egui::Ui,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
) {
    let base_color = match diff_kind {
        ObjInsDiffKind::None | ObjInsDiffKind::OpMismatch | ObjInsDiffKind::ArgMismatch => {
            appearance.text_color
        }
        ObjInsDiffKind::Replace => appearance.replace_color,
        ObjInsDiffKind::Delete => appearance.delete_color,
        ObjInsDiffKind::Insert => appearance.insert_color,
    };

    let highlighted_op =
        matches!(ins_view_state.highlight, HighlightKind::Opcode(op) if op == ins.op);
    let op_label = RichText::new(ins.mnemonic.clone())
        .font(appearance.code_font.clone())
        .color(if highlighted_op {
            appearance.emphasized_text_color
        } else {
            match diff_kind {
                ObjInsDiffKind::OpMismatch => appearance.replace_color,
                _ => base_color,
            }
        })
        .background_color(if highlighted_op {
            appearance.deemphasized_text_color
        } else {
            Color32::TRANSPARENT
        });
    if ui.add(Label::new(op_label).sense(Sense::click())).clicked() {
        if highlighted_op {
            ins_view_state.highlight = HighlightKind::None;
        } else {
            ins_view_state.highlight = HighlightKind::Opcode(ins.op);
        }
    }
    let space_width = ui.fonts(|f| f.glyph_width(&appearance.code_font, ' '));
    ui.add_space(space_width * (max(11, ins.mnemonic.len()) - ins.mnemonic.len()) as f32);

    let mut writing_offset = false;
    for (i, arg) in ins.args.iter().enumerate() {
        let mut job = LayoutJob::default();
        if i == 0 {
            write_text(" ", base_color, &mut job, appearance.code_font.clone());
        }
        if i > 0 && !writing_offset {
            write_text(", ", base_color, &mut job, appearance.code_font.clone());
        }
        let highlighted_arg = match &ins_view_state.highlight {
            HighlightKind::Symbol(v) => {
                matches!(arg, ObjInsArg::Reloc | ObjInsArg::RelocWithBase)
                    && matches!(&ins.reloc, Some(reloc) if &reloc.target.name == v)
            }
            HighlightKind::Address(v) => {
                matches!(arg, ObjInsArg::BranchOffset(offset) if (offset + ins.address as i32 - base_addr as i32) as u32 == *v)
            }
            HighlightKind::Arg(v) => v == arg,
            _ => false,
        };
        let color = if highlighted_arg {
            appearance.emphasized_text_color
        } else if let Some(diff) = args.get(i).and_then(|a| a.as_ref()) {
            appearance.diff_colors[diff.idx % appearance.diff_colors.len()]
        } else {
            base_color
        };
        let text_format = TextFormat {
            font_id: appearance.code_font.clone(),
            color,
            background: if highlighted_arg {
                appearance.deemphasized_text_color
            } else {
                Color32::TRANSPARENT
            },
            ..Default::default()
        };
        let mut new_writing_offset = false;
        match arg {
            ObjInsArg::PpcArg(arg) => match arg {
                Argument::Offset(val) => {
                    job.append(&format!("{val}"), 0.0, text_format);
                    write_text("(", base_color, &mut job, appearance.code_font.clone());
                    new_writing_offset = true;
                }
                Argument::Uimm(_) | Argument::Simm(_) => {
                    job.append(&format!("{arg}"), 0.0, text_format);
                }
                _ => {
                    job.append(&format!("{arg}"), 0.0, text_format);
                }
            },
            ObjInsArg::Reloc => {
                write_reloc(
                    ins.reloc.as_ref().unwrap(),
                    base_color,
                    text_format.background,
                    &mut job,
                    appearance,
                );
            }
            ObjInsArg::RelocWithBase => {
                write_reloc(
                    ins.reloc.as_ref().unwrap(),
                    base_color,
                    text_format.background,
                    &mut job,
                    appearance,
                );
                write_text("(", base_color, &mut job, appearance.code_font.clone());
                new_writing_offset = true;
            }
            ObjInsArg::MipsArg(str) => {
                job.append(str.strip_prefix('$').unwrap_or(str), 0.0, text_format);
            }
            ObjInsArg::MipsArgWithBase(str) => {
                job.append(str.strip_prefix('$').unwrap_or(str), 0.0, text_format);
                write_text("(", base_color, &mut job, appearance.code_font.clone());
                new_writing_offset = true;
            }
            ObjInsArg::BranchOffset(offset) => {
                let addr = offset + ins.address as i32 - base_addr as i32;
                job.append(&format!("{addr:x}"), 0.0, text_format);
            }
        }
        if writing_offset {
            write_text(")", base_color, &mut job, appearance.code_font.clone());
        }
        writing_offset = new_writing_offset;
        if ui.add(Label::new(job).sense(Sense::click())).clicked() {
            if highlighted_arg {
                ins_view_state.highlight = HighlightKind::None;
            } else if matches!(arg, ObjInsArg::Reloc | ObjInsArg::RelocWithBase) {
                ins_view_state.highlight =
                    HighlightKind::Symbol(ins.reloc.as_ref().unwrap().target.name.clone());
            } else if let ObjInsArg::BranchOffset(offset) = arg {
                ins_view_state.highlight =
                    HighlightKind::Address((offset + ins.address as i32 - base_addr as i32) as u32);
            } else {
                ins_view_state.highlight = HighlightKind::Arg(arg.clone());
            }
        }
    }
}

fn ins_hover_ui(ui: &mut egui::Ui, ins: &ObjIns, appearance: &Appearance) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap = Some(false);

        ui.label(format!("{:02X?}", ins.code.to_be_bytes()));

        if let Some(orig) = &ins.orig {
            ui.label(format!("Original: {}", orig));
        }

        for arg in &ins.args {
            if let ObjInsArg::PpcArg(arg) = arg {
                match arg {
                    Argument::Uimm(v) => {
                        ui.label(format!("{} == {}", v, v.0));
                    }
                    Argument::Simm(v) => {
                        ui.label(format!("{} == {}", v, v.0));
                    }
                    Argument::Offset(v) => {
                        ui.label(format!("{} == {}", v, v.0));
                    }
                    _ => {}
                }
            }
        }

        if let Some(reloc) = &ins.reloc {
            ui.label(format!("Relocation type: {:?}", reloc.kind));
            ui.colored_label(appearance.highlight_color, format!("Name: {}", reloc.target.name));
            if let Some(section) = &reloc.target_section {
                ui.colored_label(appearance.highlight_color, format!("Section: {section}"));
                ui.colored_label(
                    appearance.highlight_color,
                    format!("Address: {:x}", reloc.target.address),
                );
                ui.colored_label(
                    appearance.highlight_color,
                    format!("Size: {:x}", reloc.target.size),
                );
            } else {
                ui.colored_label(appearance.highlight_color, "Extern".to_string());
            }
        }
    });
}

fn ins_context_menu(ui: &mut egui::Ui, ins: &ObjIns) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap = Some(false);

        // if ui.button("Copy hex").clicked() {}

        for arg in &ins.args {
            if let ObjInsArg::PpcArg(arg) = arg {
                match arg {
                    Argument::Uimm(v) => {
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = format!("{v}"));
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{}\"", v.0)).clicked() {
                            ui.output_mut(|output| output.copied_text = format!("{}", v.0));
                            ui.close_menu();
                        }
                    }
                    Argument::Simm(v) => {
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = format!("{v}"));
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{}\"", v.0)).clicked() {
                            ui.output_mut(|output| output.copied_text = format!("{}", v.0));
                            ui.close_menu();
                        }
                    }
                    Argument::Offset(v) => {
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = format!("{v}"));
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{}\"", v.0)).clicked() {
                            ui.output_mut(|output| output.copied_text = format!("{}", v.0));
                            ui.close_menu();
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(reloc) = &ins.reloc {
            if let Some(name) = &reloc.target.demangled_name {
                if ui.button(format!("Copy \"{name}\"")).clicked() {
                    ui.output_mut(|output| output.copied_text = name.clone());
                    ui.close_menu();
                }
            }
            if ui.button(format!("Copy \"{}\"", reloc.target.name)).clicked() {
                ui.output_mut(|output| output.copied_text = reloc.target.name.clone());
                ui.close_menu();
            }
        }
    });
}

fn find_symbol<'a>(obj: &'a ObjInfo, selected_symbol: &SymbolReference) -> Option<&'a ObjSymbol> {
    obj.sections.iter().find_map(|section| {
        section.symbols.iter().find(|symbol| symbol.name == selected_symbol.symbol_name)
    })
}

fn asm_row_ui(
    ui: &mut egui::Ui,
    ins_diff: &ObjInsDiff,
    symbol: &ObjSymbol,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
) {
    ui.spacing_mut().item_spacing.x = 0.0;
    if ins_diff.kind != ObjInsDiffKind::None {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }
    let mut job = LayoutJob::default();
    let Some(ins) = &ins_diff.ins else {
        ui.label("");
        return;
    };

    let base_color = match ins_diff.kind {
        ObjInsDiffKind::None | ObjInsDiffKind::OpMismatch | ObjInsDiffKind::ArgMismatch => {
            appearance.text_color
        }
        ObjInsDiffKind::Replace => appearance.replace_color,
        ObjInsDiffKind::Delete => appearance.delete_color,
        ObjInsDiffKind::Insert => appearance.insert_color,
    };
    let mut pad = 6;
    if let Some(line) = ins.line {
        let line_str = format!("{line} ");
        write_text(
            &line_str,
            appearance.deemphasized_text_color,
            &mut job,
            appearance.code_font.clone(),
        );
        pad = 12 - line_str.len();
    }
    let base_addr = symbol.address as u32;
    let addr_highlight = matches!(
        &ins_view_state.highlight,
        HighlightKind::Address(v) if *v == (ins.address - base_addr)
    );
    let addr_string = format!("{:x}", ins.address - symbol.address as u32);
    pad -= addr_string.len();
    job.append(&addr_string, 0.0, TextFormat {
        font_id: appearance.code_font.clone(),
        color: if addr_highlight { appearance.emphasized_text_color } else { base_color },
        background: if addr_highlight {
            appearance.deemphasized_text_color
        } else {
            Color32::TRANSPARENT
        },
        ..Default::default()
    });
    if ui.add(Label::new(job).sense(Sense::click())).clicked() {
        if addr_highlight {
            ins_view_state.highlight = HighlightKind::None;
        } else {
            ins_view_state.highlight = HighlightKind::Address(ins.address - base_addr);
        }
    }

    let mut job = LayoutJob::default();
    let space_width = ui.fonts(|f| f.glyph_width(&appearance.code_font, ' '));
    let spacing = space_width * pad as f32;
    job.append(": ", 0.0, TextFormat {
        font_id: appearance.code_font.clone(),
        color: base_color,
        ..Default::default()
    });
    if let Some(branch) = &ins_diff.branch_from {
        job.append("~> ", spacing, TextFormat {
            font_id: appearance.code_font.clone(),
            color: appearance.diff_colors[branch.branch_idx % appearance.diff_colors.len()],
            ..Default::default()
        });
    } else {
        job.append("   ", spacing, TextFormat {
            font_id: appearance.code_font.clone(),
            color: base_color,
            ..Default::default()
        });
    }
    ui.add(Label::new(job));
    write_ins(ins, &ins_diff.kind, &ins_diff.arg_diff, base_addr, ui, appearance, ins_view_state);
    if let Some(branch) = &ins_diff.branch_to {
        let mut job = LayoutJob::default();
        write_text(
            " ~>",
            appearance.diff_colors[branch.branch_idx % appearance.diff_colors.len()],
            &mut job,
            appearance.code_font.clone(),
        );
        ui.add(Label::new(job));
    }
}

fn asm_col_ui(
    row: &mut TableRow<'_, '_>,
    ins_diff: &ObjInsDiff,
    symbol: &ObjSymbol,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
) {
    let (_, response) = row.col(|ui| {
        asm_row_ui(ui, ins_diff, symbol, appearance, ins_view_state);
    });
    if let Some(ins) = &ins_diff.ins {
        response
            .on_hover_ui_at_pointer(|ui| {
                ins_hover_ui(ui, ins, appearance);
            })
            .context_menu(|ui| {
                ins_context_menu(ui, ins);
            });
    }
}

fn empty_col_ui(row: &mut TableRow<'_, '_>) {
    row.col(|ui| {
        ui.label("");
    });
}

fn asm_table_ui(
    table: TableBuilder<'_>,
    left_obj: Option<&ObjInfo>,
    right_obj: Option<&ObjInfo>,
    selected_symbol: &SymbolReference,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
) -> Option<()> {
    let left_symbol = left_obj.and_then(|obj| find_symbol(obj, selected_symbol));
    let right_symbol = right_obj.and_then(|obj| find_symbol(obj, selected_symbol));
    let instructions_len = left_symbol.or(right_symbol).map(|s| s.instructions.len())?;
    table.body(|body| {
        body.rows(appearance.code_font.size, instructions_len, |row_index, mut row| {
            if let Some(symbol) = left_symbol {
                asm_col_ui(
                    &mut row,
                    &symbol.instructions[row_index],
                    symbol,
                    appearance,
                    ins_view_state,
                );
            } else {
                empty_col_ui(&mut row);
            }
            if let Some(symbol) = right_symbol {
                asm_col_ui(
                    &mut row,
                    &symbol.instructions[row_index],
                    symbol,
                    appearance,
                    ins_view_state,
                );
            } else {
                empty_col_ui(&mut row);
            }
        });
    });
    Some(())
}

pub fn function_diff_ui(ui: &mut egui::Ui, state: &mut DiffViewState, appearance: &Appearance) {
    let (Some(result), Some(selected_symbol)) = (&state.build, &state.symbol_state.selected_symbol)
    else {
        return;
    };

    // Header
    let available_width = ui.available_width();
    let column_width = available_width / 2.0;
    ui.allocate_ui_with_layout(
        Vec2 { x: available_width, y: 100.0 },
        Layout::left_to_right(Align::Min),
        |ui| {
            // Left column
            ui.allocate_ui_with_layout(
                Vec2 { x: column_width, y: 100.0 },
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(column_width);

                    if ui.button("Back").clicked() {
                        state.current_view = View::SymbolDiff;
                    }

                    let demangled = demangle(&selected_symbol.symbol_name, &Default::default());
                    let name = demangled.as_deref().unwrap_or(&selected_symbol.symbol_name);
                    let mut job = LayoutJob::simple(
                        name.to_string(),
                        appearance.code_font.clone(),
                        appearance.highlight_color,
                        column_width,
                    );
                    job.wrap.break_anywhere = true;
                    job.wrap.max_rows = 1;
                    ui.label(job);

                    ui.scope(|ui| {
                        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                        ui.label("Diff target:");
                    });
                },
            );

            // Right column
            ui.allocate_ui_with_layout(
                Vec2 { x: column_width, y: 100.0 },
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(column_width);

                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!state.build_running, egui::Button::new("Build"))
                            .clicked()
                        {
                            state.queue_build = true;
                        }
                        ui.scope(|ui| {
                            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                            ui.style_mut().wrap = Some(false);
                            if state.build_running {
                                ui.colored_label(appearance.replace_color, "Building…");
                            } else {
                                ui.label("Last built:");
                                let format =
                                    format_description::parse("[hour]:[minute]:[second]").unwrap();
                                ui.label(
                                    result
                                        .time
                                        .to_offset(appearance.utc_offset)
                                        .format(&format)
                                        .unwrap(),
                                );
                            }
                        });
                    });

                    ui.scope(|ui| {
                        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                        if let Some(match_percent) = result
                            .second_obj
                            .as_ref()
                            .and_then(|obj| find_symbol(obj, selected_symbol))
                            .and_then(|symbol| symbol.match_percent)
                        {
                            ui.colored_label(
                                match_color_for_symbol(match_percent, appearance),
                                &format!("{match_percent:.0}%"),
                            );
                        } else {
                            ui.colored_label(appearance.replace_color, "Missing");
                        }
                        ui.label("Diff base:");
                    });
                },
            );
        },
    );
    ui.separator();

    // Table
    let available_height = ui.available_height();
    let table = TableBuilder::new(ui)
        .striped(false)
        .cell_layout(Layout::left_to_right(Align::Min))
        .columns(Column::exact(column_width).clip(true), 2)
        .resizable(false)
        .auto_shrink([false, false])
        .min_scrolled_height(available_height);
    asm_table_ui(
        table,
        result.first_obj.as_ref(),
        result.second_obj.as_ref(),
        selected_symbol,
        appearance,
        &mut state.function_state,
    );
}
