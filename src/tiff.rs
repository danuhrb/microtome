use std::fmt;

pub mod tags {
    pub const IMAGE_WIDTH: u16 = 256;
    pub const IMAGE_LENGTH: u16 = 257;
    pub const BITS_PER_SAMPLE: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC_INTERPRETATION: u16 = 262;
    pub const IMAGE_DESCRIPTION: u16 = 270;
    pub const SAMPLES_PER_PIXEL: u16 = 277;
    pub const TILE_WIDTH: u16 = 322;
    pub const TILE_LENGTH: u16 = 323;
    pub const TILE_OFFSETS: u16 = 324;
    pub const TILE_BYTE_COUNTS: u16 = 325;
    pub const JPEG_TABLES: u16 = 347;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Big,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TiffError {
    Truncated,
    BadByteOrder([u8; 2]),
    BadMagic(u16),
    BadOffsetSize(u16),
    UnsupportedType { tag: u16, ty: u16 },
    NotAscii(u16),
    CircularIfd(u64),
}

impl fmt::Display for TiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "unexpected end of file"),
            Self::BadByteOrder(b) => write!(f, "bad byte-order mark {b:?}"),
            Self::BadMagic(m) => write!(f, "bad magic number {m}"),
            Self::BadOffsetSize(s) => write!(f, "unsupported bigtiff offset size {s}"),
            Self::UnsupportedType { tag, ty } => write!(f, "tag {tag}: unsupported field type {ty}"),
            Self::NotAscii(tag) => write!(f, "tag {tag}: value is not ascii"),
            Self::CircularIfd(off) => write!(f, "circular ifd chain at offset {off}"),
        }
    }
}

impl std::error::Error for TiffError {}
