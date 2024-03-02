use std::{fs, io::stdout, path::PathBuf, str::FromStr};

use anyhow::{bail, Context, Result};
use argp::FromArgs;
use crossterm::{
    event,
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
    },
};
use event::KeyModifiers;
use objdiff_core::{
    config::{ProjectConfig, ProjectObject},
    diff,
    diff::display::{display_diff, DiffText, HighlightKind},
    obj,
    obj::{ObjInfo, ObjInsDiffKind, ObjSectionKind, ObjSymbol},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use crate::util::term::crossterm_panic_handler;

#[derive(FromArgs, PartialEq, Debug)]
/// Diff two object files.
#[argp(subcommand, name = "diff")]
pub struct Args {
    #[argp(option, short = '1')]
    /// Target object file
    target: Option<PathBuf>,
    #[argp(option, short = '2')]
    /// Base object file
    base: Option<PathBuf>,
    #[argp(option, short = 'p')]
    /// Project directory
    project: Option<PathBuf>,
    #[argp(option, short = 'u')]
    /// Unit name within project
    unit: Option<String>,
    #[argp(switch, short = 'x')]
    /// Relax relocation diffs
    relax_reloc_diffs: bool,
    #[argp(positional)]
    /// Function symbol to diff
    symbol: String,
}

pub fn run(args: Args) -> Result<()> {
    let (target_path, base_path, project_config) =
        match (&args.target, &args.base, &args.project, &args.unit) {
            (Some(t), Some(b), None, None) => (Some(t.clone()), Some(b.clone()), None),
            (None, None, p, u) => {
                let project = match p {
                    Some(project) => project.clone(),
                    _ => std::env::current_dir().context("Failed to get the current directory")?,
                };
                let Some((project_config, project_config_info)) =
                    objdiff_core::config::try_project_config(&project)
                else {
                    bail!("Project config not found in {}", &project.display())
                };
                let mut project_config = project_config.with_context(|| {
                    format!("Reading project config {}", project_config_info.path.display())
                })?;
                let object = {
                    let resolve_paths = |o: &mut ProjectObject| {
                        o.resolve_paths(
                            &project,
                            project_config.target_dir.as_deref(),
                            project_config.base_dir.as_deref(),
                        )
                    };
                    if let Some(u) = u {
                        let unit_path =
                            PathBuf::from_str(u).ok().and_then(|p| fs::canonicalize(p).ok());

                        let Some(object) = project_config.objects.iter_mut().find_map(|obj| {
                            if obj.name.as_deref() == Some(u) {
                                resolve_paths(obj);
                                return Some(obj);
                            }

                            let Some(up) = unit_path.as_deref() else {
                                return None;
                            };

                            resolve_paths(obj);

                            if [&obj.base_path, &obj.target_path]
                                .into_iter()
                                .filter_map(|p| p.as_ref().and_then(|p| p.canonicalize().ok()))
                                .any(|p| p == up)
                            {
                                return Some(obj);
                            }

                            None
                        }) else {
                            bail!("Unit not found: {}", u)
                        };

                        object
                    } else {
                        let mut idx = None;
                        let mut count = 0usize;
                        for (i, obj) in project_config.objects.iter_mut().enumerate() {
                            resolve_paths(obj);

                            if obj
                                .target_path
                                .as_deref()
                                .map(|o| obj::elf::has_function(o, &args.symbol))
                                .transpose()?
                                .unwrap_or(false)
                            {
                                idx = Some(i);
                                count += 1;
                                if count > 1 {
                                    break;
                                }
                            }
                        }
                        match (count, idx) {
                            (0, None) => bail!("Symbol not found: {}", &args.symbol),
                            (1, Some(i)) => &mut project_config.objects[i],
                            (2.., Some(_)) => bail!(
                                "Multiple instances of {} were found, try specifying a unit",
                                &args.symbol
                            ),
                            _ => unreachable!(),
                        }
                    }
                };
                let target_path = object.target_path.clone();
                let base_path = object.base_path.clone();
                (target_path, base_path, Some(project_config))
            }
            _ => bail!("Either target and base or project and unit must be specified"),
        };
    let time_format = time::format_description::parse_borrowed::<2>("[hour]:[minute]:[second]")
        .context("Failed to parse time format")?;
    let mut state = Box::new(FunctionDiffUi {
        redraw: true,
        relax_reloc_diffs: args.relax_reloc_diffs,
        click_xy: None,
        left_highlight: HighlightKind::None,
        right_highlight: HighlightKind::None,
        scroll_x: 0,
        scroll_state_x: ScrollbarState::default(),
        scroll_y: 0,
        scroll_state_y: ScrollbarState::default(),
        per_page: 0,
        num_rows: 0,
        symbol_name: args.symbol.clone(),
        target_path,
        base_path,
        project_config,
        left_sym: None,
        right_sym: None,
        reload_time: None,
        time_format,
    });
    state.reload()?;

    crossterm_panic_handler();
    enable_raw_mode()?;
    crossterm::queue!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        SetTitle(format!("{} - objdiff", args.symbol)),
    )?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    'outer: loop {
        loop {
            if state.redraw {
                terminal.draw(|f| state.draw(f))?;
                if state.redraw {
                    continue;
                }
            }
            match state.handle_event(event::read()?) {
                FunctionDiffResult::Break => break 'outer,
                FunctionDiffResult::Continue => {}
                FunctionDiffResult::Reload => break,
            }
        }
        state.reload()?;
    }

    // Reset terminal
    disable_raw_mode()?;
    crossterm::execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    Ok(())
}

fn find_function(obj: &ObjInfo, name: &str) -> Option<ObjSymbol> {
    for section in &obj.sections {
        if section.kind != ObjSectionKind::Code {
            continue;
        }
        for symbol in &section.symbols {
            if symbol.name == name {
                return Some(symbol.clone());
            }
        }
    }
    None
}

#[allow(dead_code)]
struct FunctionDiffUi {
    redraw: bool,
    relax_reloc_diffs: bool,
    click_xy: Option<(u16, u16)>,
    left_highlight: HighlightKind,
    right_highlight: HighlightKind,
    scroll_x: usize,
    scroll_state_x: ScrollbarState,
    scroll_y: usize,
    scroll_state_y: ScrollbarState,
    per_page: usize,
    num_rows: usize,
    symbol_name: String,
    target_path: Option<PathBuf>,
    base_path: Option<PathBuf>,
    project_config: Option<ProjectConfig>,
    left_sym: Option<ObjSymbol>,
    right_sym: Option<ObjSymbol>,
    reload_time: Option<time::OffsetDateTime>,
    time_format: Vec<time::format_description::FormatItem<'static>>,
}

enum FunctionDiffResult {
    Break,
    Continue,
    Reload,
}

impl FunctionDiffUi {
    fn draw(&mut self, f: &mut Frame) {
        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).split(f.size());
        let header_chunks = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .split(chunks[0]);
        let content_chunks = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .split(chunks[1]);

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
        if let Some(percent) = self.right_sym.as_ref().and_then(|s| s.match_percent) {
            line_r.spans.push(Span::styled(
                format!("{:.2}% ", percent),
                Style::new().fg(match_percent_color(percent)),
            ));
        }
        let reload_time = self
            .reload_time
            .as_ref()
            .and_then(|t| t.format(&self.time_format).ok())
            .unwrap_or_else(|| "N/A".to_string());
        line_r.spans.push(Span::styled(
            format!("Last reload: {}", reload_time),
            Style::new().fg(Color::White),
        ));
        f.render_widget(line_r, header_chunks[2]);

        let create_block =
            |title: &'static str| Block::new().borders(Borders::TOP).gray().title(title.bold());

        let mut left_highlight = None;
        let mut max_width = 0;
        if let Some(symbol) = &self.left_sym {
            // Render left column
            let mut text = Text::default();
            let rect = content_chunks[0].inner(&Margin::new(0, 1));
            let h = self.print_sym(&mut text, symbol, rect, &self.left_highlight);
            max_width = max_width.max(text.width());
            f.render_widget(
                Paragraph::new(text)
                    .block(create_block("TARGET"))
                    .scroll((0, self.scroll_x as u16)),
                content_chunks[0],
            );
            if let Some(h) = h {
                left_highlight = Some(h);
            }
        }

        let mut right_highlight = None;
        if let Some(symbol) = &self.right_sym {
            // Render margin
            let mut text = Text::default();
            let rect = content_chunks[1].inner(&Margin::new(1, 1));
            self.print_margin(&mut text, symbol, rect);
            f.render_widget(text, rect);

            // Render right column
            let mut text = Text::default();
            let rect = content_chunks[2].inner(&Margin::new(0, 1));
            let h = self.print_sym(&mut text, symbol, rect, &self.right_highlight);
            max_width = max_width.max(text.width());
            f.render_widget(
                Paragraph::new(text)
                    .block(create_block("CURRENT"))
                    .scroll((0, self.scroll_x as u16)),
                content_chunks[2],
            );
            if let Some(h) = h {
                right_highlight = Some(h);
            }
        }

        let max_scroll_x =
            max_width.saturating_sub(content_chunks[0].width.min(content_chunks[2].width) as usize);
        if self.scroll_x > max_scroll_x {
            self.scroll_x = max_scroll_x;
        }
        self.scroll_state_x =
            self.scroll_state_x.content_length(max_scroll_x).position(self.scroll_x);

        // Render scrollbars
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None),
            chunks[1].inner(&Margin::new(0, 1)),
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
            self.redraw = true;
            self.click_xy = None;
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
            self.redraw = true;
            self.click_xy = None;
        } else {
            self.redraw = false;
            self.click_xy = None;
        }
    }

    fn handle_event(&mut self, event: Event) -> FunctionDiffResult {
        match event {
            Event::Key(event)
                if matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                match event.code {
                    // Quit
                    KeyCode::Esc | KeyCode::Char('q') => return FunctionDiffResult::Break,
                    // Page up
                    KeyCode::PageUp => {
                        self.scroll_y = self.scroll_y.saturating_sub(self.per_page);
                        self.redraw = true;
                    }
                    // Page up (shift + space)
                    KeyCode::Char(' ') if event.modifiers.contains(KeyModifiers::SHIFT) => {
                        self.scroll_y = self.scroll_y.saturating_sub(self.per_page);
                        self.redraw = true;
                    }
                    // Page down
                    KeyCode::Char(' ') | KeyCode::PageDown => {
                        self.scroll_y += self.per_page;
                        self.redraw = true;
                    }
                    // Page down (ctrl + f)
                    KeyCode::Char('f') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll_y += self.per_page;
                        self.redraw = true;
                    }
                    // Page up (ctrl + b)
                    KeyCode::Char('b') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll_y = self.scroll_y.saturating_sub(self.per_page);
                        self.redraw = true;
                    }
                    // Half page down (ctrl + d)
                    KeyCode::Char('d') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll_y += self.per_page / 2;
                        self.redraw = true;
                    }
                    // Half page up (ctrl + u)
                    KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll_y = self.scroll_y.saturating_sub(self.per_page / 2);
                        self.redraw = true;
                    }
                    // Scroll down
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.scroll_y += 1;
                        self.redraw = true;
                    }
                    // Scroll up
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.scroll_y = self.scroll_y.saturating_sub(1);
                        self.redraw = true;
                    }
                    // Scroll to start
                    KeyCode::Char('g') => {
                        self.scroll_y = 0;
                        self.redraw = true;
                    }
                    // Scroll to end
                    KeyCode::Char('G') => {
                        self.scroll_y = self.num_rows;
                        self.redraw = true;
                    }
                    // Reload
                    KeyCode::Char('r') => {
                        self.redraw = true;
                        return FunctionDiffResult::Reload;
                    }
                    // Scroll right
                    KeyCode::Right | KeyCode::Char('l') => {
                        self.scroll_x += 1;
                        self.redraw = true;
                    }
                    // Scroll left
                    KeyCode::Left | KeyCode::Char('h') => {
                        self.scroll_x = self.scroll_x.saturating_sub(1);
                        self.redraw = true;
                    }
                    // Toggle relax relocation diffs
                    KeyCode::Char('x') => {
                        self.relax_reloc_diffs = !self.relax_reloc_diffs;
                        self.redraw = true;
                        return FunctionDiffResult::Reload;
                    }
                    _ => {}
                }
            }
            Event::Mouse(event) => match event.kind {
                MouseEventKind::ScrollDown => {
                    self.scroll_y += 3;
                    self.redraw = true;
                }
                MouseEventKind::ScrollUp => {
                    self.scroll_y = self.scroll_y.saturating_sub(3);
                    self.redraw = true;
                }
                MouseEventKind::ScrollRight => {
                    self.scroll_x += 3;
                    self.redraw = true;
                }
                MouseEventKind::ScrollLeft => {
                    self.scroll_x = self.scroll_x.saturating_sub(3);
                    self.redraw = true;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    self.click_xy = Some((event.column, event.row));
                    self.redraw = true;
                }
                _ => {}
            },
            Event::Resize(_, _) => {
                self.redraw = true;
            }
            _ => {}
        }
        FunctionDiffResult::Continue
    }

    fn print_sym(
        &self,
        out: &mut Text,
        symbol: &ObjSymbol,
        rect: Rect,
        highlight: &HighlightKind,
    ) -> Option<HighlightKind> {
        let base_addr = symbol.address as u32;
        let mut new_highlight = None;
        for (y, ins_diff) in
            symbol.instructions.iter().skip(self.scroll_y).take(rect.height as usize).enumerate()
        {
            let mut sx = rect.x;
            let sy = rect.y + y as u16;
            let mut line = Line::default();
            display_diff(ins_diff, base_addr, |text| -> Result<()> {
                let label_text;
                let mut base_color = match ins_diff.kind {
                    ObjInsDiffKind::None
                    | ObjInsDiffKind::OpMismatch
                    | ObjInsDiffKind::ArgMismatch => Color::Gray,
                    ObjInsDiffKind::Replace => Color::Cyan,
                    ObjInsDiffKind::Delete => Color::Red,
                    ObjInsDiffKind::Insert => Color::Green,
                };
                let mut pad_to = 0;
                match text {
                    DiffText::Basic(text) => {
                        label_text = text.to_string();
                    }
                    DiffText::BasicColor(s, idx) => {
                        label_text = s.to_string();
                        base_color = COLOR_ROTATION[idx % COLOR_ROTATION.len()];
                    }
                    DiffText::Line(num) => {
                        label_text = format!("{num} ");
                        base_color = Color::DarkGray;
                        pad_to = 5;
                    }
                    DiffText::Address(addr) => {
                        label_text = format!("{:x}:", addr);
                        pad_to = 5;
                    }
                    DiffText::Opcode(mnemonic, _op) => {
                        label_text = mnemonic.to_string();
                        if ins_diff.kind == ObjInsDiffKind::OpMismatch {
                            base_color = Color::Blue;
                        }
                        pad_to = 8;
                    }
                    DiffText::Argument(arg, diff) => {
                        label_text = arg.to_string();
                        if let Some(diff) = diff {
                            base_color = COLOR_ROTATION[diff.idx % COLOR_ROTATION.len()]
                        }
                    }
                    DiffText::BranchTarget(addr) => {
                        label_text = format!("{addr:x}");
                    }
                    DiffText::Symbol(sym) => {
                        let name = sym.demangled_name.as_ref().unwrap_or(&sym.name);
                        label_text = name.clone();
                        base_color = Color::White;
                    }
                    DiffText::Spacing(n) => {
                        line.spans.push(Span::raw(" ".repeat(n)));
                        sx += n as u16;
                        return Ok(());
                    }
                    DiffText::Eol => {
                        return Ok(());
                    }
                }
                let len = label_text.len();
                let highlighted = *highlight == text;
                if let Some((cx, cy)) = self.click_xy {
                    if cx >= sx && cx < sx + len as u16 && cy == sy {
                        new_highlight = Some(text.into());
                    }
                }
                let mut style = Style::new().fg(base_color);
                if highlighted {
                    style = style.bg(Color::DarkGray);
                }
                line.spans.push(Span::styled(label_text, style));
                sx += len as u16;
                if pad_to > len {
                    let pad = (pad_to - len) as u16;
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

    fn print_margin(&self, out: &mut Text, symbol: &ObjSymbol, rect: Rect) {
        for ins_diff in symbol.instructions.iter().skip(self.scroll_y).take(rect.height as usize) {
            if ins_diff.kind != ObjInsDiffKind::None {
                out.lines.push(Line::raw(match ins_diff.kind {
                    ObjInsDiffKind::Delete => "<",
                    ObjInsDiffKind::Insert => ">",
                    _ => "|",
                }));
            } else {
                out.lines.push(Line::raw(" "));
            }
        }
    }

    fn reload(&mut self) -> Result<()> {
        let mut target = self
            .target_path
            .as_deref()
            .map(|p| obj::elf::read(p).with_context(|| format!("Loading {}", p.display())))
            .transpose()?;
        let mut base = self
            .base_path
            .as_deref()
            .map(|p| obj::elf::read(p).with_context(|| format!("Loading {}", p.display())))
            .transpose()?;
        let config = diff::DiffObjConfig { relax_reloc_diffs: self.relax_reloc_diffs };
        diff::diff_objs(&config, target.as_mut(), base.as_mut())?;

        let left_sym = target.as_ref().and_then(|o| find_function(o, &self.symbol_name));
        let right_sym = base.as_ref().and_then(|o| find_function(o, &self.symbol_name));
        self.num_rows = match (&left_sym, &right_sym) {
            (Some(l), Some(r)) => l.instructions.len().max(r.instructions.len()),
            (Some(l), None) => l.instructions.len(),
            (None, Some(r)) => r.instructions.len(),
            (None, None) => bail!("Symbol not found: {}", self.symbol_name),
        };
        self.left_sym = left_sym;
        self.right_sym = right_sym;
        self.reload_time = time::OffsetDateTime::now_local().ok();
        Ok(())
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
