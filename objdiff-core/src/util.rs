use alloc::{format, vec::Vec};
use core::fmt;

use anyhow::{Result, ensure};
use num_traits::PrimInt;
use object::{Endian, Object};

// https://stackoverflow.com/questions/44711012/how-do-i-format-a-signed-integer-to-a-sign-aware-hexadecimal-representation
pub struct ReallySigned<N: PrimInt>(pub N);

impl<N: PrimInt> fmt::LowerHex for ReallySigned<N> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let num = self.0.to_i64().unwrap();
        let prefix = if f.alternate() { "0x" } else { "" };
        let bare_hex = format!("{:x}", num.abs());
        f.pad_integral(num >= 0, prefix, &bare_hex)
    }
}

impl<N: PrimInt> fmt::UpperHex for ReallySigned<N> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let num = self.0.to_i64().unwrap();
        let prefix = if f.alternate() { "0x" } else { "" };
        let bare_hex = format!("{:X}", num.abs());
        f.pad_integral(num >= 0, prefix, &bare_hex)
    }
}

pub fn read_u32(obj_file: &object::File, reader: &mut &[u8]) -> Result<u32> {
    ensure!(reader.len() >= 4, "Not enough bytes to read u32");
    let value = u32::from_ne_bytes(reader[..4].try_into()?);
    *reader = &reader[4..];
    Ok(obj_file.endianness().read_u32(value))
}

pub fn read_u16(obj_file: &object::File, reader: &mut &[u8]) -> Result<u16> {
    ensure!(reader.len() >= 2, "Not enough bytes to read u16");
    let value = u16::from_ne_bytes(reader[..2].try_into()?);
    *reader = &reader[2..];
    Ok(obj_file.endianness().read_u16(value))
}

pub fn align_size_to_4(size: usize) -> usize { (size + 3) & !3 }

#[cfg(feature = "std")]
pub fn align_data_to_4<W: std::io::Write + ?Sized>(
    writer: &mut W,
    len: usize,
) -> std::io::Result<()> {
    const ALIGN_BYTES: &[u8] = &[0; 4];
    if len % 4 != 0 {
        writer.write_all(&ALIGN_BYTES[..4 - len % 4])?;
    }
    Ok(())
}

pub fn align_u64_to(len: u64, align: u64) -> u64 { len + ((align - (len % align)) % align) }

pub fn align_data_slice_to(data: &mut Vec<u8>, align: u64) {
    data.resize(align_u64_to(data.len() as u64, align) as usize, 0);
}
