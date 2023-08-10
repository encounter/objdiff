use egui::{
    text::LayoutJob, Align, CollapsingHeader, Color32, Layout, Rgba, ScrollArea, SelectableLabel,
    TextEdit, Ui, Vec2, Widget,
};
use egui_extras::{Size, StripBuilder};

use crate::{
    jobs::objdiff::{BuildStatus, ObjDiffResult},
    obj::{ObjInfo, ObjSection, ObjSectionKind, ObjSymbol, ObjSymbolFlags},
    views::{appearance::Appearance, write_text},
};

pub struct SymbolReference {
    pub symbol_name: String,
    pub section_name: String,
}

#[allow(clippy::enum_variant_names)]
#[derive(Default, Eq, PartialEq)]
pub enum View {
    #[default]
    SymbolDiff,
    FunctionDiff,
    DataDiff,
}

#[derive(Default)]
pub struct DiffViewState {
    pub build: Option<Box<ObjDiffResult>>,
    pub current_view: View,
    pub highlighted_symbol: Option<String>,
    pub selected_symbol: Option<SymbolReference>,
    pub search: String,
}

pub fn match_color_for_symbol(match_percent: f32, appearance: &Appearance) -> Color32 {
    if match_percent == 100.0 {
        appearance.insert_color
    } else if match_percent >= 50.0 {
        appearance.replace_color
    } else {
        appearance.delete_color
    }
}

fn symbol_context_menu_ui(ui: &mut Ui, symbol: &ObjSymbol) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap = Some(false);

        if let Some(name) = &symbol.demangled_name {
            if ui.button(format!("Copy \"{name}\"")).clicked() {
                ui.output_mut(|output| output.copied_text = name.clone());
                ui.close_menu();
            }
        }
        if ui.button(format!("Copy \"{}\"", symbol.name)).clicked() {
            ui.output_mut(|output| output.copied_text = symbol.name.clone());
            ui.close_menu();
        }
    });
}

fn symbol_hover_ui(ui: &mut Ui, symbol: &ObjSymbol, appearance: &Appearance) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap = Some(false);

        ui.colored_label(appearance.highlight_color, format!("Name: {}", symbol.name));
        ui.colored_label(appearance.highlight_color, format!("Address: {:x}", symbol.address));
        if symbol.size_known {
            ui.colored_label(appearance.highlight_color, format!("Size: {:x}", symbol.size));
        } else {
            ui.colored_label(
                appearance.highlight_color,
                format!("Size: {:x} (assumed)", symbol.size),
            );
        }
    });
}

fn symbol_ui(
    ui: &mut Ui,
    symbol: &ObjSymbol,
    section: Option<&ObjSection>,
    highlighted_symbol: &mut Option<String>,
    selected_symbol: &mut Option<SymbolReference>,
    current_view: &mut View,
    appearance: &Appearance,
) {
    let mut job = LayoutJob::default();
    let name: &str =
        if let Some(demangled) = &symbol.demangled_name { demangled } else { &symbol.name };
    let mut selected = false;
    if let Some(sym) = highlighted_symbol {
        selected = sym == &symbol.name;
    }
    write_text("[", appearance.text_color, &mut job, appearance.code_font.clone());
    if symbol.flags.0.contains(ObjSymbolFlags::Common) {
        write_text(
            "c",
            appearance.replace_color, /* Color32::from_rgb(0, 255, 255) */
            &mut job,
            appearance.code_font.clone(),
        );
    } else if symbol.flags.0.contains(ObjSymbolFlags::Global) {
        write_text("g", appearance.insert_color, &mut job, appearance.code_font.clone());
    } else if symbol.flags.0.contains(ObjSymbolFlags::Local) {
        write_text("l", appearance.text_color, &mut job, appearance.code_font.clone());
    }
    if symbol.flags.0.contains(ObjSymbolFlags::Weak) {
        write_text("w", appearance.text_color, &mut job, appearance.code_font.clone());
    }
    write_text("] ", appearance.text_color, &mut job, appearance.code_font.clone());
    if let Some(match_percent) = symbol.match_percent {
        write_text("(", appearance.text_color, &mut job, appearance.code_font.clone());
        write_text(
            &format!("{match_percent:.0}%"),
            match_color_for_symbol(match_percent, appearance),
            &mut job,
            appearance.code_font.clone(),
        );
        write_text(") ", appearance.text_color, &mut job, appearance.code_font.clone());
    }
    write_text(name, appearance.highlight_color, &mut job, appearance.code_font.clone());
    let response = SelectableLabel::new(selected, job)
        .ui(ui)
        .context_menu(|ui| symbol_context_menu_ui(ui, symbol))
        .on_hover_ui_at_pointer(|ui| symbol_hover_ui(ui, symbol, appearance));
    if response.clicked() {
        if let Some(section) = section {
            if section.kind == ObjSectionKind::Code {
                *selected_symbol = Some(SymbolReference {
                    symbol_name: symbol.name.clone(),
                    section_name: section.name.clone(),
                });
                *current_view = View::FunctionDiff;
            } else if section.kind == ObjSectionKind::Data {
                *selected_symbol = Some(SymbolReference {
                    symbol_name: section.name.clone(),
                    section_name: section.name.clone(),
                });
                *current_view = View::DataDiff;
            }
        }
    } else if response.hovered() {
        *highlighted_symbol = Some(symbol.name.clone());
    }
}

fn symbol_matches_search(symbol: &ObjSymbol, search_str: &str) -> bool {
    search_str.is_empty()
        || symbol.name.contains(search_str)
        || symbol
            .demangled_name
            .as_ref()
            .map(|s| s.to_ascii_lowercase().contains(search_str))
            .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn symbol_list_ui(
    ui: &mut Ui,
    obj: &ObjInfo,
    highlighted_symbol: &mut Option<String>,
    selected_symbol: &mut Option<SymbolReference>,
    current_view: &mut View,
    lower_search: &str,
    appearance: &Appearance,
) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap = Some(false);

            if !obj.common.is_empty() {
                CollapsingHeader::new(".comm").default_open(true).show(ui, |ui| {
                    for symbol in &obj.common {
                        symbol_ui(
                            ui,
                            symbol,
                            None,
                            highlighted_symbol,
                            selected_symbol,
                            current_view,
                            appearance,
                        );
                    }
                });
            }

            for section in &obj.sections {
                CollapsingHeader::new(format!("{} ({:x})", section.name, section.size))
                    .default_open(true)
                    .show(ui, |ui| {
                        if section.kind == ObjSectionKind::Code && appearance.reverse_fn_order {
                            for symbol in section.symbols.iter().rev() {
                                if !symbol_matches_search(symbol, lower_search) {
                                    continue;
                                }
                                symbol_ui(
                                    ui,
                                    symbol,
                                    Some(section),
                                    highlighted_symbol,
                                    selected_symbol,
                                    current_view,
                                    appearance,
                                );
                            }
                        } else {
                            for symbol in &section.symbols {
                                if !symbol_matches_search(symbol, lower_search) {
                                    continue;
                                }
                                symbol_ui(
                                    ui,
                                    symbol,
                                    Some(section),
                                    highlighted_symbol,
                                    selected_symbol,
                                    current_view,
                                    appearance,
                                );
                            }
                        }
                    });
            }
        });
    });
}

fn build_log_ui(ui: &mut Ui, status: &BuildStatus, appearance: &Appearance) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap = Some(false);

            ui.colored_label(appearance.replace_color, &status.log);
        });
    });
}

pub fn symbol_diff_ui(ui: &mut Ui, state: &mut DiffViewState, appearance: &Appearance) {
    let DiffViewState { build, current_view, highlighted_symbol, selected_symbol, search } = state;
    let Some(result) = build else {
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

                    ui.scope(|ui| {
                        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                        ui.style_mut().wrap = Some(false);

                        ui.label("Build target:");
                        if result.first_status.success {
                            ui.label("OK");
                        } else {
                            ui.colored_label(Rgba::from_rgb(1.0, 0.0, 0.0), "Fail");
                        }
                    });

                    TextEdit::singleline(search).hint_text("Filter symbols").ui(ui);
                },
            );

            // Right column
            ui.allocate_ui_with_layout(
                Vec2 { x: column_width, y: 100.0 },
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(column_width);

                    ui.scope(|ui| {
                        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                        ui.style_mut().wrap = Some(false);

                        ui.label("Build base:");
                        if result.second_status.success {
                            ui.label("OK");
                        } else {
                            ui.colored_label(Rgba::from_rgb(1.0, 0.0, 0.0), "Fail");
                        }
                    });
                },
            );
        },
    );
    ui.separator();

    // Table
    let lower_search = search.to_ascii_lowercase();
    StripBuilder::new(ui).size(Size::remainder()).vertical(|mut strip| {
        strip.strip(|builder| {
            builder.sizes(Size::remainder(), 2).horizontal(|mut strip| {
                strip.cell(|ui| {
                    ui.push_id("left", |ui| {
                        if result.first_status.success {
                            if let Some(obj) = &result.first_obj {
                                symbol_list_ui(
                                    ui,
                                    obj,
                                    highlighted_symbol,
                                    selected_symbol,
                                    current_view,
                                    &lower_search,
                                    appearance,
                                );
                            }
                        } else {
                            build_log_ui(ui, &result.first_status, appearance);
                        }
                    });
                });
                strip.cell(|ui| {
                    ui.push_id("right", |ui| {
                        if result.second_status.success {
                            if let Some(obj) = &result.second_obj {
                                symbol_list_ui(
                                    ui,
                                    obj,
                                    highlighted_symbol,
                                    selected_symbol,
                                    current_view,
                                    &lower_search,
                                    appearance,
                                );
                            }
                        } else {
                            build_log_ui(ui, &result.second_status, appearance);
                        }
                    });
                });
            });
        });
    });
}
