//! Batch tile reads: coalesce file ranges, fan decode work out over the pool.

use crate::decode::{decoder_for, DecodeError, Decoder};
use crate::pool::ThreadPool;
use crate::svs::Level;
use std::fmt;
use std::sync::mpsc::channel;
use std::sync::Arc;

/// Merge ranges closer than this many bytes into one read.
pub const DEFAULT_MAX_GAP: u64 = 4096;

#[derive(Debug, PartialEq)]
pub enum ScheduleError {
    Decode(DecodeError),
    BadTileIndex(usize),
    OutOfBounds { tile: usize, offset: u64, len: u64 },
    TileSizeMismatch { tile: usize, got: (usize, usize), expected: (usize, usize) },
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "decode failed: {e}"),
            Self::BadTileIndex(t) => write!(f, "tile index {t} out of range for level"),
            Self::OutOfBounds { tile, offset, len } => {
                write!(f, "tile {tile} range {offset}+{len} lies outside the file")
            }
            Self::TileSizeMismatch { tile, got, expected } => {
                write!(f, "tile {tile} decoded to {got:?}, expected {expected:?}")
            }
        }
    }
}

impl std::error::Error for ScheduleError {}

impl From<DecodeError> for ScheduleError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

/// One coalesced read covering several tiles.
#[derive(Debug, PartialEq)]
pub struct Read {
    pub offset: u64,
    pub len: u64,
    /// Indices into the request list, in file order.
    pub tiles: Vec<usize>,
}

/// Group (offset, byte count) ranges into large reads, merging ranges that
/// are adjacent or separated by at most `max_gap` bytes.
pub fn coalesce(ranges: &[(u64, u64)], max_gap: u64) -> Vec<Read> {
    let mut order: Vec<usize> = (0..ranges.len()).collect();
    order.sort_by_key(|&i| ranges[i].0);

    let mut reads: Vec<Read> = Vec::new();
    for i in order {
        let (offset, len) = ranges[i];
        match reads.last_mut() {
            Some(r) if offset <= r.offset + r.len + max_gap => {
                r.len = r.len.max(offset + len - r.offset);
                r.tiles.push(i);
            }
            _ => reads.push(Read { offset, len, tiles: vec![i] }),
        }
    }
    reads
}

struct SendPtr(*mut u8);
unsafe impl Send for SendPtr {}

/// Decode the requested tiles of a level into one contiguous RGB8 buffer of
/// `indices.len() * tile_width * tile_height * 3` bytes, slot j holding
/// `indices[j]`. Tiles are decoded in parallel on the pool, in file order.
pub fn read_tiles(
    data: &[u8],
    level: &Level,
    jpeg_tables: Option<&[u8]>,
    indices: &[usize],
    pool: &ThreadPool,
) -> Result<Vec<u8>, ScheduleError> {
    let expected = (level.tile_width as usize, level.tile_height as usize);
    let tile_bytes = expected.0 * expected.1 * 3;
    let mut out = vec![0u8; indices.len() * tile_bytes];
    if indices.is_empty() {
        return Ok(out);
    }
    let decoder: Arc<dyn Decoder> =
        Arc::from(decoder_for(level.compression, level.photometric)?);

    let ranges: Vec<(u64, u64)> = indices
        .iter()
        .map(|&t| level.tiles.get(t).copied().ok_or(ScheduleError::BadTileIndex(t)))
        .collect::<Result<_, _>>()?;
    for (j, &(offset, len)) in ranges.iter().enumerate() {
        if offset + len > data.len() as u64 {
            return Err(ScheduleError::OutOfBounds { tile: indices[j], offset, len });
        }
    }

    let (tx, rx) = channel();
    for read in coalesce(&ranges, DEFAULT_MAX_GAP) {
        for j in read.tiles {
            let (offset, len) = ranges[j];
            let src = &data[offset as usize..(offset + len) as usize];
            // Safety: we do not return from this function until every job has
            // reported back (or been dropped), so these borrows cannot outlive
            // the data they point into, and each job writes a disjoint slot.
            let src: &'static [u8] = unsafe { std::mem::transmute(src) };
            let tables: Option<&'static [u8]> = unsafe { std::mem::transmute(jpeg_tables) };
            let dst = SendPtr(unsafe { out.as_mut_ptr().add(j * tile_bytes) });
            let decoder = Arc::clone(&decoder);
            let tx = tx.clone();
            pool.execute(move || {
                let dst = dst;
                let slot = unsafe { std::slice::from_raw_parts_mut(dst.0, tile_bytes) };
                let result = decoder.decode(src, tables, slot);
                let _ = tx.send((j, result));
            });
        }
    }
    drop(tx);

    let mut first_error = None;
    // Drain every result before returning so no job can still hold a borrow.
    while let Ok((j, result)) = rx.recv() {
        let error = match result {
            Ok(dims) if dims == expected => continue,
            Ok(got) => ScheduleError::TileSizeMismatch { tile: indices[j], got, expected },
            Err(e) => ScheduleError::Decode(e),
        };
        first_error.get_or_insert(error);
    }
    match first_error {
        None => Ok(out),
        Some(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svs::Slide;
    use crate::tiff::{field_type, tags};
    use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

    #[test]
    fn coalesces_adjacent_ranges() {
        let reads = coalesce(&[(0, 4), (4, 4), (100, 4)], 0);
        assert_eq!(
            reads,
            [
                Read { offset: 0, len: 8, tiles: vec![0, 1] },
                Read { offset: 100, len: 4, tiles: vec![2] },
            ]
        );
    }

    #[test]
    fn coalesces_across_small_gaps_only() {
        assert_eq!(coalesce(&[(0, 4), (10, 4)], 8).len(), 1);
        assert_eq!(coalesce(&[(0, 4), (10, 4)], 4).len(), 2);
    }

    #[test]
    fn coalesce_sorts_and_handles_overlap() {
        let reads = coalesce(&[(100, 4), (0, 10), (104, 4), (2, 3)], 0);
        assert_eq!(
            reads,
            [
                Read { offset: 0, len: 10, tiles: vec![1, 3] },
                Read { offset: 100, len: 8, tiles: vec![0, 2] },
            ]
        );
    }

    const TILE: usize = 16;

    fn solid_jpeg(rgb: [u8; 3]) -> Vec<u8> {
        let pixels: Vec<u8> = rgb.iter().copied().cycle().take(TILE * TILE * 3).collect();
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf, 100);
        enc.set_sampling_factor(SamplingFactor::F_1_1);
        enc.encode(&pixels, TILE as u16, TILE as u16, ColorType::Rgb).unwrap();
        buf
    }

    fn entry(b: &mut Vec<u8>, tag: u16, ty: u16, count: u32, value: u32) {
        b.extend(tag.to_le_bytes());
        b.extend(ty.to_le_bytes());
        b.extend(count.to_le_bytes());
        b.extend(value.to_le_bytes());
    }

    /// A one-level SVS with real JPEG tiles. Needs at least 2 tiles so the
    /// offset and count arrays are stored out of line.
    fn build_svs(jpegs: &[Vec<u8>], compression: u32) -> Vec<u8> {
        use field_type::{ASCII, LONG, SHORT};
        let desc = b"Aperio|MPP = 0.5|";
        let n = jpegs.len() as u32;
        assert!(n >= 2);

        let data_start = 8 + 2 + 8 * 12 + 4; // header + 8-entry IFD
        let desc_off = data_start;
        let offsets_off = desc_off + desc.len() as u32;
        let counts_off = offsets_off + 4 * n;
        let mut next = counts_off + 4 * n;
        let tile_offsets: Vec<u32> = jpegs
            .iter()
            .map(|j| {
                let o = next;
                next += j.len() as u32;
                o
            })
            .collect();

        let side = (jpegs.len() as f64).sqrt() as u32 * TILE as u32;
        let mut b = Vec::new();
        b.extend(b"II");
        b.extend(42u16.to_le_bytes());
        b.extend(8u32.to_le_bytes());
        b.extend(8u16.to_le_bytes());
        entry(&mut b, tags::IMAGE_WIDTH, LONG, 1, side);
        entry(&mut b, tags::IMAGE_LENGTH, LONG, 1, side);
        entry(&mut b, tags::COMPRESSION, SHORT, 1, compression);
        entry(&mut b, tags::IMAGE_DESCRIPTION, ASCII, desc.len() as u32, desc_off);
        entry(&mut b, tags::TILE_WIDTH, SHORT, 1, TILE as u32);
        entry(&mut b, tags::TILE_LENGTH, SHORT, 1, TILE as u32);
        entry(&mut b, tags::TILE_OFFSETS, LONG, n, offsets_off);
        entry(&mut b, tags::TILE_BYTE_COUNTS, LONG, n, counts_off);
        b.extend(0u32.to_le_bytes());
        b.extend(desc);
        for o in &tile_offsets {
            b.extend(o.to_le_bytes());
        }
        for j in jpegs {
            b.extend((j.len() as u32).to_le_bytes());
        }
        for j in jpegs {
            b.extend(j);
        }
        b
    }

    const COLORS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [128, 128, 128]];

    fn assert_slot_is_color(out: &[u8], slot: usize, rgb: [u8; 3]) {
        let bytes = TILE * TILE * 3;
        for px in out[slot * bytes..(slot + 1) * bytes].chunks_exact(3) {
            for c in 0..3 {
                assert!(px[c].abs_diff(rgb[c]) <= 3, "slot {slot}: {px:?} != {rgb:?}");
            }
        }
    }

    #[test]
    fn reads_full_batch_in_parallel() {
        let jpegs: Vec<_> = COLORS.iter().map(|&c| solid_jpeg(c)).collect();
        let data = build_svs(&jpegs, 7);
        let slide = Slide::parse(&data).unwrap();
        let level = &slide.levels[0];
        let pool = ThreadPool::new(4);

        let out = read_tiles(&data, level, None, &[0, 1, 2, 3], &pool).unwrap();
        assert_eq!(out.len(), 4 * TILE * TILE * 3);
        for (slot, &color) in COLORS.iter().enumerate() {
            assert_slot_is_color(&out, slot, color);
        }
    }

    #[test]
    fn subset_keeps_request_order() {
        let jpegs: Vec<_> = COLORS.iter().map(|&c| solid_jpeg(c)).collect();
        let data = build_svs(&jpegs, 7);
        let slide = Slide::parse(&data).unwrap();
        let pool = ThreadPool::new(2);

        let out = read_tiles(&data, &slide.levels[0], None, &[3, 1], &pool).unwrap();
        assert_slot_is_color(&out, 0, COLORS[3]);
        assert_slot_is_color(&out, 1, COLORS[1]);
    }

    #[test]
    fn empty_request_is_empty() {
        let jpegs: Vec<_> = COLORS.iter().map(|&c| solid_jpeg(c)).collect();
        let data = build_svs(&jpegs, 7);
        let slide = Slide::parse(&data).unwrap();
        let pool = ThreadPool::new(2);
        assert!(read_tiles(&data, &slide.levels[0], None, &[], &pool).unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_requests() {
        let jpegs: Vec<_> = COLORS.iter().map(|&c| solid_jpeg(c)).collect();
        let data = build_svs(&jpegs, 7);
        let slide = Slide::parse(&data).unwrap();
        let level = &slide.levels[0];
        let pool = ThreadPool::new(2);

        assert_eq!(
            read_tiles(&data, level, None, &[7], &pool).unwrap_err(),
            ScheduleError::BadTileIndex(7)
        );

        let truncated = &data[..data.len() - 10];
        assert!(matches!(
            read_tiles(truncated, level, None, &[3], &pool).unwrap_err(),
            ScheduleError::OutOfBounds { tile: 3, .. }
        ));
    }

    #[test]
    fn rejects_unsupported_compression() {
        let jpegs: Vec<_> = COLORS.iter().map(|&c| solid_jpeg(c)).collect();
        let data = build_svs(&jpegs, 33003);
        let slide = Slide::parse(&data).unwrap();
        let pool = ThreadPool::new(2);
        assert_eq!(
            read_tiles(&data, &slide.levels[0], None, &[0], &pool).unwrap_err(),
            ScheduleError::Decode(DecodeError::UnsupportedCompression(33003))
        );
    }

    #[test]
    fn corrupt_tile_reports_error_after_draining() {
        let mut jpegs: Vec<_> = COLORS.iter().map(|&c| solid_jpeg(c)).collect();
        jpegs[2] = vec![0xff; 40]; // not a JPEG
        let data = build_svs(&jpegs, 7);
        let slide = Slide::parse(&data).unwrap();
        let pool = ThreadPool::new(4);
        let err = read_tiles(&data, &slide.levels[0], None, &[0, 1, 2, 3], &pool).unwrap_err();
        assert!(matches!(err, ScheduleError::Decode(DecodeError::Jpeg(_))));
    }
}
