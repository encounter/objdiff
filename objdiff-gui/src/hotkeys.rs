use egui::{style::ScrollAnimation, vec2, Context, Key, Modifiers, PointerButton};

pub fn enter_pressed(ctx: &Context) -> bool {
    ctx.input_mut(|i| i.key_pressed(Key::Enter) || i.pointer.button_pressed(PointerButton::Extra2))
}

pub fn back_pressed(ctx: &Context) -> bool {
    ctx.input_mut(|i| {
        i.key_pressed(Key::Backspace) || i.pointer.button_pressed(PointerButton::Extra1)
    })
}

pub fn up_pressed(ctx: &Context) -> bool {
    ctx.input_mut(|i| i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::W))
}

pub fn down_pressed(ctx: &Context) -> bool {
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
    ctx.input_mut(|i| {
        i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::NONE, Key::W)
    })
}

pub fn consume_down_key(ctx: &Context) -> bool {
    ctx.input_mut(|i| {
        i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::NONE, Key::S)
    })
}
