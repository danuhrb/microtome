//! Tile decoders. CPU-only for now; the trait leaves room for a GPU backend.

use std::fmt;

pub mod compression {
    pub const NONE: u64 = 1;
    pub const JPEG: u64 = 7;
    /// Aperio JPEG 2000, YCbCr.
    pub const APERIO_J2K_YCBCR: u64 = 33003;
    /// Aperio JPEG 2000, RGB.
    pub const APERIO_J2K_RGB: u64 = 33005;
}

pub mod photometric {
    pub const RGB: u64 = 2;
    pub const YCBCR: u64 = 6;
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Jpeg(String),
    OutputTooSmall { needed: usize, got: usize },
    UnsupportedCompression(u64),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jpeg(e) => write!(f, "jpeg decode failed: {e}"),
            Self::OutputTooSmall { needed, got } => {
                write!(f, "output buffer holds {got} bytes, tile needs {needed}")
            }
            Self::UnsupportedCompression(c) => write!(f, "unsupported compression scheme {c}"),
        }
    }
}

impl std::error::Error for DecodeError {}

type Result<T> = std::result::Result<T, DecodeError>;

pub trait Decoder: Send + Sync {
    /// Decode one tile into `out` as 8-bit RGB, row-major.
    /// `tables` is the shared JPEGTables blob, if the file has one.
    /// Returns the decoded (width, height).
    fn decode(&self, tile: &[u8], tables: Option<&[u8]>, out: &mut [u8]) -> Result<(usize, usize)>;
}

/// The decoder for a TIFF compression scheme and photometric interpretation.
pub fn decoder_for(compression: u64, photometric: u64) -> Result<Box<dyn Decoder>> {
    match compression {
        compression::JPEG => Ok(Box::new(JpegTileDecoder {
            rgb: photometric == photometric::RGB,
        })),
        c => Err(DecodeError::UnsupportedCompression(c)),
    }
}

pub struct JpegTileDecoder {
    /// The TIFF photometric tag says samples are RGB, whatever the JPEG
    /// stream itself claims.
    pub rgb: bool,
}

impl Decoder for JpegTileDecoder {
    fn decode(&self, _tile: &[u8], _tables: Option<&[u8]>, _out: &mut [u8]) -> Result<(usize, usize)> {
        let _ = self.rgb;
        Err(DecodeError::Jpeg("not implemented".into()))
    }
}
