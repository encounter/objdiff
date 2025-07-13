use egui::{Id, Layout, RichText, ScrollArea, TextEdit, Ui, Widget, text::LayoutJob};
use objdiff_core::{
    build::BuildStatus,
    diff::{
        DiffObjConfig, ObjectDiff, SectionDiff, SymbolDiff,
        display::{ContextItem, HoverItem, HoverItemColor, SymbolFilter, SymbolNavigationKind},
    },
    obj::{Object, Section, Symbol},
};
use time::format_description;

use crate::{
    hotkeys,
    views::{
        appearance::Appearance,
        column_layout::{render_header, render_strips, render_table},
        data_diff::{BYTES_PER_ROW, data_row_ui, split_diffs},
        extab_diff::extab_ui,
        function_diff::{FunctionDiffContext, asm_col_ui},
        symbol_diff::{
            DiffViewAction, DiffViewNavigation, DiffViewState, SymbolDiffContext, SymbolRefByName,
            View, match_color_for_symbol, symbol_context_menu_ui, symbol_hover_ui, symbol_list_ui,
        },
        write_text,
    },
};

#[derive(Clone, Copy)]
enum SelectedSymbol {
    Symbol(usize),
    Section(usize),
}

#[derive(Clone, Copy)]
struct DiffColumnContext<'a> {
    status: &'a BuildStatus,
    obj: Option<&'a (Object, ObjectDiff)>,
    section: Option<(&'a Section, &'a SectionDiff, usize)>,
    symbol: Option<(&'a Symbol, &'a SymbolDiff, usize)>,
}

impl<'a> DiffColumnContext<'a> {
    pub fn new(
        view: View,
        status: &'a BuildStatus,
        obj: Option<&'a (Object, ObjectDiff)>,
        selected_symbol: Option<&SymbolRefByName>,
    ) -> Self {
        let selected_symbol = match view {
            View::SymbolDiff => None,
            View::FunctionDiff | View::ExtabDiff => match (obj, selected_symbol) {
                (Some(obj), Some(s)) => {
                    obj.0.symbol_by_name(&s.symbol_name).map(SelectedSymbol::Symbol)
                }
                _ => None,
            },
            View::DataDiff => match (obj, selected_symbol) {
                (Some(obj), Some(SymbolRefByName { section_name: Some(section_name), .. })) => {
                    find_section(&obj.0, section_name).map(SelectedSymbol::Section)
                }
                _ => None,
            },
        };
        let (section, symbol) = match (obj, selected_symbol) {
            (Some((obj, obj_diff)), Some(SelectedSymbol::Symbol(symbol_ref))) => {
                let symbol = &obj.symbols[symbol_ref];
                (
                    symbol.section.map(|section_idx| {
                        (&obj.sections[section_idx], &obj_diff.sections[section_idx], section_idx)
                    }),
                    Some((symbol, &obj_diff.symbols[symbol_ref], symbol_ref)),
                )
            }
            (Some((obj, obj_diff)), Some(SelectedSymbol::Section(section_idx))) => (
                Some((&obj.sections[section_idx], &obj_diff.sections[section_idx], section_idx)),
                None,
            ),
            _ => (None, None),
        };
        Self { status, obj, section, symbol }
    }

    #[inline]
    pub fn has_symbol(&self) -> bool { self.section.is_some() || self.symbol.is_some() }

    #[inline]
    pub fn id(&self) -> Option<&str> {
        self.symbol
            .map(|(symbol, _, _)| symbol.name.as_str())
            .or_else(|| self.section.map(|(section, _, _)| section.name.as_str()))
    }
}

#[must_use]
pub fn diff_view_ui(
    ui: &mut Ui,
    state: &DiffViewState,
    appearance: &Appearance,
    diff_config: &DiffObjConfig,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let Some(result) = &state.build else {
        return ret;
    };

    let left_ctx = DiffColumnContext::new(
        state.current_view,
        &result.first_status,
        result.first_obj.as_ref(),
        state.symbol_state.left_symbol.as_ref(),
    );
    let right_ctx = DiffColumnContext::new(
        state.current_view,
        &result.second_status,
        result.second_obj.as_ref(),
        state.symbol_state.right_symbol.as_ref(),
    );

    // Check if we need to perform any navigation
    let current_navigation = DiffViewNavigation {
        kind: match state.current_view {
            View::ExtabDiff => SymbolNavigationKind::Extab,
            _ => SymbolNavigationKind::Normal,
        },
        left_symbol: left_ctx.symbol.map(|(_, _, idx)| idx),
        right_symbol: right_ctx.symbol.map(|(_, _, idx)| idx),
    };
    let mut navigation = current_navigation.clone();
    if let Some((_symbol, symbol_diff, _symbol_idx)) = left_ctx.symbol {
        // If a matching symbol appears, select it
        if !right_ctx.has_symbol() {
            if let Some(target_symbol_ref) = symbol_diff.target_symbol {
                navigation.right_symbol = Some(target_symbol_ref);
            }
        }
    } else if navigation.left_symbol.is_some()
        && left_ctx.obj.is_some()
        && left_ctx.section.is_none()
    {
        // Clear selection if symbol goes missing
        navigation.left_symbol = None;
    }
    if let Some((_symbol, symbol_diff, _symbol_idx)) = right_ctx.symbol {
        // If a matching symbol appears, select it
        if !left_ctx.has_symbol() {
            if let Some(target_symbol_ref) = symbol_diff.target_symbol {
                navigation.left_symbol = Some(target_symbol_ref);
            }
        }
    } else if navigation.right_symbol.is_some()
        && right_ctx.obj.is_some()
        && right_ctx.section.is_none()
    {
        // Clear selection if symbol goes missing
        navigation.right_symbol = None;
    }
    // If both sides are missing a symbol, switch to symbol diff view
    if navigation.left_symbol.is_none() && navigation.right_symbol.is_none() {
        navigation = DiffViewNavigation::default();
    }
    // Execute navigation if it changed
    if navigation != current_navigation && state.post_build_nav.is_none() {
        ret = Some(DiffViewAction::Navigate(navigation));
    }

    let available_width = ui.available_width();
    let mut open_sections = (None, None);

    render_header(ui, available_width, 2, |ui, column| {
        if column == 0 {
            // Left column

            // First row
            if state.current_view == View::SymbolDiff {
                ui.label(RichText::new("Target object").text_style(egui::TextStyle::Monospace));
            } else {
                ui.horizontal(|ui| {
                    if ui.button("⏴ Back").clicked() || hotkeys::back_pressed(ui.ctx()) {
                        ret = Some(DiffViewAction::Navigate(DiffViewNavigation::default()));
                    }

                    if let Some((symbol, _, _)) = left_ctx.symbol {
                        ui.separator();
                        if ui
                            .add_enabled(
                                !state.scratch_running
                                    && state.scratch_available
                                    && left_ctx.has_symbol(),
                                egui::Button::new("📲 decomp.me"),
                            )
                            .on_hover_text_at_pointer("Create a new scratch on decomp.me (beta)")
                            .on_disabled_hover_text("Scratch configuration missing")
                            .clicked()
                        {
                            ret = Some(DiffViewAction::CreateScratch(symbol.name.clone()));
                        }
                    }
                });
            }

            // Second row
            if !left_ctx.status.success {
                ui.label(
                    RichText::new("Fail")
                        .font(appearance.code_font.clone())
                        .color(appearance.delete_color),
                );
            } else if state.current_view == View::SymbolDiff {
                if left_ctx.obj.is_some() {
                    ui.label(
                        RichText::new(state.object_name.clone())
                            .font(appearance.code_font.clone())
                            .color(appearance.highlight_color),
                    );
                } else {
                    ui.label(
                        RichText::new("Missing")
                            .font(appearance.code_font.clone())
                            .color(appearance.replace_color),
                    );
                }
            } else if let Some((symbol, _symbol_diff, symbol_idx)) = left_ctx.symbol {
                if let Some(action) =
                    symbol_label_ui(ui, left_ctx, symbol, symbol_idx, column, appearance)
                {
                    ret = Some(action);
                }
            } else if let Some((section, _, _)) = left_ctx.section {
                ui.label(
                    RichText::new(section.name.clone())
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
            } else if right_ctx.has_symbol() {
                ui.label(
                    RichText::new("Choose target symbol")
                        .font(appearance.code_font.clone())
                        .color(appearance.replace_color),
                );
            } else {
                ui.label(
                    RichText::new("Missing")
                        .font(appearance.code_font.clone())
                        .color(appearance.replace_color),
                );
            }

            // Third row
            if left_ctx.has_symbol() && right_ctx.has_symbol() {
                if state.current_view == View::FunctionDiff
                    && ui
                        .button("Change target")
                        .on_hover_text_at_pointer("Choose a different symbol to use as the target")
                        .clicked()
                    || hotkeys::consume_change_target_shortcut(ui.ctx())
                {
                    if let Some(symbol_ref) = state.symbol_state.right_symbol.as_ref() {
                        ret = Some(DiffViewAction::SelectingLeft(symbol_ref.clone()));
                    }
                }
            } else if left_ctx.status.success && !left_ctx.has_symbol() {
                ui.horizontal(|ui| {
                    let mut search = state.search.clone();
                    let response =
                        TextEdit::singleline(&mut search).hint_text("Filter symbols").ui(ui);
                    if hotkeys::consume_symbol_filter_shortcut(ui.ctx()) {
                        response.request_focus();
                    }
                    if response.changed() {
                        ret = Some(DiffViewAction::SetSearch(search));
                    }

                    ui.with_layout(Layout::right_to_left(egui::Align::TOP), |ui| {
                        if ui.small_button("⏷").on_hover_text_at_pointer("Expand all").clicked() {
                            open_sections.0 = Some(true);
                        }
                        if ui.small_button("⏶").on_hover_text_at_pointer("Collapse all").clicked()
                        {
                            open_sections.0 = Some(false);
                        }
                    })
                });
            }

            // Only need to check the first Object. Technically the first could not have a flow analysis
            // result while the second does but we don't want to waste space on two separate checkboxes.
            if state.current_view == View::FunctionDiff
                && result
                    .first_obj
                    .as_ref()
                    .is_some_and(|(first, _)| first.has_flow_analysis_result())
            {
                let mut value = diff_config.show_data_flow;
                if ui
                    .checkbox(&mut value, "Show data flow")
                    .on_hover_text("Show data flow analysis results in place of register names")
                    .clicked()
                {
                    ret = Some(DiffViewAction::SetShowDataFlow(value));
                }
            }
        } else if column == 1 {
            // Right column

            // First row
            ui.horizontal(|ui| {
                if ui.add_enabled(!state.build_running, egui::Button::new("Build")).clicked() {
                    ret = Some(DiffViewAction::Build);
                }
                if state.build_running {
                    ui.colored_label(
                        appearance.replace_color,
                        RichText::new("Building…").text_style(egui::TextStyle::Monospace),
                    );
                } else {
                    ui.label(RichText::new("Last built:").text_style(egui::TextStyle::Monospace));
                    let format = format_description::parse("[hour]:[minute]:[second]").unwrap();
                    ui.label(
                        RichText::new(
                            result.time.to_offset(appearance.utc_offset).format(&format).unwrap(),
                        )
                        .text_style(egui::TextStyle::Monospace),
                    );
                }
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

            // Second row
            if !right_ctx.status.success {
                ui.label(
                    RichText::new("Fail")
                        .font(appearance.code_font.clone())
                        .color(appearance.delete_color),
                );
            } else if state.current_view == View::SymbolDiff {
                if right_ctx.obj.is_some() {
                    if left_ctx.obj.is_some() {
                        ui.label(RichText::new("Base object").font(appearance.code_font.clone()));
                    } else {
                        ui.label(
                            RichText::new(state.object_name.clone())
                                .font(appearance.code_font.clone())
                                .color(appearance.highlight_color),
                        );
                    }
                } else {
                    ui.label(
                        RichText::new("Missing")
                            .font(appearance.code_font.clone())
                            .color(appearance.replace_color),
                    );
                }
            } else if let Some((symbol, _symbol_diff, symbol_idx)) = right_ctx.symbol {
                if let Some(action) =
                    symbol_label_ui(ui, right_ctx, symbol, symbol_idx, column, appearance)
                {
                    ret = Some(action);
                }
            } else if let Some((section, _, _)) = right_ctx.section {
                ui.label(
                    RichText::new(section.name.clone())
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
            } else if left_ctx.has_symbol() {
                ui.label(
                    RichText::new("Choose base symbol")
                        .font(appearance.code_font.clone())
                        .color(appearance.replace_color),
                );
            } else {
                ui.label(
                    RichText::new("Missing")
                        .font(appearance.code_font.clone())
                        .color(appearance.replace_color),
                );
            }

            // Third row
            ui.horizontal(|ui| {
                if let Some((_, symbol_diff, _symbol_idx)) = right_ctx.symbol {
                    let mut needs_separator = false;
                    if let Some(match_percent) = symbol_diff.match_percent {
                        let response = ui.label(
                            RichText::new(format!("{match_percent:.2}%"))
                                .font(appearance.code_font.clone())
                                .color(match_color_for_symbol(match_percent, appearance)),
                        );
                        if let Some((diff_score, max_score)) = symbol_diff.diff_score {
                            response.on_hover_ui_at_pointer(|ui| {
                                ui.label(
                                    RichText::new(format!("Score: {diff_score}/{max_score}"))
                                        .font(appearance.code_font.clone())
                                        .color(appearance.text_color),
                                );
                            });
                        }
                        needs_separator = true;
                    }
                    if state.current_view == View::FunctionDiff && left_ctx.has_symbol() {
                        if needs_separator {
                            ui.separator();
                        }
                        if ui
                            .button("Change base")
                            .on_hover_text_at_pointer(
                                "Choose a different symbol to use as the base",
                            )
                            .clicked()
                            || hotkeys::consume_change_base_shortcut(ui.ctx())
                        {
                            if let Some(symbol_ref) = state.symbol_state.left_symbol.as_ref() {
                                ret = Some(DiffViewAction::SelectingRight(symbol_ref.clone()));
                            }
                        }
                    }
                } else if right_ctx.status.success && !right_ctx.has_symbol() {
                    let mut search = state.search.clone();
                    let response =
                        TextEdit::singleline(&mut search).hint_text("Filter symbols").ui(ui);
                    if hotkeys::consume_symbol_filter_shortcut(ui.ctx()) {
                        response.request_focus();
                    }
                    if response.changed() {
                        ret = Some(DiffViewAction::SetSearch(search));
                    }

                    ui.with_layout(Layout::right_to_left(egui::Align::TOP), |ui| {
                        if ui.small_button("⏷").on_hover_text_at_pointer("Expand all").clicked() {
                            open_sections.1 = Some(true);
                        }
                        if ui.small_button("⏶").on_hover_text_at_pointer("Collapse all").clicked()
                        {
                            open_sections.1 = Some(false);
                        }
                    });
                }
            });
        }
    });

    // Table
    ui.push_id(Id::new(left_ctx.id()).with(right_ctx.id()), |ui| {
        if let (
            View::FunctionDiff,
            Some((left_obj, left_diff)),
            Some((right_obj, right_diff)),
            Some((_, left_symbol_diff, left_symbol_idx)),
            Some((_, right_symbol_diff, right_symbol_idx)),
        ) = (state.current_view, left_ctx.obj, right_ctx.obj, left_ctx.symbol, right_ctx.symbol)
        {
            // Joint diff view
            hotkeys::check_scroll_hotkeys(ui, true);
            if left_symbol_diff.instruction_rows.len() != right_symbol_diff.instruction_rows.len() {
                ui.label("Instruction count mismatch");
                return;
            }
            let instructions_len = left_symbol_diff.instruction_rows.len();
            render_table(
                ui,
                available_width,
                2,
                appearance.code_font.size,
                instructions_len,
                |row, column| {
                    if column == 0 {
                        if let Some(action) = asm_col_ui(
                            row,
                            FunctionDiffContext {
                                obj: left_obj,
                                diff: left_diff,
                                symbol_ref: Some(left_symbol_idx),
                            },
                            appearance,
                            &state.function_state,
                            diff_config,
                            column,
                        ) {
                            ret = Some(action);
                        }
                    } else if column == 1 {
                        if let Some(action) = asm_col_ui(
                            row,
                            FunctionDiffContext {
                                obj: right_obj,
                                diff: right_diff,
                                symbol_ref: Some(right_symbol_idx),
                            },
                            appearance,
                            &state.function_state,
                            diff_config,
                            column,
                        ) {
                            ret = Some(action);
                        }
                        if row.response().clicked() {
                            ret = Some(DiffViewAction::ClearDiffHighlight);
                        }
                    }
                },
            );
        } else if let (
            View::DataDiff,
            Some((left_obj, _left_diff)),
            Some((right_obj, _right_diff)),
            Some((_left_section, left_section_diff, _left_symbol_idx)),
            Some((_right_section, right_section_diff, _right_symbol_idx)),
        ) =
            (state.current_view, left_ctx.obj, right_ctx.obj, left_ctx.section, right_ctx.section)
        {
            // Joint diff view
            hotkeys::check_scroll_hotkeys(ui, true);
            let left_total_bytes =
                left_section_diff.data_diff.iter().fold(0usize, |accum, item| accum + item.len);
            let right_total_bytes =
                right_section_diff.data_diff.iter().fold(0usize, |accum, item| accum + item.len);
            if left_total_bytes != right_total_bytes {
                ui.label("Data size mismatch");
                return;
            }
            if left_total_bytes == 0 {
                return;
            }
            let total_rows = (left_total_bytes - 1) / BYTES_PER_ROW + 1;
            let left_diffs =
                split_diffs(&left_section_diff.data_diff, &left_section_diff.reloc_diff);
            let right_diffs =
                split_diffs(&right_section_diff.data_diff, &right_section_diff.reloc_diff);
            render_table(
                ui,
                available_width,
                2,
                appearance.code_font.size,
                total_rows,
                |row, column| {
                    let i = row.index();
                    let address = i * BYTES_PER_ROW;
                    row.col(|ui| {
                        if column == 0 {
                            data_row_ui(
                                ui,
                                Some(left_obj),
                                address,
                                &left_diffs[i],
                                appearance,
                                column,
                            );
                        } else if column == 1 {
                            data_row_ui(
                                ui,
                                Some(right_obj),
                                address,
                                &right_diffs[i],
                                appearance,
                                column,
                            );
                        }
                    });
                },
            );
        } else {
            // Split view
            render_strips(ui, available_width, 2, |ui, column| {
                if column == 0 {
                    if let Some(action) = diff_col_ui(
                        ui,
                        state,
                        appearance,
                        column,
                        left_ctx,
                        right_ctx,
                        available_width,
                        open_sections.0,
                        diff_config,
                    ) {
                        ret = Some(action);
                    }
                } else if column == 1 {
                    if let Some(action) = diff_col_ui(
                        ui,
                        state,
                        appearance,
                        column,
                        right_ctx,
                        left_ctx,
                        available_width,
                        open_sections.1,
                        diff_config,
                    ) {
                        ret = Some(action);
                    }
                }
            });
        }
    });

    ret
}

fn symbol_label_ui(
    ui: &mut Ui,
    ctx: DiffColumnContext,
    symbol: &Symbol,
    symbol_idx: usize,
    column: usize,
    appearance: &Appearance,
) -> Option<DiffViewAction> {
    let (obj, diff) = ctx.obj.unwrap();
    let ctx = SymbolDiffContext { obj, diff };
    let mut ret = None;
    egui::Label::new(
        RichText::new(symbol.demangled_name.as_deref().unwrap_or(&symbol.name))
            .font(appearance.code_font.clone())
            .color(appearance.highlight_color),
    )
    .selectable(false)
    // TODO .show_tooltip_when_elided(false)
    // https://github.com/emilk/egui/commit/071e090e2b2601e5ed4726a63a753188503dfaf2
    .ui(ui)
    .on_hover_ui_at_pointer(|ui| symbol_hover_ui(ui, ctx, symbol_idx, appearance))
    .context_menu(|ui| {
        let section = symbol.section.and_then(|section_idx| ctx.obj.sections.get(section_idx));
        if let Some(result) =
            symbol_context_menu_ui(ui, ctx, symbol_idx, symbol, section, column, appearance)
        {
            ret = Some(result);
        }
    });
    ret
}

#[must_use]
fn diff_col_ui(
    ui: &mut Ui,
    state: &DiffViewState,
    appearance: &Appearance,
    column: usize,
    ctx: DiffColumnContext,
    other_ctx: DiffColumnContext,
    available_width: f32,
    open_sections: Option<bool>,
    diff_config: &DiffObjConfig,
) -> Option<DiffViewAction> {
    let mut ret = None;
    if !ctx.status.success {
        build_log_ui(ui, ctx.status, appearance);
    } else if let Some((obj, diff)) = ctx.obj {
        if let Some((_symbol, symbol_diff, symbol_idx)) = ctx.symbol {
            hotkeys::check_scroll_hotkeys(ui, false);
            let ctx = FunctionDiffContext { obj, diff, symbol_ref: Some(symbol_idx) };
            if state.current_view == View::ExtabDiff {
                extab_ui(ui, ctx, appearance, column);
            } else {
                render_table(
                    ui,
                    available_width / 2.0,
                    1,
                    appearance.code_font.size,
                    symbol_diff.instruction_rows.len(),
                    |row, column| {
                        if let Some(action) = asm_col_ui(
                            row,
                            ctx,
                            appearance,
                            &state.function_state,
                            diff_config,
                            column,
                        ) {
                            ret = Some(action);
                        }
                        if row.response().clicked() {
                            ret = Some(DiffViewAction::ClearDiffHighlight);
                        }
                    },
                );
            }
        } else if let Some((_section, section_diff, _section_idx)) = ctx.section {
            hotkeys::check_scroll_hotkeys(ui, false);
            let total_bytes =
                section_diff.data_diff.iter().fold(0usize, |accum, item| accum + item.len);
            if total_bytes == 0 {
                return ret;
            }
            let total_rows = (total_bytes - 1) / BYTES_PER_ROW + 1;
            let diffs = split_diffs(&section_diff.data_diff, &section_diff.reloc_diff);
            render_table(
                ui,
                available_width / 2.0,
                1,
                appearance.code_font.size,
                total_rows,
                |row, _column| {
                    let i = row.index();
                    let address = i * BYTES_PER_ROW;
                    row.col(|ui| {
                        data_row_ui(ui, Some(obj), address, &diffs[i], appearance, column);
                    });
                },
            );
        } else if let Some((_other_symbol, _other_symbol_diff, other_symbol_idx)) = other_ctx.symbol
        {
            if let Some(action) = symbol_list_ui(
                ui,
                SymbolDiffContext { obj, diff },
                &state.symbol_state,
                SymbolFilter::Mapping(other_symbol_idx, None),
                appearance,
                column,
                open_sections,
            ) {
                match (column, action) {
                    (
                        0,
                        DiffViewAction::Navigate(DiffViewNavigation {
                            left_symbol: Some(symbol_idx),
                            ..
                        }),
                    ) => {
                        ret = Some(DiffViewAction::SetMapping(symbol_idx, other_symbol_idx));
                    }
                    (
                        1,
                        DiffViewAction::Navigate(DiffViewNavigation {
                            right_symbol: Some(symbol_idx),
                            ..
                        }),
                    ) => {
                        ret = Some(DiffViewAction::SetMapping(other_symbol_idx, symbol_idx));
                    }
                    (_, action) => {
                        ret = Some(action);
                    }
                }
            }
        } else {
            let filter = match &state.search_regex {
                Some(regex) => SymbolFilter::Search(regex),
                _ => SymbolFilter::None,
            };
            if let Some(result) = symbol_list_ui(
                ui,
                SymbolDiffContext { obj, diff },
                &state.symbol_state,
                filter,
                appearance,
                column,
                open_sections,
            ) {
                ret = Some(result);
            }
        }
    } else {
        missing_obj_ui(ui, appearance);
    }

    ret
}

fn build_log_ui(ui: &mut Ui, status: &BuildStatus, appearance: &Appearance) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            if !status.cmdline.is_empty() && ui.button("Copy command").clicked() {
                ui.ctx().copy_text(status.cmdline.clone());
            }
            if ui.button("Copy log").clicked() {
                ui.ctx().copy_text(format!("{}\n{}", status.stdout, status.stderr));
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

fn find_section(obj: &Object, section_name: &str) -> Option<usize> {
    obj.sections.iter().position(|section| section.name == section_name)
}

pub fn hover_items_ui(ui: &mut Ui, items: Vec<HoverItem>, appearance: &Appearance) {
    for item in items {
        match item {
            HoverItem::Text { label, value, color } => {
                let mut job = LayoutJob::default();
                if !label.is_empty() {
                    let label_color = match color {
                        HoverItemColor::Special => appearance.replace_color,
                        HoverItemColor::Delete => appearance.delete_color,
                        HoverItemColor::Insert => appearance.insert_color,
                        _ => appearance.highlight_color,
                    };
                    write_text(&label, label_color, &mut job, appearance.code_font.clone());
                    write_text(": ", label_color, &mut job, appearance.code_font.clone());
                }
                write_text(
                    &value,
                    match color {
                        HoverItemColor::Emphasized => appearance.highlight_color,
                        _ => appearance.text_color,
                    },
                    &mut job,
                    appearance.code_font.clone(),
                );
                ui.label(job);
            }
            HoverItem::Separator => {
                ui.separator();
            }
        }
    }
}

pub fn context_menu_items_ui(
    ui: &mut Ui,
    items: Vec<ContextItem>,
    column: usize,
    appearance: &Appearance,
) -> Option<DiffViewAction> {
    let mut ret = None;
    for item in items {
        match item {
            ContextItem::Copy { value, label } => {
                let mut job = LayoutJob::default();
                write_text(
                    "Copy \"",
                    appearance.text_color,
                    &mut job,
                    appearance.code_font.clone(),
                );
                write_text(
                    &value,
                    appearance.highlight_color,
                    &mut job,
                    appearance.code_font.clone(),
                );
                write_text("\"", appearance.text_color, &mut job, appearance.code_font.clone());
                if let Some(label) = label {
                    write_text(" (", appearance.text_color, &mut job, appearance.code_font.clone());
                    write_text(
                        &label,
                        appearance.text_color,
                        &mut job,
                        appearance.code_font.clone(),
                    );
                    write_text(")", appearance.text_color, &mut job, appearance.code_font.clone());
                }
                if ui.button(job).clicked() {
                    ui.ctx().copy_text(value);
                    ui.close_menu();
                }
            }
            ContextItem::Navigate { label, symbol_index, kind } => {
                if ui.button(label).clicked() {
                    ret = Some(DiffViewAction::Navigate(DiffViewNavigation::new(
                        kind,
                        symbol_index,
                        column,
                    )));
                    ui.close_menu();
                }
            }
            ContextItem::Separator => {
                ui.separator();
            }
        }
    }
    ret
}
