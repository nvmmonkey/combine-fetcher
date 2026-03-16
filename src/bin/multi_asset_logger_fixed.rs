// multi_asset_logger_fixed.rs - Multi-Asset Multi-Timeframe Logger (FIXED BINANCE EVENT TIME)
//! Logs multiple assets (BTC, ETH, SOL, XRP) across 5m and 15m markets simultaneously.
//! Each market gets its own CSV with hourly rotation. Compact console output.
//!
//! IDENTICAL to multi_asset_logger.rs except uses binance_fixed module
//! which preserves Binance exchange-level event_time for RTDS edge detection.
//! Run both loggers side-by-side to compare RTDS lag/stale data quality.
//!
//! Usage: cargo run --bin multi_asset_logger_fixed -- [--config path.yml] [duration_secs] [csv_dir] [record_interval_sec]
//!
//! Without --config, defaults to BTC 5m only.

use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

use combine_fetcher::api::binance_fixed::BinanceDataManager;
use combine_fetcher::api::gamma;
use combine_fetcher::api::market_events::MarketEventStream;
use combine_fetcher::api::polymarket::PolymarketOrderbookStream;
use combine_fetcher::api::rtds_tracker::RtdsSnapshot;
use combine_fetcher::api::rtds_tracker_multi::MultiAssetRtdsTracker;
use combine_fetcher::api::strike;
use combine_fetcher::{CSV_RECORD_INTERVAL_SEC, HTTP_TIMEOUT_SECS, MINUTES_PER_YEAR};

// ============================================================================
// CONFIGURATION — loaded from YAML config file (--config flag)
// ============================================================================

#[derive(Debug, Deserialize)]
struct LoggerConfig {
    markets: Vec<MarketConfigEntry>,
}

#[derive(Debug, Deserialize)]
struct MarketConfigEntry {
    asset: String,
    timeframe: String,
    binance_symbol: String,
}

struct MarketConfig {
    asset: String,
    timeframe_secs: i64,
    binance_symbol: String,
}

impl MarketConfig {
    fn from_entry(entry: &MarketConfigEntry) -> Result<Self> {
        let timeframe_secs = match entry.timeframe.as_str() {
            "5m" => 300i64,
            "15m" => 900i64,
            other => anyhow::bail!("unsupported timeframe '{}' (use 5m or 15m)", other),
        };
        Ok(Self {
            asset: entry.asset.to_lowercase(),
            timeframe_secs,
            binance_symbol: entry.binance_symbol.clone(),
        })
    }
}

fn load_config(path: &str) -> Result<Vec<MarketConfig>> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read config '{}': {}", path, e))?;
    let cfg: LoggerConfig = serde_yaml::from_str(&contents)
        .map_err(|e| anyhow::anyhow!("failed to parse config '{}': {}", path, e))?;
    if cfg.markets.is_empty() {
        anyhow::bail!("config '{}' has no markets defined", path);
    }
    cfg.markets.iter().map(MarketConfig::from_entry).collect()
}

/// Default: single BTC 5m market (matches previous hardcoded behavior)
fn default_markets() -> Vec<MarketConfig> {
    vec![MarketConfig {
        asset: "btc".into(),
        timeframe_secs: 300,
        binance_symbol: "BTCUSDT".into(),
    }]
}

// ============================================================================
// CSV RECORD (same schema as hourly logger for compatibility)
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
    pub timestamp_unix_ms: i64,
    pub binance_update_ms: i64,
    pub poly_yes_update_ms: i64,
    pub poly_no_update_ms: i64,
    pub latency_binance_poly_ms: i64,
    pub binance_bid: f64,
    pub binance_ask: f64,
    pub binance_spread_bps: f64,
    pub vol_1sec: f64,
    pub vol_5sec: f64,
    pub vol_13sec: f64,
    pub vol_34sec: f64,
    pub vol_89sec: f64,
    pub volatility_regime: String,
    pub volume_1sec: f64,
    pub volume_spike: bool,
    pub signal_quality: String,
    pub trade_confidence: f64,
    pub vol_data_source: String,
    pub yes_spread_bps: f64,
    pub no_spread_bps: f64,
    pub rtds_binance_price: f64,
    pub rtds_chainlink_price: f64,
    pub rtds_binance_dev_bps: f64,
    pub rtds_chainlink_dev_bps: f64,
    pub rtds_price_advantage_bps: f64,
    pub rtds_chainlink_binance_spread_bps: f64,
    pub rtds_binance_lag_ms: i64,
    pub rtds_chainlink_lag_ms: i64,
    pub rtds_binance_stale_ms: i64,
    pub rtds_chainlink_stale_ms: i64,

    // === ORDER BOOK DEPTH (from Polymarket BOOK_STREAM) ===
    pub yes_bid_depth_3: f64,
    pub yes_ask_depth_3: f64,
    pub no_bid_depth_3: f64,
    pub no_ask_depth_3: f64,
    pub yes_bid_depth_all: f64,
    pub yes_ask_depth_all: f64,
    pub no_bid_depth_all: f64,
    pub no_ask_depth_all: f64,
    pub yes_bid_levels: u16,
    pub yes_ask_levels: u16,
    pub no_bid_levels: u16,
    pub no_ask_levels: u16,
    pub yes_book_imbalance: f64,
    pub no_book_imbalance: f64,
    pub combined_obi: f64,

    // === TRADE FLOW (from Polymarket TRADE_STREAM) ===
    pub yes_last_trade_price: f64,
    pub yes_last_trade_size: f64,
    pub yes_last_trade_side: String,
    pub no_last_trade_price: f64,
    pub no_last_trade_size: f64,
    pub no_last_trade_side: String,
    pub trade_flow_imbalance: f64,

    // === BINANCE L2 DEPTH (from Binance depth20@100ms stream) ===
    pub binance_bid_depth_3: f64,
    pub binance_ask_depth_3: f64,
    pub binance_bid_depth_all: f64,
    pub binance_ask_depth_all: f64,
    pub binance_bid_levels: u16,
    pub binance_ask_levels: u16,
    pub binance_book_imbalance: f64,
    pub binance_poly_obi_divergence: f64,
}

impl CsvRecord {
    pub fn csv_header() -> &'static str {
        "timestamp,market_slug,binance_price,binance_volume,\
        yes_bid,yes_bid_size,yes_ask,yes_ask_size,\
        no_bid,no_bid_size,no_ask,no_ask_size,\
        time_to_open,time_to_expiry_minutes,strike_price,realized_volatility,\
        delta_yes,gamma,vega,theta,market_resolved,outcome,\
        timestamp_unix_ms,binance_update_ms,poly_yes_update_ms,poly_no_update_ms,latency_binance_poly_ms,\
        binance_bid,binance_ask,binance_spread_bps,\
        vol_1sec,vol_5sec,vol_13sec,vol_34sec,vol_89sec,volatility_regime,\
        volume_1sec,volume_spike,signal_quality,trade_confidence,vol_data_source,\
        yes_spread_bps,no_spread_bps,\
        rtds_binance_price,rtds_chainlink_price,\
        rtds_binance_dev_bps,rtds_chainlink_dev_bps,\
        rtds_price_advantage_bps,rtds_chainlink_binance_spread_bps,\
        rtds_binance_lag_ms,rtds_chainlink_lag_ms,\
        rtds_binance_stale_ms,rtds_chainlink_stale_ms,\
        yes_bid_depth_3,yes_ask_depth_3,no_bid_depth_3,no_ask_depth_3,\
        yes_bid_depth_all,yes_ask_depth_all,no_bid_depth_all,no_ask_depth_all,\
        yes_bid_levels,yes_ask_levels,no_bid_levels,no_ask_levels,\
        yes_book_imbalance,no_book_imbalance,combined_obi,\
        yes_last_trade_price,yes_last_trade_size,yes_last_trade_side,\
        no_last_trade_price,no_last_trade_size,no_last_trade_side,\
        trade_flow_imbalance,\
        binance_bid_depth_3,binance_ask_depth_3,\
        binance_bid_depth_all,binance_ask_depth_all,\
        binance_bid_levels,binance_ask_levels,\
        binance_book_imbalance,binance_poly_obi_divergence"
    }

    pub fn to_csv_line(&self) -> String {
        format!(
            "{},{},{:.2},{:.2},\
            {:.4},{:.0},{:.4},{:.0},\
            {:.4},{:.0},{:.4},{:.0},\
            {},{:.2},{:.2},{:.6},\
            {:.8},{:.8},{:.8},{:.8},{},{},\
            {},{},{},{},{},\
            {:.2},{:.2},{:.4},\
            {:.6},{:.6},{:.6},{:.6},{:.6},{},\
            {:.2},{},{},{:.4},{},\
            {:.4},{:.4},\
            {:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{},{},\
            {:.0},{:.0},{:.0},{:.0},\
            {:.0},{:.0},{:.0},{:.0},\
            {},{},{},{},\
            {:.4},{:.4},{:.4},\
            {:.4},{:.0},{},{:.4},{:.0},{},{:.4},\
            {:.4},{:.4},{:.4},{:.4},{},{},{:.4},{:.4}",
            self.timestamp, self.market_slug,
            self.binance_price, self.binance_volume,
            self.yes_bid, self.yes_bid_size, self.yes_ask, self.yes_ask_size,
            self.no_bid, self.no_bid_size, self.no_ask, self.no_ask_size,
            self.time_to_open, self.time_to_expiry_minutes,
            self.strike_price, self.realized_volatility,
            self.delta_yes, self.gamma, self.vega, self.theta,
            self.market_resolved, self.outcome,
            self.timestamp_unix_ms, self.binance_update_ms,
            self.poly_yes_update_ms, self.poly_no_update_ms,
            self.latency_binance_poly_ms,
            self.binance_bid, self.binance_ask, self.binance_spread_bps,
            self.vol_1sec, self.vol_5sec, self.vol_13sec,
            self.vol_34sec, self.vol_89sec, self.volatility_regime,
            self.volume_1sec, self.volume_spike,
            self.signal_quality, self.trade_confidence, self.vol_data_source,
            self.yes_spread_bps, self.no_spread_bps,
            self.rtds_binance_price, self.rtds_chainlink_price,
            self.rtds_binance_dev_bps, self.rtds_chainlink_dev_bps,
            self.rtds_price_advantage_bps, self.rtds_chainlink_binance_spread_bps,
            self.rtds_binance_lag_ms, self.rtds_chainlink_lag_ms,
            self.rtds_binance_stale_ms, self.rtds_chainlink_stale_ms,
            // Order Book Depth columns
            self.yes_bid_depth_3,
            self.yes_ask_depth_3,
            self.no_bid_depth_3,
            self.no_ask_depth_3,
            self.yes_bid_depth_all,
            self.yes_ask_depth_all,
            self.no_bid_depth_all,
            self.no_ask_depth_all,
            self.yes_bid_levels,
            self.yes_ask_levels,
            self.no_bid_levels,
            self.no_ask_levels,
            self.yes_book_imbalance,
            self.no_book_imbalance,
            self.combined_obi,
            // Trade Flow columns
            self.yes_last_trade_price,
            self.yes_last_trade_size,
            self.yes_last_trade_side,
            self.no_last_trade_price,
            self.no_last_trade_size,
            self.no_last_trade_side,
            self.trade_flow_imbalance,
            // Binance L2 Depth columns
            self.binance_bid_depth_3,
            self.binance_ask_depth_3,
            self.binance_bid_depth_all,
            self.binance_ask_depth_all,
            self.binance_bid_levels,
            self.binance_ask_levels,
            self.binance_book_imbalance,
            self.binance_poly_obi_divergence,
        )
    }
}

// ============================================================================
// HOURLY CSV WRITER (per-market)
// ============================================================================

fn generate_hourly_filename(base_stem: &str, hour_time: DateTime<Utc>) -> String {
    let timestamp = hour_time.format("%Y-%m-%d_%H");
    format!("{}_{}.csv", base_stem, timestamp)
}

fn open_csv_file(path: &str) -> Result<BufWriter<std::fs::File>> {
    let lock_path = format!("{}.lock", path);
    let my_pid = std::process::id();
    if let Ok(existing_pid) = std::fs::read_to_string(&lock_path) {
        let existing_pid = existing_pid.trim();
        let pid_alive = existing_pid
            .parse::<u32>()
            .ok()
            .map(|pid| std::path::Path::new(&format!("/proc/{}", pid)).exists())
            .unwrap_or(false);
        if pid_alive && existing_pid != my_pid.to_string() {
            warn!("Lock file {} exists with PID {} (alive). Proceeding with append.", lock_path, existing_pid);
        }
    }
    let _ = std::fs::write(&lock_path, my_pid.to_string());

    let file_exists_with_data = std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false);
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut writer = BufWriter::with_capacity(64 * 1024, file);

    if !file_exists_with_data {
        writeln!(writer, "{}", CsvRecord::csv_header())?;
        writer.flush()?;
    }
    Ok(writer)
}

struct MarketCsvWriter {
    base_stem: String,
    current_file: Option<BufWriter<std::fs::File>>,
    current_file_path: String,
    current_hour: u32,
    records_since_flush: u32,
    last_flush_ms: i64,
}

impl MarketCsvWriter {
    fn new(base_stem: String) -> Self {
        Self {
            base_stem,
            current_file: None,
            current_file_path: String::new(),
            current_hour: u32::MAX,
            records_since_flush: 0,
            last_flush_ms: 0,
        }
    }

    fn check_rotation(&mut self, now: &DateTime<Utc>) -> Result<bool> {
        let hour = now.hour();
        if self.current_hour == u32::MAX || hour != self.current_hour {
            self.rotate(now)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn rotate(&mut self, now: &DateTime<Utc>) -> Result<()> {
        if let Some(mut file) = self.current_file.take() {
            file.flush()?;
            let _ = std::fs::remove_file(format!("{}.lock", self.current_file_path));
        }
        let new_path = generate_hourly_filename(&self.base_stem, *now);
        self.current_file_path = new_path.clone();
        self.current_hour = now.hour();
        self.records_since_flush = 0;
        self.last_flush_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        self.current_file = Some(open_csv_file(&new_path)?);
        info!("[CSV] New file: {}", new_path);
        Ok(())
    }

    fn write_record(&mut self, record: &CsvRecord) -> Result<()> {
        if let Some(ref mut file) = self.current_file {
            let mut line = record.to_csv_line();
            line.push('\n');
            file.write_all(line.as_bytes())?;
            self.records_since_flush += 1;
            let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
            if self.records_since_flush >= 100 || (now_ms - self.last_flush_ms) >= 1000 {
                file.flush()?;
                self.records_since_flush = 0;
                self.last_flush_ms = now_ms;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if let Some(mut file) = self.current_file.take() {
            file.flush()?;
            let _ = std::fs::remove_file(format!("{}.lock", self.current_file_path));
        }
        Ok(())
    }
}

// ============================================================================
// HELPERS
// ============================================================================

fn calculate_spread_bps(bid: f64, ask: f64) -> f64 {
    if bid <= 0.0 || ask <= bid { return 0.0; }
    let mid = (bid + ask) / 2.0;
    if mid <= 0.0 { return 0.0; }
    ((ask - bid) / mid) * 10000.0
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Greeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub theta: f64,
}

fn calculate_greeks(spot: f64, strike: f64, ttx_min: f64, vol: f64) -> Greeks {
    if ttx_min <= 0.01 || vol <= 0.0 || spot <= 0.0 || strike <= 0.0
        || vol.is_nan() || spot.is_nan() || strike.is_nan() || ttx_min.is_nan()
    {
        return Greeks { delta: if spot >= strike { 1.0 } else { 0.0 }, ..Default::default() };
    }
    let t = ttx_min / MINUTES_PER_YEAR;
    let sqrt_t = t.sqrt();
    let d1 = ((spot / strike).ln() + (vol * vol / 2.0) * t) / (vol * sqrt_t);
    let d2 = ((spot / strike).ln() - (vol * vol / 2.0) * t) / (vol * sqrt_t);
    let norm_pdf = |x: f64| -> f64 { (-x * x / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt() };
    let n_prime_d2 = norm_pdf(d2);
    let delta = (n_prime_d2 / (spot * vol * sqrt_t)).clamp(0.0, 1.0);
    let gamma_val = (n_prime_d2 * d1 / (spot * spot * vol * vol * t)).clamp(-1e6, 1e6);
    let vega = (n_prime_d2 * sqrt_t * 0.01).clamp(-1e6, 1e6);
    let theta = (-(spot * n_prime_d2 * vol) / (2.0 * sqrt_t) / 365.0).clamp(-1e6, 1e6);
    Greeks {
        delta: if delta.is_finite() { delta } else { 0.0 },
        gamma: if gamma_val.is_finite() { gamma_val } else { 0.0 },
        vega: if vega.is_finite() { vega } else { 0.0 },
        theta: if theta.is_finite() { theta } else { 0.0 },
    }
}

fn truncate_id(id: &str, first: usize, last: usize) -> String {
    let len = id.len();
    if len <= first + last { return id.to_string(); }
    format!("{}...{}", &id[..first], &id[len - last..])
}

fn sanitize(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

fn timeframe_label(secs: i64) -> &'static str {
    if secs == 300 { "5m" } else { "15m" }
}

fn market_tag(asset: &str, secs: i64) -> String {
    format!("{}-{}", asset.to_uppercase(), timeframe_label(secs))
}

// ============================================================================
// PER-MARKET RUNTIME STATE
// ============================================================================

struct MarketState {
    config_idx: usize,
    market: gamma::MarketInfo,
    csv_writer: MarketCsvWriter,
    gamma_outcome: Arc<Mutex<Option<String>>>,
    gamma_fetch_in_progress: Arc<AtomicBool>,
    /// Generation counter: increments on every market rotation.
    /// Spawned Gamma API tasks capture the current generation and only
    /// write their result if the generation hasn't changed (prevents stale
    /// outcome from a previous market leaking into the new one).
    gamma_generation: Arc<AtomicU64>,
    consecutive_fetch_failures: u32,
    last_fetch_attempt_ms: i64,
    gamma_last_outcome_fetch_ms: i64,
    resolution_grace_start_ms: i64,
    tag: String,
}

// ============================================================================
// MAIN
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("multi_asset_logger=info".parse().unwrap())
                .add_directive("combine_fetcher=info".parse().unwrap()),
        )
        .init();

    // --- Parse CLI args ---
    // Usage: multi_asset_logger_fixed [--config path.yml] [duration_secs] [csv_dir] [record_interval_sec]
    let args: Vec<String> = std::env::args().collect();
    let (config_path, positional_start) = {
        if let Some(pos) = args.iter().position(|a| a == "--config") {
            match args.get(pos + 1) {
                Some(p) => (Some(p.clone()), pos + 2),
                None => {
                    eprintln!("--config requires a path argument");
                    std::process::exit(1);
                }
            }
        } else {
            (None, 1)
        }
    };
    let positional: Vec<&String> = args[positional_start..].iter()
        .filter(|a| *a != "--config")
        .collect();

    let markets: Vec<MarketConfig> = match &config_path {
        Some(path) => load_config(path)?,
        None => default_markets(),
    };

    let duration: u64 = positional.first().and_then(|s| s.parse().ok()).unwrap_or(28800);
    let csv_dir = positional.get(1).map(|s| s.as_str()).unwrap_or(".").to_string();
    let csv_record_interval_sec: u64 = positional.get(2).and_then(|s| s.parse().ok()).unwrap_or(CSV_RECORD_INTERVAL_SEC);

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║  Multi-Asset Logger FIXED (Hourly CSV | Binance event_time) ║");
    println!("║  {} market(s) | Config: {:<37} ║", markets.len(),
        config_path.as_deref().unwrap_or("(default)"));
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    println!("Duration: {}s ({:.1}h) | CSV dir: {} | Interval: {}s", duration, duration as f64 / 3600.0, csv_dir, csv_record_interval_sec);
    println!("Markets:");
    for cfg in &markets {
        println!("  - {}-{} ({})", cfg.asset.to_uppercase(), timeframe_label(cfg.timeframe_secs), cfg.binance_symbol);
    }
    println!();

    // Ensure CSV directory exists
    if csv_dir != "." {
        std::fs::create_dir_all(&csv_dir)?;
    }

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(3))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?;

    // --- Fetch all initial markets ---
    println!("Fetching initial markets...");
    let mut market_states: Vec<MarketState> = Vec::with_capacity(markets.len());

    for (i, cfg) in markets.iter().enumerate() {
        let tag = market_tag(&cfg.asset, cfg.timeframe_secs);
        print!("  [{}] ...", tag);
        match gamma::fetch_current_market_for(&http, &cfg.asset, cfg.timeframe_secs).await {
            Ok(m) => {
                println!(" {} YES:{}... NO:{}...", m.unique_id,
                    truncate_id(&m.yes_token_id, 8, 0),
                    truncate_id(&m.no_token_id, 8, 0));
                let tf = timeframe_label(cfg.timeframe_secs);
                let market_dir = if csv_dir == "." {
                    format!("{}/{}", cfg.asset, tf)
                } else {
                    format!("{}/{}/{}", csv_dir, cfg.asset, tf)
                };
                std::fs::create_dir_all(&market_dir)?;
                let base_stem = format!("{}/{}_{}_data", market_dir, cfg.asset, tf);
                market_states.push(MarketState {
                    config_idx: i,
                    market: m,
                    csv_writer: MarketCsvWriter::new(base_stem),
                    gamma_outcome: Arc::new(Mutex::new(None)),
                    gamma_fetch_in_progress: Arc::new(AtomicBool::new(false)),
                    gamma_generation: Arc::new(AtomicU64::new(0)),
                    consecutive_fetch_failures: 0,
                    last_fetch_attempt_ms: 0,
                    gamma_last_outcome_fetch_ms: 0,
                    resolution_grace_start_ms: 0,
                    tag,
                });
            }
            Err(e) => {
                println!(" FAILED: {}", e);
                warn!("[{}] Skipping — market fetch failed: {}", tag, e);
            }
        }
    }

    if market_states.is_empty() {
        error!("No markets could be fetched. Exiting.");
        return Err(anyhow::anyhow!("No markets available"));
    }
    println!("Fetched {}/{} markets\n", market_states.len(), markets.len());

    // --- Collect unique Binance symbols ---
    let unique_symbols: Vec<String> = {
        let mut seen = HashSet::new();
        market_states.iter()
            .filter_map(|ms| {
                let sym = &markets[ms.config_idx].binance_symbol;
                if seen.insert(sym.clone()) { Some(sym.clone()) } else { None }
            })
            .collect()
    };
    let symbols_str: Vec<&str> = unique_symbols.iter().map(|s| s.as_str()).collect();

    // --- Setup Binance ---
    let mut binance_manager = BinanceDataManager::new();
    binance_manager.connect(&symbols_str, None).await?;
    println!("Binance connected: {:?}", unique_symbols);
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Bootstrap volatility for each symbol
    for sym in &unique_symbols {
        print!("  Bootstrapping {} ...", sym);
        match binance_manager.bootstrap_volatility(sym, 70).await {
            Ok(count) => println!(" {} candles", count),
            Err(e) => println!(" failed: {}", e),
        }
    }

    // --- Fetch strikes for all markets (infinite retry, matches reference) ---
    println!("\nFetching strike prices (will retry indefinitely)...");
    for ms in &mut market_states {
        print!("  [{}] ...", ms.tag);
        let s = strike::fetch_strike_with_retry(&http, &ms.market, None).await?;
        ms.market.strike_price = Some(s);
        ms.market.strike_fetched = true;
        println!(" ${:.2}", s);
    }

    // --- Setup Polymarket ---
    let poly_stream = Arc::new(PolymarketOrderbookStream::new());
    let all_token_ids: Vec<String> = market_states.iter()
        .flat_map(|ms| vec![ms.market.yes_token_id.clone(), ms.market.no_token_id.clone()])
        .collect();

    let mut rx = poly_stream.clone().stream(all_token_ids.clone()).await?;
    tokio::spawn(async move {
        while let Some(_update) = rx.recv().await {}
    });
    println!("Polymarket connected ({} tokens)", market_states.len() * 2);
    tokio::time::sleep(Duration::from_secs(2)).await;

    // --- Setup RTDS (only assets from config) ---
    let unique_assets: Vec<String> = {
        let mut seen = HashSet::new();
        markets.iter()
            .filter_map(|c| if seen.insert(c.asset.clone()) { Some(c.asset.clone()) } else { None })
            .collect()
    };
    let asset_refs: Vec<&str> = unique_assets.iter().map(|s| s.as_str()).collect();
    let mut multi_rtds = MultiAssetRtdsTracker::new(&asset_refs);
    multi_rtds.start();
    println!("RTDS tracker connected ({})", unique_assets.iter().map(|a| a.to_uppercase()).collect::<Vec<_>>().join("/"));
    tokio::time::sleep(Duration::from_secs(2)).await;

    // --- Setup Market Event Stream (WS new_market + market_resolved) ---
    let market_events = MarketEventStream::new();
    market_events.start(all_token_ids.clone());
    println!("Market event stream connected ({} tokens)", all_token_ids.len());

    // --- Initialize CSV files ---
    let now = Utc::now();
    for ms in &mut market_states {
        ms.csv_writer.check_rotation(&now)?;
    }
    println!("CSV files created\n");

    // --- Signal handling ---
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("Received SIGINT, shutting down...");
        shutdown_signal.store(true, Ordering::SeqCst);
    });

    let start_time = SystemTime::now();
    let end_time = start_time + Duration::from_secs(duration);
    let mut tick: u64 = 0;
    let mut last_log_ms: i64 = 0;

    const MAX_CONSECUTIVE_FETCH_FAILURES: u32 = 10;
    const FETCH_RETRY_COOLDOWN_MS: i64 = 5000;
    const RESOLUTION_GRACE_SECS: i64 = 30;
    const RESOLUTION_GRACE_MS: i64 = RESOLUTION_GRACE_SECS * 1000;

    info!("Starting multi-asset data collection...\n");

    // ========================================================================
    // MAIN LOOP
    // ========================================================================

    loop {
        if SystemTime::now() >= end_time || shutdown.load(Ordering::SeqCst) {
            break;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;

        let now = Utc::now();
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;

        // Hourly CSV rotation
        for ms in &mut market_states {
            let _ = ms.csv_writer.check_rotation(&now);
        }

        // Frequency control
        let should_record = if csv_record_interval_sec == 0 {
            now_ms - last_log_ms >= 10
        } else {
            now_ms - last_log_ms >= (csv_record_interval_sec * 1000) as i64
        };
        if !should_record { continue; }
        last_log_ms = now_ms;

        let now_ts = now.timestamp();

        // Cache Binance snapshots per symbol (fetch once)
        let mut binance_snapshots: HashMap<&str, _> = HashMap::new();
        let mut binance_bid_asks: HashMap<&str, (f64, f64)> = HashMap::new();
        for sym in &unique_symbols {
            if let Some(snap) = binance_manager.get_market_snapshot(sym).await {
                let ba = binance_manager.get_bid_ask(sym).await.unwrap_or((snap.spot_price, snap.spot_price));
                binance_bid_asks.insert(sym.as_str(), ba);
                binance_snapshots.insert(sym.as_str(), snap);
            }
        }

        // Cache Binance depth data per symbol (fetch once)
        let mut binance_depths: HashMap<&str, _> = HashMap::new();
        for sym in &unique_symbols {
            if let Some(depth) = binance_manager.get_depth_data(sym).await {
                binance_depths.insert(sym.as_str(), depth);
            }
        }

        // RTDS snapshots per asset (pre-computed once, shared across same-asset markets)
        let mut rtds_per_asset: HashMap<&str, RtdsSnapshot> = HashMap::new();
        for cfg in &markets {
            if rtds_per_asset.contains_key(cfg.asset.as_str()) { continue; }
            if let Some(snap) = binance_snapshots.get(cfg.binance_symbol.as_str()) {
                rtds_per_asset.insert(
                    &cfg.asset,
                    multi_rtds.get_snapshot(&cfg.asset, snap.spot_price, snap.timestamp),
                );
            }
        }

        tick += 1;

        // Process each market
        let mut needs_resubscribe = false;
        for ms in &mut market_states {
            let cfg = &markets[ms.config_idx];
            let sym = cfg.binance_symbol.as_str();

            let snapshot = match binance_snapshots.get(sym) {
                Some(s) => s,
                None => continue,
            };
            let (binance_bid, binance_ask) = binance_bid_asks.get(sym).copied().unwrap_or((snapshot.spot_price, snapshot.spot_price));

            // Polymarket prices
            let yes_prices = poly_stream.get_prices(&ms.market.yes_token_id);
            let no_prices = poly_stream.get_prices(&ms.market.no_token_id);

            // Treat last_update_ms == 0 as "never updated" — substitute now_ms to avoid
            // computing lag as (binance_ts - 0) = a huge misleading number during startup
            // or when REST fallback hasn't run yet for illiquid markets.
            let poly_yes_update_ms = yes_prices.as_ref()
                .map(|p| if p.last_update_ms > 0 { p.last_update_ms } else { now_ms })
                .unwrap_or(now_ms);
            let poly_no_update_ms = no_prices.as_ref()
                .map(|p| if p.last_update_ms > 0 { p.last_update_ms } else { now_ms })
                .unwrap_or(now_ms);
            let binance_update_ms = snapshot.timestamp;
            let poly_avg_update = (poly_yes_update_ms + poly_no_update_ms) / 2;
            let latency_ms = binance_update_ms - poly_avg_update;

            let time_to_open = ms.market.window_start - now_ts;
            let ttx = if time_to_open > 0 {
                (ms.market.window_end - now_ts) as f64 / 60.0
            } else {
                ((ms.market.window_end - now_ts) as f64 / 60.0).max(0.0)
            };
            let resolved = ttx <= 0.0;

            let strike_val = ms.market.strike_price.unwrap_or(snapshot.spot_price);
            let greeks = calculate_greeks(snapshot.spot_price, strike_val, ttx, snapshot.realized_volatility);

            let yes_bid = yes_prices.as_ref().and_then(|p| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
            let yes_ask = yes_prices.as_ref().and_then(|p| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
            let no_bid = no_prices.as_ref().and_then(|p| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
            let no_ask = no_prices.as_ref().and_then(|p| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);

            let yes_spread = calculate_spread_bps(yes_bid, yes_ask);
            let no_spread = calculate_spread_bps(no_bid, no_ask);

            // Resolution detection: WS market_resolved → price-based → Gamma API fallback
            let outcome = if let Some(resolved_event) = market_events.take_resolved_for(&ms.market.slug) {
                // Primary: WS market_resolved event (instant, no HTTP call)
                info!(
                    "[{}] WS resolution: {} -> {}",
                    ms.tag, resolved_event.slug, resolved_event.winning_outcome
                );
                if resolved_event.winning_outcome == "Up" {
                    "YES".to_string()
                } else if resolved_event.winning_outcome == "Down" {
                    "NO".to_string()
                } else {
                    resolved_event.winning_outcome
                }
            } else if yes_bid >= 0.99 {
                "YES".to_string()
            } else if no_bid >= 0.99 {
                "NO".to_string()
            } else if resolved {
                // Market expired but no WS event and prices not conclusive — Gamma API fallback
                let cached = ms.gamma_outcome.lock().unwrap().clone();
                if let Some(result) = cached {
                    result
                } else if !ms.gamma_fetch_in_progress.load(Ordering::SeqCst)
                    && (now_ms - ms.gamma_last_outcome_fetch_ms >= 100) {
                    ms.gamma_last_outcome_fetch_ms = now_ms;
                    // Spawn async Gamma API fetch with generation guard to prevent
                    // stale results from a previous market leaking into a new one
                    ms.gamma_fetch_in_progress.store(true, Ordering::SeqCst);
                    let cache = ms.gamma_outcome.clone();
                    let in_progress = ms.gamma_fetch_in_progress.clone();
                    let gen = ms.gamma_generation.clone();
                    let gen_at_spawn = gen.load(Ordering::SeqCst);
                    let slug = ms.market.slug.clone();
                    let client = http.clone();
                    tokio::spawn(async move {
                        match gamma::fetch_outcome(&client, &slug).await {
                            Ok(result) => {
                                // Only write if generation hasn't changed (no rotation happened)
                                if gen.load(Ordering::SeqCst) == gen_at_spawn {
                                    info!("Gamma API outcome for {}: {}", slug, result);
                                    *cache.lock().unwrap() = Some(result);
                                } else {
                                    info!("Gamma API outcome for {} arrived after rotation, discarding", slug);
                                }
                            }
                            Err(e) => { warn!("Gamma outcome failed for {}: {}", slug, e); }
                        }
                        in_progress.store(false, Ordering::SeqCst);
                    });
                    String::new()
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            let vol_data_source = if snapshot.volatility_regime == "BOOTSTRAP" {
                "BOOTSTRAP".to_string()
            } else {
                snapshot.vol_source_path.clone()
            };

            // --- Order Book Depth ---
            let yes_bid_depth_3 = yes_prices.as_ref().map(|p| p.bid_depth_3).unwrap_or(0.0);
            let yes_ask_depth_3 = yes_prices.as_ref().map(|p| p.ask_depth_3).unwrap_or(0.0);
            let no_bid_depth_3 = no_prices.as_ref().map(|p| p.bid_depth_3).unwrap_or(0.0);
            let no_ask_depth_3 = no_prices.as_ref().map(|p| p.ask_depth_3).unwrap_or(0.0);
            let yes_bid_depth_all = yes_prices.as_ref().map(|p| p.bid_depth_all).unwrap_or(0.0);
            let yes_ask_depth_all = yes_prices.as_ref().map(|p| p.ask_depth_all).unwrap_or(0.0);
            let no_bid_depth_all = no_prices.as_ref().map(|p| p.bid_depth_all).unwrap_or(0.0);
            let no_ask_depth_all = no_prices.as_ref().map(|p| p.ask_depth_all).unwrap_or(0.0);
            let yes_bid_levels = yes_prices.as_ref().map(|p| p.bid_levels_count).unwrap_or(0);
            let yes_ask_levels = yes_prices.as_ref().map(|p| p.ask_levels_count).unwrap_or(0);
            let no_bid_levels = no_prices.as_ref().map(|p| p.bid_levels_count).unwrap_or(0);
            let no_ask_levels = no_prices.as_ref().map(|p| p.ask_levels_count).unwrap_or(0);
            let yes_book_imbalance = yes_prices.as_ref().map(|p| p.book_imbalance).unwrap_or(0.0);
            let no_book_imbalance = no_prices.as_ref().map(|p| p.book_imbalance).unwrap_or(0.0);
            let all_bids = yes_bid_depth_all + no_bid_depth_all;
            let all_asks = yes_ask_depth_all + no_ask_depth_all;
            let combined_obi = if (all_bids + all_asks) > 0.0 {
                (all_bids - all_asks) / (all_bids + all_asks)
            } else { 0.0 };

            // --- Trade Flow ---
            let yes_last_trade_price = yes_prices.as_ref().map(|p| p.last_trade_price).unwrap_or(0.0);
            let yes_last_trade_size = yes_prices.as_ref().map(|p| p.last_trade_size).unwrap_or(0.0);
            let yes_last_trade_side = yes_prices.as_ref().map(|p| p.last_trade_side.clone()).unwrap_or_default();
            let no_last_trade_price = no_prices.as_ref().map(|p| p.last_trade_price).unwrap_or(0.0);
            let no_last_trade_size = no_prices.as_ref().map(|p| p.last_trade_size).unwrap_or(0.0);
            let no_last_trade_side = no_prices.as_ref().map(|p| p.last_trade_side.clone()).unwrap_or_default();
            let yes_taker_buy = yes_prices.as_ref().map(|p| p.taker_buy_volume).unwrap_or(0.0);
            let yes_taker_sell = yes_prices.as_ref().map(|p| p.taker_sell_volume).unwrap_or(0.0);
            let no_taker_buy = no_prices.as_ref().map(|p| p.taker_buy_volume).unwrap_or(0.0);
            let no_taker_sell = no_prices.as_ref().map(|p| p.taker_sell_volume).unwrap_or(0.0);
            let total_buy = yes_taker_buy + no_taker_buy;
            let total_sell = yes_taker_sell + no_taker_sell;
            let trade_flow_imbalance = if (total_buy + total_sell) > 0.0 {
                (total_buy - total_sell) / (total_buy + total_sell)
            } else { 0.0 };

            // --- Binance L2 Depth ---
            let binance_depth = binance_depths.get(sym);
            let binance_bid_depth_3 = binance_depth.map(|d| d.bid_depth_3).unwrap_or(0.0);
            let binance_ask_depth_3 = binance_depth.map(|d| d.ask_depth_3).unwrap_or(0.0);
            let binance_bid_depth_all = binance_depth.map(|d| d.bid_depth_all).unwrap_or(0.0);
            let binance_ask_depth_all = binance_depth.map(|d| d.ask_depth_all).unwrap_or(0.0);
            let binance_bid_levels = binance_depth.map(|d| d.bid_levels).unwrap_or(0);
            let binance_ask_levels = binance_depth.map(|d| d.ask_levels).unwrap_or(0);
            let binance_book_imbalance = binance_depth.map(|d| d.book_imbalance).unwrap_or(0.0);
            let binance_poly_obi_divergence = binance_book_imbalance - yes_book_imbalance;

            let rtds_snap = rtds_per_asset.get(cfg.asset.as_str()).cloned().unwrap_or_default();
            let r = &rtds_snap;

            let record = CsvRecord {
                timestamp: now.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
                market_slug: ms.market.slug.clone(),
                binance_price: snapshot.spot_price,
                binance_volume: sanitize(snapshot.volume_1min),
                yes_bid, yes_bid_size: yes_prices.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0),
                yes_ask, yes_ask_size: yes_prices.as_ref().map(|p| p.best_ask_size).unwrap_or(0.0),
                no_bid, no_bid_size: no_prices.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0),
                no_ask, no_ask_size: no_prices.as_ref().map(|p| p.best_ask_size).unwrap_or(0.0),
                time_to_open,
                time_to_expiry_minutes: ttx,
                strike_price: strike_val,
                realized_volatility: sanitize(snapshot.realized_volatility),
                delta_yes: greeks.delta, gamma: greeks.gamma, vega: greeks.vega, theta: greeks.theta,
                market_resolved: resolved,
                outcome: outcome.clone(),
                timestamp_unix_ms: now_ms,
                binance_update_ms,
                poly_yes_update_ms, poly_no_update_ms,
                latency_binance_poly_ms: latency_ms,
                binance_bid, binance_ask,
                binance_spread_bps: sanitize(snapshot.spread_bps),
                vol_1sec: sanitize(snapshot.vol_1sec),
                vol_5sec: sanitize(snapshot.vol_5sec),
                vol_13sec: sanitize(snapshot.vol_13sec),
                vol_34sec: sanitize(snapshot.vol_34sec),
                vol_89sec: sanitize(snapshot.vol_89sec),
                volatility_regime: snapshot.volatility_regime.clone(),
                volume_1sec: sanitize(snapshot.volume_1sec),
                volume_spike: snapshot.volume_spike,
                signal_quality: snapshot.signal_quality.clone(),
                trade_confidence: sanitize(snapshot.trade_confidence),
                vol_data_source: vol_data_source.clone(),
                yes_spread_bps: sanitize(yes_spread),
                no_spread_bps: sanitize(no_spread),
                rtds_binance_price: sanitize(r.rtds_binance_price),
                rtds_chainlink_price: sanitize(r.rtds_chainlink_price),
                rtds_binance_dev_bps: sanitize(r.rtds_binance_dev_bps),
                rtds_chainlink_dev_bps: sanitize(r.rtds_chainlink_dev_bps),
                rtds_price_advantage_bps: sanitize(r.rtds_price_advantage_bps),
                rtds_chainlink_binance_spread_bps: sanitize(r.rtds_chainlink_binance_spread_bps),
                rtds_binance_lag_ms: r.rtds_binance_lag_ms,
                rtds_chainlink_lag_ms: r.rtds_chainlink_lag_ms,
                rtds_binance_stale_ms: r.rtds_binance_stale_ms,
                rtds_chainlink_stale_ms: r.rtds_chainlink_stale_ms,
                // Order Book Depth columns
                yes_bid_depth_3: sanitize(yes_bid_depth_3),
                yes_ask_depth_3: sanitize(yes_ask_depth_3),
                no_bid_depth_3: sanitize(no_bid_depth_3),
                no_ask_depth_3: sanitize(no_ask_depth_3),
                yes_bid_depth_all: sanitize(yes_bid_depth_all),
                yes_ask_depth_all: sanitize(yes_ask_depth_all),
                no_bid_depth_all: sanitize(no_bid_depth_all),
                no_ask_depth_all: sanitize(no_ask_depth_all),
                yes_bid_levels,
                yes_ask_levels,
                no_bid_levels,
                no_ask_levels,
                yes_book_imbalance: sanitize(yes_book_imbalance),
                no_book_imbalance: sanitize(no_book_imbalance),
                combined_obi: sanitize(combined_obi),
                // Trade Flow columns
                yes_last_trade_price: sanitize(yes_last_trade_price),
                yes_last_trade_size: sanitize(yes_last_trade_size),
                yes_last_trade_side,
                no_last_trade_price: sanitize(no_last_trade_price),
                no_last_trade_size: sanitize(no_last_trade_size),
                no_last_trade_side,
                trade_flow_imbalance: sanitize(trade_flow_imbalance),
                // Binance L2 Depth columns
                binance_bid_depth_3: sanitize(binance_bid_depth_3),
                binance_ask_depth_3: sanitize(binance_ask_depth_3),
                binance_bid_depth_all: sanitize(binance_bid_depth_all),
                binance_ask_depth_all: sanitize(binance_ask_depth_all),
                binance_bid_levels,
                binance_ask_levels,
                binance_book_imbalance: sanitize(binance_book_imbalance),
                binance_poly_obi_divergence: sanitize(binance_poly_obi_divergence),
            };

            ms.csv_writer.write_record(&record)?;

            // --- Compact logging (every 100 ticks) ---
            if tick % 100 == 0 {
                let lag_display = {
                    let yes_has_data = yes_prices.as_ref().map(|p| p.last_update_ms > 0).unwrap_or(false);
                    let no_has_data = no_prices.as_ref().map(|p| p.last_update_ms > 0).unwrap_or(false);
                    if yes_has_data && no_has_data {
                        format!("{}ms", latency_ms)
                    } else {
                        "N/A".to_string()
                    }
                };
                info!(
                    "[{}] #{} {} TTX={:.1}m | ${:.0} | Y:{:.2}@{:.0} N:{:.2}@{:.0} | Vol:{:.1}% \u{0394}:{:.4} | Lag={} | {} | {}",
                    ms.tag, tick, ms.market.unique_id, ttx,
                    snapshot.spot_price,
                    yes_bid, yes_prices.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0),
                    no_bid, no_prices.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0),
                    snapshot.realized_volatility * 100.0, greeks.delta,
                    lag_display, snapshot.volatility_regime, vol_data_source,
                );
            }

            // --- Market rotation ---
            // Start grace period timer when market first expires
            if resolved && ms.resolution_grace_start_ms == 0 {
                ms.resolution_grace_start_ms = now_ms;
                info!("[{}] ⏳ Market {} expired — starting {}s resolution grace period", ms.tag, ms.market.unique_id, RESOLUTION_GRACE_SECS);
            }

            let grace_expired = ms.resolution_grace_start_ms > 0
                && (now_ms - ms.resolution_grace_start_ms) >= RESOLUTION_GRACE_MS;
            let has_outcome = !outcome.is_empty();

            if resolved && (has_outcome || grace_expired) {
                if grace_expired && !has_outcome {
                    warn!("[{}] ⚠️ Resolution grace period expired with no outcome — forcing rotation", ms.tag);
                } else if has_outcome {
                    info!("[{}] ✅ Outcome '{}' detected during grace period — rotating", ms.tag, outcome);
                }

                let time_since_last = if ms.last_fetch_attempt_ms > 0 {
                    now_ms.saturating_sub(ms.last_fetch_attempt_ms)
                } else { i64::MAX };

                let should_attempt = ms.consecutive_fetch_failures == 0
                    || (time_since_last >= FETCH_RETRY_COOLDOWN_MS
                        && ms.consecutive_fetch_failures < MAX_CONSECUTIVE_FETCH_FAILURES);

                if should_attempt {
                    ms.last_fetch_attempt_ms = now_ms;
                    info!("[{}] Market {} expired (TTX={:.1}m) — rotating...", ms.tag, ms.market.unique_id, ttx);

                    // --- Priority 2 fix: ensure outcome is captured before rotation ---
                    // If we still don't have an outcome, do a BLOCKING Gamma API fetch
                    // (with short timeout) and write a final resolution record to CSV.
                    if outcome.is_empty() {
                        info!("[{}] No outcome yet — blocking Gamma fetch before rotation", ms.tag);
                        let final_outcome = match tokio::time::timeout(
                            Duration::from_secs(3),
                            gamma::fetch_outcome(&http, &ms.market.slug),
                        ).await {
                            Ok(Ok(result)) => {
                                info!("[{}] Blocking Gamma outcome: {}", ms.tag, result);
                                result
                            }
                            Ok(Err(e)) => {
                                warn!("[{}] Blocking Gamma fetch failed: {}", ms.tag, e);
                                String::new()
                            }
                            Err(_) => {
                                warn!("[{}] Blocking Gamma fetch timed out (3s)", ms.tag);
                                String::new()
                            }
                        };
                        if !final_outcome.is_empty() {
                            // Write one final record with the resolution outcome
                            let mut final_record = record.clone();
                            final_record.outcome = final_outcome;
                            ms.csv_writer.write_record(&final_record)?;
                            info!("[{}] Final resolution record written: {}", ms.tag, final_record.outcome);
                        }
                    }

                    // Try WS new_market event first, fall back to Gamma API
                    let slug_prefix = format!("{}-updown-{}-", cfg.asset, timeframe_label(cfg.timeframe_secs));
                    let next_market_result = match market_events.take_new_market_for(&slug_prefix) {
                        Some(nm) if nm.outcomes.len() == 2 && nm.asset_ids.len() == 2 => {
                            info!("[{}] WS new_market: {} (instant, no HTTP)", ms.tag, nm.slug);
                            let next_ws = ms.market.window_start + cfg.timeframe_secs;
                            let up_idx = nm.outcomes.iter().position(|o| o == "Up");
                            let down_idx = nm.outcomes.iter().position(|o| o == "Down");
                            let (yes_id, no_id) = match (up_idx, down_idx) {
                                (Some(ui), Some(di)) => (nm.asset_ids[ui].clone(), nm.asset_ids[di].clone()),
                                _ => (nm.asset_ids[0].clone(), nm.asset_ids[1].clone()),
                            };
                            Ok(gamma::MarketInfo {
                                unique_id: nm.slug.clone(),
                                slug: nm.slug,
                                condition_id: String::new(),
                                yes_token_id: yes_id,
                                no_token_id: no_id,
                                window_start: next_ws,
                                window_end: next_ws + cfg.timeframe_secs,
                                strike_price: None,
                                strike_fetched: false,
                                asset: cfg.asset.to_string(),
                                window_seconds: cfg.timeframe_secs,
                            })
                        }
                        _ => {
                            // No WS event or bad shape — Gamma API fallback
                            gamma::fetch_next_market_for(&http, &cfg.asset, cfg.timeframe_secs, ms.market.window_start).await
                        }
                    };

                    match next_market_result {
                        Ok(next_market) => {
                            info!("[{}] Rotated to: {}", ms.tag, next_market.unique_id);

                            // Remove old tokens, update market
                            let old_tokens = vec![ms.market.yes_token_id.clone(), ms.market.no_token_id.clone()];
                            poly_stream.remove_tokens(&old_tokens);

                            ms.market = next_market;
                            // Increment generation BEFORE clearing — any in-flight
                            // Gamma fetch from the old market will see a stale generation
                            // and discard its result instead of writing it here.
                            ms.gamma_generation.fetch_add(1, Ordering::SeqCst);
                            *ms.gamma_outcome.lock().unwrap() = None;
                            ms.gamma_fetch_in_progress.store(false, Ordering::SeqCst);
                            ms.consecutive_fetch_failures = 0;
                            ms.gamma_last_outcome_fetch_ms = 0;
                            ms.resolution_grace_start_ms = 0;

                            // Fetch strike for new market (infinite retry, matches reference)
                            let s = strike::fetch_strike_with_retry(&http, &ms.market, None).await?;
                            ms.market.strike_price = Some(s);
                            ms.market.strike_fetched = true;
                            info!("[{}] Strike: ${:.2}", ms.tag, s);

                            // Flag for post-loop resubscribe with ALL tokens
                            // (calling stream() here with only 2 tokens would kill
                            // all other markets' WS streams — stream() increments
                            // the generation counter, terminating all prior tasks)
                            needs_resubscribe = true;
                        }
                        Err(e) => {
                            ms.consecutive_fetch_failures += 1;
                            warn!("[{}] Rotation failed ({}/{}): {}", ms.tag,
                                ms.consecutive_fetch_failures, MAX_CONSECUTIVE_FETCH_FAILURES, e);
                        }
                    }
                }
            }
        }

        // After any market rotation, re-subscribe ALL tokens so every market
        // keeps its WS stream alive. stream() kills prior streams via generation
        // counter, so we must pass the full set of active tokens.
        if needs_resubscribe {
            let all_tokens: Vec<String> = market_states.iter()
                .flat_map(|ms| vec![ms.market.yes_token_id.clone(), ms.market.no_token_id.clone()])
                .collect();
            info!("[POLY] Re-subscribing all {} tokens after rotation", all_tokens.len());
            let mut new_rx = poly_stream.clone().stream(all_tokens).await?;
            tokio::spawn(async move { while let Some(_) = new_rx.recv().await {} });
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        // RTDS summary (every 500 ticks)
        //
        // NOTE: In multi-asset mode, all assets often show identical stale_ms values.
        // This is EXPECTED: the RTDS server batches all symbol updates together.
        // Chainlink feeds share an Ethereum block timestamp; Binance relay batches
        // all symbols with the same server-assigned timestamp. This is NOT a routing
        // bug — the per-asset prices below prove each asset receives independent data.
        if tick % 500 == 0 {
            for asset in &unique_assets {
                if let Some(rtds) = rtds_per_asset.get(asset.as_str()) {
                    let (asset_b, asset_c) = multi_rtds.update_counts(asset);
                    info!(
                        "[RTDS] {:<3} price=B:{:.4}/C:{:.4} dev=B:{:+.1}/C:{:+.1}bps stale=B:{}ms/C:{}ms n=B:{}/C:{}",
                        asset.to_uppercase(),
                        rtds.rtds_binance_price, rtds.rtds_chainlink_price,
                        rtds.rtds_binance_dev_bps, rtds.rtds_chainlink_dev_bps,
                        rtds.rtds_binance_stale_ms, rtds.rtds_chainlink_stale_ms,
                        asset_b, asset_c,
                    );
                }
            }
        }

        // Periodic volatility refresh (every ~5 min)
        if tick % (300 * 90) == 0 {
            for sym in &unique_symbols {
                let _ = binance_manager.bootstrap_volatility(sym, 10).await;
            }
        }
    }

    // Flush all CSV files
    for ms in &mut market_states {
        ms.csv_writer.flush()?;
    }

    info!("Collection complete! {} markets tracked, {} ticks", market_states.len(), tick);
    println!("\nDone!");

    Ok(())
}
