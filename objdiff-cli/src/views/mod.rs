use anyhow::Result;
use crossterm::event::Event;
use ratatui::Frame;

use crate::cmd::diff::AppState;

pub mod function_diff;

#[derive(Default)]
pub struct EventResult {
    pub redraw: bool,
    pub click_xy: Option<(u16, u16)>,
}

pub enum EventControlFlow {
    Break,
    Continue(EventResult),
    Reload,
}

pub trait UiView {
    fn draw(&mut self, state: &AppState, f: &mut Frame, result: &mut EventResult);
    fn handle_event(&mut self, state: &mut AppState, event: Event) -> EventControlFlow;
    fn reload(&mut self, state: &AppState) -> Result<()>;
}
