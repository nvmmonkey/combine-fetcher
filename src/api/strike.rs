//! Strike price fetching from Polymarket crypto API with configurable retry.

use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::DateTime;
use tracing::{debug, info, warn};
use serde::Deserialize;

use super::gamma::MarketInfo;
use crate::constants::{HTTP_TIMEOUT_SECS, POLY_CRYPTO_API_URL, STRIKE_RETRY_INTERVAL_MS};

/// Configuration for retry behavior in strike price fetching.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts. `None` means infinite retries.
    pub max_attempts: Option<u32>,
    /// Initial backoff duration in milliseconds before first retry.
    pub initial_backoff_ms: u64,
    /// Maximum backoff duration in milliseconds (caps exponential growth).
    pub max_backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: None,
            initial_backoff_ms: STRIKE_RETRY_INTERVAL_MS,
            max_backoff_ms: 500,
        }
    }
}

#[derive(Debug, Deserialize)]
struct PolymarketStrikeResponse {
    #[serde(rename = "openPrice")]
    open_price: Option<f64>,
}

/// Map window_seconds to the Polymarket variant query parameter.
fn variant_for_window(window_seconds: i64) -> &'static str {
    if window_seconds == 300 {
        "fiveminute"
    } else {
        "fifteen"
    }
}

/// Build the Polymarket crypto-price API URL for a given market's time window.
///
/// Derives `symbol` from `market.asset` (uppercased, defaults to "BTC" if empty)
/// and `variant` from `market.window_seconds` (300 -> "fiveminute", else "fifteen").
fn build_strike_url(market: &MarketInfo) -> Result<String> {
    let symbol = if market.asset.is_empty() {
        "BTC".to_string()
    } else {
        market.asset.to_uppercase()
    };
    let variant = variant_for_window(market.window_seconds);

    let start =
        DateTime::from_timestamp(market.window_start, 0).ok_or_else(|| anyhow!("Bad timestamp"))?;
    let end =
        DateTime::from_timestamp(market.window_end, 0).ok_or_else(|| anyhow!("Bad timestamp"))?;

    Ok(format!(
        "{}/crypto-price?symbol={}&eventStartTime={}&variant={}&endDate={}",
        POLY_CRYPTO_API_URL,
        symbol,
        start.format("%Y-%m-%dT%H:%M:%SZ"),
        variant,
        end.format("%Y-%m-%dT%H:%M:%SZ")
    ))
}

/// Fetch the strike price for a market in a single attempt.
///
/// Returns `Ok(price)` on success, or an error if the request fails,
/// the response cannot be parsed, or the price field is absent.
pub async fn fetch_strike(client: &reqwest::Client, market: &MarketInfo) -> Result<f64> {
    let url = build_strike_url(market)?;
    debug!("Strike fetch URL for {}: {}", market.unique_id, url);

    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow!(
            "Strike fetch for {} returned HTTP {}",
            market.unique_id,
            resp.status()
        ));
    }

    let data: PolymarketStrikeResponse = resp.json().await?;
    data.open_price
        .ok_or_else(|| anyhow!("Strike price not yet available for {}", market.unique_id))
}

/// Fetch the strike price with exponential-backoff retry.
///
/// Uses the provided [`RetryConfig`] or sensible defaults (infinite retries,
/// 200 ms initial backoff, 5 s max backoff).
pub async fn fetch_strike_with_retry(
    client: &reqwest::Client,
    market: &MarketInfo,
    config: Option<RetryConfig>,
) -> Result<f64> {
    let config = config.unwrap_or_default();
    let mut attempt: u32 = 1;
    let mut backoff_ms = config.initial_backoff_ms;

    loop {
        debug!(
            "Attempting strike fetch (attempt {}) for {}",
            attempt, market.unique_id
        );

        match fetch_strike(client, market).await {
            Ok(price) => {
                info!(
                    "Strike fetched for {} on attempt {}: ${:.2}",
                    market.unique_id, attempt, price
                );
                return Ok(price);
            }
            Err(e) => {
                warn!(
                    "Attempt {} for {}: {}",
                    attempt, market.unique_id, e
                );
            }
        }

        if let Some(max) = config.max_attempts {
            if attempt >= max {
                return Err(anyhow!(
                    "Strike fetch for {} failed after {} attempts",
                    market.unique_id,
                    attempt
                ));
            }
        }

        info!(
            "Strike fetch not yet available, retrying in {}ms... (attempt {})",
            backoff_ms, attempt
        );
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;

        backoff_ms = (backoff_ms * 2).min(config.max_backoff_ms);
        attempt += 1;
    }
}
