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

        Ok(Slide {
            tiff,
            levels,
            description,
            mpp,
            magnification,
        })
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
