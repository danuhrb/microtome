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
}
