use alloc::vec::Vec;

use anyhow::{Context, Result, bail, ensure};
use object::{Endianness, Object, ObjectSection};

use super::{Section, SectionKind};

const HDRR_SIZE: usize = 0x60;
const FDR_SIZE: usize = 0x48;
const PDR_SIZE: usize = 0x34;
const SYMR_SIZE: usize = 0x0c;

const ST_PROC: u8 = 6;
const ST_STATICPROC: u8 = 14;
const ST_END: u8 = 8;

pub(super) fn parse_line_info_mdebug(
    obj_file: &object::File,
    sections: &mut [Section],
) -> Result<()> {
    let Some(section) = obj_file.section_by_name(".mdebug") else {
        return Ok(());
    };

    let data = section.data().context("failed to read .mdebug contents")?;
    if data.len() < HDRR_SIZE {
        return Ok(());
    }

    let section_file_offset = section.file_range().map(|(offset, _)| offset as usize);

    let endianness = obj_file.endianness();
    let header = Header::parse(data, endianness)?;

    let symbols_data = slice_at(
        data,
        header.cb_sym_offset,
        header.isym_max.checked_mul(SYMR_SIZE as u32).context("symbol table size overflow")?,
        section_file_offset,
    )?;
    let symbols = parse_symbols(symbols_data, endianness)?;

    let fdr_data = slice_at(
        data,
        header.cb_fd_offset,
        header
            .ifd_max
            .checked_mul(FDR_SIZE as u32)
            .context("file descriptor table size overflow")?,
        section_file_offset,
    )?;

    for fdr_index in 0..header.ifd_max as usize {
        let fdr_offset = fdr_index * FDR_SIZE;
        let fdr = FileDescriptor::parse(&fdr_data[fdr_offset..fdr_offset + FDR_SIZE], endianness)?;
        if fdr.cpd == 0 || fdr.csym == 0 {
            continue;
        }

        let sym_base = fdr.isym_base as usize;
        let sym_end = sym_base + fdr.csym as usize;
        if sym_end > symbols.len() {
            continue;
        }

        let Some(line_file_offset) = header.cb_line_offset.checked_add(fdr.cb_line_offset) else {
            continue;
        };
        let Some(line_file_base) =
            resolve_offset(line_file_offset, data.len(), section_file_offset)
        else {
            continue;
        };
        let Some(line_file_end) = line_file_base.checked_add(fdr.cb_line as usize) else {
            continue;
        };
        if line_file_end > data.len() {
            continue;
        }

        for local_proc_index in 0..fdr.cpd as usize {
            let pdr_index = fdr.ipd_first as usize + local_proc_index;
            let pdr_offset = header
                .cb_pd_offset
                .checked_add((pdr_index as u32) * PDR_SIZE as u32)
                .context("procedure descriptor offset overflow")?;
            let pdr_data = match slice_at(data, pdr_offset, PDR_SIZE as u32, section_file_offset) {
                Ok(data) => data,
                Err(_) => continue,
            };
            let pdr = ProcDescriptor::parse(pdr_data, endianness)?;
            if pdr.isym as usize >= fdr.csym as usize {
                continue;
            }
            let global_sym_index = sym_base + pdr.isym as usize;
            let Some(start_symbol) = symbols.get(global_sym_index) else {
                continue;
            };
            if start_symbol.st != ST_PROC && start_symbol.st != ST_STATICPROC {
                continue;
            }

            let local_index = pdr.isym as u32;
            let mut end_address = None;
            for sym in &symbols[global_sym_index..sym_end] {
                if sym.st == ST_END && sym.index == local_index {
                    end_address = Some(sym.value);
                    break;
                }
            }
            let Some(end_address) = end_address else {
                continue;
            };
            let Some(size) = end_address.checked_sub(start_symbol.value) else {
                continue;
            };
            if size == 0 || size % 4 != 0 {
                continue;
            }
            let word_count = (size / 4) as usize;
            if word_count == 0 {
                continue;
            }

            let Some(mut cursor) = line_file_base.checked_add(pdr.cb_line_offset as usize) else {
                continue;
            };
            if cursor >= line_file_end {
                continue;
            }

            let mut line_number = pdr.ln_low as i32;
            let mut lines = Vec::with_capacity(word_count);
            while lines.len() < word_count && cursor < line_file_end {
                let b0 = data[cursor];
                cursor += 1;
                let count = (b0 & 0x0f) as usize + 1;
                let delta = decode_delta(endianness, b0 >> 4, data, &mut cursor, line_file_end)?;
                line_number = line_number.wrapping_add(delta as i32);
                for _ in 0..count {
                    if lines.len() == word_count {
                        break;
                    }
                    lines.push(line_number);
                }
            }

            if lines.len() != word_count {
                continue;
            }

            assign_lines(sections, fdr.adr as u64 + pdr.addr as u64, &lines);
        }
    }

    Ok(())
}

fn assign_lines(sections: &mut [Section], base_address: u64, lines: &[i32]) {
    let mut address = base_address;
    for &line in lines {
        if line >= 0
            && let Some(section) = find_code_section(sections, address)
        {
            section.line_info.insert(address, line as u32);
        }
        address = address.wrapping_add(4);
    }
}

fn find_code_section(sections: &mut [Section], address: u64) -> Option<&mut Section> {
    sections.iter_mut().find(|section| {
        section.kind == SectionKind::Code
            && address >= section.address
            && address < section.address + section.size
    })
}

fn decode_delta(
    endianness: Endianness,
    nibble: u8,
    data: &[u8],
    cursor: &mut usize,
    end: usize,
) -> Result<i32> {
    if nibble == 8 {
        ensure!(*cursor + 2 <= end, "extended delta out of range");
        let bytes: [u8; 2] = data[*cursor..*cursor + 2].try_into().unwrap();
        *cursor += 2;
        Ok(match endianness {
            Endianness::Big => i16::from_be_bytes(bytes) as i32,
            Endianness::Little => i16::from_le_bytes(bytes) as i32,
        })
    } else {
        let mut value = (nibble & 0x0f) as i32;
        if value & 0x8 != 0 {
            value -= 0x10;
        }
        Ok(value)
    }
}

fn slice_at(
    data: &[u8],
    offset: u32,
    size: u32,
    section_file_offset: Option<usize>,
) -> Result<&[u8]> {
    let size = size as usize;
    if size == 0 {
        ensure!(
            resolve_offset(offset, data.len(), section_file_offset).is_some(),
            "offset outside of .mdebug section"
        );
        return Ok(&data[0..0]);
    }
    let Some(offset) = resolve_offset(offset, data.len(), section_file_offset) else {
        bail!("offset outside of .mdebug section");
    };
    let end = offset.checked_add(size).context("range overflow")?;
    ensure!(end <= data.len(), "range exceeds .mdebug size");
    Ok(&data[offset..end])
}

fn resolve_offset(
    offset: u32,
    data_len: usize,
    section_file_offset: Option<usize>,
) -> Option<usize> {
    let offset = offset as usize;
    if offset <= data_len {
        Some(offset)
    } else if let Some(file_offset) = section_file_offset {
        offset.checked_sub(file_offset).filter(|rel| *rel <= data_len)
    } else {
        None
    }
}

#[derive(Clone, Copy)]
struct Header {
    cb_line_offset: u32,
    cb_pd_offset: u32,
    cb_sym_offset: u32,
    cb_fd_offset: u32,
    isym_max: u32,
    ifd_max: u32,
}

impl Header {
    fn parse(data: &[u8], endianness: Endianness) -> Result<Self> {
        ensure!(HDRR_SIZE <= data.len(), ".mdebug header truncated");
        let mut cursor = 0;
        let magic = read_u16(data, &mut cursor, endianness)?;
        let _vstamp = read_u16(data, &mut cursor, endianness)?;
        ensure!(magic == 0x7009, "unexpected .mdebug magic: {magic:#x}");
        let _iline_max = read_u32(data, &mut cursor, endianness)?;
        let _cb_line = read_u32(data, &mut cursor, endianness)?;
        let cb_line_offset = read_u32(data, &mut cursor, endianness)?;
        let _idn_max = read_u32(data, &mut cursor, endianness)?;
        let _cb_dn_offset = read_u32(data, &mut cursor, endianness)?;
        let _ipd_max = read_u32(data, &mut cursor, endianness)?;
        let cb_pd_offset = read_u32(data, &mut cursor, endianness)?;
        let isym_max = read_u32(data, &mut cursor, endianness)?;
        let cb_sym_offset = read_u32(data, &mut cursor, endianness)?;
        let _iopt_max = read_u32(data, &mut cursor, endianness)?;
        let _cb_opt_offset = read_u32(data, &mut cursor, endianness)?;
        let _iaux_max = read_u32(data, &mut cursor, endianness)?;
        let _cb_aux_offset = read_u32(data, &mut cursor, endianness)?;
        let _iss_max = read_u32(data, &mut cursor, endianness)?;
        let _cb_ss_offset = read_u32(data, &mut cursor, endianness)?;
        let _iss_ext_max = read_u32(data, &mut cursor, endianness)?;
        let _cb_ss_ext_offset = read_u32(data, &mut cursor, endianness)?;
        let ifd_max = read_u32(data, &mut cursor, endianness)?;
        let cb_fd_offset = read_u32(data, &mut cursor, endianness)?;
        let _crfd = read_u32(data, &mut cursor, endianness)?;
        let _cb_rfd_offset = read_u32(data, &mut cursor, endianness)?;
        let _iext_max = read_u32(data, &mut cursor, endianness)?;
        let _cb_ext_offset = read_u32(data, &mut cursor, endianness)?;

        Ok(Header { cb_line_offset, cb_pd_offset, cb_sym_offset, cb_fd_offset, isym_max, ifd_max })
    }
}

#[derive(Clone, Copy)]
struct FileDescriptor {
    adr: u32,
    isym_base: u32,
    csym: u32,
    ipd_first: u16,
    cpd: u16,
    cb_line_offset: u32,
    cb_line: u32,
}

impl FileDescriptor {
    fn parse(data: &[u8], endianness: Endianness) -> Result<Self> {
        ensure!(data.len() >= FDR_SIZE, "FDR truncated");
        let mut cursor = 0;
        let adr = read_u32(data, &mut cursor, endianness)?;
        let _rss = read_u32(data, &mut cursor, endianness)?;
        let _iss_base = read_u32(data, &mut cursor, endianness)?;
        let _cb_ss = read_u32(data, &mut cursor, endianness)?;
        let isym_base = read_u32(data, &mut cursor, endianness)?;
        let csym = read_u32(data, &mut cursor, endianness)?;
        let _iline_base = read_u32(data, &mut cursor, endianness)?;
        let _cline = read_u32(data, &mut cursor, endianness)?;
        let _iopt_base = read_u32(data, &mut cursor, endianness)?;
        let _copt = read_u32(data, &mut cursor, endianness)?;
        let ipd_first = read_u16(data, &mut cursor, endianness)?;
        let cpd = read_u16(data, &mut cursor, endianness)?;
        let _iaux_base = read_u32(data, &mut cursor, endianness)?;
        let _caux = read_u32(data, &mut cursor, endianness)?;
        let _rfd_base = read_u32(data, &mut cursor, endianness)?;
        let _crfd = read_u32(data, &mut cursor, endianness)?;
        let _bits = read_u32(data, &mut cursor, endianness)?;
        let cb_line_offset = read_u32(data, &mut cursor, endianness)?;
        let cb_line = read_u32(data, &mut cursor, endianness)?;

        Ok(FileDescriptor { adr, isym_base, csym, ipd_first, cpd, cb_line_offset, cb_line })
    }
}

#[derive(Clone, Copy)]
struct ProcDescriptor {
    addr: u32,
    isym: u32,
    ln_low: i32,
    cb_line_offset: u32,
}

impl ProcDescriptor {
    fn parse(data: &[u8], endianness: Endianness) -> Result<Self> {
        ensure!(data.len() >= PDR_SIZE, "PDR truncated");
        let mut cursor = 0;
        let addr = read_u32(data, &mut cursor, endianness)?;
        let isym = read_u32(data, &mut cursor, endianness)?;
        let _iline = read_u32(data, &mut cursor, endianness)?;
        let _regmask = read_u32(data, &mut cursor, endianness)?;
        let _regoffset = read_u32(data, &mut cursor, endianness)?;
        let _iopt = read_u32(data, &mut cursor, endianness)?;
        let _fregmask = read_u32(data, &mut cursor, endianness)?;
        let _fregoffset = read_u32(data, &mut cursor, endianness)?;
        let _frameoffset = read_u32(data, &mut cursor, endianness)?;
        let _framereg = read_u16(data, &mut cursor, endianness)?;
        let _pcreg = read_u16(data, &mut cursor, endianness)?;
        let ln_low = read_i32(data, &mut cursor, endianness)?;
        let _ln_high = read_i32(data, &mut cursor, endianness)?;
        let cb_line_offset = read_u32(data, &mut cursor, endianness)?;

        Ok(ProcDescriptor { addr, isym, ln_low, cb_line_offset })
    }
}

#[derive(Clone, Copy)]
struct SymbolEntry {
    value: u32,
    st: u8,
    index: u32,
}

fn parse_symbols(data: &[u8], endianness: Endianness) -> Result<Vec<SymbolEntry>> {
    ensure!(data.len().is_multiple_of(SYMR_SIZE), "symbol table misaligned");
    let mut symbols = Vec::with_capacity(data.len() / SYMR_SIZE);
    let mut cursor = 0;
    while cursor + SYMR_SIZE <= data.len() {
        let _iss = read_u32(data, &mut cursor, endianness)?;
        let value = read_u32(data, &mut cursor, endianness)?;
        let bits = read_u32(data, &mut cursor, endianness)?;
        let (st, index) = match endianness {
            Endianness::Big => (((bits >> 26) & 0x3f) as u8, bits & 0x000f_ffff),
            Endianness::Little => (((bits & 0x3f) as u8), (bits >> 12) & 0x000f_ffff),
        };
        symbols.push(SymbolEntry { value, st, index });
    }
    Ok(symbols)
}

fn read_u16(data: &[u8], cursor: &mut usize, endianness: Endianness) -> Result<u16> {
    ensure!(*cursor + 2 <= data.len(), "unexpected EOF while reading u16");
    let bytes: [u8; 2] = data[*cursor..*cursor + 2].try_into().unwrap();
    *cursor += 2;
    Ok(match endianness {
        Endianness::Big => u16::from_be_bytes(bytes),
        Endianness::Little => u16::from_le_bytes(bytes),
    })
}

fn read_u32(data: &[u8], cursor: &mut usize, endianness: Endianness) -> Result<u32> {
    ensure!(*cursor + 4 <= data.len(), "unexpected EOF while reading u32");
    let bytes: [u8; 4] = data[*cursor..*cursor + 4].try_into().unwrap();
    *cursor += 4;
    Ok(match endianness {
        Endianness::Big => u32::from_be_bytes(bytes),
        Endianness::Little => u32::from_le_bytes(bytes),
    })
}

fn read_i32(data: &[u8], cursor: &mut usize, endianness: Endianness) -> Result<i32> {
    Ok(read_u32(data, cursor, endianness)? as i32)
}
