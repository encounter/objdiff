use std::mem::take;

use egui::{
    text::LayoutJob, CollapsingHeader, Color32, Id, OpenUrl, ScrollArea, SelectableLabel, TextEdit,
    Ui, Widget,
};
use objdiff_core::{
    arch::ObjArch,
    diff::{ObjDiff, ObjSymbolDiff},
    obj::{
        ObjInfo, ObjSection, ObjSectionKind, ObjSymbol, ObjSymbolFlags, ObjSymbolKind, SymbolRef,
    },
};
use regex::{Regex, RegexBuilder};

use crate::{
    app::AppStateRef,
    jobs::{
        create_scratch::{start_create_scratch, CreateScratchConfig, CreateScratchResult},
        objdiff::{BuildStatus, ObjDiffResult},
        Job, JobQueue, JobResult,
    },
    views::{
        appearance::Appearance,
        column_layout::{render_header, render_strips},
        function_diff::FunctionViewState,
        write_text,
    },
};

#[derive(Debug, Clone)]
pub struct SymbolRefByName {
    pub symbol_name: String,
    pub section_name: Option<String>,
}

impl SymbolRefByName {
    pub fn new(symbol: &ObjSymbol, section: Option<&ObjSection>) -> Self {
        Self { symbol_name: symbol.name.clone(), section_name: section.map(|s| s.name.clone()) }
    }
}

#[expect(clippy::enum_variant_names)]
#[derive(Debug, Default, Eq, PartialEq, Copy, Clone, Hash)]
pub enum View {
    #[default]
    SymbolDiff,
    FunctionDiff,
    DataDiff,
    ExtabDiff,
}

#[derive(Debug, Clone, Default)]
pub struct SymbolUiResult {
    pub view: Option<View>,
    pub left_symbol: Option<SymbolRefByName>,
    pub right_symbol: Option<SymbolRefByName>,
}

impl SymbolUiResult {
    pub fn function_diff(
        view: View,
        other_ctx: Option<SymbolDiffContext<'_>>,
        symbol: &ObjSymbol,
        section: &ObjSection,
        symbol_diff: &ObjSymbolDiff,
        column: usize,
    ) -> Self {
        let symbol1 = Some(SymbolRefByName::new(symbol, Some(section)));
        let symbol2 = symbol_diff.diff_symbol.and_then(|symbol_ref| {
            other_ctx.map(|ctx| {
                let (section, symbol) = ctx.obj.section_symbol(symbol_ref);
                SymbolRefByName::new(symbol, section)
            })
        });
        match column {
            0 => Self { view: Some(view), left_symbol: symbol1, right_symbol: symbol2 },
            1 => Self { view: Some(view), left_symbol: symbol2, right_symbol: symbol1 },
            _ => unreachable!("Invalid column index"),
        }
    }

    pub fn data_diff(section: &ObjSection) -> Self {
        let symbol = SymbolRefByName {
            symbol_name: section.name.clone(),
            section_name: Some(section.name.clone()),
        };
        Self {
            view: Some(View::DataDiff),
            left_symbol: Some(symbol.clone()),
            right_symbol: Some(symbol),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SymbolOverrideAction {
    ClearLeft(SymbolRefByName, SymbolRefByName),
    ClearRight(SymbolRefByName, SymbolRefByName),
    Set(SymbolRefByName, SymbolRefByName),
}

#[derive(Default)]
pub struct DiffViewState {
    pub build: Option<Box<ObjDiffResult>>,
    pub scratch: Option<Box<CreateScratchResult>>,
    pub current_view: View,
    pub symbol_state: SymbolViewState,
    pub function_state: FunctionViewState,
    pub search: String,
    pub search_regex: Option<Regex>,
    pub queue_build: bool,
    pub build_running: bool,
    pub scratch_available: bool,
    pub queue_scratch: bool,
    pub scratch_running: bool,
    pub source_path_available: bool,
    pub queue_open_source_path: bool,
    pub match_action: Option<SymbolOverrideAction>,
    pub post_build_nav: Option<SymbolUiResult>,
    pub object_name: String,
}

#[derive(Default)]
pub struct SymbolViewState {
    pub highlighted_symbol: (Option<SymbolRef>, Option<SymbolRef>),
    pub left_symbol: Option<SymbolRefByName>,
    pub right_symbol: Option<SymbolRefByName>,
    pub reverse_fn_order: bool,
    pub disable_reverse_fn_order: bool,
    pub show_hidden_symbols: bool,
}

impl DiffViewState {
    pub fn pre_update(&mut self, jobs: &mut JobQueue, state: &AppStateRef) {
        jobs.results.retain_mut(|result| match result {
            JobResult::ObjDiff(result) => {
                self.build = take(result);
                if let Some(result) = self.post_build_nav.take() {
                    if let Some(view) = result.view {
                        self.current_view = view;
                    }
                    self.symbol_state.left_symbol = result.left_symbol;
                    self.symbol_state.right_symbol = result.right_symbol;
                }
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
        if let Ok(state) = state.read() {
            if let Some(obj_config) = &state.config.selected_obj {
                if let Some(value) = obj_config.reverse_fn_order {
                    self.symbol_state.reverse_fn_order = value;
                    self.symbol_state.disable_reverse_fn_order = true;
                }
                self.source_path_available = obj_config.source_path.is_some();
            } else {
                self.source_path_available = false;
            }
            self.scratch_available = CreateScratchConfig::is_available(&state.config);
            self.object_name =
                state.config.selected_obj.as_ref().map(|o| o.name.clone()).unwrap_or_default();
        }
    }

    pub fn post_update(&mut self, ctx: &egui::Context, jobs: &mut JobQueue, state: &AppStateRef) {
        if let Some(result) = take(&mut self.scratch) {
            ctx.output_mut(|o| o.open_url = Some(OpenUrl::new_tab(result.scratch_url)));
        }

        if self.queue_build && !jobs.is_running(Job::ObjDiff) {
            self.queue_build = false;
            if let Ok(mut state) = state.write() {
                match self.match_action.take() {
                    Some(SymbolOverrideAction::ClearLeft(left_ref, right_ref)) => {
                        let symbol_overrides = &mut state.config.diff_obj_config.symbol_overrides;
                        symbol_overrides.remove_left(&left_ref.symbol_name, &right_ref.symbol_name);
                    }
                    Some(SymbolOverrideAction::ClearRight(left_ref, right_ref)) => {
                        let symbol_overrides = &mut state.config.diff_obj_config.symbol_overrides;
                        symbol_overrides
                            .remove_right(&left_ref.symbol_name, &right_ref.symbol_name);
                    }
                    Some(SymbolOverrideAction::Set(left_ref, right_ref)) => {
                        let symbol_overrides = &mut state.config.diff_obj_config.symbol_overrides;
                        symbol_overrides.set(left_ref.symbol_name, right_ref.symbol_name);
                    }
                    None => {}
                }
                state.queue_build = true;
            }
        }

        if self.queue_scratch {
            self.queue_scratch = false;
            if let Some(function_name) =
                self.symbol_state.left_symbol.as_ref().map(|sym| sym.symbol_name.clone())
            {
                if let Ok(state) = state.read() {
                    match CreateScratchConfig::from_config(&state.config, function_name) {
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

        if self.queue_open_source_path {
            self.queue_open_source_path = false;
            if let Ok(state) = state.read() {
                if let (Some(project_dir), Some(source_path)) = (
                    &state.config.project_dir,
                    state.config.selected_obj.as_ref().and_then(|obj| obj.source_path.as_ref()),
                ) {
                    let source_path = project_dir.join(source_path);
                    log::info!("Opening file {}", source_path.display());
                    open::that_detached(source_path).unwrap_or_else(|err| {
                        log::error!("Failed to open source file: {err}");
                    });
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

fn symbol_context_menu_ui(
    ui: &mut Ui,
    ctx: SymbolDiffContext<'_>,
    other_ctx: Option<SymbolDiffContext<'_>>,
    symbol: &ObjSymbol,
    symbol_diff: &ObjSymbolDiff,
    section: Option<&ObjSection>,
    column: usize,
) -> Option<SymbolUiResult> {
    let mut ret = None;
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        if let Some(name) = &symbol.demangled_name {
            if ui.button(format!("Copy \"{name}\"")).clicked() {
                ui.output_mut(|output| output.copied_text.clone_from(name));
                ui.close_menu();
            }
        }
        if ui.button(format!("Copy \"{}\"", symbol.name)).clicked() {
            ui.output_mut(|output| output.copied_text.clone_from(&symbol.name));
            ui.close_menu();
        }
        if let Some(address) = symbol.virtual_address {
            if ui.button(format!("Copy \"{:#x}\" (virtual address)", address)).clicked() {
                ui.output_mut(|output| output.copied_text = format!("{:#x}", address));
                ui.close_menu();
            }
        }
        if let Some(section) = section {
            let has_extab =
                ctx.obj.arch.ppc().and_then(|ppc| ppc.extab_for_symbol(symbol)).is_some();
            if has_extab && ui.button("Decode exception table").clicked() {
                ret = Some(SymbolUiResult::function_diff(
                    View::ExtabDiff,
                    other_ctx,
                    symbol,
                    section,
                    symbol_diff,
                    column,
                ));
                ui.close_menu();
            }
        }
    });
    ret
}

fn symbol_hover_ui(ui: &mut Ui, arch: &dyn ObjArch, symbol: &ObjSymbol, appearance: &Appearance) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

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
            ui.colored_label(appearance.replace_color, format!("Virtual address: {:#x}", address));
        }
        if let Some(extab) = arch.ppc().and_then(|ppc| ppc.extab_for_symbol(symbol)) {
            ui.colored_label(
                appearance.highlight_color,
                format!("extab symbol: {}", &extab.etb_symbol.name),
            );
            ui.colored_label(
                appearance.highlight_color,
                format!("extabindex symbol: {}", &extab.eti_symbol.name),
            );
        }
    });
}

#[must_use]
#[expect(clippy::too_many_arguments)]
fn symbol_ui(
    ui: &mut Ui,
    ctx: SymbolDiffContext<'_>,
    other_ctx: Option<SymbolDiffContext<'_>>,
    symbol: &ObjSymbol,
    symbol_diff: &ObjSymbolDiff,
    section: Option<&ObjSection>,
    state: &mut SymbolViewState,
    appearance: &Appearance,
    column: usize,
) -> Option<SymbolUiResult> {
    if symbol.flags.0.contains(ObjSymbolFlags::Hidden) && !state.show_hidden_symbols {
        return None;
    }
    let mut ret = None;
    let mut job = LayoutJob::default();
    let name: &str =
        if let Some(demangled) = &symbol.demangled_name { demangled } else { &symbol.name };
    let mut selected = false;
    if let Some(sym_ref) =
        if column == 0 { state.highlighted_symbol.0 } else { state.highlighted_symbol.1 }
    {
        selected = symbol_diff.symbol_ref == sym_ref;
    }
    if !symbol.flags.0.is_empty() {
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
        if symbol.flags.0.contains(ObjSymbolFlags::HasExtra) {
            write_text("e", appearance.text_color, &mut job, appearance.code_font.clone());
        }
        if symbol.flags.0.contains(ObjSymbolFlags::Hidden) {
            write_text(
                "h",
                appearance.deemphasized_text_color,
                &mut job,
                appearance.code_font.clone(),
            );
        }
        write_text("] ", appearance.text_color, &mut job, appearance.code_font.clone());
    }
    if let Some(match_percent) = symbol_diff.match_percent {
        write_text("(", appearance.text_color, &mut job, appearance.code_font.clone());
        write_text(
            &format!("{:.0}%", match_percent.floor()),
            match_color_for_symbol(match_percent, appearance),
            &mut job,
            appearance.code_font.clone(),
        );
        write_text(") ", appearance.text_color, &mut job, appearance.code_font.clone());
    }
    write_text(name, appearance.highlight_color, &mut job, appearance.code_font.clone());
    let response = SelectableLabel::new(selected, job).ui(ui).on_hover_ui_at_pointer(|ui| {
        symbol_hover_ui(ui, ctx.obj.arch.as_ref(), symbol, appearance)
    });
    response.context_menu(|ui| {
        if let Some(result) =
            symbol_context_menu_ui(ui, ctx, other_ctx, symbol, symbol_diff, section, column)
        {
            ret = Some(result);
        }
    });
    if response.clicked() {
        if let Some(section) = section {
            match section.kind {
                ObjSectionKind::Code => {
                    ret = Some(SymbolUiResult::function_diff(
                        View::FunctionDiff,
                        other_ctx,
                        symbol,
                        section,
                        symbol_diff,
                        column,
                    ));
                }
                ObjSectionKind::Data => {
                    ret = Some(SymbolUiResult::data_diff(section));
                }
                ObjSectionKind::Bss => {}
            }
        }
    } else if response.hovered() {
        state.highlighted_symbol = if let Some(diff_symbol) = symbol_diff.diff_symbol {
            if column == 0 {
                (Some(symbol_diff.symbol_ref), Some(diff_symbol))
            } else {
                (Some(diff_symbol), Some(symbol_diff.symbol_ref))
            }
        } else {
            (None, None)
        };
    }
    ret
}

fn symbol_matches_filter(symbol: &ObjSymbol, filter: SymbolFilter<'_>) -> bool {
    match filter {
        SymbolFilter::None => true,
        SymbolFilter::Search(regex) => {
            regex.is_match(&symbol.name)
                || symbol.demangled_name.as_ref().map(|s| regex.is_match(s)).unwrap_or(false)
        }
        SymbolFilter::Kind(kind) => symbol.kind == kind,
    }
}

#[derive(Copy, Clone)]
pub enum SymbolFilter<'a> {
    None,
    Search(&'a Regex),
    Kind(ObjSymbolKind),
}

#[must_use]
pub fn symbol_list_ui(
    ui: &mut Ui,
    ctx: SymbolDiffContext<'_>,
    other_ctx: Option<SymbolDiffContext<'_>>,
    state: &mut SymbolViewState,
    filter: SymbolFilter<'_>,
    appearance: &Appearance,
    column: usize,
) -> Option<SymbolUiResult> {
    let mut ret = None;
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

            // Skip sections with all symbols filtered out
            if !ctx.obj.common.is_empty()
                && (matches!(filter, SymbolFilter::None)
                    || ctx
                        .obj
                        .common
                        .iter()
                        .zip(&ctx.diff.common)
                        .any(|(symbol, _)| symbol_matches_filter(symbol, filter)))
            {
                CollapsingHeader::new(".comm").default_open(true).show(ui, |ui| {
                    for (symbol, symbol_diff) in ctx.obj.common.iter().zip(&ctx.diff.common) {
                        if !symbol_matches_filter(symbol, filter) {
                            continue;
                        }
                        if let Some(result) = symbol_ui(
                            ui,
                            ctx,
                            other_ctx,
                            symbol,
                            symbol_diff,
                            None,
                            state,
                            appearance,
                            column,
                        ) {
                            ret = Some(result);
                        }
                    }
                });
            }

            for (section, section_diff) in ctx.obj.sections.iter().zip(&ctx.diff.sections) {
                // Skip sections with all symbols filtered out
                if !matches!(filter, SymbolFilter::None)
                    && !section
                        .symbols
                        .iter()
                        .zip(&section_diff.symbols)
                        .any(|(symbol, _)| symbol_matches_filter(symbol, filter))
                {
                    continue;
                }
                let mut header = LayoutJob::simple_singleline(
                    format!("{} ({:x})", section.name, section.size),
                    appearance.code_font.clone(),
                    Color32::PLACEHOLDER,
                );
                if let Some(match_percent) = section_diff.match_percent {
                    write_text(
                        " (",
                        Color32::PLACEHOLDER,
                        &mut header,
                        appearance.code_font.clone(),
                    );
                    write_text(
                        &format!("{:.0}%", match_percent.floor()),
                        match_color_for_symbol(match_percent, appearance),
                        &mut header,
                        appearance.code_font.clone(),
                    );
                    write_text(
                        ")",
                        Color32::PLACEHOLDER,
                        &mut header,
                        appearance.code_font.clone(),
                    );
                }
                CollapsingHeader::new(header)
                    .id_salt(Id::new(section.name.clone()).with(section.orig_index))
                    .default_open(true)
                    .show(ui, |ui| {
                        if section.kind == ObjSectionKind::Code && state.reverse_fn_order {
                            for (symbol, symbol_diff) in
                                section.symbols.iter().zip(&section_diff.symbols).rev()
                            {
                                if !symbol_matches_filter(symbol, filter) {
                                    continue;
                                }
                                if let Some(result) = symbol_ui(
                                    ui,
                                    ctx,
                                    other_ctx,
                                    symbol,
                                    symbol_diff,
                                    Some(section),
                                    state,
                                    appearance,
                                    column,
                                ) {
                                    ret = Some(result);
                                }
                            }
                        } else {
                            for (symbol, symbol_diff) in
                                section.symbols.iter().zip(&section_diff.symbols)
                            {
                                if !symbol_matches_filter(symbol, filter) {
                                    continue;
                                }
                                if let Some(result) = symbol_ui(
                                    ui,
                                    ctx,
                                    other_ctx,
                                    symbol,
                                    symbol_diff,
                                    Some(section),
                                    state,
                                    appearance,
                                    column,
                                ) {
                                    ret = Some(result);
                                }
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
            if !status.cmdline.is_empty() && ui.button("Copy command").clicked() {
                ui.output_mut(|output| output.copied_text.clone_from(&status.cmdline));
            }
            if ui.button("Copy log").clicked() {
                ui.output_mut(|output| {
                    output.copied_text = format!("{}\n{}", status.stdout, status.stderr)
                });
            }
        });
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

            if !status.cmdline.is_empty() {
                ui.label(&status.cmdline);
            }
            if !status.stdout.is_empty() {
                ui.colored_label(appearance.replace_color, &status.stdout);
            }
            if !status.stderr.is_empty() {
                ui.colored_label(appearance.delete_color, &status.stderr);
            }
        });
    });
}

fn missing_obj_ui(ui: &mut Ui, appearance: &Appearance) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        ui.colored_label(appearance.replace_color, "No object configured");
    });
}

#[derive(Copy, Clone)]
pub struct SymbolDiffContext<'a> {
    pub obj: &'a ObjInfo,
    pub diff: &'a ObjDiff,
}

pub fn symbol_diff_ui(ui: &mut Ui, state: &mut DiffViewState, appearance: &Appearance) {
    let DiffViewState { build, current_view, symbol_state, search, search_regex, .. } = state;
    let Some(result) = build else {
        return;
    };

    // Header
    let available_width = ui.available_width();
    render_header(ui, available_width, 2, |ui, column| {
        if column == 0 {
            // Left column
            ui.scope(|ui| {
                ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);

                ui.label("Target object");
                if result.first_status.success {
                    if result.first_obj.is_none() {
                        ui.colored_label(appearance.replace_color, "Missing");
                    } else {
                        ui.colored_label(appearance.highlight_color, state.object_name.clone());
                    }
                } else {
                    ui.colored_label(appearance.delete_color, "Fail");
                }
            });

            if TextEdit::singleline(search).hint_text("Filter symbols").ui(ui).changed() {
                if search.is_empty() {
                    *search_regex = None;
                } else if let Ok(regex) = RegexBuilder::new(search).case_insensitive(true).build() {
                    *search_regex = Some(regex);
                } else {
                    *search_regex = None;
                }
            }
        } else if column == 1 {
            // Right column
            ui.horizontal(|ui| {
                ui.scope(|ui| {
                    ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                    ui.label("Base object");
                });
                ui.separator();
                if ui
                    .add_enabled(state.source_path_available, egui::Button::new("🖹 Source file"))
                    .on_hover_text_at_pointer("Open the source file in the default editor")
                    .on_disabled_hover_text("Source file metadata missing")
                    .clicked()
                {
                    state.queue_open_source_path = true;
                }
            });

            ui.scope(|ui| {
                ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                if result.second_status.success {
                    if result.second_obj.is_none() {
                        ui.colored_label(appearance.replace_color, "Missing");
                    } else {
                        ui.colored_label(appearance.highlight_color, "OK");
                    }
                } else {
                    ui.colored_label(appearance.delete_color, "Fail");
                }
            });

            if ui.add_enabled(!state.build_running, egui::Button::new("Build")).clicked() {
                state.queue_build = true;
            }
        }
    });

    // Table
    let filter = match search_regex {
        Some(regex) => SymbolFilter::Search(regex),
        _ => SymbolFilter::None,
    };
    let mut ret = None;
    render_strips(ui, available_width, 2, |ui, column| {
        if column == 0 {
            // Left column
            if result.first_status.success {
                if let Some((obj, diff)) = &result.first_obj {
                    if let Some(result) = symbol_list_ui(
                        ui,
                        SymbolDiffContext { obj, diff },
                        result
                            .second_obj
                            .as_ref()
                            .map(|(obj, diff)| SymbolDiffContext { obj, diff }),
                        symbol_state,
                        filter,
                        appearance,
                        column,
                    ) {
                        ret = Some(result);
                    }
                } else {
                    missing_obj_ui(ui, appearance);
                }
            } else {
                build_log_ui(ui, &result.first_status, appearance);
            }
        } else if column == 1 {
            // Right column
            if result.second_status.success {
                if let Some((obj, diff)) = &result.second_obj {
                    if let Some(result) = symbol_list_ui(
                        ui,
                        SymbolDiffContext { obj, diff },
                        result
                            .first_obj
                            .as_ref()
                            .map(|(obj, diff)| SymbolDiffContext { obj, diff }),
                        symbol_state,
                        filter,
                        appearance,
                        column,
                    ) {
                        ret = Some(result);
                    }
                } else {
                    missing_obj_ui(ui, appearance);
                }
            } else {
                build_log_ui(ui, &result.second_status, appearance);
            }
        }
    });
    if let Some(result) = ret {
        if let Some(view) = result.view {
            *current_view = view;
        }
        symbol_state.left_symbol = result.left_symbol;
        symbol_state.right_symbol = result.right_symbol;
    }
}
