use std::{collections::HashSet, fs, io::Cursor, path::Path};

use anyhow::{anyhow, bail, ensure, Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use filetime::FileTime;
use flagset::Flags;
use object::{
    Architecture, BinaryFormat, File, Object, ObjectSection, ObjectSymbol, RelocationTarget, SectionIndex, SectionKind, Symbol, SymbolKind, SymbolScope, SymbolSection
};
use cwextab::decode_extab;

use crate::{
    arch::{new_arch, ObjArch},
    diff::DiffObjConfig,
    obj::{
        split_meta::{SplitMeta, SPLITMETA_SECTION},
        ObjInfo, ObjReloc, ObjSection, ObjSectionKind, ObjSymbol, ObjSymbolFlagSet, ObjSymbolFlags,
		ObjExtab,
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
    let address = arch.symbol_address(symbol);
    let section_address = if let Some(section) =
        symbol.section_index().and_then(|idx| obj_file.section_by_index(idx).ok())
    {
        address - section.address()
    } else {
        address
    };
    let demangled_name = arch.demangle(name);
    // Find the virtual address for the symbol if available
    let virtual_address = split_meta
        .and_then(|m| m.virtual_addresses.as_ref())
        .and_then(|v| v.get(symbol.index().0).cloned());
    Ok(ObjSymbol {
        name: name.to_string(),
        demangled_name,
		has_extab: false,
        address,
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
            line_info: Default::default(),
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

fn section_by_name<'a>(sections: &'a mut [ObjSection], name : &str) -> Option<&'a mut ObjSection> {
	for section in sections {
		if section.name == name {
			return Some(section);
		}
	}
	None
}

fn exception_tables(
	_arch: &dyn ObjArch,
    sections: &mut [ObjSection],
    obj_file: &File<'_>,
    _split_meta: Option<&SplitMeta>,
) -> Option<Vec<ObjExtab>> {

	//PowerPC only
	if obj_file.architecture() != Architecture::PowerPc {
		return None;
	}

	//Find the extab/extabindex sections
	let extab_section = section_by_name(sections, "extab")?.clone();
	let extabindex_section = section_by_name(sections, "extabindex")?.clone();
	let text_section = section_by_name(sections, ".text")?;

	//Convert the extab/extabindex section data
	let mut result: Vec<ObjExtab> = vec![];
	let extab_symbol_count = extab_section.symbols.len();
	let extab_reloc_count = extab_section.relocations.len();
	let table_count = extab_symbol_count;
	let mut extab_reloc_index : usize = 0;

	//Go through each pair
	for i in 0..table_count {
		let extab = &extab_section.symbols[i];
		let extab_start_addr = extab.address;
		let extab_end_addr = extab_start_addr + extab.size;

		/* Get the function symbol from the extabindex relocations array. Each extabindex
		entry has two relocations (the first for the function, the second for the extab entry),
		so get the first of each. */
		let extab_func = extabindex_section.relocations[i*2].target.clone();

		//Find the function in the text section, and set the has extab flag
		for i in 0..text_section.symbols.len() {
			let func = &mut text_section.symbols[i];
			if func.name == extab_func.name {
				func.has_extab = true;
			}
		}

		/* Iterate through the list of extab relocations, continuing until we hit a relocation
		that isn't within the current extab symbol. Get the target dtor function symbol from
		each relocation used, and add them to the list. */
		let mut dtors : Vec<ObjSymbol> = vec![];
		
		while extab_reloc_index < extab_reloc_count {
			let extab_reloc = &extab_section.relocations[extab_reloc_index];
			//If the current entry is past the current extab table, stop here
			if extab_reloc.address >= extab_end_addr {
				break;
			}
			
			//Otherwise, the current relocation is used by the current table
			dtors.push(extab_reloc.target.clone());
			//Go to the next entry
			extab_reloc_index += 1;
		}

		//Decode the extab data
		let start_index = extab_start_addr as usize;
		let end_index = extab_end_addr as usize;
		let extab_data = extab_section.data[start_index..end_index].try_into().unwrap();
		let data = decode_extab(extab_data)?;

		//Add the new entry to the list
		let entry = ObjExtab {func: extab_func, data, dtors};
		result.push(entry);
	}

	Some(result)
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
		has_extab: false,
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
        // println!("Reloc: {reloc:?}, symbol: {symbol:?}, addend: {addend:#x}");
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

fn line_info(obj_file: &File<'_>, sections: &mut [ObjSection]) -> Result<()> {
    // DWARF 1.1
    if let Some(section) = obj_file.section_by_name(".line") {
        let data = section.uncompressed_data()?;
        let mut reader = Cursor::new(data.as_ref());

        let mut text_sections = obj_file.sections().filter(|s| s.kind() == SectionKind::Text);
        while reader.position() < data.len() as u64 {
            let text_section_index = text_sections
                .next()
                .ok_or_else(|| anyhow!("Next text section not found for line info"))?
                .index()
                .0;
            let start = reader.position();
            let size = reader.read_u32::<BigEndian>()?;
            let base_address = reader.read_u32::<BigEndian>()? as u64;
            let Some(out_section) =
                sections.iter_mut().find(|s| s.orig_index == text_section_index)
            else {
                // Skip line info for sections we filtered out
                reader.set_position(start + size as u64);
                continue;
            };
            let end = start + size as u64;
            while reader.position() < end {
                let line_number = reader.read_u32::<BigEndian>()? as u64;
                let statement_pos = reader.read_u16::<BigEndian>()?;
                if statement_pos != 0xFFFF {
                    log::warn!("Unhandled statement pos {}", statement_pos);
                }
                let address_delta = reader.read_u32::<BigEndian>()? as u64;
                out_section.line_info.insert(base_address + address_delta, line_number);
                log::debug!("Line: {:#x} -> {}", base_address + address_delta, line_number);
            }
        }
    }

    // DWARF 2+
    #[cfg(feature = "dwarf")]
    {
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
        if let Some(header) = iter.next()? {
            let unit = dwarf.unit(header)?;
            if let Some(program) = unit.line_program.clone() {
                let mut text_sections =
                    obj_file.sections().filter(|s| s.kind() == SectionKind::Text);
                let section_index = text_sections.next().map(|s| s.index().0);
                let mut lines = section_index.map(|index| {
                    &mut sections.iter_mut().find(|s| s.orig_index == index).unwrap().line_info
                });

                let mut rows = program.rows();
                while let Some((_header, row)) = rows.next_row()? {
                    if let (Some(line), Some(lines)) = (row.line(), &mut lines) {
                        lines.insert(row.address(), line.get());
                    }
                    if row.end_sequence() {
                        // The next row is the start of a new sequence, which means we must
                        // advance to the next .text section.
                        let section_index = text_sections.next().map(|s| s.index().0);
                        lines = section_index.map(|index| {
                            &mut sections
                                .iter_mut()
                                .find(|s| s.orig_index == index)
                                .unwrap()
                                .line_info
                        });
                    }
                }
            }
        }
        if iter.next()?.is_some() {
            log::warn!("Multiple units found in DWARF data, only processing the first");
        }
    }

    Ok(())
}

fn update_combined_symbol(symbol: ObjSymbol, address_change: i64) -> Result<ObjSymbol> {
    Ok(ObjSymbol {
        name: symbol.name,
        demangled_name: symbol.demangled_name,
		has_extab: symbol.has_extab,
        address: (symbol.address as i64 + address_change).try_into()?,
        section_address: (symbol.section_address as i64 + address_change).try_into()?,
        size: symbol.size,
        size_known: symbol.size_known,
        flags: symbol.flags,
        addend: symbol.addend,
        virtual_address: if let Some(virtual_address) = symbol.virtual_address {
            Some((virtual_address as i64 + address_change).try_into()?)
        } else {
            None
        },
    })
}

fn combine_sections(section: ObjSection, combine: ObjSection) -> Result<ObjSection> {
    let mut data = section.data;
    data.extend(combine.data);

    let address_change: i64 = (section.address + section.size) as i64 - combine.address as i64;
    let mut symbols = section.symbols;
    for symbol in combine.symbols {
        symbols.push(update_combined_symbol(symbol, address_change)?);
    }

    let mut relocations = section.relocations;
    for reloc in combine.relocations {
        relocations.push(ObjReloc {
            flags: reloc.flags,
            address: (reloc.address as i64 + address_change).try_into()?,
            target: reloc.target,                 // TODO: Should be updated?
            target_section: reloc.target_section, // TODO: Same as above
        });
    }

    let mut line_info = section.line_info;
    for (addr, line) in combine.line_info {
        let key = (addr as i64 + address_change).try_into()?;
        line_info.insert(key, line);
    }

    Ok(ObjSection {
        name: section.name,
        kind: section.kind,
        address: section.address,
        size: section.size + combine.size,
        data,
        orig_index: section.orig_index,
        symbols,
        relocations,
        virtual_address: section.virtual_address,
        line_info,
    })
}

fn combine_data_sections(sections: &mut Vec<ObjSection>) -> Result<()> {
    let names_to_combine: HashSet<_> = sections
        .iter()
        .filter(|s| s.kind == ObjSectionKind::Data)
        .map(|s| s.name.clone())
        .collect();

    for name in names_to_combine {
        // Take section with lowest index
        let (mut section_index, _) = sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name == name)
            .min_by_key(|(_, s)| s.orig_index)
            // Should not happen
            .context("No combine section found with name")?;
        let mut section = sections.remove(section_index);

        // Remove equally named sections
        let mut combines = vec![];
        for i in (0..sections.len()).rev() {
            if sections[i].name != name || sections[i].orig_index == section.orig_index {
                continue;
            }
            combines.push(sections.remove(i));
            if i < section_index {
                section_index -= 1;
            }
        }

        // Combine sections ordered by index
        combines.sort_unstable_by_key(|c| c.orig_index);
        for combine in combines {
            section = combine_sections(section, combine)?;
        }
        sections.insert(section_index, section);
    }
    Ok(())
}

pub fn read(obj_path: &Path, config: &DiffObjConfig) -> Result<ObjInfo> {
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
    if config.combine_data_sections {
        combine_data_sections(&mut sections)?;
    }
    line_info(&obj_file, &mut sections)?;
    let common = common_symbols(arch.as_ref(), &obj_file, split_meta.as_ref())?;
	let extab = exception_tables(arch.as_ref(), &mut sections, &obj_file, split_meta.as_ref());
    Ok(ObjInfo { arch, path: obj_path.to_owned(), timestamp, sections, common, extab, split_meta })
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
