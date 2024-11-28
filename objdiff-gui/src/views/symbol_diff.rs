use std::{collections::BTreeMap, mem::take};

use egui::{
    text::LayoutJob, CollapsingHeader, Color32, Id, OpenUrl, ScrollArea, SelectableLabel, TextEdit,
    Ui, Widget,
};
use objdiff_core::{
    arch::ObjArch,
    diff::{display::HighlightKind, ObjDiff, ObjSymbolDiff},
    obj::{
        ObjInfo, ObjSection, ObjSectionKind, ObjSymbol, ObjSymbolFlags, SymbolRef, SECTION_COMMON,
    },
};
use regex::{Regex, RegexBuilder};

use crate::{
    app::AppStateRef,
    hotkeys,
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

#[derive(Debug, Clone)]
pub enum DiffViewAction {
    /// Queue a rebuild of the current object(s)
    Build,
    /// Navigate to a new diff view
    Navigate(DiffViewNavigation),
    /// Set the highlighted symbols in the symbols view
    SetSymbolHighlight(Option<SymbolRef>, Option<SymbolRef>),
    /// Set the symbols view search filter
    SetSearch(String),
    /// Submit the current function to decomp.me
    CreateScratch(String),
    /// Open the source path of the current object
    OpenSourcePath,
    /// Set the highlight for a diff column
    SetDiffHighlight(usize, HighlightKind),
    /// Clear the highlight for all diff columns
    ClearDiffHighlight,
    /// Start selecting a left symbol for mapping.
    /// The symbol reference is the right symbol to map to.
    SelectingLeft(SymbolRefByName),
    /// Start selecting a right symbol for mapping.
    /// The symbol reference is the left symbol to map to.
    SelectingRight(SymbolRefByName),
    /// Set a symbol mapping.
    SetMapping(View, SymbolRefByName, SymbolRefByName),
    /// Set the show_mapped_symbols flag
    SetShowMappedSymbols(bool),
}

#[derive(Debug, Clone, Default)]
pub struct DiffViewNavigation {
    pub view: Option<View>,
    pub left_symbol: Option<SymbolRefByName>,
    pub right_symbol: Option<SymbolRefByName>,
}

impl DiffViewNavigation {
    pub fn symbol_diff() -> Self {
        Self { view: Some(View::SymbolDiff), left_symbol: None, right_symbol: None }
    }

    pub fn with_symbols(
        view: View,
        other_ctx: Option<SymbolDiffContext<'_>>,
        symbol: &ObjSymbol,
        section: &ObjSection,
        symbol_diff: &ObjSymbolDiff,
        column: usize,
    ) -> Self {
        let symbol1 = Some(SymbolRefByName::new(symbol, Some(section)));
        let symbol2 = symbol_diff.target_symbol.and_then(|symbol_ref| {
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
    pub build_running: bool,
    pub scratch_available: bool,
    pub scratch_running: bool,
    pub source_path_available: bool,
    pub post_build_nav: Option<DiffViewNavigation>,
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
    pub show_mapped_symbols: bool,
}

impl DiffViewState {
    pub fn pre_update(&mut self, jobs: &mut JobQueue, state: &AppStateRef) {
        jobs.results.retain_mut(|result| match result {
            JobResult::ObjDiff(result) => {
                self.build = take(result);

                // TODO: where should this go?
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

    pub fn post_update(
        &mut self,
        action: Option<DiffViewAction>,
        ctx: &egui::Context,
        jobs: &mut JobQueue,
        state: &AppStateRef,
    ) {
        if let Some(result) = take(&mut self.scratch) {
            ctx.output_mut(|o| o.open_url = Some(OpenUrl::new_tab(result.scratch_url)));
        }

        let Some(action) = action else {
            return;
        };
        match action {
            DiffViewAction::Build => {
                if let Ok(mut state) = state.write() {
                    state.queue_build = true;
                }
            }
            DiffViewAction::Navigate(nav) => {
                if self.post_build_nav.is_some() {
                    // Ignore action if we're already navigating
                    return;
                }
                self.symbol_state.highlighted_symbol = (None, None);
                let Ok(mut state) = state.write() else {
                    return;
                };
                if (nav.left_symbol.is_some() && nav.right_symbol.is_some())
                    || (nav.left_symbol.is_none() && nav.right_symbol.is_none())
                    || nav.view != Some(View::FunctionDiff)
                {
                    // Regular navigation
                    if state.is_selecting_symbol() {
                        // Cancel selection and reload
                        state.clear_selection();
                        self.post_build_nav = Some(nav);
                    } else {
                        // Navigate immediately
                        if let Some(view) = nav.view {
                            self.current_view = view;
                        }
                        self.symbol_state.left_symbol = nav.left_symbol;
                        self.symbol_state.right_symbol = nav.right_symbol;
                    }
                } else {
                    // Enter selection mode
                    match (&nav.left_symbol, &nav.right_symbol) {
                        (Some(left_ref), None) => {
                            state.set_selecting_right(&left_ref.symbol_name);
                        }
                        (None, Some(right_ref)) => {
                            state.set_selecting_left(&right_ref.symbol_name);
                        }
                        (Some(_), Some(_)) => unreachable!(),
                        (None, None) => unreachable!(),
                    }
                    self.post_build_nav = Some(nav);
                }
            }
            DiffViewAction::SetSymbolHighlight(left, right) => {
                self.symbol_state.highlighted_symbol = (left, right);
            }
            DiffViewAction::SetSearch(search) => {
                self.search_regex = if search.is_empty() {
                    None
                } else if let Ok(regex) = RegexBuilder::new(&search).case_insensitive(true).build()
                {
                    Some(regex)
                } else {
                    None
                };
                self.search = search;
            }
            DiffViewAction::CreateScratch(function_name) => {
                let Ok(state) = state.read() else {
                    return;
                };
                match CreateScratchConfig::from_config(&state.config, function_name) {
                    Ok(config) => {
                        jobs.push_once(Job::CreateScratch, || start_create_scratch(ctx, config));
                    }
                    Err(err) => {
                        log::error!("Failed to create scratch config: {err}");
                    }
                }
            }
            DiffViewAction::OpenSourcePath => {
                let Ok(state) = state.read() else {
                    return;
                };
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
            DiffViewAction::SetDiffHighlight(column, kind) => {
                self.function_state.set_highlight(column, kind);
            }
            DiffViewAction::ClearDiffHighlight => {
                self.function_state.clear_highlight();
            }
            DiffViewAction::SelectingLeft(right_ref) => {
                if self.post_build_nav.is_some() {
                    // Ignore action if we're already navigating
                    return;
                }
                let Ok(mut state) = state.write() else {
                    return;
                };
                state.set_selecting_left(&right_ref.symbol_name);
                self.post_build_nav = Some(DiffViewNavigation {
                    view: Some(View::FunctionDiff),
                    left_symbol: None,
                    right_symbol: Some(right_ref),
                });
            }
            DiffViewAction::SelectingRight(left_ref) => {
                if self.post_build_nav.is_some() {
                    // Ignore action if we're already navigating
                    return;
                }
                let Ok(mut state) = state.write() else {
                    return;
                };
                state.set_selecting_right(&left_ref.symbol_name);
                self.post_build_nav = Some(DiffViewNavigation {
                    view: Some(View::FunctionDiff),
                    left_symbol: Some(left_ref),
                    right_symbol: None,
                });
            }
            DiffViewAction::SetMapping(view, left_ref, right_ref) => {
                if self.post_build_nav.is_some() {
                    // Ignore action if we're already navigating
                    return;
                }
                let Ok(mut state) = state.write() else {
                    return;
                };
                state.set_symbol_mapping(
                    left_ref.symbol_name.clone(),
                    right_ref.symbol_name.clone(),
                );
                if view == View::SymbolDiff {
                    self.post_build_nav = Some(DiffViewNavigation::symbol_diff());
                } else {
                    self.post_build_nav = Some(DiffViewNavigation {
                        view: Some(view),
                        left_symbol: Some(left_ref),
                        right_symbol: Some(right_ref),
                    });
                }
            }
            DiffViewAction::SetShowMappedSymbols(value) => {
                self.symbol_state.show_mapped_symbols = value;
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
) -> Option<DiffViewNavigation> {
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
                ret = Some(DiffViewNavigation::with_symbols(
                    View::ExtabDiff,
                    other_ctx,
                    symbol,
                    section,
                    symbol_diff,
                    column,
                ));
                ui.close_menu();
            }

            if ui.button("Map symbol").clicked() {
                let symbol_ref = SymbolRefByName::new(symbol, Some(section));
                if column == 0 {
                    ret = Some(DiffViewNavigation {
                        view: Some(View::FunctionDiff),
                        left_symbol: Some(symbol_ref),
                        right_symbol: None,
                    });
                } else {
                    ret = Some(DiffViewNavigation {
                        view: Some(View::FunctionDiff),
                        left_symbol: None,
                        right_symbol: Some(symbol_ref),
                    });
                }
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
    state: &SymbolViewState,
    appearance: &Appearance,
    column: usize,
) -> Option<DiffViewAction> {
    let mut ret = None;
    if symbol.flags.0.contains(ObjSymbolFlags::Hidden) && !state.show_hidden_symbols {
        return ret;
    }
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
            ret = Some(DiffViewAction::Navigate(result));
        }
    });
    if response.clicked() || (selected && hotkeys::enter_pressed(ui.ctx())) {
        if let Some(section) = section {
            match section.kind {
                ObjSectionKind::Code => {
                    ret = Some(DiffViewAction::Navigate(DiffViewNavigation::with_symbols(
                        View::FunctionDiff,
                        other_ctx,
                        symbol,
                        section,
                        symbol_diff,
                        column,
                    )));
                }
                ObjSectionKind::Data => {
                    ret = Some(DiffViewAction::Navigate(DiffViewNavigation::with_symbols(
                        View::DataDiff,
                        other_ctx,
                        symbol,
                        section,
                        symbol_diff,
                        column,
                    )));
                }
                ObjSectionKind::Bss => {}
            }
        }
    } else if response.hovered() {
        ret = Some(if let Some(target_symbol) = symbol_diff.target_symbol {
            if column == 0 {
                DiffViewAction::SetSymbolHighlight(
                    Some(symbol_diff.symbol_ref),
                    Some(target_symbol),
                )
            } else {
                DiffViewAction::SetSymbolHighlight(
                    Some(target_symbol),
                    Some(symbol_diff.symbol_ref),
                )
            }
        } else {
            DiffViewAction::SetSymbolHighlight(None, None)
        });
    }
    ret
}

fn symbol_matches_filter(
    symbol: &ObjSymbol,
    diff: &ObjSymbolDiff,
    filter: SymbolFilter<'_>,
) -> bool {
    match filter {
        SymbolFilter::None => true,
        SymbolFilter::Search(regex) => {
            regex.is_match(&symbol.name)
                || symbol.demangled_name.as_ref().map(|s| regex.is_match(s)).unwrap_or(false)
        }
        SymbolFilter::Mapping(symbol_ref) => diff.target_symbol == Some(symbol_ref),
    }
}

#[derive(Copy, Clone)]
pub enum SymbolFilter<'a> {
    None,
    Search(&'a Regex),
    Mapping(SymbolRef),
}

#[must_use]
pub fn symbol_list_ui(
    ui: &mut Ui,
    ctx: SymbolDiffContext<'_>,
    other_ctx: Option<SymbolDiffContext<'_>>,
    state: &SymbolViewState,
    filter: SymbolFilter<'_>,
    appearance: &Appearance,
    column: usize,
) -> Option<DiffViewAction> {
    let mut ret = None;
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        let mut mapping = BTreeMap::new();
        if let SymbolFilter::Mapping(target_ref) = filter {
            let mut show_mapped_symbols = state.show_mapped_symbols;
            if ui.checkbox(&mut show_mapped_symbols, "Show mapped symbols").changed() {
                ret = Some(DiffViewAction::SetShowMappedSymbols(show_mapped_symbols));
            }
            for mapping_diff in &ctx.diff.mapping_symbols {
                if mapping_diff.target_symbol == Some(target_ref) {
                    if !show_mapped_symbols {
                        let symbol_diff = ctx.diff.symbol_diff(mapping_diff.symbol_ref);
                        if symbol_diff.target_symbol.is_some() {
                            continue;
                        }
                    }
                    mapping.insert(mapping_diff.symbol_ref, mapping_diff);
                }
            }
        } else {
            for (symbol, diff) in ctx.obj.common.iter().zip(&ctx.diff.common) {
                if !symbol_matches_filter(symbol, diff, filter) {
                    continue;
                }
                mapping.insert(diff.symbol_ref, diff);
            }
            for (section, section_diff) in ctx.obj.sections.iter().zip(&ctx.diff.sections) {
                for (symbol, symbol_diff) in section.symbols.iter().zip(&section_diff.symbols) {
                    if !symbol_matches_filter(symbol, symbol_diff, filter) {
                        continue;
                    }
                    mapping.insert(symbol_diff.symbol_ref, symbol_diff);
                }
            }
        }

        hotkeys::check_scroll_hotkeys(ui);

        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

            // Skip sections with all symbols filtered out
            if mapping.keys().any(|symbol_ref| symbol_ref.section_idx == SECTION_COMMON) {
                CollapsingHeader::new(".comm").default_open(true).show(ui, |ui| {
                    for (symbol_ref, symbol_diff) in mapping
                        .iter()
                        .filter(|(symbol_ref, _)| symbol_ref.section_idx == SECTION_COMMON)
                    {
                        let symbol = ctx.obj.section_symbol(*symbol_ref).1;
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

            for ((section_index, section), section_diff) in
                ctx.obj.sections.iter().enumerate().zip(&ctx.diff.sections)
            {
                // Skip sections with all symbols filtered out
                if !mapping.keys().any(|symbol_ref| symbol_ref.section_idx == section_index) {
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
                            for (symbol, symbol_diff) in mapping
                                .iter()
                                .filter(|(symbol_ref, _)| symbol_ref.section_idx == section_index)
                                .rev()
                            {
                                let symbol = ctx.obj.section_symbol(*symbol).1;
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
                            for (symbol, symbol_diff) in mapping
                                .iter()
                                .filter(|(symbol_ref, _)| symbol_ref.section_idx == section_index)
                            {
                                let symbol = ctx.obj.section_symbol(*symbol).1;
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

#[must_use]
pub fn symbol_diff_ui(
    ui: &mut Ui,
    state: &mut DiffViewState,
    appearance: &Appearance,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let Some(result) = &state.build else {
        return ret;
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

            let mut search = state.search.clone();
            if TextEdit::singleline(&mut search).hint_text("Filter symbols").ui(ui).changed() {
                ret = Some(DiffViewAction::SetSearch(search));
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
                    ret = Some(DiffViewAction::OpenSourcePath);
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
                ret = Some(DiffViewAction::Build);
            }
        }
    });

    // Table
    let filter = match &state.search_regex {
        Some(regex) => SymbolFilter::Search(regex),
        _ => SymbolFilter::None,
    };
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
                        &state.symbol_state,
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
                        &state.symbol_state,
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
    ret
}
