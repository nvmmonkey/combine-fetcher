use std::sync::atomic::{AtomicI64, AtomicU64, AtomicU8, Ordering};

pub const MAX_SLOTS: usize = 16;
pub const TOKEN_ID_LEN: usize = 96;
pub const SHM_MAGIC: u64 = 0x504F4C595349474E; // "POLYSIGN"
pub const SHM_VERSION: u32 = 1;
pub const DEFAULT_SHM_PATH: &str = "/dev/shm/poly-signals.shm";

// ---------------------------------------------------------------------------
// Header — one per SHM file, at offset 0
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct ShmHeader {
    pub magic: AtomicU64,
    pub version: AtomicU64, // stored as u64 for alignment
    pub slot_count: AtomicU64,
    pub engine_pid: AtomicU64,
    pub engine_heartbeat_us: AtomicU64,
    pub engine_start_us: AtomicU64,
    _pad: [u8; 16],
}

impl ShmHeader {
    /// Validate magic number and version.
    pub fn validate(&self) -> bool {
        let magic = self.magic.load(Ordering::Relaxed);
        let version = self.version.load(Ordering::Relaxed);
        magic == SHM_MAGIC && version == SHM_VERSION as u64
    }
}

// ---------------------------------------------------------------------------
// Asset Slot — one per tracked market
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct AssetSlot {
    // Identity (written once per rotation)
    pub asset: [AtomicU8; 8], // e.g. "btc\0\0\0\0\0"
    pub timeframe_secs: AtomicU64, // 300 or 900

    // Seqlock: odd = write in progress, even = consistent
    pub seq: AtomicU64,

    // Timestamps (microseconds since unix epoch)
    pub binance_update_us: AtomicU64,
    pub poly_yes_update_us: AtomicU64,
    pub poly_no_update_us: AtomicU64,
    pub rtds_update_us: AtomicU64,

    // Binance BBA (f64 stored as u64 bits)
    pub binance_mid: AtomicU64,
    pub binance_bid: AtomicU64,
    pub binance_ask: AtomicU64,
    pub binance_volume_1m: AtomicU64,

    // Polymarket BBA
    pub yes_bid: AtomicU64,
    pub yes_ask: AtomicU64,
    pub yes_bid_size: AtomicU64,
    pub yes_ask_size: AtomicU64,
    pub no_bid: AtomicU64,
    pub no_ask: AtomicU64,
    pub no_bid_size: AtomicU64,
    pub no_ask_size: AtomicU64,

    // Market context
    pub strike: AtomicU64,
    pub ttx_seconds: AtomicI64,
    pub window_start: AtomicI64,
    pub window_end: AtomicI64,
    pub market_epoch: AtomicU64, // rotation counter

    // Derived: Volatility
    pub volatility: AtomicU64,
    pub vol_1sec: AtomicU64,
    pub vol_5sec: AtomicU64,
    pub vol_13sec: AtomicU64,
    pub vol_34sec: AtomicU64,
    pub vol_89sec: AtomicU64,

    // OBI
    pub yes_book_imbalance: AtomicU64,
    pub combined_obi: AtomicU64,
    pub binance_obi: AtomicU64,

    // Depth (top 3 levels aggregated)
    pub yes_bid_depth_3: AtomicU64,
    pub yes_ask_depth_3: AtomicU64,
    pub no_bid_depth_3: AtomicU64,
    pub no_ask_depth_3: AtomicU64,
    pub yes_bid_depth_all: AtomicU64,
    pub yes_ask_depth_all: AtomicU64,
    pub no_bid_depth_all: AtomicU64,
    pub no_ask_depth_all: AtomicU64,

    // RTDS
    pub rtds_binance: AtomicU64,
    pub rtds_chainlink: AtomicU64,
    pub rtds_binance_dev_bps: AtomicU64,
    pub rtds_chainlink_dev_bps: AtomicU64,
    pub rtds_advantage_bps: AtomicU64,

    // Greeks
    pub delta: AtomicU64,
    pub gamma_greek: AtomicU64,
    pub vega: AtomicU64,
    pub theta: AtomicU64,

    // Trade flow
    pub yes_last_trade_price: AtomicU64,
    pub yes_last_trade_size: AtomicU64,
    pub no_last_trade_price: AtomicU64,
    pub no_last_trade_size: AtomicU64,

    // Token IDs (written on rotation, protected by seqlock)
    pub yes_token_id: [AtomicU8; TOKEN_ID_LEN],
    pub no_token_id: [AtomicU8; TOKEN_ID_LEN],
    pub condition_id: [AtomicU8; TOKEN_ID_LEN],
    pub slug: [AtomicU8; TOKEN_ID_LEN],

    // Resolution state
    pub market_resolved: AtomicU8,
    pub outcome: [AtomicU8; 8],

    _pad: [u8; 7], // align to 64 bytes
}

// ---------------------------------------------------------------------------
// Helper methods on AssetSlot
// ---------------------------------------------------------------------------

impl AssetSlot {
    /// Read an `AtomicU64` field as `f64` via `from_bits`.
    #[inline]
    pub fn read_f64(field: &AtomicU64) -> f64 {
        f64::from_bits(field.load(Ordering::Relaxed))
    }

    /// Write an `f64` into an `AtomicU64` field via `to_bits`.
    #[inline]
    pub fn write_f64(field: &AtomicU64, val: f64) {
        field.store(val.to_bits(), Ordering::Relaxed);
    }

    /// Read a null-terminated UTF-8 string from an `AtomicU8` array.
    pub fn read_string(field: &[AtomicU8]) -> String {
        let mut bytes = Vec::with_capacity(field.len());
        for atom in field {
            let b = atom.load(Ordering::Relaxed);
            if b == 0 {
                break;
            }
            bytes.push(b);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Write a string into an `AtomicU8` array, null-padding the remainder.
    pub fn write_string(field: &[AtomicU8], val: &str) {
        let bytes = val.as_bytes();
        let len = bytes.len().min(field.len());
        for (i, atom) in field.iter().enumerate() {
            if i < len {
                atom.store(bytes[i], Ordering::Relaxed);
            } else {
                atom.store(0, Ordering::Relaxed);
            }
        }
    }

    /// Begin a seqlock write section (makes seq odd).
    pub fn begin_write(&self) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s + 1, Ordering::Release); // odd = writing
    }

    /// End a seqlock write section (makes seq even).
    pub fn end_write(&self) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s + 1, Ordering::Release); // even = done
    }
}

// ---------------------------------------------------------------------------
// SignalSnapshot — a plain-data copy of one slot, no atomics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SignalSnapshot {
    pub asset: String,
    pub timeframe_secs: u64,
    pub seq: u64,

    pub binance_update_us: u64,
    pub poly_yes_update_us: u64,
    pub poly_no_update_us: u64,
    pub rtds_update_us: u64,

    pub binance_mid: f64,
    pub binance_bid: f64,
    pub binance_ask: f64,
    pub binance_volume_1m: f64,

    pub yes_bid: f64,
    pub yes_ask: f64,
    pub yes_bid_size: f64,
    pub yes_ask_size: f64,
    pub no_bid: f64,
    pub no_ask: f64,
    pub no_bid_size: f64,
    pub no_ask_size: f64,

    pub strike: f64,
    pub ttx_seconds: i64,
    pub window_start: i64,
    pub window_end: i64,
    pub market_epoch: u64,

    pub volatility: f64,
    pub vol_1sec: f64,
    pub vol_5sec: f64,
    pub vol_13sec: f64,
    pub vol_34sec: f64,
    pub vol_89sec: f64,

    pub yes_book_imbalance: f64,
    pub combined_obi: f64,
    pub binance_obi: f64,

    pub yes_bid_depth_3: f64,
    pub yes_ask_depth_3: f64,
    pub no_bid_depth_3: f64,
    pub no_ask_depth_3: f64,
    pub yes_bid_depth_all: f64,
    pub yes_ask_depth_all: f64,
    pub no_bid_depth_all: f64,
    pub no_ask_depth_all: f64,

    pub rtds_binance: f64,
    pub rtds_chainlink: f64,
    pub rtds_binance_dev_bps: f64,
    pub rtds_chainlink_dev_bps: f64,
    pub rtds_advantage_bps: f64,

    pub delta: f64,
    pub gamma_greek: f64,
    pub vega: f64,
    pub theta: f64,

    pub yes_last_trade_price: f64,
    pub yes_last_trade_size: f64,
    pub no_last_trade_price: f64,
    pub no_last_trade_size: f64,

    pub yes_token_id: String,
    pub no_token_id: String,
    pub condition_id: String,
    pub slug: String,

    pub market_resolved: bool,
    pub outcome: String,
}

impl Default for SignalSnapshot {
    fn default() -> Self {
        Self {
            asset: String::new(),
            timeframe_secs: 0,
            seq: 0,

            binance_update_us: 0,
            poly_yes_update_us: 0,
            poly_no_update_us: 0,
            rtds_update_us: 0,

            binance_mid: 0.0,
            binance_bid: 0.0,
            binance_ask: 0.0,
            binance_volume_1m: 0.0,

            yes_bid: 0.0,
            yes_ask: 0.0,
            yes_bid_size: 0.0,
            yes_ask_size: 0.0,
            no_bid: 0.0,
            no_ask: 0.0,
            no_bid_size: 0.0,
            no_ask_size: 0.0,

            strike: 0.0,
            ttx_seconds: 0,
            window_start: 0,
            window_end: 0,
            market_epoch: 0,

            volatility: 0.0,
            vol_1sec: 0.0,
            vol_5sec: 0.0,
            vol_13sec: 0.0,
            vol_34sec: 0.0,
            vol_89sec: 0.0,

            yes_book_imbalance: 0.0,
            combined_obi: 0.0,
            binance_obi: 0.0,

            yes_bid_depth_3: 0.0,
            yes_ask_depth_3: 0.0,
            no_bid_depth_3: 0.0,
            no_ask_depth_3: 0.0,
            yes_bid_depth_all: 0.0,
            yes_ask_depth_all: 0.0,
            no_bid_depth_all: 0.0,
            no_ask_depth_all: 0.0,

            rtds_binance: 0.0,
            rtds_chainlink: 0.0,
            rtds_binance_dev_bps: 0.0,
            rtds_chainlink_dev_bps: 0.0,
            rtds_advantage_bps: 0.0,

            delta: 0.0,
            gamma_greek: 0.0,
            vega: 0.0,
            theta: 0.0,

            yes_last_trade_price: 0.0,
            yes_last_trade_size: 0.0,
            no_last_trade_price: 0.0,
            no_last_trade_size: 0.0,

            yes_token_id: String::new(),
            no_token_id: String::new(),
            condition_id: String::new(),
            slug: String::new(),

            market_resolved: false,
            outcome: String::new(),
        }
    }
}

impl SignalSnapshot {
    /// Build a snapshot by reading all fields from an `AssetSlot`.
    /// Caller is responsible for seqlock validation.
    pub(crate) fn from_slot(slot: &AssetSlot) -> Self {
        Self {
            asset: AssetSlot::read_string(&slot.asset),
            timeframe_secs: slot.timeframe_secs.load(Ordering::Relaxed),
            seq: slot.seq.load(Ordering::Relaxed),

            binance_update_us: slot.binance_update_us.load(Ordering::Relaxed),
            poly_yes_update_us: slot.poly_yes_update_us.load(Ordering::Relaxed),
            poly_no_update_us: slot.poly_no_update_us.load(Ordering::Relaxed),
            rtds_update_us: slot.rtds_update_us.load(Ordering::Relaxed),

            binance_mid: AssetSlot::read_f64(&slot.binance_mid),
            binance_bid: AssetSlot::read_f64(&slot.binance_bid),
            binance_ask: AssetSlot::read_f64(&slot.binance_ask),
            binance_volume_1m: AssetSlot::read_f64(&slot.binance_volume_1m),

            yes_bid: AssetSlot::read_f64(&slot.yes_bid),
            yes_ask: AssetSlot::read_f64(&slot.yes_ask),
            yes_bid_size: AssetSlot::read_f64(&slot.yes_bid_size),
            yes_ask_size: AssetSlot::read_f64(&slot.yes_ask_size),
            no_bid: AssetSlot::read_f64(&slot.no_bid),
            no_ask: AssetSlot::read_f64(&slot.no_ask),
            no_bid_size: AssetSlot::read_f64(&slot.no_bid_size),
            no_ask_size: AssetSlot::read_f64(&slot.no_ask_size),

            strike: AssetSlot::read_f64(&slot.strike),
            ttx_seconds: slot.ttx_seconds.load(Ordering::Relaxed),
            window_start: slot.window_start.load(Ordering::Relaxed),
            window_end: slot.window_end.load(Ordering::Relaxed),
            market_epoch: slot.market_epoch.load(Ordering::Relaxed),

            volatility: AssetSlot::read_f64(&slot.volatility),
            vol_1sec: AssetSlot::read_f64(&slot.vol_1sec),
            vol_5sec: AssetSlot::read_f64(&slot.vol_5sec),
            vol_13sec: AssetSlot::read_f64(&slot.vol_13sec),
            vol_34sec: AssetSlot::read_f64(&slot.vol_34sec),
            vol_89sec: AssetSlot::read_f64(&slot.vol_89sec),

            yes_book_imbalance: AssetSlot::read_f64(&slot.yes_book_imbalance),
            combined_obi: AssetSlot::read_f64(&slot.combined_obi),
            binance_obi: AssetSlot::read_f64(&slot.binance_obi),

            yes_bid_depth_3: AssetSlot::read_f64(&slot.yes_bid_depth_3),
            yes_ask_depth_3: AssetSlot::read_f64(&slot.yes_ask_depth_3),
            no_bid_depth_3: AssetSlot::read_f64(&slot.no_bid_depth_3),
            no_ask_depth_3: AssetSlot::read_f64(&slot.no_ask_depth_3),
            yes_bid_depth_all: AssetSlot::read_f64(&slot.yes_bid_depth_all),
            yes_ask_depth_all: AssetSlot::read_f64(&slot.yes_ask_depth_all),
            no_bid_depth_all: AssetSlot::read_f64(&slot.no_bid_depth_all),
            no_ask_depth_all: AssetSlot::read_f64(&slot.no_ask_depth_all),

            rtds_binance: AssetSlot::read_f64(&slot.rtds_binance),
            rtds_chainlink: AssetSlot::read_f64(&slot.rtds_chainlink),
            rtds_binance_dev_bps: AssetSlot::read_f64(&slot.rtds_binance_dev_bps),
            rtds_chainlink_dev_bps: AssetSlot::read_f64(&slot.rtds_chainlink_dev_bps),
            rtds_advantage_bps: AssetSlot::read_f64(&slot.rtds_advantage_bps),

            delta: AssetSlot::read_f64(&slot.delta),
            gamma_greek: AssetSlot::read_f64(&slot.gamma_greek),
            vega: AssetSlot::read_f64(&slot.vega),
            theta: AssetSlot::read_f64(&slot.theta),

            yes_last_trade_price: AssetSlot::read_f64(&slot.yes_last_trade_price),
            yes_last_trade_size: AssetSlot::read_f64(&slot.yes_last_trade_size),
            no_last_trade_price: AssetSlot::read_f64(&slot.no_last_trade_price),
            no_last_trade_size: AssetSlot::read_f64(&slot.no_last_trade_size),

            yes_token_id: AssetSlot::read_string(&slot.yes_token_id),
            no_token_id: AssetSlot::read_string(&slot.no_token_id),
            condition_id: AssetSlot::read_string(&slot.condition_id),
            slug: AssetSlot::read_string(&slot.slug),

            market_resolved: slot.market_resolved.load(Ordering::Relaxed) != 0,
            outcome: AssetSlot::read_string(&slot.outcome),
        }
    }
}
