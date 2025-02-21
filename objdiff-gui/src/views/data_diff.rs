use std::{cmp::min, default::Default, mem::take};

use egui::{text::LayoutJob, Label, Sense, Widget};
use objdiff_core::{
    diff::{DataDiff, DataDiffKind, DataRelocationDiff},
    obj::Object,
};

use crate::views::{appearance::Appearance, write_text};

pub(crate) const BYTES_PER_ROW: usize = 16;

fn data_row_hover_ui(
    ui: &mut egui::Ui,
    _obj: &Object,
    _diffs: &[(DataDiff, Vec<DataRelocationDiff>)],
    _appearance: &Appearance,
) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        // TODO
        // let reloc_diffs = diffs.iter().flat_map(|(_, reloc_diffs)| reloc_diffs);
        // let mut prev_reloc = None;
        // for reloc_diff in reloc_diffs {
        //     let reloc = &reloc_diff.reloc;
        //     if prev_reloc == Some(reloc) {
        //         // Avoid showing consecutive duplicate relocations.
        //         // We do this because a single relocation can span across multiple diffs if the
        //         // bytes in the relocation changed (e.g. first byte is added, second is unchanged).
        //         continue;
        //     }
        //     prev_reloc = Some(reloc);
        //
        //     let color = get_color_for_diff_kind(reloc_diff.kind, appearance);
        //
        //     // TODO: Most of this code is copy-pasted from ins_hover_ui.
        //     // Try to separate this out into a shared function.
        //     ui.label(format!("Relocation type: {}", obj.arch.display_reloc(reloc.flags)));
        //     ui.label(format!("Relocation address: {:x}", reloc.address));
        //     let addend_str = match reloc.addend.cmp(&0i64) {
        //         Ordering::Greater => format!("+{:x}", reloc.addend),
        //         Ordering::Less => format!("-{:x}", -reloc.addend),
        //         _ => "".to_string(),
        //     };
        //     ui.colored_label(color, format!("Name: {}{}", reloc.target.name, addend_str));
        //     if let Some(orig_section_index) = reloc.target.orig_section_index {
        //         if let Some(section) =
        //             obj.sections.iter().find(|s| s.orig_index == orig_section_index)
        //         {
        //             ui.colored_label(color, format!("Section: {}", section.name));
        //         }
        //         ui.colored_label(
        //             color,
        //             format!("Address: {:x}{}", reloc.target.address, addend_str),
        //         );
        //         ui.colored_label(color, format!("Size: {:x}", reloc.target.size));
        //         if reloc.addend >= 0 && reloc.target.bytes.len() > reloc.addend as usize {}
        //     } else {
        //         ui.colored_label(color, "Extern".to_string());
        //     }
        // }
    });
}

fn data_row_context_menu(ui: &mut egui::Ui, _diffs: &[(DataDiff, Vec<DataRelocationDiff>)]) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        // TODO
        // let reloc_diffs = diffs.iter().flat_map(|(_, reloc_diffs)| reloc_diffs);
        // let mut prev_reloc = None;
        // for reloc_diff in reloc_diffs {
        //     let reloc = &reloc_diff.reloc;
        //     if prev_reloc == Some(reloc) {
        //         // Avoid showing consecutive duplicate relocations.
        //         // We do this because a single relocation can span across multiple diffs if the
        //         // bytes in the relocation changed (e.g. first byte is added, second is unchanged).
        //         continue;
        //     }
        //     prev_reloc = Some(reloc);
        //
        //     // TODO: This code is copy-pasted from ins_context_menu.
        //     // Try to separate this out into a shared function.
        //     if let Some(name) = &reloc.target.demangled_name {
        //         if ui.button(format!("Copy \"{name}\"")).clicked() {
        //             ui.output_mut(|output| output.copied_text.clone_from(name));
        //             ui.close_menu();
        //         }
        //     }
        //     if ui.button(format!("Copy \"{}\"", reloc.target.name)).clicked() {
        //         ui.output_mut(|output| output.copied_text.clone_from(&reloc.target.name));
        //         ui.close_menu();
        //     }
        // }
    });
}

fn get_color_for_diff_kind(diff_kind: DataDiffKind, appearance: &Appearance) -> egui::Color32 {
    match diff_kind {
        DataDiffKind::None => appearance.text_color,
        DataDiffKind::Replace => appearance.replace_color,
        DataDiffKind::Delete => appearance.delete_color,
        DataDiffKind::Insert => appearance.insert_color,
    }
}

pub(crate) fn data_row_ui(
    ui: &mut egui::Ui,
    obj: Option<&Object>,
    address: usize,
    diffs: &[(DataDiff, Vec<DataRelocationDiff>)],
    appearance: &Appearance,
) {
    if diffs.iter().any(|(dd, rds)| {
        dd.kind != DataDiffKind::None || rds.iter().any(|rd| rd.kind != DataDiffKind::None)
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
                    reloc_diff.kind != DataDiffKind::None
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

pub(crate) fn split_diffs(
    diffs: &[DataDiff],
    reloc_diffs: &[DataRelocationDiff],
) -> Vec<Vec<(DataDiff, Vec<DataRelocationDiff>)>> {
    let mut split_diffs = Vec::<Vec<(DataDiff, Vec<DataRelocationDiff>)>>::new();
    let mut row_diffs = Vec::<(DataDiff, Vec<DataRelocationDiff>)>::new();
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

            let data_diff = DataDiff {
                data: if diff.data.is_empty() {
                    Vec::new()
                } else {
                    diff.data[cur_len..cur_len + len].to_vec()
                },
                kind: diff.kind,
                len,
                symbol: String::new(), // TODO
            };
            let row_reloc_diffs: Vec<DataRelocationDiff> = if diff.data.is_empty() {
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
