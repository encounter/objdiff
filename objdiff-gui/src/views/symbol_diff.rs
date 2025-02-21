use std::mem::take;

use egui::{
    style::ScrollAnimation, text::LayoutJob, CollapsingHeader, Color32, Id, OpenUrl, ScrollArea,
    SelectableLabel, Ui, Widget,
};
use objdiff_core::{
    diff::{
        display::{
            display_sections, symbol_context, symbol_hover, ContextMenuItem, HighlightKind,
            HoverItem, HoverItemColor, SectionDisplay, SymbolFilter,
        },
        ObjectDiff, SymbolDiff,
    },
    jobs::{create_scratch::CreateScratchResult, objdiff::ObjDiffResult, Job, JobQueue, JobResult},
    obj::{Object, Section, SectionKind, Symbol, SymbolFlag},
};
use regex::{Regex, RegexBuilder};

use crate::{
    app::AppStateRef,
    hotkeys,
    jobs::{is_create_scratch_available, start_create_scratch},
    views::{appearance::Appearance, function_diff::FunctionViewState, write_text},
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SymbolRefByName {
    pub symbol_name: String,
    pub section_name: Option<String>,
}

impl SymbolRefByName {
    pub fn new(symbol: &Symbol, section: Option<&Section>) -> Self {
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
    /// Set the highlighted symbols in the symbols view, optionally scrolling them into view.
    SetSymbolHighlight(Option<usize>, Option<usize>, bool),
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

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct DiffViewNavigation {
    pub view: View,
    pub left_symbol: Option<SymbolRefByName>,
    pub right_symbol: Option<SymbolRefByName>,
}

impl DiffViewNavigation {
    pub fn symbol_diff() -> Self {
        Self { view: View::SymbolDiff, left_symbol: None, right_symbol: None }
    }

    pub fn with_symbols(
        view: View,
        other_ctx: Option<SymbolDiffContext<'_>>,
        symbol: &Symbol,
        section: &Section,
        symbol_diff: &SymbolDiff,
        column: usize,
    ) -> Self {
        let symbol1 = Some(SymbolRefByName::new(symbol, Some(section)));
        let symbol2 = symbol_diff.target_symbol.and_then(|symbol_ref| {
            other_ctx.map(|ctx| {
                let symbol = &ctx.obj.symbols[symbol_ref];
                let section =
                    symbol.section.and_then(|section_idx| ctx.obj.sections.get(section_idx));
                SymbolRefByName::new(symbol, section)
            })
        });
        match column {
            0 => Self { view, left_symbol: symbol1, right_symbol: symbol2 },
            1 => Self { view, left_symbol: symbol2, right_symbol: symbol1 },
            _ => unreachable!("Invalid column index"),
        }
    }

    pub fn data_diff(section: &Section, column: usize) -> Self {
        let symbol = Some(SymbolRefByName {
            symbol_name: "".to_string(),
            section_name: Some(section.name.clone()),
        });
        match column {
            0 => Self {
                view: View::DataDiff,
                left_symbol: symbol.clone(),
                right_symbol: symbol.clone(),
            },
            1 => Self { view: View::DataDiff, left_symbol: symbol.clone(), right_symbol: symbol },
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
    pub highlighted_symbol: (Option<usize>, Option<usize>),
    pub autoscroll_to_highlighted_symbols: bool,
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
                    self.current_view = result.view;
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
            self.scratch_available = is_create_scratch_available(&state.config);
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

        // Clear the autoscroll flag so that it doesn't scroll continuously.
        self.symbol_state.autoscroll_to_highlighted_symbols = false;

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
                let Ok(mut state) = state.write() else {
                    return;
                };
                if (nav.left_symbol.is_some() && nav.right_symbol.is_some())
                    || (nav.left_symbol.is_none() && nav.right_symbol.is_none())
                    || nav.view != View::FunctionDiff
                {
                    // Regular navigation
                    if state.is_selecting_symbol() {
                        // Cancel selection and reload
                        state.clear_selection();
                        self.post_build_nav = Some(nav);
                    } else {
                        // Navigate immediately
                        self.current_view = nav.view;
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
            DiffViewAction::SetSymbolHighlight(left, right, autoscroll) => {
                self.symbol_state.highlighted_symbol = (left, right);
                self.symbol_state.autoscroll_to_highlighted_symbols = autoscroll;
            }
            DiffViewAction::SetSearch(search) => {
                self.search_regex = if search.is_empty() {
                    None
                } else {
                    RegexBuilder::new(&search).case_insensitive(true).build().ok()
                };
                self.search = search;
            }
            DiffViewAction::CreateScratch(function_name) => {
                let Ok(state) = state.read() else {
                    return;
                };
                start_create_scratch(ctx, jobs, &state, function_name);
            }
            DiffViewAction::OpenSourcePath => {
                let Ok(state) = state.read() else {
                    return;
                };
                if let Some(source_path) =
                    state.config.selected_obj.as_ref().and_then(|obj| obj.source_path.as_ref())
                {
                    log::info!("Opening file {}", source_path);
                    open::that_detached(source_path.as_str()).unwrap_or_else(|err| {
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
                    view: View::FunctionDiff,
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
                    view: View::FunctionDiff,
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
                        view,
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
    symbol: &Symbol,
    symbol_diff: &SymbolDiff,
    section: Option<&Section>,
    column: usize,
) -> Option<DiffViewNavigation> {
    let mut ret = None;
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        for item in symbol_context(ctx.obj, symbol) {
            match item {
                ContextMenuItem::Copy { value, label } => {
                    let label = if let Some(extra) = label {
                        format!("Copy \"{value}\" ({extra})")
                    } else {
                        format!("Copy \"{value}\"")
                    };
                    if ui.button(label).clicked() {
                        ui.output_mut(|output| output.copied_text = value);
                        ui.close_menu();
                    }
                }
                ContextMenuItem::Navigate { label } => {
                    if ui.button(label).clicked() {
                        // TODO other navigation
                        ret = Some(DiffViewNavigation::with_symbols(
                            View::ExtabDiff,
                            other_ctx,
                            symbol,
                            section.unwrap(),
                            symbol_diff,
                            column,
                        ));
                        ui.close_menu();
                    }
                }
            }
        }

        if let Some(section) = section {
            if ui.button("Map symbol").clicked() {
                let symbol_ref = SymbolRefByName::new(symbol, Some(section));
                if column == 0 {
                    ret = Some(DiffViewNavigation {
                        view: View::FunctionDiff,
                        left_symbol: Some(symbol_ref),
                        right_symbol: None,
                    });
                } else {
                    ret = Some(DiffViewNavigation {
                        view: View::FunctionDiff,
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

fn symbol_hover_ui(
    ui: &mut Ui,
    ctx: SymbolDiffContext<'_>,
    symbol: &Symbol,
    appearance: &Appearance,
) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        for HoverItem { text, color } in symbol_hover(ctx.obj, symbol) {
            let color = match color {
                HoverItemColor::Normal => appearance.text_color,
                HoverItemColor::Emphasized => appearance.highlight_color,
                HoverItemColor::Special => appearance.replace_color,
            };
            ui.colored_label(color, text);
        }
    });
}

#[must_use]
fn symbol_ui(
    ui: &mut Ui,
    ctx: SymbolDiffContext<'_>,
    other_ctx: Option<SymbolDiffContext<'_>>,
    symbol: &Symbol,
    symbol_diff: &SymbolDiff,
    symbol_idx: usize,
    section: Option<&Section>,
    state: &SymbolViewState,
    appearance: &Appearance,
    column: usize,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let mut job = LayoutJob::default();
    let name: &str =
        if let Some(demangled) = &symbol.demangled_name { demangled } else { &symbol.name };
    let mut selected = false;
    if let Some(sym_ref) =
        if column == 0 { state.highlighted_symbol.0 } else { state.highlighted_symbol.1 }
    {
        selected = symbol_idx == sym_ref;
    }
    if !symbol.flags.is_empty() {
        write_text("[", appearance.text_color, &mut job, appearance.code_font.clone());
        if symbol.flags.contains(SymbolFlag::Common) {
            write_text("c", appearance.replace_color, &mut job, appearance.code_font.clone());
        } else if symbol.flags.contains(SymbolFlag::Global) {
            write_text("g", appearance.insert_color, &mut job, appearance.code_font.clone());
        } else if symbol.flags.contains(SymbolFlag::Local) {
            write_text("l", appearance.text_color, &mut job, appearance.code_font.clone());
        }
        if symbol.flags.contains(SymbolFlag::Weak) {
            write_text("w", appearance.text_color, &mut job, appearance.code_font.clone());
        }
        if symbol.flags.contains(SymbolFlag::HasExtra) {
            write_text("e", appearance.text_color, &mut job, appearance.code_font.clone());
        }
        if symbol.flags.contains(SymbolFlag::Hidden) {
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
    let response = SelectableLabel::new(selected, job)
        .ui(ui)
        .on_hover_ui_at_pointer(|ui| symbol_hover_ui(ui, ctx, symbol, appearance));
    response.context_menu(|ui| {
        if let Some(result) =
            symbol_context_menu_ui(ui, ctx, other_ctx, symbol, symbol_diff, section, column)
        {
            ret = Some(DiffViewAction::Navigate(result));
        }
    });
    if selected && state.autoscroll_to_highlighted_symbols {
        // Automatically scroll the view to encompass the selected symbol in case the user selected
        // an offscreen symbol by using a keyboard shortcut.
        ui.scroll_to_rect_animation(response.rect, None, ScrollAnimation::none());
        // This autoscroll state flag will be reset in DiffViewState::post_update at the end of
        // every frame so that we don't continuously scroll the view back when the user is trying to
        // manually scroll away.
    }
    if response.clicked() || (selected && hotkeys::enter_pressed(ui.ctx())) {
        if let Some(section) = section {
            match section.kind {
                SectionKind::Code => {
                    ret = Some(DiffViewAction::Navigate(DiffViewNavigation::with_symbols(
                        View::FunctionDiff,
                        other_ctx,
                        symbol,
                        section,
                        symbol_diff,
                        column,
                    )));
                }
                SectionKind::Data => {
                    ret = Some(DiffViewAction::Navigate(DiffViewNavigation::data_diff(
                        section, column,
                    )));
                }
                _ => {}
            }
        }
    } else if response.hovered() {
        ret = Some(if column == 0 {
            DiffViewAction::SetSymbolHighlight(Some(symbol_idx), symbol_diff.target_symbol, false)
        } else {
            DiffViewAction::SetSymbolHighlight(symbol_diff.target_symbol, Some(symbol_idx), false)
        });
    }
    ret
}

fn find_prev_symbol(section_display: &[SectionDisplay], current: usize) -> Option<usize> {
    section_display
        .iter()
        .flat_map(|s| s.symbols.iter())
        .rev()
        .skip_while(|s| s.symbol != current)
        .nth(1)
        .map(|s| s.symbol)
        // Wrap around to the last symbol if we're at the beginning of the list
        .or_else(|| find_last_symbol(section_display))
}

fn find_next_symbol(section_display: &[SectionDisplay], current: usize) -> Option<usize> {
    section_display
        .iter()
        .flat_map(|s| s.symbols.iter())
        .skip_while(|s| s.symbol != current)
        .nth(1)
        .map(|s| s.symbol)
        // Wrap around to the first symbol if we're at the end of the list
        .or_else(|| find_first_symbol(section_display))
}

fn find_first_symbol(section_display: &[SectionDisplay]) -> Option<usize> {
    section_display.iter().flat_map(|s| s.symbols.iter()).next().map(|s| s.symbol)
}

fn find_last_symbol(section_display: &[SectionDisplay]) -> Option<usize> {
    section_display.iter().flat_map(|s| s.symbols.iter()).next_back().map(|s| s.symbol)
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
    open_sections: Option<bool>,
) -> Option<DiffViewAction> {
    let mut ret = None;
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        let mut show_mapped_symbols = state.show_mapped_symbols;
        if let SymbolFilter::Mapping(_, _) = filter {
            if ui.checkbox(&mut show_mapped_symbols, "Show mapped symbols").changed() {
                ret = Some(DiffViewAction::SetShowMappedSymbols(show_mapped_symbols));
            }
        }
        let section_display = display_sections(
            ctx.obj,
            ctx.diff,
            filter,
            state.show_hidden_symbols,
            show_mapped_symbols,
            state.reverse_fn_order,
        );

        hotkeys::check_scroll_hotkeys(ui, false);

        let mut new_key_value_to_highlight = None;
        if let Some(sym_ref) =
            if column == 0 { state.highlighted_symbol.0 } else { state.highlighted_symbol.1 }
        {
            let up = if hotkeys::consume_up_key(ui.ctx()) {
                Some(true)
            } else if hotkeys::consume_down_key(ui.ctx()) {
                Some(false)
            } else {
                None
            };
            if let Some(up) = up {
                new_key_value_to_highlight = if up {
                    find_prev_symbol(&section_display, sym_ref)
                } else {
                    find_next_symbol(&section_display, sym_ref)
                };
            };
        } else {
            // No symbol is highlighted in this column. Select the topmost symbol instead.
            // Note that we intentionally do not consume the up/down key presses in this case, but
            // we do when a symbol is highlighted. This is so that if only one column has a symbol
            // highlighted, that one takes precedence over the one with nothing highlighted.
            if hotkeys::up_pressed(ui.ctx()) || hotkeys::down_pressed(ui.ctx()) {
                new_key_value_to_highlight = find_first_symbol(&section_display);
            }
        }
        if let Some(new_sym_ref) = new_key_value_to_highlight {
            let target_symbol = ctx.diff.symbols[new_sym_ref].target_symbol;
            ret = Some(if column == 0 {
                DiffViewAction::SetSymbolHighlight(Some(new_sym_ref), target_symbol, true)
            } else {
                DiffViewAction::SetSymbolHighlight(target_symbol, Some(new_sym_ref), true)
            });
        }

        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

            for section_display in section_display {
                let mut header = LayoutJob::simple_singleline(
                    section_display.name.clone(),
                    appearance.code_font.clone(),
                    Color32::PLACEHOLDER,
                );
                if section_display.size > 0 {
                    write_text(
                        &format!(" ({:x})", section_display.size),
                        Color32::PLACEHOLDER,
                        &mut header,
                        appearance.code_font.clone(),
                    );
                }
                if let Some(match_percent) = section_display.match_percent {
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
                    .id_salt(Id::new(&section_display.id))
                    .default_open(true)
                    .open(open_sections)
                    .show(ui, |ui| {
                        for symbol_display in &section_display.symbols {
                            let symbol = &ctx.obj.symbols[symbol_display.symbol];
                            let section = symbol
                                .section
                                .and_then(|section_idx| ctx.obj.sections.get(section_idx));
                            let symbol_diff = if symbol_display.is_mapping_symbol {
                                ctx.diff
                                    .mapping_symbols
                                    .iter()
                                    .find(|d| d.symbol_index == symbol_display.symbol)
                                    .map(|d| &d.symbol_diff)
                                    .unwrap()
                            } else {
                                &ctx.diff.symbols[symbol_display.symbol]
                            };
                            if let Some(result) = symbol_ui(
                                ui,
                                ctx,
                                other_ctx,
                                symbol,
                                symbol_diff,
                                symbol_display.symbol,
                                section,
                                state,
                                appearance,
                                column,
                            ) {
                                ret = Some(result);
                            }
                        }
                    });
            }
        });
    });
    ret
}

#[derive(Copy, Clone)]
pub struct SymbolDiffContext<'a> {
    pub obj: &'a Object,
    pub diff: &'a ObjectDiff,
}
