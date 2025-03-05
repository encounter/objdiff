use alloc::format;
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
