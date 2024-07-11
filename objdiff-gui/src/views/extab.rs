use egui::{TextStyle, ScrollArea};

use crate::views::appearance::Appearance;


const EXTAB_DATA: [u8; 344] = [
    0x20, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3C, 0x00, 0x02, 0x00, 0x28, 0x00, 0x00, 0x00, 0x74,
    0x00, 0x00, 0x00, 0x94, 0x00, 0x00, 0x00, 0x98, 0x00, 0x00, 0x00, 0xC4, 0x00, 0x00, 0x00, 0xA4,
    0x00, 0x00, 0x01, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x21, 0x9C,
    0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x21, 0x6C, 0x00, 0x00, 0x00, 0x00,
    0x07, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x20, 0xC8, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1E,
    0x00, 0x00, 0x01, 0xA8, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x00, 0x64,
    0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x00, 0x44, 0x00, 0x00, 0x00, 0x00,
    0x07, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x00, 0x24, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1E,
    0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x86, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1C, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
    0x07, 0x80, 0x00, 0x1D, 0x00, 0x00, 0x00, 0x9C, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1E,
    0x00, 0x00, 0x21, 0x9C, 0x00, 0x00, 0x00, 0x00, 0x87, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x21, 0x6C,
    0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1D, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
    0x07, 0x80, 0x00, 0x1C, 0x00, 0x00, 0x1E, 0xF4, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1C,
    0x00, 0x00, 0x1E, 0xDC, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x21, 0x9C,
    0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x21, 0x6C, 0x00, 0x00, 0x00, 0x00,
    0x87, 0x80, 0x00, 0x1E, 0x00, 0x00, 0x20, 0xC8, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1C,
    0x00, 0x00, 0x1E, 0xF4, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1C, 0x00, 0x00, 0x1E, 0xDC,
    0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1C, 0x00, 0x00, 0x1E, 0xC4, 0x00, 0x00, 0x00, 0x00,
    0x07, 0x80, 0x00, 0x1C, 0x00, 0x00, 0x1E, 0xBC, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1C,
    0x00, 0x00, 0x1E, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x07, 0x80, 0x00, 0x1C, 0x00, 0x00, 0x1D, 0xC8,
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0xE8,
];

const EXTAB_FUNCS: [&str; 25] = [
    "__dt__Q22cf7CVisionFv",
    "__dt__Q22cf12CSuddenCommuFv",
    "__dt__800D8DBC",
    "__dt__800D8B9C",
    "__dt__reslist_cf_IBattleEvent",
    "__dt__reslist_cf_CfObjectActor",
    "__dt__reslist_cf_CfObjectActor",
    "__dt__reslist_cf_CfObjectActor",
    "__dt__800D8884",
    "__dt__Q22cf12CChainEffectFv",
    "__dt__Q22cf11CChainTimerFv",
    "__dt__Q22cf7CVisionFv",
    "__dt__Q22cf12CSuddenCommuFv",
    "__dt__Q22cf12CChainEffectFv",
    "__dt__Q22cf11CChainComboFv",
    "__dt__Q22cf12CChainChanceFv",
    "__dt__Q22cf7CVisionFv",
    "__dt__Q22cf12CSuddenCommuFv",
    "__dt__800D8DBC",
    "__dt__Q22cf11CChainComboFv",
    "__dt__Q22cf12CChainChanceFv",
    "__dt__Q22cf10CChainTimeFv",
    "__dt__Q22cf11CChainTimerFv",
    "__dt__Q22cf11CChainTimerFv",
    "__dt__Q22cf12CChainMemberFv",
];


#[derive(Default)]
pub struct ExtabViewState {
	show_text: bool,
	pub text: String,
}

fn decode_extab(state: &mut ExtabViewState){
	if let Some(extab_data) = cwextab::decode_extab(&EXTAB_DATA) {
		if let Some(decoded) = extab_data.to_string(&EXTAB_FUNCS) {
			state.text = decoded;
		}
	} else {
		state.text = String::from("[invalid]");
	}
	state.show_text = true;
}

pub fn extab_window(
    ctx: &egui::Context,
    show: &mut bool,
    state: &mut ExtabViewState,
    appearance: &Appearance,
) {
    egui::Window::new("Extab Decoder").open(show).show(ctx, |ui| {
		if ui.button("Test").clicked() {
			decode_extab(state);
		}
        
		if state.show_text {
			ScrollArea::vertical()
            	.show(ui, |ui| {
        	    ui.style_mut().override_text_style = Some(TextStyle::Monospace);
        	    ui.colored_label(appearance.replace_color, &state.text);
			});
		}
    });
}
