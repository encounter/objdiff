use std::{borrow::Cow, collections::BTreeMap, fs, io::Cursor, path::Path};

use anyhow::{anyhow, bail, ensure, Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use filetime::FileTime;
use flagset::Flags;
use object::{
    elf, Architecture, Endianness, File, Object, ObjectSection, ObjectSymbol, RelocationKind,
    RelocationTarget, SectionIndex, SectionKind, Symbol, SymbolKind, SymbolScope, SymbolSection,
};

use crate::obj::{
    ObjArchitecture, ObjInfo, ObjReloc, ObjRelocKind, ObjSection, ObjSectionKind, ObjSymbol,
    ObjSymbolFlagSet, ObjSymbolFlags,
};

fn to_obj_section_kind(kind: SectionKind) -> Option<ObjSectionKind> {
    match kind {
        SectionKind::Text => Some(ObjSectionKind::Code),
        SectionKind::Data | SectionKind::ReadOnlyData => Some(ObjSectionKind::Data),
        SectionKind::UninitializedData => Some(ObjSectionKind::Bss),
        _ => None,
    }
}

fn to_obj_symbol(obj_file: &File<'_>, symbol: &Symbol<'_, '_>, addend: i64) -> Result<ObjSymbol> {
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
    if symbol.scope() == SymbolScope::Linkage {
        flags = ObjSymbolFlagSet(flags.0 | ObjSymbolFlags::Hidden);
    }
    let section_address = if let Some(section) =
        symbol.section_index().and_then(|idx| obj_file.section_by_index(idx).ok())
    {
        symbol.address() - section.address()
    } else {
        symbol.address()
    };
    let mut demangled_name = None;
    #[cfg(feature = "ppc")]
    if obj_file.architecture() == Architecture::PowerPc {
        demangled_name = cwdemangle::demangle(name, &Default::default());
    }
    Ok(ObjSymbol {
        name: name.to_string(),
        demangled_name,
        address: symbol.address(),
        section_address,
        size: symbol.size(),
        size_known: symbol.size() != 0,
        flags,
        addend,
        diff_symbol: None,
        instructions: vec![],
        match_percent: None,
    })
}

fn filter_sections(obj_file: &File<'_>) -> Result<Vec<ObjSection>> {
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
        result.push(ObjSection {
            name: name.to_string(),
            kind,
            address: section.address(),
            size: section.size(),
            data: data.to_vec(),
            index: section.index().0,
            symbols: Vec::new(),
            relocations: Vec::new(),
            data_diff: vec![],
            match_percent: 0.0,
        });
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

fn symbols_by_section(obj_file: &File<'_>, section: &ObjSection) -> Result<Vec<ObjSymbol>> {
    let mut result = Vec::<ObjSymbol>::new();
    for symbol in obj_file.symbols() {
        if symbol.kind() == SymbolKind::Section {
            continue;
        }
        if let Some(index) = symbol.section().index() {
            if index.0 == section.index {
                if symbol.is_local() && section.kind == ObjSectionKind::Code {
                    // TODO strip local syms in diff?
                    let name = symbol.name().context("Failed to process symbol name")?;
                    if symbol.size() == 0 || name.starts_with("lbl_") {
                        continue;
                    }
                }
                result.push(to_obj_symbol(obj_file, &symbol, 0)?);
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

fn common_symbols(obj_file: &File<'_>) -> Result<Vec<ObjSymbol>> {
    obj_file
        .symbols()
        .filter(Symbol::is_common)
        .map(|symbol| to_obj_symbol(obj_file, &symbol, 0))
        .collect::<Result<Vec<ObjSymbol>>>()
}

fn find_section_symbol(
    obj_file: &File<'_>,
    target: &Symbol<'_, '_>,
    address: u64,
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
        return to_obj_symbol(obj_file, &symbol, 0);
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
        diff_symbol: None,
        instructions: vec![],
        match_percent: None,
    })
}

fn relocations_by_section(
    arch: ObjArchitecture,
    obj_file: &File<'_>,
    section: &ObjSection,
) -> Result<Vec<ObjReloc>> {
    let obj_section = obj_file.section_by_index(SectionIndex(section.index))?;
    let mut relocations = Vec::<ObjReloc>::new();
    for (address, reloc) in obj_section.relocations() {
        let symbol = match reloc.target() {
            RelocationTarget::Symbol(idx) => obj_file
                .symbol_by_index(idx)
                .context("Failed to locate relocation target symbol")?,
            _ => bail!("Unhandled relocation target: {:?}", reloc.target()),
        };
        let kind = match reloc.kind() {
            RelocationKind::Absolute => ObjRelocKind::Absolute,
            RelocationKind::Elf(kind) => match arch {
                #[cfg(feature = "ppc")]
                ObjArchitecture::PowerPc => match kind {
                    elf::R_PPC_ADDR16_LO => ObjRelocKind::PpcAddr16Lo,
                    elf::R_PPC_ADDR16_HI => ObjRelocKind::PpcAddr16Hi,
                    elf::R_PPC_ADDR16_HA => ObjRelocKind::PpcAddr16Ha,
                    elf::R_PPC_REL24 => ObjRelocKind::PpcRel24,
                    elf::R_PPC_REL14 => ObjRelocKind::PpcRel14,
                    elf::R_PPC_EMB_SDA21 => ObjRelocKind::PpcEmbSda21,
                    _ => bail!("Unhandled PPC relocation type: {kind}"),
                },
                #[cfg(feature = "mips")]
                ObjArchitecture::Mips => match kind {
                    elf::R_MIPS_26 => ObjRelocKind::Mips26,
                    elf::R_MIPS_HI16 => ObjRelocKind::MipsHi16,
                    elf::R_MIPS_LO16 => ObjRelocKind::MipsLo16,
                    elf::R_MIPS_GOT16 => ObjRelocKind::MipsGot16,
                    elf::R_MIPS_CALL16 => ObjRelocKind::MipsCall16,
                    elf::R_MIPS_GPREL16 => ObjRelocKind::MipsGpRel16,
                    elf::R_MIPS_GPREL32 => ObjRelocKind::MipsGpRel32,
                    _ => bail!("Unhandled MIPS relocation type: {kind}"),
                },
            },
            _ => bail!("Unhandled relocation type: {:?}", reloc.kind()),
        };
        let target_section = match symbol.section() {
            SymbolSection::Common => Some(".comm".to_string()),
            SymbolSection::Section(idx) => {
                obj_file.section_by_index(idx).and_then(|s| s.name().map(|s| s.to_string())).ok()
            }
            _ => None,
        };
        let addend = if reloc.has_implicit_addend() {
            let addend = u32::from_be_bytes(
                section.data[address as usize..address as usize + 4].try_into()?,
            );
            match kind {
                ObjRelocKind::Absolute => addend as i64,
                #[cfg(feature = "mips")]
                ObjRelocKind::MipsHi16 => ((addend & 0x0000FFFF) << 16) as i32 as i64,
                #[cfg(feature = "mips")]
                ObjRelocKind::MipsLo16
                | ObjRelocKind::MipsGot16
                | ObjRelocKind::MipsCall16
                | ObjRelocKind::MipsGpRel16 => (addend & 0x0000FFFF) as i16 as i64,
                #[cfg(feature = "mips")]
                ObjRelocKind::MipsGpRel32 => addend as i32 as i64,
                #[cfg(feature = "mips")]
                ObjRelocKind::Mips26 => ((addend & 0x03FFFFFF) << 2) as i64,
                _ => bail!("Unsupported implicit relocation {kind:?}"),
            }
        } else {
            reloc.addend()
        };
        // println!("Reloc: {reloc:?}, symbol: {symbol:?}, addend: {addend:#X}");
        let target = match symbol.kind() {
            SymbolKind::Text | SymbolKind::Data | SymbolKind::Label | SymbolKind::Unknown => {
                to_obj_symbol(obj_file, &symbol, addend)
            }
            SymbolKind::Section => {
                ensure!(addend >= 0, "Negative addend in reloc: {addend}");
                find_section_symbol(obj_file, &symbol, addend as u64)
            }
            kind => Err(anyhow!("Unhandled relocation symbol type {kind:?}")),
        }?;
        relocations.push(ObjReloc { kind, address, target, target_section });
    }
    Ok(relocations)
}

fn line_info(obj_file: &File<'_>) -> Result<Option<BTreeMap<u64, u64>>> {
    // DWARF 1.1
    let mut map = BTreeMap::new();
    if let Some(section) = obj_file.section_by_name(".line") {
        if section.size() == 0 {
            return Ok(None);
        }
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
            map.insert(base_address + address_delta, line_number);
        }
    }

    // DWARF 2+
    #[cfg(feature = "dwarf")]
    {
        let dwarf_cow = gimli::Dwarf::load(|id| {
            Ok::<_, gimli::Error>(
                obj_file
                    .section_by_name(id.name())
                    .and_then(|section| section.uncompressed_data().ok())
                    .unwrap_or(Cow::Borrowed(&[][..])),
            )
        })?;
        let endian = match obj_file.endianness() {
            Endianness::Little => gimli::RunTimeEndian::Little,
            Endianness::Big => gimli::RunTimeEndian::Big,
        };
        let dwarf = dwarf_cow.borrow(|section| gimli::EndianSlice::new(section, endian));
        let mut iter = dwarf.units();
        while let Some(header) = iter.next()? {
            let unit = dwarf.unit(header)?;
            if let Some(program) = unit.line_program.clone() {
                let mut rows = program.rows();
                while let Some((_header, row)) = rows.next_row()? {
                    if let Some(line) = row.line() {
                        map.insert(row.address(), line.get());
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
    let architecture = match obj_file.architecture() {
        #[cfg(feature = "ppc")]
        Architecture::PowerPc => ObjArchitecture::PowerPc,
        #[cfg(feature = "mips")]
        Architecture::Mips => ObjArchitecture::Mips,
        _ => bail!("Unsupported architecture: {:?}", obj_file.architecture()),
    };
    let mut result = ObjInfo {
        architecture,
        path: obj_path.to_owned(),
        timestamp,
        sections: filter_sections(&obj_file)?,
        common: common_symbols(&obj_file)?,
        line_info: line_info(&obj_file)?,
    };
    for section in &mut result.sections {
        section.symbols = symbols_by_section(&obj_file, section)?;
        section.relocations = relocations_by_section(architecture, &obj_file, section)?;
    }
    Ok(result)
}
