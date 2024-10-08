use std::default::Default;

use egui::{text::LayoutJob, Id, Label, Response, RichText, Sense, Widget};
use egui_extras::TableRow;
use objdiff_core::{
    arch::ObjArch,
    diff::{
        display::{display_diff, DiffText, HighlightKind},
        ObjDiff, ObjInsDiff, ObjInsDiffKind,
    },
    obj::{
        ObjInfo, ObjIns, ObjInsArg, ObjInsArgValue, ObjSection, ObjSymbol, ObjSymbolKind, SymbolRef,
    },
};
use time::format_description;

use crate::views::{
    appearance::Appearance,
    column_layout::{render_header, render_strips, render_table},
    symbol_diff::{
        match_color_for_symbol, symbol_list_ui, DiffViewState, SymbolDiffContext, SymbolFilter,
        SymbolOverrideAction, SymbolRefByName, SymbolUiResult, SymbolViewState, View,
    },
};

#[derive(Default)]
pub struct FunctionViewState {
    left_highlight: HighlightKind,
    right_highlight: HighlightKind,
}

impl FunctionViewState {
    fn highlight(&self, column: usize) -> &HighlightKind {
        match column {
            0 => &self.left_highlight,
            1 => &self.right_highlight,
            _ => &HighlightKind::None,
        }
    }

    fn set_highlight(&mut self, column: usize, highlight: HighlightKind) {
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

    fn clear_highlight(&mut self) {
        self.left_highlight = HighlightKind::None;
        self.right_highlight = HighlightKind::None;
    }
}

fn ins_hover_ui(
    ui: &mut egui::Ui,
    arch: &dyn ObjArch,
    section: &ObjSection,
    ins: &ObjIns,
    symbol: &ObjSymbol,
    appearance: &Appearance,
) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        let offset = ins.address - section.address;
        ui.label(format!(
            "{:02x?}",
            &section.data[offset as usize..(offset + ins.size as u64) as usize]
        ));

        if let Some(virtual_address) = symbol.virtual_address {
            let offset = ins.address - symbol.address;
            ui.colored_label(
                appearance.replace_color,
                format!("Virtual address: {:#x}", virtual_address + offset),
            );
        }

        if let Some(orig) = &ins.orig {
            ui.label(format!("Original: {}", orig));
        }

        for arg in &ins.args {
            if let ObjInsArg::Arg(arg) = arg {
                match arg {
                    ObjInsArgValue::Signed(v) => {
                        ui.label(format!("{arg} == {v}"));
                    }
                    ObjInsArgValue::Unsigned(v) => {
                        ui.label(format!("{arg} == {v}"));
                    }
                    _ => {}
                }
            }
        }

        if let Some(reloc) = &ins.reloc {
            ui.label(format!("Relocation type: {}", arch.display_reloc(reloc.flags)));
            ui.colored_label(appearance.highlight_color, format!("Name: {}", reloc.target.name));
            if let Some(section) = &reloc.target_section {
                ui.colored_label(appearance.highlight_color, format!("Section: {section}"));
                ui.colored_label(
                    appearance.highlight_color,
                    format!("Address: {:x}", reloc.target.address),
                );
                ui.colored_label(
                    appearance.highlight_color,
                    format!("Size: {:x}", reloc.target.size),
                );
                if let Some(s) = arch
                    .guess_data_type(ins)
                    .and_then(|ty| arch.display_data_type(ty, &reloc.target.bytes))
                {
                    ui.colored_label(appearance.highlight_color, s);
                }
            } else {
                ui.colored_label(appearance.highlight_color, "Extern".to_string());
            }
        }
    });
}

fn ins_context_menu(ui: &mut egui::Ui, section: &ObjSection, ins: &ObjIns, symbol: &ObjSymbol) {
    ui.scope(|ui| {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

        if ui.button(format!("Copy \"{}\"", ins.formatted)).clicked() {
            ui.output_mut(|output| output.copied_text.clone_from(&ins.formatted));
            ui.close_menu();
        }

        let mut hex_string = "0x".to_string();
        for byte in &section.data[ins.address as usize..(ins.address + ins.size as u64) as usize] {
            hex_string.push_str(&format!("{:02x}", byte));
        }
        if ui.button(format!("Copy \"{hex_string}\" (instruction bytes)")).clicked() {
            ui.output_mut(|output| output.copied_text = hex_string);
            ui.close_menu();
        }

        if let Some(virtual_address) = symbol.virtual_address {
            let offset = ins.address - symbol.address;
            let offset_string = format!("{:#x}", virtual_address + offset);
            if ui.button(format!("Copy \"{offset_string}\" (virtual address)")).clicked() {
                ui.output_mut(|output| output.copied_text = offset_string);
                ui.close_menu();
            }
        }

        for arg in &ins.args {
            if let ObjInsArg::Arg(arg) = arg {
                match arg {
                    ObjInsArgValue::Signed(v) => {
                        if ui.button(format!("Copy \"{arg}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = arg.to_string());
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = v.to_string());
                            ui.close_menu();
                        }
                    }
                    ObjInsArgValue::Unsigned(v) => {
                        if ui.button(format!("Copy \"{arg}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = arg.to_string());
                            ui.close_menu();
                        }
                        if ui.button(format!("Copy \"{v}\"")).clicked() {
                            ui.output_mut(|output| output.copied_text = v.to_string());
                            ui.close_menu();
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(reloc) = &ins.reloc {
            if let Some(name) = &reloc.target.demangled_name {
                if ui.button(format!("Copy \"{name}\"")).clicked() {
                    ui.output_mut(|output| output.copied_text.clone_from(name));
                    ui.close_menu();
                }
            }
            if ui.button(format!("Copy \"{}\"", reloc.target.name)).clicked() {
                ui.output_mut(|output| output.copied_text.clone_from(&reloc.target.name));
                ui.close_menu();
            }
        }
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

#[expect(clippy::too_many_arguments)]
fn diff_text_ui(
    ui: &mut egui::Ui,
    text: DiffText<'_>,
    ins_diff: &ObjInsDiff,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
    column: usize,
    space_width: f32,
    response_cb: impl Fn(Response) -> Response,
) {
    let label_text;
    let mut base_color = match ins_diff.kind {
        ObjInsDiffKind::None | ObjInsDiffKind::OpMismatch | ObjInsDiffKind::ArgMismatch => {
            appearance.text_color
        }
        ObjInsDiffKind::Replace => appearance.replace_color,
        ObjInsDiffKind::Delete => appearance.delete_color,
        ObjInsDiffKind::Insert => appearance.insert_color,
    };
    let mut pad_to = 0;
    match text {
        DiffText::Basic(text) => {
            label_text = text.to_string();
        }
        DiffText::BasicColor(s, idx) => {
            label_text = s.to_string();
            base_color = appearance.diff_colors[idx % appearance.diff_colors.len()];
        }
        DiffText::Line(num) => {
            label_text = num.to_string();
            base_color = appearance.deemphasized_text_color;
            pad_to = 5;
        }
        DiffText::Address(addr) => {
            label_text = format!("{:x}:", addr);
            pad_to = 5;
        }
        DiffText::Opcode(mnemonic, _op) => {
            label_text = mnemonic.to_string();
            if ins_diff.kind == ObjInsDiffKind::OpMismatch {
                base_color = appearance.replace_color;
            }
            pad_to = 8;
        }
        DiffText::Argument(arg, diff) => {
            label_text = arg.to_string();
            if let Some(diff) = diff {
                base_color = appearance.diff_colors[diff.idx % appearance.diff_colors.len()]
            }
        }
        DiffText::BranchDest(addr, diff) => {
            label_text = format!("{addr:x}");
            if let Some(diff) = diff {
                base_color = appearance.diff_colors[diff.idx % appearance.diff_colors.len()]
            }
        }
        DiffText::Symbol(sym) => {
            let name = sym.demangled_name.as_ref().unwrap_or(&sym.name);
            label_text = name.clone();
            base_color = appearance.emphasized_text_color;
        }
        DiffText::Spacing(n) => {
            ui.add_space(n as f32 * space_width);
            return;
        }
        DiffText::Eol => {
            label_text = "\n".to_string();
        }
    }

    let len = label_text.len();
    let highlight = *ins_view_state.highlight(column) == text;
    let mut response = Label::new(LayoutJob::single_section(
        label_text,
        appearance.code_text_format(base_color, highlight),
    ))
    .sense(Sense::click())
    .ui(ui);
    response = response_cb(response);
    if response.clicked() {
        ins_view_state.set_highlight(column, text.into());
    }
    if len < pad_to {
        ui.add_space((pad_to - len) as f32 * space_width);
    }
}

fn asm_row_ui(
    ui: &mut egui::Ui,
    ins_diff: &ObjInsDiff,
    symbol: &ObjSymbol,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
    column: usize,
    response_cb: impl Fn(Response) -> Response,
) {
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    if ins_diff.kind != ObjInsDiffKind::None {
        ui.painter().rect_filled(ui.available_rect_before_wrap(), 0.0, ui.visuals().faint_bg_color);
    }
    let space_width = ui.fonts(|f| f.glyph_width(&appearance.code_font, ' '));
    display_diff(ins_diff, symbol.address, |text| {
        diff_text_ui(
            ui,
            text,
            ins_diff,
            appearance,
            ins_view_state,
            column,
            space_width,
            &response_cb,
        );
        Ok::<_, ()>(())
    })
    .unwrap();
}

fn asm_col_ui(
    row: &mut TableRow<'_, '_>,
    ctx: FunctionDiffContext<'_>,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
    column: usize,
) {
    let Some(symbol_ref) = ctx.symbol_ref else {
        return;
    };
    let (section, symbol) = ctx.obj.section_symbol(symbol_ref);
    let section = section.unwrap();
    let ins_diff = &ctx.diff.symbol_diff(symbol_ref).instructions[row.index()];
    let response_cb = |response: Response| {
        if let Some(ins) = &ins_diff.ins {
            response.context_menu(|ui| ins_context_menu(ui, section, ins, symbol));
            response.on_hover_ui_at_pointer(|ui| {
                ins_hover_ui(ui, ctx.obj.arch.as_ref(), section, ins, symbol, appearance)
            })
        } else {
            response
        }
    };
    let (_, response) = row.col(|ui| {
        asm_row_ui(ui, ins_diff, symbol, appearance, ins_view_state, column, response_cb);
    });
    response_cb(response);
}

fn asm_table_ui(
    ui: &mut egui::Ui,
    available_width: f32,
    left_ctx: Option<FunctionDiffContext<'_>>,
    right_ctx: Option<FunctionDiffContext<'_>>,
    appearance: &Appearance,
    ins_view_state: &mut FunctionViewState,
    symbol_state: &mut SymbolViewState,
) -> Option<SymbolOverrideAction> {
    let left_len = left_ctx.and_then(|ctx| {
        ctx.symbol_ref.map(|symbol_ref| ctx.diff.symbol_diff(symbol_ref).instructions.len())
    });
    let right_len = right_ctx.and_then(|ctx| {
        ctx.symbol_ref.map(|symbol_ref| ctx.diff.symbol_diff(symbol_ref).instructions.len())
    });
    let instructions_len = match (left_len, right_len) {
        (Some(left_len), Some(right_len)) => {
            if left_len != right_len {
                ui.label("Instruction count mismatch");
                return None;
            }
            left_len
        }
        (Some(left_len), None) => left_len,
        (None, Some(right_len)) => right_len,
        (None, None) => {
            ui.label("No symbol selected");
            return None;
        }
    };
    let mut ret = None;
    if left_len.is_some() && right_len.is_some() {
        // Joint view
        render_table(
            ui,
            available_width,
            2,
            appearance.code_font.size,
            instructions_len,
            |row, column| {
                if column == 0 {
                    if let Some(ctx) = left_ctx {
                        asm_col_ui(row, ctx, appearance, ins_view_state, column);
                    }
                } else if column == 1 {
                    if let Some(ctx) = right_ctx {
                        asm_col_ui(row, ctx, appearance, ins_view_state, column);
                    }
                    if row.response().clicked() {
                        ins_view_state.clear_highlight();
                    }
                }
            },
        );
    } else {
        // Split view, one side is the symbol list
        render_strips(ui, available_width, 2, |ui, column| {
            if column == 0 {
                if let Some(ctx) = left_ctx {
                    if ctx.has_symbol() {
                        render_table(
                            ui,
                            available_width / 2.0,
                            1,
                            appearance.code_font.size,
                            instructions_len,
                            |row, column| {
                                asm_col_ui(row, ctx, appearance, ins_view_state, column);
                                if row.response().clicked() {
                                    ins_view_state.clear_highlight();
                                }
                            },
                        );
                    } else if let Some(result) = symbol_list_ui(
                        ui,
                        SymbolDiffContext { obj: ctx.obj, diff: ctx.diff },
                        None,
                        symbol_state,
                        SymbolFilter::Kind(ObjSymbolKind::Function),
                        appearance,
                        column,
                    ) {
                        let right_symbol = right_ctx
                            .and_then(|ctx| {
                                ctx.symbol_ref.map(|symbol_ref| ctx.obj.section_symbol(symbol_ref))
                            })
                            .map(|(section, symbol)| SymbolRefByName::new(symbol, section));
                        if let (Some(left_symbol), Some(right_symbol)) =
                            (result.left_symbol, right_symbol)
                        {
                            ret = Some(SymbolOverrideAction::Set(left_symbol, right_symbol));
                        }
                    }
                } else {
                    ui.label("No left object");
                }
            } else if column == 1 {
                if let Some(ctx) = right_ctx {
                    if ctx.has_symbol() {
                        render_table(
                            ui,
                            available_width / 2.0,
                            1,
                            appearance.code_font.size,
                            instructions_len,
                            |row, column| {
                                asm_col_ui(row, ctx, appearance, ins_view_state, column);
                                if row.response().clicked() {
                                    ins_view_state.clear_highlight();
                                }
                            },
                        );
                    } else if let Some(result) = symbol_list_ui(
                        ui,
                        SymbolDiffContext { obj: ctx.obj, diff: ctx.diff },
                        None,
                        symbol_state,
                        SymbolFilter::Kind(ObjSymbolKind::Function),
                        appearance,
                        column,
                    ) {
                        let left_symbol = left_ctx
                            .and_then(|ctx| {
                                ctx.symbol_ref.map(|symbol_ref| ctx.obj.section_symbol(symbol_ref))
                            })
                            .map(|(section, symbol)| SymbolRefByName::new(symbol, section));
                        if let (Some(left_symbol), Some(right_symbol)) =
                            (left_symbol, result.right_symbol)
                        {
                            ret = Some(SymbolOverrideAction::Set(left_symbol, right_symbol));
                        }
                    }
                } else {
                    ui.label("No right object");
                }
            }
        });
    }
    ret
}

#[derive(Clone, Copy)]
pub struct FunctionDiffContext<'a> {
    pub obj: &'a ObjInfo,
    pub diff: &'a ObjDiff,
    pub symbol_ref: Option<SymbolRef>,
}

impl<'a> FunctionDiffContext<'a> {
    pub fn new(
        obj: Option<&'a (ObjInfo, ObjDiff)>,
        selected_symbol: Option<&SymbolRefByName>,
    ) -> Option<Self> {
        obj.map(|(obj, diff)| Self {
            obj,
            diff,
            symbol_ref: selected_symbol.and_then(|s| find_symbol(obj, s)),
        })
    }

    #[inline]
    pub fn has_symbol(&self) -> bool { self.symbol_ref.is_some() }
}

pub fn function_diff_ui(ui: &mut egui::Ui, state: &mut DiffViewState, appearance: &Appearance) {
    let Some(result) = &state.build else {
        return;
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
        ctx.symbol_ref.and_then(|symbol_ref| ctx.diff.symbol_diff(symbol_ref).diff_symbol)
    });
    let right_diff_symbol = right_ctx.and_then(|ctx| {
        ctx.symbol_ref.and_then(|symbol_ref| ctx.diff.symbol_diff(symbol_ref).diff_symbol)
    });
    if left_diff_symbol.is_some() && right_ctx.map_or(false, |ctx| !ctx.has_symbol()) {
        let (right_section, right_symbol) =
            right_ctx.unwrap().obj.section_symbol(left_diff_symbol.unwrap());
        let symbol_ref = SymbolRefByName::new(right_symbol, right_section);
        right_ctx = FunctionDiffContext::new(result.second_obj.as_ref(), Some(&symbol_ref));
        state.symbol_state.right_symbol = Some(symbol_ref);
    } else if right_diff_symbol.is_some() && left_ctx.map_or(false, |ctx| !ctx.has_symbol()) {
        let (left_section, left_symbol) =
            left_ctx.unwrap().obj.section_symbol(right_diff_symbol.unwrap());
        let symbol_ref = SymbolRefByName::new(left_symbol, left_section);
        left_ctx = FunctionDiffContext::new(result.first_obj.as_ref(), Some(&symbol_ref));
        state.symbol_state.left_symbol = Some(symbol_ref);
    }

    // If both sides are missing a symbol, switch to symbol diff view
    if !right_ctx.map_or(false, |ctx| ctx.has_symbol())
        && !left_ctx.map_or(false, |ctx| ctx.has_symbol())
    {
        state.current_view = View::SymbolDiff;
        state.symbol_state.left_symbol = None;
        state.symbol_state.right_symbol = None;
        return;
    }

    // Header
    let available_width = ui.available_width();
    render_header(ui, available_width, 2, |ui, column| {
        if column == 0 {
            // Left column
            ui.horizontal(|ui| {
                if ui.button("⏴ Back").clicked() {
                    state.current_view = View::SymbolDiff;
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
                    state.queue_scratch = true;
                }
            });

            if let Some((section, symbol)) = left_ctx
                .and_then(|ctx| ctx.symbol_ref.map(|symbol_ref| ctx.obj.section_symbol(symbol_ref)))
            {
                let name = symbol.demangled_name.as_deref().unwrap_or(&symbol.name);
                ui.label(
                    RichText::new(name)
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
                if right_ctx.map_or(false, |m| m.has_symbol())
                    && ui
                        .button("Change target")
                        .on_hover_text_at_pointer("Choose a different symbol to use as the target")
                        .clicked()
                {
                    state.match_action = Some(SymbolOverrideAction::ClearLeft(
                        SymbolRefByName::new(symbol, section),
                        state.symbol_state.right_symbol.clone().unwrap(),
                    ));
                    state.post_build_nav = Some(SymbolUiResult {
                        view: Some(View::FunctionDiff),
                        left_symbol: None,
                        right_symbol: state.symbol_state.right_symbol.clone(),
                    });
                    state.queue_build = true;
                }
            } else {
                ui.label(
                    RichText::new("Missing")
                        .font(appearance.code_font.clone())
                        .color(appearance.replace_color),
                );
                ui.label(
                    RichText::new("Choose target symbol")
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
            }
        } else if column == 1 {
            // Right column
            ui.horizontal(|ui| {
                if ui.add_enabled(!state.build_running, egui::Button::new("Build")).clicked() {
                    state.queue_build = true;
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
                    state.queue_open_source_path = true;
                }
            });

            if let Some(((section, symbol), symbol_diff)) = right_ctx.and_then(|ctx| {
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
                ui.horizontal(|ui| {
                    if let Some(match_percent) = symbol_diff.match_percent {
                        ui.label(
                            RichText::new(format!("{:.0}%", match_percent.floor()))
                                .font(appearance.code_font.clone())
                                .color(match_color_for_symbol(match_percent, appearance)),
                        );
                    }
                    if left_ctx.map_or(false, |m| m.has_symbol()) {
                        ui.separator();
                        if ui
                            .button("Change base")
                            .on_hover_text_at_pointer(
                                "Choose a different symbol to use as the base",
                            )
                            .clicked()
                        {
                            state.match_action = Some(SymbolOverrideAction::ClearRight(
                                state.symbol_state.left_symbol.clone().unwrap(),
                                SymbolRefByName::new(symbol, section),
                            ));
                            state.post_build_nav = Some(SymbolUiResult {
                                view: Some(View::FunctionDiff),
                                left_symbol: state.symbol_state.left_symbol.clone(),
                                right_symbol: None,
                            });
                            state.queue_build = true;
                        }
                    }
                });
            } else {
                ui.label(
                    RichText::new("Missing")
                        .font(appearance.code_font.clone())
                        .color(appearance.replace_color),
                );
                ui.label(
                    RichText::new("Choose base symbol")
                        .font(appearance.code_font.clone())
                        .color(appearance.highlight_color),
                );
            }
        }
    });

    // Table
    let id = Id::new(state.symbol_state.left_symbol.as_ref().map(|s| s.symbol_name.as_str()))
        .with(state.symbol_state.right_symbol.as_ref().map(|s| s.symbol_name.as_str()));
    if let Some(result) = ui
        .push_id(id, |ui| {
            asm_table_ui(
                ui,
                available_width,
                left_ctx,
                right_ctx,
                appearance,
                &mut state.function_state,
                &mut state.symbol_state,
            )
        })
        .inner
    {
        match result {
            SymbolOverrideAction::Set(left, right) => {
                state.match_action = Some(SymbolOverrideAction::Set(left.clone(), right.clone()));
                state.post_build_nav = Some(SymbolUiResult {
                    view: Some(View::FunctionDiff),
                    left_symbol: Some(left),
                    right_symbol: Some(right),
                });
            }
            _ => todo!(),
        }
        state.queue_build = true;
    }
}
