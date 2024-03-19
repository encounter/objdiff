use std::mem::take;

use egui::{
    text::LayoutJob, Align, CollapsingHeader, Color32, Id, Layout, OpenUrl, ScrollArea,
    SelectableLabel, TextEdit, Ui, Vec2, Widget,
};
use egui_extras::{Size, StripBuilder};
use objdiff_core::{
    diff::{ObjDiff, ObjSymbolDiff},
    obj::{ObjInfo, ObjSection, ObjSectionKind, ObjSymbol, ObjSymbolFlags},
};

use crate::{
    app::AppConfigRef,
    jobs::{
        create_scratch::{start_create_scratch, CreateScratchConfig, CreateScratchResult},
        objdiff::{BuildStatus, ObjDiffResult},
        Job, JobQueue, JobResult,
    },
    views::{appearance::Appearance, function_diff::FunctionViewState, write_text},
};

pub struct SymbolRefByName {
    pub symbol_name: String,
    pub demangled_symbol_name: Option<String>,
    pub section_name: String,
}

#[allow(clippy::enum_variant_names)]
#[derive(Default, Eq, PartialEq, Copy, Clone)]
pub enum View {
    #[default]
    SymbolDiff,
    FunctionDiff,
    DataDiff,
}

#[derive(Default)]
pub struct DiffViewState {
    pub build: Option<Box<ObjDiffResult>>,
    pub scratch: Option<Box<CreateScratchResult>>,
    pub current_view: View,
    pub symbol_state: SymbolViewState,
    pub function_state: FunctionViewState,
    pub search: String,
    pub queue_build: bool,
    pub build_running: bool,
    pub scratch_available: bool,
    pub queue_scratch: bool,
    pub scratch_running: bool,
}

#[derive(Default)]
pub struct SymbolViewState {
    pub highlighted_symbol: Option<String>,
    pub selected_symbol: Option<SymbolRefByName>,
    pub reverse_fn_order: bool,
    pub disable_reverse_fn_order: bool,
    pub show_hidden_symbols: bool,
}

impl DiffViewState {
    pub fn pre_update(&mut self, jobs: &mut JobQueue, config: &AppConfigRef) {
        jobs.results.retain_mut(|result| match result {
            JobResult::ObjDiff(result) => {
                self.build = take(result);
                false
            }
            JobResult::CreateScratch(result) => {
                self.scratch = take(result);
                false
            }
            _ => true,
        });
        self.build_running = jobs.is_running(Job::ObjDiff);
        self.scratch_running = jobs.is_running(Job::CreateScratch);

        self.symbol_state.disable_reverse_fn_order = false;
        if let Ok(config) = config.read() {
            if let Some(obj_config) = &config.selected_obj {
                if let Some(value) = obj_config.reverse_fn_order {
                    self.symbol_state.reverse_fn_order = value;
                    self.symbol_state.disable_reverse_fn_order = true;
                }
            }
            self.scratch_available = CreateScratchConfig::is_available(&config);
        }
    }

    pub fn post_update(&mut self, ctx: &egui::Context, jobs: &mut JobQueue, config: &AppConfigRef) {
        if let Some(result) = take(&mut self.scratch) {
            ctx.output_mut(|o| o.open_url = Some(OpenUrl::new_tab(result.scratch_url)));
        }

        if self.queue_build {
            self.queue_build = false;
            if let Ok(mut config) = config.write() {
                config.queue_build = true;
            }
        }

        if self.queue_scratch {
            self.queue_scratch = false;
            if let Some(function_name) =
                self.symbol_state.selected_symbol.as_ref().map(|sym| sym.symbol_name.clone())
            {
                if let Ok(config) = config.read() {
                    match CreateScratchConfig::from_config(&config, function_name) {
                        Ok(config) => {
                            jobs.push_once(Job::CreateScratch, || {
                                start_create_scratch(ctx, config)
                            });
                        }
                        Err(err) => {
                            log::error!("Failed to create scratch config: {err}");
                        }
                    }
                }
            }
        }
    }
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
        if let Some(address) = symbol.virtual_address {
            if ui.button(format!("Copy \"{:#x}\" (virtual address)", address)).clicked() {
                ui.output_mut(|output| output.copied_text = format!("{:#x}", address));
                ui.close_menu();
            }
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
        if let Some(address) = symbol.virtual_address {
            ui.colored_label(
                appearance.highlight_color,
                format!("Virtual address: {:#x}", address),
            );
        }
    });
}

#[must_use]
fn symbol_ui(
    ui: &mut Ui,
    symbol: &ObjSymbol,
    symbol_diff: &ObjSymbolDiff,
    section: Option<&ObjSection>,
    state: &mut SymbolViewState,
    appearance: &Appearance,
) -> Option<View> {
    if symbol.flags.0.contains(ObjSymbolFlags::Hidden) && !state.show_hidden_symbols {
        return None;
    }
    let mut ret = None;
    let mut job = LayoutJob::default();
    let name: &str =
        if let Some(demangled) = &symbol.demangled_name { demangled } else { &symbol.name };
    let mut selected = false;
    if let Some(sym) = &state.highlighted_symbol {
        selected = sym == &symbol.name;
    }
    write_text("[", appearance.text_color, &mut job, appearance.code_font.clone());
    if symbol.flags.0.contains(ObjSymbolFlags::Common) {
        write_text("c", appearance.replace_color, &mut job, appearance.code_font.clone());
    } else if symbol.flags.0.contains(ObjSymbolFlags::Global) {
        write_text("g", appearance.insert_color, &mut job, appearance.code_font.clone());
    } else if symbol.flags.0.contains(ObjSymbolFlags::Local) {
        write_text("l", appearance.text_color, &mut job, appearance.code_font.clone());
    }
    if symbol.flags.0.contains(ObjSymbolFlags::Weak) {
        write_text("w", appearance.text_color, &mut job, appearance.code_font.clone());
    }
    if symbol.flags.0.contains(ObjSymbolFlags::Hidden) {
        write_text("h", appearance.deemphasized_text_color, &mut job, appearance.code_font.clone());
    }
    write_text("] ", appearance.text_color, &mut job, appearance.code_font.clone());
    if let Some(match_percent) = symbol_diff.match_percent {
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
        .on_hover_ui_at_pointer(|ui| symbol_hover_ui(ui, symbol, appearance));
    response.context_menu(|ui| symbol_context_menu_ui(ui, symbol));
    if response.clicked() {
        if let Some(section) = section {
            if section.kind == ObjSectionKind::Code {
                state.selected_symbol = Some(SymbolRefByName {
                    symbol_name: symbol.name.clone(),
                    demangled_symbol_name: symbol.demangled_name.clone(),
                    section_name: section.name.clone(),
                });
                ret = Some(View::FunctionDiff);
            } else if section.kind == ObjSectionKind::Data {
                state.selected_symbol = Some(SymbolRefByName {
                    symbol_name: section.name.clone(),
                    demangled_symbol_name: None,
                    section_name: section.name.clone(),
                });
                ret = Some(View::DataDiff);
            }
        }
    } else if response.hovered() {
        state.highlighted_symbol = Some(symbol.name.clone());
    }
    ret
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

#[must_use]
fn symbol_list_ui(
    ui: &mut Ui,
    obj: &(ObjInfo, ObjDiff),
    state: &mut SymbolViewState,
    lower_search: &str,
    appearance: &Appearance,
) -> Option<View> {
    let mut ret = None;
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap = Some(false);

            if !obj.0.common.is_empty() {
                CollapsingHeader::new(".comm").default_open(true).show(ui, |ui| {
                    for (symbol, symbol_diff) in obj.0.common.iter().zip(&obj.1.common) {
                        ret = ret.or(symbol_ui(ui, symbol, symbol_diff, None, state, appearance));
                    }
                });
            }

            for (section, section_diff) in obj.0.sections.iter().zip(&obj.1.sections) {
                CollapsingHeader::new(format!("{} ({:x})", section.name, section.size))
                    .id_source(Id::new(section.name.clone()).with(section.index))
                    .default_open(true)
                    .show(ui, |ui| {
                        if section.kind == ObjSectionKind::Code && state.reverse_fn_order {
                            for (symbol, symbol_diff) in
                                section.symbols.iter().zip(&section_diff.symbols).rev()
                            {
                                if !symbol_matches_search(symbol, lower_search) {
                                    continue;
                                }
                                ret = ret.or(symbol_ui(
                                    ui,
                                    symbol,
                                    symbol_diff,
                                    Some(section),
                                    state,
                                    appearance,
                                ));
                            }
                        } else {
                            for (symbol, symbol_diff) in
                                section.symbols.iter().zip(&section_diff.symbols)
                            {
                                if !symbol_matches_search(symbol, lower_search) {
                                    continue;
                                }
                                ret = ret.or(symbol_ui(
                                    ui,
                                    symbol,
                                    symbol_diff,
                                    Some(section),
                                    state,
                                    appearance,
                                ));
                            }
                        }
                    });
            }
        });
    });
    ret
}

fn build_log_ui(ui: &mut Ui, status: &BuildStatus, appearance: &Appearance) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            if ui.button("Copy command").clicked() {
                ui.output_mut(|output| output.copied_text = status.cmdline.clone());
            }
            if ui.button("Copy log").clicked() {
                ui.output_mut(|output| {
                    output.copied_text = format!("{}\n{}", status.stdout, status.stderr)
                });
            }
        });
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap = Some(false);

            ui.label(&status.cmdline);
            ui.colored_label(appearance.replace_color, &status.stdout);
            ui.colored_label(appearance.delete_color, &status.stderr);
        });
    });
}

fn missing_obj_ui(ui: &mut Ui, appearance: &Appearance) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap = Some(false);

        ui.colored_label(appearance.replace_color, "No object configured");
    });
}

pub fn symbol_diff_ui(ui: &mut Ui, state: &mut DiffViewState, appearance: &Appearance) {
    let DiffViewState { build, current_view, symbol_state, search, .. } = state;
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
                            if result.first_obj.is_none() {
                                ui.colored_label(appearance.replace_color, "Missing");
                            } else {
                                ui.label("OK");
                            }
                        } else {
                            ui.colored_label(appearance.delete_color, "Fail");
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
                            if result.second_obj.is_none() {
                                ui.colored_label(appearance.replace_color, "Missing");
                            } else {
                                ui.label("OK");
                            }
                        } else {
                            ui.colored_label(appearance.delete_color, "Fail");
                        }
                    });

                    if ui.add_enabled(!state.build_running, egui::Button::new("Build")).clicked() {
                        state.queue_build = true;
                    }
                },
            );
        },
    );
    ui.separator();

    // Table
    let mut ret = None;
    let lower_search = search.to_ascii_lowercase();
    StripBuilder::new(ui).size(Size::remainder()).vertical(|mut strip| {
        strip.strip(|builder| {
            builder.sizes(Size::remainder(), 2).horizontal(|mut strip| {
                strip.cell(|ui| {
                    ui.push_id("left", |ui| {
                        if result.first_status.success {
                            if let Some(obj) = &result.first_obj {
                                ret = ret.or(symbol_list_ui(
                                    ui,
                                    obj,
                                    symbol_state,
                                    &lower_search,
                                    appearance,
                                ));
                            } else {
                                missing_obj_ui(ui, appearance);
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
                                ret = ret.or(symbol_list_ui(
                                    ui,
                                    obj,
                                    symbol_state,
                                    &lower_search,
                                    appearance,
                                ));
                            } else {
                                missing_obj_ui(ui, appearance);
                            }
                        } else {
                            build_log_ui(ui, &result.second_status, appearance);
                        }
                    });
                });
            });
        });
    });

    if let Some(view) = ret {
        *current_view = view;
    }
}
