use anyhow::{Context, Result};
use object::{Object, ObjectSection};
use typed_arena::Arena;

use crate::obj::{Section, SectionKind};

/// Parse line information from DWARF 2+ sections.
pub(crate) fn parse_line_info_dwarf2(
    obj_file: &object::File,
    sections: &mut [Section],
) -> Result<()> {
    let arena_data = Arena::new();
    let arena_relocations = Arena::new();
    let endian = match obj_file.endianness() {
        object::Endianness::Little => gimli::RunTimeEndian::Little,
        object::Endianness::Big => gimli::RunTimeEndian::Big,
    };
    let dwarf = gimli::Dwarf::load(|id: gimli::SectionId| -> Result<_> {
        load_file_section(id, obj_file, endian, &arena_data, &arena_relocations)
    })
    .context("loading DWARF sections")?;

    let mut iter = dwarf.units();
    if let Some(header) = iter.next().map_err(|e| gimli_error(e, "iterating over DWARF units"))? {
        let unit = dwarf.unit(header).map_err(|e| gimli_error(e, "loading DWARF unit"))?;
        if let Some(program) = unit.line_program.clone() {
            let mut text_sections = sections.iter_mut().filter(|s| s.kind == SectionKind::Code);
            let mut lines = text_sections.next().map(|section| &mut section.line_info);

            let mut rows = program.rows();
            while let Some((_header, row)) =
                rows.next_row().map_err(|e| gimli_error(e, "loading program row"))?
            {
                if let (Some(line), Some(lines)) = (row.line(), &mut lines) {
                    lines.insert(row.address(), line.get() as u32);
                }
                if row.end_sequence() {
                    // The next row is the start of a new sequence, which means we must
                    // advance to the next .text section.
                    lines = text_sections.next().map(|section| &mut section.line_info);
                }
            }
        }
    }
    if iter.next().map_err(|e| gimli_error(e, "checking for next unit"))?.is_some() {
        log::warn!("Multiple units found in DWARF data, only processing the first");
    }

    Ok(())
}

#[derive(Debug, Default)]
struct RelocationMap(object::read::RelocationMap);

impl RelocationMap {
    fn add(&mut self, file: &object::File, section: &object::Section) {
        for (offset, relocation) in section.relocations() {
            if let Err(e) = self.0.add(file, offset, relocation) {
                log::error!(
                    "Relocation error for section {} at offset 0x{:08x}: {}",
                    section.name().unwrap(),
                    offset,
                    e
                );
            }
        }
    }
}

impl gimli::read::Relocate for &'_ RelocationMap {
    fn relocate_address(&self, offset: usize, value: u64) -> gimli::Result<u64> {
        Ok(self.0.relocate(offset as u64, value))
    }

    fn relocate_offset(&self, offset: usize, value: usize) -> gimli::Result<usize> {
        <usize as gimli::ReaderOffset>::from_u64(self.0.relocate(offset as u64, value as u64))
    }
}

type Relocate<'a, R> = gimli::RelocateReader<R, &'a RelocationMap>;

fn load_file_section<'input, 'arena, Endian: gimli::Endianity>(
    id: gimli::SectionId,
    file: &object::File<'input>,
    endian: Endian,
    arena_data: &'arena Arena<alloc::borrow::Cow<'input, [u8]>>,
    arena_relocations: &'arena Arena<RelocationMap>,
) -> Result<Relocate<'arena, gimli::EndianSlice<'arena, Endian>>> {
    let mut relocations = RelocationMap::default();
    let data = match file.section_by_name(id.name()) {
        Some(ref section) => {
            relocations.add(file, section);
            section.uncompressed_data()?
        }
        // Use a non-zero capacity so that `ReaderOffsetId`s are unique.
        None => alloc::borrow::Cow::Owned(Vec::with_capacity(1)),
    };
    let data_ref = arena_data.alloc(data);
    let section = gimli::EndianSlice::new(data_ref, endian);
    let relocations = arena_relocations.alloc(relocations);
    Ok(Relocate::new(section, relocations))
}

fn gimli_error(e: gimli::Error, context: &str) -> anyhow::Error {
    anyhow::anyhow!("gimli error {context}: {e:?}")
}
