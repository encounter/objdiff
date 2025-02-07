use egui::{Id, Layout, RichText, ScrollArea, TextEdit, Ui, Widget};
use objdiff_core::{
    build::BuildStatus,
    diff::{ObjDiff, ObjSectionDiff, ObjSymbolDiff},
    obj::{ObjInfo, ObjSection, ObjSectionKind, ObjSymbol, SymbolRef},
};
use time::format_description;

use crate::{
    hotkeys,
    views::{
        appearance::Appearance,
        column_layout::{render_header, render_strips, render_table},
        data_diff::{data_row_ui, split_diffs, BYTES_PER_ROW},
        extab_diff::extab_ui,
        function_diff::{asm_col_ui, FunctionDiffContext},
        symbol_diff::{
            match_color_for_symbol, symbol_list_ui, DiffViewAction, DiffViewNavigation,
            DiffViewState, SymbolDiffContext, SymbolFilter, SymbolRefByName, View,
        },
    },
};

#[derive(Clone, Copy)]
enum SelectedSymbol {
    Symbol(SymbolRef),
    Section(usize),
}

#[derive(Clone, Copy)]
struct DiffColumnContext<'a> {
    status: &'a BuildStatus,
    obj: Option<&'a (ObjInfo, ObjDiff)>,
    section: Option<(&'a ObjSection, &'a ObjSectionDiff)>,
    symbol: Option<(&'a ObjSymbol, &'a ObjSymbolDiff)>,
}

impl<'a> DiffColumnContext<'a> {
    pub fn new(
        view: View,
        status: &'a BuildStatus,
        obj: Option<&'a (ObjInfo, ObjDiff)>,
        selected_symbol: Option<&SymbolRefByName>,
    ) -> Self {
        let selected_symbol = match view {
            View::SymbolDiff => None,
            View::FunctionDiff | View::ExtabDiff => match (obj, selected_symbol) {
                (Some(obj), Some(s)) => find_symbol(&obj.0, s).map(SelectedSymbol::Symbol),
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
                let (section, symbol) = obj.section_symbol(symbol_ref);
                (
                    section.map(|s| (s, obj_diff.section_diff(symbol_ref.section_idx))),
                    Some((symbol, obj_diff.symbol_diff(symbol_ref))),
                )
            }
            (Some((obj, obj_diff)), Some(SelectedSymbol::Section(section_idx))) => {
                (Some((&obj.sections[section_idx], obj_diff.section_diff(section_idx))), None)
            }
            _ => (None, None),
        };
        Self { status, obj, section, symbol }
    }

    #[inline]
    pub fn has_symbol(&self) -> bool { self.section.is_some() || self.symbol.is_some() }

    #[inline]
    pub fn id(&self) -> Option<&str> {
        self.symbol
            .map(|(symbol, _)| symbol.name.as_str())
            .or_else(|| self.section.map(|(section, _)| section.name.as_str()))
    }
}

#[must_use]
pub fn diff_view_ui(
    ui: &mut Ui,
    state: &DiffViewState,
    appearance: &Appearance,
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
        view: state.current_view,
        left_symbol: state.symbol_state.left_symbol.clone(),
        right_symbol: state.symbol_state.right_symbol.clone(),
    };
    let mut navigation = current_navigation.clone();
    if let Some((_symbol, symbol_diff)) = left_ctx.symbol {
        // If a matching symbol appears, select it
        if !right_ctx.has_symbol() {
            if let Some(target_symbol_ref) = symbol_diff.target_symbol {
                let (target_section, target_symbol) =
                    right_ctx.obj.unwrap().0.section_symbol(target_symbol_ref);
                navigation.right_symbol = Some(SymbolRefByName::new(target_symbol, target_section));
            }
        }
    } else if navigation.left_symbol.is_some()
        && left_ctx.obj.is_some()
        && left_ctx.section.is_none()
    {
        // Clear selection if symbol goes missing
        navigation.left_symbol = None;
    }
    if let Some((_symbol, symbol_diff)) = right_ctx.symbol {
        // If a matching symbol appears, select it
        if !left_ctx.has_symbol() {
            if let Some(target_symbol_ref) = symbol_diff.target_symbol {
                let (target_section, target_symbol) =
                    left_ctx.obj.unwrap().0.section_symbol(target_symbol_ref);
                navigation.left_symbol = Some(SymbolRefByName::new(target_symbol, target_section));
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
        navigation.view = View::SymbolDiff;
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
                        ret = Some(DiffViewAction::Navigate(DiffViewNavigation::symbol_diff()));
                    }

                    if let Some((symbol, _)) = left_ctx.symbol {
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
            } else if let Some((symbol, _)) = left_ctx.symbol {
                ui.label(
                    RichText::new(symbol.demangled_name.as_deref().unwrap_or(&symbol.name))
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
            } else if let Some((section, _)) = left_ctx.section {
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
            } else if let Some((symbol, _)) = right_ctx.symbol {
                ui.label(
                    RichText::new(symbol.demangled_name.as_deref().unwrap_or(&symbol.name))
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
            } else if let Some((section, _)) = right_ctx.section {
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
                if let Some((_, symbol_diff)) = right_ctx.symbol {
                    if let Some(match_percent) = symbol_diff.match_percent {
                        ui.label(
                            RichText::new(format!("{:.0}%", match_percent.floor()))
                                .font(appearance.code_font.clone())
                                .color(match_color_for_symbol(match_percent, appearance)),
                        );
                    }
                    if state.current_view == View::FunctionDiff && left_ctx.has_symbol() {
                        ui.separator();
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
            Some((_, left_symbol_diff)),
            Some((_, right_symbol_diff)),
        ) = (state.current_view, left_ctx.obj, right_ctx.obj, left_ctx.symbol, right_ctx.symbol)
        {
            // Joint diff view
            hotkeys::check_scroll_hotkeys(ui, true);
            if left_symbol_diff.instructions.len() != right_symbol_diff.instructions.len() {
                ui.label("Instruction count mismatch");
                return;
            }
            let instructions_len = left_symbol_diff.instructions.len();
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
                                symbol_ref: Some(left_symbol_diff.symbol_ref),
                            },
                            appearance,
                            &state.function_state,
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
                                symbol_ref: Some(right_symbol_diff.symbol_ref),
                            },
                            appearance,
                            &state.function_state,
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
            Some((_left_section, left_section_diff)),
            Some((_right_section, right_section_diff)),
        ) =
            (state.current_view, left_ctx.obj, right_ctx.obj, left_ctx.section, right_ctx.section)
        {
            // Joint diff view
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
                            data_row_ui(ui, Some(left_obj), address, &left_diffs[i], appearance);
                        } else if column == 1 {
                            data_row_ui(ui, Some(right_obj), address, &right_diffs[i], appearance);
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
                    ) {
                        ret = Some(action);
                    }
                }
            });
        }
    });

    ret
}

#[must_use]
#[allow(clippy::too_many_arguments)]
fn diff_col_ui(
    ui: &mut Ui,
    state: &DiffViewState,
    appearance: &Appearance,
    column: usize,
    ctx: DiffColumnContext,
    other_ctx: DiffColumnContext,
    available_width: f32,
    open_sections: Option<bool>,
) -> Option<DiffViewAction> {
    let mut ret = None;
    if !ctx.status.success {
        build_log_ui(ui, ctx.status, appearance);
    } else if let Some((obj, diff)) = ctx.obj {
        if let Some((_symbol, symbol_diff)) = ctx.symbol {
            hotkeys::check_scroll_hotkeys(ui, false);
            let ctx = FunctionDiffContext { obj, diff, symbol_ref: Some(symbol_diff.symbol_ref) };
            if state.current_view == View::ExtabDiff {
                extab_ui(ui, ctx, appearance, column);
            } else {
                render_table(
                    ui,
                    available_width / 2.0,
                    1,
                    appearance.code_font.size,
                    symbol_diff.instructions.len(),
                    |row, column| {
                        if let Some(action) =
                            asm_col_ui(row, ctx, appearance, &state.function_state, column)
                        {
                            ret = Some(action);
                        }
                        if row.response().clicked() {
                            ret = Some(DiffViewAction::ClearDiffHighlight);
                        }
                    },
                );
            }
        } else if let Some((_section, section_diff)) = ctx.section {
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
                        data_row_ui(ui, Some(obj), address, &diffs[i], appearance);
                    });
                },
            );
        } else if let (
            Some((other_section, _other_section_diff)),
            Some((other_symbol, other_symbol_diff)),
        ) = (other_ctx.section, other_ctx.symbol)
        {
            if let Some(action) = symbol_list_ui(
                ui,
                SymbolDiffContext { obj, diff },
                None,
                &state.symbol_state,
                SymbolFilter::Mapping(other_symbol_diff.symbol_ref, None),
                appearance,
                column,
                open_sections,
            ) {
                match (column, action) {
                    (
                        0,
                        DiffViewAction::Navigate(DiffViewNavigation {
                            left_symbol: Some(left_symbol_ref),
                            ..
                        }),
                    ) => {
                        ret = Some(DiffViewAction::SetMapping(
                            match other_section.kind {
                                ObjSectionKind::Code => View::FunctionDiff,
                                _ => View::SymbolDiff,
                            },
                            left_symbol_ref,
                            SymbolRefByName::new(other_symbol, Some(other_section)),
                        ));
                    }
                    (
                        1,
                        DiffViewAction::Navigate(DiffViewNavigation {
                            right_symbol: Some(right_symbol_ref),
                            ..
                        }),
                    ) => {
                        ret = Some(DiffViewAction::SetMapping(
                            match other_section.kind {
                                ObjSectionKind::Code => View::FunctionDiff,
                                _ => View::SymbolDiff,
                            },
                            SymbolRefByName::new(other_symbol, Some(other_section)),
                            right_symbol_ref,
                        ));
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
                other_ctx.obj.map(|(obj, diff)| SymbolDiffContext { obj, diff }),
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

fn find_section(obj: &ObjInfo, section_name: &str) -> Option<usize> {
    obj.sections.iter().position(|section| section.name == section_name)
}
