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
        let mut next = match tiff.uint_at(2, 2)? as u16 {
            42 => tiff.uint_at(4, 4)?,
            43 => {
                tiff.bigtiff = true;
                let offset_size = tiff.uint_at(4, 2)? as u16;
                if offset_size != 8 || tiff.uint_at(6, 2)? != 0 {
                    return Err(TiffError::BadOffsetSize(offset_size));
                }
                tiff.uint_at(8, 8)?
            }
            m => return Err(TiffError::BadMagic(m)),
        };
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

    /// The value of an entry as raw bytes, following the offset if not inline.
    pub fn value_bytes<'b>(&'b self, e: &'b Entry) -> Result<&'b [u8]> {
        let size = type_size(e.ty).ok_or(TiffError::UnsupportedType { tag: e.tag, ty: e.ty })?;
        let total = usize::try_from(e.count)
            .ok()
            .and_then(|c| c.checked_mul(size))
            .ok_or(TiffError::Truncated)?;
        let inline = if self.bigtiff { 8 } else { 4 };
        if total <= inline {
            Ok(&e.raw[..total])
        } else {
            let offset = self.read_uint(&e.raw[..inline]);
            self.bytes_at(offset, total)
        }
    }

    /// All values of an unsigned integer entry (BYTE, SHORT, LONG, or LONG8).
    pub fn uints(&self, e: &Entry) -> Result<Vec<u64>> {
        use field_type::*;
        if !matches!(e.ty, BYTE | SHORT | LONG | LONG8) {
            return Err(TiffError::UnsupportedType { tag: e.tag, ty: e.ty });
        }
        let size = type_size(e.ty).unwrap();
        let bytes = self.value_bytes(e)?;
        Ok(bytes.chunks_exact(size).map(|c| self.read_uint(c)).collect())
    }

    /// The single value of an unsigned integer entry.
    pub fn uint(&self, e: &Entry) -> Result<u64> {
        self.uints(e)?.first().copied().ok_or(TiffError::Truncated)
    }

    /// An ASCII entry as a string, with trailing NULs removed.
    pub fn ascii<'b>(&'b self, e: &'b Entry) -> Result<&'b str> {
        if e.ty != field_type::ASCII {
            return Err(TiffError::UnsupportedType { tag: e.tag, ty: e.ty });
        }
        let bytes = self.value_bytes(e)?;
        let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        std::str::from_utf8(&bytes[..end]).map_err(|_| TiffError::NotAscii(e.tag))
    }

    fn parse_ifd(&self, offset: u64) -> Result<(Ifd, u64)> {
        let (count, mut pos, entry_size) = if self.bigtiff {
            (self.uint_at(offset, 8)?, offset + 8, 20)
        } else {
            (self.uint_at(offset, 2)?, offset + 2, 12)
        };
        let mut entries = Vec::with_capacity(count.min(4096) as usize);
        for _ in 0..count {
            entries.push(self.parse_entry(pos)?);
            pos += entry_size;
        }
        let next = self.uint_at(pos, if self.bigtiff { 8 } else { 4 })?;
        Ok((Ifd { offset, entries }, next))
    }

    fn parse_entry(&self, pos: u64) -> Result<Entry> {
        let tag = self.uint_at(pos, 2)? as u16;
        let ty = self.uint_at(pos + 2, 2)? as u16;
        let (count, value_pos, value_len) = if self.bigtiff {
            (self.uint_at(pos + 4, 8)?, pos + 12, 8)
        } else {
            (self.uint_at(pos + 4, 4)?, pos + 8, 4)
        };
        let mut raw = [0u8; 8];
        raw[..value_len].copy_from_slice(self.bytes_at(value_pos, value_len)?);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn load(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), name)).unwrap()
    }

    #[test]
    fn parses_smoke_svs() {
        let data = load("smoke.svs");
        let tiff = Tiff::parse(&data).unwrap();

        assert_eq!(tiff.order, ByteOrder::Little);
        assert!(!tiff.bigtiff);
        assert_eq!(tiff.ifds.len(), 1);

        let ifd = &tiff.ifds[0];
        assert_eq!(ifd.entries.len(), 12);

        let get = |tag| ifd.get(tag).unwrap();
        assert_eq!(tiff.uint(get(tags::IMAGE_WIDTH)).unwrap(), 512);
        assert_eq!(tiff.uint(get(tags::IMAGE_LENGTH)).unwrap(), 512);
        assert_eq!(tiff.uints(get(tags::BITS_PER_SAMPLE)).unwrap(), [8, 8, 8]);
        assert_eq!(tiff.uint(get(tags::COMPRESSION)).unwrap(), 7);
        assert_eq!(tiff.uint(get(tags::PHOTOMETRIC_INTERPRETATION)).unwrap(), 6);
        assert_eq!(tiff.uint(get(tags::SAMPLES_PER_PIXEL)).unwrap(), 3);
        assert_eq!(tiff.uint(get(tags::TILE_WIDTH)).unwrap(), 256);
        assert_eq!(tiff.uint(get(tags::TILE_LENGTH)).unwrap(), 256);
        assert_eq!(
            tiff.ascii(get(tags::IMAGE_DESCRIPTION)).unwrap(),
            "Aperio|AppMag = 20|MPP = 0.4990|"
        );
        assert_eq!(
            tiff.uints(get(tags::TILE_OFFSETS)).unwrap(),
            [239, 243, 247, 251]
        );
        assert_eq!(tiff.uints(get(tags::TILE_BYTE_COUNTS)).unwrap(), [4, 4, 4, 4]);

        let tables = tiff.value_bytes(get(tags::JPEG_TABLES)).unwrap();
        assert_eq!(&tables[..2], b"\xff\xd8");
        assert_eq!(&tables[8..], b"\xff\xd9");
    }

    #[test]
    fn reads_tile_data() {
        let data = load("smoke.svs");
        let tiff = Tiff::parse(&data).unwrap();
        let ifd = &tiff.ifds[0];
        let offsets = tiff.uints(ifd.get(tags::TILE_OFFSETS).unwrap()).unwrap();
        let counts = tiff.uints(ifd.get(tags::TILE_BYTE_COUNTS).unwrap()).unwrap();
        for (off, count) in offsets.iter().zip(&counts) {
            let tile = &data[*off as usize..(*off + *count) as usize];
            assert_eq!(tile, b"ZZZZ");
        }
    }

    #[test]
    fn parses_test_tags_svs() {
        let data = load("test_tags.svs");
        let tiff = Tiff::parse(&data).unwrap();
        let ifd = &tiff.ifds[0];
        assert_eq!(
            tiff.ascii(ifd.get(tags::IMAGE_DESCRIPTION).unwrap()).unwrap(),
            "Aperio Fake|AppMag = 40|MPP = 0.2500|"
        );
        assert_eq!(
            tiff.uints(ifd.get(tags::TILE_OFFSETS).unwrap()).unwrap(),
            [244, 248, 252, 256]
        );
    }
}
