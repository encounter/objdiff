//I hate rust i wish i could burn it with fire btw - Amber

use std::default::Default;

use egui::{text::LayoutJob, Align, Layout, Vec2, ScrollArea, Ui};
use egui_extras::{Size, StripBuilder};
use objdiff_core::{
    diff::ObjDiff,
    obj::{ObjInfo, ObjSymbol, SymbolRef, ObjExtab},
};
use time::format_description;

use crate::views::{
    appearance::Appearance,
    symbol_diff::{match_color_for_symbol, DiffViewState, SymbolRefByName, View},
};


#[derive(Default)]
pub struct ExtabViewState {
}

fn find_symbol(obj: &ObjInfo, selected_symbol: &SymbolRefByName) -> Option<SymbolRef> {
    for (section_idx, section) in obj.sections.iter().enumerate() {
        for (symbol_idx, symbol) in section.symbols.iter().enumerate() {
            if symbol.name == selected_symbol.symbol_name {
                return Some(SymbolRef { section_idx, symbol_idx });
            }
        }
    }
    None
}

fn decode_extab(extab: &ObjExtab) -> String {
    let mut text = String::from("");

    let mut dtor_names: Vec<&str> = vec![];
    for dtor in &extab.dtors {
        //For each function name, use the demangled name by default,
        //and if not available fallback to the original name
        let name =
        match &dtor.demangled_name {
            Some(demangled_name) => demangled_name,
            None => &dtor.name
        };
        dtor_names.push(name.as_str());
    }
    if let Some(decoded) = extab.data.to_string(&dtor_names) {
        text += decoded.as_str();
    }

    text
}

fn find_extab_entry(obj : &ObjInfo, symbol : &ObjSymbol) -> Option<ObjExtab> {
    if let Some(extab_array) = &obj.extab {
        for extab_entry in extab_array {
            if extab_entry.func.name == symbol.name {
                return Some(extab_entry.clone());
            }
        }
    }else{
        return None;
    }

    None
}

fn extab_text_ui(ui: &mut Ui, obj : &(ObjInfo, ObjDiff), symbol_ref : SymbolRef,
appearance: &Appearance, _state : &mut ExtabViewState) -> Option<()> {
    let (_section, symbol) = obj.0.section_symbol(symbol_ref);

    if let Some(extab_entry) = find_extab_entry(&obj.0, symbol) {
        let text = decode_extab(&extab_entry);
        ui.colored_label(appearance.replace_color, &text);
        return Some(());
    }

    None
}

fn extab_ui(
    ui: &mut Ui,
    obj: Option<&(ObjInfo, ObjDiff)>,
    selected_symbol: &SymbolRefByName,
    appearance: &Appearance,
    _left: bool,
    state: &mut ExtabViewState,
) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap = Some(false);

            let symbol =  obj.and_then(|(obj, _)| find_symbol(obj, selected_symbol));

            if let (Some(object), Some(symbol_ref)) = (obj, symbol) {
                extab_text_ui(ui, object, symbol_ref, appearance, state);
            }
        });
    });
}

pub fn extab_diff_ui(ui: &mut egui::Ui, state: &mut DiffViewState, appearance: &Appearance) {
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
                            .and_then(|(obj, diff)| {
                                find_symbol(obj, selected_symbol).map(|sref| {
                                    &diff.sections[sref.section_idx].symbols[sref.symbol_idx]
                                })
                            })
                            .and_then(|symbol| symbol.match_percent)
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
    StripBuilder::new(ui).size(Size::remainder()).vertical(|mut strip| {
        strip.strip(|builder| {
            builder.sizes(Size::remainder(), 2).horizontal(|mut strip| {
                strip.cell(|ui| {
                    ui.push_id("left", |ui| {
                        extab_ui(
                            ui,
                            result.first_obj.as_ref(),
                            selected_symbol,
                            appearance,
                            true,
                            &mut state.extab_state,
                        );
                    });
                });
                strip.cell(|ui| {
                    ui.push_id("right", |ui| {
                        extab_ui(
                            ui,
                            result.second_obj.as_ref(),
                            selected_symbol,
                            appearance,
                            false,
                            &mut state.extab_state,
                        );
                    });
                });
            });
        });
    });
}
