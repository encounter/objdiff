use std::{collections::{HashMap, HashSet}, fs, io::Cursor, mem::size_of, path::Path};

use anyhow::{anyhow, bail, ensure, Context, Result};
use filetime::FileTime;
use flagset::Flags;
use object::{
    endian::LittleEndian as LE,
    pe::{ImageAuxSymbolFunctionBeginEnd, ImageLinenumber},
    read::coff::{CoffFile, CoffHeader, ImageSymbol},
    BinaryFormat, File, Object, ObjectSection, ObjectSymbol, RelocationTarget, SectionIndex,
    SectionKind, Symbol, SymbolIndex, SymbolKind, SymbolScope, SymbolSection,
};

use crate::{
    arch::{new_arch, ObjArch},
    diff::DiffObjConfig,
    obj::{
        split_meta::{SplitMeta, SPLITMETA_SECTION},
        ObjInfo, ObjReloc, ObjSection, ObjSectionKind, ObjSymbol, ObjSymbolFlagSet, ObjSymbolFlags,
    },
    util::{read_u16, read_u32},
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
    if arch
        .ppc()
        .and_then(|a| a.extab.as_ref())
        .map_or(false, |e| e.contains_key(&symbol.index().0))
    {
        flags = ObjSymbolFlagSet(flags.0 | ObjSymbolFlags::HasExtra);
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
        address,
        section_address,
        size: symbol.size(),
        size_known: symbol.size() != 0,
        flags,
        addend,
        virtual_address,
        original_index: Some(symbol.index().0),
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
    result.sort_by(|a, b| a.address.cmp(&b.address).then(a.size.cmp(&b.size)));
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
    if result.is_empty() {
        // Dummy symbol for empty sections
        result.push(ObjSymbol {
            name: format!("[{}]", section.name),
            demangled_name: None,
            address: 0,
            section_address: 0,
            size: section.size,
            size_known: true,
            flags: Default::default(),
            addend: 0,
            virtual_address: None,
            original_index: None,
        });
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
        original_index: None,
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
            arch.implcit_addend(obj_file, section, address, &reloc)?
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

fn line_info(obj_file: &File<'_>, sections: &mut [ObjSection], obj_data: &[u8]) -> Result<()> {
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
            let size = read_u32(obj_file, &mut reader)?;
            let base_address = read_u32(obj_file, &mut reader)? as u64;
            let Some(out_section) =
                sections.iter_mut().find(|s| s.orig_index == text_section_index)
            else {
                // Skip line info for sections we filtered out
                reader.set_position(start + size as u64);
                continue;
            };
            let end = start + size as u64;
            while reader.position() < end {
                let line_number = read_u32(obj_file, &mut reader)?;
                let statement_pos = read_u16(obj_file, &mut reader)?;
                if statement_pos != 0xFFFF {
                    log::warn!("Unhandled statement pos {}", statement_pos);
                }
                let address_delta = read_u32(obj_file, &mut reader)? as u64;
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
                        lines.insert(row.address(), line.get() as u32);
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

    // COFF
    if let File::Coff(coff) = obj_file {
        line_info_coff(coff, sections, obj_data)?;
    }

    Ok(())
}

fn line_info_coff(coff: &CoffFile, sections: &mut [ObjSection], obj_data: &[u8]) -> Result<()> {
    let symbol_table = coff.coff_header().symbols(obj_data)?;

    // Enumerate over all sections.
    for sect in coff.sections() {
        let ptr_linenums = sect.coff_section().pointer_to_linenumbers.get(LE) as usize;
        let num_linenums = sect.coff_section().number_of_linenumbers.get(LE) as usize;

        // If we have no line number, skip this section.
        if num_linenums == 0 {
            continue;
        }

        // Find this section in our out_section. If it's not in out_section,
        // skip it.
        let Some(out_section) = sections.iter_mut().find(|s| s.orig_index == sect.index().0) else {
            continue;
        };

        // Turn the line numbers into an ImageLinenumber slice.
        let Some(linenums) =
            &obj_data.get(ptr_linenums..ptr_linenums + num_linenums * size_of::<ImageLinenumber>())
        else {
            continue;
        };
        let Ok(linenums) = object::pod::slice_from_all_bytes::<ImageLinenumber>(linenums) else {
            continue;
        };

        // In COFF, the line numbers are stored relative to the start of the
        // function. Because of this, we need to know the line number where the
        // function starts, so we can sum the two and get the line number
        // relative to the start of the file.
        //
        // This variable stores the line number where the function currently
        // being processed starts. It is set to None when we failed to find the
        // line number of the start of the function.
        let mut cur_fun_start_linenumber = None;
        for linenum in linenums {
            let line_number = linenum.linenumber.get(LE);
            if line_number == 0 {
                // Starting a new function. We need to find the line where that
                // function is located in the file. To do this, we need to find
                // the `.bf` symbol "associated" with this function. The .bf
                // symbol will have a Function Begin/End Auxillary Record, which
                // contains the line number of the start of the function.

                // First, set cur_fun_start_linenumber to None. If we fail to
                // find the start of the function, this will make sure the
                // subsequent line numbers will be ignored until the next start
                // of function.
                cur_fun_start_linenumber = None;

                // Get the symbol associated with this function. We'll need it
                // for logging purposes, but also to acquire its Function
                // Auxillary Record, which tells us where to find our .bf symbol.
                let symtable_entry = linenum.symbol_table_index_or_virtual_address.get(LE);
                let Ok(symbol) = symbol_table.symbol(SymbolIndex(symtable_entry as usize)) else {
                    continue;
                };
                let Ok(aux_fun) = symbol_table.aux_function(SymbolIndex(symtable_entry as usize))
                else {
                    continue;
                };

                // Get the .bf symbol associated with this symbol. To do so, we
                // look at the Function Auxillary Record's tag_index, which is
                // an index in the symbol table pointing to our .bf symbol.
                if aux_fun.tag_index.get(LE) == 0 {
                    continue;
                }
                let Ok(bf_symbol) =
                    symbol_table.symbol(SymbolIndex(aux_fun.tag_index.get(LE) as usize))
                else {
                    continue;
                };
                // Do some sanity checks that we are, indeed, looking at a .bf
                // symbol.
                if bf_symbol.name(symbol_table.strings()) != Ok(b".bf") {
                    continue;
                }
                // Get the Function Begin/End Auxillary Record associated with
                // our .bf symbol, where we'll fine the linenumber of the start
                // of our function.
                let Ok(bf_aux) = symbol_table.get::<ImageAuxSymbolFunctionBeginEnd>(
                    SymbolIndex(aux_fun.tag_index.get(LE) as usize),
                    1,
                ) else {
                    continue;
                };
                // Set cur_fun_start_linenumber so the following linenumber
                // records will know at what line the current function start.
                cur_fun_start_linenumber = Some(bf_aux.linenumber.get(LE) as u32);
                // Let's also synthesize a line number record from the start of
                // the function, as the linenumber records don't always cover it.
                out_section.line_info.insert(
                    sect.address() + symbol.value() as u64,
                    bf_aux.linenumber.get(LE) as u32,
                );
            } else if let Some(cur_linenumber) = cur_fun_start_linenumber {
                let vaddr = linenum.symbol_table_index_or_virtual_address.get(LE);
                out_section
                    .line_info
                    .insert(sect.address() + vaddr as u64, cur_linenumber + line_number as u32);
            }
        }
    }
    Ok(())
}

fn update_combined_symbol(symbol: ObjSymbol, address_change: i64) -> Result<ObjSymbol> {
    Ok(ObjSymbol {
        name: symbol.name,
        demangled_name: symbol.demangled_name,
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
        original_index: symbol.original_index,
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
    let mut obj = parse(&data, config)?;
    obj.path = Some(obj_path.to_owned());
    obj.timestamp = Some(timestamp);
    Ok(obj)
}

pub fn parse(data: &[u8], config: &DiffObjConfig) -> Result<ObjInfo> {
    let obj_file = File::parse(data)?;
    let arch = new_arch(&obj_file)?;
    let split_meta = split_meta(&obj_file)?;
    let mut sections = filter_sections(&obj_file, split_meta.as_ref())?;
    for section in &mut sections {
        section.symbols =
            symbols_by_section(arch.as_ref(), &obj_file, section, split_meta.as_ref())?;
        section.relocations =
            relocations_by_section(arch.as_ref(), &obj_file, section, split_meta.as_ref())?;
    }
    // The dummy symbols all have the same name, which means that only the first one can be
    // compared
    let all_names = sections
        .iter()
        .flat_map(|section| section.symbols.iter().map(|symbol| symbol.name.clone()));
    let mut name_counts: HashMap<String, usize> = HashMap::new();
    for name in all_names {
        name_counts.entry(name).and_modify(|i| *i += 1).or_insert(1);
    }
    // Reversing the iterator here means the symbol sections will be given in the expected order
    // since they're assigned from the count down to prevent
    // having to use another hashmap which counts up
    // TODO what about reverse_fn_order?
    for section in sections.iter_mut().rev() {
        for symbol in section.symbols.iter_mut().rev() {
            if name_counts[&symbol.name] > 1 {
                name_counts.entry(symbol.name.clone()).and_modify(|i| *i -= 1);
                symbol.name = format!("{} {}", &symbol.name, name_counts[&symbol.name]);
            }
        }
    }
    if config.combine_data_sections {
        combine_data_sections(&mut sections)?;
    }
    line_info(&obj_file, &mut sections, data)?;
    let common = common_symbols(arch.as_ref(), &obj_file, split_meta.as_ref())?;
    Ok(ObjInfo { arch, path: None, timestamp: None, sections, common, split_meta })
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
