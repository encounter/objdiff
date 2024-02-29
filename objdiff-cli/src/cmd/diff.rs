use std::{
    io::{stdout, Write},
    path::PathBuf,
};

use anyhow::{bail, Context, Result};
use argp::FromArgs;
use crossterm::{
    cursor::{Hide, MoveRight, MoveTo, Show},
    event,
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    style::{Color, PrintStyledContent, Stylize},
    terminal::{
        disable_raw_mode, enable_raw_mode, size as terminal_size, Clear, ClearType,
        EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
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
    #[argp(positional)]
    /// Function symbol to diff
    symbol: String,
}

pub fn run(args: Args) -> Result<()> {
    let (target_path, base_path, project_config) =
        match (&args.target, &args.base, &args.project, &args.unit) {
            (Some(t), Some(b), _, _) => (Some(t.clone()), Some(b.clone()), None),
            (_, _, p, u) => {
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
                        let Some(object) =
                            project_config.objects.iter_mut().find(|obj| obj.name() == u)
                        else {
                            bail!("Unit not found: {}", u)
                        };
                        resolve_paths(object);
                        object
                    } else {
                        let mut idx = None;
                        let mut count = 0usize;
                        for (i, obj) in project_config.objects.iter_mut().enumerate() {
                            resolve_paths(obj);
                            if load_obj(&obj.target_path)?
                                .and_then(|o| find_function(&o, &args.symbol))
                                .is_some()
                            {
                                idx = Some(i);
                                count += 1;
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
        };
    let mut state = FunctionDiffUi {
        clear: true,
        redraw: true,
        size: (0, 0),
        click_xy: None,
        left_highlight: HighlightKind::None,
        right_highlight: HighlightKind::None,
        skip: 0,
        y_offset: 2,
        per_page: 0,
        max_len: 0,
        symbol_name: args.symbol.clone(),
        target_path,
        base_path,
        project_config,
        left_sym: None,
        right_sym: None,
        reload_time: time::OffsetDateTime::now_local()?,
    };
    state.reload()?;

    crossterm_panic_handler();
    enable_raw_mode()?;
    crossterm::queue!(
        stdout(),
        EnterAlternateScreen,
        SetTitle(format!("{} - objdiff", args.symbol)),
        Hide,
        EnableMouseCapture,
    )?;
    state.size = terminal_size()?;

    loop {
        let reload = loop {
            if state.redraw {
                state.draw()?;
                if state.redraw {
                    continue;
                }
            }
            match state.handle_event(event::read()?) {
                FunctionDiffResult::Break => break false,
                FunctionDiffResult::Continue => {}
                FunctionDiffResult::Reload => break true,
            }
        };
        if reload {
            state.reload()?;
        } else {
            break;
        }
    }

    // Reset terminal
    crossterm::execute!(stdout(), LeaveAlternateScreen, Show, DisableMouseCapture)?;
    disable_raw_mode()?;
    Ok(())
}

fn load_obj(path: &Option<PathBuf>) -> Result<Option<ObjInfo>> {
    path.as_deref()
        .map(|p| obj::elf::read(p).with_context(|| format!("Loading {}", p.display())))
        .transpose()
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
    clear: bool,
    redraw: bool,
    size: (u16, u16),
    click_xy: Option<(u16, u16)>,
    left_highlight: HighlightKind,
    right_highlight: HighlightKind,
    skip: usize,
    y_offset: usize,
    per_page: usize,
    max_len: usize,
    symbol_name: String,
    target_path: Option<PathBuf>,
    base_path: Option<PathBuf>,
    project_config: Option<ProjectConfig>,
    left_sym: Option<ObjSymbol>,
    right_sym: Option<ObjSymbol>,
    reload_time: time::OffsetDateTime,
}

enum FunctionDiffResult {
    Break,
    Continue,
    Reload,
}

impl FunctionDiffUi {
    fn draw(&mut self) -> Result<()> {
        let mut w = stdout().lock();
        if self.clear {
            crossterm::queue!(w, Clear(ClearType::All))?;
        }
        let format = time::format_description::parse("[hour]:[minute]:[second]").unwrap();
        let reload_time = self.reload_time.format(&format).unwrap();
        crossterm::queue!(
            w,
            MoveTo(0, 0),
            PrintStyledContent(self.symbol_name.clone().with(Color::White)),
            MoveTo(0, 1),
            PrintStyledContent(" ".repeat(self.size.0 as usize).underlined()),
            MoveTo(0, 1),
            PrintStyledContent("TARGET ".underlined()),
            MoveTo(self.size.0 / 2, 0),
            PrintStyledContent(format!("Last reload: {}", reload_time).with(Color::White)),
            MoveTo(self.size.0 / 2, 1),
            PrintStyledContent("BASE ".underlined()),
        )?;
        if let Some(percent) = self.right_sym.as_ref().and_then(|s| s.match_percent) {
            crossterm::queue!(
                w,
                PrintStyledContent(
                    format!("{:.2}%", percent).with(match_percent_color(percent)).underlined()
                )
            )?;
        }

        self.per_page = self.size.1 as usize - self.y_offset;
        let max_skip = self.max_len.saturating_sub(self.per_page);
        if self.skip > max_skip {
            self.skip = max_skip;
        }
        let mut left_highlight = None;
        if let Some(symbol) = &self.left_sym {
            let h = self.print_sym(
                &mut w,
                symbol,
                (0, self.y_offset as u16),
                (self.size.0 / 2 - 1, self.size.1),
                &self.left_highlight,
            )?;
            if let Some(h) = h {
                left_highlight = Some(h);
            }
        }
        let mut right_highlight = None;
        if let Some(symbol) = &self.right_sym {
            let h = self.print_sym(
                &mut w,
                symbol,
                (self.size.0 / 2, self.y_offset as u16),
                self.size,
                &self.right_highlight,
            )?;
            if let Some(h) = h {
                right_highlight = Some(h);
            }
        }
        w.flush()?;
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
            self.clear = false;
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
            self.clear = false;
        } else {
            self.redraw = false;
            self.click_xy = None;
            self.clear = true;
        }
        Ok(())
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
                        self.skip = self.skip.saturating_sub(self.per_page);
                        self.redraw = true;
                    }
                    // Page up (shift + space)
                    KeyCode::Char(' ') if event.modifiers.contains(KeyModifiers::SHIFT) => {
                        self.skip = self.skip.saturating_sub(self.per_page);
                        self.redraw = true;
                    }
                    // Page down
                    KeyCode::Char(' ') | KeyCode::PageDown => {
                        self.skip += self.per_page;
                        self.redraw = true;
                    }
                    // Page down (ctrl + f)
                    KeyCode::Char('f') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.skip += self.per_page;
                        self.redraw = true;
                    }
                    // Page up (ctrl + b)
                    KeyCode::Char('b') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.skip = self.skip.saturating_sub(self.per_page);
                        self.redraw = true;
                    }
                    // Half page down (ctrl + d)
                    KeyCode::Char('d') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.skip += self.per_page / 2;
                        self.redraw = true;
                    }
                    // Half page up (ctrl + u)
                    KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.skip = self.skip.saturating_sub(self.per_page / 2);
                        self.redraw = true;
                    }
                    // Scroll down
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.skip += 1;
                        self.redraw = true;
                    }
                    // Scroll up
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.skip = self.skip.saturating_sub(1);
                        self.redraw = true;
                    }
                    // Scroll to start
                    KeyCode::Char('g') => {
                        self.skip = 0;
                        self.redraw = true;
                    }
                    // Scroll to end
                    KeyCode::Char('G') => {
                        self.skip = self.max_len;
                        self.redraw = true;
                    }
                    // Reload
                    KeyCode::Char('r') => {
                        self.redraw = true;
                        return FunctionDiffResult::Reload;
                    }
                    _ => {}
                }
            }
            Event::Mouse(event) => match event.kind {
                MouseEventKind::ScrollDown => {
                    self.skip += 3;
                    self.redraw = true;
                }
                MouseEventKind::ScrollUp => {
                    self.skip = self.skip.saturating_sub(3);
                    self.redraw = true;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    self.click_xy = Some((event.column, event.row));
                    self.redraw = true;
                }
                _ => {}
            },
            Event::Resize(x, y) => {
                self.size = (x, y);
                self.redraw = true;
            }
            _ => {}
        }
        FunctionDiffResult::Continue
    }

    fn print_sym<W>(
        &self,
        w: &mut W,
        symbol: &ObjSymbol,
        origin: (u16, u16),
        max: (u16, u16),
        highlight: &HighlightKind,
    ) -> Result<Option<HighlightKind>>
    where
        W: Write,
    {
        let base_addr = symbol.address as u32;
        let mut new_highlight = None;
        let mut sy = origin.1;
        for ins_diff in symbol.instructions.iter().skip(self.skip) {
            let mut sx = origin.0;
            if ins_diff.kind != ObjInsDiffKind::None && sx > 2 {
                crossterm::queue!(w, MoveTo(sx - 2, sy))?;
                let s = match ins_diff.kind {
                    ObjInsDiffKind::Delete => "< ",
                    ObjInsDiffKind::Insert => "> ",
                    _ => "| ",
                };
                crossterm::queue!(w, PrintStyledContent(s.with(Color::DarkGrey)))?;
            } else {
                crossterm::queue!(w, MoveTo(sx, sy))?;
            }
            display_diff(ins_diff, base_addr, |text| -> Result<()> {
                let mut label_text;
                let mut base_color = match ins_diff.kind {
                    ObjInsDiffKind::None
                    | ObjInsDiffKind::OpMismatch
                    | ObjInsDiffKind::ArgMismatch => Color::Grey,
                    ObjInsDiffKind::Replace => Color::DarkCyan,
                    ObjInsDiffKind::Delete => Color::DarkRed,
                    ObjInsDiffKind::Insert => Color::DarkGreen,
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
                        base_color = Color::DarkGrey;
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
                        crossterm::queue!(w, MoveRight(n as u16))?;
                        sx += n as u16;
                        return Ok(());
                    }
                    DiffText::Eol => {
                        sy += 1;
                        return Ok(());
                    }
                }
                let len = label_text.len();
                if sx >= max.0 {
                    return Ok(());
                }
                let highlighted = *highlight == text;
                if let Some((cx, cy)) = self.click_xy {
                    if cx >= sx && cx < sx + len as u16 && cy == sy {
                        new_highlight = Some(text.into());
                    }
                }
                label_text.truncate(max.0 as usize - sx as usize);
                let mut content = label_text.with(base_color);
                if highlighted {
                    content = content.on_dark_grey();
                }
                crossterm::queue!(w, PrintStyledContent(content))?;
                sx += len as u16;
                if pad_to > len {
                    let pad = (pad_to - len) as u16;
                    crossterm::queue!(w, MoveRight(pad))?;
                    sx += pad;
                }
                Ok(())
            })?;
            if sy >= max.1 {
                break;
            }
        }
        Ok(new_highlight)
    }

    fn reload(&mut self) -> Result<()> {
        let mut target = load_obj(&self.target_path)?;
        let mut base = load_obj(&self.base_path)?;
        let config = diff::DiffObjConfig::default();
        diff::diff_objs(&config, target.as_mut(), base.as_mut())?;

        let left_sym = target.as_ref().and_then(|o| find_function(o, &self.symbol_name));
        let right_sym = base.as_ref().and_then(|o| find_function(o, &self.symbol_name));
        self.max_len = match (&left_sym, &right_sym) {
            (Some(l), Some(r)) => l.instructions.len().max(r.instructions.len()),
            (Some(l), None) => l.instructions.len(),
            (None, Some(r)) => r.instructions.len(),
            (None, None) => bail!("Symbol not found: {}", self.symbol_name),
        };
        self.left_sym = left_sym;
        self.right_sym = right_sym;
        self.reload_time = time::OffsetDateTime::now_local()?;
        Ok(())
    }
}

pub const COLOR_ROTATION: [Color; 8] = [
    Color::Magenta,
    Color::Cyan,
    Color::Green,
    Color::Red,
    Color::Yellow,
    Color::DarkMagenta,
    Color::Blue,
    Color::Green,
];

pub fn match_percent_color(match_percent: f32) -> Color {
    if match_percent == 100.0 {
        Color::Green
    } else if match_percent >= 50.0 {
        Color::Blue
    } else {
        Color::Red
    }
}
