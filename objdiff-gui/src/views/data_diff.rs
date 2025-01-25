use std::{
    cmp::{min, Ordering},
    default::Default,
    mem::take,
};

use egui::{text::LayoutJob, Id, Label, RichText, Sense, Widget};
use objdiff_core::{
    diff::{ObjDataDiff, ObjDataDiffKind, ObjDataRelocDiff, ObjDiff},
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

fn data_row_hover_ui(
    ui: &mut egui::Ui,
    obj: &ObjInfo,
    diffs: &[(ObjDataDiff, Vec<ObjDataRelocDiff>)],
    appearance: &Appearance,
) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        let reloc_diffs = diffs.iter().flat_map(|(_, reloc_diffs)| reloc_diffs);
        let mut prev_reloc = None;
        for reloc_diff in reloc_diffs {
            let reloc = &reloc_diff.reloc;
            if prev_reloc == Some(reloc) {
                // Avoid showing consecutive duplicate relocations.
                // We do this because a single relocation can span across multiple diffs if the
                // bytes in the relocation changed (e.g. first byte is added, second is unchanged).
                continue;
            }
            prev_reloc = Some(reloc);

            let color = get_color_for_diff_kind(reloc_diff.kind, appearance);

            // TODO: Most of this code is copy-pasted from ins_hover_ui.
            // Try to separate this out into a shared function.
            ui.label(format!("Relocation type: {}", obj.arch.display_reloc(reloc.flags)));
            ui.label(format!("Relocation address: {:x}", reloc.address));
            let addend_str = match reloc.addend.cmp(&0i64) {
                Ordering::Greater => format!("+{:x}", reloc.addend),
                Ordering::Less => format!("-{:x}", -reloc.addend),
                _ => "".to_string(),
            };
            ui.colored_label(color, format!("Name: {}{}", reloc.target.name, addend_str));
            if let Some(orig_section_index) = reloc.target.orig_section_index {
                if let Some(section) =
                    obj.sections.iter().find(|s| s.orig_index == orig_section_index)
                {
                    ui.colored_label(color, format!("Section: {}", section.name));
                }
                ui.colored_label(
                    color,
                    format!("Address: {:x}{}", reloc.target.address, addend_str),
                );
                ui.colored_label(color, format!("Size: {:x}", reloc.target.size));
                if reloc.addend >= 0 && reloc.target.bytes.len() > reloc.addend as usize {}
            } else {
                ui.colored_label(color, "Extern".to_string());
            }
        }
    });
}

fn data_row_context_menu(ui: &mut egui::Ui, diffs: &[(ObjDataDiff, Vec<ObjDataRelocDiff>)]) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        let reloc_diffs = diffs.iter().flat_map(|(_, reloc_diffs)| reloc_diffs);
        let mut prev_reloc = None;
        for reloc_diff in reloc_diffs {
            let reloc = &reloc_diff.reloc;
            if prev_reloc == Some(reloc) {
                // Avoid showing consecutive duplicate relocations.
                // We do this because a single relocation can span across multiple diffs if the
                // bytes in the relocation changed (e.g. first byte is added, second is unchanged).
                continue;
            }
            prev_reloc = Some(reloc);

            // TODO: This code is copy-pasted from ins_context_menu.
            // Try to separate this out into a shared function.
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

fn get_color_for_diff_kind(diff_kind: ObjDataDiffKind, appearance: &Appearance) -> egui::Color32 {
    match diff_kind {
        ObjDataDiffKind::None => appearance.text_color,
        ObjDataDiffKind::Replace => appearance.replace_color,
        ObjDataDiffKind::Delete => appearance.delete_color,
        ObjDataDiffKind::Insert => appearance.insert_color,
    }
}

fn data_row_ui(
    ui: &mut egui::Ui,
    obj: Option<&ObjInfo>,
    address: usize,
    diffs: &[(ObjDataDiff, Vec<ObjDataRelocDiff>)],
    appearance: &Appearance,
) {
    if diffs.iter().any(|(dd, rds)| {
        dd.kind != ObjDataDiffKind::None || rds.iter().any(|rd| rd.kind != ObjDataDiffKind::None)
    }) {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }
    let mut job = LayoutJob::default();
    write_text(
        format!("{address:08x}: ").as_str(),
        appearance.text_color,
        &mut job,
        appearance.code_font.clone(),
    );
    // The offset shown on the side of the GUI, shifted by insertions/deletions.
    let mut cur_addr = 0usize;
    // The offset into the actual bytes of the section on this side, ignoring differences.
    let mut cur_addr_actual = address;
    for (diff, reloc_diffs) in diffs {
        let base_color = get_color_for_diff_kind(diff.kind, appearance);
        if diff.data.is_empty() {
            let mut str = "   ".repeat(diff.len);
            let n1 = cur_addr / 8;
            let n2 = (diff.len + cur_addr) / 8;
            str.push_str(" ".repeat(n2 - n1).as_str());
            write_text(str.as_str(), base_color, &mut job, appearance.code_font.clone());
            cur_addr += diff.len;
        } else {
            for byte in &diff.data {
                let mut byte_color = base_color;
                if let Some(reloc_diff) = reloc_diffs.iter().find(|reloc_diff| {
                    reloc_diff.kind != ObjDataDiffKind::None
                        && reloc_diff.range.contains(&cur_addr_actual)
                }) {
                    byte_color = get_color_for_diff_kind(reloc_diff.kind, appearance);
                }
                let byte_text = format!("{byte:02x} ");
                write_text(byte_text.as_str(), byte_color, &mut job, appearance.code_font.clone());
                cur_addr += 1;
                cur_addr_actual += 1;
                if cur_addr % 8 == 0 {
                    write_text(" ", base_color, &mut job, appearance.code_font.clone());
                }
            }
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
    for (diff, _) in diffs {
        let base_color = get_color_for_diff_kind(diff.kind, appearance);
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

    let response = Label::new(job).sense(Sense::click()).ui(ui);
    if let Some(obj) = obj {
        response
            .on_hover_ui_at_pointer(|ui| data_row_hover_ui(ui, obj, diffs, appearance))
            .context_menu(|ui| data_row_context_menu(ui, diffs));
    }
}

fn split_diffs(
    diffs: &[ObjDataDiff],
    reloc_diffs: &[ObjDataRelocDiff],
) -> Vec<Vec<(ObjDataDiff, Vec<ObjDataRelocDiff>)>> {
    let mut split_diffs = Vec::<Vec<(ObjDataDiff, Vec<ObjDataRelocDiff>)>>::new();
    let mut row_diffs = Vec::<(ObjDataDiff, Vec<ObjDataRelocDiff>)>::new();
    // The offset shown on the side of the GUI, shifted by insertions/deletions.
    let mut cur_addr = 0usize;
    // The offset into the actual bytes of the section on this side, ignoring differences.
    let mut cur_addr_actual = 0usize;
    for diff in diffs {
        let mut cur_len = 0usize;
        while cur_len < diff.len {
            let remaining_len = diff.len - cur_len;
            let mut remaining_in_row = BYTES_PER_ROW - (cur_addr % BYTES_PER_ROW);
            let len = min(remaining_len, remaining_in_row);

            let data_diff = ObjDataDiff {
                data: if diff.data.is_empty() {
                    Vec::new()
                } else {
                    diff.data[cur_len..cur_len + len].to_vec()
                },
                kind: diff.kind,
                len,
                symbol: String::new(), // TODO
            };
            let row_reloc_diffs: Vec<ObjDataRelocDiff> = if diff.data.is_empty() {
                Vec::new()
            } else {
                let diff_range = cur_addr_actual + cur_len..cur_addr_actual + cur_len + len;
                reloc_diffs
                    .iter()
                    .filter_map(|reloc_diff| {
                        if reloc_diff.range.start < diff_range.end
                            && diff_range.start < reloc_diff.range.end
                        {
                            Some(reloc_diff.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            let row_diff = (data_diff, row_reloc_diffs);

            row_diffs.push(row_diff);
            remaining_in_row -= len;
            cur_len += len;
            cur_addr += len;
            if remaining_in_row == 0 {
                split_diffs.push(take(&mut row_diffs));
            }
        }
        cur_addr_actual += diff.data.len();
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
    let left_obj = left_ctx.map(|ctx| ctx.obj);
    let right_obj = right_ctx.map(|ctx| ctx.obj);
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

    let left_diffs =
        left_section.map(|(_, section)| split_diffs(&section.data_diff, &section.reloc_diff));
    let right_diffs =
        right_section.map(|(_, section)| split_diffs(&section.data_diff, &section.reloc_diff));

    hotkeys::check_scroll_hotkeys(ui, true);

    render_table(ui, available_width, 2, config.code_font.size, total_rows, |row, column| {
        let i = row.index();
        let address = i * BYTES_PER_ROW;
        row.col(|ui| {
            if column == 0 {
                if let Some(left_diffs) = &left_diffs {
                    data_row_ui(ui, left_obj, address, &left_diffs[i], config);
                }
            } else if column == 1 {
                if let Some(right_diffs) = &right_diffs {
                    data_row_ui(ui, right_obj, address, &right_diffs[i], config);
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
