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
