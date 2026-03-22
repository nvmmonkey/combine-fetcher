//! Binance SBE WebSocket Manager — ALL streams via SBE (no JSON fallback)
//!
//! Drop-in replacement for binance_fixed_sbe.rs with the depth20 stream
//! also decoded via SBE binary encoding instead of the JSON fallback.
//!
//! SBE streams used (all on one authenticated WS connection):
//!   - `@bestBidAsk` → bid/ask prices (replaces JSON `@bookTicker`)
//!   - `@trade`      → trade volume (replaces JSON `@aggTrade`)
//!   - `@depth20`    → L2 orderbook depth (20 levels, 50ms updates via SBE)
//!
//! Requires `BINANCE_API_KEY` env var (Ed25519 API key) for SBE endpoint auth.
//! The key is passed via `X-MBX-APIKEY` HTTP header during WebSocket upgrade.
//!
//! All timestamps from SBE are in microseconds; converted to milliseconds
//! for compatibility with existing code.

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::volatility::{FibonacciVolatilityCalculator, MarketSnapshot, VolumeTracker};

// Re-export types from binance_fixed so callers can use them without changing imports
pub use super::binance_fixed::{BinanceDepthData, BinancePrice, Kline};

// ============================================================================
// CONSTANTS
// ============================================================================

/// Binance REST API base URL (same as JSON module — REST is unchanged)
const BINANCE_REST_API_URL: &str = "https://api.binance.com/api/v3";

/// Default timeout for REST API requests
const REST_API_TIMEOUT_SECS: u64 = 10;

/// Maximum klines per request (Binance limit)
const MAX_KLINES_PER_REQUEST: usize = 1000;

/// SBE WebSocket base URL
const SBE_WS_BASE: &str = "wss://stream-sbe.binance.com/ws";

// ============================================================================
// KLINE PARSING HELPER (duplicated since Kline::from_json is private)
// ============================================================================

/// Parse a Kline from Binance REST API JSON array.
/// Binance returns: [open_time, open, high, low, close, volume, close_time,
///                   quote_volume, trades, taker_buy_base, taker_buy_quote, ignore]
fn parse_kline_from_json(arr: &[serde_json::Value]) -> Option<Kline> {
    if arr.len() < 9 {
        return None;
    }
    Some(Kline {
        open_time: arr[0].as_i64()?,
        open: arr[1].as_str()?.parse().ok()?,
        high: arr[2].as_str()?.parse().ok()?,
        low: arr[3].as_str()?.parse().ok()?,
        close: arr[4].as_str()?.parse().ok()?,
        volume: arr[5].as_str()?.parse().ok()?,
        close_time: arr[6].as_i64()?,
        quote_volume: arr[7].as_str()?.parse().ok()?,
        trades: arr[8].as_i64()?,
    })
}

// ============================================================================
// SBE DECODER (inline — extended with DepthSnapshotStreamEvent)
// ============================================================================

/// SBE message header size in bytes.
const SBE_HEADER_SIZE: usize = 8;

/// BestBidAskStreamEvent templateId from stream_1_0.xml schema.
const BEST_BID_ASK_TEMPLATE_ID: u16 = 10001;

/// TradesStreamEvent templateId from stream_1_0.xml schema.
const TRADES_TEMPLATE_ID: u16 = 10002;

/// DepthSnapshotStreamEvent templateId.
/// Schema says 10002, but production trades also use 10002. We use the header's
/// blockLength to distinguish — depth has blockLength=18, trades has 59, BBA has 50.

/// Minimum BBA frame: header (8) + root block (50) + symbol length byte (1).
const MIN_BBA_FRAME_SIZE: usize = SBE_HEADER_SIZE + 50 + 1;

/// Minimum trade frame: header (8) + root block (59) + symbol length byte (1).
const MIN_TRADE_FRAME_SIZE: usize = SBE_HEADER_SIZE + 59 + 1;

/// Root block size for DepthSnapshot: eventTime(8) + bookUpdateId(8) + priceExp(1) + qtyExp(1) = 18
const DEPTH_SNAPSHOT_BLOCK_LENGTH: u16 = 18;

/// Decoded BestBidAskStreamEvent fields.
#[derive(Debug, Clone)]
struct SbeBestBidAsk {
    /// Event timestamp in microseconds since epoch.
    event_time_us: i64,
    /// Best bid price.
    bid_price: Decimal,
    /// Best ask price.
    ask_price: Decimal,
    /// Mid price = (bid + ask) / 2.
    mid_price: Decimal,
    /// Event timestamp converted to milliseconds.
    event_time_ms: i64,
    /// Symbol extracted from the frame.
    symbol: String,
}

/// Decoded TradesStreamEvent fields.
#[derive(Debug, Clone)]
struct SbeTrade {
    /// Event timestamp in microseconds since epoch.
    event_time_us: i64,
    /// Trade price.
    price: Decimal,
    /// Trade quantity.
    quantity: Decimal,
    /// Event timestamp converted to milliseconds.
    event_time_ms: i64,
    /// Symbol extracted from the frame.
    symbol: String,
}

/// A single order book level (price + quantity).
#[derive(Debug, Clone)]
struct DepthLevel {
    qty: f64,
}

/// Decoded DepthSnapshotStreamEvent fields.
#[derive(Debug, Clone)]
struct SbeDepthSnapshot {
    /// Event timestamp in microseconds since epoch.
    event_time_us: i64,
    /// Bid levels (up to 20).
    bids: Vec<DepthLevel>,
    /// Ask levels (up to 20).
    asks: Vec<DepthLevel>,
    /// Symbol extracted from the frame (may be empty if no varString follows).
    symbol: String,
}

/// Result of decoding an SBE frame.
#[derive(Debug)]
enum SbeDecodeResult {
    BestBidAsk(SbeBestBidAsk),
    Trade(SbeTrade),
    DepthSnapshot(SbeDepthSnapshot),
    UnknownTemplate(u16),
    TooShort(usize),
    Invalid(String),
}

/// Reconstruct a Decimal from mantissa + exponent.
/// `value = mantissa * 10^exponent`
#[inline(always)]
fn mantissa_to_decimal(mantissa: i64, exponent: i8) -> Decimal {
    if exponent <= 0 {
        Decimal::new(mantissa, (-exponent) as u32)
    } else {
        let multiplier = 10i64.pow(exponent as u32);
        Decimal::new(mantissa * multiplier, 0)
    }
}

/// Reconstruct an f64 from mantissa + exponent.
/// `value = mantissa * 10^exponent`
#[inline(always)]
fn mantissa_to_f64(mantissa: i64, exponent: i8) -> f64 {
    mantissa as f64 * 10f64.powi(exponent as i32)
}

/// Read an i64 from little-endian bytes at the given offset.
#[inline(always)]
fn read_i64_le(buf: &[u8], offset: usize) -> i64 {
    i64::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ])
}

/// Read a u16 from little-endian bytes at the given offset.
#[inline(always)]
fn read_u16_le(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

/// Read an i8 from the buffer at the given offset.
#[inline(always)]
fn read_i8(buf: &[u8], offset: usize) -> i8 {
    buf[offset] as i8
}

/// Extract symbol string from SBE variable-length field.
/// The field starts with a u8 length byte followed by the symbol bytes.
#[inline]
fn read_var_string(buf: &[u8], offset: usize) -> String {
    if offset >= buf.len() {
        return String::new();
    }
    let len = buf[offset] as usize;
    let start = offset + 1;
    let end = (start + len).min(buf.len());
    String::from_utf8_lossy(&buf[start..end]).to_uppercase()
}

/// Decode a binary SBE frame. Handles BestBidAsk, Trade, and DepthSnapshot events.
///
/// Disambiguation strategy:
/// - templateId=10001 → always BBA
/// - templateId=10002 → use blockLength to distinguish: 59=Trade, 18=DepthSnapshot
/// - templateId=10003 → DepthDiff (decoded as DepthSnapshot since layout is similar)
/// - templateId=10004 → DepthSnapshot (legacy)
/// - Other → UnknownTemplate
#[inline]
fn decode_sbe_frame(buf: &[u8]) -> SbeDecodeResult {
    if buf.len() < SBE_HEADER_SIZE {
        return SbeDecodeResult::TooShort(buf.len());
    }

    let block_length = read_u16_le(buf, 0);
    let template_id = read_u16_le(buf, 2);

    match template_id {
        BEST_BID_ASK_TEMPLATE_ID => decode_bba(buf),
        // templateId 10002: could be Trade OR DepthSnapshot depending on schema version.
        // Disambiguate by blockLength.
        TRADES_TEMPLATE_ID => {
            if block_length == DEPTH_SNAPSHOT_BLOCK_LENGTH {
                // DepthSnapshot with templateId=10002 (per stream_1_0.xml schema)
                decode_depth_snapshot(buf, block_length)
            } else {
                // Trade (blockLength=59)
                decode_trade(buf)
            }
        }
        // DepthDiffStreamEvent (templateId=10003) — similar repeating group structure.
        // We can decode it as a depth snapshot for OBI purposes.
        10003 => {
            if block_length <= DEPTH_SNAPSHOT_BLOCK_LENGTH + 8 {
                // DepthDiff has extra fields (firstBookUpdateId, lastBookUpdateId)
                // but the repeating groups are the same
                decode_depth_diff(buf, block_length)
            } else {
                SbeDecodeResult::UnknownTemplate(template_id)
            }
        }
        // Legacy depth snapshot templateId
        10004 => decode_depth_snapshot(buf, block_length),
        _ => {
            debug!(template_id, block_length, "SBE: skipping unknown message type");
            SbeDecodeResult::UnknownTemplate(template_id)
        }
    }
}

/// Decode BestBidAskStreamEvent from SBE binary frame.
///
/// Root block layout (50 bytes, after 8-byte header):
///   [0..8]   eventTime      i64 — microseconds
///   [8..16]  bookUpdateId   i64
///   [16]     priceExponent  i8
///   [17]     qtyExponent    i8
///   [18..26] bidPrice       i64 — mantissa
///   [26..34] bidQty         i64 — mantissa
///   [34..42] askPrice       i64 — mantissa
///   [42..50] askQty         i64 — mantissa
///   [50]     symbolLength   u8
///   [51..]   symbolData     [u8]
fn decode_bba(buf: &[u8]) -> SbeDecodeResult {
    if buf.len() < MIN_BBA_FRAME_SIZE {
        return SbeDecodeResult::TooShort(buf.len());
    }

    let root = &buf[SBE_HEADER_SIZE..];

    let event_time_us = read_i64_le(root, 0);
    let price_exponent = read_i8(root, 16);
    let bid_price_mantissa = read_i64_le(root, 18);
    let ask_price_mantissa = read_i64_le(root, 34);

    let bid_price = mantissa_to_decimal(bid_price_mantissa, price_exponent);
    let ask_price = mantissa_to_decimal(ask_price_mantissa, price_exponent);

    if bid_price.is_zero() && ask_price.is_zero() {
        return SbeDecodeResult::Invalid("both bid and ask are zero".into());
    }

    let two = Decimal::new(2, 0);
    let mid_price = (bid_price + ask_price) / two;
    let event_time_ms = event_time_us / 1000;

    let symbol = read_var_string(root, 50);

    SbeDecodeResult::BestBidAsk(SbeBestBidAsk {
        event_time_us,
        bid_price,
        ask_price,
        mid_price,
        event_time_ms,
        symbol,
    })
}

/// Decode TradesStreamEvent from SBE binary frame.
///
/// Root block layout (59 bytes, after 8-byte header):
///   [0..8]   eventTime      i64 — microseconds
///   [8..16]  tradeId        i64
///   [16]     priceExponent  i8
///   [17]     qtyExponent    i8
///   [18..26] price          i64 — mantissa
///   [26..34] quantity       i64 — mantissa
///   [34..42] buyerOrderId   i64
///   [42..50] sellerOrderId  i64
///   [50..58] tradeTime      i64 — microseconds
///   [58]     isBuyerMaker   u8
///   [59]     symbolLength   u8
///   [60..]   symbolData     [u8]
fn decode_trade(buf: &[u8]) -> SbeDecodeResult {
    if buf.len() < MIN_TRADE_FRAME_SIZE {
        return SbeDecodeResult::TooShort(buf.len());
    }

    let root = &buf[SBE_HEADER_SIZE..];

    let event_time_us = read_i64_le(root, 0);
    let price_exponent = read_i8(root, 16);
    let qty_exponent = read_i8(root, 17);
    let price_mantissa = read_i64_le(root, 18);
    let qty_mantissa = read_i64_le(root, 26);

    let price = mantissa_to_decimal(price_mantissa, price_exponent);
    let quantity = mantissa_to_decimal(qty_mantissa, qty_exponent);
    let event_time_ms = event_time_us / 1000;

    let symbol = read_var_string(root, 59);

    SbeDecodeResult::Trade(SbeTrade {
        event_time_us,
        price,
        quantity,
        event_time_ms,
        symbol,
    })
}

/// Decode DepthSnapshotStreamEvent from SBE binary frame.
///
/// Root block layout (18 bytes, after 8-byte header):
///   [0..8]   eventTime      i64 — microseconds
///   [8..16]  bookUpdateId   i64
///   [16]     priceExponent  i8
///   [17]     qtyExponent    i8
///
/// Followed by repeating groups:
///   Bids group: groupSize16 header (4 bytes: blockLength u16 + numInGroup u16)
///               then N entries, each of `blockLength` bytes containing:
///                 [0..8] price   i64 — mantissa (uses priceExponent)
///                 [8..16] qty   i64 — mantissa (uses qtyExponent)
///   Asks group: same format as bids
///
/// Followed by variable-length data:
///   symbol: varString8 (u8 length + bytes)
fn decode_depth_snapshot(buf: &[u8], block_length: u16) -> SbeDecodeResult {
    let root_end = SBE_HEADER_SIZE + block_length as usize;
    if buf.len() < root_end + 4 {
        return SbeDecodeResult::TooShort(buf.len());
    }

    let root = &buf[SBE_HEADER_SIZE..];

    let event_time_us = read_i64_le(root, 0);
    let price_exponent = read_i8(root, 16);
    let qty_exponent = read_i8(root, 17);

    // Parse bids repeating group
    let mut cursor = root_end; // absolute position in buf

    if cursor + 4 > buf.len() {
        return SbeDecodeResult::Invalid("depth: truncated bids group header".into());
    }
    let bids_block_length = read_u16_le(buf, cursor) as usize;
    let bids_num = read_u16_le(buf, cursor + 2) as usize;
    cursor += 4;

    // Each bid entry is bids_block_length bytes, containing at minimum price(8) + qty(8) = 16
    let entry_size = if bids_block_length >= 16 { bids_block_length } else { 16 };
    let bids_data_end = cursor + bids_num * entry_size;
    if bids_data_end > buf.len() {
        return SbeDecodeResult::Invalid(format!(
            "depth: truncated bids data (need {}, have {})",
            bids_data_end,
            buf.len()
        ));
    }

    let mut bids = Vec::with_capacity(bids_num.min(20));
    for i in 0..bids_num.min(20) {
        let entry_offset = cursor + i * entry_size;
        let _price = mantissa_to_f64(read_i64_le(buf, entry_offset), price_exponent);
        let qty = mantissa_to_f64(read_i64_le(buf, entry_offset + 8), qty_exponent);
        bids.push(DepthLevel { qty });
    }
    cursor = bids_data_end;

    // Parse asks repeating group
    if cursor + 4 > buf.len() {
        return SbeDecodeResult::Invalid("depth: truncated asks group header".into());
    }
    let asks_block_length = read_u16_le(buf, cursor) as usize;
    let asks_num = read_u16_le(buf, cursor + 2) as usize;
    cursor += 4;

    let ask_entry_size = if asks_block_length >= 16 { asks_block_length } else { 16 };
    let asks_data_end = cursor + asks_num * ask_entry_size;
    if asks_data_end > buf.len() {
        return SbeDecodeResult::Invalid(format!(
            "depth: truncated asks data (need {}, have {})",
            asks_data_end,
            buf.len()
        ));
    }

    let mut asks = Vec::with_capacity(asks_num.min(20));
    for i in 0..asks_num.min(20) {
        let entry_offset = cursor + i * ask_entry_size;
        let _price = mantissa_to_f64(read_i64_le(buf, entry_offset), price_exponent);
        let qty = mantissa_to_f64(read_i64_le(buf, entry_offset + 8), qty_exponent);
        asks.push(DepthLevel { qty });
    }
    cursor = asks_data_end;

    // Try to read symbol from varString8 after the groups
    let symbol = if cursor < buf.len() {
        read_var_string(buf, cursor)
    } else {
        String::new()
    };

    SbeDecodeResult::DepthSnapshot(SbeDepthSnapshot {
        event_time_us,
        bids,
        asks,
        symbol,
    })
}

/// Decode DepthDiffStreamEvent from SBE binary frame.
///
/// Root block layout (26 bytes, after 8-byte header):
///   [0..8]   eventTime         i64 — microseconds
///   [8..16]  firstBookUpdateId i64
///   [16..24] lastBookUpdateId  i64
///   [24]     priceExponent     i8
///   [25]     qtyExponent       i8
///
/// Followed by same repeating groups as DepthSnapshot (bids + asks).
/// We decode it into SbeDepthSnapshot for OBI calculation purposes.
fn decode_depth_diff(buf: &[u8], block_length: u16) -> SbeDecodeResult {
    let root_end = SBE_HEADER_SIZE + block_length as usize;
    if buf.len() < root_end + 4 {
        return SbeDecodeResult::TooShort(buf.len());
    }

    let root = &buf[SBE_HEADER_SIZE..];

    let event_time_us = read_i64_le(root, 0);
    // For DepthDiff, priceExponent and qtyExponent are at different offsets
    // due to the extra firstBookUpdateId and lastBookUpdateId fields
    let _price_exponent = read_i8(root, 24);
    let qty_exponent = read_i8(root, 25);

    // Parse bids repeating group (same structure as depth snapshot)
    let mut cursor = root_end;

    if cursor + 4 > buf.len() {
        return SbeDecodeResult::Invalid("depth_diff: truncated bids group header".into());
    }
    let bids_block_length = read_u16_le(buf, cursor) as usize;
    let bids_num = read_u16_le(buf, cursor + 2) as usize;
    cursor += 4;

    let entry_size = if bids_block_length >= 16 { bids_block_length } else { 16 };
    let bids_data_end = cursor + bids_num * entry_size;
    if bids_data_end > buf.len() {
        return SbeDecodeResult::Invalid(format!(
            "depth_diff: truncated bids data (need {}, have {})",
            bids_data_end,
            buf.len()
        ));
    }

    let mut bids = Vec::with_capacity(bids_num.min(20));
    for i in 0..bids_num.min(20) {
        let entry_offset = cursor + i * entry_size;
        let qty = mantissa_to_f64(read_i64_le(buf, entry_offset + 8), qty_exponent);
        if qty > 0.0 {
            bids.push(DepthLevel { qty });
        }
    }
    cursor = bids_data_end;

    // Parse asks repeating group
    if cursor + 4 > buf.len() {
        return SbeDecodeResult::Invalid("depth_diff: truncated asks group header".into());
    }
    let asks_block_length = read_u16_le(buf, cursor) as usize;
    let asks_num = read_u16_le(buf, cursor + 2) as usize;
    cursor += 4;

    let ask_entry_size = if asks_block_length >= 16 { asks_block_length } else { 16 };
    let asks_data_end = cursor + asks_num * ask_entry_size;
    if asks_data_end > buf.len() {
        return SbeDecodeResult::Invalid(format!(
            "depth_diff: truncated asks data (need {}, have {})",
            asks_data_end,
            buf.len()
        ));
    }

    let mut asks = Vec::with_capacity(asks_num.min(20));
    for i in 0..asks_num.min(20) {
        let entry_offset = cursor + i * ask_entry_size;
        let qty = mantissa_to_f64(read_i64_le(buf, entry_offset + 8), qty_exponent);
        if qty > 0.0 {
            asks.push(DepthLevel { qty });
        }
    }
    cursor = asks_data_end;

    let symbol = if cursor < buf.len() {
        read_var_string(buf, cursor)
    } else {
        String::new()
    };

    SbeDecodeResult::DepthSnapshot(SbeDepthSnapshot {
        event_time_us,
        bids,
        asks,
        symbol,
    })
}

// ============================================================================
// BINANCE DATA MANAGER (SBE + OBI)
// ============================================================================

/// Binance SBE WebSocket Data Manager — ALL streams via SBE
///
/// Drop-in replacement for `binance_fixed::BinanceDataManager` and
/// `binance_fixed_sbe::BinanceDataManager`. Uses SBE binary encoding
/// for ALL streams including depth20 (no JSON fallback).
pub struct BinanceDataManager {
    /// Thread-safe price storage: symbol -> BinancePrice
    prices: Arc<RwLock<HashMap<String, BinancePrice>>>,
    /// Connected symbols
    symbols: Vec<String>,
    /// Whether a reconnection is in progress (SBE)
    reconnecting_sbe: Arc<RwLock<bool>>,
    /// Volatility calculators per symbol: symbol -> calculator
    vol_calculators: HashMap<String, Arc<RwLock<FibonacciVolatilityCalculator>>>,
    /// Volume trackers per symbol: symbol -> tracker
    volume_trackers: HashMap<String, Arc<Mutex<VolumeTracker>>>,
    /// Latest event times per symbol: symbol -> event_time_ms
    event_times: Arc<RwLock<HashMap<String, u64>>>,
    /// Thread-safe depth data storage: symbol -> BinanceDepthData
    depth_data: Arc<RwLock<HashMap<String, BinanceDepthData>>>,
    /// HTTP client for REST API calls (reusable)
    http_client: reqwest::Client,
}

impl BinanceDataManager {
    /// Create a new BinanceDataManager with default configuration
    pub fn new() -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REST_API_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            prices: Arc::new(RwLock::new(HashMap::new())),
            symbols: Vec::new(),
            reconnecting_sbe: Arc::new(RwLock::new(false)),
            vol_calculators: HashMap::new(),
            volume_trackers: HashMap::new(),
            event_times: Arc::new(RwLock::new(HashMap::new())),
            depth_data: Arc::new(RwLock::new(HashMap::new())),
            http_client,
        }
    }

    // ========================================================================
    // REST API METHODS (Historical Data Bootstrap) — identical to binance_fixed
    // ========================================================================

    /// Fetch historical klines from Binance REST API
    pub async fn fetch_klines(
        &self,
        symbol: &str,
        interval: &str,
        limit: usize,
    ) -> Result<Vec<Kline>> {
        let limit = limit.min(MAX_KLINES_PER_REQUEST);
        let upper_symbol = symbol.to_uppercase();

        let url = format!(
            "{}/klines?symbol={}&interval={}&limit={}",
            BINANCE_REST_API_URL, upper_symbol, interval, limit
        );

        debug!("Fetching klines: {}", url);

        let resp = self.http_client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Err(anyhow!(
                "Kline fetch failed for {}: HTTP {}",
                upper_symbol,
                resp.status()
            ));
        }

        let data: Vec<Vec<serde_json::Value>> = resp.json().await?;

        let klines: Vec<Kline> = data
            .iter()
            .filter_map(|arr| parse_kline_from_json(arr))
            .collect();

        debug!(
            "Fetched {} klines for {} (requested {})",
            klines.len(),
            upper_symbol,
            limit
        );

        Ok(klines)
    }

    /// Bootstrap volatility calculator with historical 1-minute klines
    pub async fn bootstrap_volatility(&self, symbol: &str, limit: usize) -> Result<usize> {
        let upper_symbol = symbol.to_uppercase();
        let limit = limit.min(MAX_KLINES_PER_REQUEST);

        info!(
            "Bootstrapping volatility for {} with {} 1m klines (SBE+OBI module)...",
            upper_symbol, limit
        );

        let klines = self.fetch_klines(&upper_symbol, "1m", limit).await?;

        if klines.is_empty() {
            return Err(anyhow!("No klines returned for {}", upper_symbol));
        }

        let vol_calculator = self.vol_calculators.get(&upper_symbol).ok_or_else(|| {
            anyhow!(
                "No volatility calculator for {}. Call connect() first.",
                upper_symbol
            )
        })?;

        let mut calc = vol_calculator.write().await;
        let mut loaded = 0;

        for kline in &klines {
            if kline.close > 0.0 {
                calc.add_price_1min(kline.close);
                loaded += 1;
            }
        }

        // Seed real-time buffer with recent kline closes
        let recent_klines = klines.iter().rev().take(5);
        for kline in recent_klines {
            if kline.close > 0.0 {
                let ts = if kline.close_time > 1_000_000_000_000 {
                    kline.close_time
                } else {
                    kline.close_time * 1000
                };
                calc.add_price(ts, kline.close);
            }
        }

        info!(
            "Bootstrapped {} (SBE+OBI) with {} klines | 1min buffer: {} | unified buffer: {}",
            upper_symbol,
            loaded,
            calc.get_1min_buffer_size(),
            calc.get_unified_buffer_size()
        );

        Ok(loaded)
    }

    /// Bootstrap volatility for all connected symbols
    pub async fn bootstrap_all_volatility(
        &self,
        limit: usize,
    ) -> HashMap<String, Result<usize, String>> {
        let mut results = HashMap::new();

        for symbol in &self.symbols {
            match self.bootstrap_volatility(symbol, limit).await {
                Ok(count) => {
                    results.insert(symbol.clone(), Ok(count));
                }
                Err(e) => {
                    warn!("Failed to bootstrap {}: {}", symbol, e);
                    results.insert(symbol.clone(), Err(e.to_string()));
                }
            }
        }

        results
    }

    // ========================================================================
    // WEBSOCKET CONNECTION
    // ========================================================================

    /// Connect to Binance WebSocket and subscribe to symbols
    ///
    /// Uses SBE binary endpoint for ALL streams (bestBidAsk + trade + depth20).
    /// No JSON fallback — everything through one authenticated SBE connection.
    ///
    /// # Arguments
    /// * `symbols` - Array of symbols to subscribe (e.g., ["BTCUSDT", "ETHUSDT"])
    /// * `_streams_url` - Ignored (kept for API compatibility). SBE uses its own endpoint.
    pub async fn connect(&mut self, symbols: &[&str], _streams_url: Option<String>) -> Result<()> {
        if symbols.is_empty() {
            return Err(anyhow!("Cannot connect: no symbols provided"));
        }

        self.symbols = symbols.iter().map(|s| s.to_uppercase()).collect();

        // Create per-symbol volatility calculators and volume trackers
        for symbol in &self.symbols {
            self.vol_calculators.insert(
                symbol.clone(),
                Arc::new(RwLock::new(FibonacciVolatilityCalculator::new(symbol))),
            );
            self.volume_trackers
                .insert(symbol.clone(), Arc::new(Mutex::new(VolumeTracker::new())));
        }

        // Read API key from environment, fallback to hardcoded key
        let api_key = std::env::var("BINANCE_API_KEY").unwrap_or_else(|_| {
            const FALLBACK_KEY: &str = "[redacted]";
            warn!("BINANCE_API_KEY not set — using hardcoded fallback key");
            FALLBACK_KEY.to_string()
        });

        // Build SBE combined stream URL: bestBidAsk + trade + depth20 for all symbols
        let sbe_streams: Vec<String> = self
            .symbols
            .iter()
            .flat_map(|s| {
                let lower = s.to_lowercase();
                vec![
                    format!("{}@bestBidAsk", lower),
                    format!("{}@trade", lower),
                    format!("{}@depth20", lower),
                ]
            })
            .collect();
        let sbe_url = format!("{}/{}", SBE_WS_BASE, sbe_streams.join("/"));

        info!(
            "Connecting to Binance SBE WebSocket (bestBidAsk+trade+depth20): {}",
            sbe_url
        );

        // Spawn SBE connection task (all streams combined on one connection)
        let prices = self.prices.clone();
        let reconnecting_sbe = self.reconnecting_sbe.clone();
        let vol_calculators = self.vol_calculators.clone();
        let volume_trackers = self.volume_trackers.clone();
        let event_times = self.event_times.clone();
        let depth_data = self.depth_data.clone();
        let api_key_clone = api_key.clone();
        let symbols_clone = self.symbols.clone();

        tokio::spawn(async move {
            Self::sbe_connection_loop(
                sbe_url,
                prices,
                reconnecting_sbe,
                vol_calculators,
                volume_trackers,
                event_times,
                depth_data,
                api_key_clone,
                symbols_clone,
            )
            .await
        });

        Ok(())
    }

    // ========================================================================
    // PRICE/DATA ACCESS METHODS — identical API to binance_fixed
    // ========================================================================

    /// Get the current price for a symbol
    pub async fn get_price(&self, symbol: &str) -> Option<BinancePrice> {
        let prices = self.prices.read().await;
        prices.get(&symbol.to_uppercase()).cloned()
    }

    /// Get bid and ask prices separately for spread calculation
    pub async fn get_bid_ask(&self, symbol: &str) -> Option<(f64, f64)> {
        let prices = self.prices.read().await;
        prices
            .get(&symbol.to_uppercase())
            .map(|p| (p.bid.to_f64().unwrap_or(0.0), p.ask.to_f64().unwrap_or(0.0)))
    }

    /// Get all current prices
    pub async fn get_all_prices(&self) -> HashMap<String, BinancePrice> {
        self.prices.read().await.clone()
    }

    /// Check if a symbol has price data
    pub async fn has_price(&self, symbol: &str) -> bool {
        let prices = self.prices.read().await;
        prices.contains_key(&symbol.to_uppercase())
    }

    /// Get rolling window volume for a symbol
    pub async fn get_rolling_volume(&self, symbol: &str) -> Option<(f64, f64)> {
        let upper_symbol = symbol.to_uppercase();
        let volume_tracker = self.volume_trackers.get(&upper_symbol)?;

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let tracker = volume_tracker.lock().await;
        Some((
            tracker.get_volume_1sec(current_time),
            tracker.get_volume_1min(),
        ))
    }

    /// Check if volatility calculator is ready (has enough data)
    pub async fn is_volatility_ready(&self, symbol: &str) -> Option<bool> {
        let upper_symbol = symbol.to_uppercase();
        let vol_calculator = self.vol_calculators.get(&upper_symbol)?;
        let calc = vol_calculator.read().await;
        Some(calc.is_bootstrapped())
    }

    /// Get the volatility source path for a symbol
    pub async fn get_vol_source_path(&self, symbol: &str) -> Option<String> {
        let upper_symbol = symbol.to_uppercase();
        let vol_calculator = self.vol_calculators.get(&upper_symbol)?;
        let calc = vol_calculator.read().await;
        Some(calc.get_vol_source_path().to_string())
    }

    /// Get market snapshot including realized volatility for a symbol
    pub async fn get_market_snapshot(&self, symbol: &str) -> Option<MarketSnapshot> {
        let upper_symbol = symbol.to_uppercase();

        let price_data = {
            let prices = self.prices.read().await;
            prices.get(&upper_symbol).cloned()?
        };

        let spot_price = price_data.mid.to_f64().unwrap_or(0.0);

        let current_time = {
            let event_times = self.event_times.read().await;
            event_times.get(&upper_symbol).copied().unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
            }) as i64
        };

        let vol_calculator = self.vol_calculators.get(&upper_symbol)?;
        let (vol_1sec, vol_5sec, vol_13sec, vol_34sec, vol_89sec, realized_vol, vol_source_path, volatility_regime) = {
            let mut vol_calc_clone = {
                let guard = vol_calculator.read().await;
                (*guard).clone()
            };
            let result = vol_calc_clone.calculate_all(current_time);
            let source_path = vol_calc_clone.get_vol_source_path().to_string();
            let regime = vol_calc_clone.get_volatility_regime();
            (result.0, result.1, result.2, result.3, result.4, result.5, source_path, regime)
        };

        let volume_tracker = self.volume_trackers.get(&upper_symbol)?;
        let (volume_1sec, volume_1min, volume_spike) = {
            let vol_tracker = volume_tracker.lock().await;
            (
                vol_tracker.get_volume_1sec(current_time),
                vol_tracker.get_volume_1min(),
                vol_tracker.is_volume_spike(current_time),
            )
        };

        let spread_bps = if price_data.ask > price_data.bid && price_data.bid > Decimal::from(0) {
            let spread = (price_data.ask - price_data.bid).to_f64().unwrap_or(0.0);
            let mid = price_data.mid.to_f64().unwrap_or(1.0);
            (spread / mid) * 10000.0
        } else {
            0.0
        };

        Some(MarketSnapshot {
            timestamp: current_time,
            symbol: upper_symbol,
            spot_price,
            realized_volatility: realized_vol,
            vol_1sec,
            vol_5sec,
            vol_13sec,
            vol_34sec,
            vol_89sec,
            spread_bps,
            volume_1sec,
            volume_1min,
            volume_spike,
            signal_quality: if realized_vol > 0.5 { "GOOD" } else { "LOW" }.to_string(),
            trade_confidence: if realized_vol > 0.5 { 0.8 } else { 0.5 },
            volatility_regime,
            vol_source_path,
        })
    }

    /// Get the latest depth data (OBI + book levels) for a symbol
    pub async fn get_depth_data(&self, symbol: &str) -> Option<BinanceDepthData> {
        let depth = self.depth_data.read().await;
        depth.get(&symbol.to_uppercase()).cloned()
    }

    // ========================================================================
    // SBE CONNECTION LOOP (bestBidAsk + trade + depth20 — ALL on one connection)
    // ========================================================================

    /// Main SBE connection loop with auto-reconnect.
    /// Handles bestBidAsk, trade, AND depth20 streams on a single SBE connection.
    async fn sbe_connection_loop(
        url: String,
        prices: Arc<RwLock<HashMap<String, BinancePrice>>>,
        reconnecting: Arc<RwLock<bool>>,
        vol_calculators: HashMap<String, Arc<RwLock<FibonacciVolatilityCalculator>>>,
        volume_trackers: HashMap<String, Arc<Mutex<VolumeTracker>>>,
        event_times: Arc<RwLock<HashMap<String, u64>>>,
        depth_data: Arc<RwLock<HashMap<String, BinanceDepthData>>>,
        api_key: String,
        symbols: Vec<String>,
    ) {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::http::HeaderValue;

        let mut reconnect_delay = tokio::time::Duration::from_secs(1);
        const MAX_RECONNECT_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(30);
        let mut bba_count: u64 = 0;
        let mut trade_count: u64 = 0;
        let mut depth_count: u64 = 0;

        // Fallback symbol for depth when symbol field is empty
        let fallback_symbol = symbols.first().cloned().unwrap_or_default();

        loop {
            {
                let mut reconnecting_guard = reconnecting.write().await;
                if *reconnecting_guard {
                    debug!("SBE reconnection already in progress, waiting to retry...");
                    drop(reconnecting_guard);
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
                *reconnecting_guard = true;
            }

            // Build request with API key header
            let connect_result = {
                let request_result = url.as_str().into_client_request();
                match request_result {
                    Ok(mut request) => {
                        if !api_key.is_empty() {
                            if let Ok(val) = HeaderValue::from_str(&api_key) {
                                request.headers_mut().insert("X-MBX-APIKEY", val);
                            } else {
                                error!("BINANCE_SBE_OBI: invalid API key header value");
                            }
                        }
                        connect_async(request).await
                    }
                    Err(e) => {
                        error!("BINANCE_SBE_OBI: failed to build WS request: {}", e);
                        {
                            let mut reconnecting_guard = reconnecting.write().await;
                            *reconnecting_guard = false;
                        }
                        warn!("Reconnecting SBE+OBI in {:?}...", reconnect_delay);
                        tokio::time::sleep(reconnect_delay).await;
                        reconnect_delay = std::cmp::min(reconnect_delay * 2, MAX_RECONNECT_DELAY);
                        continue;
                    }
                }
            };

            match connect_result {
                Ok((ws_stream, _)) => {
                    reconnect_delay = tokio::time::Duration::from_secs(1);
                    info!("BINANCE_SBE_OBI: connected to SBE endpoint (bestBidAsk+trade+depth20)");

                    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
                    let mut decode_errors: u64 = 0;

                    while let Some(msg_result) = ws_receiver.next().await {
                        match msg_result {
                            Ok(Message::Binary(bytes)) => {
                                // SBE binary frame — the hot path
                                match decode_sbe_frame(&bytes) {
                                    SbeDecodeResult::BestBidAsk(bba) => {
                                        // Update event time (SBE gives us exchange time in us)
                                        {
                                            let mut et = event_times.write().await;
                                            et.insert(bba.symbol.clone(), bba.event_time_ms as u64);
                                        }

                                        // Update volatility calculator
                                        let mid_f64 = bba.mid_price.to_f64().unwrap_or(0.0);
                                        if mid_f64 > 0.0 {
                                            if let Some(vc) = vol_calculators.get(&bba.symbol) {
                                                let mut calc = vc.write().await;
                                                calc.add_price(bba.event_time_ms as i64, mid_f64);
                                            }
                                        }

                                        // Update price store
                                        {
                                            let mut prices_guard = prices.write().await;
                                            let volume = prices_guard
                                                .get(&bba.symbol)
                                                .map(|p| p.volume)
                                                .unwrap_or_else(|| Decimal::from(0));
                                            let price = BinancePrice::new(
                                                bba.bid_price,
                                                bba.ask_price,
                                                volume,
                                            );
                                            prices_guard.insert(bba.symbol.clone(), price);
                                        }

                                        bba_count += 1;
                                        if bba_count == 1 {
                                            info!(
                                                "BINANCE_SBE_OBI: first bestBidAsk: bid={} ask={} mid={} event_us={}",
                                                bba.bid_price, bba.ask_price, bba.mid_price, bba.event_time_us
                                            );
                                        }
                                        if bba_count % 10000 == 0 {
                                            debug!(
                                                "BINANCE_SBE_OBI: #{} BBA updates, {} trades, {} depth",
                                                bba_count, trade_count, depth_count
                                            );
                                        }
                                    }
                                    SbeDecodeResult::Trade(trade) => {
                                        // Update event time
                                        {
                                            let mut et = event_times.write().await;
                                            et.insert(trade.symbol.clone(), trade.event_time_ms as u64);
                                        }

                                        // Update volume tracker
                                        let trade_volume = trade.price * trade.quantity;
                                        let trade_volume_f64 = trade_volume.to_f64().unwrap_or(0.0);
                                        if trade_volume_f64 > 0.0 {
                                            if let Some(vt) = volume_trackers.get(&trade.symbol) {
                                                let mut tracker = vt.lock().await;
                                                tracker.add_trade(
                                                    trade.event_time_ms as i64,
                                                    trade_volume_f64,
                                                );
                                            }
                                        }

                                        trade_count += 1;
                                        if trade_count == 1 {
                                            info!(
                                                "BINANCE_SBE_OBI: first trade: price={} qty={} vol={:.2} event_us={}",
                                                trade.price, trade.quantity, trade_volume_f64, trade.event_time_us
                                            );
                                        }
                                    }
                                    SbeDecodeResult::DepthSnapshot(depth) => {
                                        // Determine symbol — use depth.symbol if available,
                                        // otherwise fall back to inferring from subscription order
                                        let sym = if depth.symbol.is_empty() {
                                            fallback_symbol.clone()
                                        } else {
                                            depth.symbol.clone()
                                        };

                                        // Convert to BinanceDepthData (same format as JSON)
                                        let bid_quantities: Vec<f64> = depth
                                            .bids
                                            .iter()
                                            .map(|l| l.qty)
                                            .filter(|q| *q > 0.0)
                                            .collect();
                                        let ask_quantities: Vec<f64> = depth
                                            .asks
                                            .iter()
                                            .map(|l| l.qty)
                                            .filter(|q| *q > 0.0)
                                            .collect();

                                        let bid_depth_3: f64 = bid_quantities.iter().take(3).sum();
                                        let ask_depth_3: f64 = ask_quantities.iter().take(3).sum();
                                        let bid_depth_all: f64 = bid_quantities.iter().sum();
                                        let ask_depth_all: f64 = ask_quantities.iter().sum();

                                        let total = bid_depth_all + ask_depth_all;
                                        let book_imbalance = if total > 0.0 {
                                            (bid_depth_all - ask_depth_all) / total
                                        } else {
                                            0.0
                                        };

                                        let now_ms = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .as_millis()
                                            as u64;

                                        let data = BinanceDepthData {
                                            bid_depth_3,
                                            ask_depth_3,
                                            bid_depth_all,
                                            ask_depth_all,
                                            bid_levels: bid_quantities.len() as u16,
                                            ask_levels: ask_quantities.len() as u16,
                                            book_imbalance,
                                            last_update_ms: now_ms,
                                        };

                                        {
                                            let mut depth_guard = depth_data.write().await;
                                            depth_guard.insert(sym.clone(), data);
                                        }

                                        depth_count += 1;
                                        if depth_count == 1 {
                                            info!(
                                                "BINANCE_SBE_OBI: first depth20: symbol={} bids={} asks={} OBI={:.4} event_us={}",
                                                sym,
                                                bid_quantities.len(),
                                                ask_quantities.len(),
                                                book_imbalance,
                                                depth.event_time_us,
                                            );
                                        }
                                        if depth_count % 1000 == 0 {
                                            debug!(
                                                "BINANCE_SBE_OBI: #{} depth updates for {} (OBI={:.4})",
                                                depth_count, sym, book_imbalance,
                                            );
                                        }
                                    }
                                    SbeDecodeResult::UnknownTemplate(id) => {
                                        debug!("BINANCE_SBE_OBI: unknown templateId {}", id);
                                    }
                                    SbeDecodeResult::TooShort(len) => {
                                        decode_errors += 1;
                                        if decode_errors <= 5 {
                                            warn!("BINANCE_SBE_OBI: frame too short ({} bytes)", len);
                                        }
                                    }
                                    SbeDecodeResult::Invalid(reason) => {
                                        decode_errors += 1;
                                        if decode_errors <= 10 {
                                            warn!("BINANCE_SBE_OBI: invalid frame: {}", reason);
                                        }
                                    }
                                }
                            }
                            Ok(Message::Text(json)) => {
                                // JSON text frames are subscription ACK / error responses
                                debug!("BINANCE_SBE_OBI: control message: {}", json);
                            }
                            Ok(Message::Ping(data)) => {
                                if let Err(e) = ws_sender.send(Message::Pong(data)).await {
                                    error!("BINANCE_SBE_OBI: failed to send pong: {}", e);
                                    break;
                                }
                            }
                            Ok(Message::Close(_)) => {
                                warn!("BINANCE_SBE_OBI: server sent close frame");
                                break;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                error!("BINANCE_SBE_OBI: WebSocket error: {}", e);
                                break;
                            }
                        }
                    }

                    if decode_errors > 0 {
                        warn!(
                            "BINANCE_SBE_OBI: session had {} decode errors, {} BBA, {} trades, {} depth",
                            decode_errors, bba_count, trade_count, depth_count
                        );
                    }
                }
                Err(e) => {
                    error!("BINANCE_SBE_OBI: failed to connect: {}", e);
                }
            }

            {
                let mut reconnecting_guard = reconnecting.write().await;
                *reconnecting_guard = false;
            }

            warn!("Reconnecting SBE+OBI in {:?}...", reconnect_delay);
            tokio::time::sleep(reconnect_delay).await;
            reconnect_delay = std::cmp::min(reconnect_delay * 2, MAX_RECONNECT_DELAY);
        }
    }
}

impl Default for BinanceDataManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic BestBidAskStreamEvent binary frame.
    fn build_bba_frame(
        bid_mantissa: i64,
        ask_mantissa: i64,
        price_exp: i8,
        event_time_us: i64,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(72);

        // Header (8 bytes)
        buf.extend_from_slice(&50u16.to_le_bytes()); // blockLength = 50
        buf.extend_from_slice(&BEST_BID_ASK_TEMPLATE_ID.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // schemaId
        buf.extend_from_slice(&0u16.to_le_bytes()); // version

        // Root block (50 bytes)
        buf.extend_from_slice(&event_time_us.to_le_bytes()); // eventTime
        buf.extend_from_slice(&1i64.to_le_bytes()); // bookUpdateId
        buf.push(price_exp as u8); // priceExponent
        buf.push((-8i8) as u8); // qtyExponent
        buf.extend_from_slice(&bid_mantissa.to_le_bytes()); // bidPrice mantissa
        buf.extend_from_slice(&100_000_000i64.to_le_bytes()); // bidQty mantissa (1.0)
        buf.extend_from_slice(&ask_mantissa.to_le_bytes()); // askPrice mantissa
        buf.extend_from_slice(&200_000_000i64.to_le_bytes()); // askQty mantissa (2.0)

        // varString8 symbol
        buf.push(7); // length
        buf.extend_from_slice(b"BTCUSDT");

        buf
    }

    /// Build a synthetic TradesStreamEvent binary frame.
    fn build_trade_frame(
        price_mantissa: i64,
        qty_mantissa: i64,
        price_exp: i8,
        qty_exp: i8,
        event_time_us: i64,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(80);

        // Header (8 bytes)
        buf.extend_from_slice(&59u16.to_le_bytes()); // blockLength = 59
        buf.extend_from_slice(&TRADES_TEMPLATE_ID.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // schemaId
        buf.extend_from_slice(&0u16.to_le_bytes()); // version

        // Root block (59 bytes)
        buf.extend_from_slice(&event_time_us.to_le_bytes()); // eventTime
        buf.extend_from_slice(&12345i64.to_le_bytes()); // tradeId
        buf.push(price_exp as u8); // priceExponent
        buf.push(qty_exp as u8); // qtyExponent
        buf.extend_from_slice(&price_mantissa.to_le_bytes()); // price mantissa
        buf.extend_from_slice(&qty_mantissa.to_le_bytes()); // quantity mantissa
        buf.extend_from_slice(&100i64.to_le_bytes()); // buyerOrderId
        buf.extend_from_slice(&200i64.to_le_bytes()); // sellerOrderId
        buf.extend_from_slice(&event_time_us.to_le_bytes()); // tradeTime
        buf.push(0u8); // isBuyerMaker = false

        // varString8 symbol
        buf.push(7); // length
        buf.extend_from_slice(b"BTCUSDT");

        buf
    }

    /// Build a synthetic DepthSnapshotStreamEvent binary frame.
    fn build_depth_frame(
        num_bids: u16,
        num_asks: u16,
        price_exp: i8,
        qty_exp: i8,
        event_time_us: i64,
        template_id: u16,
    ) -> Vec<u8> {
        let block_length: u16 = DEPTH_SNAPSHOT_BLOCK_LENGTH;
        let entry_block_length: u16 = 16; // price(8) + qty(8)

        // Calculate total size
        let bids_size = 4 + (num_bids as usize) * (entry_block_length as usize);
        let asks_size = 4 + (num_asks as usize) * (entry_block_length as usize);
        let total = 8 + (block_length as usize) + bids_size + asks_size + 1 + 7; // +symbol
        let mut buf = Vec::with_capacity(total);

        // Header (8 bytes)
        buf.extend_from_slice(&block_length.to_le_bytes());
        buf.extend_from_slice(&template_id.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // schemaId
        buf.extend_from_slice(&0u16.to_le_bytes()); // version

        // Root block (18 bytes)
        buf.extend_from_slice(&event_time_us.to_le_bytes()); // eventTime
        buf.extend_from_slice(&42i64.to_le_bytes()); // bookUpdateId
        buf.push(price_exp as u8); // priceExponent
        buf.push(qty_exp as u8); // qtyExponent

        // Bids group header (4 bytes)
        buf.extend_from_slice(&entry_block_length.to_le_bytes()); // entry blockLength
        buf.extend_from_slice(&num_bids.to_le_bytes()); // numInGroup

        // Bid entries
        for i in 0..num_bids {
            // price mantissa (descending from 7064680)
            let price = 7064680i64 - (i as i64);
            buf.extend_from_slice(&price.to_le_bytes());
            // qty mantissa (increasing: 100, 200, 300, ...)
            let qty = ((i as i64) + 1) * 100_000_000;
            buf.extend_from_slice(&qty.to_le_bytes());
        }

        // Asks group header (4 bytes)
        buf.extend_from_slice(&entry_block_length.to_le_bytes()); // entry blockLength
        buf.extend_from_slice(&num_asks.to_le_bytes()); // numInGroup

        // Ask entries
        for i in 0..num_asks {
            // price mantissa (ascending from 7064681)
            let price = 7064681i64 + (i as i64);
            buf.extend_from_slice(&price.to_le_bytes());
            // qty mantissa (increasing: 50, 100, 150, ...)
            let qty = ((i as i64) + 1) * 50_000_000;
            buf.extend_from_slice(&qty.to_le_bytes());
        }

        // varString8 symbol
        buf.push(7); // length
        buf.extend_from_slice(b"BTCUSDT");

        buf
    }

    #[test]
    fn test_decode_bba_basic() {
        let frame = build_bba_frame(7064677, 7064678, -2, 1_700_000_000_000_000);

        match decode_sbe_frame(&frame) {
            SbeDecodeResult::BestBidAsk(bba) => {
                assert_eq!(bba.bid_price, Decimal::new(7064677, 2));
                assert_eq!(bba.ask_price, Decimal::new(7064678, 2));
                assert_eq!(bba.event_time_ms, 1_700_000_000_000);
                assert_eq!(bba.event_time_us, 1_700_000_000_000_000);
                assert_eq!(bba.mid_price, Decimal::new(70646775, 3));
                assert_eq!(bba.symbol, "BTCUSDT");
            }
            other => panic!("expected BestBidAsk, got {:?}", other),
        }
    }

    #[test]
    fn test_decode_bba_exponent_minus_8() {
        let frame = build_bba_frame(7064677000000, 7064678000000, -8, 1_700_000_000_000_000);

        match decode_sbe_frame(&frame) {
            SbeDecodeResult::BestBidAsk(bba) => {
                assert_eq!(bba.bid_price.to_string(), "70646.77000000");
                assert_eq!(bba.ask_price.to_string(), "70646.78000000");
            }
            other => panic!("expected BestBidAsk, got {:?}", other),
        }
    }

    #[test]
    fn test_decode_trade_basic() {
        let frame = build_trade_frame(7064677, 100_000_000, -2, -8, 1_700_000_000_000_000);

        match decode_sbe_frame(&frame) {
            SbeDecodeResult::Trade(trade) => {
                assert_eq!(trade.price, Decimal::new(7064677, 2));
                assert_eq!(trade.quantity, Decimal::new(100_000_000, 8));
                assert_eq!(trade.event_time_ms, 1_700_000_000_000);
                assert_eq!(trade.symbol, "BTCUSDT");
            }
            other => panic!("expected Trade, got {:?}", other),
        }
    }

    #[test]
    fn test_decode_depth_snapshot() {
        // 5 bids, 3 asks, price_exp=-2, qty_exp=-8
        let frame = build_depth_frame(5, 3, -2, -8, 1_700_000_000_000_000, 10004);

        match decode_sbe_frame(&frame) {
            SbeDecodeResult::DepthSnapshot(depth) => {
                assert_eq!(depth.bids.len(), 5);
                assert_eq!(depth.asks.len(), 3);
                assert_eq!(depth.symbol, "BTCUSDT");
                assert_eq!(depth.event_time_us, 1_700_000_000_000_000);
                // First bid qty: 1 * 100_000_000 * 10^(-8) = 1.0
                assert!((depth.bids[0].qty - 1.0).abs() < 0.001);
                // Second bid qty: 2 * 100_000_000 * 10^(-8) = 2.0
                assert!((depth.bids[1].qty - 2.0).abs() < 0.001);
                // First ask qty: 1 * 50_000_000 * 10^(-8) = 0.5
                assert!((depth.asks[0].qty - 0.5).abs() < 0.001);
            }
            other => panic!("expected DepthSnapshot, got {:?}", other),
        }
    }

    #[test]
    fn test_decode_depth_snapshot_templateid_10002() {
        // Test that templateId=10002 with blockLength=18 is decoded as depth, not trade
        let frame = build_depth_frame(3, 3, -2, -8, 1_700_000_000_000_000, 10002);

        match decode_sbe_frame(&frame) {
            SbeDecodeResult::DepthSnapshot(depth) => {
                assert_eq!(depth.bids.len(), 3);
                assert_eq!(depth.asks.len(), 3);
            }
            other => panic!("expected DepthSnapshot (via templateId=10002 + blockLength=18), got {:?}", other),
        }
    }

    #[test]
    fn test_decode_depth_empty() {
        // 0 bids, 0 asks
        let frame = build_depth_frame(0, 0, -2, -8, 1_700_000_000_000_000, 10004);

        match decode_sbe_frame(&frame) {
            SbeDecodeResult::DepthSnapshot(depth) => {
                assert_eq!(depth.bids.len(), 0);
                assert_eq!(depth.asks.len(), 0);
            }
            other => panic!("expected DepthSnapshot with empty book, got {:?}", other),
        }
    }

    #[test]
    fn test_decode_depth_20_levels() {
        // Full 20/20 levels
        let frame = build_depth_frame(20, 20, -8, -8, 1_700_000_000_000_000, 10004);

        match decode_sbe_frame(&frame) {
            SbeDecodeResult::DepthSnapshot(depth) => {
                assert_eq!(depth.bids.len(), 20);
                assert_eq!(depth.asks.len(), 20);
            }
            other => panic!("expected DepthSnapshot with 20 levels, got {:?}", other),
        }
    }

    #[test]
    fn test_depth_obi_calculation() {
        // 3 bids with qty [1.0, 2.0, 3.0] and 3 asks with qty [0.5, 1.0, 1.5]
        let frame = build_depth_frame(3, 3, -2, -8, 1_700_000_000_000_000, 10004);

        match decode_sbe_frame(&frame) {
            SbeDecodeResult::DepthSnapshot(depth) => {
                let bid_qty_sum: f64 = depth.bids.iter().map(|l| l.qty).sum();
                let ask_qty_sum: f64 = depth.asks.iter().map(|l| l.qty).sum();
                let total = bid_qty_sum + ask_qty_sum;
                let obi = (bid_qty_sum - ask_qty_sum) / total;
                // bid_total = 1+2+3 = 6.0, ask_total = 0.5+1.0+1.5 = 3.0
                // OBI = (6-3)/(6+3) = 0.333...
                assert!((obi - 1.0 / 3.0).abs() < 0.01);
            }
            other => panic!("expected DepthSnapshot, got {:?}", other),
        }
    }

    #[test]
    fn test_too_short() {
        let buf = vec![0u8; 4];
        assert!(matches!(
            decode_sbe_frame(&buf),
            SbeDecodeResult::TooShort(4)
        ));
    }

    #[test]
    fn test_unknown_template() {
        let mut buf = vec![0u8; 60];
        buf[2] = 0x0F;
        buf[3] = 0x27; // 9999 in little-endian
        assert!(matches!(
            decode_sbe_frame(&buf),
            SbeDecodeResult::UnknownTemplate(9999)
        ));
    }

    #[tokio::test]
    async fn test_sbe_obi_binance_data_manager_new() {
        let manager = BinanceDataManager::new();
        assert!(!manager.has_price("BTCUSDT").await);
        assert!(manager.get_price("BTCUSDT").await.is_none());
        assert!(manager.get_depth_data("BTCUSDT").await.is_none());
    }

    #[tokio::test]
    async fn test_sbe_obi_binance_data_manager_default() {
        let manager = BinanceDataManager::default();
        assert!(!manager.has_price("BTCUSDT").await);
    }
}
