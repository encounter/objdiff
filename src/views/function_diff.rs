use std::default::Default;

use cwdemangle::demangle;
use egui::{text::LayoutJob, Color32, FontId, Label, Sense};
use egui_extras::{Size, StripBuilder, TableBuilder};
use ppc750cl::Argument;
use time::format_description;

use crate::{
    app::{SymbolReference, View, ViewConfig, ViewState},
    jobs::Job,
    obj::{
        ObjInfo, ObjIns, ObjInsArg, ObjInsArgDiff, ObjInsDiff, ObjInsDiffKind, ObjReloc,
        ObjRelocKind, ObjSymbol,
    },
    views::{symbol_diff::match_color_for_symbol, write_text, COLOR_RED},
};

fn write_reloc_name(reloc: &ObjReloc, color: Color32, job: &mut LayoutJob, font_id: FontId) {
    let name = reloc.target.demangled_name.as_ref().unwrap_or(&reloc.target.name);
    write_text(name, Color32::LIGHT_GRAY, job, font_id.clone());
    if reloc.target.addend != 0 {
        write_text(&format!("+{:X}", reloc.target.addend), color, job, font_id);
    }
}

fn write_reloc(reloc: &ObjReloc, color: Color32, job: &mut LayoutJob, font_id: FontId) {
    match reloc.kind {
        ObjRelocKind::PpcAddr16Lo => {
            write_reloc_name(reloc, color, job, font_id.clone());
            write_text("@l", color, job, font_id);
        }
        ObjRelocKind::PpcAddr16Hi => {
            write_reloc_name(reloc, color, job, font_id.clone());
            write_text("@h", color, job, font_id);
        }
        ObjRelocKind::PpcAddr16Ha => {
            write_reloc_name(reloc, color, job, font_id.clone());
            write_text("@ha", color, job, font_id);
        }
        ObjRelocKind::PpcEmbSda21 => {
            write_reloc_name(reloc, color, job, font_id.clone());
            write_text("@sda21", color, job, font_id);
        }
        ObjRelocKind::MipsHi16 => {
            write_text("%hi(", color, job, font_id.clone());
            write_reloc_name(reloc, color, job, font_id.clone());
            write_text(")", color, job, font_id);
        }
        ObjRelocKind::MipsLo16 => {
            write_text("%lo(", color, job, font_id.clone());
            write_reloc_name(reloc, color, job, font_id.clone());
            write_text(")", color, job, font_id);
        }
        ObjRelocKind::Absolute
        | ObjRelocKind::PpcRel24
        | ObjRelocKind::PpcRel14
        | ObjRelocKind::Mips26 => {
            write_reloc_name(reloc, color, job, font_id);
        }
    };
}

fn write_ins(
    ins: &ObjIns,
    diff_kind: &ObjInsDiffKind,
    args: &[Option<ObjInsArgDiff>],
    base_addr: u32,
    job: &mut LayoutJob,
    config: &ViewConfig,
) {
    let base_color = match diff_kind {
        ObjInsDiffKind::None | ObjInsDiffKind::OpMismatch | ObjInsDiffKind::ArgMismatch => {
            Color32::GRAY
        }
        ObjInsDiffKind::Replace => Color32::LIGHT_BLUE,
        ObjInsDiffKind::Delete => COLOR_RED,
        ObjInsDiffKind::Insert => Color32::GREEN,
    };
    write_text(
        &format!("{:<11}", ins.mnemonic),
        match diff_kind {
            ObjInsDiffKind::OpMismatch => Color32::LIGHT_BLUE,
            _ => base_color,
        },
        job,
        config.code_font.clone(),
    );
    let mut writing_offset = false;
    for (i, arg) in ins.args.iter().enumerate() {
        if i == 0 {
            write_text(" ", base_color, job, config.code_font.clone());
        }
        if i > 0 && !writing_offset {
            write_text(", ", base_color, job, config.code_font.clone());
        }
        let color = if let Some(diff) = args.get(i).and_then(|a| a.as_ref()) {
            config.diff_colors[diff.idx % config.diff_colors.len()]
        } else {
            base_color
        };
        match arg {
            ObjInsArg::PpcArg(arg) => match arg {
                Argument::Offset(val) => {
                    write_text(&format!("{val}"), color, job, config.code_font.clone());
                    write_text("(", base_color, job, config.code_font.clone());
                    writing_offset = true;
                    continue;
                }
                Argument::Uimm(_) | Argument::Simm(_) => {
                    write_text(&format!("{arg}"), color, job, config.code_font.clone());
                }
                _ => {
                    write_text(&format!("{arg}"), color, job, config.code_font.clone());
                }
            },
            ObjInsArg::Reloc => {
                write_reloc(ins.reloc.as_ref().unwrap(), base_color, job, config.code_font.clone());
            }
            ObjInsArg::RelocWithBase => {
                write_reloc(ins.reloc.as_ref().unwrap(), base_color, job, config.code_font.clone());
                write_text("(", base_color, job, config.code_font.clone());
                writing_offset = true;
                continue;
            }
            ObjInsArg::MipsArg(str) => {
                write_text(
                    str.strip_prefix('$').unwrap_or(str),
                    color,
                    job,
                    config.code_font.clone(),
                );
            }
            ObjInsArg::BranchOffset(offset) => {
                let addr = offset + ins.address as i32 - base_addr as i32;
                write_text(&format!("{addr:x}"), color, job, config.code_font.clone());
            }
        }
        if writing_offset {
            write_text(")", base_color, job, config.code_font.clone());
            writing_offset = false;
        }
    }
}

fn ins_hover_ui(ui: &mut egui::Ui, ins: &ObjIns) {
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
            ui.colored_label(Color32::WHITE, format!("Name: {}", reloc.target.name));
            if let Some(section) = &reloc.target_section {
                ui.colored_label(Color32::WHITE, format!("Section: {section}"));
                ui.colored_label(Color32::WHITE, format!("Address: {:x}", reloc.target.address));
                ui.colored_label(Color32::WHITE, format!("Size: {:x}", reloc.target.size));
            } else {
                ui.colored_label(Color32::WHITE, "Extern".to_string());
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
                            ui.output().copied_text = format!("{v}");
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{}\"", v.0)).clicked() {
                            ui.output().copied_text = format!("{}", v.0);
                            ui.close_menu();
                        }
                    }
                    Argument::Simm(v) => {
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output().copied_text = format!("{v}");
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{}\"", v.0)).clicked() {
                            ui.output().copied_text = format!("{}", v.0);
                            ui.close_menu();
                        }
                    }
                    Argument::Offset(v) => {
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output().copied_text = format!("{v}");
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{}\"", v.0)).clicked() {
                            ui.output().copied_text = format!("{}", v.0);
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
                    ui.output().copied_text = name.clone();
                    ui.close_menu();
                }
            }
            if ui.button(format!("Copy \"{}\"", reloc.target.name)).clicked() {
                ui.output().copied_text = reloc.target.name.clone();
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

fn asm_row_ui(ui: &mut egui::Ui, ins_diff: &ObjInsDiff, symbol: &ObjSymbol, config: &ViewConfig) {
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
            Color32::GRAY
        }
        ObjInsDiffKind::Replace => Color32::LIGHT_BLUE,
        ObjInsDiffKind::Delete => COLOR_RED,
        ObjInsDiffKind::Insert => Color32::GREEN,
    };
    write_text(
        &format!("{:<6}", format!("{:x}:", ins.address - symbol.address as u32)),
        base_color,
        &mut job,
        config.code_font.clone(),
    );
    if let Some(branch) = &ins_diff.branch_from {
        write_text(
            "~> ",
            config.diff_colors[branch.branch_idx % config.diff_colors.len()],
            &mut job,
            config.code_font.clone(),
        );
    } else {
        write_text("   ", base_color, &mut job, config.code_font.clone());
    }
    write_ins(ins, &ins_diff.kind, &ins_diff.arg_diff, symbol.address as u32, &mut job, config);
    if let Some(branch) = &ins_diff.branch_to {
        write_text(
            " ~>",
            config.diff_colors[branch.branch_idx % config.diff_colors.len()],
            &mut job,
            config.code_font.clone(),
        );
    }
    ui.add(Label::new(job).sense(Sense::click()))
        .on_hover_ui_at_pointer(|ui| ins_hover_ui(ui, ins))
        .context_menu(|ui| ins_context_menu(ui, ins));
}

fn asm_table_ui(
    table: TableBuilder<'_>,
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    selected_symbol: &SymbolReference,
    config: &ViewConfig,
) -> Option<()> {
    let left_symbol = find_symbol(left_obj, selected_symbol);
    let right_symbol = find_symbol(right_obj, selected_symbol);
    let instructions_len = left_symbol.or(right_symbol).map(|s| s.instructions.len())?;
    table.body(|body| {
        body.rows(config.code_font.size, instructions_len, |row_index, mut row| {
            row.col(|ui| {
                if let Some(symbol) = left_symbol {
                    asm_row_ui(ui, &symbol.instructions[row_index], symbol, config);
                }
            });
            row.col(|ui| {
                if let Some(symbol) = right_symbol {
                    asm_row_ui(ui, &symbol.instructions[row_index], symbol, config);
                }
            });
        });
    });
    Some(())
}

pub fn function_diff_ui(ui: &mut egui::Ui, view_state: &mut ViewState) -> bool {
    let mut rebuild = false;
    let (Some(result), Some(selected_symbol)) = (&view_state.build, &view_state.selected_symbol) else {
        return rebuild;
    };
    StripBuilder::new(ui)
        .size(Size::exact(20.0))
        .size(Size::exact(40.0))
        .size(Size::remainder())
        .vertical(|mut strip| {
            strip.strip(|builder| {
                builder.sizes(Size::remainder(), 2).horizontal(|mut strip| {
                    strip.cell(|ui| {
                        ui.horizontal(|ui| {
                            if ui.button("Back").clicked() {
                                view_state.current_view = View::SymbolDiff;
                            }
                        });
                    });
                    strip.cell(|ui| {
                        ui.horizontal(|ui| {
                            if ui.button("Build").clicked() {
                                rebuild = true;
                            }
                            ui.scope(|ui| {
                                ui.style_mut().override_text_style =
                                    Some(egui::TextStyle::Monospace);
                                ui.style_mut().wrap = Some(false);
                                if view_state.jobs.iter().any(|job| job.job_type == Job::ObjDiff) {
                                    ui.label("Building...");
                                } else {
                                    ui.label("Last built:");
                                    let format =
                                        format_description::parse("[hour]:[minute]:[second]")
                                            .unwrap();
                                    ui.label(
                                        result
                                            .time
                                            .to_offset(view_state.utc_offset)
                                            .format(&format)
                                            .unwrap(),
                                    );
                                }
                            });
                        });
                    });
                });
            });
            strip.strip(|builder| {
                builder.sizes(Size::remainder(), 2).horizontal(|mut strip| {
                    let demangled = demangle(&selected_symbol.symbol_name, &Default::default());
                    strip.cell(|ui| {
                        ui.scope(|ui| {
                            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                            ui.style_mut().wrap = Some(false);
                            ui.colored_label(
                                Color32::WHITE,
                                demangled.as_ref().unwrap_or(&selected_symbol.symbol_name),
                            );
                            ui.label("Diff target:");
                            ui.separator();
                        });
                    });
                    strip.cell(|ui| {
                        ui.scope(|ui| {
                            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                            ui.style_mut().wrap = Some(false);
                            if let Some(match_percent) = result
                                .second_obj
                                .as_ref()
                                .and_then(|obj| find_symbol(obj, selected_symbol))
                                .and_then(|symbol| symbol.match_percent)
                            {
                                ui.colored_label(
                                    match_color_for_symbol(match_percent),
                                    &format!("{match_percent:.0}%"),
                                );
                            }
                            ui.label("Diff base:");
                            ui.separator();
                        });
                    });
                });
            });
            strip.cell(|ui| {
                if let (Some(left_obj), Some(right_obj)) = (&result.first_obj, &result.second_obj) {
                    let table = TableBuilder::new(ui)
                        .striped(false)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Min))
                        .column(Size::relative(0.5))
                        .column(Size::relative(0.5))
                        .resizable(false);
                    asm_table_ui(
                        table,
                        left_obj,
                        right_obj,
                        selected_symbol,
                        &view_state.view_config,
                    );
                }
            });
        });
    rebuild
}
