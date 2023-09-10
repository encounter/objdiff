use std::{cmp::Ordering, default::Default};

use cwdemangle::demangle;
use eframe::emath::Align;
use egui::{text::LayoutJob, Color32, FontId, Label, Layout, Sense, Vec2};
use egui_extras::{Column, TableBuilder};
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

fn write_reloc_name(
    reloc: &ObjReloc,
    color: Color32,
    job: &mut LayoutJob,
    font_id: FontId,
    appearance: &Appearance,
) {
    let name = reloc.target.demangled_name.as_ref().unwrap_or(&reloc.target.name);
    write_text(name, appearance.emphasized_text_color, job, font_id.clone());
    match reloc.target.addend.cmp(&0i64) {
        Ordering::Greater => {
            write_text(&format!("+{:#X}", reloc.target.addend), color, job, font_id)
        }
        Ordering::Less => {
            write_text(&format!("-{:#X}", -reloc.target.addend), color, job, font_id);
        }
        _ => {}
    }
}

fn write_reloc(
    reloc: &ObjReloc,
    color: Color32,
    job: &mut LayoutJob,
    font_id: FontId,
    appearance: &Appearance,
) {
    match reloc.kind {
        ObjRelocKind::PpcAddr16Lo => {
            write_reloc_name(reloc, color, job, font_id.clone(), appearance);
            write_text("@l", color, job, font_id);
        }
        ObjRelocKind::PpcAddr16Hi => {
            write_reloc_name(reloc, color, job, font_id.clone(), appearance);
            write_text("@h", color, job, font_id);
        }
        ObjRelocKind::PpcAddr16Ha => {
            write_reloc_name(reloc, color, job, font_id.clone(), appearance);
            write_text("@ha", color, job, font_id);
        }
        ObjRelocKind::PpcEmbSda21 => {
            write_reloc_name(reloc, color, job, font_id.clone(), appearance);
            write_text("@sda21", color, job, font_id);
        }
        ObjRelocKind::MipsHi16 => {
            write_text("%hi(", color, job, font_id.clone());
            write_reloc_name(reloc, color, job, font_id.clone(), appearance);
            write_text(")", color, job, font_id);
        }
        ObjRelocKind::MipsLo16 => {
            write_text("%lo(", color, job, font_id.clone());
            write_reloc_name(reloc, color, job, font_id.clone(), appearance);
            write_text(")", color, job, font_id);
        }
        ObjRelocKind::MipsGot16 => {
            write_text("%got(", color, job, font_id.clone());
            write_reloc_name(reloc, color, job, font_id.clone(), appearance);
            write_text(")", color, job, font_id);
        }
        ObjRelocKind::MipsCall16 => {
            write_text("%call16(", color, job, font_id.clone());
            write_reloc_name(reloc, color, job, font_id.clone(), appearance);
            write_text(")", color, job, font_id);
        }
        ObjRelocKind::MipsGpRel16 => {
            write_text("%gp_rel(", color, job, font_id.clone());
            write_reloc_name(reloc, color, job, font_id.clone(), appearance);
            write_text(")", color, job, font_id);
        }
        ObjRelocKind::PpcRel24 | ObjRelocKind::PpcRel14 | ObjRelocKind::Mips26 => {
            write_reloc_name(reloc, color, job, font_id, appearance);
        }
        ObjRelocKind::Absolute | ObjRelocKind::MipsGpRel32 => {
            write_text("[INVALID]", color, job, font_id);
        }
    };
}

fn write_ins(
    ins: &ObjIns,
    diff_kind: &ObjInsDiffKind,
    args: &[Option<ObjInsArgDiff>],
    base_addr: u32,
    job: &mut LayoutJob,
    appearance: &Appearance,
) {
    let base_color = match diff_kind {
        ObjInsDiffKind::None | ObjInsDiffKind::OpMismatch | ObjInsDiffKind::ArgMismatch => {
            appearance.text_color
        }
        ObjInsDiffKind::Replace => appearance.replace_color,
        ObjInsDiffKind::Delete => appearance.delete_color,
        ObjInsDiffKind::Insert => appearance.insert_color,
    };
    write_text(
        &format!("{:<11}", ins.mnemonic),
        match diff_kind {
            ObjInsDiffKind::OpMismatch => appearance.replace_color,
            _ => base_color,
        },
        job,
        appearance.code_font.clone(),
    );
    let mut writing_offset = false;
    for (i, arg) in ins.args.iter().enumerate() {
        if i == 0 {
            write_text(" ", base_color, job, appearance.code_font.clone());
        }
        if i > 0 && !writing_offset {
            write_text(", ", base_color, job, appearance.code_font.clone());
        }
        let color = if let Some(diff) = args.get(i).and_then(|a| a.as_ref()) {
            appearance.diff_colors[diff.idx % appearance.diff_colors.len()]
        } else {
            base_color
        };
        match arg {
            ObjInsArg::PpcArg(arg) => match arg {
                Argument::Offset(val) => {
                    write_text(&format!("{val}"), color, job, appearance.code_font.clone());
                    write_text("(", base_color, job, appearance.code_font.clone());
                    writing_offset = true;
                    continue;
                }
                Argument::Uimm(_) | Argument::Simm(_) => {
                    write_text(&format!("{arg}"), color, job, appearance.code_font.clone());
                }
                _ => {
                    write_text(&format!("{arg}"), color, job, appearance.code_font.clone());
                }
            },
            ObjInsArg::Reloc => {
                write_reloc(
                    ins.reloc.as_ref().unwrap(),
                    base_color,
                    job,
                    appearance.code_font.clone(),
                    appearance,
                );
            }
            ObjInsArg::RelocWithBase => {
                write_reloc(
                    ins.reloc.as_ref().unwrap(),
                    base_color,
                    job,
                    appearance.code_font.clone(),
                    appearance,
                );
                write_text("(", base_color, job, appearance.code_font.clone());
                writing_offset = true;
                continue;
            }
            ObjInsArg::MipsArg(str) => {
                write_text(
                    str.strip_prefix('$').unwrap_or(str),
                    color,
                    job,
                    appearance.code_font.clone(),
                );
            }
            ObjInsArg::MipsArgWithBase(str) => {
                write_text(
                    str.strip_prefix('$').unwrap_or(str),
                    color,
                    job,
                    appearance.code_font.clone(),
                );
                write_text("(", base_color, job, appearance.code_font.clone());
                writing_offset = true;
                continue;
            }
            ObjInsArg::BranchOffset(offset) => {
                let addr = offset + ins.address as i32 - base_addr as i32;
                write_text(&format!("{addr:x}"), color, job, appearance.code_font.clone());
            }
        }
        if writing_offset {
            write_text(")", base_color, job, appearance.code_font.clone());
            writing_offset = false;
        }
    }
}

fn ins_hover_ui(ui: &mut egui::Ui, ins: &ObjIns, appearance: &Appearance) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap = Some(false);

        ui.label(format!("{:02X?}", ins.code.to_be_bytes()));

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
) {
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
    write_text(
        &format!("{:<1$}", format!("{:x}: ", ins.address - symbol.address as u32), pad),
        base_color,
        &mut job,
        appearance.code_font.clone(),
    );
    if let Some(branch) = &ins_diff.branch_from {
        write_text(
            "~> ",
            appearance.diff_colors[branch.branch_idx % appearance.diff_colors.len()],
            &mut job,
            appearance.code_font.clone(),
        );
    } else {
        write_text("   ", base_color, &mut job, appearance.code_font.clone());
    }
    write_ins(ins, &ins_diff.kind, &ins_diff.arg_diff, symbol.address as u32, &mut job, appearance);
    if let Some(branch) = &ins_diff.branch_to {
        write_text(
            " ~>",
            appearance.diff_colors[branch.branch_idx % appearance.diff_colors.len()],
            &mut job,
            appearance.code_font.clone(),
        );
    }
    ui.add(Label::new(job).sense(Sense::click()))
        .on_hover_ui_at_pointer(|ui| ins_hover_ui(ui, ins, appearance))
        .context_menu(|ui| ins_context_menu(ui, ins));
}

fn asm_table_ui(
    table: TableBuilder<'_>,
    left_obj: Option<&ObjInfo>,
    right_obj: Option<&ObjInfo>,
    selected_symbol: &SymbolReference,
    appearance: &Appearance,
) -> Option<()> {
    let left_symbol = left_obj.and_then(|obj| find_symbol(obj, selected_symbol));
    let right_symbol = right_obj.and_then(|obj| find_symbol(obj, selected_symbol));
    let instructions_len = left_symbol.or(right_symbol).map(|s| s.instructions.len())?;
    table.body(|body| {
        body.rows(appearance.code_font.size, instructions_len, |row_index, mut row| {
            row.col(|ui| {
                if let Some(symbol) = left_symbol {
                    asm_row_ui(ui, &symbol.instructions[row_index], symbol, appearance);
                }
            });
            row.col(|ui| {
                if let Some(symbol) = right_symbol {
                    asm_row_ui(ui, &symbol.instructions[row_index], symbol, appearance);
                }
            });
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
    );
}
