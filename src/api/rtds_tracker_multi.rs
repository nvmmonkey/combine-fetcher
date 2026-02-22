//! Multi-Asset RTDS Price Tracker
//!
//! Extends rtds_tracker.rs to support BTC, ETH, SOL, XRP simultaneously.
//! Uses a single Binance connection and a single Chainlink connection,
//! routing messages to per-asset price sources by symbol client-side.
//!
//! ## SDK Notes
//! - `subscribe_crypto_prices(None)` returns ALL symbols ("btcusdt", "ethusdt", etc.)
//! - `subscribe_chainlink_prices(None)` returns ALL symbols ("btc/usd", "eth/usd", etc.)
//! - `Some(vec![...])` silently produces an empty stream — SDK bug, use None always.
//! - One WS connection handles all symbols via single Arc<Client>.

use rust_decimal::prelude::ToPrimitive;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::StreamExt as _;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::api::rtds_tracker::RtdsSnapshot;

// ============================================================================
// RTDS PRICE DATA (same pattern as rtds_tracker.rs — private there, duplicated here)
// ============================================================================

/// Grouped price data read/written atomically to prevent torn reads.
#[derive(Clone, Copy)]
struct RtdsPriceData {
    price_bits: i64,
    source_ts_ms: i64,
    received_ts_ms: i64,
}

/// Thread-safe storage for a single RTDS price source.
struct RtdsPriceSource {
    data: RwLock<RtdsPriceData>,
    update_count: AtomicI64,
}

impl RtdsPriceSource {
    fn new() -> Self {
        Self {
            data: RwLock::new(RtdsPriceData {
                price_bits: 0,
                source_ts_ms: 0,
                received_ts_ms: 0,
            }),
            update_count: AtomicI64::new(0),
        }
    }

    #[inline]
    fn store(&self, price: f64, source_ts: i64, received_ts: i64) {
        let mut data = self.data.write().unwrap();
        data.price_bits = price.to_bits() as i64;
        data.source_ts_ms = source_ts;
        data.received_ts_ms = received_ts;
        drop(data);
        self.update_count.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn load(&self) -> (f64, i64, i64) {
        let data = self.data.read().unwrap();
        let price = f64::from_bits(data.price_bits as u64);
        (price, data.source_ts_ms, data.received_ts_ms)
    }

    #[inline]
    fn count(&self) -> i64 {
        self.update_count.load(Ordering::Relaxed)
    }
}

// ============================================================================
// SYMBOL ROUTING
// ============================================================================

fn binance_symbol_to_asset(symbol: &str) -> Option<&'static str> {
    match symbol {
        "btcusdt" => Some("btc"),
        "ethusdt" => Some("eth"),
        "solusdt" => Some("sol"),
        "xrpusdt" => Some("xrp"),
        _ => None,
    }
}

fn chainlink_symbol_to_asset(symbol: &str) -> Option<&'static str> {
    match symbol {
        "btc/usd" => Some("btc"),
        "eth/usd" => Some("eth"),
        "sol/usd" => Some("sol"),
        "xrp/usd" => Some("xrp"),
        _ => None,
    }
}

// ============================================================================
// MULTI-ASSET RTDS TRACKER (public API)
// ============================================================================

pub struct MultiAssetRtdsTracker {
    binance_feeds: HashMap<String, Arc<RtdsPriceSource>>,
    chainlink_feeds: HashMap<String, Arc<RtdsPriceSource>>,
    tasks: Vec<JoinHandle<()>>,
}

impl MultiAssetRtdsTracker {
    /// Create tracker for the given assets (e.g. `&["btc", "eth", "sol", "xrp"]`).
    pub fn new(assets: &[&str]) -> Self {
        let mut binance_feeds = HashMap::new();
        let mut chainlink_feeds = HashMap::new();
        for &asset in assets {
            binance_feeds.insert(asset.to_string(), Arc::new(RtdsPriceSource::new()));
            chainlink_feeds.insert(asset.to_string(), Arc::new(RtdsPriceSource::new()));
        }
        Self {
            binance_feeds,
            chainlink_feeds,
            tasks: Vec::new(),
        }
    }

    /// Start background RTDS streaming tasks (one Binance + one Chainlink connection).
    pub fn start(&mut self) {
        use polymarket_client_sdk::rtds::Client;

        let client = Arc::new(Client::default());

        // Clone the Arc<RtdsPriceSource> maps so background tasks share the same sources
        let binance_feeds = self.binance_feeds.clone();
        let chainlink_feeds = self.chainlink_feeds.clone();
        let binance_client = client.clone();
        let chainlink_client = client.clone();

        let binance_task = tokio::spawn(async move {
            multi_rtds_binance_loop(binance_feeds, binance_client).await;
        });
        let chainlink_task = tokio::spawn(async move {
            multi_rtds_chainlink_loop(chainlink_feeds, chainlink_client).await;
        });

        self.tasks.push(binance_task);
        self.tasks.push(chainlink_task);
        info!("MultiAssetRtdsTracker started (Binance relay + Chainlink oracle, all assets)");
    }

    /// Get current RTDS snapshot for the given asset.
    ///
    /// `direct_price`: Latest direct Binance price for this asset
    /// `direct_ts_ms`: Local timestamp (ms) when the direct Binance price arrived
    pub fn get_snapshot(&self, asset: &str, direct_price: f64, direct_ts_ms: i64) -> RtdsSnapshot {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let (b_price, b_source_ts, _) = self
            .binance_feeds
            .get(asset)
            .map(|f| f.load())
            .unwrap_or((0.0, 0, 0));

        let (c_price, c_source_ts, _) = self
            .chainlink_feeds
            .get(asset)
            .map(|f| f.load())
            .unwrap_or((0.0, 0, 0));

        // ── CLOCK-SAFE: Price deviation (bps) ──
        let rtds_binance_dev_bps = if b_price > 0.0 && direct_price > 0.0 {
            ((direct_price - b_price) / direct_price) * 10_000.0
        } else {
            0.0
        };

        let rtds_chainlink_dev_bps = if c_price > 0.0 && direct_price > 0.0 {
            ((direct_price - c_price) / direct_price) * 10_000.0
        } else {
            0.0
        };

        let rtds_price_advantage_bps =
            rtds_binance_dev_bps.abs().max(rtds_chainlink_dev_bps.abs());

        // ── CLOCK-SAFE: Chainlink vs RTDS Binance spread ──
        let rtds_chainlink_binance_spread_bps = if b_price > 0.0 && c_price > 0.0 {
            ((b_price - c_price) / b_price) * 10_000.0
        } else {
            0.0
        };

        // ── APPROXIMATE: Timestamp-based lag (monitoring only) ──
        let rtds_binance_lag_ms = if b_source_ts > 0 && direct_ts_ms > 0 {
            direct_ts_ms - b_source_ts
        } else {
            0
        };

        let rtds_chainlink_lag_ms = if c_source_ts > 0 && direct_ts_ms > 0 {
            direct_ts_ms - c_source_ts
        } else {
            0
        };

        let rtds_binance_stale_ms = if b_source_ts > 0 { now_ms - b_source_ts } else { -1 };
        let rtds_chainlink_stale_ms = if c_source_ts > 0 { now_ms - c_source_ts } else { -1 };

        RtdsSnapshot {
            rtds_binance_price: b_price,
            rtds_chainlink_price: c_price,
            rtds_binance_dev_bps,
            rtds_chainlink_dev_bps,
            rtds_price_advantage_bps,
            rtds_chainlink_binance_spread_bps,
            rtds_binance_lag_ms,
            rtds_chainlink_lag_ms,
            rtds_binance_stale_ms,
            rtds_chainlink_stale_ms,
        }
    }

    /// Returns (binance_count, chainlink_count) of updates received for the given asset.
    pub fn update_counts(&self, asset: &str) -> (i64, i64) {
        let b = self.binance_feeds.get(asset).map(|f| f.count()).unwrap_or(0);
        let c = self.chainlink_feeds.get(asset).map(|f| f.count()).unwrap_or(0);
        (b, c)
    }

    /// Returns total update counts across all tracked assets: (binance_total, chainlink_total).
    pub fn total_update_counts(&self) -> (i64, i64) {
        let b: i64 = self.binance_feeds.values().map(|f| f.count()).sum();
        let c: i64 = self.chainlink_feeds.values().map(|f| f.count()).sum();
        (b, c)
    }
}

// ============================================================================
// BACKGROUND STREAMING LOOPS
// ============================================================================

const INITIAL_RECONNECT_DELAY_MS: u64 = 1000;
const MAX_RECONNECT_DELAY_MS: u64 = 60000;
const RECONNECT_BACKOFF_MULTIPLIER: f64 = 2.0;

async fn multi_rtds_binance_loop(
    feeds: HashMap<String, Arc<RtdsPriceSource>>,
    client: Arc<polymarket_client_sdk::rtds::Client>,
) {
    let mut delay_ms = INITIAL_RECONNECT_DELAY_MS;

    loop {
        info!("MultiRTDS: Connecting to Binance relay (crypto_prices, all assets)...");

        match connect_multi_rtds_binance(&feeds, &client).await {
            Ok(0) => {
                warn!("MultiRTDS Binance: stream ended with 0 messages — verify subscribe_crypto_prices(None) is used");
                delay_ms = INITIAL_RECONNECT_DELAY_MS;
            }
            Ok(count) => {
                info!(
                    "MultiRTDS Binance stream ended after {} messages, reconnecting...",
                    count
                );
                delay_ms = INITIAL_RECONNECT_DELAY_MS;
            }
            Err(e) => {
                warn!("MultiRTDS Binance connection error: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms =
            ((delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64).min(MAX_RECONNECT_DELAY_MS);
    }
}

async fn multi_rtds_chainlink_loop(
    feeds: HashMap<String, Arc<RtdsPriceSource>>,
    client: Arc<polymarket_client_sdk::rtds::Client>,
) {
    let mut delay_ms = INITIAL_RECONNECT_DELAY_MS;

    loop {
        info!("MultiRTDS: Connecting to Chainlink oracle (crypto_prices_chainlink, all assets)...");

        match connect_multi_rtds_chainlink(&feeds, &client).await {
            Ok(count) => {
                info!(
                    "MultiRTDS Chainlink stream ended after {} messages, reconnecting...",
                    count
                );
                delay_ms = INITIAL_RECONNECT_DELAY_MS;
            }
            Err(e) => {
                warn!("MultiRTDS Chainlink connection error: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms =
            ((delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64).min(MAX_RECONNECT_DELAY_MS);
    }
}

async fn connect_multi_rtds_binance(
    feeds: &HashMap<String, Arc<RtdsPriceSource>>,
    client: &Arc<polymarket_client_sdk::rtds::Client>,
) -> anyhow::Result<u64> {
    // ✅ None = subscribe to ALL symbols, filter manually below.
    // Some(vec!["btcusdt"]) silently produces an empty stream — SDK bug.
    let stream = client
        .subscribe_crypto_prices(None)
        .map_err(|e| anyhow::anyhow!("MultiRTDS Binance subscribe failed: {}", e))?;

    let mut stream = Box::pin(stream);
    let mut count: u64 = 0;
    // Track per-asset first updates to confirm independent routing for each asset.
    // NOTE: In multi-asset mode, all assets typically share the same source_ts because
    // the RTDS server batches all symbol updates with a single server-side timestamp.
    // This is expected server behavior, not a routing bug. The distinct prices per
    // asset (logged on first update) are the proof that routing works correctly.
    let mut first_seen: std::collections::HashSet<&'static str> =
        std::collections::HashSet::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(price) => {
                let sym = price.symbol.to_lowercase();
                let asset = match binance_symbol_to_asset(&sym) {
                    Some(a) => a,
                    None => continue,
                };

                let feed = match feeds.get(asset) {
                    Some(f) => f,
                    None => continue,
                };

                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;

                let source_ts = price.timestamp as i64;
                let price_f64 = match price.value.to_f64() {
                    Some(v) => v,
                    None => {
                        warn!("MultiRTDS Binance: Decimal->f64 failed for {}", price.value);
                        continue;
                    }
                };

                feed.store(price_f64, source_ts, now_ms);

                count += 1;
                if !first_seen.contains(asset) {
                    info!(
                        "MultiRTDS Binance first {} update: ${:.4} source_ts={} stale={}ms",
                        asset,
                        price_f64,
                        source_ts,
                        now_ms - source_ts
                    );
                    first_seen.insert(asset);
                }
                if count % 1000 == 0 {
                    debug!(
                        "MultiRTDS Binance #{}: {} ${:.4} stale={}ms",
                        count,
                        asset,
                        price_f64,
                        now_ms - source_ts
                    );
                }
            }
            Err(e) => {
                debug!("MultiRTDS Binance stream error: {}", e);
            }
        }
    }

    Ok(count)
}

async fn connect_multi_rtds_chainlink(
    feeds: &HashMap<String, Arc<RtdsPriceSource>>,
    client: &Arc<polymarket_client_sdk::rtds::Client>,
) -> anyhow::Result<u64> {
    // ✅ None = subscribe to ALL symbols, filter manually below.
    // Some(vec![...]) silently produces an empty stream — SDK bug.
    let stream = client
        .subscribe_chainlink_prices(None)
        .map_err(|e| anyhow::anyhow!("MultiRTDS Chainlink subscribe failed: {}", e))?;

    let mut stream = Box::pin(stream);
    let mut count: u64 = 0;
    // Track per-asset first updates to confirm independent routing.
    // Chainlink updates all 4 oracle feeds in the same Ethereum block → same block
    // timestamp for btc/usd, eth/usd, sol/usd, xrp/usd. This causes identical stale_ms
    // across all Chainlink assets. It is expected server behavior, not a routing bug.
    let mut first_seen: std::collections::HashSet<&'static str> =
        std::collections::HashSet::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(price) => {
                let sym = price.symbol.to_lowercase();
                let asset = match chainlink_symbol_to_asset(&sym) {
                    Some(a) => a,
                    None => continue,
                };

                let feed = match feeds.get(asset) {
                    Some(f) => f,
                    None => continue,
                };

                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;

                let source_ts = price.timestamp as i64;
                let price_f64 = match price.value.to_f64() {
                    Some(v) => v,
                    None => {
                        warn!("MultiRTDS Chainlink: Decimal->f64 failed for {}", price.value);
                        continue;
                    }
                };

                feed.store(price_f64, source_ts, now_ms);

                count += 1;
                if !first_seen.contains(asset) {
                    info!(
                        "MultiRTDS Chainlink first {} update: ${:.4} source_ts={} stale={}ms",
                        asset,
                        price_f64,
                        source_ts,
                        now_ms - source_ts
                    );
                    first_seen.insert(asset);
                }
                if count % 100 == 0 {
                    debug!(
                        "MultiRTDS Chainlink #{}: {} ${:.4} stale={}ms",
                        count,
                        asset,
                        price_f64,
                        now_ms - source_ts
                    );
                }
            }
            Err(e) => {
                debug!("MultiRTDS Chainlink stream error: {}", e);
            }
        }
    }

    Ok(count)
}
