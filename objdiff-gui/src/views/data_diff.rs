use std::default::Default;

use egui::{Label, Sense, Widget, text::LayoutJob};
use objdiff_core::{
    diff::{
        DataDiffKind, DataDiffRow,
        data::BYTES_PER_ROW,
        display::{data_row_context, data_row_hover},
    },
    obj::Object,
};

use super::diff::{context_menu_items_ui, hover_items_ui};
use crate::views::{appearance::Appearance, write_text};

fn data_row_hover_ui(
    ui: &mut egui::Ui,
    obj: &Object,
    diff_row: &DataDiffRow,
    appearance: &Appearance,
) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        hover_items_ui(ui, data_row_hover(obj, diff_row), appearance);
    });
}

fn data_row_context_menu(
    ui: &mut egui::Ui,
    obj: &Object,
    diff_row: &DataDiffRow,
    column: usize,
    appearance: &Appearance,
) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        context_menu_items_ui(ui, data_row_context(obj, diff_row), column, appearance);
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
    base_address: u64,
    row_address: u64,
    diff_row: &DataDiffRow,
    appearance: &Appearance,
    column: usize,
) {
    if diff_row.segments.iter().any(|dd| dd.kind != DataDiffKind::None)
        || diff_row.relocations.iter().any(|rd| rd.kind != DataDiffKind::None)
    {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }
    let mut job = LayoutJob::default();
    write_text(
        format!("{row_address:08x}: ").as_str(),
        appearance.text_color,
        &mut job,
        appearance.code_font.clone(),
    );
    // The offset shown on the side of the GUI, shifted by insertions/deletions.
    let mut cur_addr = 0usize;
    // The offset into the actual bytes of the section on this side, ignoring differences.
    let mut cur_addr_actual = base_address + row_address;
    for diff in diff_row.segments.iter() {
        let base_color = get_color_for_diff_kind(diff.kind, appearance);
        if diff.data.is_empty() {
            let mut str = "   ".repeat(diff.size);
            let n1 = cur_addr / 8;
            let n2 = (diff.size + cur_addr) / 8;
            str.push_str(" ".repeat(n2 - n1).as_str());
            write_text(str.as_str(), base_color, &mut job, appearance.code_font.clone());
            cur_addr += diff.size;
        } else {
            for byte in &diff.data {
                let mut byte_text = format!("{byte:02x} ");
                let mut byte_color = base_color;
                if let Some(reloc_diff) = diff_row
                    .relocations
                    .iter()
                    .find(|reloc_diff| reloc_diff.range.contains(&cur_addr_actual))
                {
                    if *byte == 0 {
                        // Display 00 data bytes with a relocation as ?? instead.
                        byte_text = "?? ".to_string();
                    }
                    if reloc_diff.kind != DataDiffKind::None {
                        byte_color = get_color_for_diff_kind(reloc_diff.kind, appearance);
                    }
                }
                write_text(byte_text.as_str(), byte_color, &mut job, appearance.code_font.clone());
                cur_addr += 1;
                cur_addr_actual += 1;
                if cur_addr.is_multiple_of(8) {
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
    for diff in diff_row.segments.iter() {
        let base_color = get_color_for_diff_kind(diff.kind, appearance);
        if diff.data.is_empty() {
            write_text(
                " ".repeat(diff.size).as_str(),
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
        response.context_menu(|ui| data_row_context_menu(ui, obj, diff_row, column, appearance));
        response.on_hover_ui_at_pointer(|ui| data_row_hover_ui(ui, obj, diff_row, appearance));
    }
}
