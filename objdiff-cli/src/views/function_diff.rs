use core::cmp::Ordering;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use objdiff_core::{
    build::BuildStatus,
    diff::{
        DiffObjConfig, FunctionRelocDiffs, InstructionDiffKind, ObjectDiff, SymbolDiff,
        display::{DiffText, DiffTextColor, HighlightKind, display_row},
    },
    obj::Object,
};
use ratatui::{
    Frame,
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use super::{EventControlFlow, EventResult, UiView};
use crate::cmd::diff::AppState;

#[allow(dead_code)]
#[derive(Default)]
pub struct FunctionDiffUi {
    pub symbol_name: String,
    pub left_highlight: HighlightKind,
    pub right_highlight: HighlightKind,
    pub scroll_x: usize,
    pub scroll_state_x: ScrollbarState,
    pub scroll_y: usize,
    pub scroll_state_y: ScrollbarState,
    pub per_page: usize,
    pub num_rows: usize,
    pub left_sym: Option<usize>,
    pub right_sym: Option<usize>,
    pub prev_sym: Option<usize>,
    pub open_options: bool,
    pub three_way: bool,
}

impl UiView for FunctionDiffUi {
    fn draw(&mut self, state: &AppState, f: &mut Frame, result: &mut EventResult) {
        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).split(f.area());
        let header_chunks = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .split(chunks[0]);
        let content_chunks = if self.three_way {
            Layout::horizontal([
                Constraint::Fill(1),
                Constraint::Length(3),
                Constraint::Fill(1),
                Constraint::Length(3),
                Constraint::Fill(1),
                Constraint::Length(2),
            ])
            .split(chunks[1])
        } else {
            Layout::horizontal([
                Constraint::Fill(1),
                Constraint::Length(3),
                Constraint::Fill(1),
                Constraint::Length(2),
            ])
            .split(chunks[1])
        };

        self.per_page = chunks[1].height.saturating_sub(2) as usize;
        let max_scroll_y = self.num_rows.saturating_sub(self.per_page);
        if self.scroll_y > max_scroll_y {
            self.scroll_y = max_scroll_y;
        }
        self.scroll_state_y =
            self.scroll_state_y.content_length(max_scroll_y).position(self.scroll_y);

        let mut line_l = Line::default();
        line_l
            .spans
            .push(Span::styled(self.symbol_name.clone(), Style::new().fg(Color::White).bold()));
        f.render_widget(line_l, header_chunks[0]);

        let mut line_r = Line::default();
        if let Some(percent) = get_symbol(state.right_obj.as_ref(), self.right_sym)
            .and_then(|(_, _, d)| d.match_percent)
        {
            line_r.spans.push(Span::styled(
                format!("{percent:.2}% "),
                Style::new().fg(match_percent_color(percent)),
            ));
        }
        let reload_time = state
            .reload_time
            .as_ref()
            .and_then(|t| t.format(&state.time_format).ok())
            .unwrap_or_else(|| "N/A".to_string());
        line_r.spans.push(Span::styled(
            format!("Last reload: {reload_time}"),
            Style::new().fg(Color::White),
        ));
        line_r.spans.push(Span::styled(
            format!(" ({} jobs)", state.jobs.jobs.len()),
            Style::new().fg(Color::LightYellow),
        ));
        f.render_widget(line_r, header_chunks[2]);

        let mut left_text = None;
        let mut left_highlight = None;
        let mut max_width = 0;
        if let Some((obj, symbol_idx, symbol_diff)) =
            get_symbol(state.left_obj.as_ref(), self.left_sym)
        {
            let mut text = Text::default();
            let rect = content_chunks[0].inner(Margin::new(0, 1));
            left_highlight = self.print_sym(
                &mut text,
                obj,
                symbol_idx,
                symbol_diff,
                &state.diff_obj_config,
                rect,
                &self.left_highlight,
                result,
                false,
            );
            max_width = max_width.max(text.width());
            left_text = Some(text);
        } else if let Some(status) = &state.left_status {
            let mut text = Text::default();
            self.print_build_status(&mut text, status);
            max_width = max_width.max(text.width());
            left_text = Some(text);
        }

        let mut right_text = None;
        let mut right_highlight = None;
        let mut margin_text = None;
        if let Some((obj, symbol_idx, symbol_diff)) =
            get_symbol(state.right_obj.as_ref(), self.right_sym)
        {
            let mut text = Text::default();
            let rect = content_chunks[2].inner(Margin::new(0, 1));
            right_highlight = self.print_sym(
                &mut text,
                obj,
                symbol_idx,
                symbol_diff,
                &state.diff_obj_config,
                rect,
                &self.right_highlight,
                result,
                false,
            );
            max_width = max_width.max(text.width());
            right_text = Some(text);

            // Render margin
            let mut text = Text::default();
            let rect = content_chunks[1].inner(Margin::new(1, 1));
            self.print_margin(&mut text, symbol_diff, rect);
            margin_text = Some(text);
        } else if let Some(status) = &state.right_status {
            let mut text = Text::default();
            self.print_build_status(&mut text, status);
            max_width = max_width.max(text.width());
            right_text = Some(text);
        }

        let mut prev_text = None;
        let mut prev_margin_text = None;
        if self.three_way
            && let Some((obj, symbol_idx, symbol_diff)) =
                get_symbol(state.prev_obj.as_ref(), self.prev_sym)
        {
            let mut text = Text::default();
            let rect = content_chunks[4].inner(Margin::new(0, 1));
            self.print_sym(
                &mut text,
                obj,
                symbol_idx,
                symbol_diff,
                &state.diff_obj_config,
                rect,
                &self.right_highlight,
                result,
                true,
            );
            max_width = max_width.max(text.width());
            prev_text = Some(text);

            // Render margin
            let mut text = Text::default();
            let rect = content_chunks[3].inner(Margin::new(1, 1));
            self.print_margin(&mut text, symbol_diff, rect);
            prev_margin_text = Some(text);
        }

        let max_scroll_x =
            max_width.saturating_sub(content_chunks[0].width.min(content_chunks[2].width) as usize);
        if self.scroll_x > max_scroll_x {
            self.scroll_x = max_scroll_x;
        }
        self.scroll_state_x =
            self.scroll_state_x.content_length(max_scroll_x).position(self.scroll_x);

        if let Some(text) = left_text {
            // Render left column
            f.render_widget(
                Paragraph::new(text)
                    .block(
                        Block::new()
                            .borders(Borders::TOP)
                            .border_style(Style::new().fg(Color::Gray))
                            .title_style(Style::new().bold())
                            .title("TARGET"),
                    )
                    .scroll((0, self.scroll_x as u16)),
                content_chunks[0],
            );
        }
        if let Some(text) = margin_text {
            f.render_widget(text, content_chunks[1].inner(Margin::new(1, 1)));
        }
        if let Some(text) = right_text {
            f.render_widget(
                Paragraph::new(text)
                    .block(
                        Block::new()
                            .borders(Borders::TOP)
                            .border_style(Style::new().fg(Color::Gray))
                            .title_style(Style::new().bold())
                            .title("CURRENT"),
                    )
                    .scroll((0, self.scroll_x as u16)),
                content_chunks[2],
            );
        }

        if self.three_way {
            if let Some(text) = prev_margin_text {
                f.render_widget(text, content_chunks[3].inner(Margin::new(1, 1)));
            }
            let block = Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(Color::Gray))
                .title_style(Style::new().bold())
                .title("SAVED");
            if let Some(text) = prev_text {
                f.render_widget(
                    Paragraph::new(text).block(block.clone()).scroll((0, self.scroll_x as u16)),
                    content_chunks[4],
                );
            } else {
                f.render_widget(block, content_chunks[4]);
            }
        }

        // Render scrollbars
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None),
            chunks[1].inner(Margin::new(0, 1)),
            &mut self.scroll_state_y,
        );
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::HorizontalBottom).thumb_symbol("■"),
            content_chunks[0],
            &mut self.scroll_state_x,
        );
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::HorizontalBottom).thumb_symbol("■"),
            content_chunks[2],
            &mut self.scroll_state_x,
        );
        if self.three_way {
            f.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::HorizontalBottom).thumb_symbol("■"),
                content_chunks[4],
                &mut self.scroll_state_x,
            );
        }

        if let Some(new_highlight) = left_highlight {
            if new_highlight == self.left_highlight {
                if self.left_highlight != self.right_highlight {
                    self.right_highlight = self.left_highlight.clone();
                } else {
                    self.left_highlight = HighlightKind::None;
                    self.right_highlight = HighlightKind::None;
                }
            } else {
                self.left_highlight = new_highlight;
            }
            result.redraw = true;
        } else if let Some(new_highlight) = right_highlight {
            if new_highlight == self.right_highlight {
                if self.left_highlight != self.right_highlight {
                    self.left_highlight = self.right_highlight.clone();
                } else {
                    self.left_highlight = HighlightKind::None;
                    self.right_highlight = HighlightKind::None;
                }
            } else {
                self.right_highlight = new_highlight;
            }
            result.redraw = true;
        }

        if self.open_options {
            self.draw_options(f, result);
        }
    }

    fn handle_event(&mut self, state: &mut AppState, event: Event) -> EventControlFlow {
        let mut result = EventResult::default();
        match event {
            Event::Key(event)
                if matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                match event.code {
                    // Quit
                    KeyCode::Esc | KeyCode::Char('q') => return EventControlFlow::Break,
                    // Page up
                    KeyCode::PageUp => {
                        self.page_up(false);
                        result.redraw = true;
                    }
                    // Page up (shift + space)
                    KeyCode::Char(' ') if event.modifiers.contains(KeyModifiers::SHIFT) => {
                        self.page_up(false);
                        result.redraw = true;
                    }
                    // Page down
                    KeyCode::Char(' ') | KeyCode::PageDown => {
                        self.page_down(false);
                        result.redraw = true;
                    }
                    // Page down (ctrl + f)
                    KeyCode::Char('f') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.page_down(false);
                        result.redraw = true;
                    }
                    // Page up (ctrl + b)
                    KeyCode::Char('b') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.page_up(false);
                        result.redraw = true;
                    }
                    // Half page down (ctrl + d)
                    KeyCode::Char('d') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.page_down(true);
                        result.redraw = true;
                    }
                    // Half page up (ctrl + u)
                    KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.page_up(true);
                        result.redraw = true;
                    }
                    // Scroll down
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.scroll_y += 1;
                        result.redraw = true;
                    }
                    // Scroll up
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.scroll_y = self.scroll_y.saturating_sub(1);
                        result.redraw = true;
                    }
                    // Scroll to start
                    KeyCode::Char('g') => {
                        self.scroll_y = 0;
                        result.redraw = true;
                    }
                    // Scroll to end
                    KeyCode::Char('G') => {
                        self.scroll_y = self.num_rows;
                        result.redraw = true;
                    }
                    // Reload
                    KeyCode::Char('r') => {
                        result.redraw = true;
                        return EventControlFlow::Reload;
                    }
                    // Scroll right
                    KeyCode::Right | KeyCode::Char('l') => {
                        self.scroll_x += 1;
                        result.redraw = true;
                    }
                    // Scroll left
                    KeyCode::Left | KeyCode::Char('h') => {
                        self.scroll_x = self.scroll_x.saturating_sub(1);
                        result.redraw = true;
                    }
                    // Cycle through function relocation diff mode
                    KeyCode::Char('x') => {
                        state.diff_obj_config.function_reloc_diffs =
                            match state.diff_obj_config.function_reloc_diffs {
                                FunctionRelocDiffs::None => FunctionRelocDiffs::NameAddress,
                                FunctionRelocDiffs::NameAddress => FunctionRelocDiffs::DataValue,
                                FunctionRelocDiffs::DataValue => FunctionRelocDiffs::All,
                                FunctionRelocDiffs::All => FunctionRelocDiffs::None,
                            };
                        result.redraw = true;
                        return EventControlFlow::Reload;
                    }
                    // Toggle three-way diff
                    KeyCode::Char('3') => {
                        self.three_way = !self.three_way;
                        result.redraw = true;
                    }
                    // Toggle options
                    KeyCode::Char('o') => {
                        self.open_options = !self.open_options;
                        result.redraw = true;
                    }
                    _ => {}
                }
            }
            Event::Mouse(event) => match event.kind {
                MouseEventKind::ScrollDown => {
                    self.scroll_y += 3;
                    result.redraw = true;
                }
                MouseEventKind::ScrollUp => {
                    self.scroll_y = self.scroll_y.saturating_sub(3);
                    result.redraw = true;
                }
                MouseEventKind::ScrollRight => {
                    self.scroll_x += 3;
                    result.redraw = true;
                }
                MouseEventKind::ScrollLeft => {
                    self.scroll_x = self.scroll_x.saturating_sub(3);
                    result.redraw = true;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    result.click_xy = Some((event.column, event.row));
                    result.redraw = true;
                }
                _ => {}
            },
            Event::Resize(_, _) => {
                result.redraw = true;
            }
            _ => {}
        }
        EventControlFlow::Continue(result)
    }

    fn reload(&mut self, state: &AppState) -> Result<()> {
        let left_sym =
            state.left_obj.as_ref().and_then(|(o, _)| o.symbol_by_name(&self.symbol_name));
        let right_sym =
            state.right_obj.as_ref().and_then(|(o, _)| o.symbol_by_name(&self.symbol_name));
        let prev_sym =
            state.prev_obj.as_ref().and_then(|(o, _)| o.symbol_by_name(&self.symbol_name));
        self.num_rows = match (
            get_symbol(state.left_obj.as_ref(), left_sym),
            get_symbol(state.right_obj.as_ref(), right_sym),
        ) {
            (Some((_l, _ls, ld)), Some((_r, _rs, rd))) => {
                ld.instruction_rows.len().max(rd.instruction_rows.len())
            }
            (Some((_l, _ls, ld)), None) => ld.instruction_rows.len(),
            (None, Some((_r, _rs, rd))) => rd.instruction_rows.len(),
            (None, None) => 0,
        };
        self.left_sym = left_sym;
        self.right_sym = right_sym;
        self.prev_sym = prev_sym;
        Ok(())
    }
}

impl FunctionDiffUi {
    pub fn draw_options(&mut self, f: &mut Frame, _result: &mut EventResult) {
        let percent_x = 50;
        let percent_y = 50;
        let popup_rect = Layout::vertical([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(f.area())[1];
        let popup_rect = Layout::horizontal([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_rect)[1];

        let popup = Block::default()
            .borders(Borders::ALL)
            .title("Options")
            .title_style(Style::default().fg(Color::White).bg(Color::Black));
        f.render_widget(Clear, popup_rect);
        f.render_widget(popup, popup_rect);
    }

    fn page_up(&mut self, half: bool) {
        self.scroll_y = self.scroll_y.saturating_sub(self.per_page / if half { 2 } else { 1 });
    }

    fn page_down(&mut self, half: bool) {
        self.scroll_y += self.per_page / if half { 2 } else { 1 };
    }

    fn print_sym(
        &self,
        out: &mut Text<'static>,
        obj: &Object,
        symbol_index: usize,
        symbol_diff: &SymbolDiff,
        diff_config: &DiffObjConfig,
        rect: Rect,
        highlight: &HighlightKind,
        result: &EventResult,
        only_changed: bool,
    ) -> Option<HighlightKind> {
        let mut new_highlight = None;
        for (y, ins_row) in symbol_diff
            .instruction_rows
            .iter()
            .skip(self.scroll_y)
            .take(rect.height as usize)
            .enumerate()
        {
            if only_changed && ins_row.kind == InstructionDiffKind::None {
                out.lines.push(Line::default());
                continue;
            }
            let mut sx = rect.x;
            let sy = rect.y + y as u16;
            let mut line = Line::default();
            display_row(obj, symbol_index, ins_row, diff_config, |segment| {
                let highlight_kind = HighlightKind::from(&segment.text);
                let label_text = match segment.text {
                    DiffText::Basic(text) => text.to_string(),
                    DiffText::Line(num) => format!("{num} "),
                    DiffText::Address(addr) => format!("{addr:x}:"),
                    DiffText::Opcode(mnemonic, _op) => format!("{mnemonic} "),
                    DiffText::Argument(arg) => arg.to_string(),
                    DiffText::BranchDest(addr) => format!("{addr:x}"),
                    DiffText::Symbol(sym) => {
                        sym.demangled_name.as_ref().unwrap_or(&sym.name).clone()
                    }
                    DiffText::Addend(addend) => match addend.cmp(&0i64) {
                        Ordering::Greater => format!("+{addend:#x}"),
                        Ordering::Less => format!("-{:#x}", -addend),
                        _ => String::new(),
                    },
                    DiffText::Spacing(n) => {
                        line.spans.push(Span::raw(" ".repeat(n as usize)));
                        sx += n as u16;
                        return Ok(());
                    }
                    DiffText::Eol => return Ok(()),
                };

                let len = label_text.len();
                let highlighted =
                    highlight_kind != HighlightKind::None && *highlight == highlight_kind;
                if let Some((cx, cy)) = result.click_xy
                    && cx >= sx
                    && cx < sx + len as u16
                    && cy == sy
                {
                    new_highlight = Some(highlight_kind);
                }
                let mut style = Style::new().fg(match segment.color {
                    DiffTextColor::Normal => Color::Gray,
                    DiffTextColor::Dim => Color::DarkGray,
                    DiffTextColor::Bright => Color::White,
                    DiffTextColor::DataFlow => Color::LightCyan,
                    DiffTextColor::Replace => Color::Cyan,
                    DiffTextColor::Delete => Color::Red,
                    DiffTextColor::Insert => Color::Green,
                    DiffTextColor::Rotating(i) => COLOR_ROTATION[i as usize % COLOR_ROTATION.len()],
                });
                if highlighted {
                    style = style.bg(Color::DarkGray);
                }
                line.spans.push(Span::styled(label_text, style));
                sx += len as u16;
                if segment.pad_to as usize > len {
                    let pad = (segment.pad_to as usize - len) as u16;
                    line.spans.push(Span::raw(" ".repeat(pad as usize)));
                    sx += pad;
                }
                Ok(())
            })
            .unwrap();
            out.lines.push(line);
        }
        new_highlight
    }

    fn print_margin(&self, out: &mut Text, symbol: &SymbolDiff, rect: Rect) {
        for ins_row in symbol.instruction_rows.iter().skip(self.scroll_y).take(rect.height as usize)
        {
            if ins_row.kind != InstructionDiffKind::None {
                out.lines.push(Line::raw(match ins_row.kind {
                    InstructionDiffKind::Delete => "<",
                    InstructionDiffKind::Insert => ">",
                    _ => "|",
                }));
            } else {
                out.lines.push(Line::raw(" "));
            }
        }
    }

    fn print_build_status<'a>(&self, out: &mut Text<'a>, status: &'a BuildStatus) {
        if !status.cmdline.is_empty() {
            out.lines.push(Line::styled(status.cmdline.clone(), Style::new().fg(Color::LightBlue)));
        }
        for line in status.stdout.lines() {
            out.lines.push(Line::styled(line, Style::new().fg(Color::White)));
        }
        for line in status.stderr.lines() {
            out.lines.push(Line::styled(line, Style::new().fg(Color::Red)));
        }
    }
}

pub const COLOR_ROTATION: [Color; 7] = [
    Color::Magenta,
    Color::Cyan,
    Color::Green,
    Color::Red,
    Color::Yellow,
    Color::Blue,
    Color::Green,
];

pub fn match_percent_color(match_percent: f32) -> Color {
    if match_percent == 100.0 {
        Color::Green
    } else if match_percent >= 50.0 {
        Color::LightBlue
    } else {
        Color::LightRed
    }
}

#[inline]
fn get_symbol(
    obj: Option<&(Object, ObjectDiff)>,
    sym: Option<usize>,
) -> Option<(&Object, usize, &SymbolDiff)> {
    let (obj, diff) = obj?;
    let sym = sym?;
    Some((obj, sym, &diff.symbols[sym]))
}
