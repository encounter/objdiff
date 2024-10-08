use egui::{Align, Layout, Sense, Vec2};
use egui_extras::{Column, Size, StripBuilder, TableBuilder, TableRow};

pub fn render_header(
    ui: &mut egui::Ui,
    available_width: f32,
    num_columns: usize,
    mut add_contents: impl FnMut(&mut egui::Ui, usize),
) {
    let column_width = available_width / num_columns as f32;
    ui.allocate_ui_with_layout(
        Vec2 { x: available_width, y: 100.0 },
        Layout::left_to_right(Align::Min),
        |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for i in 0..num_columns {
                ui.allocate_ui_with_layout(
                    Vec2 { x: column_width, y: 100.0 },
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(column_width);
                        add_contents(ui, i);
                    },
                );
            }
        },
    );
    ui.separator();
}

pub fn render_table(
    ui: &mut egui::Ui,
    available_width: f32,
    num_columns: usize,
    row_height: f32,
    total_rows: usize,
    mut add_contents: impl FnMut(&mut TableRow, usize),
) {
    ui.style_mut().interaction.selectable_labels = false;
    let column_width = available_width / num_columns as f32;
    let available_height = ui.available_height();
    let table = TableBuilder::new(ui)
        .striped(false)
        .cell_layout(Layout::left_to_right(Align::Min))
        .columns(Column::exact(column_width).clip(true), num_columns)
        .resizable(false)
        .auto_shrink([false, false])
        .min_scrolled_height(available_height)
        .sense(Sense::click());
    table.body(|body| {
        body.rows(row_height, total_rows, |mut row| {
            row.set_hovered(false); // Disable hover effect
            for i in 0..num_columns {
                add_contents(&mut row, i);
            }
        });
    });
}

pub fn render_strips(
    ui: &mut egui::Ui,
    available_width: f32,
    num_columns: usize,
    mut add_contents: impl FnMut(&mut egui::Ui, usize),
) {
    let column_width = available_width / num_columns as f32;
    StripBuilder::new(ui).size(Size::remainder()).clip(true).vertical(|mut strip| {
        strip.strip(|builder| {
            builder.sizes(Size::exact(column_width), num_columns).clip(true).horizontal(
                |mut strip| {
                    for i in 0..num_columns {
                        strip.cell(|ui| {
                            ui.push_id(i, |ui| {
                                add_contents(ui, i);
                            });
                        });
                    }
                },
            );
        });
    });
}
