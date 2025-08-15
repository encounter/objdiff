use core::any::Any;

use egui::ScrollArea;
use objdiff_core::{arch::ppc::ExceptionInfo, obj::Object};

use crate::views::{appearance::Appearance, function_diff::FunctionDiffContext};

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

fn find_extab_entry(obj: &Object, symbol_index: usize) -> Option<&ExceptionInfo> {
    (obj.arch.as_ref() as &dyn Any)
        .downcast_ref::<objdiff_core::arch::ppc::ArchPpc>()
        .and_then(|ppc| ppc.extab_for_symbol(symbol_index))
}

fn extab_text_ui(
    ui: &mut egui::Ui,
    ctx: FunctionDiffContext<'_>,
    symbol_index: usize,
    appearance: &Appearance,
) -> Option<()> {
    if let Some(extab_entry) = find_extab_entry(ctx.obj, symbol_index) {
        let text = decode_extab(extab_entry);
        ui.colored_label(appearance.replace_color, &text);
        return Some(());
    }

    None
}

pub(crate) fn extab_ui(
    ui: &mut egui::Ui,
    ctx: FunctionDiffContext<'_>,
    appearance: &Appearance,
    _column: usize,
) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.scope(|ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

            if let Some(symbol_index) = ctx.symbol_ref {
                extab_text_ui(ui, ctx, symbol_index, appearance);
            }
        });
    });
}
