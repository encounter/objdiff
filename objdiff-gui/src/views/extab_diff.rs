use egui::{RichText, ScrollArea};
use objdiff_core::{
    arch::ppc::ExceptionInfo,
    obj::{ObjInfo, ObjSymbol},
};
use time::format_description;

use crate::{
    hotkeys,
    views::{
        appearance::Appearance,
        column_layout::{render_header, render_strips},
        function_diff::FunctionDiffContext,
        symbol_diff::{
            match_color_for_symbol, DiffViewAction, DiffViewNavigation, DiffViewState,
            SymbolRefByName, View,
        },
    },
};

fn decode_extab(extab: &ExceptionInfo) -> String {
    let mut text = String::from("");

    let mut dtor_names: Vec<String> = vec![];
    for dtor in &extab.dtors {
        //For each function name, use the demangled name by default,
        //and if not available fallback to the original name
        let name: String = match &dtor.demangled_name {
            Some(demangled_name) => demangled_name.to_string(),
            None => dtor.name.clone(),
        };
        dtor_names.push(name);
    }
    if let Some(decoded) = extab.data.to_string(dtor_names) {
        text += decoded.as_str();
    }

    text
}

fn find_extab_entry<'a>(obj: &'a ObjInfo, symbol: &ObjSymbol) -> Option<&'a ExceptionInfo> {
    obj.arch.ppc().and_then(|ppc| ppc.extab_for_symbol(symbol))
}

fn extab_text_ui(
    ui: &mut egui::Ui,
    ctx: FunctionDiffContext<'_>,
    symbol: &ObjSymbol,
    appearance: &Appearance,
) -> Option<()> {
    if let Some(extab_entry) = find_extab_entry(ctx.obj, symbol) {
        let text = decode_extab(extab_entry);
        ui.colored_label(appearance.replace_color, &text);
        return Some(());
    }

    None
}

fn extab_ui(
    ui: &mut egui::Ui,
    ctx: FunctionDiffContext<'_>,
    appearance: &Appearance,
    _column: usize,
) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

            if let Some((_section, symbol)) =
                ctx.symbol_ref.map(|symbol_ref| ctx.obj.section_symbol(symbol_ref))
            {
                extab_text_ui(ui, ctx, symbol, appearance);
            }
        });
    });
}

#[must_use]
pub fn extab_diff_ui(
    ui: &mut egui::Ui,
    state: &DiffViewState,
    appearance: &Appearance,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let Some(result) = &state.build else {
        return ret;
    };

    let mut left_ctx = FunctionDiffContext::new(
        result.first_obj.as_ref(),
        state.symbol_state.left_symbol.as_ref(),
    );
    let mut right_ctx = FunctionDiffContext::new(
        result.second_obj.as_ref(),
        state.symbol_state.right_symbol.as_ref(),
    );

    // If one side is missing a symbol, but the diff process found a match, use that symbol
    let left_diff_symbol = left_ctx.and_then(|ctx| {
        ctx.symbol_ref.and_then(|symbol_ref| ctx.diff.symbol_diff(symbol_ref).target_symbol)
    });
    let right_diff_symbol = right_ctx.and_then(|ctx| {
        ctx.symbol_ref.and_then(|symbol_ref| ctx.diff.symbol_diff(symbol_ref).target_symbol)
    });
    if left_diff_symbol.is_some() && right_ctx.map_or(false, |ctx| !ctx.has_symbol()) {
        let (right_section, right_symbol) =
            right_ctx.unwrap().obj.section_symbol(left_diff_symbol.unwrap());
        let symbol_ref = SymbolRefByName::new(right_symbol, right_section);
        right_ctx = FunctionDiffContext::new(result.second_obj.as_ref(), Some(&symbol_ref));
        ret = Some(DiffViewAction::Navigate(DiffViewNavigation {
            view: Some(View::FunctionDiff),
            left_symbol: state.symbol_state.left_symbol.clone(),
            right_symbol: Some(symbol_ref),
        }));
    } else if right_diff_symbol.is_some() && left_ctx.map_or(false, |ctx| !ctx.has_symbol()) {
        let (left_section, left_symbol) =
            left_ctx.unwrap().obj.section_symbol(right_diff_symbol.unwrap());
        let symbol_ref = SymbolRefByName::new(left_symbol, left_section);
        left_ctx = FunctionDiffContext::new(result.first_obj.as_ref(), Some(&symbol_ref));
        ret = Some(DiffViewAction::Navigate(DiffViewNavigation {
            view: Some(View::FunctionDiff),
            left_symbol: Some(symbol_ref),
            right_symbol: state.symbol_state.right_symbol.clone(),
        }));
    }

    // If both sides are missing a symbol, switch to symbol diff view
    if right_ctx.map_or(false, |ctx| !ctx.has_symbol())
        && left_ctx.map_or(false, |ctx| !ctx.has_symbol())
    {
        return Some(DiffViewAction::Navigate(DiffViewNavigation::symbol_diff()));
    }

    // Header
    let available_width = ui.available_width();
    render_header(ui, available_width, 2, |ui, column| {
        if column == 0 {
            // Left column
            ui.horizontal(|ui| {
                if ui.button("⏴ Back").clicked() || hotkeys::back_pressed(ui.ctx()) {
                    ret = Some(DiffViewAction::Navigate(DiffViewNavigation::symbol_diff()));
                }
                ui.separator();
                if ui
                    .add_enabled(
                        !state.scratch_running
                            && state.scratch_available
                            && left_ctx.map_or(false, |ctx| ctx.has_symbol()),
                        egui::Button::new("📲 decomp.me"),
                    )
                    .on_hover_text_at_pointer("Create a new scratch on decomp.me (beta)")
                    .on_disabled_hover_text("Scratch configuration missing")
                    .clicked()
                {
                    if let Some((_section, symbol)) = left_ctx.and_then(|ctx| {
                        ctx.symbol_ref.map(|symbol_ref| ctx.obj.section_symbol(symbol_ref))
                    }) {
                        ret = Some(DiffViewAction::CreateScratch(symbol.name.clone()));
                    }
                }
            });

            if let Some((_section, symbol)) = left_ctx
                .and_then(|ctx| ctx.symbol_ref.map(|symbol_ref| ctx.obj.section_symbol(symbol_ref)))
            {
                let name = symbol.demangled_name.as_deref().unwrap_or(&symbol.name);
                ui.label(
                    RichText::new(name)
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
        } else if column == 1 {
            // Right column
            ui.horizontal(|ui| {
                if ui.add_enabled(!state.build_running, egui::Button::new("Build")).clicked() {
                    ret = Some(DiffViewAction::Build);
                }
                ui.scope(|ui| {
                    ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                    if state.build_running {
                        ui.colored_label(appearance.replace_color, "Building…");
                    } else {
                        ui.label("Last built:");
                        let format = format_description::parse("[hour]:[minute]:[second]").unwrap();
                        ui.label(
                            result.time.to_offset(appearance.utc_offset).format(&format).unwrap(),
                        );
                    }
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

            if let Some(((_section, symbol), symbol_diff)) = right_ctx.and_then(|ctx| {
                ctx.symbol_ref.map(|symbol_ref| {
                    (ctx.obj.section_symbol(symbol_ref), ctx.diff.symbol_diff(symbol_ref))
                })
            }) {
                let name = symbol.demangled_name.as_deref().unwrap_or(&symbol.name);
                ui.label(
                    RichText::new(name)
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
                if let Some(match_percent) = symbol_diff.match_percent {
                    ui.label(
                        RichText::new(format!("{:.0}%", match_percent.floor()))
                            .font(appearance.code_font.clone())
                            .color(match_color_for_symbol(match_percent, appearance)),
                    );
                }
            } else {
                ui.label(
                    RichText::new("Missing")
                        .font(appearance.code_font.clone())
                        .color(appearance.replace_color),
                );
            }
        }
    });

    // Table
    render_strips(ui, available_width, 2, |ui, column| {
        if column == 0 {
            if let Some(ctx) = left_ctx {
                extab_ui(ui, ctx, appearance, column);
            }
        } else if column == 1 {
            if let Some(ctx) = right_ctx {
                extab_ui(ui, ctx, appearance, column);
            }
        }
    });
    ret
}
