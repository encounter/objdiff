use std::{cmp::Ordering, collections::BTreeSet, default::Default};

use egui::{Label, Response, Sense, Widget, text::LayoutJob};
use egui_extras::TableRow;
use objdiff_core::{
    diff::{
        DiffObjConfig, InstructionDiffKind, InstructionDiffRow, ObjectDiff,
        display::{
            DiffText, DiffTextColor, DiffTextSegment, HighlightKind, display_row,
            instruction_context, instruction_hover,
        },
    },
    obj::{InstructionArgValue, InstructionRef, Object},
    util::ReallySigned,
};

use crate::views::{
    appearance::Appearance,
    diff::{context_menu_items_ui, hover_items_ui},
    symbol_diff::DiffViewAction,
};

#[derive(Default)]
pub struct FunctionViewState {
    left_highlight: HighlightKind,
    right_highlight: HighlightKind,
    /// Selected row indices for the left column
    pub left_selected_rows: BTreeSet<usize>,
    /// Selected row indices for the right column
    pub right_selected_rows: BTreeSet<usize>,
    /// Last clicked row index for shift-click range selection
    last_selected_row: Option<(usize, usize)>, // (column, row_index)
}

impl FunctionViewState {
    pub fn highlight(&self, column: usize) -> &HighlightKind {
        match column {
            0 => &self.left_highlight,
            1 => &self.right_highlight,
            _ => &HighlightKind::None,
        }
    }

    pub fn set_highlight(&mut self, column: usize, highlight: HighlightKind) {
        match column {
            0 => {
                if highlight == self.left_highlight {
                    if highlight == self.right_highlight {
                        self.left_highlight = HighlightKind::None;
                        self.right_highlight = HighlightKind::None;
                    } else {
                        self.right_highlight = self.left_highlight.clone();
                    }
                } else {
                    self.left_highlight = highlight;
                }
            }
            1 => {
                if highlight == self.right_highlight {
                    if highlight == self.left_highlight {
                        self.left_highlight = HighlightKind::None;
                        self.right_highlight = HighlightKind::None;
                    } else {
                        self.left_highlight = self.right_highlight.clone();
                    }
                } else {
                    self.right_highlight = highlight;
                }
            }
            _ => {}
        }
    }

    pub fn clear_highlight(&mut self) {
        self.left_highlight = HighlightKind::None;
        self.right_highlight = HighlightKind::None;
    }

    /// Get selected rows for a column
    pub fn selected_rows(&self, column: usize) -> &BTreeSet<usize> {
        match column {
            0 => &self.left_selected_rows,
            1 => &self.right_selected_rows,
            _ => &self.left_selected_rows, // fallback
        }
    }

    /// Check if a row is selected in a column
    pub fn is_row_selected(&self, column: usize, row_index: usize) -> bool {
        match column {
            0 => self.left_selected_rows.contains(&row_index),
            1 => self.right_selected_rows.contains(&row_index),
            _ => false,
        }
    }

    /// Toggle selection of a single row
    pub fn toggle_row_selection(&mut self, column: usize, row_index: usize, shift_held: bool) {
        let selected_rows = match column {
            0 => &mut self.left_selected_rows,
            1 => &mut self.right_selected_rows,
            _ => return,
        };

        if shift_held {
            // Range selection: select all rows between last selected and current
            if let Some((last_col, last_row)) = self.last_selected_row {
                if last_col == column {
                    let start = last_row.min(row_index);
                    let end = last_row.max(row_index);
                    for i in start..=end {
                        selected_rows.insert(i);
                    }
                } else {
                    // Different column, just toggle the current row
                    if selected_rows.contains(&row_index) {
                        selected_rows.remove(&row_index);
                    } else {
                        selected_rows.insert(row_index);
                    }
                }
            } else {
                // No previous selection, just select the current row
                selected_rows.insert(row_index);
            }
        } else {
            // Single toggle
            if selected_rows.contains(&row_index) {
                selected_rows.remove(&row_index);
            } else {
                selected_rows.insert(row_index);
            }
        }

        self.last_selected_row = Some((column, row_index));
    }

    /// Clear all row selections for a column
    pub fn clear_row_selection(&mut self, column: usize) {
        match column {
            0 => self.left_selected_rows.clear(),
            1 => self.right_selected_rows.clear(),
            _ => {}
        }
        if self.last_selected_row.map_or(false, |(col, _)| col == column) {
            self.last_selected_row = None;
        }
    }

    /// Clear all row selections for all columns
    pub fn clear_all_row_selections(&mut self) {
        self.left_selected_rows.clear();
        self.right_selected_rows.clear();
        self.last_selected_row = None;
    }

    /// Check if any rows are selected in a column
    pub fn has_selected_rows(&self, column: usize) -> bool {
        match column {
            0 => !self.left_selected_rows.is_empty(),
            1 => !self.right_selected_rows.is_empty(),
            _ => false,
        }
    }
}

fn ins_hover_ui(
    ui: &mut egui::Ui,
    obj: &Object,
    symbol_idx: usize,
    ins_ref: InstructionRef,
    diff_config: &DiffObjConfig,
    appearance: &Appearance,
) {
    let Some(resolved) = obj.resolve_instruction_ref(symbol_idx, ins_ref) else {
        ui.colored_label(appearance.delete_color, "Failed to resolve instruction");
        return;
    };
    let ins = match obj.arch.process_instruction(resolved, diff_config) {
        Ok(ins) => ins,
        Err(e) => {
            ui.colored_label(
                appearance.delete_color,
                format!("Failed to process instruction: {e}"),
            );
            return;
        }
    };

    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        hover_items_ui(ui, instruction_hover(obj, resolved, &ins), appearance);
    });
}

fn ins_context_menu(
    ui: &mut egui::Ui,
    obj: &Object,
    symbol_idx: usize,
    ins_ref: InstructionRef,
    column: usize,
    diff_config: &DiffObjConfig,
    appearance: &Appearance,
    has_selection: bool,
) -> Option<DiffViewAction> {
    let mut ret = None;

    // Add copy/clear selection options if there are selections
    if has_selection {
        if ui.button("📋 Copy selected rows").clicked() {
            ret = Some(DiffViewAction::CopySelectedRows(column));
            ui.close();
        }
        if ui.button("✖ Clear selection").clicked() {
            ret = Some(DiffViewAction::ClearRowSelection(column));
            ui.close();
        }
        ui.separator();
    }

    let Some(resolved) = obj.resolve_instruction_ref(symbol_idx, ins_ref) else {
        ui.colored_label(appearance.delete_color, "Failed to resolve instruction");
        return ret;
    };
    let ins = match obj.arch.process_instruction(resolved, diff_config) {
        Ok(ins) => ins,
        Err(e) => {
            ui.colored_label(
                appearance.delete_color,
                format!("Failed to process instruction: {e}"),
            );
            return ret;
        }
    };

    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        if let Some(action) = context_menu_items_ui(ui, instruction_context(obj, resolved, &ins), column, appearance) {
            ret = Some(action);
        }
    });

    ret
}

#[must_use]
fn diff_text_ui(
    ui: &mut egui::Ui,
    segment: DiffTextSegment,
    appearance: &Appearance,
    ins_view_state: &FunctionViewState,
    column: usize,
    space_width: f32,
    response_cb: impl Fn(Response) -> Response,
) -> Option<DiffViewAction> {
    let highlight_kind = HighlightKind::from(&segment.text);
    let label_text = match segment.text {
        DiffText::Basic(text) => text.to_string(),
        DiffText::Line(num) => format!("{num} "),
        DiffText::Address(addr) => format!("{addr:x}:"),
        DiffText::Opcode(mnemonic, _op) => format!("{mnemonic} "),
        DiffText::Argument(arg) => match arg {
            InstructionArgValue::Signed(v) => format!("{:#x}", ReallySigned(v)),
            InstructionArgValue::Unsigned(v) => format!("{v:#x}"),
            InstructionArgValue::Opaque(v) => v.into_owned(),
        },
        DiffText::BranchDest(addr) => format!("{addr:x}"),
        DiffText::Symbol(sym) => sym.demangled_name.as_ref().unwrap_or(&sym.name).clone(),
        DiffText::Addend(addend) => match addend.cmp(&0i64) {
            Ordering::Greater => format!("+{addend:#x}"),
            Ordering::Less => format!("-{:#x}", -addend),
            _ => String::new(),
        },
        DiffText::Spacing(n) => {
            ui.add_space(n as f32 * space_width);
            return None;
        }
        DiffText::Eol => "\n".to_string(),
    };

    let len = label_text.len();
    let highlight = highlight_kind != HighlightKind::None
        && *ins_view_state.highlight(column) == highlight_kind;
    let color = match segment.color {
        DiffTextColor::Normal => appearance.text_color,
        DiffTextColor::Dim => appearance.deemphasized_text_color,
        DiffTextColor::Bright => appearance.emphasized_text_color,
        DiffTextColor::DataFlow => appearance.dataflow_color,
        DiffTextColor::Replace => appearance.replace_color,
        DiffTextColor::Delete => appearance.delete_color,
        DiffTextColor::Insert => appearance.insert_color,
        DiffTextColor::Rotating(i) => {
            appearance.diff_colors[i as usize % appearance.diff_colors.len()]
        }
    };
    let mut response = Label::new(LayoutJob::single_section(
        label_text,
        appearance.code_text_format(color, highlight),
    ))
    .sense(Sense::click())
    .ui(ui);
    response = response_cb(response);
    let mut ret = None;
    if response.clicked() {
        ret = Some(DiffViewAction::SetDiffHighlight(column, highlight_kind));
    }
    if len < segment.pad_to as usize {
        ui.add_space((segment.pad_to as usize - len) as f32 * space_width);
    }
    ret
}

#[must_use]
fn asm_row_ui(
    ui: &mut egui::Ui,
    obj: &Object,
    ins_diff: &InstructionDiffRow,
    symbol_idx: usize,
    appearance: &Appearance,
    ins_view_state: &FunctionViewState,
    diff_config: &DiffObjConfig,
    column: usize,
    row_index: usize,
    response_cb: impl Fn(Response) -> Response,
) -> Option<DiffViewAction> {
    let mut ret = None;
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

    // Show selection highlight
    let is_selected = ins_view_state.is_row_selected(column, row_index);
    if is_selected {
        ui.painter().rect_filled(
            ui.available_rect_before_wrap(),
            0.0,
            appearance.highlight_color.gamma_multiply(0.3),
        );
    } else if ins_diff.kind != InstructionDiffKind::None {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }

    let space_width = ui.fonts_mut(|f| f.glyph_width(&appearance.code_font, ' '));
    display_row(obj, symbol_idx, ins_diff, diff_config, |segment| {
        if let Some(action) =
            diff_text_ui(ui, segment, appearance, ins_view_state, column, space_width, &response_cb)
        {
            ret = Some(action);
        }
        Ok(())
    })
    .unwrap();
    ret
}

#[must_use]
pub(crate) fn asm_col_ui(
    row: &mut TableRow<'_, '_>,
    ctx: FunctionDiffContext<'_>,
    appearance: &Appearance,
    ins_view_state: &FunctionViewState,
    diff_config: &DiffObjConfig,
    column: usize,
) -> Option<DiffViewAction> {
    let mut ret = None;
    let symbol_ref = ctx.symbol_ref?;
    let row_index = row.index();
    let ins_row = &ctx.diff.symbols[symbol_ref].instruction_rows[row_index];
    let has_selection = ins_view_state.has_selected_rows(column);

    let (_, mut response) = row.col(|ui| {
        if let Some(action) = asm_row_ui(
            ui,
            ctx.obj,
            ins_row,
            symbol_ref,
            appearance,
            ins_view_state,
            diff_config,
            column,
            row_index,
            |r| r, // Simple passthrough
        ) {
            ret = Some(action);
        }
    });

    // Handle context menu
    if let Some(ins_ref) = ins_row.ins_ref {
        response = response.context_menu(|ui| {
            if let Some(action) = ins_context_menu(
                ui, ctx.obj, symbol_ref, ins_ref, column, diff_config, appearance, has_selection,
            ) {
                ret = Some(action);
            }
        });
        response = response.on_hover_ui_at_pointer(|ui| {
            ins_hover_ui(ui, ctx.obj, symbol_ref, ins_ref, diff_config, appearance)
        });
    } else if has_selection {
        // Even rows without instructions can have context menu for copy/clear selected
        response = response.context_menu(|ui| {
            if ui.button("📋 Copy selected rows").clicked() {
                ret = Some(DiffViewAction::CopySelectedRows(column));
                ui.close();
            }
            if ui.button("✖ Clear selection").clicked() {
                ret = Some(DiffViewAction::ClearRowSelection(column));
                ui.close();
            }
        });
    }

    // Handle Ctrl+Click for row selection toggle
    if response.clicked() {
        let modifiers = response.ctx.input(|i| i.modifiers);
        if modifiers.ctrl || modifiers.command {
            ret = Some(DiffViewAction::ToggleRowSelection(column, row_index, modifiers.shift));
        }
    }

    ret
}

#[derive(Clone, Copy)]
pub struct FunctionDiffContext<'a> {
    pub obj: &'a Object,
    pub diff: &'a ObjectDiff,
    pub symbol_ref: Option<usize>,
}
