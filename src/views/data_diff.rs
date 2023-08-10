use std::{cmp::min, default::Default, mem::take};

use egui::{text::LayoutJob, Align, Label, Layout, Sense, Vec2};
use egui_extras::{Column, TableBuilder};
use time::format_description;

use crate::{
    jobs::{Job, JobQueue},
    obj::{ObjDataDiff, ObjDataDiffKind, ObjInfo, ObjSection},
    views::{
        appearance::Appearance,
        symbol_diff::{DiffViewState, SymbolReference, View},
        write_text,
    },
};

const BYTES_PER_ROW: usize = 16;

fn find_section<'a>(obj: &'a ObjInfo, selected_symbol: &SymbolReference) -> Option<&'a ObjSection> {
    obj.sections.iter().find(|section| section.name == selected_symbol.section_name)
}

fn data_row_ui(ui: &mut egui::Ui, address: usize, diffs: &[ObjDataDiff], appearance: &Appearance) {
    if diffs.iter().any(|d| d.kind != ObjDataDiffKind::None) {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }
    let mut job = LayoutJob::default();
    write_text(
        format!("{address:08X}: ").as_str(),
        appearance.text_color,
        &mut job,
        appearance.code_font.clone(),
    );
    let mut cur_addr = 0usize;
    for diff in diffs {
        let base_color = match diff.kind {
            ObjDataDiffKind::None => appearance.text_color,
            ObjDataDiffKind::Replace => appearance.replace_color,
            ObjDataDiffKind::Delete => appearance.delete_color,
            ObjDataDiffKind::Insert => appearance.insert_color,
        };
        if diff.data.is_empty() {
            let mut str = "   ".repeat(diff.len);
            str.push_str(" ".repeat(diff.len / 8).as_str());
            write_text(str.as_str(), base_color, &mut job, appearance.code_font.clone());
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
            write_text(text.as_str(), base_color, &mut job, appearance.code_font.clone());
        }
    }
    if cur_addr < BYTES_PER_ROW {
        let n = BYTES_PER_ROW - cur_addr;
        let mut str = " ".to_string();
        str.push_str("   ".repeat(n).as_str());
        str.push_str(" ".repeat(n / 8).as_str());
        write_text(str.as_str(), appearance.text_color, &mut job, appearance.code_font.clone());
    }
    write_text(" ", appearance.text_color, &mut job, appearance.code_font.clone());
    for diff in diffs {
        let base_color = match diff.kind {
            ObjDataDiffKind::None => appearance.text_color,
            ObjDataDiffKind::Replace => appearance.replace_color,
            ObjDataDiffKind::Delete => appearance.delete_color,
            ObjDataDiffKind::Insert => appearance.insert_color,
        };
        if diff.data.is_empty() {
            write_text(
                " ".repeat(diff.len).as_str(),
                base_color,
                &mut job,
                appearance.code_font.clone(),
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
            write_text(text.as_str(), base_color, &mut job, appearance.code_font.clone());
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
    config: &Appearance,
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

pub fn data_diff_ui(
    ui: &mut egui::Ui,
    jobs: &JobQueue,
    state: &mut DiffViewState,
    appearance: &Appearance,
) -> bool {
    let mut rebuild = false;
    let (Some(result), Some(selected_symbol)) = (&state.build, &state.selected_symbol) else {
        return rebuild;
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

                    ui.scope(|ui| {
                        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                        ui.style_mut().wrap = Some(false);
                        ui.colored_label(appearance.highlight_color, &selected_symbol.symbol_name);
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
                        if ui.button("Build").clicked() {
                            rebuild = true;
                        }
                        ui.scope(|ui| {
                            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                            ui.style_mut().wrap = Some(false);
                            if jobs.is_running(Job::ObjDiff) {
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
                        ui.style_mut().wrap = Some(false);
                        ui.label("");
                        ui.label("Diff base:");
                    });
                },
            );
        },
    );
    ui.separator();

    // Table
    if let (Some(left_obj), Some(right_obj)) = (&result.first_obj, &result.second_obj) {
        let available_height = ui.available_height();
        let table = TableBuilder::new(ui)
            .striped(false)
            .cell_layout(Layout::left_to_right(Align::Min))
            .columns(Column::exact(column_width).clip(true), 2)
            .resizable(false)
            .auto_shrink([false, false])
            .min_scrolled_height(available_height);
        data_table_ui(table, left_obj, right_obj, selected_symbol, appearance);
    }

    rebuild
}
