use std::fs::{File, OpenOptions};
use std::io;
use std::mem;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use memmap2::MmapMut;

use crate::layout::{AssetSlot, ShmHeader, SHM_MAGIC, SHM_VERSION};

/// Writer handle to a shared-memory signal file.
///
/// There must be exactly one writer per SHM file. The writer owns the mmap
/// and is responsible for initialising the header and keeping the heartbeat
/// alive.
pub struct SignalWriter {
    _file: File,
    mmap: MmapMut,
}

impl SignalWriter {
    /// Create (or overwrite) a SHM file at `path` with `slot_count` asset
    /// slots. The file is truncated to the correct size, zero-initialised,
    /// and the header is written.
    pub fn create(path: &str, slot_count: usize) -> io::Result<Self> {
        if slot_count == 0 || slot_count > crate::layout::MAX_SLOTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "slot_count must be 1..={}, got {}",
                    crate::layout::MAX_SLOTS,
                    slot_count
                ),
            ));
        }

        let total_size =
            mem::size_of::<ShmHeader>() + slot_count * mem::size_of::<AssetSlot>();

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;

        file.set_len(total_size as u64)?;

        // SAFETY: we just created/truncated the file to `total_size` bytes
        // and we are the sole writer. The mmap is backed by a regular file
        // (or tmpfs on /dev/shm) so the kernel guarantees the mapping is
        // valid for the file's lifetime.
        let mut mmap = unsafe { MmapMut::map_mut(&file)? };

        // Zero-initialise the entire region.
        mmap.fill(0);

        // Write header fields.
        {
            let header = unsafe { &*(mmap.as_ptr() as *const ShmHeader) };
            header.magic.store(SHM_MAGIC, Ordering::Relaxed);
            header.version.store(SHM_VERSION as u64, Ordering::Relaxed);
            header.slot_count.store(slot_count as u64, Ordering::Relaxed);
            header
                .engine_pid
                .store(std::process::id() as u64, Ordering::Relaxed);

            let now_us = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64;
            header.engine_start_us.store(now_us, Ordering::Relaxed);
            header.engine_heartbeat_us.store(now_us, Ordering::Relaxed);
        }

        // Flush initial header to backing store.
        mmap.flush()?;

        Ok(Self { _file: file, mmap })
    }

    /// Immutable reference to the SHM header.
    pub fn header(&self) -> &ShmHeader {
        unsafe { &*(self.mmap.as_ptr() as *const ShmHeader) }
    }

    /// Reference to asset slot at `index` (0-based).
    ///
    /// # Panics
    ///
    /// Panics if `index >= slot_count`.
    pub fn slot(&self, index: usize) -> &AssetSlot {
        let slot_count = self.header().slot_count.load(Ordering::Relaxed) as usize;
        assert!(
            index < slot_count,
            "slot index {} out of range (slot_count={})",
            index,
            slot_count
        );

        let offset = mem::size_of::<ShmHeader>() + index * mem::size_of::<AssetSlot>();
        unsafe { &*(self.mmap.as_ptr().add(offset) as *const AssetSlot) }
    }

    /// Update the engine heartbeat timestamp to *now*.
    /// Call this periodically (every ~1 s) so readers can detect a dead
    /// engine.
    pub fn heartbeat(&self) {
        let now_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        self.header()
            .engine_heartbeat_us
            .store(now_us, Ordering::Relaxed);
    }
}
