//! Gamma API client for Polymarket event data.
//!
//! Provides functions to fetch market info and resolution outcomes from the
//! Gamma API. Supports multiple assets (btc, eth, sol, xrp) and timeframes
//! (5m = 300s, 15m = 900s).

use anyhow::{anyhow, Result};
use chrono::Utc;
use std::time::Duration;
use tracing::info;

use crate::constants::{GAMMA_API_URL, HTTP_TIMEOUT_SECS, MARKET_WINDOW_SECONDS};

/// Market information for an up/down binary market window.
#[derive(Debug, Clone)]
pub struct MarketInfo {
    pub unique_id: String,
    pub slug: String,
    pub condition_id: String,
    pub yes_token_id: String,
    pub no_token_id: String,
    pub window_start: i64,
    pub window_end: i64,
    pub strike_price: Option<f64>,
    pub strike_fetched: bool,
    pub asset: String,
    pub window_seconds: i64,
}

/// Convert window_seconds to a timeframe label used in slugs.
fn timeframe_str(window_seconds: i64) -> &'static str {
    if window_seconds == 300 {
        "5m"
    } else {
        "15m"
    }
}

/// Fetch market data from the Gamma API for a specific asset, timeframe, and window.
///
/// Builds a slug like `{asset}-updown-{timeframe}-{window_start}`, calls the
/// Gamma API, and parses the Up/Down token IDs from the response.
pub async fn fetch_market_for(
    client: &reqwest::Client,
    asset: &str,
    window_seconds: i64,
    window_start: i64,
) -> Result<MarketInfo> {
    let tf = timeframe_str(window_seconds);
    let window_end = window_start + window_seconds;
    let unique_id = format!("{}-updown-{}-{}", asset, tf, window_start);
    let slug = unique_id.clone();
    let url = format!("{}/events/slug/{}", GAMMA_API_URL, slug);

    info!("Fetching market: {}", unique_id);
    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("Market fetch failed: {}", resp.status()));
    }

    let data: serde_json::Value = resp.json().await?;
    let markets = data["markets"]
        .as_array()
        .ok_or_else(|| anyhow!("No markets"))?;

    for m in markets {
        if !m["active"].as_bool().unwrap_or(false) {
            continue;
        }
        let outcomes: Vec<String> =
            serde_json::from_str(m["outcomes"].as_str().unwrap_or("[]")).unwrap_or_default();
        let tokens: Vec<String> =
            serde_json::from_str(m["clobTokenIds"].as_str().unwrap_or("[]")).unwrap_or_default();
        if outcomes.len() != 2 || tokens.len() != 2 {
            continue;
        }

        let up_idx = outcomes.iter().position(|o| o == "Up");
        let down_idx = outcomes.iter().position(|o| o == "Down");

        if let (Some(ui), Some(di)) = (up_idx, down_idx) {
            return Ok(MarketInfo {
                unique_id,
                slug,
                condition_id: m["conditionId"].as_str().unwrap_or("").to_string(),
                yes_token_id: tokens[ui].clone(),
                no_token_id: tokens[di].clone(),
                window_start,
                window_end,
                strike_price: None,
                strike_fetched: false,
                asset: asset.to_string(),
                window_seconds,
            });
        }
    }
    Err(anyhow!(
        "No valid {} market for window {}",
        asset.to_uppercase(),
        window_start
    ))
}

/// Fetch the current market for a given asset and timeframe.
///
/// Window calculation: if we're within the first 60 seconds of a window,
/// use the previous window (to avoid fetching a market that doesn't exist yet).
pub async fn fetch_current_market_for(
    client: &reqwest::Client,
    asset: &str,
    window_seconds: i64,
) -> Result<MarketInfo> {
    let now = Utc::now().timestamp();
    let pos = now % window_seconds;
    let window_start = if pos < 60 {
        ((now / window_seconds) - 1) * window_seconds
    } else {
        (now / window_seconds) * window_seconds
    };
    fetch_market_for(client, asset, window_seconds, window_start).await
}

/// Fetch the next market after the given window start for a given asset and timeframe.
pub async fn fetch_next_market_for(
    client: &reqwest::Client,
    asset: &str,
    window_seconds: i64,
    current_window_start: i64,
) -> Result<MarketInfo> {
    let next_window_start = current_window_start + window_seconds;
    fetch_market_for(client, asset, window_seconds, next_window_start).await
}

/// Fetch the current BTC 15m market (backward-compatible wrapper).
pub async fn fetch_current_market(client: &reqwest::Client) -> Result<MarketInfo> {
    fetch_current_market_for(client, "btc", MARKET_WINDOW_SECONDS).await
}

/// Fetch the next BTC 15m market (backward-compatible wrapper).
pub async fn fetch_next_market(
    client: &reqwest::Client,
    current_window_start: i64,
) -> Result<MarketInfo> {
    fetch_next_market_for(client, "btc", MARKET_WINDOW_SECONDS, current_window_start).await
}

/// Fetch market outcome from the Gamma API for resolution detection.
///
/// Reads `outcomePrices` from the event's market data to determine which outcome won.
/// Returns `"YES"` if Up won, `"NO"` if Down won, or an error if inconclusive.
pub async fn fetch_outcome(client: &reqwest::Client, slug: &str) -> Result<String> {
    let url = format!("{}/events/slug/{}", GAMMA_API_URL, slug);
    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("Gamma outcome fetch failed: {}", resp.status()));
    }
    let data: serde_json::Value = resp.json().await?;

    if let Some(markets) = data["markets"].as_array() {
        for m in markets {
            let outcomes: Vec<String> =
                serde_json::from_str(m["outcomes"].as_str().unwrap_or("[]")).unwrap_or_default();
            let outcome_prices_str = m["outcomePrices"].as_str().unwrap_or("[]");
            let prices: Vec<String> =
                serde_json::from_str(outcome_prices_str).unwrap_or_default();

            if outcomes.len() >= 2 && prices.len() >= 2 {
                let up_idx = outcomes.iter().position(|o| o == "Up");
                let down_idx = outcomes.iter().position(|o| o == "Down");

                if let (Some(ui), Some(di)) = (up_idx, down_idx) {
                    let up_price = prices[ui].parse::<f64>().unwrap_or(0.0);
                    let down_price = prices[di].parse::<f64>().unwrap_or(0.0);
                    if up_price >= 0.95 {
                        return Ok("YES".to_string());
                    } else if down_price >= 0.95 {
                        return Ok("NO".to_string());
                    }
                }
            }
        }
    }

    Err(anyhow!("No conclusive outcome from Gamma API for {}", slug))
}
