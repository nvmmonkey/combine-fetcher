//! Binance WebSocket Manager (FIXED: exchange-level event_time for RTDS edge)
//!
//! Identical to binance.rs except aggTrade uses trade.event_time (Binance exchange
//! timestamp) instead of local SystemTime. This preserves the RTDS timing edge:
//! Chainlink aggregation latency + Polygon block time become visible in lag metrics.
//!
//! Three independent WebSocket streams:
//!   - `bookTicker` → bid/ask prices → `BinancePrice` (Arc<RwLock<HashMap>>)
//!   - `aggTrade`   → trade volume   → `VolumeTracker` (rolling 1s/1min windows)
//!   - `depth20@100ms` → L2 orderbook depth (20 levels) → `BinanceDepthData` (OBI calculation)
//!
//! Run both loggers side-by-side to compare data quality.

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::volatility::{FibonacciVolatilityCalculator, MarketSnapshot, VolumeTracker};

// ============================================================================
// CONSTANTS
// ============================================================================

/// Binance REST API base URL
const BINANCE_REST_API_URL: &str = "https://api.binance.com/api/v3";

/// Default timeout for REST API requests
const REST_API_TIMEOUT_SECS: u64 = 10;

/// Maximum klines per request (Binance limit)
const MAX_KLINES_PER_REQUEST: usize = 1000;

// ============================================================================
// PRICE DATA STRUCTURES
// ============================================================================

/// Binance price data containing bid, ask, volume, and calculated mid price
#[derive(Debug, Clone)]
pub struct BinancePrice {
    /// Best bid price
    pub bid: Decimal,
    /// Best ask price
    pub ask: Decimal,
    /// Mid price (average of bid and ask)
    pub mid: Decimal,
    /// Cumulative volume in USDT since connection start (for reference)
    /// For rolling window volume, use get_market_snapshot() which provides
    /// volume_1sec and volume_1min fields
    pub volume: Decimal,
}

impl BinancePrice {
    /// Create a new BinancePrice from bid, ask, and volume values
    pub fn new(bid: Decimal, ask: Decimal, volume: Decimal) -> Self {
        let mid = (bid + ask) / Decimal::from(2);
        Self {
            bid,
            ask,
            mid,
            volume,
        }
    }

    /// Create a new BinancePrice from bid and ask (volume defaults to zero)
    pub fn new_no_volume(bid: Decimal, ask: Decimal) -> Self {
        Self::new(bid, ask, Decimal::from(0))
    }
}

// ============================================================================
// WEBSOCKET MESSAGE STRUCTURES
// ============================================================================

/// Binance bookTicker message format
#[derive(Debug, Deserialize)]
struct BinanceBookTicker {
    /// Symbol (e.g., "BTCUSDT")
    #[serde(rename = "s")]
    symbol: String,
    /// Best bid price
    #[serde(rename = "b")]
    bid_price: String,
    #[serde(rename = "B")]
    _bid_qty: String,
    /// Best ask price
    #[serde(rename = "a")]
    ask_price: String,
    #[serde(rename = "A")]
    _ask_qty: String,
}

/// Binance aggregate trade event from @aggTrade stream
#[derive(Debug, Deserialize)]
struct BinanceAggTradeEvent {
    /// Event type (always "aggTrade")
    #[serde(rename = "e")]
    _event_type: String,
    /// Event time in milliseconds
    #[serde(rename = "E")]
    event_time: u64,
    /// Symbol (e.g., "BTCUSDT")
    #[serde(rename = "s")]
    symbol: String,
    /// Trade price
    #[serde(rename = "p")]
    price: String,
    /// Trade quantity
    #[serde(rename = "q")]
    quantity: String,
    /// Trade time in milliseconds
    #[serde(rename = "T")]
    _trade_time: i64,
}

/// Binance partial book depth snapshot from @depth20@100ms stream
#[derive(Debug, Deserialize)]
struct BinanceDepthEvent {
    /// Last update ID for this snapshot
    #[serde(rename = "lastUpdateId")]
    _last_update_id: u64,
    /// Bid levels: [[price, qty], ...]
    bids: Vec<[String; 2]>,
    /// Ask levels: [[price, qty], ...]
    asks: Vec<[String; 2]>,
}

/// Combined stream wrapper from /stream?streams= endpoint
/// Format: {"stream":"btcusdt@depth20@100ms","data":{...}}
#[derive(Debug, Deserialize)]
struct CombinedDepthMessage {
    stream: String,
    data: BinanceDepthEvent,
}

/// Processed Binance depth data (thread-safe snapshot)
#[derive(Debug, Clone, Default)]
pub struct BinanceDepthData {
    /// Sum of qty for top 3 bid levels
    pub bid_depth_3: f64,
    /// Sum of qty for top 3 ask levels
    pub ask_depth_3: f64,
    /// Sum of qty for ALL bid levels (up to 20)
    pub bid_depth_all: f64,
    /// Sum of qty for ALL ask levels (up to 20)
    pub ask_depth_all: f64,
    /// Number of bid price levels with qty > 0
    pub bid_levels: u16,
    /// Number of ask price levels with qty > 0
    pub ask_levels: u16,
    /// OBI: (bid_depth_all - ask_depth_all) / (bid_depth_all + ask_depth_all), range [-1, 1]
    pub book_imbalance: f64,
    /// Binance event receive timestamp (local clock, for staleness monitoring only)
    pub last_update_ms: u64,
}

// ============================================================================
// REST API STRUCTURES (for historical kline bootstrap)
// ============================================================================

/// Parsed kline data from REST API
#[derive(Debug, Clone)]
pub struct Kline {
    /// Kline open time (milliseconds)
    pub open_time: i64,
    /// Open price
    pub open: f64,
    /// High price
    pub high: f64,
    /// Low price
    pub low: f64,
    /// Close price
    pub close: f64,
    /// Volume (base asset)
    pub volume: f64,
    /// Kline close time (milliseconds)
    pub close_time: i64,
    /// Quote asset volume
    pub quote_volume: f64,
    /// Number of trades
    pub trades: i64,
}

impl Kline {
    /// Parse a kline from Binance REST API response
    /// Binance returns: [open_time, open, high, low, close, volume, close_time,
    ///                   quote_volume, trades, taker_buy_base, taker_buy_quote, ignore]
    fn from_json(arr: &[serde_json::Value]) -> Option<Self> {
        if arr.len() < 9 {
            return None;
        }

        Some(Self {
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
}

// ============================================================================
// BINANCE DATA MANAGER
// ============================================================================

/// Binance WebSocket Data Manager
///
/// Manages connections to Binance WebSocket streams and maintains
/// a thread-safe cache of latest prices and volumes for all subscribed symbols.
/// Also provides REST API methods for bootstrapping historical data.
pub struct BinanceDataManager {
    /// Thread-safe price storage: symbol -> BinancePrice
    prices: Arc<RwLock<HashMap<String, BinancePrice>>>,
    /// Binance WebSocket URL (default: wss://stream.binance.com:9443/ws)
    ws_url: String,
    /// Connected symbols
    symbols: Vec<String>,
    /// Whether a reconnection is in progress (bookTicker)
    reconnecting: Arc<RwLock<bool>>,
    /// Whether a reconnection is in progress (aggTrade)
    reconnecting_agg: Arc<RwLock<bool>>,
    /// Volatility calculators per symbol: symbol -> calculator
    vol_calculators: HashMap<String, Arc<RwLock<FibonacciVolatilityCalculator>>>,
    /// Volume trackers per symbol: symbol -> tracker
    volume_trackers: HashMap<String, Arc<Mutex<VolumeTracker>>>,
    /// Latest event times per symbol: symbol -> event_time_ms (from Binance streams)
    event_times: Arc<RwLock<HashMap<String, u64>>>,
    /// Thread-safe depth data storage: symbol -> BinanceDepthData
    depth_data: Arc<RwLock<HashMap<String, BinanceDepthData>>>,
    /// Whether a reconnection is in progress (depth stream)
    reconnecting_depth: Arc<RwLock<bool>>,
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
            ws_url: "wss://stream.binance.com:9443/ws".to_string(),
            symbols: Vec::new(),
            reconnecting: Arc::new(RwLock::new(false)),
            reconnecting_agg: Arc::new(RwLock::new(false)),
            vol_calculators: HashMap::new(),
            volume_trackers: HashMap::new(),
            event_times: Arc::new(RwLock::new(HashMap::new())),
            depth_data: Arc::new(RwLock::new(HashMap::new())),
            reconnecting_depth: Arc::new(RwLock::new(false)),
            http_client,
        }
    }

    /// Create a new BinanceDataManager with a custom WebSocket URL
    ///
    /// # Arguments
    /// * `ws_url` - Custom WebSocket URL (e.g., for testnet)
    pub fn with_url(ws_url: String) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REST_API_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            prices: Arc::new(RwLock::new(HashMap::new())),
            ws_url,
            symbols: Vec::new(),
            reconnecting: Arc::new(RwLock::new(false)),
            reconnecting_agg: Arc::new(RwLock::new(false)),
            vol_calculators: HashMap::new(),
            volume_trackers: HashMap::new(),
            event_times: Arc::new(RwLock::new(HashMap::new())),
            depth_data: Arc::new(RwLock::new(HashMap::new())),
            reconnecting_depth: Arc::new(RwLock::new(false)),
            http_client,
        }
    }

    // ========================================================================
    // REST API METHODS (Historical Data Bootstrap)
    // ========================================================================

    /// Fetch historical klines from Binance REST API
    ///
    /// # Arguments
    /// * `symbol` - Trading pair (e.g., "BTCUSDT")
    /// * `interval` - Kline interval (e.g., "1m", "5m", "1h")
    /// * `limit` - Number of klines to fetch (max 1000)
    ///
    /// # Returns
    /// Vector of Kline structs, ordered from oldest to newest
    ///
    /// # Example
    /// ```no_run
    /// let manager = BinanceDataManager::new();
    /// let klines = manager.fetch_klines("BTCUSDT", "1m", 60).await?;
    /// ```
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
            .filter_map(|arr| Kline::from_json(arr))
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
    ///
    /// Fetches historical klines from Binance REST API and populates
    /// the volatility calculator's 1-minute price buffer. This should be
    /// called AFTER connect() and BEFORE the main loop to ensure accurate
    /// volatility calculations from the start.
    ///
    /// # Arguments
    /// * `symbol` - Trading pair (e.g., "BTCUSDT")
    /// * `limit` - Number of 1-minute candles to fetch (default: 70, max: 1000)
    ///
    /// # Returns
    /// * `Ok(usize)` - Number of klines successfully loaded
    /// * `Err` - If fetch fails or symbol not found
    ///
    /// # Example
    /// ```no_run
    /// let mut manager = BinanceDataManager::new();
    /// manager.connect(&["BTCUSDT"], None).await?;
    ///
    /// // Bootstrap with 70 1-minute candles (enough for 60-period lookback)
    /// let loaded = manager.bootstrap_volatility("BTCUSDT", 70).await?;
    /// println!("Bootstrapped with {} candles", loaded);
    /// ```
    pub async fn bootstrap_volatility(&self, symbol: &str, limit: usize) -> Result<usize> {
        let upper_symbol = symbol.to_uppercase();
        let limit = limit.min(MAX_KLINES_PER_REQUEST);

        info!(
            "📊 Bootstrapping volatility for {} with {} 1m klines...",
            upper_symbol, limit
        );

        // Fetch historical 1-minute klines
        let klines = self.fetch_klines(&upper_symbol, "1m", limit).await?;

        if klines.is_empty() {
            return Err(anyhow!("No klines returned for {}", upper_symbol));
        }

        // Get the volatility calculator for this symbol
        let vol_calculator = self.vol_calculators.get(&upper_symbol).ok_or_else(|| {
            anyhow!(
                "No volatility calculator for {}. Call connect() first.",
                upper_symbol
            )
        })?;

        // Lock and populate the 1-minute buffer
        let mut calc = vol_calculator.write().await;
        let mut loaded = 0;

        for kline in &klines {
            if kline.close > 0.0 {
                calc.add_price_1min(kline.close);
                loaded += 1;
            }
        }

        // Also seed the real-time buffer with recent prices for immediate vol calculation
        // Use the last 90 seconds worth of data (if we have high-frequency data)
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // For each of the last few klines, add synthetic ticks at the close price
        // This helps bridge the gap until real WebSocket data arrives
        let recent_klines = klines.iter().rev().take(5);
        for kline in recent_klines {
            if kline.close > 0.0 {
                // Use kline close time as timestamp (convert to ms if needed)
                let ts = if kline.close_time > 1_000_000_000_000 {
                    kline.close_time // Already in ms
                } else {
                    kline.close_time * 1000 // Convert to ms
                };
                calc.add_price(ts, kline.close);
            }
        }

        info!(
            "✅ Bootstrapped {} with {} klines | 1min buffer: {} | unified buffer: {}",
            upper_symbol,
            loaded,
            calc.get_1min_buffer_size(),
            calc.get_unified_buffer_size()
        );

        Ok(loaded)
    }

    /// Bootstrap volatility for all connected symbols
    ///
    /// Convenience method to bootstrap all symbols at once.
    ///
    /// # Arguments
    /// * `limit` - Number of 1-minute candles per symbol
    ///
    /// # Returns
    /// HashMap of symbol -> klines loaded (or error message)
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
    // WEBSOCKET CONNECTION METHODS
    // ========================================================================

    /// Connect to Binance WebSocket and subscribe to symbols
    ///
    /// # Arguments
    /// * `symbols` - Array of symbols to subscribe (e.g., ["BTCUSDT", "ETHUSDT"])
    /// * `streams_url` - Optional custom streams URL (None uses default)
    ///
    /// # Returns
    /// Returns Ok(()) on successful connection initiation
    ///
    /// # Example
    /// ```no_run
    /// let manager = BinanceDataManager::new();
    /// manager.connect(&["BTCUSDT"], None).await.unwrap();
    /// ```
    pub async fn connect(&mut self, symbols: &[&str], streams_url: Option<String>) -> Result<()> {
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

        // Build bookTicker streams URL
        let bookticker_url = if let Some(custom_url) = &streams_url {
            custom_url.clone()
        } else {
            let streams: Vec<String> = self
                .symbols
                .iter()
                .map(|s| format!("{}@bookTicker", s.to_lowercase()))
                .collect();
            format!("{}/{}", self.ws_url, streams.join("/"))
        };

        // Build aggTrade streams URL (separate connection)
        let aggtrade_url = {
            let streams: Vec<String> = self
                .symbols
                .iter()
                .map(|s| format!("{}@aggTrade", s.to_lowercase()))
                .collect();
            format!("{}/{}", self.ws_url, streams.join("/"))
        };

        info!(
            "Connecting to Binance WebSocket (bookTicker): {}",
            bookticker_url
        );
        info!(
            "Connecting to Binance WebSocket (aggTrade): {}",
            aggtrade_url
        );

        let prices = self.prices.clone();
        let reconnecting = self.reconnecting.clone();
        let symbols_clone = self.symbols.clone();
        let ws_url_clone = self.ws_url.clone();
        let vol_calculators = self.vol_calculators.clone();
        let event_times = self.event_times.clone();

        // Spawn the bookTicker WebSocket connection task
        tokio::spawn(async move {
            Self::bookticker_connection_loop(
                bookticker_url,
                prices,
                reconnecting,
                symbols_clone,
                ws_url_clone,
                vol_calculators,
                event_times,
            )
            .await
        });

        // Spawn the aggTrade WebSocket connection task for volume
        let prices_agg = self.prices.clone();
        let reconnecting_agg = self.reconnecting_agg.clone();
        let symbols_agg = self.symbols.clone();
        let ws_url_agg = self.ws_url.clone();
        let volume_trackers = self.volume_trackers.clone();
        let event_times_agg = self.event_times.clone();

        tokio::spawn(async move {
            Self::aggtrade_connection_loop(
                aggtrade_url,
                prices_agg,
                reconnecting_agg,
                symbols_agg,
                ws_url_agg,
                volume_trackers,
                event_times_agg,
            )
            .await
        });

        // Build depth streams URL (3rd connection)
        // Use /stream?streams= combined endpoint so each message includes the stream name.
        // depth20 payloads have no symbol field (unlike bookTicker/aggTrade),
        // so we need the wrapper to route messages to the correct symbol.
        let depth_url = {
            let streams: Vec<String> = self
                .symbols
                .iter()
                .map(|s| format!("{}@depth20@100ms", s.to_lowercase()))
                .collect();
            let base = self.ws_url.replace("/ws", "/stream");
            format!("{}?streams={}", base, streams.join("/"))
        };

        info!("Connecting to Binance WebSocket (depth): {}", depth_url);

        let depth_data_clone = self.depth_data.clone();
        let reconnecting_depth_clone = self.reconnecting_depth.clone();
        let symbols_depth = self.symbols.clone();

        tokio::spawn(async move {
            Self::depth_connection_loop(
                depth_url,
                depth_data_clone,
                reconnecting_depth_clone,
                symbols_depth,
            )
            .await
        });

        Ok(())
    }

    // ========================================================================
    // PRICE/DATA ACCESS METHODS
    // ========================================================================

    /// Get the current price for a symbol
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol (case-insensitive, e.g., "BTCUSDT")
    ///
    /// # Returns
    /// * `Some(BinancePrice)` - If price data is available
    /// * `None` - If symbol not found or no data received yet
    pub async fn get_price(&self, symbol: &str) -> Option<BinancePrice> {
        let prices = self.prices.read().await;
        prices.get(&symbol.to_uppercase()).cloned()
    }

    /// Get bid and ask prices separately for spread calculation
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol (case-insensitive)
    ///
    /// # Returns
    /// * `Some((bid, ask))` - Bid and ask prices as f64
    /// * `None` - If symbol not found
    pub async fn get_bid_ask(&self, symbol: &str) -> Option<(f64, f64)> {
        let prices = self.prices.read().await;
        prices
            .get(&symbol.to_uppercase())
            .map(|p| (p.bid.to_f64().unwrap_or(0.0), p.ask.to_f64().unwrap_or(0.0)))
    }

    /// Get all current prices
    ///
    /// # Returns
    /// A clone of the internal prices HashMap
    pub async fn get_all_prices(&self) -> HashMap<String, BinancePrice> {
        self.prices.read().await.clone()
    }

    /// Check if a symbol has price data
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol
    ///
    /// # Returns
    /// true if price data exists for the symbol
    pub async fn has_price(&self, symbol: &str) -> bool {
        let prices = self.prices.read().await;
        prices.contains_key(&symbol.to_uppercase())
    }

    /// Get rolling window volume for a symbol
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol (case-insensitive)
    ///
    /// # Returns
    /// * `Some((volume_1sec, volume_1min))` - Rolling window volumes in USDT
    /// * `None` - If symbol not found
    ///
    /// # Note
    /// This provides rolling window volume (last 1 second, last 1 minute)
    /// rather than cumulative volume since connection start.
    pub async fn get_rolling_volume(&self, symbol: &str) -> Option<(f64, f64)> {
        let upper_symbol = symbol.to_uppercase();
        let volume_tracker = self.volume_trackers.get(&upper_symbol)?;

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // using aggregate trade event time for more accurate volume tracking
        // Get Binance event time for this symbol (from aggTrade stream)
        // let current_time = {
        //     let event_times = self.event_times.read().await;
        //     event_times.get(&upper_symbol).copied().unwrap_or_else(|| {
        //         std::time::SystemTime::now()
        //             .duration_since(std::time::UNIX_EPOCH)
        //             .unwrap()
        //             .as_millis() as u64
        //     }) as i64
        // };

        let tracker = volume_tracker.lock().await;
        Some((
            tracker.get_volume_1sec(current_time),
            tracker.get_volume_1min(),
        ))
    }

    /// Check if volatility calculator is ready (has enough data)
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol (case-insensitive)
    ///
    /// # Returns
    /// * `Some(true)` - Calculator is bootstrapped and ready
    /// * `Some(false)` - Calculator exists but needs more data
    /// * `None` - Symbol not found
    pub async fn is_volatility_ready(&self, symbol: &str) -> Option<bool> {
        let upper_symbol = symbol.to_uppercase();
        let vol_calculator = self.vol_calculators.get(&upper_symbol)?;
        let calc = vol_calculator.read().await;
        Some(calc.is_bootstrapped())
    }

    /// Get the volatility source path for a symbol
    ///
    /// This returns the actual data source that was used in the last volatility calculation:
    /// - "REALTIME_FIBONACCI" - Using real-time WebSocket tick data
    /// - "1MIN_FALLBACK" - Using 1-minute historical kline data
    /// - "DEFAULT" - Using default volatility (insufficient data)
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol (case-insensitive)
    ///
    /// # Returns
    /// * `Some(String)` - The volatility source path
    /// * `None` - If symbol not found
    pub async fn get_vol_source_path(&self, symbol: &str) -> Option<String> {
        let upper_symbol = symbol.to_uppercase();
        let vol_calculator = self.vol_calculators.get(&upper_symbol)?;
        let calc = vol_calculator.read().await;
        Some(calc.get_vol_source_path().to_string())
    }

    /// Get market snapshot including realized volatility for a symbol
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol (case-insensitive)
    ///
    /// # Returns
    /// * `Some(MarketSnapshot)` - If price data is available
    /// * `None` - If symbol not found or no data received yet
    pub async fn get_market_snapshot(&self, symbol: &str) -> Option<MarketSnapshot> {
        let upper_symbol = symbol.to_uppercase();

        // Clone the price data and release the prices lock FIRST
        // This prevents deadlock while acquiring other locks
        let price_data = {
            let prices = self.prices.read().await;
            prices.get(&upper_symbol).cloned()?
        };

        let spot_price = price_data.mid.to_f64().unwrap_or(0.0);

        // Get Binance event time for this symbol (use the most recent from either stream)
        // This is the event time from Binance's streams, not local computer time
        let current_time = {
            let event_times = self.event_times.read().await;
            event_times.get(&upper_symbol).copied().unwrap_or_else(|| {
                // Fallback to local time if no event time available yet
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
            }) as i64
        };

        // Get volatility calculator for this symbol (prices lock already released)
        // Clone-compute pattern: clone under a brief read lock, then compute on the clone.
        // This prevents lock contention between WS ingestion (add_price) and snapshot reads.
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

        // Get volume tracker for this symbol (prices lock already released)
        let volume_tracker = self.volume_trackers.get(&upper_symbol)?;
        let (volume_1sec, volume_1min, volume_spike) = {
            let vol_tracker = volume_tracker.lock().await;
            (
                vol_tracker.get_volume_1sec(current_time),
                vol_tracker.get_volume_1min(),
                vol_tracker.is_volume_spike(current_time),
            )
        };

        // Calculate spread in basis points
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
    // WEBSOCKET CONNECTION LOOPS (Internal)
    // ========================================================================

    /// Main bookTicker connection loop with auto-reconnect
    async fn bookticker_connection_loop(
        url: String,
        prices: Arc<RwLock<HashMap<String, BinancePrice>>>,
        reconnecting: Arc<RwLock<bool>>,
        _symbols: Vec<String>,
        _ws_url: String,
        vol_calculators: HashMap<String, Arc<RwLock<FibonacciVolatilityCalculator>>>,
        event_times: Arc<RwLock<HashMap<String, u64>>>,
    ) {
        let mut reconnect_delay = tokio::time::Duration::from_secs(1);
        const MAX_RECONNECT_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(30);

        loop {
            // Check if another task is already reconnecting
            // FIX H2: Use sleep+continue instead of return to prevent permanent exit
            // if the flag gets stuck true (e.g., panic before reset)
            {
                let mut reconnecting_guard = reconnecting.write().await;
                if *reconnecting_guard {
                    debug!("BookTicker reconnection already in progress, waiting to retry...");
                    drop(reconnecting_guard);
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
                *reconnecting_guard = true;
            }

            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    // Reset reconnect delay on successful connection
                    reconnect_delay = tokio::time::Duration::from_secs(1);

                    info!("Connected to Binance bookTicker WebSocket");

                    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

                    // Note: Binance sends ping frames, tungstenite handles pong responses automatically
                    debug!("BookTicker message loop started");

                    // Main message loop
                    while let Some(msg_result) = ws_receiver.next().await {
                        match msg_result {
                            Ok(Message::Text(text)) => {
                                if let Err(e) = Self::handle_bookticker_message(
                                    &text,
                                    &prices,
                                    &vol_calculators,
                                    &event_times,
                                )
                                .await
                                {
                                    warn!("Failed to parse bookTicker message: {}", e);
                                }
                            }
                            Ok(Message::Ping(data)) => {
                                // Respond to ping with pong
                                if let Err(e) = ws_sender.send(Message::Pong(data)).await {
                                    error!("Failed to send pong: {}", e);
                                    break;
                                }
                            }
                            Ok(Message::Pong(_)) => {
                                debug!("Received pong from Binance (bookTicker)");
                            }
                            Ok(Message::Close(_)) => {
                                warn!("Binance bookTicker WebSocket connection closed");
                                break;
                            }
                            Ok(Message::Binary(_)) => {
                                // Ignore binary messages
                            }
                            Ok(Message::Frame(_)) => {
                                // Ignore raw frames
                            }
                            Err(e) => {
                                error!("BookTicker WebSocket error: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect to Binance bookTicker WebSocket: {}", e);
                }
            }

            // Clear reconnecting flag
            {
                let mut reconnecting_guard = reconnecting.write().await;
                *reconnecting_guard = false;
            }

            // Exponential backoff before reconnection
            warn!("Reconnecting bookTicker in {:?}...", reconnect_delay);
            tokio::time::sleep(reconnect_delay).await;

            reconnect_delay = std::cmp::min(reconnect_delay * 2, MAX_RECONNECT_DELAY);
        }
    }

    /// Main aggTrade connection loop with auto-reconnect for volume
    async fn aggtrade_connection_loop(
        url: String,
        prices: Arc<RwLock<HashMap<String, BinancePrice>>>,
        reconnecting: Arc<RwLock<bool>>,
        _symbols: Vec<String>,
        _ws_url: String,
        volume_trackers: HashMap<String, Arc<Mutex<VolumeTracker>>>,
        event_times: Arc<RwLock<HashMap<String, u64>>>,
    ) {
        let mut reconnect_delay = tokio::time::Duration::from_secs(1);
        const MAX_RECONNECT_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(30);

        loop {
            // Check if another task is already reconnecting
            // FIX H2: Use sleep+continue instead of return to prevent permanent exit
            // if the flag gets stuck true (e.g., panic before reset)
            {
                let mut reconnecting_guard = reconnecting.write().await;
                if *reconnecting_guard {
                    debug!("AggTrade reconnection already in progress, waiting to retry...");
                    drop(reconnecting_guard);
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
                *reconnecting_guard = true;
            }

            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    // Reset reconnect delay on successful connection
                    reconnect_delay = tokio::time::Duration::from_secs(1);

                    info!("Connected to Binance aggTrade WebSocket");

                    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

                    debug!("AggTrade message loop started");

                    // Main message loop
                    while let Some(msg_result) = ws_receiver.next().await {
                        match msg_result {
                            Ok(Message::Text(text)) => {
                                if let Err(e) = Self::handle_aggtrade_message(
                                    &text,
                                    &prices,
                                    &volume_trackers,
                                    &event_times,
                                )
                                .await
                                {
                                    warn!("Failed to parse aggTrade message: {}", e);
                                }
                            }
                            Ok(Message::Ping(data)) => {
                                // Respond to ping with pong
                                if let Err(e) = ws_sender.send(Message::Pong(data)).await {
                                    error!("Failed to send pong: {}", e);
                                    break;
                                }
                            }
                            Ok(Message::Pong(_)) => {
                                debug!("Received pong from Binance (aggTrade)");
                            }
                            Ok(Message::Close(_)) => {
                                warn!("Binance aggTrade WebSocket connection closed");
                                break;
                            }
                            Ok(Message::Binary(_)) => {
                                // Ignore binary messages
                            }
                            Ok(Message::Frame(_)) => {
                                // Ignore raw frames
                            }
                            Err(e) => {
                                error!("AggTrade WebSocket error: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect to Binance aggTrade WebSocket: {}", e);
                }
            }

            // Clear reconnecting flag
            {
                let mut reconnecting_guard = reconnecting.write().await;
                *reconnecting_guard = false;
            }

            // Exponential backoff before reconnection
            warn!("Reconnecting aggTrade in {:?}...", reconnect_delay);
            tokio::time::sleep(reconnect_delay).await;

            reconnect_delay = std::cmp::min(reconnect_delay * 2, MAX_RECONNECT_DELAY);
        }
    }

    /// Main depth connection loop with auto-reconnect
    async fn depth_connection_loop(
        url: String,
        depth_data: Arc<RwLock<HashMap<String, BinanceDepthData>>>,
        reconnecting: Arc<RwLock<bool>>,
        symbols: Vec<String>,
    ) {
        let mut reconnect_delay = tokio::time::Duration::from_secs(1);
        const MAX_RECONNECT_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(30);

        // Fallback for single-symbol connections (without combined stream wrapper)
        let fallback_symbol = symbols.first().cloned().unwrap_or_default();

        loop {
            {
                let mut reconnecting_guard = reconnecting.write().await;
                if *reconnecting_guard {
                    debug!("Depth reconnection already in progress, waiting to retry...");
                    drop(reconnecting_guard);
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
                *reconnecting_guard = true;
            }

            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    reconnect_delay = tokio::time::Duration::from_secs(1);
                    info!("Connected to Binance depth WebSocket");
                    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
                    debug!("Depth message loop started");

                    while let Some(msg_result) = ws_receiver.next().await {
                        match msg_result {
                            Ok(Message::Text(text)) => {
                                // Combined stream format: {"stream":"btcusdt@depth20@100ms","data":{...}}
                                // Extract symbol from stream name since depth20 payloads have no symbol field
                                if let Ok(combined) = serde_json::from_str::<CombinedDepthMessage>(&text) {
                                    let symbol = combined.stream
                                        .split('@')
                                        .next()
                                        .unwrap_or("")
                                        .to_uppercase();
                                    if let Err(e) = Self::handle_depth_event(combined.data, &depth_data, &symbol).await {
                                        warn!("Failed to process depth for {}: {}", symbol, e);
                                    }
                                } else if let Err(e) = Self::handle_depth_message(&text, &depth_data, &fallback_symbol).await {
                                    warn!("Failed to parse depth message: {}", e);
                                }
                            }
                            Ok(Message::Ping(data)) => {
                                if let Err(e) = ws_sender.send(Message::Pong(data)).await {
                                    error!("Failed to send pong (depth): {}", e);
                                    break;
                                }
                            }
                            Ok(Message::Pong(_)) => {
                                debug!("Received pong from Binance (depth)");
                            }
                            Ok(Message::Close(_)) => {
                                warn!("Binance depth WebSocket connection closed");
                                break;
                            }
                            Ok(Message::Binary(_)) => {}
                            Ok(Message::Frame(_)) => {}
                            Err(e) => {
                                error!("Depth WebSocket error: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect to Binance depth WebSocket: {}", e);
                }
            }

            {
                let mut reconnecting_guard = reconnecting.write().await;
                *reconnecting_guard = false;
            }

            warn!("Reconnecting depth in {:?}...", reconnect_delay);
            tokio::time::sleep(reconnect_delay).await;
            reconnect_delay = std::cmp::min(reconnect_delay * 2, MAX_RECONNECT_DELAY);
        }
    }

    // ========================================================================
    // MESSAGE HANDLERS (Internal)
    // ========================================================================

    /// Handle incoming bookTicker WebSocket message
    async fn handle_bookticker_message(
        text: &str,
        prices: &Arc<RwLock<HashMap<String, BinancePrice>>>,
        vol_calculators: &HashMap<String, Arc<RwLock<FibonacciVolatilityCalculator>>>,
        event_times: &Arc<RwLock<HashMap<String, u64>>>,
    ) -> Result<()> {
        // Parse the bookTicker message
        let ticker: BinanceBookTicker =
            serde_json::from_str(text).map_err(|e| anyhow!("Failed to parse bookTicker: {}", e))?;

        let symbol = ticker.symbol.to_uppercase();
        let bid = ticker
            .bid_price
            .parse::<Decimal>()
            .map_err(|_| anyhow!("Invalid bid price: {}", ticker.bid_price))?;
        let ask = ticker
            .ask_price
            .parse::<Decimal>()
            .map_err(|_| anyhow!("Invalid ask price: {}", ticker.ask_price))?;

        // bookTicker doesn't have an event_time field, so we use the current time
        // when we RECEIVE the message. This represents when the price update arrived.
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // compare using aggregate timestamp
        // Get Binance event time for this symbol (from aggTrade stream)
        // let current_time = {
        //     let event_times = self.event_times.read().await;
        //     event_times.get(&upper_symbol).copied().unwrap_or_else(|| {
        //         std::time::SystemTime::now()
        //             .duration_since(std::time::UNIX_EPOCH)
        //             .unwrap()
        //             .as_millis() as u64
        //     }) as i64
        // };

        // Store this timestamp - it represents the freshest price data timing
        {
            let mut event_times_guard = event_times.write().await;
            event_times_guard.insert(symbol.clone(), current_time);
        }

        // Calculate mid price for volatility tracking
        let mid = (bid + ask) / Decimal::from(2);

        // Update volatility calculator with current time
        let event_time_i64 = current_time as i64;
        let mid_price = mid.to_f64().unwrap_or(0.0);
        if mid_price > 0.0 {
            if let Some(vol_calculator) = vol_calculators.get(&symbol) {
                let mut vol_calc = vol_calculator.write().await;
                vol_calc.add_price(event_time_i64, mid_price);
            }
        }

        // Update prices store, preserving existing volume
        {
            let mut prices_guard = prices.write().await;
            let volume = prices_guard
                .get(&symbol)
                .map(|p| p.volume)
                .unwrap_or_else(|| Decimal::from(0));
            let price = BinancePrice::new(bid, ask, volume);
            prices_guard.insert(symbol.clone(), price);
        }

        debug!(
            "Updated bookTicker for {}: bid={}, ask={}",
            symbol, bid, ask
        );

        Ok(())
    }

    /// Handle incoming aggTrade WebSocket message for volume updates
    ///
    /// FIX: Volume tracking moved entirely to VolumeTracker for rolling window calculations.
    /// The BinancePrice.volume field is now only set by bookTicker and serves as a reference.
    /// Rolling window volume (1sec, 1min) is available via get_market_snapshot().
    async fn handle_aggtrade_message(
        text: &str,
        _prices: &Arc<RwLock<HashMap<String, BinancePrice>>>,
        volume_trackers: &HashMap<String, Arc<Mutex<VolumeTracker>>>,
        event_times: &Arc<RwLock<HashMap<String, u64>>>,
    ) -> Result<()> {
        // Parse the aggTrade message
        let trade: BinanceAggTradeEvent =
            serde_json::from_str(text).map_err(|e| anyhow!("Failed to parse aggTrade: {}", e))?;

        let symbol = trade.symbol.to_uppercase();
        let price = trade
            .price
            .parse::<Decimal>()
            .map_err(|_| anyhow!("Invalid trade price: {}", trade.price))?;
        let quantity = trade
            .quantity
            .parse::<Decimal>()
            .map_err(|_| anyhow!("Invalid trade quantity: {}", trade.quantity))?;

        // Store the Binance event time (exchange-level timestamp)
        // This preserves the true timing edge vs RTDS Chainlink/Binance relay feeds.
        // Chainlink prices go through: aggregation (latency 1) → Polygon block time (latency 2)
        // Using exchange time lets RTDS detect exactly how stale oracle prices are.
        // Note: bookticker uses local time because bookTicker messages lack event_time.
        // aggTrade messages DO have event_time — the actual exchange trade timestamp.
        {
            let mut event_times_guard = event_times.write().await;
            event_times_guard.insert(symbol.clone(), trade.event_time);
        }

        // Calculate volume in USDT (price * quantity)
        let trade_volume = price * quantity;
        let trade_volume_f64 = trade_volume.to_f64().unwrap_or(0.0);

        // Update volume tracker with Binance event time (not local time)
        if trade_volume_f64 > 0.0 {
            if let Some(volume_tracker) = volume_trackers.get(&symbol) {
                let mut vol_tracker = volume_tracker.lock().await;
                let event_time_i64 = trade.event_time as i64;
                vol_tracker.add_trade(event_time_i64, trade_volume_f64);
            }
        }

        // FIX: Remove the race condition by NOT updating prices here for volume.
        // The volume field in BinancePrice is now purely for display/reference and
        // is only updated by bookTicker. For accurate rolling window volume,
        // use get_market_snapshot() which provides volume_1sec and volume_1min.
        //
        // Previous code had a read-modify-write race condition:
        //   let existing = prices_guard.get(&symbol);  // READ
        //   let new_volume = existing_price.volume + trade_volume;  // MODIFY
        //   prices_guard.insert(symbol.clone(), updated);  // WRITE
        // Multiple concurrent trades could lose updates.

        debug!(
            "Updated volume tracker for {}: trade_volume={}",
            symbol, trade_volume
        );

        Ok(())
    }

    /// Handle incoming partial book depth WebSocket message (raw JSON string)
    async fn handle_depth_message(
        text: &str,
        depth_data: &Arc<RwLock<HashMap<String, BinanceDepthData>>>,
        symbol: &str,
    ) -> Result<()> {
        let event: BinanceDepthEvent =
            serde_json::from_str(text).map_err(|e| anyhow!("Failed to parse depth: {}", e))?;
        Self::handle_depth_event(event, depth_data, symbol).await
    }

    /// Process a pre-parsed depth event (used by both raw and combined stream paths)
    async fn handle_depth_event(
        event: BinanceDepthEvent,
        depth_data: &Arc<RwLock<HashMap<String, BinanceDepthData>>>,
        symbol: &str,
    ) -> Result<()> {
        let upper_symbol = symbol.to_uppercase();

        let bid_quantities: Vec<f64> = event
            .bids
            .iter()
            .filter_map(|level| level[1].parse::<f64>().ok())
            .filter(|qty| *qty > 0.0)
            .collect();
        let ask_quantities: Vec<f64> = event
            .asks
            .iter()
            .filter_map(|level| level[1].parse::<f64>().ok())
            .filter(|qty| *qty > 0.0)
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
            .as_millis() as u64;

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
            depth_guard.insert(upper_symbol.clone(), data);
        }

        debug!(
            "Updated depth for {}: OBI={:.4}, bids={}, asks={}",
            upper_symbol,
            book_imbalance,
            bid_quantities.len(),
            ask_quantities.len()
        );

        Ok(())
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

    #[test]
    fn test_binance_price_new() {
        let bid = Decimal::from(95000);
        let ask = Decimal::from(95100);
        let volume = Decimal::from(1000000);
        let price = BinancePrice::new(bid, ask, volume);

        assert_eq!(price.bid, bid);
        assert_eq!(price.ask, ask);
        assert_eq!(price.mid, Decimal::from(95050));
        assert_eq!(price.volume, volume);
    }

    #[test]
    fn test_binance_price_new_no_volume() {
        let bid = Decimal::from(95000);
        let ask = Decimal::from(95100);
        let price = BinancePrice::new_no_volume(bid, ask);

        assert_eq!(price.bid, bid);
        assert_eq!(price.ask, ask);
        assert_eq!(price.mid, Decimal::from(95050));
        assert_eq!(price.volume, Decimal::from(0));
    }

    #[tokio::test]
    async fn test_binance_data_manager_new() {
        let manager = BinanceDataManager::new();
        assert!(!manager.has_price("BTCUSDT").await);
        assert!(manager.get_price("BTCUSDT").await.is_none());
    }

    #[tokio::test]
    async fn test_binance_data_manager_default() {
        let manager = BinanceDataManager::default();
        assert!(!manager.has_price("BTCUSDT").await);
    }

    #[tokio::test]
    async fn test_binance_data_manager_with_url() {
        let manager = BinanceDataManager::with_url("wss://testnet.binance.vision/ws".to_string());
        // URL is stored but not directly exposed
        assert!(!manager.has_price("BTCUSDT").await);
    }

    #[test]
    fn test_kline_from_json() {
        let json_arr: Vec<serde_json::Value> = serde_json::from_str(
            r#"[1609459200000,"29000.00","29500.00","28500.00","29200.00","1000.0",1609459259999,"29000000.0",5000,"500.0","14500000.0","0"]"#
        ).unwrap();

        let kline = Kline::from_json(&json_arr).unwrap();

        assert_eq!(kline.open_time, 1609459200000);
        assert_eq!(kline.open, 29000.0);
        assert_eq!(kline.high, 29500.0);
        assert_eq!(kline.low, 28500.0);
        assert_eq!(kline.close, 29200.0);
        assert_eq!(kline.volume, 1000.0);
        assert_eq!(kline.close_time, 1609459259999);
        assert_eq!(kline.quote_volume, 29000000.0);
        assert_eq!(kline.trades, 5000);
    }

    #[test]
    fn test_kline_from_json_invalid() {
        let json_arr: Vec<serde_json::Value> = serde_json::from_str(r#"[1, 2, 3]"#).unwrap();
        let kline = Kline::from_json(&json_arr);
        assert!(kline.is_none());
    }

    #[test]
    fn test_binance_depth_data_default() {
        let data = BinanceDepthData::default();
        assert_eq!(data.bid_depth_3, 0.0);
        assert_eq!(data.ask_depth_3, 0.0);
        assert_eq!(data.book_imbalance, 0.0);
        assert_eq!(data.bid_levels, 0);
        assert_eq!(data.ask_levels, 0);
    }

    #[test]
    fn test_binance_depth_event_parse() {
        let json = r#"{
            "lastUpdateId": 160,
            "bids": [["97000.00", "1.5"], ["96999.00", "0.8"], ["96998.00", "2.1"]],
            "asks": [["97001.00", "0.9"], ["97002.00", "1.2"], ["97003.00", "0.5"]]
        }"#;
        let event: BinanceDepthEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.bids.len(), 3);
        assert_eq!(event.asks.len(), 3);
        assert_eq!(event._last_update_id, 160);
    }

    #[tokio::test]
    async fn test_get_depth_data_none_before_connect() {
        let manager = BinanceDataManager::new();
        assert!(manager.get_depth_data("BTCUSDT").await.is_none());
    }

    #[tokio::test]
    async fn test_handle_depth_message() {
        let depth_data = Arc::new(RwLock::new(HashMap::new()));
        let json = r#"{
            "lastUpdateId": 160,
            "bids": [["97000.00", "1.5"], ["96999.00", "0.8"], ["96998.00", "2.1"], ["96997.00", "0.3"], ["96996.00", "1.0"]],
            "asks": [["97001.00", "0.9"], ["97002.00", "1.2"], ["97003.00", "0.5"], ["97004.00", "0.7"], ["97005.00", "0.4"]]
        }"#;

        BinanceDataManager::handle_depth_message(json, &depth_data, "BTCUSDT")
            .await
            .unwrap();

        let guard = depth_data.read().await;
        let data = guard.get("BTCUSDT").unwrap();

        assert!((data.bid_depth_3 - 4.4).abs() < 0.001);
        assert!((data.ask_depth_3 - 2.6).abs() < 0.001);
        assert!((data.bid_depth_all - 5.7).abs() < 0.001);
        assert!((data.ask_depth_all - 3.7).abs() < 0.001);
        assert_eq!(data.bid_levels, 5);
        assert_eq!(data.ask_levels, 5);
        assert!((data.book_imbalance - 0.2128).abs() < 0.001);
        assert!(data.last_update_ms > 0);
    }
}
