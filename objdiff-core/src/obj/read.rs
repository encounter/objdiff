use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Cursor,
    path::Path,
};

use anyhow::{anyhow, bail, ensure, Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use filetime::FileTime;
use flagset::Flags;
use object::{
    BinaryFormat, File, Object, ObjectSection, ObjectSymbol, RelocationTarget, SectionIndex,
    SectionKind, Symbol, SymbolKind, SymbolScope, SymbolSection,
};

use crate::{
    arch::{new_arch, ObjArch},
    obj::{
        split_meta::{SplitMeta, SPLITMETA_SECTION},
        ObjInfo, ObjReloc, ObjSection, ObjSectionKind, ObjSymbol, ObjSymbolFlagSet, ObjSymbolFlags,
    },
};

fn to_obj_section_kind(kind: SectionKind) -> Option<ObjSectionKind> {
    match kind {
        SectionKind::Text => Some(ObjSectionKind::Code),
        SectionKind::Data | SectionKind::ReadOnlyData => Some(ObjSectionKind::Data),
        SectionKind::UninitializedData => Some(ObjSectionKind::Bss),
        _ => None,
    }
}

fn to_obj_symbol(
    arch: &dyn ObjArch,
    obj_file: &File<'_>,
    symbol: &Symbol<'_, '_>,
    addend: i64,
    split_meta: Option<&SplitMeta>,
) -> Result<ObjSymbol> {
    let mut name = symbol.name().context("Failed to process symbol name")?;
    if name.is_empty() {
        log::warn!("Found empty sym: {symbol:?}");
        name = "?";
    }
    let mut flags = ObjSymbolFlagSet(ObjSymbolFlags::none());
    if symbol.is_global() {
        flags = ObjSymbolFlagSet(flags.0 | ObjSymbolFlags::Global);
    }
    if symbol.is_local() {
        flags = ObjSymbolFlagSet(flags.0 | ObjSymbolFlags::Local);
    }
    if symbol.is_common() {
        flags = ObjSymbolFlagSet(flags.0 | ObjSymbolFlags::Common);
    }
    if symbol.is_weak() {
        flags = ObjSymbolFlagSet(flags.0 | ObjSymbolFlags::Weak);
    }
    if obj_file.format() == BinaryFormat::Elf && symbol.scope() == SymbolScope::Linkage {
        flags = ObjSymbolFlagSet(flags.0 | ObjSymbolFlags::Hidden);
    }
    let section_address = if let Some(section) =
        symbol.section_index().and_then(|idx| obj_file.section_by_index(idx).ok())
    {
        symbol.address() - section.address()
    } else {
        symbol.address()
    };
    let demangled_name = arch.demangle(name);
    // Find the virtual address for the symbol if available
    let virtual_address = split_meta
        .and_then(|m| m.virtual_addresses.as_ref())
        .and_then(|v| v.get(symbol.index().0).cloned());
    Ok(ObjSymbol {
        name: name.to_string(),
        demangled_name,
        address: symbol.address(),
        section_address,
        size: symbol.size(),
        size_known: symbol.size() != 0,
        flags,
        addend,
        virtual_address,
    })
}

fn filter_sections(obj_file: &File<'_>, split_meta: Option<&SplitMeta>) -> Result<Vec<ObjSection>> {
    let mut result = Vec::<ObjSection>::new();
    for section in obj_file.sections() {
        if section.size() == 0 {
            continue;
        }
        let Some(kind) = to_obj_section_kind(section.kind()) else {
            continue;
        };
        let name = section.name().context("Failed to process section name")?;
        let data = section.uncompressed_data().context("Failed to read section data")?;

        // Find the virtual address for the section symbol if available
        let section_symbol = obj_file.symbols().find(|s| {
            s.kind() == SymbolKind::Section && s.section_index() == Some(section.index())
        });
        let virtual_address = section_symbol.and_then(|s| {
            split_meta
                .and_then(|m| m.virtual_addresses.as_ref())
                .and_then(|v| v.get(s.index().0).cloned())
        });

        result.push(ObjSection {
            name: name.to_string(),
            kind,
            address: section.address(),
            size: section.size(),
            data: data.to_vec(),
            orig_index: section.index().0,
            symbols: Vec::new(),
            relocations: Vec::new(),
            virtual_address,
        });
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

fn symbols_by_section(
    arch: &dyn ObjArch,
    obj_file: &File<'_>,
    section: &ObjSection,
    split_meta: Option<&SplitMeta>,
) -> Result<Vec<ObjSymbol>> {
    let mut result = Vec::<ObjSymbol>::new();
    for symbol in obj_file.symbols() {
        if symbol.kind() == SymbolKind::Section {
            continue;
        }
        if let Some(index) = symbol.section().index() {
            if index.0 == section.orig_index {
                if symbol.is_local() && section.kind == ObjSectionKind::Code {
                    // TODO strip local syms in diff?
                    let name = symbol.name().context("Failed to process symbol name")?;
                    if symbol.size() == 0 || name.starts_with("lbl_") {
                        continue;
                    }
                }
                result.push(to_obj_symbol(arch, obj_file, &symbol, 0, split_meta)?);
            }
        }
    }
    result.sort_by_key(|v| v.address);
    let mut iter = result.iter_mut().peekable();
    while let Some(symbol) = iter.next() {
        if symbol.size == 0 {
            if let Some(next_symbol) = iter.peek() {
                symbol.size = next_symbol.address - symbol.address;
            } else {
                symbol.size = (section.address + section.size) - symbol.address;
            }
        }
    }
    Ok(result)
}

fn common_symbols(
    arch: &dyn ObjArch,
    obj_file: &File<'_>,
    split_meta: Option<&SplitMeta>,
) -> Result<Vec<ObjSymbol>> {
    obj_file
        .symbols()
        .filter(Symbol::is_common)
        .map(|symbol| to_obj_symbol(arch, obj_file, &symbol, 0, split_meta))
        .collect::<Result<Vec<ObjSymbol>>>()
}

fn find_section_symbol(
    arch: &dyn ObjArch,
    obj_file: &File<'_>,
    target: &Symbol<'_, '_>,
    address: u64,
    split_meta: Option<&SplitMeta>,
) -> Result<ObjSymbol> {
    let section_index =
        target.section_index().ok_or_else(|| anyhow::Error::msg("Unknown section index"))?;
    let section = obj_file.section_by_index(section_index)?;
    let mut closest_symbol: Option<Symbol<'_, '_>> = None;
    for symbol in obj_file.symbols() {
        if !matches!(symbol.section_index(), Some(idx) if idx == section_index) {
            continue;
        }
        if symbol.kind() == SymbolKind::Section || symbol.address() != address {
            if symbol.address() < address
                && symbol.size() != 0
                && (closest_symbol.is_none()
                    || matches!(&closest_symbol, Some(s) if s.address() <= symbol.address()))
            {
                closest_symbol = Some(symbol);
            }
            continue;
        }
        return to_obj_symbol(arch, obj_file, &symbol, 0, split_meta);
    }
    let (name, offset) = closest_symbol
        .and_then(|s| s.name().map(|n| (n, s.address())).ok())
        .or_else(|| section.name().map(|n| (n, section.address())).ok())
        .unwrap_or(("<unknown>", 0));
    let offset_addr = address - offset;
    Ok(ObjSymbol {
        name: name.to_string(),
        demangled_name: None,
        address: offset,
        section_address: address - section.address(),
        size: 0,
        size_known: false,
        flags: Default::default(),
        addend: offset_addr as i64,
        virtual_address: None,
    })
}

fn relocations_by_section(
    arch: &dyn ObjArch,
    obj_file: &File<'_>,
    section: &ObjSection,
    split_meta: Option<&SplitMeta>,
) -> Result<Vec<ObjReloc>> {
    let obj_section = obj_file.section_by_index(SectionIndex(section.orig_index))?;
    let mut relocations = Vec::<ObjReloc>::new();
    for (address, reloc) in obj_section.relocations() {
        let symbol = match reloc.target() {
            RelocationTarget::Symbol(idx) => {
                if idx.0 == u32::MAX as usize {
                    // ???
                    continue;
                }
                let Ok(symbol) = obj_file.symbol_by_index(idx) else {
                    log::warn!(
                        "Failed to locate relocation {:#x} target symbol {}",
                        address,
                        idx.0
                    );
                    continue;
                };
                symbol
            }
            _ => bail!("Unhandled relocation target: {:?}", reloc.target()),
        };
        let flags = reloc.flags(); // TODO validate reloc here?
        let target_section = match symbol.section() {
            SymbolSection::Common => Some(".comm".to_string()),
            SymbolSection::Section(idx) => {
                obj_file.section_by_index(idx).and_then(|s| s.name().map(|s| s.to_string())).ok()
            }
            _ => None,
        };
        let addend = if reloc.has_implicit_addend() {
            arch.implcit_addend(section, address, &reloc)?
        } else {
            reloc.addend()
        };
        // println!("Reloc: {reloc:?}, symbol: {symbol:?}, addend: {addend:#X}");
        let target = match symbol.kind() {
            SymbolKind::Text | SymbolKind::Data | SymbolKind::Label | SymbolKind::Unknown => {
                to_obj_symbol(arch, obj_file, &symbol, addend, split_meta)
            }
            SymbolKind::Section => {
                ensure!(addend >= 0, "Negative addend in reloc: {addend}");
                find_section_symbol(arch, obj_file, &symbol, addend as u64, split_meta)
            }
            kind => Err(anyhow!("Unhandled relocation symbol type {kind:?}")),
        }?;
        relocations.push(ObjReloc { flags, address, target, target_section });
    }
    Ok(relocations)
}

fn line_info(obj_file: &File<'_>) -> Result<Option<HashMap<SectionIndex, BTreeMap<u64, u64>>>> {
    let mut map = HashMap::new();

    // DWARF 1.1
    if let Some(section) = obj_file.section_by_name(".line") {
        if section.size() == 0 {
            return Ok(None);
        }
        let text_section = obj_file
            .sections()
            .find(|s| s.kind() == SectionKind::Text)
            .context("No text section found for line info")?;
        let mut lines = BTreeMap::new();

        let data = section.uncompressed_data()?;
        let mut reader = Cursor::new(data.as_ref());

        let size = reader.read_u32::<BigEndian>()?;
        let base_address = reader.read_u32::<BigEndian>()? as u64;
        while reader.position() < size as u64 {
            let line_number = reader.read_u32::<BigEndian>()? as u64;
            let statement_pos = reader.read_u16::<BigEndian>()?;
            if statement_pos != 0xFFFF {
                log::warn!("Unhandled statement pos {}", statement_pos);
            }
            let address_delta = reader.read_u32::<BigEndian>()? as u64;
            lines.insert(base_address + address_delta, line_number);
        }

        map.insert(text_section.index(), lines);
    }

    // DWARF 2+
    #[cfg(feature = "dwarf")]
    {
        let mut text_sections = obj_file.sections().filter(|s| s.kind() == SectionKind::Text);
        let first_section = text_sections.next().context("No text section found for line info")?;
        map.insert(first_section.index(), BTreeMap::new());
        let mut lines = map.get_mut(&first_section.index()).unwrap();

        let dwarf_cow = gimli::DwarfSections::load(|id| {
            Ok::<_, gimli::Error>(
                obj_file
                    .section_by_name(id.name())
                    .and_then(|section| section.uncompressed_data().ok())
                    .unwrap_or(std::borrow::Cow::Borrowed(&[][..])),
            )
        })?;
        let endian = match obj_file.endianness() {
            object::Endianness::Little => gimli::RunTimeEndian::Little,
            object::Endianness::Big => gimli::RunTimeEndian::Big,
        };
        let dwarf = dwarf_cow.borrow(|section| gimli::EndianSlice::new(section, endian));
        let mut iter = dwarf.units();
        'outer: while let Some(header) = iter.next()? {
            let unit = dwarf.unit(header)?;
            if let Some(program) = unit.line_program.clone() {
                let mut rows = program.rows();
                while let Some((_header, row)) = rows.next_row()? {
                    if let Some(line) = row.line() {
                        lines.insert(row.address(), line.get());
                    }
                    if row.end_sequence() {
                        // The next row is the start of a new sequence, which means we must
                        // advance to the next .text section.
                        if let Some(next_section) = text_sections.next() {
                            map.insert(next_section.index(), BTreeMap::new());
                            lines = map.get_mut(&next_section.index()).unwrap();
                        } else {
                            break 'outer;
                        }
                    }
                }
            }
        }
    }
    if map.is_empty() {
        return Ok(None);
    }
    Ok(Some(map))
}

pub fn read(obj_path: &Path) -> Result<ObjInfo> {
    let (data, timestamp) = {
        let file = fs::File::open(obj_path)?;
        let timestamp = FileTime::from_last_modification_time(&file.metadata()?);
        (unsafe { memmap2::Mmap::map(&file) }?, timestamp)
    };
    let obj_file = File::parse(&*data)?;
    let arch = new_arch(&obj_file)?;
    let split_meta = split_meta(&obj_file)?;
    let mut sections = filter_sections(&obj_file, split_meta.as_ref())?;
    for section in &mut sections {
        section.symbols =
            symbols_by_section(arch.as_ref(), &obj_file, section, split_meta.as_ref())?;
        section.relocations =
            relocations_by_section(arch.as_ref(), &obj_file, section, split_meta.as_ref())?;
    }
    let common = common_symbols(arch.as_ref(), &obj_file, split_meta.as_ref())?;
    Ok(ObjInfo {
        arch,
        path: obj_path.to_owned(),
        timestamp,
        sections,
        common,
        line_info: line_info(&obj_file)?,
        split_meta,
    })
}

pub fn has_function(obj_path: &Path, symbol_name: &str) -> Result<bool> {
    let data = {
        let file = fs::File::open(obj_path)?;
        unsafe { memmap2::Mmap::map(&file) }?
    };
    Ok(File::parse(&*data)?
        .symbol_by_name(symbol_name)
        .filter(|o| o.kind() == SymbolKind::Text)
        .is_some())
}

fn split_meta(obj_file: &File<'_>) -> Result<Option<SplitMeta>> {
    Ok(if let Some(section) = obj_file.section_by_name(SPLITMETA_SECTION) {
        Some(SplitMeta::from_section(section, obj_file.endianness(), obj_file.is_64())?)
    } else {
        None
    })
}
