use std::collections::HashSet;
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

type Result<T> = std::result::Result<T, TiffError>;

pub mod field_type {
    pub const BYTE: u16 = 1;
    pub const ASCII: u16 = 2;
    pub const SHORT: u16 = 3;
    pub const LONG: u16 = 4;
    pub const UNDEFINED: u16 = 7;
    pub const LONG8: u16 = 16;
}

fn type_size(ty: u16) -> Option<usize> {
    Some(match ty {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub tag: u16,
    pub ty: u16,
    pub count: u64,
    /// Inline value or offset field, as raw bytes (4 used in classic, 8 in bigtiff).
    raw: [u8; 8],
}

#[derive(Debug)]
pub struct Ifd {
    pub offset: u64,
    pub entries: Vec<Entry>,
}

impl Ifd {
    pub fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

#[derive(Debug)]
pub struct Tiff<'a> {
    data: &'a [u8],
    pub order: ByteOrder,
    pub bigtiff: bool,
    pub ifds: Vec<Ifd>,
}

impl<'a> Tiff<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let order = match data.get(0..2) {
            Some(b"II") => ByteOrder::Little,
            Some(b"MM") => ByteOrder::Big,
            Some(o) => return Err(TiffError::BadByteOrder([o[0], o[1]])),
            None => return Err(TiffError::Truncated),
        };
        let mut tiff = Tiff {
            data,
            order,
            bigtiff: false,
            ifds: Vec::new(),
        };
        let magic = tiff.uint_at(2, 2)? as u16;
        if magic != 42 {
            return Err(TiffError::BadMagic(magic));
        }
        let mut next = tiff.uint_at(4, 4)?;
        let mut seen = HashSet::new();
        while next != 0 {
            if !seen.insert(next) {
                return Err(TiffError::CircularIfd(next));
            }
            let (ifd, n) = tiff.parse_ifd(next)?;
            tiff.ifds.push(ifd);
            next = n;
        }
        Ok(tiff)
    }

    fn parse_ifd(&self, offset: u64) -> Result<(Ifd, u64)> {
        let count = self.uint_at(offset, 2)?;
        let mut pos = offset + 2;
        let mut entries = Vec::with_capacity(count.min(4096) as usize);
        for _ in 0..count {
            entries.push(self.parse_entry(pos)?);
            pos += 12;
        }
        let next = self.uint_at(pos, 4)?;
        Ok((Ifd { offset, entries }, next))
    }

    fn parse_entry(&self, pos: u64) -> Result<Entry> {
        let tag = self.uint_at(pos, 2)? as u16;
        let ty = self.uint_at(pos + 2, 2)? as u16;
        let count = self.uint_at(pos + 4, 4)?;
        let mut raw = [0u8; 8];
        raw[..4].copy_from_slice(self.bytes_at(pos + 8, 4)?);
        Ok(Entry { tag, ty, count, raw })
    }

    fn bytes_at(&self, offset: u64, len: usize) -> Result<&'a [u8]> {
        usize::try_from(offset)
            .ok()
            .and_then(|start| start.checked_add(len).map(|end| (start, end)))
            .and_then(|(start, end)| self.data.get(start..end))
            .ok_or(TiffError::Truncated)
    }

    fn uint_at(&self, offset: u64, len: usize) -> Result<u64> {
        Ok(self.read_uint(self.bytes_at(offset, len)?))
    }

    fn read_uint(&self, bytes: &[u8]) -> u64 {
        let fold = |acc: u64, b: &u8| (acc << 8) | u64::from(*b);
        match self.order {
            ByteOrder::Little => bytes.iter().rev().fold(0, fold),
            ByteOrder::Big => bytes.iter().fold(0, fold),
        }
    }
}
