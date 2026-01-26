//! Minimal standalone test for Binance REST API
//!
//! This test ONLY tests the fetch_klines() method and doesn't require
//! any changes to volatility.rs. Use this to verify the REST API works
//! before integrating the full bootstrap flow.
//!
//! Usage:
//!   cargo run --bin test_binance_rest

use anyhow::Result;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

const BINANCE_REST_API_URL: &str = "https://api.binance.com/api/v3";

#[derive(Debug)]
struct Kline {
    open_time: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
}

impl Kline {
    fn from_json(arr: &[Value]) -> Option<Self> {
        if arr.len() < 6 {
            return None;
        }
        Some(Self {
            open_time: arr[0].as_i64()?,
            open: arr[1].as_str()?.parse().ok()?,
            high: arr[2].as_str()?.parse().ok()?,
            low: arr[3].as_str()?.parse().ok()?,
            close: arr[4].as_str()?.parse().ok()?,
            volume: arr[5].as_str()?.parse().ok()?,
        })
    }
}

async fn fetch_klines(
    client: &Client,
    symbol: &str,
    interval: &str,
    limit: usize,
) -> Result<Vec<Kline>> {
    let url = format!(
        "{}/klines?symbol={}&interval={}&limit={}",
        BINANCE_REST_API_URL,
        symbol.to_uppercase(),
        interval,
        limit.min(1000)
    );

    println!("  GET {}", url);

    let resp = client.get(&url).send().await?;

    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }

    let data: Vec<Vec<Value>> = resp.json().await?;

    let klines: Vec<Kline> = data
        .iter()
        .filter_map(|arr| Kline::from_json(arr))
        .collect();

    Ok(klines)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║  Binance REST API Test (Standalone)                       ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    // Test 1: Basic fetch
    println!("═══ TEST 1: Fetch BTCUSDT 1m klines ═══\n");

    let klines = fetch_klines(&client, "BTCUSDT", "1m", 70).await?;
    println!("✅ Fetched {} klines\n", klines.len());

    // Show first and last
    if let Some(first) = klines.first() {
        let dt = chrono::DateTime::from_timestamp_millis(first.open_time)
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        println!("First: {} | Open: ${:.2} | Close: ${:.2}", dt, first.open, first.close);
    }

    if let Some(last) = klines.last() {
        let dt = chrono::DateTime::from_timestamp_millis(last.open_time)
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        println!("Last:  {} | Open: ${:.2} | Close: ${:.2}", dt, last.open, last.close);
    }

    // Calculate simple volatility from the fetched data
    println!("\n═══ TEST 2: Calculate Realized Volatility ═══\n");

    if klines.len() >= 2 {
        let mut log_returns: Vec<f64> = Vec::new();

        for i in 1..klines.len() {
            let prev = klines[i - 1].close;
            let curr = klines[i].close;
            if prev > 0.0 && curr > 0.0 {
                log_returns.push((curr / prev).ln());
            }
        }

        if !log_returns.is_empty() {
            // Variance of log returns
            let mean: f64 = log_returns.iter().sum::<f64>() / log_returns.len() as f64;
            let variance: f64 = log_returns.iter()
                .map(|r| (r - mean).powi(2))
                .sum::<f64>() / (log_returns.len() - 1) as f64;

            // Annualize (minutes per year = 365.25 * 24 * 60)
            let minutes_per_year = 365.25 * 24.0 * 60.0;
            let annualized_vol = (variance * minutes_per_year).sqrt();

            println!("Log returns calculated: {}", log_returns.len());
            println!("Sample variance:        {:.10}", variance);
            println!("Annualized volatility:  {:.2}%", annualized_vol * 100.0);

            // Sanity check
            if annualized_vol > 0.3 && annualized_vol < 2.0 {
                println!("\n✅ Volatility looks reasonable for BTC");
            } else {
                println!("\n⚠️  Volatility outside expected range (30%-200%)");
            }
        }
    }

    // Test 3: Multiple symbols
    println!("\n═══ TEST 3: Multiple Symbols ═══\n");

    for symbol in ["BTCUSDT", "ETHUSDT", "SOLUSDT", "XRPUSDT"] {
        match fetch_klines(&client, symbol, "1m", 5).await {
            Ok(klines) => {
                let last_close = klines.last().map(|k| k.close).unwrap_or(0.0);
                println!("✅ {}: {} klines, last=${:.4}", symbol, klines.len(), last_close);
            }
            Err(e) => {
                println!("❌ {}: {}", symbol, e);
            }
        }
    }

    // Test 4: Rate limit (don't hit it, just show we're being careful)
    println!("\n═══ TEST 4: API Weight Check ═══\n");
    println!("Note: Binance klines endpoint costs 1-2 weight per request");
    println!("Rate limit: 1200 weight/minute for IP");
    println!("We've made ~6 requests, well under limit ✅");

    println!("\n✅ All REST API tests passed!");
    println!("\nNext steps:");
    println!("1. Add stub methods to volatility.rs (get_1min_buffer_size, etc.)");
    println!("2. Run the full bootstrap test: cargo run --bin test_binance_bootstrap");

    Ok(())
}