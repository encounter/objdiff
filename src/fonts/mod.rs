pub mod matching;

use std::{borrow::Cow, fs, sync::Arc};

use anyhow::{Context, Result};

use crate::fonts::matching::find_best_match;

pub struct LoadedFontFamily {
    pub family_name: String,
    pub fonts: Vec<font_kit::font::Font>,
    pub handles: Vec<font_kit::handle::Handle>,
    pub properties: Vec<font_kit::properties::Properties>,
    pub default_index: usize,
}

pub struct LoadedFont {
    pub font_name: String,
    pub font_data: egui::FontData,
}

pub fn load_font_family(
    source: &font_kit::source::SystemSource,
    name: &str,
) -> Option<LoadedFontFamily> {
    let family_handle = source.select_family_by_name(name).ok()?;
    if family_handle.fonts().is_empty() {
        log::warn!("No fonts found for family '{}'", name);
        return None;
    }
    let handles = family_handle.fonts().to_vec();
    let mut loaded = Vec::with_capacity(handles.len());
    for handle in handles.iter() {
        match font_kit::loaders::default::Font::from_handle(handle) {
            Ok(font) => loaded.push(font),
            Err(err) => {
                log::warn!("Failed to load font '{}': {}", name, err);
                return None;
            }
        }
    }
    let properties = loaded.iter().map(|f| f.properties()).collect::<Vec<_>>();
    let default_index =
        find_best_match(&properties, &font_kit::properties::Properties::new()).unwrap_or(0);
    let font_family_name =
        loaded.first().map(|f| f.family_name()).unwrap_or_else(|| name.to_string());
    Some(LoadedFontFamily {
        family_name: font_family_name,
        fonts: loaded,
        handles,
        properties,
        default_index,
    })
}

pub fn load_font(handle: &font_kit::handle::Handle) -> Result<LoadedFont> {
    let loaded = font_kit::loaders::default::Font::from_handle(handle)?;
    let data = match handle {
        font_kit::handle::Handle::Memory { bytes, font_index } => egui::FontData {
            font: Cow::Owned(bytes.to_vec()),
            index: *font_index,
            tweak: Default::default(),
        },
        font_kit::handle::Handle::Path { path, font_index } => {
            let vec = fs::read(path).with_context(|| {
                format!("Failed to load font '{}' (index {})", path.display(), font_index)
            })?;
            egui::FontData { font: Cow::Owned(vec), index: *font_index, tweak: Default::default() }
        }
    };
    Ok(LoadedFont { font_name: loaded.full_name(), font_data: data })
}

pub fn load_font_if_needed(
    ctx: &egui::Context,
    source: &font_kit::source::SystemSource,
    font_id: &egui::FontId,
    base_family: egui::FontFamily,
    fonts: &mut egui::FontDefinitions,
) -> Result<()> {
    if fonts.families.contains_key(&font_id.family) {
        return Ok(());
    }
    let family_name = match &font_id.family {
        egui::FontFamily::Proportional | egui::FontFamily::Monospace => return Ok(()),
        egui::FontFamily::Name(v) => v,
    };
    let family = load_font_family(source, family_name)
        .with_context(|| format!("Failed to load font family '{}'", family_name))?;
    let default_fonts = fonts.families.get(&base_family).cloned().unwrap_or_default();
    // FIXME clean up
    let default_font_ref = family.fonts.get(family.default_index).unwrap();
    let default_font = family.handles.get(family.default_index).unwrap();
    let default_font_data = load_font(default_font).unwrap();
    log::info!("Loaded font family '{}'", family.family_name);
    fonts.font_data.insert(default_font_ref.full_name(), default_font_data.font_data);
    fonts
        .families
        .entry(egui::FontFamily::Name(Arc::from(family.family_name)))
        .or_insert_with(|| default_fonts)
        .insert(0, default_font_ref.full_name());
    ctx.set_fonts(fonts.clone());
    Ok(())
}
