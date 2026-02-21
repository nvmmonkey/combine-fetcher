//! Gamma API client for Polymarket event data.
//!
//! Provides functions to fetch BTC up/down market info and resolution outcomes
//! from the Gamma API.

use anyhow::{anyhow, Result};
use chrono::Utc;
use std::time::Duration;
use tracing::info;

use crate::constants::{GAMMA_API_URL, HTTP_TIMEOUT_SECS, MARKET_WINDOW_SECONDS};

/// Market information for a BTC up/down 15-minute window.
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
}

/// Fetch market data from the Gamma API for a given window start timestamp.
///
/// This is the shared implementation used by both `fetch_current_market` and
/// `fetch_next_market`. It constructs the slug from `window_start`, calls the
/// Gamma API, and parses the Up/Down token IDs from the response.
async fn fetch_market(client: &reqwest::Client, window_start: i64) -> Result<MarketInfo> {
    let window_end = window_start + MARKET_WINDOW_SECONDS;
    let unique_id = format!("btc-updown-15m-{}", window_start);
    let slug = format!("btc-updown-15m-{}", window_start);
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
            });
        }
    }
    Err(anyhow!(
        "No valid BTC market for window {}",
        window_start
    ))
}

/// Fetch the current market based on the current UTC time.
///
/// Window calculation: if we're within the first 60 seconds of a window,
/// use the previous window (to avoid fetching a market that doesn't exist yet).
pub async fn fetch_current_market(client: &reqwest::Client) -> Result<MarketInfo> {
    let now = Utc::now().timestamp();
    let pos = now % MARKET_WINDOW_SECONDS;
    let window_start = if pos < 60 {
        ((now / MARKET_WINDOW_SECONDS) - 1) * MARKET_WINDOW_SECONDS
    } else {
        (now / MARKET_WINDOW_SECONDS) * MARKET_WINDOW_SECONDS
    };
    fetch_market(client, window_start).await
}

/// Fetch the next market after the given window start timestamp.
pub async fn fetch_next_market(
    client: &reqwest::Client,
    current_window_start: i64,
) -> Result<MarketInfo> {
    let next_window_start = current_window_start + MARKET_WINDOW_SECONDS;
    fetch_market(client, next_window_start).await
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
                    if prices[ui] == "1" {
                        return Ok("YES".to_string());
                    } else if prices[di] == "1" {
                        return Ok("NO".to_string());
                    }
                }
            }
        }
    }

    Err(anyhow!("No conclusive outcome from Gamma API for {}", slug))
}
