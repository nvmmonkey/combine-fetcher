//! Polymarket Market Lifecycle Events via WebSocket
//!
//! This module subscribes to `new_market` and `market_resolved` WebSocket events
//! from the Polymarket CLOB. Events are stored in a HashMap keyed by slug so
//! multiple assets/timeframes can be tracked simultaneously.
//!
//! Backward compatible: `take_resolved()` / `take_new_market()` still work
//! (they return any available event). New per-slug methods are provided for
//! multi-asset consumers.

use anyhow::Result;
use futures::StreamExt;
use polymarket_client_sdk_v2::clob::ws::Client as WsClient;
use polymarket_client_sdk_v2::types::U256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Reconnection constants (same as rtds_tracker.rs / polymarket.rs)
// ---------------------------------------------------------------------------

const INITIAL_RECONNECT_DELAY_MS: u64 = 1000;
const MAX_RECONNECT_DELAY_MS: u64 = 60000;
const RECONNECT_BACKOFF_MULTIPLIER: f64 = 2.0;

/// Pattern filter for new_market events — only keep updown markets.
const UPDOWN_PATTERN: &str = "-updown-";

/// Helper: extract slug from SDK types that may be String or Option<String>
/// depending on SDK version. Works with both via Into<Option<String>>.
fn extract_slug(slug: impl Into<Option<String>>) -> Option<String> {
    slug.into()
}

// ---------------------------------------------------------------------------
// Our own simplified event types (don't leak SDK types into the binary)
// ---------------------------------------------------------------------------

/// A new market event, filtered to only updown markets.
#[derive(Debug, Clone)]
pub struct NewMarketEvent {
    pub slug: String,
    pub asset_ids: Vec<String>,
    pub outcomes: Vec<String>,
    pub timestamp: i64,
}

/// A market resolved event.
#[derive(Debug, Clone)]
pub struct MarketResolvedEvent {
    pub slug: String,
    pub winning_outcome: String,
    pub winning_asset_id: String,
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// MarketEventStream
// ---------------------------------------------------------------------------

/// Streams `new_market` and `market_resolved` WebSocket events from Polymarket.
///
/// Events are stored in HashMaps keyed by slug. Per-slug accessors allow
/// multi-asset consumers to poll for events on specific markets, while the
/// original take-and-clear methods remain for single-market consumers.
pub struct MarketEventStream {
    new_markets: Arc<Mutex<HashMap<String, NewMarketEvent>>>,
    resolved: Arc<Mutex<HashMap<String, MarketResolvedEvent>>>,
}

impl MarketEventStream {
    /// Create a new (idle) event stream. Call [`start`] to begin listening.
    #[must_use]
    pub fn new() -> Self {
        Self {
            new_markets: Arc::new(Mutex::new(HashMap::new())),
            resolved: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Begin listening for market lifecycle events.
    ///
    /// `initial_token_ids` — at least one token ID string is required by the SDK
    /// to establish the WebSocket connection.  Pass the current market's YES/NO
    /// token IDs (for multi-asset, pass all active tokens).
    pub fn start(&self, initial_token_ids: Vec<String>) {
        let asset_ids: Vec<U256> = initial_token_ids
            .iter()
            .filter_map(|s| U256::from_str(s).ok())
            .collect();

        if asset_ids.is_empty() {
            warn!("[MARKET_EVENTS] No valid token IDs provided — not starting");
            return;
        }

        // Spawn the two independent reconnection loops.
        let nm_store = Arc::clone(&self.new_markets);
        let nm_ids = asset_ids.clone();
        tokio::spawn(async move {
            new_market_loop(nm_store, nm_ids).await;
        });

        let mr_store = Arc::clone(&self.resolved);
        tokio::spawn(async move {
            market_resolved_loop(mr_store, asset_ids).await;
        });

        info!("[MARKET_EVENTS] Started new_market + market_resolved listeners");
    }

    // -----------------------------------------------------------------------
    // Backward-compatible single-event accessors (take any available event)
    // -----------------------------------------------------------------------

    /// Take (and clear) any `NewMarketEvent`. For single-market consumers.
    pub fn take_new_market(&self) -> Option<NewMarketEvent> {
        let mut map = self.new_markets.lock().ok()?;
        let key = map.keys().next()?.clone();
        map.remove(&key)
    }

    /// Take (and clear) any `MarketResolvedEvent`. For single-market consumers.
    pub fn take_resolved(&self) -> Option<MarketResolvedEvent> {
        let mut map = self.resolved.lock().ok()?;
        let key = map.keys().next()?.clone();
        map.remove(&key)
    }

    // -----------------------------------------------------------------------
    // Per-slug accessors for multi-asset consumers
    // -----------------------------------------------------------------------

    /// Take (and clear) a `NewMarketEvent` whose slug starts with `slug_prefix`.
    ///
    /// For multi-asset: pass e.g. `"eth-updown-5m-"` to get the next ETH 5m market.
    pub fn take_new_market_for(&self, slug_prefix: &str) -> Option<NewMarketEvent> {
        let mut map = self.new_markets.lock().ok()?;
        let key = map.keys().find(|k| k.starts_with(slug_prefix))?.clone();
        map.remove(&key)
    }

    /// Take (and clear) a `MarketResolvedEvent` matching the exact slug.
    pub fn take_resolved_for(&self, slug: &str) -> Option<MarketResolvedEvent> {
        let mut map = self.resolved.lock().ok()?;
        map.remove(slug)
    }

    /// Take (and clear) a `MarketResolvedEvent` whose slug starts with `slug_prefix`.
    pub fn take_resolved_for_prefix(&self, slug_prefix: &str) -> Option<MarketResolvedEvent> {
        let mut map = self.resolved.lock().ok()?;
        let key = map.keys().find(|k| k.starts_with(slug_prefix))?.clone();
        map.remove(&key)
    }
}

impl Default for MarketEventStream {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Reconnection loops (mirrors rtds_tracker.rs pattern)
// ---------------------------------------------------------------------------

async fn new_market_loop(store: Arc<Mutex<HashMap<String, NewMarketEvent>>>, asset_ids: Vec<U256>) {
    let mut delay_ms = INITIAL_RECONNECT_DELAY_MS;

    loop {
        info!("[MARKET_EVENTS] Connecting new_market stream...");

        match connect_new_market(&store, &asset_ids).await {
            Ok(count) => {
                info!(
                    "[MARKET_EVENTS] new_market stream ended after {} messages, reconnecting...",
                    count
                );
                delay_ms = INITIAL_RECONNECT_DELAY_MS;
            }
            Err(e) => {
                warn!("[MARKET_EVENTS] new_market connection error: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms =
            ((delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64).min(MAX_RECONNECT_DELAY_MS);
    }
}

async fn market_resolved_loop(
    store: Arc<Mutex<HashMap<String, MarketResolvedEvent>>>,
    asset_ids: Vec<U256>,
) {
    let mut delay_ms = INITIAL_RECONNECT_DELAY_MS;

    loop {
        info!("[MARKET_EVENTS] Connecting market_resolved stream...");

        match connect_market_resolved(&store, &asset_ids).await {
            Ok(count) => {
                info!(
                    "[MARKET_EVENTS] market_resolved stream ended after {} messages, reconnecting...",
                    count
                );
                delay_ms = INITIAL_RECONNECT_DELAY_MS;
            }
            Err(e) => {
                warn!("[MARKET_EVENTS] market_resolved connection error: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms =
            ((delay_ms as f64 * RECONNECT_BACKOFF_MULTIPLIER) as u64).min(MAX_RECONNECT_DELAY_MS);
    }
}

// ---------------------------------------------------------------------------
// Inner connection helpers
// ---------------------------------------------------------------------------

async fn connect_new_market(
    store: &Arc<Mutex<HashMap<String, NewMarketEvent>>>,
    asset_ids: &[U256],
) -> Result<u64> {
    let ws_client = WsClient::default();
    let stream = ws_client.subscribe_new_markets(asset_ids.to_vec())?;
    let mut stream = Box::pin(stream);
    let mut count: u64 = 0;

    while let Some(result) = stream.next().await {
        match result {
            Ok(nm) => {
                count += 1;

                // Handle slug as String or Option<String> (varies by SDK version)
                let slug = match extract_slug(nm.slug) {
                    Some(s) => s,
                    None => continue,
                };

                // Only keep events matching *-updown-* pattern
                if !slug.contains(UPDOWN_PATTERN) {
                    continue;
                }

                let event = NewMarketEvent {
                    slug: slug.clone(),
                    asset_ids: nm.asset_ids.iter().map(|id| id.to_string()).collect(),
                    outcomes: nm.outcomes,
                    timestamp: nm.timestamp,
                };

                info!(
                    "[MARKET_EVENTS] new_market: slug={} asset_ids={:?}",
                    event.slug, event.asset_ids
                );

                if let Ok(mut map) = store.lock() {
                    map.insert(slug, event);
                }
            }
            Err(e) => {
                warn!("[MARKET_EVENTS] new_market stream error: {}", e);
                return Err(e.into());
            }
        }
    }

    Ok(count)
}

async fn connect_market_resolved(
    store: &Arc<Mutex<HashMap<String, MarketResolvedEvent>>>,
    asset_ids: &[U256],
) -> Result<u64> {
    let ws_client = WsClient::default();
    let stream = ws_client.subscribe_market_resolutions(asset_ids.to_vec())?;
    let mut stream = Box::pin(stream);
    let mut count: u64 = 0;

    while let Some(result) = stream.next().await {
        match result {
            Ok(mr) => {
                count += 1;

                // Handle slug as String or Option<String> (varies by SDK version)
                let slug = match extract_slug(mr.slug) {
                    Some(s) => s,
                    None => continue,
                };

                let event = MarketResolvedEvent {
                    slug: slug.clone(),
                    winning_outcome: mr.winning_outcome,
                    winning_asset_id: mr.winning_asset_id.to_string(),
                    timestamp: mr.timestamp,
                };

                info!(
                    "[MARKET_EVENTS] market_resolved: slug={} winner={}",
                    event.slug, event.winning_outcome
                );

                if let Ok(mut map) = store.lock() {
                    map.insert(slug, event);
                }
            }
            Err(e) => {
                warn!("[MARKET_EVENTS] market_resolved stream error: {}", e);
                return Err(e.into());
            }
        }
    }

    Ok(count)
}
