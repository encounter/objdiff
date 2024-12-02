use egui::{
    style::ScrollAnimation, text::LayoutJob, vec2, Align, Context, FontSelection, Key,
    KeyboardShortcut, Modifiers, PointerButton, RichText, Ui, WidgetText,
};

fn any_widget_focused(ctx: &Context) -> bool { ctx.memory(|mem| mem.focused().is_some()) }

pub fn enter_pressed(ctx: &Context) -> bool {
    if any_widget_focused(ctx) {
        return false;
    }
    ctx.input_mut(|i| {
        i.key_pressed(Key::Enter)
            || i.key_pressed(Key::Space)
            || i.pointer.button_pressed(PointerButton::Extra2)
    })
}

pub fn back_pressed(ctx: &Context) -> bool {
    if any_widget_focused(ctx) {
        return false;
    }
    ctx.input_mut(|i| {
        i.key_pressed(Key::Backspace)
            || i.key_pressed(Key::Escape)
            || i.pointer.button_pressed(PointerButton::Extra1)
    })
}

pub fn up_pressed(ctx: &Context) -> bool {
    if any_widget_focused(ctx) {
        return false;
    }
    ctx.input_mut(|i| i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::W))
}

pub fn down_pressed(ctx: &Context) -> bool {
    if any_widget_focused(ctx) {
        return false;
    }
    ctx.input_mut(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::S))
}

pub fn page_up_pressed(ctx: &Context) -> bool { ctx.input_mut(|i| i.key_pressed(Key::PageUp)) }

pub fn page_down_pressed(ctx: &Context) -> bool { ctx.input_mut(|i| i.key_pressed(Key::PageDown)) }

pub fn home_pressed(ctx: &Context) -> bool { ctx.input_mut(|i| i.key_pressed(Key::Home)) }

pub fn end_pressed(ctx: &Context) -> bool { ctx.input_mut(|i| i.key_pressed(Key::End)) }

pub fn check_scroll_hotkeys(ui: &mut egui::Ui, include_small_increments: bool) {
    let ui_height = ui.available_rect_before_wrap().height();
    if up_pressed(ui.ctx()) && include_small_increments {
        ui.scroll_with_delta_animation(vec2(0.0, ui_height / 10.0), ScrollAnimation::none());
    } else if down_pressed(ui.ctx()) && include_small_increments {
        ui.scroll_with_delta_animation(vec2(0.0, -ui_height / 10.0), ScrollAnimation::none());
    } else if page_up_pressed(ui.ctx()) {
        ui.scroll_with_delta_animation(vec2(0.0, ui_height), ScrollAnimation::none());
    } else if page_down_pressed(ui.ctx()) {
        ui.scroll_with_delta_animation(vec2(0.0, -ui_height), ScrollAnimation::none());
    } else if home_pressed(ui.ctx()) {
        ui.scroll_with_delta_animation(vec2(0.0, f32::INFINITY), ScrollAnimation::none());
    } else if end_pressed(ui.ctx()) {
        ui.scroll_with_delta_animation(vec2(0.0, -f32::INFINITY), ScrollAnimation::none());
    }
}

pub fn consume_up_key(ctx: &Context) -> bool {
    if any_widget_focused(ctx) {
        return false;
    }
    ctx.input_mut(|i| {
        i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::NONE, Key::W)
    })
}

pub fn consume_down_key(ctx: &Context) -> bool {
    if any_widget_focused(ctx) {
        return false;
    }
    ctx.input_mut(|i| {
        i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::NONE, Key::S)
    })
}

const OBJECT_FILTER_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::F);

pub fn consume_object_filter_shortcut(ctx: &Context) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&OBJECT_FILTER_SHORTCUT))
}

const SYMBOL_FILTER_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::S);

pub fn consume_symbol_filter_shortcut(ctx: &Context) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&SYMBOL_FILTER_SHORTCUT))
}

const CHANGE_TARGET_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::T);

pub fn consume_change_target_shortcut(ctx: &Context) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&CHANGE_TARGET_SHORTCUT))
}

const CHANGE_BASE_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::B);

pub fn consume_change_base_shortcut(ctx: &Context) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&CHANGE_BASE_SHORTCUT))
}

fn shortcut_key(text: &str) -> (usize, char, Key) {
    let i = text.chars().position(|c| c == '_').expect("No underscore in text");
    let key = text.chars().nth(i + 1).expect("No character after underscore");
    let shortcut_key = match key {
        'a' | 'A' => Key::A,
        'b' | 'B' => Key::B,
        'c' | 'C' => Key::C,
        'd' | 'D' => Key::D,
        'e' | 'E' => Key::E,
        'f' | 'F' => Key::F,
        'g' | 'G' => Key::G,
        'h' | 'H' => Key::H,
        'i' | 'I' => Key::I,
        'j' | 'J' => Key::J,
        'k' | 'K' => Key::K,
        'l' | 'L' => Key::L,
        'm' | 'M' => Key::M,
        'n' | 'N' => Key::N,
        'o' | 'O' => Key::O,
        'p' | 'P' => Key::P,
        'q' | 'Q' => Key::Q,
        'r' | 'R' => Key::R,
        's' | 'S' => Key::S,
        't' | 'T' => Key::T,
        'u' | 'U' => Key::U,
        'v' | 'V' => Key::V,
        'w' | 'W' => Key::W,
        'x' | 'X' => Key::X,
        'y' | 'Y' => Key::Y,
        'z' | 'Z' => Key::Z,
        _ => panic!("Invalid key {}", key),
    };
    (i, key, shortcut_key)
}

fn alt_text_ui(ui: &Ui, text: &str, i: usize, key: char, interactive: bool) -> WidgetText {
    let color = if interactive {
        ui.visuals().widgets.inactive.text_color()
    } else {
        ui.visuals().widgets.noninteractive.text_color()
    };
    let mut job = LayoutJob::default();
    if i > 0 {
        let text = &text[..i];
        RichText::new(text).color(color).append_to(
            &mut job,
            ui.style(),
            FontSelection::Default,
            Align::Center,
        );
    }
    let mut rt = RichText::new(key).color(color);
    if ui.input(|i| i.modifiers.alt) {
        rt = rt.underline();
    }
    rt.append_to(&mut job, ui.style(), FontSelection::Default, Align::Center);
    if i < text.len() - 1 {
        let text = &text[i + 2..];
        RichText::new(text).color(color).append_to(
            &mut job,
            ui.style(),
            FontSelection::Default,
            Align::Center,
        );
    }
    job.into()
}

pub fn button_alt_text(ui: &Ui, text: &str) -> WidgetText {
    let (n, c, key) = shortcut_key(text);
    let result = alt_text_ui(ui, text, n, c, true);
    if ui.input_mut(|i| check_alt_key(i, c, key)) {
        let btn_id = ui.next_auto_id();
        ui.memory_mut(|m| m.request_focus(btn_id));
        ui.input_mut(|i| {
            i.events.push(egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            })
        });
    }
    result
}

pub fn alt_text(ui: &Ui, text: &str, enter: bool) -> WidgetText {
    let (n, c, key) = shortcut_key(text);
    let result = alt_text_ui(ui, text, n, c, false);
    if ui.input_mut(|i| check_alt_key(i, c, key)) {
        let btn_id = ui.next_auto_id();
        ui.memory_mut(|m| m.request_focus(btn_id));
        if enter {
            ui.input_mut(|i| {
                i.events.push(egui::Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                })
            });
        }
    }
    result
}

fn check_alt_key(i: &mut egui::InputState, c: char, key: Key) -> bool {
    if i.consume_key(Modifiers::ALT, key) {
        // Remove any text input events that match the key
        let cs = c.to_string();
        i.events.retain(|e| !matches!(e, egui::Event::Text(t) if *t == cs));
        true
    } else {
        false
    }
}
