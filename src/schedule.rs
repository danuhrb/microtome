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

