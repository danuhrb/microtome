//! Aperio SVS: slide metadata and pyramid discovery on top of the TIFF layer.

use crate::tiff::{tags, Entry, Ifd, Tiff, TiffError};
use std::fmt;

#[derive(Debug, PartialEq)]
pub enum SvsError {
    Tiff(TiffError),
    NotAperio,
    NoLevels,
    MissingTag(u16),
    TileCountMismatch { offsets: usize, counts: usize },
}

impl fmt::Display for SvsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tiff(e) => write!(f, "tiff error: {e}"),
            Self::NotAperio => write!(f, "image description is not an Aperio header"),
            Self::NoLevels => write!(f, "no tiled pyramid levels found"),
            Self::MissingTag(tag) => write!(f, "required tag {tag} is missing"),
            Self::TileCountMismatch { offsets, counts } => {
                write!(f, "{offsets} tile offsets but {counts} byte counts")
            }
        }
    }
}

impl std::error::Error for SvsError {}

impl From<TiffError> for SvsError {
    fn from(e: TiffError) -> Self {
        Self::Tiff(e)
    }
}

type Result<T> = std::result::Result<T, SvsError>;

#[derive(Debug)]
pub struct Level {
    pub ifd_index: usize,
    pub width: u64,
    pub height: u64,
    pub tile_width: u64,
    pub tile_height: u64,
    pub compression: u64,
    pub photometric: u64,
    /// Resolution relative to the base level (1.0 for the base itself).
    pub downsample: f64,
    pub mpp: Option<f64>,
    /// (offset, byte count) of each tile in the file, row-major.
    pub tiles: Vec<(u64, u64)>,
}

impl Level {
    pub fn tiles_across(&self) -> u64 {
        self.width.div_ceil(self.tile_width)
    }

    pub fn tiles_down(&self) -> u64 {
        self.height.div_ceil(self.tile_height)
    }
}

#[derive(Debug)]
pub struct Slide<'a> {
    pub tiff: Tiff<'a>,
    /// Pyramid levels, largest first.
    pub levels: Vec<Level>,
    pub description: String,
    pub mpp: Option<f64>,
    pub magnification: Option<f64>,
}

impl<'a> Slide<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let tiff = Tiff::parse(data)?;
        let first = tiff.ifds.first().ok_or(SvsError::NoLevels)?;
        let desc_entry = first
            .get(tags::IMAGE_DESCRIPTION)
            .ok_or(SvsError::NotAperio)?;
        let description = tiff.ascii(desc_entry)?.to_owned();
        if !description.contains("Aperio") {
            return Err(SvsError::NotAperio);
        }
        let mpp = field(&description, "MPP");
        let magnification = field(&description, "AppMag");

        let mut levels = Vec::new();
        for (i, ifd) in tiff.ifds.iter().enumerate() {
            if ifd.get(tags::TILE_WIDTH).is_none() {
                continue; // stripped image: thumbnail, label, or macro
            }
            if let Some(e) = ifd.get(tags::IMAGE_DESCRIPTION) {
                if let Ok(d) = tiff.ascii(e) {
                    if d.contains("label") || d.contains("macro") {
                        continue;
                    }
                }
            }
            let offsets = tiff.uints(req(ifd, tags::TILE_OFFSETS)?)?;
            let counts = tiff.uints(req(ifd, tags::TILE_BYTE_COUNTS)?)?;
            if offsets.len() != counts.len() {
                return Err(SvsError::TileCountMismatch {
                    offsets: offsets.len(),
                    counts: counts.len(),
                });
            }
            levels.push(Level {
                ifd_index: i,
                width: tiff.uint(req(ifd, tags::IMAGE_WIDTH)?)?,
                height: tiff.uint(req(ifd, tags::IMAGE_LENGTH)?)?,
                tile_width: tiff.uint(req(ifd, tags::TILE_WIDTH)?)?,
                tile_height: tiff.uint(req(ifd, tags::TILE_LENGTH)?)?,
                compression: match ifd.get(tags::COMPRESSION) {
                    Some(e) => tiff.uint(e)?,
                    None => 1, // TIFF default: uncompressed
                },
                photometric: match ifd.get(tags::PHOTOMETRIC_INTERPRETATION) {
                    Some(e) => tiff.uint(e)?,
                    None => 6, // assume YCbCr, the common case for JPEG tiles
                },
                downsample: 1.0,
                mpp: None,
                tiles: offsets.into_iter().zip(counts).collect(),
            });
        }
        if levels.is_empty() {
            return Err(SvsError::NoLevels);
        }

        levels.sort_by(|a, b| b.width.cmp(&a.width));
        let (w0, h0) = (levels[0].width as f64, levels[0].height as f64);
        for level in &mut levels {
            level.downsample = (w0 / level.width as f64 + h0 / level.height as f64) / 2.0;
            level.mpp = mpp.map(|m| m * level.downsample);
        }

        Ok(Slide {
            tiff,
            levels,
            description,
            mpp,
            magnification,
        })
    }

    /// Size of the base level in pixels.
    pub fn dimensions(&self) -> (u64, u64) {
        (self.levels[0].width, self.levels[0].height)
    }

    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// The shared JPEG tables for a level, if its IFD has them.
    pub fn jpeg_tables(&self, level: &Level) -> Option<&[u8]> {
        let ifd = &self.tiff.ifds[level.ifd_index];
        ifd.get(tags::JPEG_TABLES)
            .and_then(|e| self.tiff.value_bytes(e).ok())
    }

    /// Index of the smallest level at least as fine as `target_mpp`.
    pub fn best_level_for_mpp(&self, target_mpp: f64) -> usize {
        let Some(base) = self.mpp else { return 0 };
        let mut best = 0;
        for (i, level) in self.levels.iter().enumerate() {
            if base * level.downsample <= target_mpp * 1.0001 {
                best = i;
            }
        }
        best
    }
}

fn req<'b>(ifd: &'b Ifd, tag: u16) -> Result<&'b Entry> {
    ifd.get(tag).ok_or(SvsError::MissingTag(tag))
}

/// A numeric `key = value` field from an Aperio description string.
fn field(desc: &str, key: &str) -> Option<f64> {
    desc.split('|').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        (k.trim() == key).then(|| v.trim().parse().ok())?
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiff::field_type;

    fn load(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/data/{}", env!("CARGO_MANIFEST_DIR"), name)).unwrap()
    }

    #[test]
    fn parses_smoke_svs() {
        let data = load("smoke.svs");
        let slide = Slide::parse(&data).unwrap();

        assert_eq!(slide.dimensions(), (512, 512));
        assert_eq!(slide.level_count(), 1);
        assert_eq!(slide.mpp, Some(0.499));
        assert_eq!(slide.magnification, Some(20.0));

        let level = &slide.levels[0];
        assert_eq!(level.ifd_index, 0);
        assert_eq!((level.tile_width, level.tile_height), (256, 256));
        assert_eq!(level.compression, 7);
        assert_eq!(level.downsample, 1.0);
        assert_eq!(level.mpp, Some(0.499));
        assert_eq!(level.tiles, [(239, 4), (243, 4), (247, 4), (251, 4)]);
        assert_eq!((level.tiles_across(), level.tiles_down()), (2, 2));

        let tables = slide.jpeg_tables(level).unwrap();
        assert_eq!(&tables[..2], b"\xff\xd8");
    }

    #[test]
    fn parses_test_tags_svs() {
        let data = load("test_tags.svs");
        let slide = Slide::parse(&data).unwrap();
        assert_eq!(slide.mpp, Some(0.25));
        assert_eq!(slide.magnification, Some(40.0));
        assert_eq!(slide.description, "Aperio Fake|AppMag = 40|MPP = 0.2500|");
    }

    #[test]
    fn rejects_non_aperio() {
        let mut data = load("smoke.svs");
        let pos = data.windows(6).position(|w| w == b"Aperio").unwrap();
        data[pos] = b'X';
        assert_eq!(Slide::parse(&data).unwrap_err(), SvsError::NotAperio);
    }

    fn entry(b: &mut Vec<u8>, tag: u16, ty: u16, count: u32, value: u32) {
        b.extend(tag.to_le_bytes());
        b.extend(ty.to_le_bytes());
        b.extend(count.to_le_bytes());
        b.extend(value.to_le_bytes());
    }

    /// Classic LE TIFF: base level 1024x768 and a 4x downsampled 256x192 level.
    /// Layout: header 0..8, IFD0 8..98, IFD1 98..176, description at 176.
    fn two_level_fixture() -> Vec<u8> {
        use field_type::{ASCII, LONG, SHORT};
        let desc = b"Aperio|AppMag = 20|MPP = 0.5|";

        let mut b = Vec::new();
        b.extend(b"II");
        b.extend(42u16.to_le_bytes());
        b.extend(8u32.to_le_bytes());

        b.extend(7u16.to_le_bytes());
        entry(&mut b, tags::IMAGE_WIDTH, LONG, 1, 1024);
        entry(&mut b, tags::IMAGE_LENGTH, LONG, 1, 768);
        entry(&mut b, tags::IMAGE_DESCRIPTION, ASCII, desc.len() as u32, 176);
        entry(&mut b, tags::TILE_WIDTH, SHORT, 1, 256);
        entry(&mut b, tags::TILE_LENGTH, SHORT, 1, 256);
        entry(&mut b, tags::TILE_OFFSETS, LONG, 1, 0);
        entry(&mut b, tags::TILE_BYTE_COUNTS, LONG, 1, 0);
        b.extend(98u32.to_le_bytes());

        b.extend(6u16.to_le_bytes());
        entry(&mut b, tags::IMAGE_WIDTH, LONG, 1, 256);
        entry(&mut b, tags::IMAGE_LENGTH, LONG, 1, 192);
        entry(&mut b, tags::TILE_WIDTH, SHORT, 1, 256);
        entry(&mut b, tags::TILE_LENGTH, SHORT, 1, 256);
        entry(&mut b, tags::TILE_OFFSETS, LONG, 1, 0);
        entry(&mut b, tags::TILE_BYTE_COUNTS, LONG, 1, 0);
        b.extend(0u32.to_le_bytes());

        b.extend(desc);
        b
    }

    #[test]
    fn discovers_pyramid_levels() {
        let data = two_level_fixture();
        let slide = Slide::parse(&data).unwrap();

        assert_eq!(slide.dimensions(), (1024, 768));
        assert_eq!(slide.level_count(), 2);
        assert_eq!(slide.mpp, Some(0.5));

        assert_eq!(slide.levels[0].downsample, 1.0);
        assert_eq!(slide.levels[1].downsample, 4.0);
        assert_eq!(slide.levels[1].mpp, Some(2.0));
        assert_eq!(slide.levels[1].compression, 1);

        assert_eq!(slide.best_level_for_mpp(0.5), 0);
        assert_eq!(slide.best_level_for_mpp(1.0), 0);
        assert_eq!(slide.best_level_for_mpp(2.0), 1);
        assert_eq!(slide.best_level_for_mpp(100.0), 1);
        assert_eq!(slide.best_level_for_mpp(0.1), 0);
    }
}
