//! RTDS Price Tracker for Polymarket latency arbitrage
//!
//! Subscribes to Polymarket's RTDS (Real-Time Data Stream) to capture the
//! EXACT price feeds that Polymarket uses to price its YES/NO books:
//!   - `crypto_prices` (Binance relay) — Polymarket's view of Binance
//!   - `crypto_prices_chainlink` — Chainlink oracle aggregation (~60% Binance weight)
//!
//! ## Clock Safety
//!
//! There are THREE independent clocks in this system:
//!   1. Your local machine (SystemTime::now)
//!   2. Polymarket's server (WS message timestamps)
//!   3. Binance exchange (bookTicker has NO event_time; RTDS payload does)
//!
//! Comparing timestamps across clocks produces GARBAGE (e.g. Lag=-232ms).
//! This module provides TWO types of signals:
//!
//! **CLOCK-SAFE (use for trading):**
//!   - `rtds_binance_dev_bps`: price difference vs your direct Binance (pure math)
//!   - `rtds_chainlink_dev_bps`: price difference vs your direct Binance
//!   - `rtds_price_advantage_bps`: max edge available (max of above two)
//!   - `rtds_chainlink_binance_spread_bps`: Chainlink vs RTDS Binance deviation
//!     (same relay clock → valid comparison showing oracle lag in price terms)
//!
//! **APPROXIMATE (use for monitoring, NOT trading):**
//!   - `rtds_binance_lag_ms`: local_clock - binance_exchange_clock (~50ms NTP error)
//!   - `rtds_chainlink_lag_ms`: local_clock - chainlink_clock (~50ms NTP error)
//!   - `rtds_relay_overhead_ms`: local_received - source_ts (connection quality)
//!
//! ## Integration
//!   1. Save as src/rtds_tracker.rs
//!   2. Add `pub mod rtds_tracker;` to lib.rs
//!   3. Add `polymarket-client-sdk = { version = "0.4", features = ["rtds"] }` to Cargo.toml
//!   4. Wire into multi_market_logger_hourly.rs

use rust_decimal::prelude::ToPrimitive;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

// ============================================================================
// RTDS PRICE DATA
// ============================================================================

/// Thread-safe storage for a single RTDS price source
struct RtdsPriceSource {
    /// Latest price (stored as f64 bits for atomic access)
    price_bits: AtomicI64,
    /// Timestamp (ms) from RTDS payload — source clock (Binance or Chainlink)
    source_ts_ms: AtomicI64,
    /// Timestamp (ms) when WE received it — local clock
    received_ts_ms: AtomicI64,
    /// Count of updates received
    update_count: AtomicI64,
}

impl RtdsPriceSource {
    fn new() -> Self {
        Self {
            price_bits: AtomicI64::new(0),
            source_ts_ms: AtomicI64::new(0),
            received_ts_ms: AtomicI64::new(0),
            update_count: AtomicI64::new(0),
        }
    }

    #[inline]
    fn store(&self, price: f64, source_ts: i64, received_ts: i64) {
        self.price_bits
            .store(price.to_bits() as i64, Ordering::Release);
        self.source_ts_ms.store(source_ts, Ordering::Release);
        self.received_ts_ms.store(received_ts, Ordering::Release);
        self.update_count.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn load(&self) -> (f64, i64, i64) {
        let bits = self.price_bits.load(Ordering::Acquire) as u64;
        let price = f64::from_bits(bits);
        let source_ts = self.source_ts_ms.load(Ordering::Acquire);
        let received_ts = self.received_ts_ms.load(Ordering::Acquire);
        (price, source_ts, received_ts)
    }

    #[inline]
    fn count(&self) -> i64 {
        self.update_count.load(Ordering::Relaxed)
    }
}

// ============================================================================
// RTDS TRACKER (public API)
// ============================================================================

pub struct RtdsTracker {
    binance_feed: Arc<RtdsPriceSource>,
    chainlink_feed: Arc<RtdsPriceSource>,
    tasks: Vec<JoinHandle<()>>,
}

impl RtdsTracker {
    pub fn new() -> Self {
        Self {
            binance_feed: Arc::new(RtdsPriceSource::new()),
            chainlink_feed: Arc::new(RtdsPriceSource::new()),
            tasks: Vec::new(),
        }
    }

    /// Start background RTDS streaming tasks.
    pub fn start(&mut self) {
        use polymarket_client_sdk::rtds::Client;

        let client = Arc::new(Client::default());

        let binance_feed = self.binance_feed.clone();
        let chainlink_feed = self.chainlink_feed.clone();
        let binance_client = client.clone();
        let chainlink_client = client.clone();

        let binance_task = tokio::spawn(async move {
            rtds_binance_loop(binance_feed, binance_client).await;
        });
        let chainlink_task = tokio::spawn(async move {
            rtds_chainlink_loop(chainlink_feed, chainlink_client).await;
        });

        self.tasks.push(binance_task);
        self.tasks.push(chainlink_task);
        info!("RTDS tracker started (Binance relay + Chainlink oracle)");
    }

    /// Get current RTDS snapshot for CSV logging.
    ///
    /// `direct_binance_price`: Your latest price from direct Binance bookTicker
    /// `direct_binance_ts_ms`: snapshot.timestamp (LOCAL CLOCK when bookTicker arrived)
    ///
    /// ## Clock-safe signals (for trading):
    ///   - `rtds_*_dev_bps` — pure price comparison, no timestamps
    ///   - `rtds_chainlink_binance_spread_bps` — Chainlink vs RTDS Binance
    ///
    /// ## Approximate signals (for monitoring):
    ///   - `rtds_*_lag_ms` — mixes local clock with source clock, ~50ms NTP error
    pub fn get_snapshot(
        &self,
        direct_binance_price: f64,
        direct_binance_ts_ms: i64,
    ) -> RtdsSnapshot {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let (b_price, b_source_ts, _b_recv_ts) = self.binance_feed.load();
        let (c_price, c_source_ts, _c_recv_ts) = self.chainlink_feed.load();

        // ── CLOCK-SAFE: Price deviation (bps) ──
        // These compare two PRICES at the same moment. No clocks involved.
        // Positive = our direct Binance is HIGHER → we see a move the book hasn't priced
        let rtds_binance_dev_bps = if b_price > 0.0 && direct_binance_price > 0.0 {
            ((direct_binance_price - b_price) / direct_binance_price) * 10_000.0
        } else {
            0.0
        };

        let rtds_chainlink_dev_bps = if c_price > 0.0 && direct_binance_price > 0.0 {
            ((direct_binance_price - c_price) / direct_binance_price) * 10_000.0
        } else {
            0.0
        };

        // Max absolute edge across both feeds
        let rtds_price_advantage_bps = rtds_binance_dev_bps.abs().max(rtds_chainlink_dev_bps.abs());

        // ── CLOCK-SAFE: Chainlink vs RTDS Binance spread ──
        // Both prices come through Polymarket's RTDS relay, so comparing them
        // shows how much Chainlink deviates from Binance (oracle lag in price terms).
        // Positive = Chainlink is LOWER than RTDS Binance (Chainlink hasn't caught up)
        let rtds_chainlink_binance_spread_bps = if b_price > 0.0 && c_price > 0.0 {
            ((b_price - c_price) / b_price) * 10_000.0
        } else {
            0.0
        };

        // ── APPROXIMATE: Timestamp-based lag ──
        // WARNING: These mix local clock with source clock. NTP drift is typically
        // 10-100ms. Useful for monitoring (is RTDS alive? is it >1s stale?)
        // but NOT reliable enough for trading signals.
        //
        // direct_binance_ts_ms = LOCAL_CLOCK when bookTicker arrived
        // b_source_ts = BINANCE_EXCHANGE_CLOCK when price was generated
        // Difference ≈ network_latency + NTP_error + RTDS_relay
        let rtds_binance_lag_ms = if b_source_ts > 0 && direct_binance_ts_ms > 0 {
            direct_binance_ts_ms - b_source_ts
        } else {
            0
        };

        let rtds_chainlink_lag_ms = if c_source_ts > 0 && direct_binance_ts_ms > 0 {
            direct_binance_ts_ms - c_source_ts
        } else {
            0
        };

        // ── APPROXIMATE: Staleness (how old is RTDS data from our perspective) ──
        // now_ms - source_ts. Useful for "is RTDS feed alive?" checks.
        let rtds_binance_stale_ms = if b_source_ts > 0 {
            now_ms - b_source_ts
        } else {
            -1 // sentinel: no data received yet
        };

        let rtds_chainlink_stale_ms = if c_source_ts > 0 {
            now_ms - c_source_ts
        } else {
            -1
        };

        RtdsSnapshot {
            // Raw prices
            rtds_binance_price: b_price,
            rtds_chainlink_price: c_price,

            // CLOCK-SAFE: price deviations (USE THESE FOR TRADING)
            rtds_binance_dev_bps,
            rtds_chainlink_dev_bps,
            rtds_price_advantage_bps,
            rtds_chainlink_binance_spread_bps,

            // APPROXIMATE: timestamp-based (USE FOR MONITORING ONLY)
            rtds_binance_lag_ms,
            rtds_chainlink_lag_ms,
            rtds_binance_stale_ms,
            rtds_chainlink_stale_ms,
        }
    }

    /// Health check: returns (binance_count, chainlink_count) of updates received
    pub fn update_counts(&self) -> (i64, i64) {
        (self.binance_feed.count(), self.chainlink_feed.count())
    }
}

impl Default for RtdsTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// RTDS SNAPSHOT (for CSV logging)
// ============================================================================

/// RTDS state for CSV serialization.
///
/// 10 columns appended at END of existing CSV for backward compatibility.
///
/// Columns marked ✅ are clock-safe (pure price math).
/// Columns marked ⚠️ mix clock domains (~50ms error, monitoring only).
#[derive(Debug, Clone)]
pub struct RtdsSnapshot {
    // Raw prices from RTDS feeds
    pub rtds_binance_price: f64,   // What Polymarket sees from Binance
    pub rtds_chainlink_price: f64, // What Chainlink oracle reports

    // ✅ CLOCK-SAFE: price deviations
    pub rtds_binance_dev_bps: f64, // (direct - rtds_binance) / direct * 10000
    pub rtds_chainlink_dev_bps: f64, // (direct - rtds_chainlink) / direct * 10000
    pub rtds_price_advantage_bps: f64, // max(|binance_dev|, |chainlink_dev|)
    pub rtds_chainlink_binance_spread_bps: f64, // (rtds_binance - rtds_chainlink) / rtds_binance * 10000

    // ⚠️ APPROXIMATE: timestamp-based (monitoring only)
    pub rtds_binance_lag_ms: i64,     // local_clock - binance_source_ts
    pub rtds_chainlink_lag_ms: i64,   // local_clock - chainlink_source_ts
    pub rtds_binance_stale_ms: i64,   // now - binance_source_ts (-1 = no data)
    pub rtds_chainlink_stale_ms: i64, // now - chainlink_source_ts (-1 = no data)
}

impl Default for RtdsSnapshot {
    fn default() -> Self {
        Self {
            rtds_binance_price: 0.0,
            rtds_chainlink_price: 0.0,
            rtds_binance_dev_bps: 0.0,
            rtds_chainlink_dev_bps: 0.0,
            rtds_price_advantage_bps: 0.0,
            rtds_chainlink_binance_spread_bps: 0.0,
            rtds_binance_lag_ms: 0,
            rtds_chainlink_lag_ms: 0,
            rtds_binance_stale_ms: -1,
            rtds_chainlink_stale_ms: -1,
        }
    }
}

impl RtdsSnapshot {
    /// CSV header fragment (append after existing columns with leading comma)
    pub fn csv_header_suffix() -> &'static str {
        "rtds_binance_price,rtds_chainlink_price,\
         rtds_binance_dev_bps,rtds_chainlink_dev_bps,\
         rtds_price_advantage_bps,rtds_chainlink_binance_spread_bps,\
         rtds_binance_lag_ms,rtds_chainlink_lag_ms,\
         rtds_binance_stale_ms,rtds_chainlink_stale_ms"
    }

    /// CSV value fragment (append after existing columns with leading comma)
    pub fn to_csv_suffix(&self) -> String {
        format!(
            "{:.2},{:.2},{:.4},{:.4},{:.4},{:.4},{},{},{},{}",
            self.rtds_binance_price,
            self.rtds_chainlink_price,
            self.rtds_binance_dev_bps,
            self.rtds_chainlink_dev_bps,
            self.rtds_price_advantage_bps,
            self.rtds_chainlink_binance_spread_bps,
            self.rtds_binance_lag_ms,
            self.rtds_chainlink_lag_ms,
            self.rtds_binance_stale_ms,
            self.rtds_chainlink_stale_ms,
        )
    }
}

// ============================================================================
// BACKGROUND RTDS STREAMING LOOPS
// ============================================================================

const INITIAL_RECONNECT_DELAY_MS: u64 = 1000;
const MAX_RECONNECT_DELAY_MS: u64 = 60000;
const RECONNECT_BACKOFF_MULTIPLIER: f64 = 2.0;

async fn rtds_binance_loop(
    feed: Arc<RtdsPriceSource>,
    client: Arc<polymarket_client_sdk::rtds::Client>,
) {
    let mut delay_ms = INITIAL_RECONNECT_DELAY_MS;

    loop {
        info!("RTDS: Connecting to Binance relay (crypto_prices/btcusdt)...");

        match connect_rtds_binance(&feed, &client).await {
            Ok(0) => {
                warn!("RTDS Binance: stream ended with 0 messages — check subscribe_crypto_prices(None) is used, not Some(symbols)");
                delay_ms = INITIAL_RECONNECT_DELAY_MS;
            }
            Ok(count) => {
                info!(
                    "RTDS Binance stream ended after {} messages, reconnecting...",
                    count
                );
                delay_ms = INITIAL_RECONNECT_DELAY_MS;
            }
            Err(e) => {
                warn!("RTDS Binance connection error: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms =
            ((delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64).min(MAX_RECONNECT_DELAY_MS);
    }
}

async fn rtds_chainlink_loop(
    feed: Arc<RtdsPriceSource>,
    client: Arc<polymarket_client_sdk::rtds::Client>,
) {
    let mut delay_ms = INITIAL_RECONNECT_DELAY_MS;

    loop {
        info!("RTDS: Connecting to Chainlink oracle (crypto_prices_chainlink/btc/usd)...");

        match connect_rtds_chainlink(&feed, &client).await {
            Ok(count) => {
                info!(
                    "RTDS Chainlink stream ended after {} messages, reconnecting...",
                    count
                );
                delay_ms = INITIAL_RECONNECT_DELAY_MS;
            }
            Err(e) => {
                warn!("RTDS Chainlink connection error: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms =
            ((delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64).min(MAX_RECONNECT_DELAY_MS);
    }
}

async fn connect_rtds_binance(
    feed: &Arc<RtdsPriceSource>,
    client: &Arc<polymarket_client_sdk::rtds::Client>,
) -> anyhow::Result<u64> {
    // ✅ None = subscribe to ALL symbols, filter manually below.
    // Some(vec!["btcusdt"]) silently produces an empty stream — SDK bug.
    let stream = client
        .subscribe_crypto_prices(None)
        .map_err(|e| anyhow::anyhow!("RTDS Binance subscribe failed: {}", e))?;

    let mut stream = Box::pin(stream);
    let mut count: u64 = 0;

    while let Some(result) = stream.next().await {
        match result {
            Ok(price) => {
                // Manual symbol filter — only process btcusdt
                if price.symbol.to_lowercase() != "btcusdt" {
                    continue;
                }

                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;

                let source_ts = price.timestamp as i64;
                let price_f64 = match price.value.to_f64() {
                    Some(v) => v,
                    None => {
                        warn!("RTDS Binance: Decimal->f64 failed for {}", price.value);
                        continue;
                    }
                };

                feed.store(price_f64, source_ts, now_ms);

                count += 1;
                if count == 1 {
                    info!(
                        "RTDS Binance first update: ${:.2} (source_ts={})",
                        price_f64, source_ts
                    );
                }
                if count % 1000 == 0 {
                    debug!(
                        "RTDS Binance #{}: ${:.2} stale={}ms",
                        count,
                        price_f64,
                        now_ms - source_ts
                    );
                }
            }
            Err(e) => {
                debug!("RTDS Binance stream error: {}", e);
            }
        }
    }

    Ok(count)
}

async fn connect_rtds_chainlink(
    feed: &Arc<RtdsPriceSource>,
    client: &Arc<polymarket_client_sdk::rtds::Client>,
) -> anyhow::Result<u64> {
    let symbol = "btc/usd".to_owned();

    let stream = client
        .subscribe_chainlink_prices(Some(symbol))
        .map_err(|e| anyhow::anyhow!("RTDS Chainlink subscribe failed: {}", e))?;

    let mut stream = Box::pin(stream);
    let mut count: u64 = 0;

    while let Some(result) = stream.next().await {
        match result {
            Ok(price) => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;

                let source_ts = price.timestamp as i64;
                let price_f64 = match price.value.to_f64() {
                    Some(v) => v,
                    None => {
                        warn!("RTDS Chainlink: Decimal->f64 failed for {}", price.value);
                        continue;
                    }
                };

                feed.store(price_f64, source_ts, now_ms);

                count += 1;
                if count == 1 {
                    info!(
                        "RTDS Chainlink first update: ${:.2} (source_ts={})",
                        price.value, source_ts
                    );
                }
                if count % 100 == 0 {
                    debug!(
                        "RTDS Chainlink #{}: ${:.2} stale={}ms",
                        count,
                        price.value,
                        now_ms - source_ts
                    );
                }
            }
            Err(e) => {
                debug!("RTDS Chainlink stream error: {}", e);
            }
        }
    }

    Ok(count)
}
