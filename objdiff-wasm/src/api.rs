use alloc::{
    format,
    rc::{Rc, Weak},
    str::FromStr,
    string::{String, ToString},
    vec::Vec,
};
use core::cell::RefCell;

use objdiff_core::{diff, obj};
use regex::{Regex, RegexBuilder};
use xxhash_rust::xxh3::xxh3_64;

use super::logging;

wit_bindgen::generate!({
    world: "api",
    with: {
        "wasi:logging/logging@0.1.0-draft": logging::wasi_logging,
    },
});

use exports::objdiff::core::{
    diff::{
        DiffConfigBorrow, DiffResult, Guest as GuestDiff, GuestDiffConfig, GuestObject,
        GuestObjectDiff, Object, ObjectBorrow, ObjectDiff, ObjectDiffBorrow,
    },
    display::{
        ContextItem, ContextItemCopy, ContextItemNavigate, DiffText, DiffTextColor, DiffTextOpcode,
        DiffTextSegment, DiffTextSymbol, DisplayConfig, Guest as GuestDisplay, HoverItem,
        HoverItemColor, HoverItemText, InstructionDiffKind, InstructionDiffRow, SectionDisplay,
        SectionDisplaySymbol, SymbolDisplay, SymbolFilter, SymbolFlags, SymbolKind,
        SymbolNavigationKind, SymbolRef,
    },
};

struct Component;

impl Guest for Component {
    fn init(level: logging::wasi_logging::Level) { logging::init(level); }

    fn version() -> String { env!("CARGO_PKG_VERSION").to_string() }
}

struct ResourceObject(Rc<obj::Object>, u64);

struct ResourceObjectDiff(Rc<obj::Object>, diff::ObjectDiff);

#[repr(transparent)]
struct ResourceDiffConfig(RefCell<diff::DiffObjConfig>);

impl GuestDiff for Component {
    type DiffConfig = ResourceDiffConfig;
    type Object = ResourceObject;
    type ObjectDiff = ResourceObjectDiff;

    fn run_diff(
        left: Option<ObjectBorrow>,
        right: Option<ObjectBorrow>,
        diff_config: DiffConfigBorrow,
    ) -> Result<DiffResult, String> {
        let diff_config = diff_config.get::<ResourceDiffConfig>().0.borrow();
        log::debug!("Running diff with config: {:?}", diff_config);
        let result = diff::diff_objs(
            left.as_ref().map(|o| o.get::<ResourceObject>().0.as_ref()),
            right.as_ref().map(|o| o.get::<ResourceObject>().0.as_ref()),
            None,
            &diff_config,
            &diff::MappingConfig::default(),
        )
        .map_err(|e| e.to_string())?;
        Ok(DiffResult {
            left: result.left.map(|d| {
                ObjectDiff::new(ResourceObjectDiff(
                    left.unwrap().get::<ResourceObject>().0.clone(),
                    d,
                ))
            }),
            right: result.right.map(|d| {
                ObjectDiff::new(ResourceObjectDiff(
                    right.unwrap().get::<ResourceObject>().0.clone(),
                    d,
                ))
            }),
        })
    }
}

fn build_regex(s: &str) -> Option<Regex> {
    if s.is_empty() {
        return None;
    }
    let e = match RegexBuilder::new(s).case_insensitive(true).build() {
        Ok(regex) => return Some(regex),
        Err(e) => e,
    };
    // Use the string as a literal if the regex fails to compile
    let escaped = regex::escape(s);
    if let Ok(regex) = RegexBuilder::new(&escaped).case_insensitive(true).build() {
        return Some(regex);
    }
    // Use the original error if the escaped string also fails
    log::warn!("Failed to compile regex: {}", e);
    None
}

impl GuestDisplay for Component {
    fn display_sections(
        diff: ObjectDiffBorrow,
        filter: SymbolFilter,
        config: DisplayConfig,
    ) -> Vec<SectionDisplay> {
        let regex = filter.regex.as_deref().and_then(build_regex);
        let filter = if let Some(mapping) = filter.mapping {
            diff::display::SymbolFilter::Mapping(mapping as usize, regex.as_ref())
        } else if let Some(regex) = &regex {
            diff::display::SymbolFilter::Search(regex)
        } else {
            diff::display::SymbolFilter::None
        };
        let obj_diff = diff.get::<ResourceObjectDiff>();
        diff::display::display_sections(
            obj_diff.0.as_ref(),
            &obj_diff.1,
            filter,
            config.show_hidden_symbols,
            config.show_mapped_symbols,
            config.reverse_fn_order,
        )
        .into_iter()
        .map(|d| SectionDisplay {
            id: d.id,
            name: d.name,
            size: d.size,
            match_percent: d.match_percent,
            symbols: d
                .symbols
                .into_iter()
                .map(|s| SectionDisplaySymbol {
                    symbol: s.symbol as SymbolRef,
                    is_mapping_symbol: s.is_mapping_symbol,
                })
                .collect(),
        })
        .collect()
    }

    fn display_symbol(
        diff: ObjectDiffBorrow,
        symbol_display: SectionDisplaySymbol,
    ) -> SymbolDisplay {
        let obj_diff = diff.get::<ResourceObjectDiff>();
        let obj = obj_diff.0.as_ref();
        let obj_diff = &obj_diff.1;
        let symbol_idx = symbol_display.symbol as usize;
        let symbol = &obj.symbols[symbol_idx];
        let symbol_diff = if symbol_display.is_mapping_symbol {
            obj_diff
                .mapping_symbols
                .iter()
                .find(|s| s.symbol_index == symbol_idx)
                .map(|s| &s.symbol_diff)
                .unwrap()
        } else {
            &obj_diff.symbols[symbol_idx]
        };
        SymbolDisplay {
            name: symbol.name.clone(),
            demangled_name: symbol.demangled_name.clone(),
            address: symbol.address,
            size: symbol.size,
            kind: SymbolKind::from(symbol.kind),
            section: symbol.section.map(|s| s as u32),
            flags: SymbolFlags::from(symbol.flags),
            align: symbol.align.map(|a| a.get()),
            virtual_address: symbol.virtual_address,
            target_symbol: symbol_diff.target_symbol.map(|s| s as u32),
            match_percent: symbol_diff.match_percent,
            diff_score: symbol_diff.diff_score,
            row_count: symbol_diff.instruction_rows.len() as u32,
        }
    }

    fn display_instruction_row(
        diff: ObjectDiffBorrow,
        symbol_display: SectionDisplaySymbol,
        row_index: u32,
        diff_config: DiffConfigBorrow,
    ) -> InstructionDiffRow {
        let mut segments = Vec::with_capacity(16);
        let obj_diff = diff.get::<ResourceObjectDiff>();
        let obj = obj_diff.0.as_ref();
        let obj_diff = &obj_diff.1;
        let symbol_idx = symbol_display.symbol as usize;
        let symbol_diff = if symbol_display.is_mapping_symbol {
            obj_diff
                .mapping_symbols
                .iter()
                .find(|s| s.symbol_index == symbol_idx)
                .map(|s| &s.symbol_diff)
                .unwrap()
        } else {
            &obj_diff.symbols[symbol_idx]
        };
        let row = &symbol_diff.instruction_rows[row_index as usize];
        let diff_config = diff_config.get::<ResourceDiffConfig>().0.borrow();
        diff::display::display_row(obj, symbol_idx, row, &diff_config, |segment| {
            segments.push(DiffTextSegment::from(segment));
            Ok(())
        })
        .unwrap();
        InstructionDiffRow { segments, diff_kind: InstructionDiffKind::from(row.kind) }
    }

    fn symbol_context(
        diff: ObjectDiffBorrow,
        symbol_display: SectionDisplaySymbol,
    ) -> Vec<ContextItem> {
        let obj_diff = diff.get::<ResourceObjectDiff>();
        let obj = obj_diff.0.as_ref();
        diff::display::symbol_context(obj, symbol_display.symbol as usize)
            .into_iter()
            .map(|item| ContextItem::from(item))
            .collect()
    }

    fn symbol_hover(
        diff: ObjectDiffBorrow,
        symbol_display: SectionDisplaySymbol,
    ) -> Vec<HoverItem> {
        let obj_diff = diff.get::<ResourceObjectDiff>();
        let obj = obj_diff.0.as_ref();
        diff::display::symbol_hover(
            obj,
            symbol_display.symbol as usize,
            0,
            diff::display::HoverItemColor::Normal, // TODO: colorize replaced/deleted/inserted relocations
        )
        .into_iter()
        .map(|item| HoverItem::from(item))
        .collect()
    }

    fn instruction_context(
        diff: ObjectDiffBorrow,
        symbol_display: SectionDisplaySymbol,
        row_index: u32,
        diff_config: DiffConfigBorrow,
    ) -> Result<Vec<ContextItem>, String> {
        let obj_diff = diff.get::<ResourceObjectDiff>();
        let obj = obj_diff.0.as_ref();
        let obj_diff = &obj_diff.1;
        let symbol_idx = symbol_display.symbol as usize;
        let symbol_diff = if symbol_display.is_mapping_symbol {
            obj_diff
                .mapping_symbols
                .iter()
                .find(|s| s.symbol_index == symbol_idx)
                .map(|s| &s.symbol_diff)
                .unwrap()
        } else {
            &obj_diff.symbols[symbol_idx]
        };
        let row = &symbol_diff.instruction_rows[row_index as usize];
        let Some(ins_ref) = row.ins_ref else {
            return Ok(Vec::new());
        };
        let diff_config = diff_config.get::<ResourceDiffConfig>().0.borrow();
        let Some(resolved) = obj.resolve_instruction_ref(symbol_idx, ins_ref) else {
            return Err("Failed to resolve instruction".into());
        };
        let ins =
            obj.arch.process_instruction(resolved, &diff_config).map_err(|e| e.to_string())?;
        Ok(diff::display::instruction_context(obj, resolved, &ins)
            .into_iter()
            .map(|item| ContextItem::from(item))
            .collect())
    }

    fn instruction_hover(
        diff: ObjectDiffBorrow,
        symbol_display: SectionDisplaySymbol,
        row_index: u32,
        diff_config: DiffConfigBorrow,
    ) -> Result<Vec<HoverItem>, String> {
        let obj_diff = diff.get::<ResourceObjectDiff>();
        let obj = obj_diff.0.as_ref();
        let obj_diff = &obj_diff.1;
        let symbol_idx = symbol_display.symbol as usize;
        let symbol_diff = if symbol_display.is_mapping_symbol {
            obj_diff
                .mapping_symbols
                .iter()
                .find(|s| s.symbol_index == symbol_idx)
                .map(|s| &s.symbol_diff)
                .unwrap()
        } else {
            &obj_diff.symbols[symbol_idx]
        };
        let row = &symbol_diff.instruction_rows[row_index as usize];
        let Some(ins_ref) = row.ins_ref else {
            return Ok(Vec::new());
        };
        let diff_config = diff_config.get::<ResourceDiffConfig>().0.borrow();
        let Some(resolved) = obj.resolve_instruction_ref(symbol_idx, ins_ref) else {
            return Err("Failed to resolve instruction".into());
        };
        let ins =
            obj.arch.process_instruction(resolved, &diff_config).map_err(|e| e.to_string())?;
        Ok(diff::display::instruction_hover(obj, resolved, &ins)
            .into_iter()
            .map(|item| HoverItem::from(item))
            .collect())
    }
}

impl From<obj::SymbolKind> for SymbolKind {
    fn from(kind: obj::SymbolKind) -> Self {
        match kind {
            obj::SymbolKind::Unknown => SymbolKind::Unknown,
            obj::SymbolKind::Function => SymbolKind::Function,
            obj::SymbolKind::Object => SymbolKind::Object,
            obj::SymbolKind::Section => SymbolKind::Section,
        }
    }
}

impl From<obj::SymbolFlagSet> for SymbolFlags {
    fn from(flags: obj::SymbolFlagSet) -> SymbolFlags {
        let mut out = SymbolFlags::empty();
        for flag in flags {
            out |= match flag {
                obj::SymbolFlag::Global => SymbolFlags::GLOBAL,
                obj::SymbolFlag::Local => SymbolFlags::LOCAL,
                obj::SymbolFlag::Weak => SymbolFlags::WEAK,
                obj::SymbolFlag::Common => SymbolFlags::COMMON,
                obj::SymbolFlag::Hidden => SymbolFlags::HIDDEN,
                obj::SymbolFlag::HasExtra => SymbolFlags::HAS_EXTRA,
                obj::SymbolFlag::SizeInferred => SymbolFlags::SIZE_INFERRED,
                obj::SymbolFlag::Ignored => SymbolFlags::IGNORED,
            };
        }
        out
    }
}

impl From<diff::display::DiffText<'_>> for DiffText {
    fn from(text: diff::display::DiffText) -> Self {
        match text {
            diff::display::DiffText::Basic(v) => DiffText::Basic(v.to_string()),
            diff::display::DiffText::Line(v) => DiffText::Line(v),
            diff::display::DiffText::Address(v) => DiffText::Address(v),
            diff::display::DiffText::Opcode(n, op) => {
                DiffText::Opcode(DiffTextOpcode { mnemonic: n.to_string(), opcode: op })
            }
            diff::display::DiffText::Argument(s) => match s {
                obj::InstructionArgValue::Signed(v) => DiffText::Signed(v),
                obj::InstructionArgValue::Unsigned(v) => DiffText::Unsigned(v),
                obj::InstructionArgValue::Opaque(v) => DiffText::Opaque(v.into_owned()),
            },
            diff::display::DiffText::BranchDest(v) => DiffText::BranchDest(v),
            diff::display::DiffText::Symbol(s) => DiffText::Symbol(DiffTextSymbol {
                name: s.name.clone(),
                demangled_name: s.demangled_name.clone(),
            }),
            diff::display::DiffText::Addend(v) => DiffText::Addend(v),
            diff::display::DiffText::Spacing(v) => DiffText::Spacing(v),
            diff::display::DiffText::Eol => DiffText::Eol,
        }
    }
}

impl From<diff::display::DiffTextColor> for DiffTextColor {
    fn from(value: diff::display::DiffTextColor) -> Self {
        match value {
            diff::display::DiffTextColor::Normal => DiffTextColor::Normal,
            diff::display::DiffTextColor::Dim => DiffTextColor::Dim,
            diff::display::DiffTextColor::Bright => DiffTextColor::Bright,
            diff::display::DiffTextColor::Replace => DiffTextColor::Replace,
            diff::display::DiffTextColor::Delete => DiffTextColor::Delete,
            diff::display::DiffTextColor::Insert => DiffTextColor::Insert,
            diff::display::DiffTextColor::Rotating(v) => DiffTextColor::Rotating(v),
        }
    }
}

impl From<diff::display::DiffTextSegment<'_>> for DiffTextSegment {
    fn from(segment: diff::display::DiffTextSegment) -> Self {
        DiffTextSegment {
            text: DiffText::from(segment.text),
            color: DiffTextColor::from(segment.color),
            pad_to: segment.pad_to,
        }
    }
}

impl From<diff::InstructionDiffKind> for InstructionDiffKind {
    fn from(kind: diff::InstructionDiffKind) -> Self {
        match kind {
            diff::InstructionDiffKind::None => InstructionDiffKind::None,
            diff::InstructionDiffKind::OpMismatch => InstructionDiffKind::OpMismatch,
            diff::InstructionDiffKind::ArgMismatch => InstructionDiffKind::ArgMismatch,
            diff::InstructionDiffKind::Replace => InstructionDiffKind::Replace,
            diff::InstructionDiffKind::Insert => InstructionDiffKind::Insert,
            diff::InstructionDiffKind::Delete => InstructionDiffKind::Delete,
        }
    }
}

impl GuestDiffConfig for ResourceDiffConfig {
    fn new() -> Self { Self(RefCell::new(diff::DiffObjConfig::default())) }

    fn set_property(&self, key: String, value: String) -> Result<(), String> {
        let id = diff::ConfigPropertyId::from_str(&key)
            .map_err(|_| format!("Invalid property key {:?}", key))?;
        self.0
            .borrow_mut()
            .set_property_value_str(id, &value)
            .map_err(|_| format!("Invalid property value {:?}", value))
    }

    fn get_property(&self, key: String) -> Result<String, String> {
        let id = diff::ConfigPropertyId::from_str(&key)
            .map_err(|_| format!("Invalid property key {:?}", key))?;
        Ok(self.0.borrow().get_property_value(id).to_string())
    }
}

struct CachedObject(Weak<obj::Object>, u64);

struct ObjectCache(RefCell<Vec<CachedObject>>);

impl ObjectCache {
    #[inline]
    const fn new() -> Self { Self(RefCell::new(Vec::new())) }
}

impl core::ops::Deref for ObjectCache {
    type Target = RefCell<Vec<CachedObject>>;

    fn deref(&self) -> &Self::Target { &self.0 }
}

// Assume single-threaded environment
unsafe impl Sync for ObjectCache {}

static OBJECT_CACHE: ObjectCache = ObjectCache::new();

impl GuestObject for ResourceObject {
    fn parse(data: Vec<u8>, diff_config: DiffConfigBorrow) -> Result<Object, String> {
        let hash = xxh3_64(&data);
        let mut cached = None;
        OBJECT_CACHE.borrow_mut().retain(|c| {
            if c.0.strong_count() == 0 {
                return false;
            }
            if c.1 == hash {
                cached = c.0.upgrade();
            }
            true
        });
        if let Some(obj) = cached {
            return Ok(Object::new(ResourceObject(obj, hash)));
        }
        let diff_config = diff_config.get::<ResourceDiffConfig>().0.borrow();
        let obj = Rc::new(obj::read::parse(&data, &diff_config).map_err(|e| e.to_string())?);
        OBJECT_CACHE.borrow_mut().push(CachedObject(Rc::downgrade(&obj), hash));
        Ok(Object::new(ResourceObject(obj, hash)))
    }

    fn hash(&self) -> u64 { self.1 }
}

impl GuestObjectDiff for ResourceObjectDiff {
    fn find_symbol(&self, name: String, section_name: Option<String>) -> Option<SymbolRef> {
        let obj = self.0.as_ref();
        obj.symbols
            .iter()
            .position(|s| {
                s.name == name
                    && match section_name.as_deref() {
                        Some(section_name) => {
                            s.section.is_some_and(|n| obj.sections[n].name == section_name)
                        }
                        None => true,
                    }
            })
            .map(|i| i as SymbolRef)
    }
}

impl From<diff::display::HoverItem> for HoverItem {
    fn from(item: diff::display::HoverItem) -> Self {
        match item {
            diff::display::HoverItem::Text { label, value, color } => {
                HoverItem::Text(HoverItemText { label, value, color: HoverItemColor::from(color) })
            }
            diff::display::HoverItem::Separator => HoverItem::Separator,
        }
    }
}

impl From<diff::display::HoverItemColor> for HoverItemColor {
    fn from(color: diff::display::HoverItemColor) -> Self {
        match color {
            diff::display::HoverItemColor::Normal => HoverItemColor::Normal,
            diff::display::HoverItemColor::Emphasized => HoverItemColor::Emphasized,
            diff::display::HoverItemColor::Special => HoverItemColor::Special,
            diff::display::HoverItemColor::Delete => HoverItemColor::Delete,
            diff::display::HoverItemColor::Insert => HoverItemColor::Insert,
        }
    }
}

impl From<diff::display::ContextItem> for ContextItem {
    fn from(item: diff::display::ContextItem) -> Self {
        match item {
            diff::display::ContextItem::Copy { value, label } => {
                ContextItem::Copy(ContextItemCopy { value, label })
            }
            diff::display::ContextItem::Navigate { label, symbol_index, kind } => {
                ContextItem::Navigate(ContextItemNavigate {
                    label,
                    symbol: symbol_index as SymbolRef,
                    kind: SymbolNavigationKind::from(kind),
                })
            }
            diff::display::ContextItem::Separator => ContextItem::Separator,
        }
    }
}

impl From<diff::display::SymbolNavigationKind> for SymbolNavigationKind {
    fn from(kind: diff::display::SymbolNavigationKind) -> Self {
        match kind {
            diff::display::SymbolNavigationKind::Normal => SymbolNavigationKind::Normal,
            diff::display::SymbolNavigationKind::Extab => SymbolNavigationKind::Extab,
        }
    }
}

export!(Component);
