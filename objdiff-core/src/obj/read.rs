use alloc::{
    boxed::Box,
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::{cmp::Ordering, num::NonZeroU64};

use anyhow::{Context, Result, anyhow, bail, ensure};
use object::{Object as _, ObjectSection as _, ObjectSymbol as _};

use crate::{
    arch::{Arch, RelocationOverride, RelocationOverrideTarget, new_arch},
    diff::DiffObjConfig,
    obj::{
        FlowAnalysisResult, Object, Relocation, RelocationFlags, Section, SectionData, SectionFlag,
        SectionKind, Symbol, SymbolFlag, SymbolKind,
        split_meta::{SPLITMETA_SECTION, SplitMeta},
    },
    util::{align_data_slice_to, align_u64_to, read_u16, read_u32},
};

fn map_section_kind(section: &object::Section) -> SectionKind {
    match section.kind() {
        object::SectionKind::Text => SectionKind::Code,
        object::SectionKind::Data
        | object::SectionKind::ReadOnlyData
        | object::SectionKind::ReadOnlyString
        | object::SectionKind::Tls => SectionKind::Data,
        object::SectionKind::UninitializedData
        | object::SectionKind::UninitializedTls
        | object::SectionKind::Common => SectionKind::Bss,
        _ => SectionKind::Unknown,
    }
}

fn map_symbol(
    arch: &dyn Arch,
    file: &object::File,
    symbol: &object::Symbol,
    section_indices: &[usize],
    split_meta: Option<&SplitMeta>,
) -> Result<Symbol> {
    let mut name = symbol.name().context("Failed to process symbol name")?.to_string();
    let mut size = symbol.size();
    if let (object::SymbolKind::Section, Some(section)) =
        (symbol.kind(), symbol.section_index().and_then(|i| file.section_by_index(i).ok()))
    {
        let section_name = section.name().context("Failed to process section name")?;
        name = format!("[{section_name}]");
        // For section symbols, set the size to zero. If the size is non-zero, it will be included
        // in the diff. Most of the time, this is duplicative, given that we'll have function or
        // object symbols that cover the same range. In the case of an empty section, the size
        // inference logic below will set the size back to the section size, thus acting as a
        // placeholder symbol.
        size = 0;
    }

    let mut flags = arch.extra_symbol_flags(symbol);
    if symbol.is_global() {
        flags |= SymbolFlag::Global;
    }
    if symbol.is_local() {
        flags |= SymbolFlag::Local;
    }
    if symbol.is_common() {
        flags |= SymbolFlag::Common;
    }
    if symbol.is_weak() {
        flags |= SymbolFlag::Weak;
    }
    if file.format() == object::BinaryFormat::Elf && symbol.scope() == object::SymbolScope::Linkage
    {
        flags |= SymbolFlag::Hidden;
    }

    let kind = match symbol.kind() {
        object::SymbolKind::Text => SymbolKind::Function,
        object::SymbolKind::Data => SymbolKind::Object,
        object::SymbolKind::Section => SymbolKind::Section,
        _ => SymbolKind::Unknown,
    };
    let address = arch.symbol_address(symbol.address(), kind);
    let demangled_name = arch.demangle(&name);
    // Find the virtual address for the symbol if available
    let virtual_address = split_meta
        .and_then(|m| m.virtual_addresses.as_ref())
        .and_then(|v| v.get(symbol.index().0).cloned());
    let section = symbol.section_index().and_then(|i| section_indices.get(i.0).copied());

    Ok(Symbol {
        name,
        demangled_name,
        address,
        size,
        kind,
        section,
        flags,
        align: None, // TODO parse .comment
        virtual_address,
    })
}

fn map_symbols(
    arch: &dyn Arch,
    obj_file: &object::File,
    sections: &[Section],
    section_indices: &[usize],
    split_meta: Option<&SplitMeta>,
) -> Result<(Vec<Symbol>, Vec<usize>)> {
    let symbol_count = obj_file.symbols().count();
    let mut symbols = Vec::<Symbol>::with_capacity(symbol_count);
    let mut symbol_indices = Vec::<usize>::with_capacity(symbol_count + 1);
    for obj_symbol in obj_file.symbols() {
        if symbol_indices.len() <= obj_symbol.index().0 {
            symbol_indices.resize(obj_symbol.index().0 + 1, usize::MAX);
        }
        let symbol = map_symbol(arch, obj_file, &obj_symbol, section_indices, split_meta)?;
        symbol_indices[obj_symbol.index().0] = symbols.len();
        symbols.push(symbol);
    }

    // Infer symbol sizes for 0-size symbols
    infer_symbol_sizes(arch, &mut symbols, sections)?;

    Ok((symbols, symbol_indices))
}

/// When inferring a symbol's size, we ignore symbols that start with specific prefixes. They are
/// usually emitted as branch targets and do not represent the start of a function or object.
fn is_local_label(symbol: &Symbol) -> bool {
    const LABEL_PREFIXES: &[&str] = &[".L", "LAB_", "switchD_"];
    symbol.size == 0
        && symbol.flags.contains(SymbolFlag::Local)
        && LABEL_PREFIXES.iter().any(|p| symbol.name.starts_with(p))
}

fn infer_symbol_sizes(arch: &dyn Arch, symbols: &mut [Symbol], sections: &[Section]) -> Result<()> {
    // Create a sorted list of symbol indices by section
    let mut symbols_with_section = Vec::<usize>::with_capacity(symbols.len());
    for (i, symbol) in symbols.iter().enumerate() {
        if symbol.section.is_some() {
            symbols_with_section.push(i);
        }
    }
    symbols_with_section.sort_by(|a, b| {
        let a = &symbols[*a];
        let b = &symbols[*b];
        a.section
            .unwrap_or(usize::MAX)
            .cmp(&b.section.unwrap_or(usize::MAX))
            .then_with(|| {
                // Sort section symbols first
                if a.kind == SymbolKind::Section {
                    Ordering::Less
                } else if b.kind == SymbolKind::Section {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| a.address.cmp(&b.address))
            .then_with(|| a.size.cmp(&b.size))
    });

    // Set symbol sizes based on the next symbol's address
    let mut iter_idx = 0;
    let mut last_end = (0, 0);
    while iter_idx < symbols_with_section.len() {
        let symbol_idx = symbols_with_section[iter_idx];
        let symbol = &symbols[symbol_idx];
        let section_idx = symbol.section.unwrap();
        iter_idx += 1;
        if symbol.size != 0 {
            if symbol.kind != SymbolKind::Section {
                last_end = (section_idx, symbol.address + symbol.size);
            }
            continue;
        }
        // Skip over symbols that are contained within the previous symbol
        if last_end.0 == section_idx && last_end.1 > symbol.address {
            continue;
        }
        let next_symbol = loop {
            if iter_idx >= symbols_with_section.len() {
                break None;
            }
            let next_symbol = &symbols[symbols_with_section[iter_idx]];
            if next_symbol.section != Some(section_idx) {
                break None;
            }
            if match symbol.kind {
                SymbolKind::Function | SymbolKind::Object => {
                    // For function/object symbols, find the next function/object
                    matches!(next_symbol.kind, SymbolKind::Function | SymbolKind::Object)
                }
                SymbolKind::Unknown | SymbolKind::Section => {
                    // For labels (or anything else), stop at any symbol
                    true
                }
            } && !is_local_label(next_symbol)
            {
                break Some(next_symbol);
            }
            iter_idx += 1;
        };
        let section = &sections[section_idx];
        let next_address =
            next_symbol.map(|s| s.address).unwrap_or_else(|| section.address + section.size);
        let new_size = if section.kind == SectionKind::Code {
            arch.infer_function_size(symbol, section, next_address)?
        } else {
            next_address.saturating_sub(symbol.address)
        };
        if new_size > 0 {
            let symbol = &mut symbols[symbol_idx];
            symbol.size = new_size;
            if symbol.kind != SymbolKind::Section {
                symbol.flags |= SymbolFlag::SizeInferred;
            }
            // Set symbol kind if unknown and size is non-zero
            if symbol.kind == SymbolKind::Unknown {
                symbol.kind = match section.kind {
                    SectionKind::Code => SymbolKind::Function,
                    SectionKind::Data | SectionKind::Bss => SymbolKind::Object,
                    _ => SymbolKind::Unknown,
                };
            }
        }
    }
    Ok(())
}

fn map_sections(
    _arch: &dyn Arch,
    obj_file: &object::File,
    split_meta: Option<&SplitMeta>,
) -> Result<(Vec<Section>, Vec<usize>)> {
    let mut section_names = BTreeMap::<String, usize>::new();
    let section_count = obj_file.sections().count();
    let mut result = Vec::<Section>::with_capacity(section_count);
    let mut section_indices = Vec::<usize>::with_capacity(section_count + 1);
    for section in obj_file.sections() {
        let name = section.name().context("Failed to process section name")?;
        let kind = map_section_kind(&section);
        let data = if kind == SectionKind::Unknown {
            // Don't need to read data for unknown sections
            Vec::new()
        } else {
            section.uncompressed_data().context("Failed to read section data")?.into_owned()
        };

        // Find the virtual address for the section symbol if available
        let section_symbol = obj_file.symbols().find(|s| {
            s.kind() == object::SymbolKind::Section && s.section_index() == Some(section.index())
        });
        let virtual_address = section_symbol.and_then(|s| {
            split_meta
                .and_then(|m| m.virtual_addresses.as_ref())
                .and_then(|v| v.get(s.index().0).cloned())
        });

        let unique_id = section_names.entry(name.to_string()).or_insert(0);
        let id = format!("{name}-{unique_id}");
        *unique_id += 1;

        if section_indices.len() <= section.index().0 {
            section_indices.resize(section.index().0 + 1, usize::MAX);
        }
        section_indices[section.index().0] = result.len();
        result.push(Section {
            id,
            name: name.to_string(),
            address: section.address(),
            offset: section.file_range().map(|(start, _)| start),
            size: section.size(),
            kind,
            data: SectionData(data),
            flags: Default::default(),
            align: NonZeroU64::new(section.align()),
            relocations: Default::default(),
            virtual_address,
            line_info: Default::default(),
        });
    }
    Ok((result, section_indices))
}

const LOW_PRIORITY_SYMBOLS: &[&str] =
    &["__gnu_compiled_c", "__gnu_compiled_cplusplus", "gcc2_compiled."];

fn best_symbol<'r, 'data, 'file>(
    symbols: &'r [object::Symbol<'data, 'file>],
    address: u64,
) -> Option<(object::SymbolIndex, u64)> {
    let mut closest_symbol_index = match symbols.binary_search_by_key(&address, |s| s.address()) {
        Ok(index) => Some(index),
        Err(index) => index.checked_sub(1),
    }?;
    // The binary search may not find the first symbol at the address, so work backwards
    let target_address = symbols[closest_symbol_index].address();
    while let Some(prev_index) = closest_symbol_index.checked_sub(1) {
        if symbols[prev_index].address() != target_address {
            break;
        }
        closest_symbol_index = prev_index;
    }
    let mut best_symbol: Option<&'r object::Symbol<'data, 'file>> = None;
    for symbol in symbols.iter().skip(closest_symbol_index) {
        if symbol.address() > address {
            break;
        }
        if symbol.kind() == object::SymbolKind::Section
            || (symbol.size() > 0 && (symbol.address() + symbol.size()) <= address)
        {
            continue;
        }
        // TODO priority ranking with visibility, etc
        if let Some(best) = best_symbol {
            if LOW_PRIORITY_SYMBOLS.contains(&best.name().unwrap_or_default())
                && !LOW_PRIORITY_SYMBOLS.contains(&symbol.name().unwrap_or_default())
            {
                best_symbol = Some(symbol);
            }
        } else {
            best_symbol = Some(symbol);
        }
    }
    best_symbol.map(|s| (s.index(), s.address()))
}

fn map_section_relocations(
    arch: &dyn Arch,
    obj_file: &object::File,
    obj_section: &object::Section,
    symbol_indices: &[usize],
    ordered_symbols: &[Vec<object::Symbol>],
) -> Result<Vec<Relocation>> {
    let mut relocations = Vec::<Relocation>::with_capacity(obj_section.relocations().count());
    for (address, reloc) in obj_section.relocations() {
        let mut target_reloc = RelocationOverride {
            target: match reloc.target() {
                object::RelocationTarget::Symbol(symbol) => {
                    RelocationOverrideTarget::Symbol(symbol)
                }
                object::RelocationTarget::Section(section) => {
                    RelocationOverrideTarget::Section(section)
                }
                _ => RelocationOverrideTarget::Skip,
            },
            addend: reloc.addend(),
        };

        // Allow the architecture to override the relocation target and addend
        match arch.relocation_override(obj_file, obj_section, address, &reloc)? {
            Some(reloc_override) => {
                match reloc_override.target {
                    RelocationOverrideTarget::Keep => {}
                    target => {
                        target_reloc.target = target;
                    }
                }
                target_reloc.addend = reloc_override.addend;
            }
            None => {
                ensure!(
                    !reloc.has_implicit_addend(),
                    "Unsupported {:?} implicit relocation {:?}",
                    obj_file.architecture(),
                    reloc.flags()
                );
            }
        }

        // Resolve the relocation target symbol
        let (symbol_index, addend) = match target_reloc.target {
            RelocationOverrideTarget::Keep => unreachable!(),
            RelocationOverrideTarget::Skip => continue,
            RelocationOverrideTarget::Symbol(symbol_index) => {
                // Sometimes used to indicate "absolute"
                if symbol_index.0 == u32::MAX as usize {
                    continue;
                }

                // If the target is a section symbol, try to resolve a better symbol as the target
                if let Some(section_symbol) = obj_file
                    .symbol_by_index(symbol_index)
                    .ok()
                    .filter(|s| s.kind() == object::SymbolKind::Section)
                {
                    let section_index =
                        section_symbol.section_index().context("Section symbol without section")?;
                    let target_address =
                        section_symbol.address().wrapping_add_signed(target_reloc.addend);
                    if let Some((new_idx, addr)) = ordered_symbols
                        .get(section_index.0)
                        .and_then(|symbols| best_symbol(symbols, target_address))
                    {
                        (new_idx, target_address.wrapping_sub(addr) as i64)
                    } else {
                        (symbol_index, target_reloc.addend)
                    }
                } else {
                    (symbol_index, target_reloc.addend)
                }
            }
            RelocationOverrideTarget::Section(section_index) => {
                let section = match obj_file.section_by_index(section_index) {
                    Ok(section) => section,
                    Err(e) => {
                        log::warn!("Invalid relocation section: {e}");
                        continue;
                    }
                };
                let Ok(target_address) = u64::try_from(target_reloc.addend) else {
                    log::warn!(
                        "Negative section relocation addend: {}{}",
                        section.name()?,
                        target_reloc.addend
                    );
                    continue;
                };
                let Some(symbols) = ordered_symbols.get(section_index.0) else {
                    log::warn!(
                        "Couldn't resolve relocation target symbol for section {} (no symbols)",
                        section.name()?
                    );
                    continue;
                };
                // Attempt to resolve a target symbol for the relocation
                if let Some((new_idx, addr)) = best_symbol(symbols, target_address) {
                    (new_idx, target_address.wrapping_sub(addr) as i64)
                } else if let Some(section_symbol) =
                    symbols.iter().find(|s| s.kind() == object::SymbolKind::Section)
                {
                    (
                        section_symbol.index(),
                        target_address.wrapping_sub(section_symbol.address()) as i64,
                    )
                } else {
                    log::warn!(
                        "Couldn't resolve relocation target symbol for section {}",
                        section.name()?
                    );
                    continue;
                }
            }
        };

        let flags = match reloc.flags() {
            object::RelocationFlags::Elf { r_type } => RelocationFlags::Elf(r_type),
            object::RelocationFlags::Coff { typ } => RelocationFlags::Coff(typ),
            flags => bail!("Unhandled relocation flags: {:?}", flags),
        };
        let target_symbol = match symbol_indices.get(symbol_index.0).copied() {
            Some(i) => i,
            None => {
                log::warn!("Invalid symbol index {}", symbol_index.0);
                continue;
            }
        };
        relocations.push(Relocation { address, flags, target_symbol, addend });
    }
    relocations.sort_by_key(|r| r.address);
    Ok(relocations)
}

fn map_relocations(
    arch: &dyn Arch,
    obj_file: &object::File,
    sections: &mut [Section],
    section_indices: &[usize],
    symbol_indices: &[usize],
) -> Result<()> {
    // Generate a list of symbols for each section
    let mut ordered_symbols =
        Vec::<Vec<object::Symbol>>::with_capacity(obj_file.sections().count() + 1);
    for symbol in obj_file.symbols() {
        let Some(section_index) = symbol.section_index() else {
            continue;
        };
        if symbol.kind() == object::SymbolKind::Section {
            continue;
        }
        if section_index.0 >= ordered_symbols.len() {
            ordered_symbols.resize_with(section_index.0 + 1, Vec::new);
        }
        ordered_symbols[section_index.0].push(symbol);
    }
    // Sort symbols by address and size
    for vec in &mut ordered_symbols {
        vec.sort_by(|a, b| a.address().cmp(&b.address()).then(a.size().cmp(&b.size())));
    }
    // Map relocations for each section. Section-relative relocations use the ordered symbols list
    // to find a better target symbol, if available.
    for obj_section in obj_file.sections() {
        let section = &mut sections[section_indices[obj_section.index().0]];
        if section.kind != SectionKind::Unknown {
            section.relocations = map_section_relocations(
                arch,
                obj_file,
                &obj_section,
                symbol_indices,
                &ordered_symbols,
            )?;
        }
    }
    Ok(())
}

fn perform_data_flow_analysis(obj: &mut Object, config: &DiffObjConfig) -> Result<()> {
    // If neither of these settings are on, no flow analysis to perform
    if !config.analyze_data_flow && !config.ppc_calculate_pool_relocations {
        return Ok(());
    }

    let mut generated_relocations = Vec::<(usize, Vec<Relocation>)>::new();
    let mut generated_flow_results = Vec::<(Symbol, Box<dyn FlowAnalysisResult>)>::new();
    for (section_index, section) in obj.sections.iter().enumerate() {
        if section.kind != SectionKind::Code {
            continue;
        }
        for symbol in obj.symbols.iter() {
            if symbol.section != Some(section_index) {
                continue;
            }
            if symbol.kind != SymbolKind::Function {
                continue;
            }
            let code =
                section.data_range(symbol.address, symbol.size as usize).ok_or_else(|| {
                    anyhow!(
                        "Symbol data out of bounds: {:#x}..{:#x}",
                        symbol.address,
                        symbol.address + symbol.size
                    )
                })?;

            // Optional pooled relocation computation
            // Long view: This could be replaced by the full data flow analysis
            // once that feature has stabilized.
            if config.ppc_calculate_pool_relocations {
                let relocations = obj.arch.generate_pooled_relocations(
                    symbol.address,
                    code,
                    &section.relocations,
                    &obj.symbols,
                );
                generated_relocations.push((section_index, relocations));
            }

            // Optional full data flow analysis
            if config.analyze_data_flow
                && let Some(flow_result) =
                    obj.arch.data_flow_analysis(obj, symbol, code, &section.relocations)
            {
                generated_flow_results.push((symbol.clone(), flow_result));
            }
        }
    }
    for (symbol, flow_result) in generated_flow_results {
        obj.add_flow_analysis_result(&symbol, flow_result);
    }
    for (section_index, mut relocations) in generated_relocations {
        obj.sections[section_index].relocations.append(&mut relocations);
    }
    for section in obj.sections.iter_mut() {
        section.relocations.sort_by_key(|r| r.address);
    }
    Ok(())
}

fn parse_line_info(
    obj_file: &object::File,
    sections: &mut [Section],
    section_indices: &[usize],
    obj_data: &[u8],
) -> Result<()> {
    // DWARF 1.1
    if let Err(e) = parse_line_info_dwarf1(obj_file, sections) {
        log::warn!("Failed to parse DWARF 1.1 line info: {e}");
    }

    // DWARF 2+
    #[cfg(feature = "dwarf")]
    if let Err(e) = super::dwarf2::parse_line_info_dwarf2(obj_file, sections) {
        log::warn!("Failed to parse DWARF 2+ line info: {e}");
    }

    // COFF
    if let object::File::Coff(coff) = obj_file
        && let Err(e) = parse_line_info_coff(coff, sections, section_indices, obj_data)
    {
        log::warn!("Failed to parse COFF line info: {e}");
    }

    Ok(())
}

/// Parse .line section from DWARF 1.1 format.
fn parse_line_info_dwarf1(obj_file: &object::File, sections: &mut [Section]) -> Result<()> {
    if let Some(section) = obj_file.section_by_name(".line") {
        let data = section.uncompressed_data()?;
        let mut reader: &[u8] = data.as_ref();

        let mut text_sections = sections.iter_mut().filter(|s| s.kind == SectionKind::Code);
        while !reader.is_empty() {
            let mut section_data = reader;
            let size = read_u32(obj_file, &mut section_data)? as usize;
            if size > reader.len() {
                bail!("Line info size {size} exceeds remaining size {}", reader.len());
            }
            (section_data, reader) = reader.split_at(size);

            section_data = &section_data[4..]; // Skip the size field
            let base_address = read_u32(obj_file, &mut section_data)? as u64;
            let out_section = text_sections.next().context("No text section for line info")?;
            while !section_data.is_empty() {
                let line_number = read_u32(obj_file, &mut section_data)?;
                let statement_pos = read_u16(obj_file, &mut section_data)?;
                if statement_pos != 0xFFFF {
                    log::warn!("Unhandled statement pos {statement_pos}");
                }
                let address_delta = read_u32(obj_file, &mut section_data)? as u64;
                out_section.line_info.insert(base_address + address_delta, line_number);
            }
        }
    }
    Ok(())
}

fn parse_line_info_coff(
    coff: &object::coff::CoffFile,
    sections: &mut [Section],
    section_indices: &[usize],
    obj_data: &[u8],
) -> Result<()> {
    use object::{
        coff::{CoffHeader as _, ImageSymbol as _},
        endian::LittleEndian as LE,
    };
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
        let Some(out_section) =
            section_indices.get(sect.index().0).and_then(|&i| sections.get_mut(i))
        else {
            continue;
        };

        // Turn the line numbers into an ImageLinenumber slice.
        let Some(linenums) = &obj_data.get(
            ptr_linenums..ptr_linenums + num_linenums * size_of::<object::pe::ImageLinenumber>(),
        ) else {
            continue;
        };
        let Ok(linenums) =
            object::pod::slice_from_all_bytes::<object::pe::ImageLinenumber>(linenums)
        else {
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
                let Ok(symbol) = symbol_table.symbol(object::SymbolIndex(symtable_entry as usize))
                else {
                    continue;
                };
                let Ok(aux_fun) =
                    symbol_table.aux_function(object::SymbolIndex(symtable_entry as usize))
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
                    symbol_table.symbol(object::SymbolIndex(aux_fun.tag_index.get(LE) as usize))
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
                let Ok(bf_aux) = symbol_table.get::<object::pe::ImageAuxSymbolFunctionBeginEnd>(
                    object::SymbolIndex(aux_fun.tag_index.get(LE) as usize),
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

fn combine_sections(
    sections: &mut [Section],
    symbols: &mut [Symbol],
    config: &DiffObjConfig,
) -> Result<()> {
    let mut data_sections = BTreeMap::<String, Vec<usize>>::new();
    let mut text_sections = Vec::<usize>::new();
    for (i, section) in sections.iter().enumerate() {
        match section.kind {
            SectionKind::Data | SectionKind::Bss => {
                let base_name = if let Some(i) = section.name.rfind('$') {
                    &section.name[..i]
                } else {
                    &section.name
                };
                data_sections.entry(base_name.to_string()).or_default().push(i);
            }
            SectionKind::Code => {
                text_sections.push(i);
            }
            _ => {}
        }
    }
    if config.combine_data_sections {
        for (combined_name, mut section_indices) in data_sections {
            do_combine_sections(sections, symbols, &mut section_indices, combined_name)?;
        }
    }
    if config.combine_text_sections {
        do_combine_sections(sections, symbols, &mut text_sections, ".text".to_string())?;
    }
    Ok(())
}

fn do_combine_sections(
    sections: &mut [Section],
    symbols: &mut [Symbol],
    section_indices: &mut [usize],
    combined_name: String,
) -> Result<()> {
    if section_indices.len() < 2 {
        return Ok(());
    }
    // Sort sections lexicographically by name (for COFF section groups)
    section_indices.sort_by(|&a, &b| {
        let a_name = &sections[a].name;
        let b_name = &sections[b].name;
        // .text$di < .text$mn < .text
        if a_name.contains('$') && !b_name.contains('$') {
            return Ordering::Less;
        } else if !a_name.contains('$') && b_name.contains('$') {
            return Ordering::Greater;
        }
        a_name.cmp(b_name)
    });
    let first_section_idx = section_indices[0];

    // Calculate the new offset for each section
    let mut offsets = Vec::<u64>::with_capacity(section_indices.len());
    let mut current_offset = 0;
    let mut data_size = 0;
    let mut num_relocations = 0;
    for i in section_indices.iter().copied() {
        let section = &sections[i];
        if section.address != 0 {
            bail!("Section {} ({}) has non-zero address", i, section.name);
        }
        offsets.push(current_offset);
        current_offset += section.size;
        let align = section.combined_alignment();
        current_offset = align_u64_to(current_offset, align);
        data_size += section.data.len();
        data_size = align_u64_to(data_size as u64, align) as usize;
        num_relocations += section.relocations.len();
    }
    if data_size > 0 {
        ensure!(data_size == current_offset as usize, "Data size mismatch");
    }

    // Combine section data
    let mut data = Vec::<u8>::with_capacity(data_size);
    let mut relocations = Vec::<Relocation>::with_capacity(num_relocations);
    let mut line_info = BTreeMap::<u64, u32>::new();
    for (&i, &offset) in section_indices.iter().zip(&offsets) {
        let section = &mut sections[i];
        section.size = 0;
        data.append(&mut section.data.0);
        align_data_slice_to(&mut data, section.combined_alignment());
        section.relocations.iter_mut().for_each(|r| r.address += offset);
        relocations.append(&mut section.relocations);
        line_info.append(&mut section.line_info.iter().map(|(&a, &l)| (a + offset, l)).collect());
        section.line_info.clear();
        if offset > 0 {
            section.kind = SectionKind::Unknown;
        }
    }
    {
        let first_section = &mut sections[first_section_idx];
        first_section.id = format!("{combined_name}-combined");
        first_section.name = combined_name;
        first_section.size = current_offset;
        first_section.data = SectionData(data);
        first_section.flags |= SectionFlag::Combined;
        first_section.relocations = relocations;
        first_section.line_info = line_info;
    }

    // Find all section symbols for the merged sections
    let mut section_symbols = symbols
        .iter()
        .enumerate()
        .filter(|&(_, s)| {
            s.kind == SymbolKind::Section && s.section.is_some_and(|i| section_indices.contains(&i))
        })
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    section_symbols.sort_by_key(|&i| symbols[i].section.unwrap());
    let target_section_symbol = section_symbols.first().copied();

    // Adjust symbol addresses and section indices
    for symbol in symbols.iter_mut() {
        let Some(section_index) = symbol.section else {
            continue;
        };
        let Some(merge_index) = section_indices.iter().position(|&i| i == section_index) else {
            continue;
        };
        symbol.address += offsets[merge_index];
        symbol.section = Some(first_section_idx);
    }

    // Adjust relocations to section symbols
    for relocation in sections.iter_mut().flat_map(|s| s.relocations.iter_mut()) {
        let target_symbol = &symbols[relocation.target_symbol];
        if target_symbol.kind != SymbolKind::Section {
            continue;
        }
        if !target_symbol.section.is_some_and(|i| section_indices.contains(&i)) {
            continue;
        }
        // The section symbol's address will have the offset applied
        relocation.target_symbol = target_section_symbol.context("No target section symbol")?;
        relocation.addend = relocation
            .addend
            .checked_add_unsigned(target_symbol.address)
            .context("Relocation addend overflow")?;
    }

    // Reset section symbols
    for (i, &symbol_index) in section_symbols.iter().enumerate() {
        let symbol = &mut symbols[symbol_index];
        symbol.address = 0;
        if i > 0 {
            // Remove the section symbol
            symbol.kind = SymbolKind::Unknown;
            symbol.section = None;
        }
    }

    Ok(())
}

#[cfg(feature = "std")]
pub fn read(obj_path: &std::path::Path, config: &DiffObjConfig) -> Result<Object> {
    let (data, timestamp) = {
        let file = std::fs::File::open(obj_path)?;
        let timestamp = filetime::FileTime::from_last_modification_time(&file.metadata()?);
        (unsafe { memmap2::Mmap::map(&file) }?, timestamp)
    };
    let mut obj = parse(&data, config)?;
    obj.path = Some(obj_path.to_path_buf());
    obj.timestamp = Some(timestamp);
    Ok(obj)
}

pub fn parse(data: &[u8], config: &DiffObjConfig) -> Result<Object> {
    let obj_file = object::File::parse(data)?;
    let mut arch = new_arch(&obj_file)?;
    let split_meta = parse_split_meta(&obj_file)?;
    let (mut sections, section_indices) =
        map_sections(arch.as_ref(), &obj_file, split_meta.as_ref())?;
    let (mut symbols, symbol_indices) =
        map_symbols(arch.as_ref(), &obj_file, &sections, &section_indices, split_meta.as_ref())?;
    map_relocations(arch.as_ref(), &obj_file, &mut sections, &section_indices, &symbol_indices)?;
    parse_line_info(&obj_file, &mut sections, &section_indices, data)?;
    if config.combine_data_sections || config.combine_text_sections {
        combine_sections(&mut sections, &mut symbols, config)?;
    }
    arch.post_init(&sections, &symbols);
    let mut obj = Object {
        arch,
        endianness: obj_file.endianness(),
        symbols,
        sections,
        split_meta,
        #[cfg(feature = "std")]
        path: None,
        #[cfg(feature = "std")]
        timestamp: None,
        flow_analysis_results: Default::default(),
    };

    // Need to construct the obj first so that we have a convinient package to
    // pass to flow analysis. Then the flow analysis will mutate obj adding
    // additional data to it.
    perform_data_flow_analysis(&mut obj, config)?;
    Ok(obj)
}

#[cfg(feature = "std")]
pub fn has_function(obj_path: &std::path::Path, symbol_name: &str) -> Result<bool> {
    let data = {
        let file = std::fs::File::open(obj_path)?;
        unsafe { memmap2::Mmap::map(&file) }?
    };
    Ok(object::File::parse(&*data)?
        .symbol_by_name(symbol_name)
        .filter(|o| o.kind() == object::SymbolKind::Text)
        .is_some())
}

fn parse_split_meta(obj_file: &object::File) -> Result<Option<SplitMeta>> {
    Ok(if let Some(section) = obj_file.section_by_name(SPLITMETA_SECTION) {
        Some(SplitMeta::from_section(section, obj_file.endianness(), obj_file.is_64())?)
    } else {
        None
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_combine_sections() {
        let mut sections = vec![
            Section {
                id: ".text-0".to_string(),
                name: ".text".to_string(),
                size: 8,
                kind: SectionKind::Code,
                data: SectionData(vec![0; 8]),
                relocations: vec![
                    Relocation {
                        address: 0,
                        flags: RelocationFlags::Elf(0),
                        target_symbol: 0,
                        addend: 0,
                    },
                    Relocation {
                        address: 2,
                        flags: RelocationFlags::Elf(0),
                        target_symbol: 1,
                        addend: 0,
                    },
                    Relocation {
                        address: 4,
                        flags: RelocationFlags::Elf(0),
                        target_symbol: 3,
                        addend: 2,
                    },
                ],
                ..Default::default()
            },
            Section {
                id: ".data-0".to_string(),
                name: ".data".to_string(),
                size: 4,
                kind: SectionKind::Data,
                data: SectionData(vec![1, 2, 3, 4]),
                relocations: vec![Relocation {
                    address: 0,
                    flags: RelocationFlags::Elf(0),
                    target_symbol: 2,
                    addend: 0,
                }],
                line_info: [(0, 1)].into_iter().collect(),
                ..Default::default()
            },
            Section {
                id: ".data-1".to_string(),
                name: ".data".to_string(),
                size: 4,
                kind: SectionKind::Data,
                data: SectionData(vec![5, 6, 7, 8]),
                relocations: vec![Relocation {
                    address: 0,
                    flags: RelocationFlags::Elf(0),
                    target_symbol: 2,
                    addend: 0,
                }],
                ..Default::default()
            },
            Section {
                id: ".data-2".to_string(),
                name: ".data".to_string(),
                size: 4,
                kind: SectionKind::Data,
                data: SectionData(vec![9, 10, 11, 12]),
                line_info: [(0, 2)].into_iter().collect(),
                ..Default::default()
            },
        ];
        let mut symbols = vec![
            Symbol {
                name: ".data".to_string(),
                address: 0,
                kind: SymbolKind::Section,
                section: Some(2),
                ..Default::default()
            },
            Symbol {
                name: "symbol".to_string(),
                address: 0,
                kind: SymbolKind::Object,
                size: 4,
                section: Some(2),
                ..Default::default()
            },
            Symbol {
                name: "function".to_string(),
                address: 0,
                size: 8,
                kind: SymbolKind::Function,
                section: Some(0),
                ..Default::default()
            },
            Symbol {
                name: ".data".to_string(),
                address: 0,
                kind: SymbolKind::Section,
                section: Some(3),
                ..Default::default()
            },
        ];
        do_combine_sections(&mut sections, &mut symbols, &mut [1, 2, 3], ".data".to_string())
            .unwrap();
        assert_eq!(sections[1].data.0, (1..=12).collect::<Vec<_>>());
        insta::assert_debug_snapshot!((sections, symbols));
    }
}
