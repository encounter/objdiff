use egui::{
    text::LayoutJob, CollapsingHeader, Color32, Rgba, ScrollArea, SelectableLabel, Ui, Widget,
};
use egui_extras::{Size, StripBuilder};

use crate::{
    app::{View, ViewState},
    jobs::objdiff::BuildStatus,
    obj::{ObjInfo, ObjSymbol, ObjSymbolFlags},
    views::write_text,
};

pub fn match_color_for_symbol(symbol: &ObjSymbol) -> Color32 {
    if symbol.match_percent == 100.0 {
        Color32::GREEN
    } else if symbol.match_percent >= 50.0 {
        Color32::LIGHT_BLUE
    } else {
        Color32::RED
    }
}

fn symbol_context_menu_ui(ui: &mut Ui, symbol: &ObjSymbol) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap = Some(false);

        if let Some(name) = &symbol.demangled_name {
            if ui.button(format!("Copy \"{}\"", name)).clicked() {
                ui.output().copied_text = name.clone();
                ui.close_menu();
            }
        }
        if ui.button(format!("Copy \"{}\"", symbol.name)).clicked() {
            ui.output().copied_text = symbol.name.clone();
            ui.close_menu();
        }
    });
}

fn symbol_hover_ui(ui: &mut Ui, symbol: &ObjSymbol) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap = Some(false);

        ui.colored_label(Color32::WHITE, format!("Name: {}", symbol.name));
        ui.colored_label(Color32::WHITE, format!("Address: {:x}", symbol.address));
        ui.colored_label(Color32::WHITE, format!("Size: {:x}", symbol.size));
    });
}

fn symbol_ui(
    ui: &mut Ui,
    symbol: &ObjSymbol,
    highlighted_symbol: &mut Option<String>,
    selected_symbol: &mut Option<String>,
    current_view: &mut View,
) {
    let mut job = LayoutJob::default();
    let name: &str =
        if let Some(demangled) = &symbol.demangled_name { demangled } else { &symbol.name };
    let mut selected = false;
    if let Some(sym) = highlighted_symbol {
        selected = sym == &symbol.name;
    }
    write_text("[", Color32::GRAY, &mut job);
    if symbol.flags.0.contains(ObjSymbolFlags::Common) {
        write_text("c", Color32::from_rgb(0, 255, 255), &mut job);
    } else if symbol.flags.0.contains(ObjSymbolFlags::Global) {
        write_text("g", Color32::GREEN, &mut job);
    } else if symbol.flags.0.contains(ObjSymbolFlags::Local) {
        write_text("l", Color32::GRAY, &mut job);
    }
    if symbol.flags.0.contains(ObjSymbolFlags::Weak) {
        write_text("w", Color32::GRAY, &mut job);
    }
    write_text("] ", Color32::GRAY, &mut job);
    if symbol.match_percent > 0.0 {
        write_text("(", Color32::GRAY, &mut job);
        write_text(
            &format!("{:.0}%", symbol.match_percent),
            match_color_for_symbol(symbol),
            &mut job,
        );
        write_text(") ", Color32::GRAY, &mut job);
    }
    write_text(name, Color32::WHITE, &mut job);
    let response = SelectableLabel::new(selected, job)
        .ui(ui)
        .context_menu(|ui| symbol_context_menu_ui(ui, symbol))
        .on_hover_ui_at_pointer(|ui| symbol_hover_ui(ui, symbol));
    if response.clicked() {
        *selected_symbol = Some(symbol.name.clone());
        *current_view = View::FunctionDiff;
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

fn symbol_list_ui(
    ui: &mut Ui,
    obj: &ObjInfo,
    highlighted_symbol: &mut Option<String>,
    selected_symbol: &mut Option<String>,
    current_view: &mut View,
    reverse_function_order: bool,
    search: &mut String,
) {
    ui.text_edit_singleline(search);
    let lower_search = search.to_ascii_lowercase();

    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap = Some(false);

            if !obj.common.is_empty() {
                CollapsingHeader::new(".comm").default_open(true).show(ui, |ui| {
                    for symbol in &obj.common {
                        symbol_ui(ui, symbol, highlighted_symbol, selected_symbol, current_view);
                    }
                });
            }

            for section in &obj.sections {
                CollapsingHeader::new(format!("{} ({:x})", section.name, section.size))
                    .default_open(true)
                    .show(ui, |ui| {
                        if section.name == ".text" && reverse_function_order {
                            for symbol in section.symbols.iter().rev() {
                                if !symbol_matches_search(symbol, &lower_search) {
                                    continue;
                                }
                                symbol_ui(
                                    ui,
                                    symbol,
                                    highlighted_symbol,
                                    selected_symbol,
                                    current_view,
                                );
                            }
                        } else {
                            for symbol in &section.symbols {
                                if !symbol_matches_search(symbol, &lower_search) {
                                    continue;
                                }
                                symbol_ui(
                                    ui,
                                    symbol,
                                    highlighted_symbol,
                                    selected_symbol,
                                    current_view,
                                );
                            }
                        }
                    });
            }
        });
    });
}

fn build_log_ui(ui: &mut Ui, status: &BuildStatus) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap = Some(false);

            ui.colored_label(Color32::from_rgb(255, 0, 0), &status.log);
        });
    });
}

pub fn symbol_diff_ui(ui: &mut Ui, view_state: &mut ViewState) {
    if let (Some(result), highlighted_symbol, selected_symbol, current_view, search) = (
        &view_state.build,
        &mut view_state.highlighted_symbol,
        &mut view_state.selected_symbol,
        &mut view_state.current_view,
        &mut view_state.search,
    ) {
        StripBuilder::new(ui).size(Size::exact(40.0)).size(Size::remainder()).vertical(
            |mut strip| {
                strip.strip(|builder| {
                    builder.sizes(Size::remainder(), 2).horizontal(|mut strip| {
                        strip.cell(|ui| {
                            ui.scope(|ui| {
                                ui.style_mut().override_text_style =
                                    Some(egui::TextStyle::Monospace);
                                ui.style_mut().wrap = Some(false);

                                ui.label("Build target:");
                                if result.first_status.success {
                                    ui.label("OK");
                                } else {
                                    ui.colored_label(Rgba::from_rgb(1.0, 0.0, 0.0), "Fail");
                                }
                            });
                            ui.separator();
                        });
                        strip.cell(|ui| {
                            ui.scope(|ui| {
                                ui.style_mut().override_text_style =
                                    Some(egui::TextStyle::Monospace);
                                ui.style_mut().wrap = Some(false);

                                ui.label("Build base:");
                                if result.second_status.success {
                                    ui.label("OK");
                                } else {
                                    ui.colored_label(Rgba::from_rgb(1.0, 0.0, 0.0), "Fail");
                                }
                            });
                            ui.separator();
                        });
                    });
                });
                strip.strip(|builder| {
                    builder.sizes(Size::remainder(), 2).horizontal(|mut strip| {
                        strip.cell(|ui| {
                            if result.first_status.success {
                                if let Some(obj) = &result.first_obj {
                                    ui.push_id("left", |ui| {
                                        symbol_list_ui(
                                            ui,
                                            obj,
                                            highlighted_symbol,
                                            selected_symbol,
                                            current_view,
                                            view_state.reverse_fn_order,
                                            search,
                                        );
                                    });
                                }
                            } else {
                                build_log_ui(ui, &result.first_status);
                            }
                        });
                        strip.cell(|ui| {
                            if result.second_status.success {
                                if let Some(obj) = &result.second_obj {
                                    ui.push_id("right", |ui| {
                                        symbol_list_ui(
                                            ui,
                                            obj,
                                            highlighted_symbol,
                                            selected_symbol,
                                            current_view,
                                            view_state.reverse_fn_order,
                                            search,
                                        );
                                    });
                                }
                            } else {
                                build_log_ui(ui, &result.second_status);
                            }
                        });
                    });
                });
            },
        );
    }
}
