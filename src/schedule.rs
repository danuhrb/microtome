//! Batch tile reads: coalesce file ranges, fan decode work out over the pool.

use std::fmt;

/// Merge ranges closer than this many bytes into one read.
pub const DEFAULT_MAX_GAP: u64 = 4096;

#[derive(Debug, PartialEq)]
pub enum ScheduleError {
    Decode(crate::decode::DecodeError),
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

impl From<crate::decode::DecodeError> for ScheduleError {
    fn from(e: crate::decode::DecodeError) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
