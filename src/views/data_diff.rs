use std::{cmp::min, default::Default, mem::take};

use egui::{text::LayoutJob, Color32, Label, Sense};
use egui_extras::{Size, StripBuilder, TableBuilder};
use time::format_description;

use crate::{
    app::{SymbolReference, View, ViewConfig, ViewState},
    jobs::Job,
    obj::{ObjDataDiff, ObjDataDiffKind, ObjInfo, ObjSection},
    views::{write_text, COLOR_RED},
};

const BYTES_PER_ROW: usize = 16;

fn find_section<'a>(obj: &'a ObjInfo, selected_symbol: &SymbolReference) -> Option<&'a ObjSection> {
    obj.sections.iter().find(|section| {
        section.symbols.iter().any(|symbol| symbol.name == selected_symbol.symbol_name)
    })
}

fn data_row_ui(ui: &mut egui::Ui, address: usize, diffs: &[ObjDataDiff], config: &ViewConfig) {
    if diffs.iter().any(|d| d.kind != ObjDataDiffKind::None) {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }
    let mut job = LayoutJob::default();
    write_text(
        format!("{address:08X}: ").as_str(),
        Color32::GRAY,
        &mut job,
        config.code_font.clone(),
    );
    let mut cur_addr = 0usize;
    for diff in diffs {
        let base_color = match diff.kind {
            ObjDataDiffKind::None => Color32::GRAY,
            ObjDataDiffKind::Replace => Color32::LIGHT_BLUE,
            ObjDataDiffKind::Delete => COLOR_RED,
            ObjDataDiffKind::Insert => Color32::GREEN,
        };
        if diff.data.is_empty() {
            let mut str = "   ".repeat(diff.len);
            str.push_str(" ".repeat(diff.len / 8).as_str());
            write_text(str.as_str(), base_color, &mut job, config.code_font.clone());
            cur_addr += diff.len;
        } else {
            let mut text = String::new();
            for byte in &diff.data {
                text.push_str(format!("{byte:02X} ").as_str());
                cur_addr += 1;
                if cur_addr % 8 == 0 {
                    text.push(' ');
                }
            }
            write_text(text.as_str(), base_color, &mut job, config.code_font.clone());
        }
    }
    if cur_addr < BYTES_PER_ROW {
        let n = BYTES_PER_ROW - cur_addr;
        let mut str = " ".to_string();
        str.push_str("   ".repeat(n).as_str());
        str.push_str(" ".repeat(n / 8).as_str());
        write_text(str.as_str(), Color32::GRAY, &mut job, config.code_font.clone());
    }
    write_text(" ", Color32::GRAY, &mut job, config.code_font.clone());
    for diff in diffs {
        let base_color = match diff.kind {
            ObjDataDiffKind::None => Color32::GRAY,
            ObjDataDiffKind::Replace => Color32::LIGHT_BLUE,
            ObjDataDiffKind::Delete => COLOR_RED,
            ObjDataDiffKind::Insert => Color32::GREEN,
        };
        if diff.data.is_empty() {
            write_text(
                " ".repeat(diff.len).as_str(),
                base_color,
                &mut job,
                config.code_font.clone(),
            );
        } else {
            let mut text = String::new();
            for byte in &diff.data {
                let c = char::from(*byte);
                if c.is_ascii() && !c.is_ascii_control() {
                    text.push(c);
                } else {
                    text.push('.');
                }
            }
            write_text(text.as_str(), base_color, &mut job, config.code_font.clone());
        }
    }
    ui.add(Label::new(job).sense(Sense::click()));
    //     .on_hover_ui_at_pointer(|ui| ins_hover_ui(ui, ins))
    //     .context_menu(|ui| ins_context_menu(ui, ins));
}

fn split_diffs(diffs: &[ObjDataDiff]) -> Vec<Vec<ObjDataDiff>> {
    let mut split_diffs = Vec::<Vec<ObjDataDiff>>::new();
    let mut row_diffs = Vec::<ObjDataDiff>::new();
    let mut cur_addr = 0usize;
    for diff in diffs {
        let mut cur_len = 0usize;
        while cur_len < diff.len {
            let remaining_len = diff.len - cur_len;
            let mut remaining_in_row = BYTES_PER_ROW - (cur_addr % BYTES_PER_ROW);
            let len = min(remaining_len, remaining_in_row);
            row_diffs.push(ObjDataDiff {
                data: if diff.data.is_empty() {
                    Vec::new()
                } else {
                    diff.data[cur_len..cur_len + len].to_vec()
                },
                kind: diff.kind,
                len,
                // TODO
                symbol: String::new(),
            });
            remaining_in_row -= len;
            cur_len += len;
            cur_addr += len;
            if remaining_in_row == 0 {
                split_diffs.push(take(&mut row_diffs));
            }
        }
    }
    if !row_diffs.is_empty() {
        split_diffs.push(take(&mut row_diffs));
    }
    split_diffs
}

fn data_table_ui(
    table: TableBuilder<'_>,
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    selected_symbol: &SymbolReference,
    config: &ViewConfig,
) -> Option<()> {
    let left_section = find_section(left_obj, selected_symbol)?;
    let right_section = find_section(right_obj, selected_symbol)?;

    let total_bytes = left_section.data_diff.iter().fold(0usize, |accum, item| accum + item.len);
    if total_bytes == 0 {
        return None;
    }
    let total_rows = (total_bytes - 1) / BYTES_PER_ROW + 1;

    let left_diffs = split_diffs(&left_section.data_diff);
    let right_diffs = split_diffs(&right_section.data_diff);

    table.body(|body| {
        body.rows(config.code_font.size, total_rows, |row_index, mut row| {
            let address = row_index * BYTES_PER_ROW;
            row.col(|ui| {
                data_row_ui(ui, address, &left_diffs[row_index], config);
            });
            row.col(|ui| {
                data_row_ui(ui, address, &right_diffs[row_index], config);
            });
        });
    });
    Some(())
}

pub fn data_diff_ui(ui: &mut egui::Ui, view_state: &mut ViewState) -> bool {
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
                    strip.cell(|ui| {
                        ui.scope(|ui| {
                            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                            ui.style_mut().wrap = Some(false);
                            ui.colored_label(Color32::WHITE, &selected_symbol.symbol_name);
                            ui.label("Diff target:");
                            ui.separator();
                        });
                    });
                    strip.cell(|ui| {
                        ui.scope(|ui| {
                            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                            ui.style_mut().wrap = Some(false);
                            ui.label("");
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
                    data_table_ui(
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
