use std::{io::stdout, panic};

use crossterm::{
    cursor::Show,
    event::DisableMouseCapture,
    terminal::{LeaveAlternateScreen, disable_raw_mode},
};

pub fn crossterm_panic_handler() {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = crossterm::execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture, Show);
        let _ = disable_raw_mode();
        original_hook(panic_info);
    }));
}
