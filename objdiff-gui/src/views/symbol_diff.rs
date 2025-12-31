use std::{mem::take};

use egui::{
    CollapsingHeader, Color32, Id, OpenUrl, ScrollArea, Ui, Widget, style::ScrollAnimation,
    text::LayoutJob,
};
use objdiff_core::{
    diff::{
        DiffObjConfig, ObjectDiff, ShowSymbolSizes, SymbolDiff,
        display::{
            HighlightKind, SectionDisplay, SymbolFilter, SymbolNavigationKind, display_sections,
            symbol_context, symbol_hover,
        },
    },
    jobs::{Job, JobQueue, JobResult, create_scratch::CreateScratchResult, objdiff::ObjDiffResult},
    obj::{Object, Section, SectionKind, Symbol, SymbolFlag},
};
use regex::{Regex, RegexBuilder};

use crate::{
    app::AppStateRef,
    hotkeys,
    jobs::{is_create_scratch_available, start_create_scratch},
    views::{
        appearance::Appearance,
        diff::{context_menu_items_ui, hover_items_ui},
        function_diff::FunctionViewState,
        write_text,
    },
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
    SetMapping(usize, usize),
    /// Set a batch of relocation mappings.
    SetRelocMappings(Vec<(String, String)>),
    /// Set the show_mapped_symbols flag
    SetShowMappedSymbols(bool),
    /// Set the show_data_flow flag
    SetShowDataFlow(bool),
    // Scrolls a row of the function view table into view.
    ScrollToRow(usize),
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct DiffViewNavigation {
    pub kind: SymbolNavigationKind,
    pub left_symbol: Option<usize>,
    pub right_symbol: Option<usize>,
}

impl DiffViewNavigation {
    pub fn new(kind: SymbolNavigationKind, symbol_idx: usize, column: usize) -> Self {
        match column {
            0 => Self { kind, left_symbol: Some(symbol_idx), right_symbol: None },
            1 => Self { kind, left_symbol: None, right_symbol: Some(symbol_idx) },
            _ => panic!("Invalid column index"),
        }
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ResolvedNavigation {
    pub view: View,
    pub left_symbol: Option<SymbolRefByName>,
    pub right_symbol: Option<SymbolRefByName>,
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
    pub post_build_nav: Option<ResolvedNavigation>,
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

                // Clear reload flag so that we don't reload the view immediately
                if let Ok(mut state) = state.write() {
                    state.queue_reload = false;
                }

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
            ctx.open_url(OpenUrl::new_tab(result.scratch_url));
        }

        // Clear the scroll flags to prevent it from scrolling continuously.
        self.symbol_state.autoscroll_to_highlighted_symbols = false;
        self.function_state.scroll_to_row = None;

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

                let mut resolved_left = self.resolve_symbol(nav.left_symbol, 0);
                let mut resolved_right = self.resolve_symbol(nav.right_symbol, 1);
                if let Some(resolved_right) = &resolved_right
                    && resolved_left.is_none()
                {
                    resolved_left = resolved_right
                        .target_symbol
                        .and_then(|idx| self.resolve_symbol(Some(idx), 0));
                }
                if let Some(resolved_left) = &resolved_left
                    && resolved_right.is_none()
                {
                    resolved_right = resolved_left
                        .target_symbol
                        .and_then(|idx| self.resolve_symbol(Some(idx), 1));
                }
                let resolved_nav = resolve_navigation(nav.kind, resolved_left, resolved_right);
                if (resolved_nav.left_symbol.is_some() && resolved_nav.right_symbol.is_some())
                    || (resolved_nav.left_symbol.is_none() && resolved_nav.right_symbol.is_none())
                {
                    // Regular navigation
                    if state.is_selecting_symbol() {
                        // Cancel selection and reload
                        state.clear_selection();
                        self.post_build_nav = Some(resolved_nav);
                    } else {
                        // Navigate immediately
                        self.current_view = resolved_nav.view;
                        self.symbol_state.left_symbol = resolved_nav.left_symbol;
                        self.symbol_state.right_symbol = resolved_nav.right_symbol;
                    }
                } else {
                    // Enter selection mode
                    match (&resolved_nav.left_symbol, &resolved_nav.right_symbol) {
                        (Some(left_ref), None) => {
                            state.set_selecting_right(&left_ref.symbol_name);
                        }
                        (None, Some(right_ref)) => {
                            state.set_selecting_left(&right_ref.symbol_name);
                        }
                        (Some(_), Some(_)) => unreachable!(),
                        (None, None) => unreachable!(),
                    }
                    self.post_build_nav = Some(resolved_nav);
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
                    log::info!("Opening file {source_path}");
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
                self.post_build_nav = Some(ResolvedNavigation {
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
                self.post_build_nav = Some(ResolvedNavigation {
                    view: View::FunctionDiff,
                    left_symbol: Some(left_ref),
                    right_symbol: None,
                });
            }
            DiffViewAction::SetMapping(left_ref, right_ref) => {
                if self.post_build_nav.is_some() {
                    // Ignore action if we're already navigating
                    return;
                }
                let Ok(mut state) = state.write() else {
                    return;
                };
                let resolved_nav = if let (Some(left_ref), Some(right_ref)) = (
                    self.resolve_symbol(Some(left_ref), 0),
                    self.resolve_symbol(Some(right_ref), 1),
                ) {
                    state.set_symbol_mapping(
                        left_ref.symbol.name.clone(),
                        right_ref.symbol.name.clone(),
                    );
                    resolve_navigation(
                        SymbolNavigationKind::Normal,
                        Some(left_ref),
                        Some(right_ref),
                    )
                } else {
                    ResolvedNavigation::default()
                };
                self.post_build_nav = Some(resolved_nav);
            }
            DiffViewAction::SetRelocMappings(mappings) => {
                let Ok(mut state) = state.write() else {
                    return;
                };
                for (left_name, right_name) in mappings {
                    state.set_symbol_mapping(left_name, right_name);
                }
            }
            DiffViewAction::SetShowMappedSymbols(value) => {
                self.symbol_state.show_mapped_symbols = value;
            }
            DiffViewAction::SetShowDataFlow(value) => {
                let Ok(mut state) = state.write() else {
                    return;
                };
                state.config.diff_obj_config.show_data_flow = value;
            }
            DiffViewAction::ScrollToRow(row) => {
                self.function_state.scroll_to_row = Some(row);
            }
        }
    }

    fn resolve_symbol(
        &self,
        symbol_idx: Option<usize>,
        column: usize,
    ) -> Option<ResolvedSymbol<'_>> {
        let symbol_idx = symbol_idx?;
        let result = self.build.as_deref()?;
        let (obj, diff) = match column {
            0 => result.first_obj.as_ref()?,
            1 => result.second_obj.as_ref()?,
            _ => return None,
        };
        let symbol = obj.symbols.get(symbol_idx)?;
        let section_idx = symbol.section?;
        let section = obj.sections.get(section_idx)?;
        let symbol_diff = diff.symbols.get(symbol_idx)?;
        Some(ResolvedSymbol {
            symbol_ref: SymbolRefByName::new(symbol, Some(section)),
            symbol,
            section,
            target_symbol: symbol_diff.target_symbol,
        })
    }
}

struct ResolvedSymbol<'obj> {
    symbol_ref: SymbolRefByName,
    symbol: &'obj Symbol,
    section: &'obj Section,
    target_symbol: Option<usize>,
}

/// Determine the navigation target based on the resolved symbols.
fn resolve_navigation(
    kind: SymbolNavigationKind,
    resolved_left: Option<ResolvedSymbol>,
    resolved_right: Option<ResolvedSymbol>,
) -> ResolvedNavigation {
    match (resolved_left, resolved_right) {
        (Some(left), Some(right)) => match (left.section.kind, right.section.kind) {
            (SectionKind::Code, SectionKind::Code) => ResolvedNavigation {
                view: match kind {
                    SymbolNavigationKind::Normal => View::FunctionDiff,
                    SymbolNavigationKind::Extab => View::ExtabDiff,
                },
                left_symbol: Some(left.symbol_ref),
                right_symbol: Some(right.symbol_ref),
            },
            (SectionKind::Data, SectionKind::Data) => ResolvedNavigation {
                view: View::DataDiff,
                left_symbol: Some(left.symbol_ref),
                right_symbol: Some(right.symbol_ref),
            },
            _ => ResolvedNavigation::default(),
        },
        (Some(left), None) => match left.section.kind {
            SectionKind::Code => ResolvedNavigation {
                view: match kind {
                    SymbolNavigationKind::Normal => View::FunctionDiff,
                    SymbolNavigationKind::Extab => View::ExtabDiff,
                },
                left_symbol: Some(left.symbol_ref),
                right_symbol: None,
            },
            SectionKind::Data => ResolvedNavigation {
                view: View::DataDiff,
                left_symbol: Some(left.symbol_ref),
                right_symbol: None,
            },
            _ => ResolvedNavigation::default(),
        },
        (None, Some(right)) => match right.section.kind {
            SectionKind::Code => ResolvedNavigation {
                view: match kind {
                    SymbolNavigationKind::Normal => View::FunctionDiff,
                    SymbolNavigationKind::Extab => View::ExtabDiff,
                },
                left_symbol: None,
                right_symbol: Some(right.symbol_ref),
            },
            SectionKind::Data => ResolvedNavigation {
                view: View::DataDiff,
                left_symbol: None,
                right_symbol: Some(right.symbol_ref),
            },
            _ => ResolvedNavigation::default(),
        },
        (None, None) => ResolvedNavigation::default(),
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

pub fn symbol_context_menu_ui(
    ui: &mut Ui,
    ctx: SymbolDiffContext<'_>,
    symbol_idx: usize,
    symbol: &Symbol,
    section: Option<&Section>,
    column: usize,
    appearance: &Appearance,
) -> Option<DiffViewAction> {
    let mut ret = None;
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);

        if let Some(action) =
            context_menu_items_ui(ui, symbol_context(ctx.obj, symbol_idx), column, appearance)
        {
            ret = Some(action);
        }

        if let Some(section) = section
            && ui.button("Map symbol").clicked()
        {
            let symbol_ref = SymbolRefByName::new(symbol, Some(section));
            if column == 0 {
                ret = Some(DiffViewAction::SelectingRight(symbol_ref));
            } else {
                ret = Some(DiffViewAction::SelectingLeft(symbol_ref));
            }
            ui.close();
        }
    });
    ret
}

pub fn symbol_hover_ui(
    ui: &mut Ui,
    ctx: SymbolDiffContext<'_>,
    symbol_idx: usize,
    appearance: &Appearance,
) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        hover_items_ui(ui, symbol_hover(ctx.obj, symbol_idx, 0, None), appearance);
    });
}

#[must_use]
fn symbol_ui(
    ui: &mut Ui,
    ctx: SymbolDiffContext<'_>,
    symbol: &Symbol,
    symbol_diff: &SymbolDiff,
    symbol_idx: usize,
    section: Option<&Section>,
    state: &SymbolViewState,
    appearance: &Appearance,
    column: usize,
    diff_config: &DiffObjConfig,
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
    if diff_config.show_symbol_sizes == ShowSymbolSizes::Decimal {
        write_text(
            &format!(" (size={})", symbol.size),
            appearance.deemphasized_text_color,
            &mut job,
            appearance.code_font.clone(),
        );
    } else if diff_config.show_symbol_sizes == ShowSymbolSizes::Hex {
        write_text(
            &format!(" (size={:x})", symbol.size),
            appearance.deemphasized_text_color,
            &mut job,
            appearance.code_font.clone(),
        );
    }
    let response = egui::Button::selectable(selected, job)
        .ui(ui)
        .on_hover_ui_at_pointer(|ui| symbol_hover_ui(ui, ctx, symbol_idx, appearance));
    response.context_menu(|ui| {
        if let Some(result) =
            symbol_context_menu_ui(ui, ctx, symbol_idx, symbol, section, column, appearance)
        {
            ret = Some(result);
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
        ret = Some(DiffViewAction::Navigate(DiffViewNavigation::new(
            SymbolNavigationKind::Normal,
            symbol_idx,
            column,
        )));
    } else if response.hovered() {
        let new_highlighted_symbol = if column == 0 {
            (Some(symbol_idx), symbol_diff.target_symbol)
        } else {
            (symbol_diff.target_symbol, Some(symbol_idx))
        };
        // Only set the highlight if it changed from the previous frame.
        // This prevents passive mouse hovers from overriding keyboard actions.
        if new_highlighted_symbol != state.highlighted_symbol {
            ret = Some(DiffViewAction::SetSymbolHighlight(
                new_highlighted_symbol.0,
                new_highlighted_symbol.1,
                false,
            ));
        }
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
    state: &SymbolViewState,
    filter: SymbolFilter<'_>,
    appearance: &Appearance,
    column: usize,
    open_sections: Option<bool>,
    diff_config: &DiffObjConfig,
) -> Option<DiffViewAction> {
    let mut ret = None;
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        let mut show_mapped_symbols = state.show_mapped_symbols;
        if let SymbolFilter::Mapping(_, _) = filter
            && ui.checkbox(&mut show_mapped_symbols, "Show mapped symbols").changed()
        {
            ret = Some(DiffViewAction::SetShowMappedSymbols(show_mapped_symbols));
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
                                symbol,
                                symbol_diff,
                                symbol_display.symbol,
                                section,
                                state,
                                appearance,
                                column,
                                diff_config,
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
