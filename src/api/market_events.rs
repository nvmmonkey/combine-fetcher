//! Polymarket Market Lifecycle Events via WebSocket
//!
//! This module subscribes to `new_market` and `market_resolved` WebSocket events
//! from the Polymarket CLOB. Events are stored in a take-and-clear pattern so
//! the consumer can poll for the latest event without missing it.
//!
//! Only `new_market` events matching the `btc-updown-15m-*` slug pattern are kept.

use anyhow::Result;
use futures::StreamExt;
use polymarket_client_sdk::clob::ws::Client as WsClient;
use polymarket_client_sdk::types::U256;
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

/// Slug prefix filter — only keep new_market events for BTC 15-min up/down markets.
const SLUG_PREFIX: &str = "btc-updown-15m-";

// ---------------------------------------------------------------------------
// Our own simplified event types (don't leak SDK types into the binary)
// ---------------------------------------------------------------------------

/// A new market event, filtered to only BTC 15-min up/down markets.
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
/// Uses the take-and-clear pattern: each call to [`take_new_market`] /
/// [`take_resolved`] returns the latest event (if any) and clears the slot.
pub struct MarketEventStream {
    latest_new_market: Arc<Mutex<Option<NewMarketEvent>>>,
    latest_resolved: Arc<Mutex<Option<MarketResolvedEvent>>>,
}

impl MarketEventStream {
    /// Create a new (idle) event stream. Call [`start`] to begin listening.
    #[must_use]
    pub fn new() -> Self {
        Self {
            latest_new_market: Arc::new(Mutex::new(None)),
            latest_resolved: Arc::new(Mutex::new(None)),
        }
    }

    /// Begin listening for market lifecycle events.
    ///
    /// `initial_token_ids` — at least one token ID string is required by the SDK
    /// to establish the WebSocket connection.  Pass the current market's YES/NO
    /// token IDs.
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
        let nm_store = Arc::clone(&self.latest_new_market);
        let nm_ids = asset_ids.clone();
        tokio::spawn(async move {
            new_market_loop(nm_store, nm_ids).await;
        });

        let mr_store = Arc::clone(&self.latest_resolved);
        tokio::spawn(async move {
            market_resolved_loop(mr_store, asset_ids).await;
        });

        info!("[MARKET_EVENTS] Started new_market + market_resolved listeners");
    }

    /// Take (and clear) the latest `NewMarketEvent`, if any.
    pub fn take_new_market(&self) -> Option<NewMarketEvent> {
        self.latest_new_market.lock().ok()?.take()
    }

    /// Take (and clear) the latest `MarketResolvedEvent`, if any.
    pub fn take_resolved(&self) -> Option<MarketResolvedEvent> {
        self.latest_resolved.lock().ok()?.take()
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

async fn new_market_loop(store: Arc<Mutex<Option<NewMarketEvent>>>, asset_ids: Vec<U256>) {
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
    store: Arc<Mutex<Option<MarketResolvedEvent>>>,
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
    store: &Arc<Mutex<Option<NewMarketEvent>>>,
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

                // Only keep events matching our slug prefix
                if !nm.slug.starts_with(SLUG_PREFIX) {
                    continue;
                }

                let event = NewMarketEvent {
                    slug: nm.slug,
                    asset_ids: nm.asset_ids.iter().map(|id| id.to_string()).collect(),
                    outcomes: nm.outcomes,
                    timestamp: nm.timestamp,
                };

                info!(
                    "[MARKET_EVENTS] new_market: slug={} asset_ids={:?}",
                    event.slug, event.asset_ids
                );

                if let Ok(mut slot) = store.lock() {
                    *slot = Some(event);
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
    store: &Arc<Mutex<Option<MarketResolvedEvent>>>,
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

                let event = MarketResolvedEvent {
                    slug: mr.slug,
                    winning_outcome: mr.winning_outcome,
                    winning_asset_id: mr.winning_asset_id.to_string(),
                    timestamp: mr.timestamp,
                };

                info!(
                    "[MARKET_EVENTS] market_resolved: slug={} winner={}",
                    event.slug, event.winning_outcome
                );

                if let Ok(mut slot) = store.lock() {
                    *slot = Some(event);
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
