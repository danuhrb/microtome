//! Tile decoders. CPU-only for now; the trait leaves room for a GPU backend.

use std::fmt;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

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
    fn decode(&self, tile: &[u8], tables: Option<&[u8]>, out: &mut [u8]) -> Result<(usize, usize)> {
        let spliced;
        let stream = match tables {
            Some(t) => {
                spliced = splice_tables(tile, t)?;
                spliced.as_slice()
            }
            None => tile,
        };
        let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut decoder =
            zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(stream), options);
        decoder
            .decode_headers()
            .map_err(|e| DecodeError::Jpeg(e.to_string()))?;
        // Aperio writes raw RGB samples into JPEG streams whose headers imply
        // YCbCr. When the TIFF tag says RGB, ask for YCbCr output so the
        // decode is a component passthrough instead of a bogus conversion.
        if self.rgb && decoder.input_colorspace() == Some(ColorSpace::YCbCr) {
            let opts = decoder.options().jpeg_set_out_colorspace(ColorSpace::YCbCr);
            decoder.set_options(opts);
        }
        let needed = decoder
            .output_buffer_size()
            .ok_or_else(|| DecodeError::Jpeg("image dimensions overflow".into()))?;
        if out.len() < needed {
            return Err(DecodeError::OutputTooSmall {
                needed,
                got: out.len(),
            });
        }
        decoder
            .decode_into(out)
            .map_err(|e| DecodeError::Jpeg(e.to_string()))?;
        Ok(decoder.dimensions().unwrap())
    }
}

const SOI: [u8; 2] = [0xff, 0xd8];
const EOI: [u8; 2] = [0xff, 0xd9];

/// Abbreviated SVS tiles carry no quantization or Huffman tables. The shared
/// tables from the JPEGTables tag (an SOI..EOI stream) are spliced in ahead
/// of the tile's own segments.
fn splice_tables(tile: &[u8], tables: &[u8]) -> Result<Vec<u8>> {
    if tables.len() < 4 || tables[..2] != SOI || tile.len() < 2 || tile[..2] != SOI {
        return Err(DecodeError::Jpeg("missing SOI marker".into()));
    }
    let mut tables_body = &tables[2..];
    if tables_body.ends_with(&EOI) {
        tables_body = &tables_body[..tables_body.len() - 2];
    }
    let mut out = Vec::with_capacity(tables.len() + tile.len());
    out.extend(SOI);
    out.extend(tables_body);
    out.extend(&tile[2..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

    fn gradient(w: usize, h: usize) -> Vec<u8> {
        let mut px = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                px.push((x * 255 / w) as u8);
                px.push((y * 255 / h) as u8);
                px.push(128);
            }
        }
        px
    }

    fn encode(pixels: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf, 100);
        enc.set_sampling_factor(SamplingFactor::F_1_1);
        enc.encode(pixels, w as u16, h as u16, ColorType::Rgb).unwrap();
        buf
    }

    fn max_channel_diff(a: &[u8], b: &[u8]) -> u8 {
        a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap()
    }

    #[test]
    fn decodes_jpeg_tile() {
        let (w, h) = (16, 16);
        let pixels = gradient(w, h);
        let jpeg = encode(&pixels, w, h);

        let mut out = vec![0u8; w * h * 3];
        let dims = JpegTileDecoder { rgb: false }.decode(&jpeg, None, &mut out).unwrap();
        assert_eq!(dims, (w, h));
        assert!(max_channel_diff(&pixels, &out) <= 8);
    }

    /// Split a full JPEG into a tables stream (DQT + DHT) and an abbreviated
    /// tile stream, the way SVS files store them.
    fn split_tables(jpeg: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut tables = SOI.to_vec();
        let mut tile = SOI.to_vec();
        let mut i = 2;
        loop {
            assert_eq!(jpeg[i], 0xff);
            let marker = jpeg[i + 1];
            if marker == 0xda {
                tile.extend(&jpeg[i..]); // SOS and entropy data to end
                break;
            }
            let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
            let segment = &jpeg[i..i + 2 + len];
            if marker == 0xdb || marker == 0xc4 {
                tables.extend(segment);
            } else {
                tile.extend(segment);
            }
            i += 2 + len;
        }
        tables.extend(EOI);
        (tables, tile)
    }
}
