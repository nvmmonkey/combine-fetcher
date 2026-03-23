use std::fs::File;
use std::io;
use std::mem;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use memmap2::Mmap;

use crate::layout::{AssetSlot, ShmHeader, SignalSnapshot};

/// Read-only handle to a shared-memory signal file.
///
/// Multiple readers can co-exist with a single writer. The seqlock on each
/// `AssetSlot` protects against torn reads.
pub struct SignalReader {
    mmap: Mmap,
}

/// Maximum number of seqlock retry attempts before giving up.
const MAX_SEQLOCK_RETRIES: usize = 3;

/// Default staleness threshold: 5 seconds in microseconds.
const DEFAULT_HEARTBEAT_STALE_US: u64 = 5_000_000;

impl SignalReader {
    /// Open an existing SHM file read-only.
    ///
    /// Returns an error if the file does not exist, is too small, or fails
    /// header validation (bad magic / version).
    pub fn open(path: &str) -> io::Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;

        if (metadata.len() as usize) < mem::size_of::<ShmHeader>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SHM file too small to contain header",
            ));
        }

        // SAFETY: the file already exists with a known size and will be
        // mapped read-only. The writer is the only process that mutates
        // the underlying data; we rely on the seqlock for consistency.
        let mmap = unsafe { Mmap::map(&file)? };

        let header = unsafe { &*(mmap.as_ptr() as *const ShmHeader) };
        if !header.validate() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "SHM header validation failed (magic={:#x}, version={})",
                    header.magic.load(Ordering::Relaxed),
                    header.version.load(Ordering::Relaxed),
                ),
            ));
        }

        let slot_count = header.slot_count.load(Ordering::Relaxed) as usize;
        let expected_size =
            mem::size_of::<ShmHeader>() + slot_count * mem::size_of::<AssetSlot>();
        if (metadata.len() as usize) < expected_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "SHM file too small: expected {} bytes for {} slots, got {}",
                    expected_size,
                    slot_count,
                    metadata.len(),
                ),
            ));
        }

        Ok(Self { mmap })
    }

    /// Reference to the SHM header.
    pub fn header(&self) -> &ShmHeader {
        unsafe { &*(self.mmap.as_ptr() as *const ShmHeader) }
    }

    /// Number of active asset slots.
    pub fn slot_count(&self) -> usize {
        self.header().slot_count.load(Ordering::Relaxed) as usize
    }

    /// Internal helper: get a slot reference by index.
    fn get_slot(&self, index: usize) -> Option<&AssetSlot> {
        if index >= self.slot_count() {
            return None;
        }
        let offset =
            mem::size_of::<ShmHeader>() + index * mem::size_of::<AssetSlot>();
        Some(unsafe { &*(self.mmap.as_ptr().add(offset) as *const AssetSlot) })
    }

    /// Read a consistent snapshot from a slot using the seqlock protocol.
    ///
    /// Retries up to 3 times if a torn read is detected (seq changed
    /// between pre-read and post-read, or seq is odd during read).
    /// Returns `None` if the index is out of range or all retries fail.
    pub fn read_slot(&self, index: usize) -> Option<SignalSnapshot> {
        let slot = self.get_slot(index)?;

        for _ in 0..MAX_SEQLOCK_RETRIES {
            let seq1 = slot.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                // Odd = write in progress, retry.
                continue;
            }

            let snapshot = SignalSnapshot::from_slot(slot);

            let seq2 = slot.seq.load(Ordering::Acquire);
            if seq1 == seq2 {
                return Some(snapshot);
            }
            // seq changed between reads — torn read, retry.
        }

        None // all retries exhausted
    }

    /// Find a slot by `asset` name and `timeframe_secs`, then read a
    /// consistent snapshot.
    pub fn read(&self, asset: &str, timeframe_secs: u64) -> Option<SignalSnapshot> {
        let count = self.slot_count();
        for i in 0..count {
            if let Some(slot) = self.get_slot(i) {
                let slot_asset = AssetSlot::read_string(&slot.asset);
                let slot_tf = slot.timeframe_secs.load(Ordering::Relaxed);
                if slot_asset == asset && slot_tf == timeframe_secs {
                    return self.read_slot(i);
                }
            }
        }
        None
    }

    /// Check whether the engine process is alive.
    ///
    /// "Alive" means: (1) the PID recorded in the header still exists, and
    /// (2) the heartbeat timestamp is not older than 5 seconds.
    pub fn engine_alive(&self) -> bool {
        let header = self.header();
        let pid = header.engine_pid.load(Ordering::Relaxed) as i32;
        let heartbeat_us = header.engine_heartbeat_us.load(Ordering::Relaxed);

        // Check PID exists (signal 0 = no signal, just check existence).
        let pid_exists = unsafe { libc::kill(pid, 0) } == 0;
        if !pid_exists {
            return false;
        }

        let now_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        now_us.saturating_sub(heartbeat_us) < DEFAULT_HEARTBEAT_STALE_US
    }

    /// Check whether a snapshot's market data is stale.
    ///
    /// A snapshot is stale if *both* the Binance update and the Polymarket
    /// YES update are older than `max_stale_us` microseconds from now.
    pub fn is_stale(snapshot: &SignalSnapshot, max_stale_us: u64) -> bool {
        let now_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        let binance_age = now_us.saturating_sub(snapshot.binance_update_us);
        let poly_age = now_us.saturating_sub(snapshot.poly_yes_update_us);

        binance_age > max_stale_us && poly_age > max_stale_us
    }
}
