use std::{cmp::min, default::Default, mem::take};

use egui::{text::LayoutJob, Id, Label, RichText, Sense, Widget};
use objdiff_core::{
    diff::{ObjDataDiff, ObjDataDiffKind, ObjDiff},
    obj::ObjInfo,
};
use time::format_description;

use crate::{
    hotkeys,
    views::{
        appearance::Appearance,
        column_layout::{render_header, render_table},
        symbol_diff::{DiffViewAction, DiffViewNavigation, DiffViewState},
        write_text,
    },
};

const BYTES_PER_ROW: usize = 16;

fn find_section(obj: &ObjInfo, section_name: &str) -> Option<usize> {
    obj.sections.iter().position(|section| section.name == section_name)
}

fn data_row_ui(ui: &mut egui::Ui, address: usize, diffs: &[ObjDataDiff], appearance: &Appearance) {
    if diffs.iter().any(|d| d.kind != ObjDataDiffKind::None) {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }
    let mut job = LayoutJob::default();
    write_text(
        format!("{address:08x}: ").as_str(),
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
                text.push_str(format!("{byte:02x} ").as_str());
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
    Label::new(job).sense(Sense::click()).ui(ui);
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

#[derive(Clone, Copy)]
struct SectionDiffContext<'a> {
    obj: &'a ObjInfo,
    diff: &'a ObjDiff,
    section_index: Option<usize>,
}

impl<'a> SectionDiffContext<'a> {
    pub fn new(obj: Option<&'a (ObjInfo, ObjDiff)>, section_name: Option<&str>) -> Option<Self> {
        obj.map(|(obj, diff)| Self {
            obj,
            diff,
            section_index: section_name.and_then(|section_name| find_section(obj, section_name)),
        })
    }

    #[inline]
    pub fn has_section(&self) -> bool { self.section_index.is_some() }
}

fn data_table_ui(
    ui: &mut egui::Ui,
    available_width: f32,
    left_ctx: Option<SectionDiffContext<'_>>,
    right_ctx: Option<SectionDiffContext<'_>>,
    config: &Appearance,
) -> Option<()> {
    let left_section = left_ctx
        .and_then(|ctx| ctx.section_index.map(|i| (&ctx.obj.sections[i], &ctx.diff.sections[i])));
    let right_section = right_ctx
        .and_then(|ctx| ctx.section_index.map(|i| (&ctx.obj.sections[i], &ctx.diff.sections[i])));
    let total_bytes = left_section
        .or(right_section)?
        .1
        .data_diff
        .iter()
        .fold(0usize, |accum, item| accum + item.len);
    if total_bytes == 0 {
        return None;
    }
    let total_rows = (total_bytes - 1) / BYTES_PER_ROW + 1;

    let left_diffs = left_section.map(|(_, section)| split_diffs(&section.data_diff));
    let right_diffs = right_section.map(|(_, section)| split_diffs(&section.data_diff));

    hotkeys::check_scroll_hotkeys(ui);

    render_table(ui, available_width, 2, config.code_font.size, total_rows, |row, column| {
        let i = row.index();
        let address = i * BYTES_PER_ROW;
        row.col(|ui| {
            if column == 0 {
                if let Some(left_diffs) = &left_diffs {
                    data_row_ui(ui, address, &left_diffs[i], config);
                }
            } else if column == 1 {
                if let Some(right_diffs) = &right_diffs {
                    data_row_ui(ui, address, &right_diffs[i], config);
                }
            }
        });
    });
    Some(())
}

#[must_use]
pub fn data_diff_ui(
    ui: &mut egui::Ui,
    state: &DiffViewState,
    appearance: &Appearance,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let Some(result) = &state.build else {
        return ret;
    };

    let section_name =
        state.symbol_state.left_symbol.as_ref().and_then(|s| s.section_name.as_deref()).or_else(
            || state.symbol_state.right_symbol.as_ref().and_then(|s| s.section_name.as_deref()),
        );
    let left_ctx = SectionDiffContext::new(result.first_obj.as_ref(), section_name);
    let right_ctx = SectionDiffContext::new(result.second_obj.as_ref(), section_name);

    // If both sides are missing a symbol, switch to symbol diff view
    if !right_ctx.is_some_and(|ctx| ctx.has_section())
        && !left_ctx.is_some_and(|ctx| ctx.has_section())
    {
        return Some(DiffViewAction::Navigate(DiffViewNavigation::symbol_diff()));
    }

    // Header
    let available_width = ui.available_width();
    render_header(ui, available_width, 2, |ui, column| {
        if column == 0 {
            // Left column
            if ui.button("⏴ Back").clicked() || hotkeys::back_pressed(ui.ctx()) {
                ret = Some(DiffViewAction::Navigate(DiffViewNavigation::symbol_diff()));
            }

            if let Some(section) =
                left_ctx.and_then(|ctx| ctx.section_index.map(|i| &ctx.obj.sections[i]))
            {
                ui.label(
                    RichText::new(section.name.clone())
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
            } else {
                ui.label(
                    RichText::new("Missing")
                        .font(appearance.code_font.clone())
                        .color(appearance.replace_color),
                );
            }
        } else if column == 1 {
            // Right column
            ui.horizontal(|ui| {
                if ui.add_enabled(!state.build_running, egui::Button::new("Build")).clicked() {
                    ret = Some(DiffViewAction::Build);
                }
                ui.scope(|ui| {
                    ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                    if state.build_running {
                        ui.colored_label(appearance.replace_color, "Building…");
                    } else {
                        ui.label("Last built:");
                        let format = format_description::parse("[hour]:[minute]:[second]").unwrap();
                        ui.label(
                            result.time.to_offset(appearance.utc_offset).format(&format).unwrap(),
                        );
                    }
                });
            });

            if let Some(section) =
                right_ctx.and_then(|ctx| ctx.section_index.map(|i| &ctx.obj.sections[i]))
            {
                ui.label(
                    RichText::new(section.name.clone())
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
            } else {
                ui.label(
                    RichText::new("Missing")
                        .font(appearance.code_font.clone())
                        .color(appearance.replace_color),
                );
            }
        }
    });

    // Table
    let id =
        Id::new(state.symbol_state.left_symbol.as_ref().and_then(|s| s.section_name.as_deref()))
            .with(state.symbol_state.right_symbol.as_ref().and_then(|s| s.section_name.as_deref()));
    ui.push_id(id, |ui| {
        data_table_ui(ui, available_width, left_ctx, right_ctx, appearance);
    });
    ret
}
