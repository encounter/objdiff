use egui::{
    Context, Key, KeyboardShortcut, Modifiers, PointerButton, style::ScrollAnimation, vec2,
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

const GO_TO_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::G);

pub fn consume_go_to_shortcut(ctx: &Context) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&GO_TO_SHORTCUT))
}
