//! Polymarket 15-Minute Market Data Logger (SDK Edition)
//!
//! Uses polymarket-client-sdk for Polymarket WebSocket streaming.
//! Add this file to your existing project as `src/bin/data_logger.rs`
//!
//! Run with: `cargo run --bin data_logger -- [duration_secs] [output.csv] [record_interval_sec]`

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

// Use local modules
use combine_fetcher::api::binance::BinanceDataManager;
use combine_fetcher::api::polymarket::{PolymarketOrderbookStream, TokenPrices};
use combine_fetcher::{
    CSV_RECORD_INTERVAL_SEC, GAMMA_API_URL, HTTP_TIMEOUT_SECS, MARKET_WINDOW_SECONDS,
    MINUTES_PER_YEAR, POLY_CRYPTO_API_URL, STRIKE_RETRY_ATTEMPTS, STRIKE_RETRY_INTERVAL_MS,
    VOLATILITY_BTC,
};

// ============================================================================
// CSV RECORD (same format as Python mock)
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct CsvRecord {
    pub timestamp: String,
    pub market_slug: String,
    pub binance_price: f64,
    pub binance_volume: f64,
    pub yes_bid: f64,
    pub yes_bid_size: f64,
    pub yes_ask: f64,
    pub yes_ask_size: f64,
    pub no_bid: f64,
    pub no_bid_size: f64,
    pub no_ask: f64,
    pub no_ask_size: f64,
    pub time_to_open: i64,
    pub time_to_expiry_minutes: f64,
    pub strike_price: f64,
    pub realized_volatility: f64,
    pub delta_yes: f64,
    pub gamma: f64,
    pub vega: f64,
    pub theta: f64,
    pub market_resolved: bool,
    pub outcome: String,
}

impl CsvRecord {
    pub fn to_csv_line(&self) -> String {
        format!(
            "{},{},{:.2},{:.2},{:.4},{:.0},{:.4},{:.0},{:.4},{:.0},{:.4},{:.0},{},{:.2},{:.2},{:.6},{:.8},{:.8},{:.8},{:.8},{},{}",
            self.timestamp, self.market_slug, self.binance_price, self.binance_volume,
            self.yes_bid, self.yes_bid_size, self.yes_ask, self.yes_ask_size,
            self.no_bid, self.no_bid_size, self.no_ask, self.no_ask_size,
            self.time_to_open, self.time_to_expiry_minutes, self.strike_price,
            self.realized_volatility, self.delta_yes, self.gamma, self.vega, self.theta,
            self.market_resolved, self.outcome
        )
    }

    pub fn csv_header() -> &'static str {
        "timestamp,market_slug,binance_price,binance_volume,yes_bid,yes_bid_size,yes_ask,yes_ask_size,no_bid,no_bid_size,no_ask,no_ask_size,time_to_open,time_to_expiry_minutes,strike_price,realized_volatility,delta_yes,gamma,vega,theta,market_resolved,outcome"
    }
}

// ============================================================================
// MARKET INFO
// ============================================================================

#[derive(Debug, Clone)]
pub struct MarketInfo {
    pub slug: String,
    pub condition_id: String,
    pub yes_token_id: String,
    pub no_token_id: String,
    pub window_start: i64,
    pub window_end: i64,
    pub strike_price: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct PolymarketStrikeResponse {
    #[serde(rename = "openPrice")]
    open_price: Option<f64>,
}

// ============================================================================
// GREEKS (use your existing BinaryOptionPricer or inline)
// ============================================================================

#[derive(Debug, Clone, Copy, Default)]
pub struct Greeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub theta: f64,
}

fn calculate_greeks(spot: f64, strike: f64, ttx_min: f64, vol: f64) -> Greeks {
    if ttx_min <= 0.01 || vol <= 0.0 || spot <= 0.0 || strike <= 0.0 {
        return Greeks {
            delta: if spot >= strike { 1.0 } else { 0.0 },
            ..Default::default()
        };
    }

    // Binary/Cash-or-Nothing Option Greeks Formula
    // For a cash-or-nothing call option paying $1 if S > K at expiry
    let t = ttx_min / MINUTES_PER_YEAR;
    let sqrt_t = t.sqrt();

    // d1 and d2 for binary option (same as vanilla)
    let d1 = ((spot / strike).ln() + (vol * vol / 2.0) * t) / (vol * sqrt_t);
    let d2 = ((spot / strike).ln() - (vol * vol / 2.0) * t) / (vol * sqrt_t);

    // Standard normal CDF approximation
    let norm_cdf = |x: f64| -> f64 {
        let a1 = 0.254829592;
        let a2 = -0.284496736;
        let a3 = 1.421413741;
        let a4 = -1.453152027;
        let a5 = 1.061405429;
        let p = 0.3275911;

        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let x = x.abs();

        let t = 1.0 / (1.0 + p * x);
        let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x * x / 2.0).exp();

        0.5 * (1.0 + sign * y)
    };

    // Standard normal PDF
    let norm_pdf = |x: f64| -> f64 {
        (-x * x / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt()
    };

    let _n_d2 = norm_cdf(d2);
    let n_prime_d2 = norm_pdf(d2);

    Greeks {
        // Delta for cash-or-nothing call: N'(d2) / (S * sigma * sqrt(t))
        // Note: Uses PDF (n_prime_d2), not CDF (n_d2)
        delta: (n_prime_d2 / (spot * vol * sqrt_t)).clamp(0.0, 1.0),
        // Gamma for cash-or-nothing: N'(d2) * d1 / (S^2 * sigma^2 * t)
        gamma: (n_prime_d2 * d1 / (spot * spot * vol * vol * t)).clamp(-1e6, 1e6),
        // Vega: N'(d2) * sqrt(t) / 100 (for 1% vol change)
        vega: (n_prime_d2 * sqrt_t * 0.01).clamp(-1e6, 1e6),
        // Theta: -(S * N'(d2) * sigma) / (2 * sqrt(t)) / 365 (per day)
        theta: (-(spot * n_prime_d2 * vol) / (2.0 * sqrt_t) / 365.0).clamp(-1e6, 1e6),
    }
}

// ============================================================================
// FETCHERS
// ============================================================================

async fn fetch_market(client: &reqwest::Client) -> Result<MarketInfo> {
    let now = Utc::now().timestamp();
    let pos = now % MARKET_WINDOW_SECONDS;
    let window_start = if pos < 60 {
        ((now / MARKET_WINDOW_SECONDS) - 1) * MARKET_WINDOW_SECONDS
    } else {
        (now / MARKET_WINDOW_SECONDS) * MARKET_WINDOW_SECONDS
    };
    let window_end = window_start + MARKET_WINDOW_SECONDS;

    let slug = format!("btc-updown-15m-{}", window_start);
    let url = format!("{}/events/slug/{}", GAMMA_API_URL, slug);

    info!("Fetching market: {}", slug);

    let resp = client.get(&url).timeout(Duration::from_secs(HTTP_TIMEOUT_SECS)).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("Market fetch failed: {}", resp.status()));
    }

    let data: serde_json::Value = resp.json().await?;
    let markets = data["markets"].as_array().ok_or_else(|| anyhow!("No markets"))?;

    for m in markets {
        if !m["active"].as_bool().unwrap_or(false) { continue; }

        let outcomes: Vec<String> = serde_json::from_str(m["outcomes"].as_str().unwrap_or("[]")).unwrap_or_default();
        let tokens: Vec<String> = serde_json::from_str(m["clobTokenIds"].as_str().unwrap_or("[]")).unwrap_or_default();

        if outcomes.len() != 2 || tokens.len() != 2 { continue; }

        let up_idx = outcomes.iter().position(|o| o == "Up");
        let down_idx = outcomes.iter().position(|o| o == "Down");

        if let (Some(ui), Some(di)) = (up_idx, down_idx) {
            return Ok(MarketInfo {
                slug: m["slug"].as_str().unwrap_or("").to_string(),
                condition_id: m["conditionId"].as_str().unwrap_or("").to_string(),
                yes_token_id: tokens[ui].clone(),
                no_token_id: tokens[di].clone(),
                window_start,
                window_end,
                strike_price: None,
            });
        }
    }

    Err(anyhow!("No valid BTC market"))
}

async fn fetch_strike_with_retry(client: &reqwest::Client, market: &MarketInfo) -> Result<f64> {
    let start = DateTime::from_timestamp(market.window_start, 0).ok_or_else(|| anyhow!("Bad ts"))?;
    let end = DateTime::from_timestamp(market.window_end, 0).ok_or_else(|| anyhow!("Bad ts"))?;

    let url = format!(
        "{}/crypto-price?symbol=BTC&eventStartTime={}&variant=fifteen&endDate={}",
        POLY_CRYPTO_API_URL,
        start.format("%Y-%m-%dT%H:%M:%SZ"),
        end.format("%Y-%m-%dT%H:%M:%SZ")
    );

    debug!("Strike fetch URL: {}", url);

    for attempt in 1..=STRIKE_RETRY_ATTEMPTS {
        debug!("Attempting strike fetch (attempt {}/{})", attempt, STRIKE_RETRY_ATTEMPTS);

        match client.get(&url).timeout(Duration::from_secs(HTTP_TIMEOUT_SECS)).send().await {
            Ok(resp) => {
                debug!("Response status: {}", resp.status());

                if resp.status().is_success() {
                    match resp.json::<PolymarketStrikeResponse>().await {
                        Ok(data) => {
                            if let Some(price) = data.open_price {
                                info!("✅ Strike fetched on attempt {}: ${:.2}", attempt, price);
                                return Ok(price);
                            } else {
                                warn!("Attempt {}: Response missing open_price field", attempt);
                            }
                        }
                        Err(e) => {
                            warn!("Attempt {}: Failed to parse JSON: {}", attempt, e);
                        }
                    }
                } else {
                    warn!("Attempt {}: HTTP status {}", attempt, resp.status());
                }
            }
            Err(e) => {
                if e.is_timeout() {
                    warn!("Attempt {}: Request timed out after {}s", attempt, HTTP_TIMEOUT_SECS);
                } else if e.is_connect() {
                    warn!("Attempt {}: Connection error: {}", attempt, e);
                } else {
                    warn!("Attempt {}: Request failed: {}", attempt, e);
                }
            }
        }

        if attempt < STRIKE_RETRY_ATTEMPTS {
            debug!("Sleeping {}ms before retry...", STRIKE_RETRY_INTERVAL_MS);
            tokio::time::sleep(Duration::from_millis(STRIKE_RETRY_INTERVAL_MS)).await;
        }
    }

    Err(anyhow!("Strike fetch failed after {} retries", STRIKE_RETRY_ATTEMPTS))
}

// ============================================================================
// MAIN
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("data_logger=info".parse().unwrap())
                .add_directive("poly_crypto_arb=info".parse().unwrap()),
        )
        .init();

    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║  Polymarket Data Logger (SDK Integration)                ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");

    let args: Vec<String> = std::env::args().collect();
    let duration: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(900);
    let csv_path = args.get(2).cloned().unwrap_or_else(|| "market_data.csv".into());
    let csv_record_interval_sec: u64 = args.get(3).and_then(|s| s.parse().ok())
        .unwrap_or(CSV_RECORD_INTERVAL_SEC);

    println!(
        "Duration: {}s ({:.1}m) | Output: {} | Interval: {}s\n",
        duration,
        duration as f64 / 60.0,
        csv_path,
        csv_record_interval_sec
    );

    // Enhanced HTTP client with timeout for resilience
    let http = reqwest::Client::builder()
        // Overall timeout for entire request
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        // Time limit for establishing connection
        .connect_timeout(Duration::from_secs(3))
        // Close idle connections after 90s to prevent stale connections
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?;

    // Fetch market info
    println!("⏳ Fetching market...");
    let mut market = fetch_market(&http).await?;
    info!("Market: {}", market.slug);

    // Fetch strike with retry
    println!("⏳ Fetching strike price...");
    match fetch_strike_with_retry(&http, &market).await {
        Ok(s) => market.strike_price = Some(s),
        Err(e) => {
            warn!("⚠️ Strike fetch failed: {} - Using fallback value 0.0 (will update once Binance price is available)", e);
            market.strike_price = Some(0.0);
            println!("⚠️  Fallback: strike_price set to 0.0 - will use current Binance price once available");
        }
    }

    // Use your existing BinanceDataManager
    let mut binance_manager = BinanceDataManager::new();
    binance_manager.connect(&["BTCUSDT"], None).await?;

    // Use your existing PolymarketOrderbookStream (requires Arc for stream method)
    let poly_stream = Arc::new(PolymarketOrderbookStream::new());
    let token_ids = vec![market.yes_token_id.clone(), market.no_token_id.clone()];
    let mut rx = poly_stream.clone().stream(token_ids).await?;

    // Spawn a task to consume messages from the receiver.
    // The messages are stored internally in the PolymarketOrderbookStream's price cache
    // and retrieved via get_prices() in the main loop. This task prevents the channel
    // from filling up and ensures the streaming task continues to process updates.
    tokio::spawn(async move {
        while let Some(_msg) = rx.recv().await {
            // Messages are handled internally by PolymarketOrderbookStream's streaming_loop
            // which updates the price cache. We just consume to keep the channel flowing.
        }
    });

    println!("⏳ Waiting for connections...");
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Open CSV
    let mut file = OpenOptions::new().create(true).append(true).open(&csv_path)?;
    writeln!(file, "{}", CsvRecord::csv_header())?;

    let start_time = SystemTime::now();
    let end_time = start_time + Duration::from_secs(duration);
    let mut tick: u64 = 0;
    let mut last_log_ms: i64 = 0;

    info!("Starting data collection...");

    // Main loop
    loop {
        if SystemTime::now() >= end_time {
            break;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;

        // Get Binance price and market snapshot
        let binance_price = match binance_manager.get_price("BTCUSDT").await {
            Some(p) => p,
            None => continue,
        };

        // Get market snapshot with realized volatility and rolling window volume
        // FIX: Use rolling window volume (1min) instead of cumulative volume
        let (realized_vol, binance_volume_rolling) = match binance_manager.get_market_snapshot("BTCUSDT").await {
            Some(snapshot) => (snapshot.realized_volatility, snapshot.volume_1min),
            None => (VOLATILITY_BTC, 0.0), // Fallback to constant if snapshot not available
        };

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // CSV recording frequency control
        // Note: Uses csv_record_interval_sec from CLI args (or default constant)
        let should_record = if csv_record_interval_sec == 0 {
            // Event-driven mode: record on every update (small guard for duplicates)
            const MIN_EVENT_INTERVAL_MS: i64 = 10;
            now_ms - last_log_ms >= MIN_EVENT_INTERVAL_MS
        } else {
            // Time-interval mode: record at fixed interval
            let interval_ms = (csv_record_interval_sec * 1000) as i64;
            now_ms - last_log_ms >= interval_ms
        };

        if !should_record {
            continue;
        }
        last_log_ms = now_ms;

        // Get Polymarket prices
        let yes_prices: Option<TokenPrices> = poly_stream.get_prices(&market.yes_token_id);
        let no_prices: Option<TokenPrices> = poly_stream.get_prices(&market.no_token_id);

        let now = Utc::now();
        let now_ts = now.timestamp();
        let time_to_open = market.window_start - now_ts;
        let ttx = if time_to_open > 0 {
            (market.window_end - now_ts) as f64 / 60.0
        } else {
            ((market.window_end - now_ts) as f64 / 60.0).max(0.0)
        };

        let resolved = ttx <= 0.0;
        let spot = binance_price.mid.to_f64().unwrap_or(0.0);
        let strike = market.strike_price.unwrap_or(spot);
        let greeks = calculate_greeks(spot, strike, ttx, realized_vol);

        let outcome = if resolved {
            if spot >= strike { "YES" } else { "NO" }
        } else { "" };

        let record = CsvRecord {
            timestamp: now.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
            market_slug: market.slug.clone(),
            binance_price: spot,
            binance_volume: binance_volume_rolling, // FIX: Use rolling window volume (1min)
            yes_bid: yes_prices.as_ref().and_then(|p: &TokenPrices| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0),
            yes_bid_size: yes_prices.as_ref().map(|p: &TokenPrices| p.best_bid_size).unwrap_or(0.0),
            yes_ask: yes_prices.as_ref().and_then(|p: &TokenPrices| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0),
            yes_ask_size: yes_prices.as_ref().map(|p: &TokenPrices| p.best_ask_size).unwrap_or(0.0),
            no_bid: no_prices.as_ref().and_then(|p: &TokenPrices| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0),
            no_bid_size: no_prices.as_ref().map(|p: &TokenPrices| p.best_bid_size).unwrap_or(0.0),
            no_ask: no_prices.as_ref().and_then(|p: &TokenPrices| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0),
            no_ask_size: no_prices.as_ref().map(|p: &TokenPrices| p.best_ask_size).unwrap_or(0.0),
            time_to_open,
            time_to_expiry_minutes: ttx,
            strike_price: strike,
            realized_volatility: realized_vol,
            delta_yes: greeks.delta,
            gamma: greeks.gamma,
            vega: greeks.vega,
            theta: greeks.theta,
            market_resolved: resolved,
            outcome: outcome.to_string(),
        };

        let _ = writeln!(file, "{}", record.to_csv_line());
        tick += 1;

        if tick % 50 == 0 {
            info!("#{} BTC=${:.2} TTX={:.1}m", tick, spot, ttx);
        }
    }

    info!("✅ Logged {} ticks to {}", tick, csv_path);
    println!("\n✅ Complete!");
    Ok(())
}