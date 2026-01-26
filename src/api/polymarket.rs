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
use polymarket_client_sdk::clob::ws::Client as WsClient;
use polymarket_client_sdk::clob::{Client as ClobClient, Config};
use polymarket_client_sdk::clob::types::request::{LastTradePriceRequest, MidpointRequest};
use polymarket_client_sdk::types::U256;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
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
/// Silent disconnect threshold: if no data received for this long while "connected", force reconnect (milliseconds)
const SILENT_DISCONNECT_THRESHOLD_MS: i64 = 10000;
/// Health check interval: how often to check for silent disconnects (seconds)
const HEALTH_CHECK_INTERVAL_SECS: u64 = 5;

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
    pub best_bid_size: f64,        // Size at best bid price (from orderbook feed)
    pub best_ask_size: f64,        // Size at best ask price (from orderbook feed)
    pub mid_price: Option<Decimal>,
    pub last_trade: Option<f64>,   // Last trade price
    pub last_update_ms: i64,
    pub ws_connected: bool,        // WebSocket connection status
    pub reconnect_count: u32,      // Number of reconnections for this token's stream
    pub last_disconnect_ms: i64,   // Timestamp of last disconnection
    pub last_successful_connect_ms: i64,  // Timestamp of last successful connection
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

    /// Check if the connection is silently disconnected (connected but no data for a while)
    pub fn is_silently_disconnected(&self) -> bool {
        if !self.ws_connected || self.last_update_ms == 0 {
            return false; // Not silently disconnected if not connected or never got data
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

    /// Get data source indicator: "WS" for WebSocket, "API" for REST fallback
    pub fn data_source(&self) -> &str {
        if self.ws_connected && self.age_ms() < WS_CONNECTED_AGE_MS {
            "WS"
        } else {
            "API"
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
                if let (Some(price), Some(order_size)) = (order.price.to_f64(), order.size.to_f64()) {
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
    /// Flag to force price stream reconnection (used by health checker)
    force_price_reconnect: Arc<AtomicU32>,
    /// Flag to force orderbook stream reconnection (used by health checker)
    force_book_reconnect: Arc<AtomicU32>,
    /// Silent disconnect detection count
    silent_disconnect_count: Arc<AtomicU32>,
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
            force_price_reconnect: Arc::new(AtomicU32::new(0)),
            force_book_reconnect: Arc::new(AtomicU32::new(0)),
            silent_disconnect_count: Arc::new(AtomicU32::new(0)),
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
            Some(prices) => prices.ws_connected && prices.age_ms() < self.max_stale_ms,
            None => false,
        }
    }

    /// Get overall connection health (percent of tokens that are connected)
    ///
    /// Returns a health score from 0-100% based on:
    /// - Recent updates (within staleness threshold, not 30s window)
    /// - WebSocket connection status
    /// - Non-stale data
    ///
    /// This distinguishes between "disconnected" and "connected but quiet":
    /// - If ws_connected=false, health is reduced
    /// - If ws_connected=true but data is old, health is still good (quiet market)
    /// - Only if data is older than staleness threshold AND disconnected is health poor
    pub fn connection_health(&self) -> f64 {
        if self.prices.is_empty() {
            return 0.0;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let healthy_count = self.prices.iter()
            .filter(|p| {
                let prices = p.value();
                // Consider healthy if:
                // 1. WebSocket is connected AND data is within staleness threshold
                // OR
                // 2. Data is very recent (within 10 seconds) - even if ws_connected is temporarily false
                let data_age = now_ms.saturating_sub(prices.last_update_ms);
                (prices.ws_connected && data_age < self.max_stale_ms)
                    || (!prices.ws_connected && data_age < 10000)
            })
            .count();
        (healthy_count as f64 / self.prices.len() as f64) * 100.0
    }

    /// Get the total number of price stream reconnections
    pub fn price_reconnect_count(&self) -> u32 {
        self.price_reconnect_count.load(Ordering::Relaxed)
    }

    /// Get the total number of orderbook stream reconnections
    pub fn book_reconnect_count(&self) -> u32 {
        self.book_reconnect_count.load(Ordering::Relaxed)
    }

    /// Get the total number of silent disconnects detected
    pub fn silent_disconnect_count(&self) -> u32 {
        self.silent_disconnect_count.load(Ordering::Relaxed)
    }

    /// Get the timestamp of the last price stream reconnection (milliseconds since epoch)
    pub fn last_price_reconnect_ms(&self) -> u64 {
        self.last_price_reconnect_ms.load(Ordering::Relaxed)
    }

    /// Get the timestamp of the last orderbook stream reconnection (milliseconds since epoch)
    pub fn last_book_reconnect_ms(&self) -> u64 {
        self.last_book_reconnect_ms.load(Ordering::Relaxed)
    }

    /// Get connection status summary for logging
    pub fn connection_status(&self) -> String {
        let health = self.connection_health();
        let price_recos = self.price_reconnect_count();
        let book_recos = self.book_reconnect_count();
        let silent_discos = self.silent_disconnect_count();
        let now_ms = chrono::Utc::now().timestamp_millis();

        let last_price_reco = self.last_price_reconnect_ms();
        let last_book_reco = self.last_book_reconnect_ms();

        let price_reco_age = if last_price_reco > 0 {
            format!("{}s ago", (now_ms.saturating_sub(last_price_reco as i64) / 1000))
        } else {
            "never".to_string()
        };

        let book_reco_age = if last_book_reco > 0 {
            format!("{}s ago", (now_ms.saturating_sub(last_book_reco as i64) / 1000))
        } else {
            "never".to_string()
        };

        format!(
            "Health: {:.1}% | Price reco: {} ({}) | Book reco: {} ({}) | Silent discos: {} | Tokens: {}",
            health,
            price_recos,
            price_reco_age,
            book_recos,
            book_reco_age,
            silent_discos,
            self.prices.len()
        )
    }

    /// Start streaming orderbook updates for the given token IDs
    ///
    /// Returns a channel that receives price updates.
    /// Also spawns a REST API fallback task that polls when WebSocket data is stale.
    pub async fn stream(
        self: Arc<Self>,
        token_ids: Vec<String>,
    ) -> Result<mpsc::UnboundedReceiver<PolyPriceUpdate>> {
        let (tx, rx) = mpsc::unbounded_channel();

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
        let force_price_reco = Arc::clone(&self.force_price_reconnect);

        tokio::spawn(async move {
            let mut reconnect_attempt = 0u32;
            let mut reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;

            info!("[PRICE_STREAM] Task starting for {} tokens", asset_ids_price.len());
            info!("[PRICE_STREAM] Attempting initial connection...");

            // Outer reconnection loop - runs forever until channel closes
            loop {
                // Check if sender is still alive
                if tx_clone.is_closed() {
                    info!("[PRICE_STREAM] Sender channel closed, exiting reconnection loop");
                    break;
                }

                // Create new WebSocket client for each connection attempt
                let ws_client = WsClient::default();

                // Log state transition: CONNECTING
                if reconnect_attempt > 0 {
                    info!("[PRICE_STREAM] State transition: DISCONNECTED -> CONNECTING (attempt {})",
                          reconnect_attempt + 1);
                }

                // Mark existing prices as potentially stale before reconnect
                for asset_id in &asset_ids_price {
                    let token_id = asset_id.to_string();
                    if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                        prices.ws_connected = false;
                        prices.last_disconnect_ms = chrono::Utc::now().timestamp_millis();
                    }
                }

                match ws_client.subscribe_prices(asset_ids_price.clone()) {
                    Ok(price_stream) => {
                        // Successfully connected
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        if reconnect_attempt > 0 {
                            last_price_reconnect_ts.store(now_ms as u64, Ordering::Relaxed);
                            info!("[PRICE_STREAM] State transition: CONNECTING -> CONNECTED (reconnection successful)");
                            info!(
                                "[PRICE_STREAM] Reconnected after {} attempts (delay: {}ms)",
                                reconnect_attempt, reconnect_delay_ms
                            );
                        } else {
                            info!("[PRICE_STREAM] State transition: CONNECTING -> CONNECTED (initial connection)");
                        }

                        // Mark all tokens as having just connected
                        for asset_id in &asset_ids_price {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.last_successful_connect_ms = now_ms;
                                prices.ws_connected = true;
                            }
                        }

                        let mut stream = Box::pin(price_stream);
                        reconnect_attempt = 0;
                        reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;

                        // Process stream until it ends or force reconnect is triggered
                        loop {
                            // Use tokio::select! to check both stream messages and force reconnect flag
                            // Check flag every 500ms for responsive silent disconnect handling
                            let mut force_check_interval = tokio::time::interval(Duration::from_millis(500));

                            tokio::select! {
                                // Check force reconnect flag periodically
                                _ = force_check_interval.tick() => {
                                    if force_price_reco.load(Ordering::Relaxed) > 0 {
                                        // Clear the flag and force reconnection
                                        force_price_reco.store(0, Ordering::Relaxed);
                                        warn!("[PRICE_STREAM] Force reconnect flag detected (silent disconnect), breaking message loop to reconnect");
                                        break;
                                    }
                                }
                                // Process stream messages
                                result = stream.next() => {
                                    match result {
                                        Some(Ok(price_change)) => {
                                            // Process each price change in the batch
                                            for change in &price_change.price_changes {
                                                let change_token_id = change.asset_id.to_string();
                                                let change_token_id_hex = format!("{:#x}", change.asset_id);

                                                // Update prices from PriceChange
                                                let mut prices = prices_clone
                                                    .entry(change_token_id.clone())
                                                    .or_insert_with(|| TokenPrices::new(change_token_id.clone()));

                                                prices.best_bid = change.best_bid;
                                                prices.best_ask = change.best_ask;

                                                // Track last trade price from the price field
                                                prices.last_trade = change.price.to_f64();

                                                // Calculate mid price
                                                prices.mid_price = match (prices.best_bid, prices.best_ask) {
                                                    (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::from(2)),
                                                    (Some(bid), None) => Some(bid),
                                                    (None, Some(ask)) => Some(ask),
                                                    _ => None,
                                                };

                                                prices.last_update_ms = price_change.timestamp;
                                                prices.ws_connected = true;

                                                // Send update
                                                let update = PolyPriceUpdate {
                                                    token_id: change_token_id.clone(),
                                                    best_bid: prices.best_bid,
                                                    best_ask: prices.best_ask,
                                                    mid_price: prices.mid_price,
                                                };

                                                if let Err(e) = tx_clone.send(update) {
                                                    error!("Failed to send price update: {:?}", e);
                                                    // Channel closed - exit reconnection loop
                                                    return;
                                                }

                                                debug!("PriceChange for token: {}", change_token_id_hex);
                                            }
                                        }
                                        Some(Err(e)) => {
                                            let error_str = e.to_string().to_lowercase();
                                            let error_type = if error_str.contains("connection reset")
                                                || error_str.contains("connection refused")
                                                || error_str.contains("timeout")
                                                || error_str.contains("network") {
                                                "NETWORK"
                                            } else if error_str.contains("websocket")
                                                || error_str.contains("handshake")
                                                || error_str.contains("protocol") {
                                                "PROTOCOL"
                                            } else if error_str.contains("closed")
                                                || error_str.contains("disconnect")
                                                || error_str.contains("eof") {
                                                "SERVER_DISCONNECT"
                                            } else {
                                                "UNKNOWN"
                                            };
                                            error!("[PRICE_STREAM] Stream error [{}]: {}", error_type, e);
                                            // Continue processing, will reconnect if stream ends
                                        }
                                        None => {
                                            // Stream ended naturally (None returned)
                                            warn!("[PRICE_STREAM] Stream ended naturally (None returned), breaking message loop to reconnect");
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        // Stream ended or force reconnect triggered - log state transition and reconnect
                        warn!("[PRICE_STREAM] State transition: CONNECTED -> DISCONNECTED (stream terminated or force reconnect)");
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
                        let error_str = e.to_string().to_lowercase();
                        let error_type = if error_str.contains("connection reset")
                            || error_str.contains("connection refused")
                            || error_str.contains("timeout")
                            || error_str.contains("network") {
                            "NETWORK"
                        } else if error_str.contains("websocket")
                            || error_str.contains("handshake")
                            || error_str.contains("protocol") {
                            "PROTOCOL"
                        } else if error_str.contains("closed")
                            || error_str.contains("disconnect")
                            || error_str.contains("eof") {
                            "SERVER_DISCONNECT"
                        } else {
                            "UNKNOWN"
                        };
                        error!(
                            "[PRICE_STREAM] Connection failed [{}] (attempt {}): {}",
                            error_type,
                            reconnect_attempt + 1,
                            e
                        );

                        // Mark all tokens as disconnected
                        for asset_id in &asset_ids_price {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.ws_connected = false;
                                prices.last_disconnect_ms = chrono::Utc::now().timestamp_millis();
                            }
                        }
                    }
                }

                // Check max reconnect attempts
                if MAX_RECONNECT_ATTEMPTS > 0 && reconnect_attempt >= MAX_RECONNECT_ATTEMPTS {
                    error!("[PRICE_STREAM] Max reconnect attempts ({}) reached, giving up", MAX_RECONNECT_ATTEMPTS);
                    break;
                }

                // Exponential backoff before reconnect
                let next_attempt = reconnect_attempt + 1;
                if reconnect_delay_ms >= MAX_RECONNECT_DELAY_MS {
                    warn!("[PRICE_STREAM] Max backoff reached ({}ms), will retry periodically", MAX_RECONNECT_DELAY_MS);
                } else {
                    info!(
                        "[PRICE_STREAM] Waiting {}ms before reconnect attempt {} (backoff: {:.1}s)",
                        reconnect_delay_ms,
                        next_attempt,
                        reconnect_delay_ms as f64 / 1000.0
                    );
                }
                tokio::time::sleep(Duration::from_millis(reconnect_delay_ms)).await;

                // Calculate next delay with exponential backoff
                reconnect_delay_ms = std::cmp::min(
                    (reconnect_delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64,
                    MAX_RECONNECT_DELAY_MS
                );
                reconnect_attempt += 1;
            }

            error!("[PRICE_STREAM] Task exited");
        });

        // Spawn task to handle orderbook updates with reconnection logic
        let prices_clone = Arc::clone(&self.prices);
        let asset_ids_book = asset_ids.clone();
        let tx_book = tx.clone();
        let book_reconnect_counter = Arc::clone(&self.book_reconnect_count);
        let last_book_reconnect_ts = Arc::clone(&self.last_book_reconnect_ms);
        let force_book_reco = Arc::clone(&self.force_book_reconnect);

        tokio::spawn(async move {
            let mut reconnect_attempt = 0u32;
            let mut reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;

            info!("[BOOK_STREAM] Task starting for {} tokens", asset_ids_book.len());
            info!("[BOOK_STREAM] Attempting initial connection...");

            // Outer reconnection loop - runs forever until channel closes
            loop {
                // Check if sender is still alive
                if tx_book.is_closed() {
                    info!("[BOOK_STREAM] Sender channel closed, exiting reconnection loop");
                    break;
                }

                // Create new WebSocket client for each connection attempt
                let ws_client = WsClient::default();

                // Log state transition: CONNECTING
                if reconnect_attempt > 0 {
                    info!("[BOOK_STREAM] State transition: DISCONNECTED -> CONNECTING (attempt {})",
                          reconnect_attempt + 1);
                }

                // Mark existing prices as potentially stale before reconnect
                for asset_id in &asset_ids_book {
                    let token_id = asset_id.to_string();
                    if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                        prices.ws_connected = false;
                        prices.last_disconnect_ms = chrono::Utc::now().timestamp_millis();
                    }
                }

                match ws_client.subscribe_orderbook(asset_ids_book.clone()) {
                    Ok(book_stream) => {
                        // Successfully connected
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        if reconnect_attempt > 0 {
                            last_book_reconnect_ts.store(now_ms as u64, Ordering::Relaxed);
                            info!("[BOOK_STREAM] State transition: CONNECTING -> CONNECTED (reconnection successful)");
                            info!(
                                "[BOOK_STREAM] Reconnected after {} attempts (delay: {}ms)",
                                reconnect_attempt, reconnect_delay_ms
                            );
                        } else {
                            info!("[BOOK_STREAM] State transition: CONNECTING -> CONNECTED (initial connection)");
                        }

                        // Mark all tokens as having just connected
                        for asset_id in &asset_ids_book {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.last_successful_connect_ms = now_ms;
                                prices.ws_connected = true;
                            }
                        }

                        let mut stream = Box::pin(book_stream);
                        reconnect_attempt = 0;
                        reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;

                        // Process stream until it ends or force reconnect is triggered
                        loop {
                            // Use tokio::select! to check both stream messages and force reconnect flag
                            // Check flag every 500ms for responsive silent disconnect handling
                            let mut force_check_interval = tokio::time::interval(Duration::from_millis(500));

                            tokio::select! {
                                // Check force reconnect flag periodically
                                _ = force_check_interval.tick() => {
                                    if force_book_reco.load(Ordering::Relaxed) > 0 {
                                        // Clear the flag and force reconnection
                                        force_book_reco.store(0, Ordering::Relaxed);
                                        warn!("[BOOK_STREAM] Force reconnect flag detected (silent disconnect), breaking message loop to reconnect");
                                        break;
                                    }
                                }
                                // Process stream messages
                                result = stream.next() => {
                                    match result {
                                        Some(Ok(book)) => {
                                            let token_id = book.asset_id.to_string();
                                            let token_id_hex = format!("{:#x}", book.asset_id);

                                            // Update prices from BookUpdate
                                            let mut prices = prices_clone
                                                .entry(token_id.clone())
                                                .or_insert_with(|| TokenPrices::new(token_id.clone()));

                                            // Get current best bid/ask prices (set by price feed)
                                            let target_bid_price = prices.best_bid;
                                            let target_ask_price = prices.best_ask;

                                            // Convert orderbook to our OrderBookLevel format
                                            let bid_levels: Vec<OrderBookLevel> = book.bids.iter()
                                                .map(|b| OrderBookLevel { price: b.price, size: b.size })
                                                .collect();
                                            let ask_levels: Vec<OrderBookLevel> = book.asks.iter()
                                                .map(|a| OrderBookLevel { price: a.price, size: a.size })
                                                .collect();

                                            // Find sizes at the best prices using helper function
                                            let bid_size = find_size_at_price(&bid_levels, target_bid_price);
                                            let mut ask_size = find_size_at_price(&ask_levels, target_ask_price);

                                            // If no match found with target price, use first level size as fallback
                                            if ask_size == 0.0 {
                                                if let Some(first_ask) = ask_levels.first() {
                                                    ask_size = first_ask.size.to_f64().unwrap_or(0.0);
                                                }
                                            }

                                            // Update sizes
                                            prices.best_bid_size = bid_size;
                                            prices.best_ask_size = ask_size;

                                            // Calculate mid price
                                            prices.mid_price = match (prices.best_bid, prices.best_ask) {
                                                (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::from(2)),
                                                (Some(bid), None) => Some(bid),
                                                (None, Some(ask)) => Some(ask),
                                                _ => None,
                                            };

                                            prices.last_update_ms = chrono::Utc::now().timestamp_millis();
                                            prices.ws_connected = true;

                                            // Send update
                                            let update = PolyPriceUpdate {
                                                token_id: token_id.clone(),
                                                best_bid: prices.best_bid,
                                                best_ask: prices.best_ask,
                                                mid_price: prices.mid_price,
                                            };

                                            if let Err(e) = tx_book.send(update) {
                                                error!("Failed to send book update: {:?}", e);
                                                // Channel closed - exit reconnection loop
                                                return;
                                            }

                                            debug!(
                                                "BookUpdate for token: {} (bids: {}, asks: {}) bid_size={:.1}, ask_size={:.1}",
                                                token_id_hex,
                                                book.bids.len(),
                                                book.asks.len(),
                                                bid_size,
                                                ask_size
                                            );
                                        }
                                        Some(Err(e)) => {
                                            let error_str = e.to_string().to_lowercase();
                                            let error_type = if error_str.contains("connection reset")
                                                || error_str.contains("connection refused")
                                                || error_str.contains("timeout")
                                                || error_str.contains("network") {
                                                "NETWORK"
                                            } else if error_str.contains("websocket")
                                                || error_str.contains("handshake")
                                                || error_str.contains("protocol") {
                                                "PROTOCOL"
                                            } else if error_str.contains("closed")
                                                || error_str.contains("disconnect")
                                                || error_str.contains("eof") {
                                                "SERVER_DISCONNECT"
                                            } else {
                                                "UNKNOWN"
                                            };
                                            error!("[BOOK_STREAM] Stream error [{}]: {}", error_type, e);
                                            // Continue processing, will reconnect if stream ends
                                        }
                                        None => {
                                            // Stream ended naturally (None returned)
                                            warn!("[BOOK_STREAM] Stream ended naturally (None returned), breaking message loop to reconnect");
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        // Stream ended or force reconnect triggered - log state transition and reconnect
                        warn!("[BOOK_STREAM] State transition: CONNECTED -> DISCONNECTED (stream terminated or force reconnect)");
                        book_reconnect_counter.fetch_add(1, Ordering::Relaxed);

                        // Increment reconnect count for each tracked token
                        for asset_id in &asset_ids_book {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.reconnect_count += 1;
                                prices.ws_connected = false;
                            }
                        }
                    }
                    Err(e) => {
                        let error_str = e.to_string().to_lowercase();
                        let error_type = if error_str.contains("connection reset")
                            || error_str.contains("connection refused")
                            || error_str.contains("timeout")
                            || error_str.contains("network") {
                            "NETWORK"
                        } else if error_str.contains("websocket")
                            || error_str.contains("handshake")
                            || error_str.contains("protocol") {
                            "PROTOCOL"
                        } else if error_str.contains("closed")
                            || error_str.contains("disconnect")
                            || error_str.contains("eof") {
                            "SERVER_DISCONNECT"
                        } else {
                            "UNKNOWN"
                        };
                        error!(
                            "[BOOK_STREAM] Connection failed [{}] (attempt {}): {}",
                            error_type,
                            reconnect_attempt + 1,
                            e
                        );

                        // Mark all tokens as disconnected
                        for asset_id in &asset_ids_book {
                            let token_id = asset_id.to_string();
                            if let Some(mut prices) = prices_clone.get_mut(&token_id) {
                                prices.ws_connected = false;
                                prices.last_disconnect_ms = chrono::Utc::now().timestamp_millis();
                            }
                        }
                    }
                }

                // Check max reconnect attempts
                if MAX_RECONNECT_ATTEMPTS > 0 && reconnect_attempt >= MAX_RECONNECT_ATTEMPTS {
                    error!("[BOOK_STREAM] Max reconnect attempts ({}) reached, giving up", MAX_RECONNECT_ATTEMPTS);
                    break;
                }

                // Exponential backoff before reconnect
                let next_attempt = reconnect_attempt + 1;
                if reconnect_delay_ms >= MAX_RECONNECT_DELAY_MS {
                    warn!("[BOOK_STREAM] Max backoff reached ({}ms), will retry periodically", MAX_RECONNECT_DELAY_MS);
                } else {
                    info!(
                        "[BOOK_STREAM] Waiting {}ms before reconnect attempt {} (backoff: {:.1}s)",
                        reconnect_delay_ms,
                        next_attempt,
                        reconnect_delay_ms as f64 / 1000.0
                    );
                }
                tokio::time::sleep(Duration::from_millis(reconnect_delay_ms)).await;

                // Calculate next delay with exponential backoff
                reconnect_delay_ms = std::cmp::min(
                    (reconnect_delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64,
                    MAX_RECONNECT_DELAY_MS
                );
                reconnect_attempt += 1;
            }

            error!("[BOOK_STREAM] Task exited");
        });

        // Spawn health monitoring task to detect silent disconnects
        let prices_health = Arc::clone(&self.prices);
        let asset_ids_health = asset_ids.clone();
        let silent_disconnect_counter = Arc::clone(&self.silent_disconnect_count);
        let force_price_reco = Arc::clone(&self.force_price_reconnect);
        let force_book_reco = Arc::clone(&self.force_book_reconnect);
        let max_stale_ms_for_health = self.max_stale_ms;

        tokio::spawn(async move {
            let mut health_ticker = interval(Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS));
            let mut tick_count = 0u32;

            info!("[HEALTH_MONITOR] Task started (checks every {}s for silent disconnects)",
                HEALTH_CHECK_INTERVAL_SECS);

            loop {
                health_ticker.tick().await;
                tick_count += 1;
                let now_ms = chrono::Utc::now().timestamp_millis();

                // Calculate overall health
                let total_tokens = asset_ids_health.len();
                let mut healthy_tokens = 0;
                let mut stale_tokens = vec![];
                let mut disconnected_tokens = vec![];

                for asset_id in &asset_ids_health {
                    let token_id = asset_id.to_string();

                    if let Some(prices) = prices_health.get(&token_id) {
                        let age_sec = prices.age_ms() / 1000;
                        if prices.ws_connected && age_sec < (max_stale_ms_for_health / 1000) {
                            healthy_tokens += 1;
                        } else if !prices.ws_connected {
                            let short_id = if token_id.len() > 8 { &token_id[..8] } else { &token_id };
                            disconnected_tokens.push(format!("{}(age:{}s)", short_id, age_sec));
                        } else if age_sec >= (max_stale_ms_for_health / 1000) {
                            let short_id = if token_id.len() > 8 { &token_id[..8] } else { &token_id };
                            stale_tokens.push(format!("{}(age:{}s)", short_id, age_sec));
                        }

                        // Check if silently disconnected (connected but no data for too long)
                        if prices.is_silently_disconnected() {
                            let short_id = if token_id.len() > 8 { &token_id[..8] } else { &token_id };
                            warn!(
                                "[HEALTH_MONITOR] Silent disconnect detected: token={} connected={} age={}s (>{}s threshold)",
                                short_id,
                                prices.ws_connected,
                                age_sec,
                                SILENT_DISCONNECT_THRESHOLD_MS / 1000
                            );

                            // Mark as disconnected so REST fallback can take over
                            if let Some(mut p) = prices_health.get_mut(&token_id) {
                                p.ws_connected = false;
                                p.last_disconnect_ms = now_ms;
                            }

                            silent_disconnect_counter.fetch_add(1, Ordering::Relaxed);
                            // Increment force reconnect flags
                            force_price_reco.fetch_add(1, Ordering::Relaxed);
                            force_book_reco.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }

                let health_pct = if total_tokens > 0 {
                    (healthy_tokens as f64 / total_tokens as f64) * 100.0
                } else {
                    0.0
                };

                // Log health status every 30 seconds (every 6 ticks at 5s interval)
                // or when health drops below 50%
                if health_pct < 50.0 || tick_count % 6 == 0 {
                    if health_pct < 50.0 {
                        let stale_str = if stale_tokens.is_empty() {
                            "none".to_string()
                        } else {
                            stale_tokens.join(", ")
                        };
                        let disconnected_str = if disconnected_tokens.is_empty() {
                            "none".to_string()
                        } else {
                            disconnected_tokens.join(", ")
                        };
                        warn!("[HEALTH_CHECK] Health: {:.0}% ({}/{} healthy) stale: [{}] disconnected: [{}]",
                            health_pct, healthy_tokens, total_tokens,
                            stale_str,
                            disconnected_str
                        );
                    } else {
                        let max_age = if total_tokens > 0 {
                            prices_health.iter()
                                .map(|p| p.value().age_ms() / 1000)
                                .max()
                                .unwrap_or(0)
                        } else {
                            0
                        };
                        info!("[HEALTH_CHECK] Health: {:.0}% ({}/{} healthy) last_update: {}s ago",
                            health_pct, healthy_tokens, total_tokens, max_age
                        );
                    }
                }
            }
        });

        // Spawn REST API fallback task
        let prices_fallback = Arc::clone(&self.prices);
        let token_ids_fallback = Arc::clone(&self.token_ids);
        let tx_fallback = tx.clone();

        tokio::spawn(async move {
            // Create CLOB client for REST API calls
            let clob_client = match ClobClient::new(CLOB_API_URL, Config::default()) {
                Ok(client) => Arc::new(client),
                Err(e) => {
                    error!("[REST_FALLBACK] Failed to create CLOB client: {:?}", e);
                    return;
                }
            };

            let mut ticker = interval(Duration::from_secs(REST_FALLBACK_INTERVAL_SECS));
            let mut fallback_count = 0u32;

            info!("[REST_FALLBACK] Task started (polls every {}s when WebSocket is disconnected)",
                REST_FALLBACK_INTERVAL_SECS);

            loop {
                ticker.tick().await;
                let now_ms = chrono::Utc::now().timestamp_millis();

                // Get list of token IDs to check
                let token_ids_to_check = match token_ids_fallback.lock() {
                    Ok(ids) => ids.clone(),
                    Err(_) => continue,
                };

                // Identify which tokens need REST fallback
                let mut tokens_needing_fallback: Vec<(String, U256)> = vec![];

                for token_id_str in &token_ids_to_check {
                    // Parse token ID to U256 for API call
                    let token_id = match U256::from_str(&token_id_str) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };

                    // Check if data is stale and needs fallback
                    let should_fallback = {
                        if let Some(prices) = prices_fallback.get(token_id_str) {
                            !prices.ws_connected || (now_ms - prices.last_update_ms) > WS_CONNECTED_AGE_MS
                        } else {
                            true
                        }
                    };

                    if should_fallback {
                        tokens_needing_fallback.push((token_id_str.clone(), token_id));
                    }
                }

                // Parallel fetch: Spawn tasks for all tokens simultaneously
                let fetch_tasks: Vec<_> = tokens_needing_fallback
                    .into_iter()
                    .map(|(token_id_str, token_id)| {
                        let clob_client = Arc::clone(&clob_client);
                        async move {
                            // Fetch midpoint from REST API
                            let mid_req = MidpointRequest::builder().token_id(token_id).build();
                            let result = match clob_client.midpoint(&mid_req).await {
                                Ok(mid_resp) => {
                                    if let Some(mid) = mid_resp.mid.to_f64() {
                                        // Fetch last trade price from REST API
                                        let last_req = LastTradePriceRequest::builder().token_id(token_id).build();
                                        let last_trade = clob_client.last_trade_price(&last_req).await
                                            .ok()
                                            .and_then(|t| t.price.to_f64())
                                            .unwrap_or(0.0);

                                        Some((token_id_str.clone(), mid, last_trade))
                                    } else {
                                        None
                                    }
                                }
                                Err(_) => None,
                            };
                            result
                        }
                    })
                    .collect();

                // Wait for all fetches to complete in parallel
                let results = futures::future::join_all(fetch_tasks).await;

                let mut tokens_using_fallback = vec![];

                // Process results and update shared state
                for result in results {
                    if let Some((token_id_str, mid, last_trade)) = result {
                        // Update cache with REST API data
                        let mut prices = prices_fallback
                            .entry(token_id_str.clone())
                            .or_insert_with(|| TokenPrices::new(token_id_str.clone()));

                        prices.mid_price = Some(Decimal::from_f64(mid).unwrap_or(Decimal::ZERO));
                        prices.last_trade = Some(last_trade);
                        prices.last_update_ms = now_ms;
                        // Note: ws_connected remains false, indicating API fallback
                        prices.ws_connected = false;

                        // Send update with API-fetched data
                        let update = PolyPriceUpdate {
                            token_id: token_id_str.clone(),
                            best_bid: prices.best_bid,
                            best_ask: prices.best_ask,
                            mid_price: prices.mid_price,
                        };

                        if let Err(e) = tx_fallback.send(update) {
                            debug!("[REST_FALLBACK] Failed to send update: {:?}", e);
                        }

                        let short_id = if token_id_str.len() > 8 { &token_id_str[..8] } else { &token_id_str };
                        tokens_using_fallback.push(short_id.to_string());
                        fallback_count += 1;
                    }
                }

                // Log when switching to REST fallback
                if !tokens_using_fallback.is_empty() && fallback_count % 10 == 1 {
                    // Only log occasionally to avoid spam
                    warn!("[REST_FALLBACK] Using REST API for {} tokens: [{}]",
                        tokens_using_fallback.len(),
                        tokens_using_fallback.join(", ")
                    );
                    info!("[REST_FALLBACK] State transition: WS_DISCONNECTED -> REST_FALLBACK active");
                }
            }
        });

        info!(
            "Polymarket orderbook stream started for {} tokens with REST fallback",
            token_ids.len()
        );
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
            force_price_reconnect: Arc::clone(&self.force_price_reconnect),
            force_book_reconnect: Arc::clone(&self.force_book_reconnect),
            silent_disconnect_count: Arc::clone(&self.silent_disconnect_count),
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
        assert!(prices.mid_price.is_none());
        assert_eq!(prices.best_bid_size, 0.0);
        assert_eq!(prices.best_ask_size, 0.0);
        assert!(prices.last_trade.is_none());
        assert!(!prices.ws_connected);
        assert_eq!(prices.last_successful_connect_ms, 0); // New field
        assert!(prices.is_stale(1000)); // No updates yet
        assert_eq!(prices.data_source(), "API"); // Default to API when not connected
    }

    #[test]
    fn test_find_size_at_price_with_tolerance() {
        // Create orderbook levels with prices that differ by less than tolerance
        let orders = vec![
            OrderBookLevel {
                price: Decimal::from_str("0.50005").unwrap(),
                size: Decimal::from_str("100.0").unwrap(),
            },
            OrderBookLevel {
                price: Decimal::from_str("0.50010").unwrap(),
                size: Decimal::from_str("200.0").unwrap(),
            },
        ];

        // Target price is slightly different but within tolerance
        let target = Some(Decimal::from_str("0.50006").unwrap());
        let size = find_size_at_price(&orders, target);

        // Should match the first level (0.50005 vs 0.50006 < 0.0001 tolerance)
        assert_eq!(size, 100.0);
    }

    #[test]
    fn test_find_size_at_price_exact_match() {
        let orders = vec![
            OrderBookLevel {
                price: Decimal::from_str("0.50").unwrap(),
                size: Decimal::from_str("150.0").unwrap(),
            },
            OrderBookLevel {
                price: Decimal::from_str("0.51").unwrap(),
                size: Decimal::from_str("250.0").unwrap(),
            },
        ];

        let target = Some(Decimal::from_str("0.50").unwrap());
        let size = find_size_at_price(&orders, target);

        assert_eq!(size, 150.0);
    }

    #[test]
    fn test_find_size_at_price_no_match() {
        let orders = vec![
            OrderBookLevel {
                price: Decimal::from_str("0.50").unwrap(),
                size: Decimal::from_str("100.0").unwrap(),
            },
        ];

        // Target price is far from any level
        let target = Some(Decimal::from_str("0.60").unwrap());
        let size = find_size_at_price(&orders, target);

        // Should return 0.0 when no match found
        assert_eq!(size, 0.0);
    }

    #[test]
    fn test_find_size_at_price_no_target() {
        let orders = vec![
            OrderBookLevel {
                price: Decimal::from_str("0.50").unwrap(),
                size: Decimal::from_str("100.0").unwrap(),
            },
        ];

        // No target price provided
        let size = find_size_at_price(&orders, None);

        // Should return 0.0 when no target
        assert_eq!(size, 0.0);
    }

    #[test]
    fn test_stale_data_detection() {
        let mut prices = TokenPrices::new("test_token".to_string());

        // Initially stale (no updates)
        assert!(prices.is_stale(1000));

        // Update with current time
        prices.last_update_ms = chrono::Utc::now().timestamp_millis();
        prices.ws_connected = true;

        // Should not be stale immediately
        assert!(!prices.is_stale(DEFAULT_STALENESS_MS));

        // Manually set old timestamp
        prices.last_update_ms = chrono::Utc::now().timestamp_millis() - DEFAULT_STALENESS_MS - 1000;

        // Should be stale now
        assert!(prices.is_stale(DEFAULT_STALENESS_MS));
    }

    #[test]
    fn test_data_source_ws_vs_api() {
        let mut prices = TokenPrices::new("test_token".to_string());

        // Initially uses API
        assert_eq!(prices.data_source(), "API");

        // Update with current time and mark as connected
        prices.last_update_ms = chrono::Utc::now().timestamp_millis();
        prices.ws_connected = true;

        // Should show WS now
        assert_eq!(prices.data_source(), "WS");

        // Set old timestamp
        prices.last_update_ms = chrono::Utc::now().timestamp_millis() - WS_CONNECTED_AGE_MS - 1000;

        // Should revert to API for stale data
        assert_eq!(prices.data_source(), "API");
    }

    #[test]
    fn test_price_constants() {
        // Verify constants are set correctly
        assert_eq!(PRICE_MATCH_TOLERANCE, 0.0001);
        assert_eq!(DEFAULT_STALENESS_MS, 5000);
        assert_eq!(WS_CONNECTED_AGE_MS, 5000);
        assert_eq!(REST_FALLBACK_INTERVAL_SECS, 3);
    }

    #[test]
    fn test_connection_health() {
        let stream = PolymarketOrderbookStream::new();

        // Empty stream has 0% health
        assert_eq!(stream.connection_health(), 0.0);

        // Add some simulated price data
        let prices1 = TokenPrices::new("token1".to_string());
        stream.prices.insert("token1".to_string(), prices1);

        let mut prices2 = TokenPrices::new("token2".to_string());
        prices2.ws_connected = true;
        prices2.last_update_ms = chrono::Utc::now().timestamp_millis();
        stream.prices.insert("token2".to_string(), prices2);

        // 50% connected (1 out of 2)
        assert!((stream.connection_health() - 50.0).abs() < 0.01);
    }
}
