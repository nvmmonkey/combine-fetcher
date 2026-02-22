//! Debug binary for testing multi-asset market resolution detection.
//!
//! Fetches a couple of current markets (btc-15m, eth-5m), starts a
//! MarketEventStream with their token IDs, and prints every WS event
//! alongside periodic Gamma API polls for comparison.
//!
//! Usage: cargo run --bin debug_resolution

use anyhow::Result;
use chrono::Utc;
use std::time::Duration;
use tracing::{info, warn};

use combine_fetcher::api::gamma;
use combine_fetcher::api::market_events::MarketEventStream;
use combine_fetcher::HTTP_TIMEOUT_SECS;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("debug_resolution=info".parse().unwrap())
                .add_directive("combine_fetcher=info".parse().unwrap()),
        )
        .init();

    println!("=== Debug Resolution Detection ===\n");

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(3))
        .build()?;

    // Fetch two markets: BTC 15m and ETH 5m
    println!("Fetching current markets...");

    let btc_15m = gamma::fetch_current_market_for(&http, "btc", 900).await?;
    println!(
        "  BTC-15m: {} | YES: {}... | NO: {}...",
        btc_15m.slug,
        &btc_15m.yes_token_id[..12.min(btc_15m.yes_token_id.len())],
        &btc_15m.no_token_id[..12.min(btc_15m.no_token_id.len())]
    );

    let eth_5m = gamma::fetch_current_market_for(&http, "eth", 300).await?;
    println!(
        "  ETH-5m:  {} | YES: {}... | NO: {}...",
        eth_5m.slug,
        &eth_5m.yes_token_id[..12.min(eth_5m.yes_token_id.len())],
        &eth_5m.no_token_id[..12.min(eth_5m.no_token_id.len())]
    );

    // Collect all token IDs
    let all_tokens = vec![
        btc_15m.yes_token_id.clone(),
        btc_15m.no_token_id.clone(),
        eth_5m.yes_token_id.clone(),
        eth_5m.no_token_id.clone(),
    ];

    // Start MarketEventStream
    println!("\nStarting MarketEventStream with {} tokens...", all_tokens.len());
    let market_events = MarketEventStream::new();
    market_events.start(all_tokens);
    println!("Listening for WS events...\n");

    let markets = vec![
        ("BTC-15m", btc_15m.slug.clone(), "btc-updown-15m-"),
        ("ETH-5m", eth_5m.slug.clone(), "eth-updown-5m-"),
    ];

    let mut gamma_poll_tick: u64 = 0;
    let poll_interval = Duration::from_secs(10);
    let mut last_gamma_poll = std::time::Instant::now();

    println!("Polling loop started (Ctrl+C to exit)");
    println!("  - WS events: checked every 500ms");
    println!("  - Gamma API: polled every 10s\n");

    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = Utc::now();

        // Check for WS resolved events (per-slug)
        for (tag, slug, _prefix) in &markets {
            if let Some(ev) = market_events.take_resolved_for(slug) {
                println!(
                    "[{}] [WS RESOLVED] {} slug={} winner={} asset_id={}...",
                    now.format("%H:%M:%S%.3f"),
                    tag,
                    ev.slug,
                    ev.winning_outcome,
                    &ev.winning_asset_id[..12.min(ev.winning_asset_id.len())]
                );
            }
        }

        // Check for WS new_market events (per prefix)
        for (tag, _slug, prefix) in &markets {
            if let Some(ev) = market_events.take_new_market_for(prefix) {
                if ev.asset_ids.len() >= 2 {
                    println!(
                        "[{}] [WS NEW_MARKET] {} slug={} outcomes={:?} asset_ids=[{}..., {}...]",
                        now.format("%H:%M:%S%.3f"),
                        tag,
                        ev.slug,
                        ev.outcomes,
                        &ev.asset_ids[0][..12.min(ev.asset_ids[0].len())],
                        &ev.asset_ids[1][..12.min(ev.asset_ids[1].len())]
                    );
                } else {
                    println!(
                        "[{}] [WS NEW_MARKET] {} slug={} outcomes={:?} asset_ids={:?} (unexpected shape)",
                        now.format("%H:%M:%S%.3f"),
                        tag,
                        ev.slug,
                        ev.outcomes,
                        ev.asset_ids
                    );
                }
            }
        }

        // Also check backward-compat methods (catches any slug not in our list)
        if let Some(ev) = market_events.take_resolved() {
            println!(
                "[{}] [WS RESOLVED (any)] slug={} winner={}",
                now.format("%H:%M:%S%.3f"),
                ev.slug,
                ev.winning_outcome
            );
        }
        if let Some(ev) = market_events.take_new_market() {
            println!(
                "[{}] [WS NEW_MARKET (any)] slug={} outcomes={:?}",
                now.format("%H:%M:%S%.3f"),
                ev.slug,
                ev.outcomes
            );
        }

        // Periodic Gamma API poll for comparison
        if last_gamma_poll.elapsed() >= poll_interval {
            last_gamma_poll = std::time::Instant::now();
            gamma_poll_tick += 1;

            for (tag, slug, _prefix) in &markets {
                match gamma::fetch_outcome(&http, slug).await {
                    Ok(outcome) => {
                        info!(
                            "[GAMMA #{:>3}] {} {} -> {}",
                            gamma_poll_tick, tag, slug, outcome
                        );
                    }
                    Err(e) => {
                        // Expected when market is still active (no resolution yet)
                        if gamma_poll_tick % 6 == 1 {
                            // Log once per minute to avoid spam
                            warn!(
                                "[GAMMA #{:>3}] {} {} -> no outcome yet ({})",
                                gamma_poll_tick, tag, slug, e
                            );
                        }
                    }
                }
            }
        }
    }
}
