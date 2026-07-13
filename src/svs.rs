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
