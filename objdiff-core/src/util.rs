use std::{
    fmt::{LowerHex, UpperHex},
    io::Read,
};

use anyhow::Result;
use byteorder::{NativeEndian, ReadBytesExt};
use num_traits::PrimInt;
use object::{Endian, Object};

// https://stackoverflow.com/questions/44711012/how-do-i-format-a-signed-integer-to-a-sign-aware-hexadecimal-representation
pub(crate) struct ReallySigned<N: PrimInt>(pub(crate) N);

impl<N: PrimInt> LowerHex for ReallySigned<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let num = self.0.to_i64().unwrap();
        let prefix = if f.alternate() { "0x" } else { "" };
        let bare_hex = format!("{:x}", num.abs());
        f.pad_integral(num >= 0, prefix, &bare_hex)
    }
}

impl<N: PrimInt> UpperHex for ReallySigned<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let num = self.0.to_i64().unwrap();
        let prefix = if f.alternate() { "0x" } else { "" };
        let bare_hex = format!("{:X}", num.abs());
        f.pad_integral(num >= 0, prefix, &bare_hex)
    }
}

pub fn read_u32<R: Read>(obj_file: &object::File, reader: &mut R) -> Result<u32> {
    Ok(obj_file.endianness().read_u32(reader.read_u32::<NativeEndian>()?))
}

pub fn read_u16<R: Read>(obj_file: &object::File, reader: &mut R) -> Result<u16> {
    Ok(obj_file.endianness().read_u16(reader.read_u16::<NativeEndian>()?))
}
