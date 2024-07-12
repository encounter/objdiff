use egui::{TextStyle, ScrollArea};

use objdiff_core::obj::ObjExtab;

use crate::views::appearance::Appearance;


#[derive(Default)]
pub struct ExtabViewState {
	pub text: String,
	pub extab_data: Option<ObjExtab>,
	pub queue_decode: bool,
}

fn decode_extab(state: &mut ExtabViewState){
	state.text = String::from("");

	if let Some(extab_data) = &state.extab_data {
		let func_name =
		match &extab_data.func.demangled_name {
			Some(demangled_name) => demangled_name,
			None => &extab_data.func.name
		};
		state.text += format!("Function: {func_name}\n\n").as_str();

		let mut dtor_names: Vec<&str> = vec![];
		for dtor in &extab_data.dtors {
			//For each function name, use the demangled name by default,
			//and if not available fallback to the original name
			let name =
			match &dtor.demangled_name {
				Some(demangled_name) => demangled_name,
				None => &dtor.name
			};
			dtor_names.push(name.as_str());
		}
		if let Some(decoded) = extab_data.data.to_string(&dtor_names) {
			state.text += decoded.as_str();
		}
	} else {
		state.text = String::from("Error: extab data is None");
	}
	state.queue_decode = false;
}

pub fn extab_window(
    ctx: &egui::Context,
    show: &mut bool,
    state: &mut ExtabViewState,
    appearance: &Appearance,
) {
    egui::Window::new("Extab Decoder").open(show).show(ctx, |ui| {
		if state.queue_decode {
			decode_extab(state);
		}
        
		ScrollArea::vertical()
            .show(ui, |ui| {
        	ui.style_mut().override_text_style = Some(TextStyle::Monospace);
        	ui.colored_label(appearance.replace_color, &state.text);
		});
    });
}
