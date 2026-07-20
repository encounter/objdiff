use anyhow::{Result, anyhow};

use crate::util::{read_u8, read_u16, read_u32};

pub const COMMENT_SECTION: &str = ".comment";

const MAGIC: &[u8] = "CodeWarrior".as_bytes();
const HEADER_SIZE: u8 = 0x2C;

// Partial implementation, does not include all fields
pub struct MWComment {
    pub version: u8,
}

impl MWComment {
    pub fn from_reader(_obj_file: &object::File, reader: &mut &[u8]) -> Result<Self> {
        let mut out = MWComment { version: 0 };
        let magic: [u8; MAGIC.len()] = reader[..MAGIC.len()].try_into()?;
        *reader = &reader[MAGIC.len()..];
        if magic != MAGIC {
            return Err(anyhow!("Invalid .comment section magic: {magic:?}"));
        }
        out.version = read_u8(reader)?;
        if !matches!(out.version, 8 | 10 | 11 | 13 | 14 | 15) {
            return Err(anyhow!("Unknown .comment section version: {}", out.version));
        }
        *reader = &reader[8..];
        let header_size = read_u8(reader)?;
        if header_size != HEADER_SIZE {
            return Err(anyhow!("Expected header size {HEADER_SIZE:#X}, got {header_size:#X}"));
        }
        *reader = &reader[0x17..];
        Ok(out)
    }
}

#[derive(Debug, Copy, Clone)]
pub struct CommentSym {
    pub align: u32,
    pub vis_flags: u8,
    pub active_flags: u8,
}

impl CommentSym {
    pub fn from_reader(obj_file: &object::File, reader: &mut &[u8]) -> Result<Self> {
        let mut out = CommentSym { align: 0, vis_flags: 0, active_flags: 0 };
        out.align = read_u32(obj_file, reader)?;
        out.vis_flags = read_u8(reader)?;
        if !matches!(out.vis_flags, 0 | 0xD | 0xE) {
            log::warn!("Unknown vis_flags: {:#X}", out.vis_flags);
        }
        out.active_flags = read_u8(reader)?;
        if !matches!(out.active_flags, 0 | 0x8 | 0x10 | 0x20) {
            log::warn!("Unknown active_flags: {:#X}", out.active_flags);
        }
        let padding = read_u16(obj_file, reader)?;
        if padding != 0 {
            return Err(anyhow!("Unexpected value after active_flags: {padding:#X}"));
        }
        Ok(out)
    }
}
