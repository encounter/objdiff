use std::{
    io,
    io::{Read, Write},
};

use object::{elf::SHT_LOUSER, Endian};

pub const SPLITMETA_SECTION: &str = ".splitmeta";
// Use the same section type as .mwcats.* so the linker ignores it
pub const SHT_SPLITMETA: u32 = SHT_LOUSER + 0x4A2A82C2;

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

/**
 * .splitmeta section format:
 * - Magic: "SPMD"
 * - Section: Magic: 4 bytes, Data size: 4 bytes, Data: variable
 *     Section size can be used to skip unknown sections
 * - Repeat section until EOF
 * Endianness matches the object file
 *
 * Sections:
 * - Generator: Magic: "GENR", Data size: 4 bytes, Data: UTF-8 string (no null terminator)
 * - Virtual addresses: Magic: "VIRT", Data size: 4 bytes, Data: array
 *     Data is u32 array for 32-bit objects, u64 array for 64-bit objects
 *     Count is size / 4 (32-bit) or size / 8 (64-bit)
 */

const SPLIT_META_MAGIC: [u8; 4] = *b"SPMD";
const GENERATOR_MAGIC: [u8; 4] = *b"GENR";
const MODULE_NAME_MAGIC: [u8; 4] = *b"MODN";
const MODULE_ID_MAGIC: [u8; 4] = *b"MODI";
const VIRTUAL_ADDRESS_MAGIC: [u8; 4] = *b"VIRT";

impl SplitMeta {
    pub fn from_reader<E, R>(reader: &mut R, e: E, is_64: bool) -> io::Result<Self>
    where
        E: Endian,
        R: Read + ?Sized,
    {
        let mut magic = [0; 4];
        reader.read_exact(&mut magic)?;
        if magic != SPLIT_META_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid split metadata magic"));
        }
        let mut result = SplitMeta::default();
        loop {
            let mut magic = [0; 4];
            match reader.read_exact(&mut magic) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            };
            let mut size_bytes = [0; 4];
            reader.read_exact(&mut size_bytes)?;
            let size = e.read_u32_bytes(size_bytes);
            let mut data = vec![0; size as usize];
            reader.read_exact(&mut data)?;
            match magic {
                GENERATOR_MAGIC => {
                    let string = String::from_utf8(data)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    result.generator = Some(string);
                }
                MODULE_NAME_MAGIC => {
                    let string = String::from_utf8(data)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    result.module_name = Some(string);
                }
                MODULE_ID_MAGIC => {
                    let id = e.read_u32_bytes(data.as_slice().try_into().map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "Invalid module ID size")
                    })?);
                    result.module_id = Some(id);
                }
                VIRTUAL_ADDRESS_MAGIC => {
                    let vec = if is_64 {
                        let mut vec = vec![0u64; data.len() / 8];
                        for i in 0..vec.len() {
                            vec[i] = e.read_u64_bytes(data[i * 8..(i + 1) * 8].try_into().unwrap());
                        }
                        vec
                    } else {
                        let mut vec = vec![0u64; data.len() / 4];
                        for i in 0..vec.len() {
                            vec[i] = e.read_u32_bytes(data[i * 4..(i + 1) * 4].try_into().unwrap())
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

    pub fn to_writer<E, W>(&self, writer: &mut W, e: E, is_64: bool) -> io::Result<()>
    where
        E: Endian,
        W: Write + ?Sized,
    {
        writer.write_all(&SPLIT_META_MAGIC)?;
        if let Some(generator) = &self.generator {
            writer.write_all(&GENERATOR_MAGIC)?;
            writer.write_all(&e.write_u32_bytes(generator.len() as u32))?;
            writer.write_all(generator.as_bytes())?;
        }
        if let Some(module_name) = &self.module_name {
            writer.write_all(&MODULE_NAME_MAGIC)?;
            writer.write_all(&e.write_u32_bytes(module_name.len() as u32))?;
            writer.write_all(module_name.as_bytes())?;
        }
        if let Some(module_id) = self.module_id {
            writer.write_all(&MODULE_ID_MAGIC)?;
            writer.write_all(&e.write_u32_bytes(4))?;
            writer.write_all(&e.write_u32_bytes(module_id))?;
        }
        if let Some(virtual_addresses) = &self.virtual_addresses {
            writer.write_all(&VIRTUAL_ADDRESS_MAGIC)?;
            let count = virtual_addresses.len() as u32;
            if is_64 {
                writer.write_all(&e.write_u32_bytes(count * 8))?;
                for &addr in virtual_addresses {
                    writer.write_all(&e.write_u64_bytes(addr))?;
                }
            } else {
                writer.write_all(&e.write_u32_bytes(count * 4))?;
                for &addr in virtual_addresses {
                    writer.write_all(&e.write_u32_bytes(addr as u32))?;
                }
            }
        }
        Ok(())
    }

    pub fn write_size(&self, is_64: bool) -> usize {
        let mut size = 4;
        if let Some(generator) = self.generator.as_deref() {
            size += 8 + generator.len();
        }
        if let Some(module_name) = self.module_name.as_deref() {
            size += 8 + module_name.len();
        }
        if self.module_id.is_some() {
            size += 12;
        }
        if let Some(virtual_addresses) = self.virtual_addresses.as_deref() {
            size += 8 + if is_64 { 8 } else { 4 } * virtual_addresses.len();
        }
        size
    }
}
