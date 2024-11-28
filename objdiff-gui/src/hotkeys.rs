use egui::{Context, Key, PointerButton};

pub fn enter_pressed(ctx: &Context) -> bool {
    ctx.input_mut(|i| i.key_pressed(Key::Enter) || i.pointer.button_pressed(PointerButton::Extra2))
}

pub fn back_pressed(ctx: &Context) -> bool {
    ctx.input_mut(|i| {
        i.key_pressed(Key::Backspace) || i.pointer.button_pressed(PointerButton::Extra1)
    })
}
