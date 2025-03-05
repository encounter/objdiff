use alloc::{string::String, vec, vec::Vec};

use anyhow::{Result, anyhow};
use object::{Endian, ObjectSection, elf::SHT_NOTE};

pub const SPLITMETA_SECTION: &str = ".note.split";
pub const SHT_SPLITMETA: u32 = SHT_NOTE;
pub const ELF_NOTE_SPLIT: &[u8] = b"Split";

/// This is used to store metadata about the source of an object file,
/// such as the original virtual addresses and the tool that wrote it.
#[derive(Debug, Default, Clone)]
pub struct SplitMeta {
    /// The tool that generated the object. Informational only.
    pub generator: Option<String>,
    /// The name of the source module. (e.g. the DOL or REL name)
    pub module_name: Option<String>,
    /// The ID of the source module. (e.g. the DOL or REL ID)
    pub module_id: Option<u32>,
    /// Original virtual addresses of each symbol in the object.
    /// Index 0 is the ELF null symbol.
    pub virtual_addresses: Option<Vec<u64>>,
}

const NT_SPLIT_GENERATOR: u32 = u32::from_be_bytes(*b"GENR");
const NT_SPLIT_MODULE_NAME: u32 = u32::from_be_bytes(*b"MODN");
const NT_SPLIT_MODULE_ID: u32 = u32::from_be_bytes(*b"MODI");
const NT_SPLIT_VIRTUAL_ADDRESSES: u32 = u32::from_be_bytes(*b"VIRT");

impl SplitMeta {
    pub fn from_section<E>(section: object::Section, e: E, is_64: bool) -> Result<Self>
    where E: Endian {
        let mut result = SplitMeta::default();
        let data = section.uncompressed_data().map_err(object_error)?;
        let mut iter = NoteIterator::new(data.as_ref(), section.align(), e, is_64)?;
        while let Some(note) = iter.next(e)? {
            if note.name != ELF_NOTE_SPLIT {
                continue;
            }
            match note.n_type {
                NT_SPLIT_GENERATOR => {
                    let string =
                        String::from_utf8(note.desc.to_vec()).map_err(anyhow::Error::new)?;
                    result.generator = Some(string);
                }
                NT_SPLIT_MODULE_NAME => {
                    let string =
                        String::from_utf8(note.desc.to_vec()).map_err(anyhow::Error::new)?;
                    result.module_name = Some(string);
                }
                NT_SPLIT_MODULE_ID => {
                    result.module_id = Some(e.read_u32_bytes(
                        note.desc.try_into().map_err(|_| anyhow!("Invalid module ID size"))?,
                    ));
                }
                NT_SPLIT_VIRTUAL_ADDRESSES => {
                    let vec = if is_64 {
                        let mut vec = vec![0u64; note.desc.len() / 8];
                        for (i, v) in vec.iter_mut().enumerate() {
                            *v =
                                e.read_u64_bytes(note.desc[i * 8..(i + 1) * 8].try_into().unwrap());
                        }
                        vec
                    } else {
                        let mut vec = vec![0u64; note.desc.len() / 4];
                        for (i, v) in vec.iter_mut().enumerate() {
                            *v = e.read_u32_bytes(note.desc[i * 4..(i + 1) * 4].try_into().unwrap())
                                as u64;
                        }
                        vec
                    };
                    result.virtual_addresses = Some(vec);
                }
                _ => {
                    // Ignore unknown sections
                }
            }
        }
        Ok(result)
    }

    #[cfg(feature = "std")]
    pub fn to_writer<E, W>(&self, writer: &mut W, e: E, is_64: bool) -> std::io::Result<()>
    where
        E: Endian,
        W: std::io::Write + ?Sized,
    {
        if let Some(generator) = &self.generator {
            write_note_header(writer, e, NT_SPLIT_GENERATOR, generator.len())?;
            writer.write_all(generator.as_bytes())?;
            align_data_to_4(writer, generator.len())?;
        }
        if let Some(module_name) = &self.module_name {
            write_note_header(writer, e, NT_SPLIT_MODULE_NAME, module_name.len())?;
            writer.write_all(module_name.as_bytes())?;
            align_data_to_4(writer, module_name.len())?;
        }
        if let Some(module_id) = self.module_id {
            write_note_header(writer, e, NT_SPLIT_MODULE_ID, 4)?;
            writer.write_all(&e.write_u32_bytes(module_id))?;
        }
        if let Some(virtual_addresses) = &self.virtual_addresses {
            let count = virtual_addresses.len();
            let size = if is_64 { count * 8 } else { count * 4 };
            write_note_header(writer, e, NT_SPLIT_VIRTUAL_ADDRESSES, size)?;
            if is_64 {
                for &addr in virtual_addresses {
                    writer.write_all(&e.write_u64_bytes(addr))?;
                }
            } else {
                for &addr in virtual_addresses {
                    writer.write_all(&e.write_u32_bytes(addr as u32))?;
                }
            }
        }
        Ok(())
    }

    pub fn write_size(&self, is_64: bool) -> usize {
        let mut size = 0;
        if let Some(generator) = self.generator.as_deref() {
            size += NOTE_HEADER_SIZE + generator.len();
            size = align_size_to_4(size);
        }
        if let Some(module_name) = self.module_name.as_deref() {
            size += NOTE_HEADER_SIZE + module_name.len();
            size = align_size_to_4(size);
        }
        if self.module_id.is_some() {
            size += NOTE_HEADER_SIZE + 4;
            size = align_size_to_4(size);
        }
        if let Some(virtual_addresses) = self.virtual_addresses.as_deref() {
            size += NOTE_HEADER_SIZE + if is_64 { 8 } else { 4 } * virtual_addresses.len();
            size = align_size_to_4(size);
        }
        size
    }
}

/// Convert an object::read::Error to a String.
#[inline]
fn object_error(err: object::read::Error) -> anyhow::Error { anyhow::Error::new(err) }

/// An ELF note entry.
struct Note<'data> {
    n_type: u32,
    name: &'data [u8],
    desc: &'data [u8],
}

/// object::read::elf::NoteIterator is awkward to use generically,
/// so wrap it in our own iterator.
enum NoteIterator<'data, E>
where E: Endian
{
    B32(object::read::elf::NoteIterator<'data, object::elf::FileHeader32<E>>),
    B64(object::read::elf::NoteIterator<'data, object::elf::FileHeader64<E>>),
}

impl<'data, E> NoteIterator<'data, E>
where E: Endian
{
    fn new(data: &'data [u8], align: u64, e: E, is_64: bool) -> Result<Self> {
        Ok(if is_64 {
            NoteIterator::B64(
                object::read::elf::NoteIterator::new(e, align, data).map_err(object_error)?,
            )
        } else {
            NoteIterator::B32(
                object::read::elf::NoteIterator::new(e, align as u32, data)
                    .map_err(object_error)?,
            )
        })
    }

    fn next(&mut self, e: E) -> Result<Option<Note<'data>>> {
        match self {
            NoteIterator::B32(iter) => Ok(iter.next().map_err(object_error)?.map(|note| Note {
                n_type: note.n_type(e),
                name: note.name(),
                desc: note.desc(),
            })),
            NoteIterator::B64(iter) => Ok(iter.next().map_err(object_error)?.map(|note| Note {
                n_type: note.n_type(e),
                name: note.name(),
                desc: note.desc(),
            })),
        }
    }
}

fn align_size_to_4(size: usize) -> usize { (size + 3) & !3 }

#[cfg(feature = "std")]
fn align_data_to_4<W: std::io::Write + ?Sized>(writer: &mut W, len: usize) -> std::io::Result<()> {
    const ALIGN_BYTES: &[u8] = &[0; 4];
    if len % 4 != 0 {
        writer.write_all(&ALIGN_BYTES[..4 - len % 4])?;
    }
    Ok(())
}

// ELF note format:
// Name Size | 4 bytes (integer)
// Desc Size | 4 bytes (integer)
// Type | 4 bytes (usually interpreted as an integer)
// Name | variable size, padded to a 4 byte boundary
// Desc | variable size, padded to a 4 byte boundary
const NOTE_HEADER_SIZE: usize = 12 + ((ELF_NOTE_SPLIT.len() + 4) & !3);

#[cfg(feature = "std")]
fn write_note_header<E, W>(writer: &mut W, e: E, kind: u32, desc_len: usize) -> std::io::Result<()>
where
    E: Endian,
    W: std::io::Write + ?Sized,
{
    writer.write_all(&e.write_u32_bytes(ELF_NOTE_SPLIT.len() as u32 + 1))?; // Name Size
    writer.write_all(&e.write_u32_bytes(desc_len as u32))?; // Desc Size
    writer.write_all(&e.write_u32_bytes(kind))?; // Type
    writer.write_all(ELF_NOTE_SPLIT)?; // Name
    writer.write_all(&[0; 1])?; // Null terminator
    align_data_to_4(writer, ELF_NOTE_SPLIT.len() + 1)?;
    Ok(())
}
