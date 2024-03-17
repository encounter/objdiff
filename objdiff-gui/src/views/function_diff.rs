use std::default::Default;

use egui::{text::LayoutJob, Align, Label, Layout, Sense, Vec2, Widget};
use egui_extras::{Column, TableBuilder, TableRow};
use objdiff_core::{
    diff::display::{display_diff, DiffText, HighlightKind},
    obj::{
        ObjInfo, ObjIns, ObjInsArg, ObjInsArgValue, ObjInsDiff, ObjInsDiffKind, ObjSection,
        ObjSymbol,
    },
};
use time::format_description;

use crate::views::{
    appearance::Appearance,
    symbol_diff::{match_color_for_symbol, DiffViewState, SymbolReference, View},
};

#[derive(Default)]
pub struct FunctionViewState {
    pub highlight: HighlightKind,
}

fn ins_hover_ui(ui: &mut egui::Ui, section: &ObjSection, ins: &ObjIns, appearance: &Appearance) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap = Some(false);

        let offset = ins.address - section.address;
        ui.label(format!(
            "{:02X?}",
            &section.data[offset as usize..(offset + ins.size as u64) as usize]
        ));

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

fn find_symbol<'a>(
    obj: &'a ObjInfo,
    selected_symbol: &SymbolReference,
) -> Option<(&'a ObjSection, &'a ObjSymbol)> {
    obj.sections.iter().find_map(|section| {
        section.symbols.iter().find_map(|symbol| {
            (symbol.name == selected_symbol.symbol_name).then_some((section, symbol))
        })
    })
}

fn diff_text_ui(
    ui: &mut egui::Ui,
    text: DiffText<'_>,
    ins_diff: &ObjInsDiff,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
    space_width: f32,
) {
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
        DiffText::BranchDest(addr) => {
            label_text = format!("{addr:x}");
        }
        DiffText::Symbol(sym) => {
            let name = sym.demangled_name.as_ref().unwrap_or(&sym.name);
            label_text = name.clone();
            base_color = appearance.emphasized_text_color;
        }
        DiffText::Spacing(n) => {
            ui.add_space(n as f32 * space_width);
            return;
        }
        DiffText::Eol => {
            label_text = "\n".to_string();
        }
    }

    let len = label_text.len();
    let highlight = ins_view_state.highlight == text;
    let response = Label::new(LayoutJob::single_section(
        label_text,
        appearance.code_text_format(base_color, highlight),
    ))
    .sense(Sense::click())
    .ui(ui);
    response.context_menu(|ui| ins_context_menu(ui, ins_diff.ins.as_ref().unwrap()));
    if response.clicked() {
        if highlight {
            ins_view_state.highlight = HighlightKind::None;
        } else {
            ins_view_state.highlight = text.into();
        }
    }
    if len < pad_to {
        ui.add_space((pad_to - len) as f32 * space_width);
    }
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
    let space_width = ui.fonts(|f| f.glyph_width(&appearance.code_font, ' '));
    display_diff(ins_diff, symbol.address, |text| {
        diff_text_ui(ui, text, ins_diff, appearance, ins_view_state, space_width);
        Ok::<_, ()>(())
    })
    .unwrap();
}

fn asm_col_ui(
    row: &mut TableRow<'_, '_>,
    ins_diff: &ObjInsDiff,
    section: &ObjSection,
    symbol: &ObjSymbol,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
) {
    let (_, response) = row.col(|ui| {
        asm_row_ui(ui, ins_diff, symbol, appearance, ins_view_state);
    });
    if let Some(ins) = &ins_diff.ins {
        response.on_hover_ui_at_pointer(|ui| ins_hover_ui(ui, section, ins, appearance));
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
    let instructions_len = left_symbol.or(right_symbol).map(|(_, s)| s.instructions.len())?;
    table.body(|body| {
        body.rows(appearance.code_font.size, instructions_len, |mut row| {
            let row_index = row.index();
            if let Some((section, symbol)) = left_symbol {
                asm_col_ui(
                    &mut row,
                    &symbol.instructions[row_index],
                    section,
                    symbol,
                    appearance,
                    ins_view_state,
                );
            } else {
                empty_col_ui(&mut row);
            }
            if let Some((section, symbol)) = right_symbol {
                asm_col_ui(
                    &mut row,
                    &symbol.instructions[row_index],
                    section,
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

                    ui.horizontal(|ui| {
                        if ui.button("⏴ Back").clicked() {
                            state.current_view = View::SymbolDiff;
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                !state.scratch_running && state.scratch_available,
                                egui::Button::new("📲 decomp.me"),
                            )
                            .on_hover_text_at_pointer("Create a new scratch on decomp.me (beta)")
                            .on_disabled_hover_text("Scratch configuration missing")
                            .clicked()
                        {
                            state.queue_scratch = true;
                        }
                    });

                    let name = selected_symbol
                        .demangled_symbol_name
                        .as_deref()
                        .unwrap_or(&selected_symbol.symbol_name);
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
                            .and_then(|(_, symbol)| symbol.match_percent)
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
    ui.style_mut().interaction.selectable_labels = false;
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
