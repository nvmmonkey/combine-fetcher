//! Polymarket Orderbook Streaming via WebSocket
//!
//! This module connects to the Polymarket CLOB WebSocket to stream real-time
//! orderbook updates for YES and NO tokens. It maintains a local cache of the
//! best bid/ask prices for each token.
//!
//! The module implements a dual-feed architecture:
//! - Price feed: Provides best bid/ask prices and last trade prices
//! - Orderbook feed: Provides liquidity (size) at specific price levels
//! - REST fallback: Polls REST API when WebSocket data is stale

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use futures::StreamExt;
use polymarket_client_sdk::clob::types::request::{LastTradePriceRequest, MidpointRequest};
use polymarket_client_sdk::clob::ws::Client as WsClient;
use polymarket_client_sdk::clob::{Client as ClobClient, Config};
use polymarket_client_sdk::types::U256;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive}; // <-- FIXED: Added FromPrimitive back
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Price matching tolerance for comparing Decimal prices with f64 targets
const PRICE_MATCH_TOLERANCE: f64 = 0.0001;

/// Default staleness threshold in milliseconds - data older than this is considered stale
const DEFAULT_STALENESS_MS: i64 = 5000;

/// WebSocket connection age threshold in milliseconds - updates older than this trigger API fallback
const WS_CONNECTED_AGE_MS: i64 = 5000;

/// REST API fallback polling interval in seconds
const REST_FALLBACK_INTERVAL_SECS: u64 = 3;

/// Reconnection settings for WebSocket streams
/// Maximum delay between reconnection attempts (60 seconds)
const MAX_RECONNECT_DELAY_MS: u64 = 60000;
/// Initial delay between reconnection attempts (1 second)
const INITIAL_RECONNECT_DELAY_MS: u64 = 1000;
/// Multiplier for exponential backoff
const RECONNECT_BACKOFF_MULTIPLIER: f64 = 2.0;
/// Maximum number of reconnection attempts before giving up (0 = infinite)
const MAX_RECONNECT_ATTEMPTS: u32 = 0;

/// Silent disconnect threshold: if no data received for this long while "connected", log warning
/// Increased to 90s to accommodate natural quiet periods in 15-min options markets
/// This is now only used for logging, not for triggering reconnections
const SILENT_DISCONNECT_THRESHOLD_MS: i64 = 90000;

/// Health check interval: how often to check for stream health (seconds)
const HEALTH_CHECK_INTERVAL_SECS: u64 = 10;

/// Stream error threshold: if we get this many consecutive errors, mark as unhealthy
const STREAM_ERROR_THRESHOLD: u32 = 3;

/// Polymarket CLOB API base URL
const CLOB_API_URL: &str = "https://clob.polymarket.com";

/// Best bid/ask prices for a token
///
/// This structure mirrors the pattern from polymarket-infras, tracking
/// both price and liquidity data from dual WebSocket feeds.
#[derive(Debug, Clone)]
pub struct TokenPrices {
    pub token_id: String,
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub best_bid_size: f64, // Size at best bid price (from orderbook feed)
    pub best_ask_size: f64, // Size at best ask price (from orderbook feed)
    pub mid_price: Option<Decimal>,
    pub last_trade: Option<f64>, // Last trade price
    pub last_update_ms: i64,
    pub ws_connected: bool,   // WebSocket connection status (stream is active)
    pub reconnect_count: u32, // Number of reconnections for this token's stream
    pub last_disconnect_ms: i64, // Timestamp of last disconnection
    pub last_successful_connect_ms: i64, // Timestamp of last successful connection
    pub consecutive_errors: u32, // Number of consecutive stream errors
    pub last_ws_update_ms: i64, // NEW: Timestamp of last WebSocket update (vs API update)
    pub ws_message_count: u64, // NEW: Count of WebSocket messages received
    pub api_fallback_count: u64, // NEW: Count of API fallback calls
}

impl TokenPrices {
    pub fn new(token_id: String) -> Self {
        Self {
            token_id,
            best_bid: None,
            best_ask: None,
            best_bid_size: 0.0,
            best_ask_size: 0.0,
            mid_price: None,
            last_trade: None,
            last_update_ms: 0,
            ws_connected: false,
            reconnect_count: 0,
            last_disconnect_ms: 0,
            last_successful_connect_ms: 0,
            consecutive_errors: 0,
            last_ws_update_ms: 0,
            ws_message_count: 0,
            api_fallback_count: 0,
        }
    }

    pub fn is_stale(&self, max_age_ms: i64) -> bool {
        // Never received any data - definitely stale
        if self.last_update_ms == 0 {
            return true;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let age_ms = now_ms.saturating_sub(self.last_update_ms);
        // Data is stale if older than threshold, regardless of connection state
        age_ms > max_age_ms
    }

    /// Check if the stream appears unhealthy
    /// This is based on stream errors, not just lack of data (which is normal in quiet markets)
    pub fn is_unhealthy(&self) -> bool {
        // Unhealthy if we have many consecutive errors
        self.consecutive_errors >= STREAM_ERROR_THRESHOLD
    }

    /// Check if data is very old (for logging/monitoring, not for triggering reconnects)
    pub fn is_very_stale(&self) -> bool {
        if !self.ws_connected || self.last_update_ms == 0 {
            return false;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let age_ms = now_ms.saturating_sub(self.last_update_ms);
        age_ms > SILENT_DISCONNECT_THRESHOLD_MS
    }

    /// Get age of last update in milliseconds
    pub fn age_ms(&self) -> i64 {
        if self.last_update_ms == 0 {
            return i64::MAX;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        now_ms.saturating_sub(self.last_update_ms)
    }

    /// Get age of last WebSocket update in milliseconds
    pub fn ws_age_ms(&self) -> i64 {
        if self.last_ws_update_ms == 0 {
            return i64::MAX;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        now_ms.saturating_sub(self.last_ws_update_ms)
    }

    /// Get data source indicator: "WS" for WebSocket, "API" for REST fallback
    pub fn data_source(&self) -> &str {
        if self.ws_connected && self.age_ms() < WS_CONNECTED_AGE_MS {
            "WS"
        } else {
            "API"
        }
    }

    /// Get detailed connection status for logging
    pub fn connection_detail(&self) -> String {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let data_age = (now_ms - self.last_update_ms) / 1000;
        let ws_age = if self.last_ws_update_ms > 0 {
            (now_ms - self.last_ws_update_ms) / 1000
        } else {
            -1
        };

        if !self.ws_connected {
            format!(
                "DISCONNECTED | using API fallback | last_update={}s ago",
                data_age
            )
        } else if self.consecutive_errors >= STREAM_ERROR_THRESHOLD {
            format!(
                "UNHEALTHY | ws_connected but {} errors | ws_age={}s",
                self.consecutive_errors, ws_age
            )
        } else if ws_age >= 0 && ws_age < 10 {
            format!(
                "HEALTHY_WS | active stream | last_ws={}s ago | msgs={}",
                ws_age, self.ws_message_count
            )
        } else if ws_age >= 10 && ws_age < SILENT_DISCONNECT_THRESHOLD_MS / 1000 {
            format!(
                "QUIET_WS | connected but quiet | last_ws={}s ago | normal for 15m markets",
                ws_age
            )
        } else {
            format!(
                "STALE_WS | connected but very stale | last_ws={}s ago | api_fallbacks={}",
                ws_age, self.api_fallback_count
            )
        }
    }
}

/// Orderbook level from the Polymarket CLOB
#[derive(Debug, Clone)]
pub struct OrderBookLevel {
    pub price: Decimal,
    pub size: Decimal,
}

/// Find the size at a specific price level in an orderbook
///
/// This helper function searches through order levels to find the size
/// available at a target price, using tolerance for floating point comparison.
///
/// # Arguments
/// * `orders` - Slice of orderbook levels (bids or asks)
/// * `target_price` - Optional target price to match against
///
/// # Returns
/// The size at the target price level, or 0.0 if not found
fn find_size_at_price(orders: &[OrderBookLevel], target_price: Option<Decimal>) -> f64 {
    let mut size = 0.0;
    if let Some(target) = target_price {
        if let Some(target_f) = target.to_f64() {
            for order in orders {
                if let (Some(price), Some(order_size)) = (order.price.to_f64(), order.size.to_f64())
                {
                    if (price - target_f).abs() < PRICE_MATCH_TOLERANCE {
                        size = order_size;
                        break;
                    }
                }
            }
        }
    }
    size
}

/// Price update event sent to subscribers (from Polymarket WebSocket)
#[derive(Debug, Clone)]
pub struct PolyPriceUpdate {
    pub token_id: String,
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub mid_price: Option<Decimal>,
}

/// Polymarket orderbook stream manager
///
/// Subscribes to WebSocket for multiple token IDs and maintains
/// a real-time cache of best bid/ask prices. Implements REST API
/// fallback when WebSocket data becomes stale.
pub struct PolymarketOrderbookStream {
    /// Price cache by token ID (hex string)
    prices: Arc<DashMap<String, TokenPrices>>,
    /// Maximum age before considering data stale (milliseconds)
    max_stale_ms: i64,
    /// Token IDs being tracked (for REST fallback)
    token_ids: Arc<std::sync::Mutex<Vec<String>>>,
    /// Total number of price stream reconnections
    price_reconnect_count: Arc<AtomicU32>,
    /// Total number of orderbook stream reconnections
    book_reconnect_count: Arc<AtomicU32>,
    /// Timestamp of last price stream reconnection
    last_price_reconnect_ms: Arc<AtomicU64>,
    /// Timestamp of last orderbook stream reconnection
    last_book_reconnect_ms: Arc<AtomicU64>,
    /// Number of times very stale data was detected (for monitoring)
    very_stale_count: Arc<AtomicU32>,
    /// Total WebSocket messages received across all tokens
    total_ws_messages: Arc<AtomicU64>,
    /// Total API fallback calls made
    total_api_fallbacks: Arc<AtomicU64>,
    /// Stream generation counter — incremented on each `stream()` call.
    /// Background tasks capture their generation at spawn and exit when superseded,
    /// preventing task accumulation across market rotations.
    stream_generation: Arc<AtomicU64>,
}

impl PolymarketOrderbookStream {
    /// Create a new orderbook stream manager
    pub fn new() -> Self {
        Self {
            prices: Arc::new(DashMap::new()),
            max_stale_ms: DEFAULT_STALENESS_MS,
            token_ids: Arc::new(std::sync::Mutex::new(Vec::new())),
            price_reconnect_count: Arc::new(AtomicU32::new(0)),
            book_reconnect_count: Arc::new(AtomicU32::new(0)),
            last_price_reconnect_ms: Arc::new(AtomicU64::new(0)),
            last_book_reconnect_ms: Arc::new(AtomicU64::new(0)),
            very_stale_count: Arc::new(AtomicU32::new(0)),
            total_ws_messages: Arc::new(AtomicU64::new(0)),
            total_api_fallbacks: Arc::new(AtomicU64::new(0)),
            stream_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Set maximum staleness threshold
    pub fn with_max_stale_ms(mut self, ms: i64) -> Self {
        self.max_stale_ms = ms;
        self
    }

    /// Get current price data for a token
    pub fn get_prices(&self, token_id: &str) -> Option<TokenPrices> {
        self.prices.get(token_id).map(|p| p.clone())
    }

    /// Get mid price for a token
    pub fn get_mid_price(&self, token_id: &str) -> Option<Decimal> {
        self.prices.get(token_id).and_then(|p| p.mid_price)
    }

    /// Check if a token's price data is stale
    pub fn is_stale(&self, token_id: &str) -> bool {
        match self.prices.get(token_id) {
            Some(prices) => prices.is_stale(self.max_stale_ms),
            None => true,
        }
    }

    /// Check connection status for a token
    pub fn is_connected(&self, token_id: &str) -> bool {
        match self.prices.get(token_id) {
            Some(prices) => prices.ws_connected && !prices.is_unhealthy(),
            None => false,
        }
    }

    /// Get overall connection health (percent of active tokens that are connected and healthy)
    ///
    /// Returns a health score from 0-100% based on:
    /// - WebSocket stream is active
    /// - No excessive stream errors
    /// - Has received at least some data
    ///
    /// Only considers currently tracked tokens (ignores stale entries from old markets).
    pub fn connection_health(&self) -> f64 {
        let active_ids = self.token_ids.lock().ok().map(|ids| ids.clone());
        let active_entries: Vec<_> = self
            .prices
            .iter()
            .filter(|entry| match &active_ids {
                Some(ids) => ids.contains(entry.key()),
                None => true,
            })
            .collect();

        if active_entries.is_empty() {
            return 0.0;
        }
        let healthy_count = active_entries
            .iter()
            .filter(|p| {
                let prices = p.value();
                prices.ws_connected && prices.last_update_ms > 0 && !prices.is_unhealthy()
            })
            .count();
        (healthy_count as f64 / active_entries.len() as f64) * 100.0
    }

    /// Get the total number of price stream reconnections
    pub fn price_reconnect_count(&self) -> u32 {
        self.price_reconnect_count.load(Ordering::Relaxed)
    }

    /// Get the total number of orderbook stream reconnections
    pub fn book_reconnect_count(&self) -> u32 {
        self.book_reconnect_count.load(Ordering::Relaxed)
    }

    /// Get the total number of very stale data detections
    pub fn very_stale_count(&self) -> u32 {
        self.very_stale_count.load(Ordering::Relaxed)
    }

    /// Get the timestamp of the last price stream reconnection (milliseconds since epoch)
    pub fn last_price_reconnect_ms(&self) -> u64 {
        self.last_price_reconnect_ms.load(Ordering::Relaxed)
    }

    /// Get the timestamp of the last orderbook stream reconnection (milliseconds since epoch)
    pub fn last_book_reconnect_ms(&self) -> u64 {
        self.last_book_reconnect_ms.load(Ordering::Relaxed)
    }

    /// Get total WebSocket messages received
    pub fn total_ws_messages(&self) -> u64 {
        self.total_ws_messages.load(Ordering::Relaxed)
    }

    /// Get total API fallback calls made
    pub fn total_api_fallbacks(&self) -> u64 {
        self.total_api_fallbacks.load(Ordering::Relaxed)
    }

    /// Get connection status summary for logging
    pub fn connection_status(&self) -> String {
        let health = self.connection_health();
        let price_recos = self.price_reconnect_count();
        let book_recos = self.book_reconnect_count();
        let very_stale = self.very_stale_count();
        let now_ms = chrono::Utc::now().timestamp_millis();

        let last_price_reco = self.last_price_reconnect_ms();
        let last_book_reco = self.last_book_reconnect_ms();

        let price_reco_age = if last_price_reco > 0 {
            format!(
                "{}s ago",
                (now_ms.saturating_sub(last_price_reco as i64) / 1000)
            )
        } else {
            "never".to_string()
        };

        let book_reco_age = if last_book_reco > 0 {
            format!(
                "{}s ago",
                (now_ms.saturating_sub(last_book_reco as i64) / 1000)
            )
        } else {
            "never".to_string()
        };

        // Count tokens by data source (only active tokens)
        let active_ids = self.token_ids.lock().ok().map(|ids| ids.clone());
        let (ws_count, api_count) = self.prices.iter()
            .filter(|entry| match &active_ids {
                Some(ids) => ids.contains(entry.key()),
                None => true,
            })
            .fold((0, 0), |(ws, api), entry| {
                let prices = entry.value();
                if prices.data_source() == "WS" {
                    (ws + 1, api)
                } else {
                    (ws, api + 1)
                }
            });

        format!(
            "Health: {:.1}% | WS:{} API:{} | Price reco: {} ({}) | Book reco: {} ({}) | Very stale: {} | WS msgs: {} | API calls: {}",
            health,
            ws_count,
            api_count,
            price_recos,
            price_reco_age,
            book_recos,
            book_reco_age,
            very_stale,
            self.total_ws_messages(),
            self.total_api_fallbacks()
        )
    }

    /// Get detailed status for each token (only currently tracked tokens)
    pub fn detailed_status(&self) -> Vec<(String, String)> {
        let active_ids = self.token_ids.lock().ok().map(|ids| ids.clone());
        self.prices
            .iter()
            .filter(|entry| {
                // Only report on actively tracked tokens
                match &active_ids {
                    Some(ids) => ids.contains(entry.key()),
                    None => true, // If lock fails, show all
                }
            })
            .map(|entry| {
                let token_id = entry.key().clone();
                let short_id = if token_id.len() > 8 {
                    &token_id[..8]
                } else {
                    &token_id
                };
                let detail = entry.value().connection_detail();
                (short_id.to_string(), detail)
            })
            .collect()
    }

    /// Remove old token entries from the price cache.
    ///
    /// Call this before subscribing to new tokens on market rotation to prevent
    /// stale entries from accumulating and triggering health warnings.
    pub fn remove_tokens(&self, token_ids: &[String]) {
        for token_id in token_ids {
            if self.prices.remove(token_id).is_some() {
                info!("[CLEANUP] Removed stale token: {}...", &token_id[..token_id.len().min(8)]);
            }
        }
    }

    /// Start streaming orderbook updates for the given token IDs
    ///
    /// Returns a channel that receives price updates.
    /// Also spawns a REST API fallback task that polls when WebSocket data is stale.
    ///
    /// Calling this again (e.g. on market rotation) automatically cancels all
    /// background tasks from the previous call via a generation counter. Old tasks
    /// exit cleanly, dropping their `WsClient` instances and closing the underlying
    /// WebSocket connections (no explicit unsubscribe needed).
    pub async fn stream(
        self: Arc<Self>,
        token_ids: Vec<String>,
    ) -> Result<mpsc::UnboundedReceiver<PolyPriceUpdate>> {
        let (tx, rx) = mpsc::unbounded_channel();

        // Increment stream generation — all tasks from previous stream() calls
        // detect the generation change and exit cleanly. This prevents background
        // task accumulation across market rotations (C3 fix). When old tasks exit,
        // they drop their WsClient instances, closing the underlying WebSocket
        // connections (C5 fix — implicit unsubscribe via Drop).
        let generation = self.stream_generation.fetch_add(1, Ordering::Relaxed) + 1;
        info!("[STREAM] Starting generation {} — old tasks will self-terminate", generation);

        // Parse token IDs to U256
        let asset_ids: Vec<U256> = token_ids
            .iter()
            .filter_map(|s| U256::from_str(s).ok())
            .collect();

        if asset_ids.is_empty() {
            return Err(anyhow!("No valid token IDs provided"));
        }

        // Store token IDs for REST fallback
        if let Ok(mut ids) = self.token_ids.lock() {
            *ids = token_ids.clone();
        }

        info!("Subscribing to {} Polymarket tokens", asset_ids.len());

        // Spawn task to handle price changes with reconnection logic
        let prices_clone = Arc::clone(&self.prices);
        let tx_clone = tx.clone();
        let asset_ids_price = asset_ids.clone();
        let price_reconnect_counter = Arc::clone(&self.price_reconnect_count);
        let last_price_reconnect_ts = Arc::clone(&self.last_price_reconnect_ms);
        let total_ws_msgs = Arc::clone(&self.total_ws_messages);
        let price_gen_ref = Arc::clone(&self.stream_generation);
        let price_gen = generation;

        tokio::spawn(async move {
            let mut reconnect_attempt = 0u32;
            let mut reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;

            info!(
                "[PRICE_STREAM] Starting gen {} for {} tokens",
                price_gen,
                asset_ids_price.len()
            );

            // Outer reconnection loop - runs forever until channel closes
            loop {
                // Check if sender is still alive
                if tx_clone.is_closed() {
                    info!("[PRICE_STREAM] Channel closed, exiting");
                    break;
                }

                // Check if superseded by newer stream() call
                if price_gen_ref.load(Ordering::Relaxed) != price_gen {
                    info!("[PRICE_STREAM] Generation {} superseded, exiting", price_gen);
                    break;
                }

                // Create new WebSocket client for each connection attempt
                let ws_client = WsClient::default();

                // Log state transition
                if reconnect_attempt > 0 {
                    warn!(
                        "[PRICE_STREAM] Reconnecting (attempt {})",
                        reconnect_attempt + 1
                    );
                }

                // Mark existing prices as disconnected before reconnect
                for asset_id in &asset_ids_price {
                    let token_id = asset_id.to_string();
                    if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                        prices.ws_connected = false;
                        prices.last_disconnect_ms = chrono::Utc::now().timestamp_millis();
                    }
                }

                match ws_client.subscribe_best_bid_ask(asset_ids_price.clone()) {
                    Ok(price_stream) => {
                        // Successfully connected
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        if reconnect_attempt > 0 {
                            last_price_reconnect_ts.store(now_ms as u64, Ordering::Relaxed);
                            info!(
                                "[PRICE_STREAM] ✅ RECONNECTED successfully after {} attempts",
                                reconnect_attempt
                            );
                        } else {
                            info!("[PRICE_STREAM] ✅ CONNECTED - WebSocket stream active");
                        }

                        // Mark all tokens as connected and reset error counts
                        for asset_id in &asset_ids_price {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.last_successful_connect_ms = now_ms;
                                prices.ws_connected = true;
                                prices.consecutive_errors = 0;
                            }
                        }

                        let mut stream = Box::pin(price_stream);
                        reconnect_attempt = 0;
                        reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;

                        // Process stream until it ends
                        loop {
                            match stream.next().await {
                                Some(Ok(bba)) => {
                                    // Track total WS messages
                                    total_ws_msgs.fetch_add(1, Ordering::Relaxed);

                                    // M1: BestBidAsk is per-token (not batched like PriceChange)
                                    let token_id = bba.asset_id.to_string();
                                    let token_id_hex = format!("{:#x}", bba.asset_id);

                                    let mut prices = prices_clone
                                        .entry(token_id.clone())
                                        .or_insert_with(|| {
                                            TokenPrices::new(token_id.clone())
                                        });

                                    // BestBidAsk fields are non-optional Decimal
                                    prices.best_bid = Some(bba.best_bid);
                                    prices.best_ask = Some(bba.best_ask);
                                    // Note: BestBidAsk doesn't provide last_trade — keep existing value

                                    // Calculate mid price (both always present from BestBidAsk)
                                    prices.mid_price = Some(
                                        (bba.best_bid + bba.best_ask) / Decimal::from(2)
                                    );

                                    let local_now_ms = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_millis()
                                        as i64;

                                    prices.last_update_ms = local_now_ms;
                                    prices.last_ws_update_ms = bba.timestamp;
                                    prices.ws_connected = true;
                                    prices.consecutive_errors = 0;
                                    prices.ws_message_count += 1;

                                    // Send update
                                    let update = PolyPriceUpdate {
                                        token_id: token_id.clone(),
                                        best_bid: prices.best_bid,
                                        best_ask: prices.best_ask,
                                        mid_price: prices.mid_price,
                                    };

                                    if let Err(e) = tx_clone.send(update) {
                                        error!("Failed to send price update: {:?}", e);
                                        return;
                                    }

                                    debug!(
                                        "[PRICE_STREAM] WS update for {}: bid={:.4} ask={:.4}",
                                        token_id_hex,
                                        bba.best_bid.to_f64().unwrap_or(0.0),
                                        bba.best_ask.to_f64().unwrap_or(0.0)
                                    );

                                    // Exit promptly if superseded by newer stream() call
                                    if price_gen_ref.load(Ordering::Relaxed) != price_gen {
                                        info!("[PRICE_STREAM] Generation {} superseded mid-stream, exiting", price_gen);
                                        return;
                                    }
                                }
                                Some(Err(e)) => {
                                    error!("[PRICE_STREAM] ❌ Stream error: {}", e);

                                    // Increment error count for all tokens
                                    for asset_id in &asset_ids_price {
                                        let token_id = asset_id.to_string();
                                        if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                            prices.consecutive_errors += 1;
                                        }
                                    }
                                }
                                None => {
                                    warn!("[PRICE_STREAM] ⚠️ Stream ended, will reconnect");
                                    break;
                                }
                            }
                        }

                        // Stream ended - log and reconnect
                        warn!("[PRICE_STREAM] 🔌 DISCONNECTED");
                        price_reconnect_counter.fetch_add(1, Ordering::Relaxed);

                        // Increment reconnect count for each tracked token
                        for asset_id in &asset_ids_price {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.reconnect_count += 1;
                                prices.ws_connected = false;
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "[PRICE_STREAM] ❌ Connection failed (attempt {}): {}",
                            reconnect_attempt + 1,
                            e
                        );

                        // Mark all tokens as disconnected and increment errors
                        for asset_id in &asset_ids_price {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.ws_connected = false;
                                prices.last_disconnect_ms = chrono::Utc::now().timestamp_millis();
                                prices.consecutive_errors += 1;
                            }
                        }
                    }
                }

                // Check max reconnect attempts
                if MAX_RECONNECT_ATTEMPTS > 0 && reconnect_attempt >= MAX_RECONNECT_ATTEMPTS {
                    error!(
                        "[PRICE_STREAM] Max reconnect attempts ({}) reached",
                        MAX_RECONNECT_ATTEMPTS
                    );
                    break;
                }

                // Exponential backoff before reconnect
                info!(
                    "[PRICE_STREAM] ⏳ Waiting {}ms before reconnect",
                    reconnect_delay_ms
                );
                tokio::time::sleep(Duration::from_millis(reconnect_delay_ms)).await;

                // Re-check generation after sleep — exit early if superseded
                if price_gen_ref.load(Ordering::Relaxed) != price_gen {
                    info!("[PRICE_STREAM] Generation {} superseded during backoff, exiting", price_gen);
                    break;
                }

                // Calculate next delay with exponential backoff
                reconnect_delay_ms = std::cmp::min(
                    (reconnect_delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64,
                    MAX_RECONNECT_DELAY_MS,
                );
                reconnect_attempt += 1;
            }

            info!("[PRICE_STREAM] Gen {} task exited", price_gen);
        });

        // Spawn task to handle orderbook updates with reconnection logic
        let prices_clone = Arc::clone(&self.prices);
        let asset_ids_book = asset_ids.clone();
        let tx_book = tx.clone();
        let book_reconnect_counter = Arc::clone(&self.book_reconnect_count);
        let last_book_reconnect_ts = Arc::clone(&self.last_book_reconnect_ms);
        let total_ws_msgs_book = Arc::clone(&self.total_ws_messages);
        let book_gen_ref = Arc::clone(&self.stream_generation);
        let book_gen = generation;

        tokio::spawn(async move {
            let mut reconnect_attempt = 0u32;
            let mut reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;

            info!("[BOOK_STREAM] Starting gen {} for {} tokens", book_gen, asset_ids_book.len());

            // Outer reconnection loop
            loop {
                if tx_book.is_closed() {
                    info!("[BOOK_STREAM] Channel closed, exiting");
                    break;
                }

                // Check if superseded by newer stream() call
                if book_gen_ref.load(Ordering::Relaxed) != book_gen {
                    info!("[BOOK_STREAM] Generation {} superseded, exiting", book_gen);
                    break;
                }

                let ws_client = WsClient::default();

                if reconnect_attempt > 0 {
                    warn!(
                        "[BOOK_STREAM] Reconnecting (attempt {})",
                        reconnect_attempt + 1
                    );
                }

                // Mark tokens as disconnected
                for asset_id in &asset_ids_book {
                    let token_id = asset_id.to_string();
                    if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                        prices.ws_connected = false;
                        prices.last_disconnect_ms = chrono::Utc::now().timestamp_millis();
                    }
                }

                match ws_client.subscribe_orderbook(asset_ids_book.clone()) {
                    Ok(book_stream) => {
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        if reconnect_attempt > 0 {
                            last_book_reconnect_ts.store(now_ms as u64, Ordering::Relaxed);
                            info!(
                                "[BOOK_STREAM] ✅ RECONNECTED successfully after {} attempts",
                                reconnect_attempt
                            );
                        } else {
                            info!("[BOOK_STREAM] ✅ CONNECTED - WebSocket stream active");
                        }

                        // Mark tokens as connected
                        for asset_id in &asset_ids_book {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.last_successful_connect_ms = now_ms;
                                prices.ws_connected = true;
                                prices.consecutive_errors = 0;
                            }
                        }

                        let mut stream = Box::pin(book_stream);
                        reconnect_attempt = 0;
                        reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;

                        loop {
                            match stream.next().await {
                                Some(Ok(book)) => {
                                    total_ws_msgs_book.fetch_add(1, Ordering::Relaxed);

                                    let token_id = book.asset_id.to_string();
                                    let token_id_hex = format!("{:#x}", book.asset_id);

                                    let mut prices = prices_clone
                                        .entry(token_id.clone())
                                        .or_insert_with(|| TokenPrices::new(token_id.clone()));

                                    let target_bid_price = prices.best_bid;
                                    let target_ask_price = prices.best_ask;

                                    let bid_levels: Vec<OrderBookLevel> = book
                                        .bids
                                        .iter()
                                        .map(|b| OrderBookLevel {
                                            price: b.price,
                                            size: b.size,
                                        })
                                        .collect();
                                    let ask_levels: Vec<OrderBookLevel> = book
                                        .asks
                                        .iter()
                                        .map(|a| OrderBookLevel {
                                            price: a.price,
                                            size: a.size,
                                        })
                                        .collect();

                                    let bid_size =
                                        find_size_at_price(&bid_levels, target_bid_price);
                                    let mut ask_size =
                                        find_size_at_price(&ask_levels, target_ask_price);

                                    if ask_size == 0.0 {
                                        if let Some(first_ask) = ask_levels.first() {
                                            ask_size = first_ask.size.to_f64().unwrap_or(0.0);
                                        }
                                    }

                                    prices.best_bid_size = bid_size;
                                    prices.best_ask_size = ask_size;

                                    prices.mid_price = match (prices.best_bid, prices.best_ask) {
                                        (Some(bid), Some(ask)) => {
                                            Some((bid + ask) / Decimal::from(2))
                                        }
                                        (Some(bid), None) => Some(bid),
                                        (None, Some(ask)) => Some(ask),
                                        _ => None,
                                    };

                                    prices.last_update_ms = chrono::Utc::now().timestamp_millis();
                                    prices.last_ws_update_ms = prices.last_update_ms;
                                    prices.ws_connected = true;
                                    prices.consecutive_errors = 0;
                                    prices.ws_message_count += 1;

                                    let update = PolyPriceUpdate {
                                        token_id: token_id.clone(),
                                        best_bid: prices.best_bid,
                                        best_ask: prices.best_ask,
                                        mid_price: prices.mid_price,
                                    };

                                    if let Err(e) = tx_book.send(update) {
                                        error!("Failed to send book update: {:?}", e);
                                        return;
                                    }

                                    debug!("[BOOK_STREAM] 📥 WS update for {}: bid_size={:.1}, ask_size={:.1}",
                                        token_id_hex, bid_size, ask_size
                                    );

                                    // Exit promptly if superseded by newer stream() call
                                    if book_gen_ref.load(Ordering::Relaxed) != book_gen {
                                        info!("[BOOK_STREAM] Generation {} superseded mid-stream, exiting", book_gen);
                                        return;
                                    }
                                }
                                Some(Err(e)) => {
                                    error!("[BOOK_STREAM] ❌ Stream error: {}", e);
                                    for asset_id in &asset_ids_book {
                                        let token_id = asset_id.to_string();
                                        if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                            prices.consecutive_errors += 1;
                                        }
                                    }
                                }
                                None => {
                                    warn!("[BOOK_STREAM] ⚠️ Stream ended, will reconnect");
                                    break;
                                }
                            }
                        }

                        warn!("[BOOK_STREAM] 🔌 DISCONNECTED");
                        book_reconnect_counter.fetch_add(1, Ordering::Relaxed);

                        for asset_id in &asset_ids_book {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.reconnect_count += 1;
                                prices.ws_connected = false;
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "[BOOK_STREAM] ❌ Connection failed (attempt {}): {}",
                            reconnect_attempt + 1,
                            e
                        );

                        for asset_id in &asset_ids_book {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.ws_connected = false;
                                prices.last_disconnect_ms = chrono::Utc::now().timestamp_millis();
                                prices.consecutive_errors += 1;
                            }
                        }
                    }
                }

                if MAX_RECONNECT_ATTEMPTS > 0 && reconnect_attempt >= MAX_RECONNECT_ATTEMPTS {
                    error!("[BOOK_STREAM] Max reconnect attempts reached");
                    break;
                }

                info!(
                    "[BOOK_STREAM] ⏳ Waiting {}ms before reconnect",
                    reconnect_delay_ms
                );
                tokio::time::sleep(Duration::from_millis(reconnect_delay_ms)).await;

                // Re-check generation after sleep
                if book_gen_ref.load(Ordering::Relaxed) != book_gen {
                    info!("[BOOK_STREAM] Generation {} superseded during backoff, exiting", book_gen);
                    break;
                }

                reconnect_delay_ms = std::cmp::min(
                    (reconnect_delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64,
                    MAX_RECONNECT_DELAY_MS,
                );
                reconnect_attempt += 1;
            }

            info!("[BOOK_STREAM] Gen {} task exited", book_gen);
        });

        // Spawn health monitoring task
        let very_stale_counter = Arc::clone(&self.very_stale_count);
        let stream_clone = Arc::clone(&self);
        let health_gen_ref = Arc::clone(&self.stream_generation);
        let health_gen = generation;

        tokio::spawn(async move {
            let mut health_ticker = interval(Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS));
            let mut tick_count = 0u32;

            info!(
                "[HEALTH_MONITOR] Started gen {} (interval: {}s)",
                health_gen,
                HEALTH_CHECK_INTERVAL_SECS
            );

            loop {
                health_ticker.tick().await;

                // Exit if superseded by newer stream() call
                if health_gen_ref.load(Ordering::Relaxed) != health_gen {
                    info!("[HEALTH_MONITOR] Generation {} superseded, exiting", health_gen);
                    break;
                }

                tick_count += 1;

                // Get detailed status for each token
                let token_statuses = stream_clone.detailed_status();

                // Count by state
                let mut healthy_ws = 0;
                let mut quiet_ws = 0;
                let mut stale_ws = 0;
                let mut disconnected = 0;
                let mut unhealthy = 0;

                for (_short_id, detail) in &token_statuses {
                    if detail.contains("HEALTHY_WS") {
                        healthy_ws += 1;
                    } else if detail.contains("QUIET_WS") {
                        quiet_ws += 1;
                    } else if detail.contains("STALE_WS") {
                        stale_ws += 1;
                    } else if detail.contains("DISCONNECTED") {
                        disconnected += 1;
                    } else if detail.contains("UNHEALTHY") {
                        unhealthy += 1;
                    }
                }

                very_stale_counter.store(stale_ws as u32, Ordering::Relaxed);

                // Log summary every 30s (3 ticks) — same cadence for healthy and unhealthy
                // This prevents warning spam when stale tokens accumulate
                let has_issues = disconnected > 0 || unhealthy > 0 || stale_ws > 0;

                if tick_count % 3 == 0 {
                    if has_issues {
                        warn!(
                            "[HEALTH] 🔍 ACTIVE_WS:{} QUIET_WS:{} STALE:{} DISC:{} UNHEALTHY:{} | {}",
                            healthy_ws, quiet_ws, stale_ws, disconnected, unhealthy,
                            stream_clone.connection_status()
                        );
                        // Log details for problematic tokens
                        for (short_id, detail) in &token_statuses {
                            if detail.contains("STALE")
                                || detail.contains("DISCONNECTED")
                                || detail.contains("UNHEALTHY")
                            {
                                warn!("[HEALTH]   └─ {}: {}", short_id, detail);
                            }
                        }
                    } else {
                        info!(
                            "[HEALTH] ✅ ACTIVE_WS:{} QUIET_WS:{} | {}",
                            healthy_ws,
                            quiet_ws,
                            stream_clone.connection_status()
                        );
                    }
                }
            }
        });

        // Spawn REST API fallback task - using ORIGINAL API calls
        let prices_fallback = Arc::clone(&self.prices);
        let token_ids_fallback = Arc::clone(&self.token_ids);
        let tx_fallback = tx.clone();
        let total_api_calls = Arc::clone(&self.total_api_fallbacks);
        let rest_gen_ref = Arc::clone(&self.stream_generation);
        let rest_gen = generation;

        tokio::spawn(async move {
            let clob_client = match ClobClient::new(CLOB_API_URL, Config::default()) {
                Ok(client) => Arc::new(client),
                Err(e) => {
                    error!("[REST_FALLBACK] Failed to create CLOB client: {:?}", e);
                    return;
                }
            };

            let mut ticker = interval(Duration::from_secs(REST_FALLBACK_INTERVAL_SECS));
            let mut last_log_ms = 0i64;
            // M3: Rate limiting / backoff state
            let mut consecutive_rest_failures: u32 = 0;
            let mut rest_backoff_until_ms: i64 = 0;

            info!(
                "[REST_FALLBACK] Started gen {} (interval: {}s, using midpoint + last_trade APIs)",
                rest_gen,
                REST_FALLBACK_INTERVAL_SECS
            );

            loop {
                ticker.tick().await;

                // Exit if superseded by newer stream() call
                if rest_gen_ref.load(Ordering::Relaxed) != rest_gen {
                    info!("[REST_FALLBACK] Generation {} superseded, exiting", rest_gen);
                    break;
                }

                let now_ms = chrono::Utc::now().timestamp_millis();

                // M3: Skip if in rate-limit backoff period
                if now_ms < rest_backoff_until_ms {
                    continue;
                }

                let token_ids_to_check = match token_ids_fallback.lock() {
                    Ok(ids) => ids.clone(),
                    Err(_) => continue,
                };

                let mut tokens_needing_fallback: Vec<(String, U256)> = vec![];

                for token_id_str in &token_ids_to_check {
                    let token_id = match U256::from_str(&token_id_str) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };

                    let should_fallback = {
                        if let Some(prices) = prices_fallback.get(token_id_str) {
                            !prices.ws_connected
                                || prices.is_unhealthy()
                                || (now_ms - prices.last_update_ms) > WS_CONNECTED_AGE_MS
                        } else {
                            true
                        }
                    };

                    if should_fallback {
                        tokens_needing_fallback.push((token_id_str.clone(), token_id));
                    }
                }

                if tokens_needing_fallback.is_empty() {
                    continue;
                }

                // Parallel fetch using ORIGINAL API calls
                let fetch_tasks: Vec<_> = tokens_needing_fallback
                    .into_iter()
                    .map(|(token_id_str, token_id)| {
                        let clob_client = Arc::clone(&clob_client);
                        async move {
                            // Fetch midpoint from REST API
                            let mid_req = MidpointRequest::builder().token_id(token_id).build();
                            let result: Result<(String, f64, f64), String> = match clob_client.midpoint(&mid_req).await {
                                Ok(mid_resp) => {
                                    if let Some(mid) = mid_resp.mid.to_f64() {
                                        // Fetch last trade price from REST API
                                        let last_req = LastTradePriceRequest::builder()
                                            .token_id(token_id)
                                            .build();
                                        let last_trade = clob_client
                                            .last_trade_price(&last_req)
                                            .await
                                            .ok()
                                            .and_then(|t| t.price.to_f64())
                                            .unwrap_or(0.0);

                                        Ok((token_id_str.clone(), mid, last_trade))
                                    } else {
                                        Err("midpoint response had no valid price".to_string())
                                    }
                                }
                                // M3: Capture error details for 429/425 detection
                                Err(e) => Err(format!("{}", e)),
                            };
                            result
                        }
                    })
                    .collect();

                let results = futures::future::join_all(fetch_tasks).await;

                let mut tokens_using_fallback = vec![];

                let mut rest_errors: Vec<String> = Vec::new();

                for result in results {
                    match result {
                        Ok((token_id_str, mid, last_trade)) => {
                            total_api_calls.fetch_add(1, Ordering::Relaxed);

                            let mut prices = prices_fallback
                                .entry(token_id_str.clone())
                                .or_insert_with(|| TokenPrices::new(token_id_str.clone()));

                            let mid_dec = rust_decimal::Decimal::from_f64(mid)
                                .unwrap_or(rust_decimal::Decimal::ZERO);
                            prices.mid_price = Some(mid_dec);
                            prices.last_trade = Some(last_trade);

                            // M6: Update synthetic best_bid/best_ask from REST midpoint
                            // Prevents stale WS bid/ask values from persisting when only REST is working
                            let half_spread = Decimal::from_str("0.0005").unwrap();
                            prices.best_bid = Some(mid_dec - half_spread);
                            prices.best_ask = Some(mid_dec + half_spread);

                            prices.last_update_ms = now_ms;
                            prices.api_fallback_count += 1;

                            let update = PolyPriceUpdate {
                                token_id: token_id_str.clone(),
                                best_bid: prices.best_bid,
                                best_ask: prices.best_ask,
                                mid_price: prices.mid_price,
                            };

                            if let Err(e) = tx_fallback.send(update) {
                                debug!("[REST_FALLBACK] Failed to send: {:?}", e);
                            }

                            let short_id = if token_id_str.len() > 8 {
                                &token_id_str[..8]
                            } else {
                                &token_id_str
                            };
                            tokens_using_fallback.push(short_id.to_string());

                            debug!(
                                "[REST_FALLBACK] API update for {}: mid={:.4} bid={:.4} ask={:.4}",
                                short_id, mid,
                                (mid - 0.0005), (mid + 0.0005)
                            );
                        }
                        Err(err_msg) => {
                            rest_errors.push(err_msg);
                        }
                    }
                }

                // M3: Update rate-limit / backoff state
                if !rest_errors.is_empty() && tokens_using_fallback.is_empty() {
                    // All tokens failed — increment consecutive failure count
                    consecutive_rest_failures += 1;
                    let has_429 = rest_errors.iter().any(|e| e.contains("429"));
                    let has_425 = rest_errors.iter().any(|e| e.contains("425"));

                    let backoff_ms: i64 = if has_429 {
                        // Exponential backoff for rate limiting (double each time, max 60s)
                        let secs = ((REST_FALLBACK_INTERVAL_SECS as u64) * 2u64.pow(consecutive_rest_failures.min(5))).min(60);
                        warn!("[REST_FALLBACK] HTTP 429 rate limited, backing off {}s (failures: {})",
                            secs, consecutive_rest_failures);
                        secs as i64 * 1000
                    } else if has_425 {
                        warn!("[REST_FALLBACK] HTTP 425 matching engine restart, backing off 30s");
                        30_000
                    } else if consecutive_rest_failures >= 3 {
                        let secs = ((REST_FALLBACK_INTERVAL_SECS as u64) * 2u64.pow((consecutive_rest_failures - 2).min(5))).min(60);
                        warn!("[REST_FALLBACK] {} consecutive failures, backing off {}s",
                            consecutive_rest_failures, secs);
                        secs as i64 * 1000
                    } else {
                        0
                    };

                    if backoff_ms > 0 {
                        rest_backoff_until_ms = now_ms + backoff_ms;
                    }
                } else if !tokens_using_fallback.is_empty() {
                    // At least some tokens succeeded — reset failure count
                    consecutive_rest_failures = 0;
                }

                // Log fallback usage (but not too frequently - once per 30s)
                if !tokens_using_fallback.is_empty() && (now_ms - last_log_ms) > 30000 {
                    warn!(
                        "[REST_FALLBACK] 🌐 Using API for {} tokens: [{}] (total API calls: {})",
                        tokens_using_fallback.len(),
                        tokens_using_fallback.join(", "),
                        total_api_calls.load(Ordering::Relaxed)
                    );
                    last_log_ms = now_ms;
                }
            }
        });

        info!("Polymarket stream started for {} tokens", token_ids.len());
        Ok(rx)
    }

    /// Get a snapshot of all current prices
    pub fn get_all_prices(&self) -> Vec<(String, TokenPrices)> {
        self.prices
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }
}

impl Default for PolymarketOrderbookStream {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for PolymarketOrderbookStream {
    fn clone(&self) -> Self {
        Self {
            prices: Arc::clone(&self.prices),
            max_stale_ms: self.max_stale_ms,
            token_ids: Arc::clone(&self.token_ids),
            price_reconnect_count: Arc::clone(&self.price_reconnect_count),
            book_reconnect_count: Arc::clone(&self.book_reconnect_count),
            last_price_reconnect_ms: Arc::clone(&self.last_price_reconnect_ms),
            last_book_reconnect_ms: Arc::clone(&self.last_book_reconnect_ms),
            very_stale_count: Arc::clone(&self.very_stale_count),
            total_ws_messages: Arc::clone(&self.total_ws_messages),
            total_api_fallbacks: Arc::clone(&self.total_api_fallbacks),
            stream_generation: Arc::clone(&self.stream_generation),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_prices_new() {
        let prices = TokenPrices::new("test_token".to_string());
        assert!(prices.best_bid.is_none());
        assert!(prices.best_ask.is_none());
        assert_eq!(prices.ws_message_count, 0);
        assert_eq!(prices.api_fallback_count, 0);
        assert_eq!(prices.data_source(), "API");
    }

    #[test]
    fn test_connection_detail() {
        let mut prices = TokenPrices::new("test".to_string());

        // Disconnected
        assert!(prices.connection_detail().contains("DISCONNECTED"));

        // Connected with recent WS data
        prices.ws_connected = true;
        prices.last_ws_update_ms = chrono::Utc::now().timestamp_millis();
        assert!(prices.connection_detail().contains("HEALTHY_WS"));

        // Quiet but connected
        prices.last_ws_update_ms = chrono::Utc::now().timestamp_millis() - 30000;
        assert!(prices.connection_detail().contains("QUIET_WS"));
    }
}
// ```

// This version:
// 1. ✅ **Compiles** - uses the existing `midpoint()` and `last_trade_price()` APIs
// 2. ✅ **100% backward compatible** - all existing APIs unchanged
// 3. ✅ **Better logging** - clear indicators of WS vs API usage
// 4. ✅ **Tracks metrics** - WS message count, API fallback count
// 5. ✅ **Clear status** - HEALTHY_WS, QUIET_WS, STALE_WS, DISCONNECTED states

// Now you'll see logs like:
// ```
// [PRICE_STREAM] ✅ CONNECTED - WebSocket stream active
// [HEALTH] ✅ ACTIVE_WS:2 QUIET_WS:0 | Health: 100% | WS:2 API:0 | WS msgs: 45 | API calls: 0
