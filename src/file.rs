use memmap2::Mmap;
use std::fs::File;
use std::io;
use std::path::Path;

/// Read-only memory-mapped slide file. One handle, shared by all threads.
pub struct SlideFile {
    mmap: Mmap,
}

impl SlideFile {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        // Safety: mapped read-only; the file must not be truncated while mapped.
        let mmap = unsafe { Mmap::map(&file)? };
        Ok(Self { mmap })
    }

    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mmap.is_empty()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.mmap
    }

    /// A slice of the file, or an error if the range falls outside it.
    pub fn slice(&self, offset: u64, len: usize) -> io::Result<&[u8]> {
        usize::try_from(offset)
            .ok()
            .and_then(|start| start.checked_add(len).map(|end| (start, end)))
            .and_then(|(start, end)| self.mmap.get(start..end))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "range {}..{} out of bounds for file of {} bytes",
                        offset,
                        offset.saturating_add(len as u64),
                        self.mmap.len()
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> SlideFile {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/smoke.svs");
        SlideFile::open(path).unwrap()
    }

    #[test]
    fn opens_and_reads_header() {
        let f = fixture();
        assert_eq!(f.len(), 255);
        assert_eq!(f.slice(0, 4).unwrap(), b"II*\0");
        assert_eq!(f.bytes()[..4], *b"II*\0");
    }

    #[test]
    fn slice_bounds() {
        let f = fixture();
        assert!(f.slice(0, 255).is_ok());
        assert!(f.slice(255, 0).is_ok());
        assert!(f.slice(0, 256).is_err());
        assert!(f.slice(250, 10).is_err());
        assert!(f.slice(u64::MAX, 1).is_err());
    }

    #[test]
    fn missing_file_errors() {
        assert!(SlideFile::open("/nonexistent.svs").is_err());
    }

    #[test]
    fn shared_across_threads() {
        let f = std::sync::Arc::new(fixture());
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let f = f.clone();
                std::thread::spawn(move || f.slice(0, 2).unwrap() == b"II")
            })
            .collect();
        for h in handles {
            assert!(h.join().unwrap());
        }
    }
}
