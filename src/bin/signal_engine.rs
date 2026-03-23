// signal_engine.rs - Signal Engine with Shared Memory Output
//!
//! Adapted from multi_asset_logger_fixed_rotation_sbe_obi.rs. Instead of
//! writing CSV records, this binary writes market signals to shared memory
//! via the `signal-shm` crate, enabling zero-copy cross-process reads by
//! poly-bot and other consumers.
//!
//! Keeps the full data pipeline:
//!   - YAML config, Gamma API market discovery
//!   - Binance SBE (BinanceDataManager) with volatility bootstrap
//!   - Polymarket WS (PolymarketOrderbookStream)
//!   - RTDS tracker (MultiAssetRtdsTracker)
//!   - Market event stream (WS new_market + market_resolved)
//!   - Pre-warm rotation (discovery at 85% + WS pre-subscribe)
//!
//! Outputs:
//!   - Primary: atomic SHM writes (seqlock-protected)
//!   - Optional: --also-csv for dual-output debugging
//!
//! Usage:
//!   cargo run --bin signal_engine -- [--config path.yml] [--shm-path /dev/shm/poly-signals.shm] [--also-csv] [--csv-dir ./] [duration_secs]

use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use clap::Parser;
use rust_decimal::prelude::*;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

use combine_fetcher::api::binance_fixed_sbe_obi::BinanceDataManager;
use combine_fetcher::api::gamma;
use combine_fetcher::api::market_events::MarketEventStream;
use combine_fetcher::api::polymarket::PolymarketOrderbookStream;
use combine_fetcher::api::rtds_tracker::RtdsSnapshot;
use combine_fetcher::api::rtds_tracker_multi::MultiAssetRtdsTracker;
use combine_fetcher::api::strike;
use combine_fetcher::{HTTP_TIMEOUT_SECS, MINUTES_PER_YEAR};

use signal_shm::{AssetSlot, SignalReader, SignalWriter};

// ============================================================================
// CLI ARGS
// ============================================================================

#[derive(Parser, Debug)]
#[command(name = "signal_engine", about = "Signal engine with SHM output")]
struct Args {
    /// Path to the shared memory file
    #[arg(long, default_value = "/dev/shm/poly-signals.shm")]
    shm_path: String,

    /// Path to YAML config file
    #[arg(long, default_value = "multi_asset_logger.yml")]
    config: String,

    /// Also write CSV files (dual-output debugging)
    #[arg(long, default_value_t = false)]
    also_csv: bool,

    /// Directory for CSV files (only used with --also-csv)
    #[arg(long, default_value = "./")]
    csv_dir: String,

    /// Duration in seconds (default: 8 hours)
    duration: Option<u64>,
}

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

/// Default: single BTC 15m market
fn default_markets() -> Vec<MarketConfig> {
    vec![MarketConfig {
        asset: "btc".into(),
        timeframe_secs: 900,
        binance_symbol: "BTCUSDT".into(),
    }]
}

// ============================================================================
// CSV RECORD + WRITER (only used with --also-csv)
// ============================================================================

#[derive(Debug, Clone)]
struct CsvRecord {
    timestamp: String,
    market_slug: String,
    binance_price: f64,
    binance_volume: f64,
    yes_bid: f64,
    yes_bid_size: f64,
    yes_ask: f64,
    yes_ask_size: f64,
    no_bid: f64,
    no_bid_size: f64,
    no_ask: f64,
    no_ask_size: f64,
    time_to_open: i64,
    time_to_expiry_minutes: f64,
    strike_price: f64,
    realized_volatility: f64,
    delta_yes: f64,
    gamma: f64,
    vega: f64,
    theta: f64,
    market_resolved: bool,
    outcome: String,
    timestamp_unix_ms: i64,
    binance_update_ms: i64,
    poly_yes_update_ms: i64,
    poly_no_update_ms: i64,
    latency_binance_poly_ms: i64,
    binance_bid: f64,
    binance_ask: f64,
    binance_spread_bps: f64,
    vol_1sec: f64,
    vol_5sec: f64,
    vol_13sec: f64,
    vol_34sec: f64,
    vol_89sec: f64,
    volatility_regime: String,
    volume_1sec: f64,
    volume_spike: bool,
    signal_quality: String,
    trade_confidence: f64,
    vol_data_source: String,
    yes_spread_bps: f64,
    no_spread_bps: f64,
    rtds_binance_price: f64,
    rtds_chainlink_price: f64,
    rtds_binance_dev_bps: f64,
    rtds_chainlink_dev_bps: f64,
    rtds_price_advantage_bps: f64,
    rtds_chainlink_binance_spread_bps: f64,
    rtds_binance_lag_ms: i64,
    rtds_chainlink_lag_ms: i64,
    rtds_binance_stale_ms: i64,
    rtds_chainlink_stale_ms: i64,
    yes_bid_depth_3: f64,
    yes_ask_depth_3: f64,
    no_bid_depth_3: f64,
    no_ask_depth_3: f64,
    yes_bid_depth_all: f64,
    yes_ask_depth_all: f64,
    no_bid_depth_all: f64,
    no_ask_depth_all: f64,
    yes_bid_levels: u16,
    yes_ask_levels: u16,
    no_bid_levels: u16,
    no_ask_levels: u16,
    yes_book_imbalance: f64,
    no_book_imbalance: f64,
    combined_obi: f64,
    yes_last_trade_price: f64,
    yes_last_trade_size: f64,
    yes_last_trade_side: String,
    no_last_trade_price: f64,
    no_last_trade_size: f64,
    no_last_trade_side: String,
    trade_flow_imbalance: f64,
    binance_bid_depth_3: f64,
    binance_ask_depth_3: f64,
    binance_bid_depth_all: f64,
    binance_ask_depth_all: f64,
    binance_bid_levels: u16,
    binance_ask_levels: u16,
    binance_book_imbalance: f64,
    binance_poly_obi_divergence: f64,
}

impl CsvRecord {
    fn csv_header() -> &'static str {
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

    fn to_csv_line(&self) -> String {
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
            self.yes_bid_depth_3, self.yes_ask_depth_3,
            self.no_bid_depth_3, self.no_ask_depth_3,
            self.yes_bid_depth_all, self.yes_ask_depth_all,
            self.no_bid_depth_all, self.no_ask_depth_all,
            self.yes_bid_levels, self.yes_ask_levels,
            self.no_bid_levels, self.no_ask_levels,
            self.yes_book_imbalance, self.no_book_imbalance, self.combined_obi,
            self.yes_last_trade_price, self.yes_last_trade_size,
            self.yes_last_trade_side,
            self.no_last_trade_price, self.no_last_trade_size,
            self.no_last_trade_side,
            self.trade_flow_imbalance,
            self.binance_bid_depth_3, self.binance_ask_depth_3,
            self.binance_bid_depth_all, self.binance_ask_depth_all,
            self.binance_bid_levels, self.binance_ask_levels,
            self.binance_book_imbalance, self.binance_poly_obi_divergence,
        )
    }
}

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
            warn!(
                "Lock file {} exists with PID {} (alive). Proceeding with append.",
                lock_path, existing_pid
            );
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
        self.last_flush_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
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
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
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
    if bid <= 0.0 || ask <= bid {
        return 0.0;
    }
    let mid = (bid + ask) / 2.0;
    if mid <= 0.0 {
        return 0.0;
    }
    ((ask - bid) / mid) * 10000.0
}

#[derive(Debug, Clone, Copy, Default)]
struct Greeks {
    delta: f64,
    gamma: f64,
    vega: f64,
    theta: f64,
}

fn calculate_greeks(spot: f64, strike: f64, ttx_min: f64, vol: f64) -> Greeks {
    if ttx_min <= 0.01
        || vol <= 0.0
        || spot <= 0.0
        || strike <= 0.0
        || vol.is_nan()
        || spot.is_nan()
        || strike.is_nan()
        || ttx_min.is_nan()
    {
        return Greeks {
            delta: if spot >= strike { 1.0 } else { 0.0 },
            ..Default::default()
        };
    }
    let t = ttx_min / MINUTES_PER_YEAR;
    let sqrt_t = t.sqrt();
    let d1 = ((spot / strike).ln() + (vol * vol / 2.0) * t) / (vol * sqrt_t);
    let d2 = ((spot / strike).ln() - (vol * vol / 2.0) * t) / (vol * sqrt_t);
    let norm_pdf =
        |x: f64| -> f64 { (-x * x / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt() };
    let n_prime_d2 = norm_pdf(d2);
    let delta = (n_prime_d2 / (spot * vol * sqrt_t)).clamp(0.0, 1.0);
    let gamma_val =
        (n_prime_d2 * d1 / (spot * spot * vol * vol * t)).clamp(-1e6, 1e6);
    let vega = (n_prime_d2 * sqrt_t * 0.01).clamp(-1e6, 1e6);
    let theta =
        (-(spot * n_prime_d2 * vol) / (2.0 * sqrt_t) / 365.0).clamp(-1e6, 1e6);
    Greeks {
        delta: if delta.is_finite() { delta } else { 0.0 },
        gamma: if gamma_val.is_finite() { gamma_val } else { 0.0 },
        vega: if vega.is_finite() { vega } else { 0.0 },
        theta: if theta.is_finite() { theta } else { 0.0 },
    }
}

fn truncate_id(id: &str, first: usize, last: usize) -> String {
    let len = id.len();
    if len <= first + last {
        return id.to_string();
    }
    format!("{}...{}", &id[..first], &id[len - last..])
}

fn sanitize(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

fn timeframe_label(secs: i64) -> &'static str {
    if secs == 300 {
        "5m"
    } else {
        "15m"
    }
}

fn market_tag(asset: &str, secs: i64) -> String {
    format!("{}-{}", asset.to_uppercase(), timeframe_label(secs))
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

// ============================================================================
// SHM WRITE HELPERS
// ============================================================================

/// Write all current signal fields to a SHM slot. Called once per market per tick.
fn write_slot_signals(
    slot: &AssetSlot,
    now_us_val: u64,
    snapshot: &combine_fetcher::MarketSnapshot,
    binance_bid: f64,
    binance_ask: f64,
    yes_bid: f64,
    yes_ask: f64,
    yes_bid_size: f64,
    yes_ask_size: f64,
    no_bid: f64,
    no_ask: f64,
    no_bid_size: f64,
    no_ask_size: f64,
    strike_val: f64,
    ttx_secs: i64,
    window_start: i64,
    window_end: i64,
    greeks: &Greeks,
    yes_book_imbalance: f64,
    combined_obi: f64,
    binance_book_imbalance: f64,
    yes_bid_depth_3: f64,
    yes_ask_depth_3: f64,
    no_bid_depth_3: f64,
    no_ask_depth_3: f64,
    yes_bid_depth_all: f64,
    yes_ask_depth_all: f64,
    no_bid_depth_all: f64,
    no_ask_depth_all: f64,
    rtds: &RtdsSnapshot,
    yes_last_trade_price: f64,
    yes_last_trade_size: f64,
    no_last_trade_price: f64,
    no_last_trade_size: f64,
    resolved: bool,
    outcome: &str,
    binance_update_ms: i64,
    poly_yes_update_ms: i64,
    poly_no_update_ms: i64,
) {
    slot.begin_write();

    // Binance
    AssetSlot::write_f64(&slot.binance_mid, snapshot.spot_price);
    AssetSlot::write_f64(&slot.binance_bid, binance_bid);
    AssetSlot::write_f64(&slot.binance_ask, binance_ask);
    AssetSlot::write_f64(&slot.binance_volume_1m, sanitize(snapshot.volume_1min));
    slot.binance_update_us
        .store((binance_update_ms as u64) * 1000, Ordering::Relaxed);

    // Polymarket BBA
    AssetSlot::write_f64(&slot.yes_bid, yes_bid);
    AssetSlot::write_f64(&slot.yes_ask, yes_ask);
    AssetSlot::write_f64(&slot.yes_bid_size, yes_bid_size);
    AssetSlot::write_f64(&slot.yes_ask_size, yes_ask_size);
    AssetSlot::write_f64(&slot.no_bid, no_bid);
    AssetSlot::write_f64(&slot.no_ask, no_ask);
    AssetSlot::write_f64(&slot.no_bid_size, no_bid_size);
    AssetSlot::write_f64(&slot.no_ask_size, no_ask_size);
    slot.poly_yes_update_us
        .store((poly_yes_update_ms as u64) * 1000, Ordering::Relaxed);
    slot.poly_no_update_us
        .store((poly_no_update_ms as u64) * 1000, Ordering::Relaxed);

    // Market context
    AssetSlot::write_f64(&slot.strike, strike_val);
    slot.ttx_seconds.store(ttx_secs, Ordering::Relaxed);
    slot.window_start.store(window_start, Ordering::Relaxed);
    slot.window_end.store(window_end, Ordering::Relaxed);

    // Volatility
    AssetSlot::write_f64(&slot.volatility, sanitize(snapshot.realized_volatility));
    AssetSlot::write_f64(&slot.vol_1sec, sanitize(snapshot.vol_1sec));
    AssetSlot::write_f64(&slot.vol_5sec, sanitize(snapshot.vol_5sec));
    AssetSlot::write_f64(&slot.vol_13sec, sanitize(snapshot.vol_13sec));
    AssetSlot::write_f64(&slot.vol_34sec, sanitize(snapshot.vol_34sec));
    AssetSlot::write_f64(&slot.vol_89sec, sanitize(snapshot.vol_89sec));

    // OBI
    AssetSlot::write_f64(&slot.yes_book_imbalance, sanitize(yes_book_imbalance));
    AssetSlot::write_f64(&slot.combined_obi, sanitize(combined_obi));
    AssetSlot::write_f64(&slot.binance_obi, sanitize(binance_book_imbalance));

    // Depth
    AssetSlot::write_f64(&slot.yes_bid_depth_3, sanitize(yes_bid_depth_3));
    AssetSlot::write_f64(&slot.yes_ask_depth_3, sanitize(yes_ask_depth_3));
    AssetSlot::write_f64(&slot.no_bid_depth_3, sanitize(no_bid_depth_3));
    AssetSlot::write_f64(&slot.no_ask_depth_3, sanitize(no_ask_depth_3));
    AssetSlot::write_f64(&slot.yes_bid_depth_all, sanitize(yes_bid_depth_all));
    AssetSlot::write_f64(&slot.yes_ask_depth_all, sanitize(yes_ask_depth_all));
    AssetSlot::write_f64(&slot.no_bid_depth_all, sanitize(no_bid_depth_all));
    AssetSlot::write_f64(&slot.no_ask_depth_all, sanitize(no_ask_depth_all));

    // RTDS
    AssetSlot::write_f64(&slot.rtds_binance, sanitize(rtds.rtds_binance_price));
    AssetSlot::write_f64(&slot.rtds_chainlink, sanitize(rtds.rtds_chainlink_price));
    AssetSlot::write_f64(
        &slot.rtds_binance_dev_bps,
        sanitize(rtds.rtds_binance_dev_bps),
    );
    AssetSlot::write_f64(
        &slot.rtds_chainlink_dev_bps,
        sanitize(rtds.rtds_chainlink_dev_bps),
    );
    AssetSlot::write_f64(
        &slot.rtds_advantage_bps,
        sanitize(rtds.rtds_price_advantage_bps),
    );
    slot.rtds_update_us.store(now_us_val, Ordering::Relaxed);

    // Greeks
    AssetSlot::write_f64(&slot.delta, greeks.delta);
    AssetSlot::write_f64(&slot.gamma_greek, greeks.gamma);
    AssetSlot::write_f64(&slot.vega, greeks.vega);
    AssetSlot::write_f64(&slot.theta, greeks.theta);

    // Trade flow
    AssetSlot::write_f64(&slot.yes_last_trade_price, sanitize(yes_last_trade_price));
    AssetSlot::write_f64(&slot.yes_last_trade_size, sanitize(yes_last_trade_size));
    AssetSlot::write_f64(&slot.no_last_trade_price, sanitize(no_last_trade_price));
    AssetSlot::write_f64(&slot.no_last_trade_size, sanitize(no_last_trade_size));

    // Resolution state
    slot.market_resolved
        .store(if resolved { 1 } else { 0 }, Ordering::Relaxed);
    if !outcome.is_empty() {
        AssetSlot::write_string(&slot.outcome, outcome);
    }

    slot.end_write();
}

/// Reset all price/signal fields in a slot to zero (on rotation).
fn reset_slot_signals(slot: &AssetSlot) {
    slot.begin_write();

    AssetSlot::write_f64(&slot.binance_mid, 0.0);
    AssetSlot::write_f64(&slot.binance_bid, 0.0);
    AssetSlot::write_f64(&slot.binance_ask, 0.0);
    AssetSlot::write_f64(&slot.binance_volume_1m, 0.0);
    slot.binance_update_us.store(0, Ordering::Relaxed);

    AssetSlot::write_f64(&slot.yes_bid, 0.0);
    AssetSlot::write_f64(&slot.yes_ask, 0.0);
    AssetSlot::write_f64(&slot.yes_bid_size, 0.0);
    AssetSlot::write_f64(&slot.yes_ask_size, 0.0);
    AssetSlot::write_f64(&slot.no_bid, 0.0);
    AssetSlot::write_f64(&slot.no_ask, 0.0);
    AssetSlot::write_f64(&slot.no_bid_size, 0.0);
    AssetSlot::write_f64(&slot.no_ask_size, 0.0);
    slot.poly_yes_update_us.store(0, Ordering::Relaxed);
    slot.poly_no_update_us.store(0, Ordering::Relaxed);

    AssetSlot::write_f64(&slot.strike, 0.0);
    slot.ttx_seconds.store(0, Ordering::Relaxed);
    slot.window_start.store(0, Ordering::Relaxed);
    slot.window_end.store(0, Ordering::Relaxed);

    AssetSlot::write_f64(&slot.volatility, 0.0);
    AssetSlot::write_f64(&slot.vol_1sec, 0.0);
    AssetSlot::write_f64(&slot.vol_5sec, 0.0);
    AssetSlot::write_f64(&slot.vol_13sec, 0.0);
    AssetSlot::write_f64(&slot.vol_34sec, 0.0);
    AssetSlot::write_f64(&slot.vol_89sec, 0.0);

    AssetSlot::write_f64(&slot.yes_book_imbalance, 0.0);
    AssetSlot::write_f64(&slot.combined_obi, 0.0);
    AssetSlot::write_f64(&slot.binance_obi, 0.0);

    AssetSlot::write_f64(&slot.yes_bid_depth_3, 0.0);
    AssetSlot::write_f64(&slot.yes_ask_depth_3, 0.0);
    AssetSlot::write_f64(&slot.no_bid_depth_3, 0.0);
    AssetSlot::write_f64(&slot.no_ask_depth_3, 0.0);
    AssetSlot::write_f64(&slot.yes_bid_depth_all, 0.0);
    AssetSlot::write_f64(&slot.yes_ask_depth_all, 0.0);
    AssetSlot::write_f64(&slot.no_bid_depth_all, 0.0);
    AssetSlot::write_f64(&slot.no_ask_depth_all, 0.0);

    AssetSlot::write_f64(&slot.rtds_binance, 0.0);
    AssetSlot::write_f64(&slot.rtds_chainlink, 0.0);
    AssetSlot::write_f64(&slot.rtds_binance_dev_bps, 0.0);
    AssetSlot::write_f64(&slot.rtds_chainlink_dev_bps, 0.0);
    AssetSlot::write_f64(&slot.rtds_advantage_bps, 0.0);
    slot.rtds_update_us.store(0, Ordering::Relaxed);

    AssetSlot::write_f64(&slot.delta, 0.0);
    AssetSlot::write_f64(&slot.gamma_greek, 0.0);
    AssetSlot::write_f64(&slot.vega, 0.0);
    AssetSlot::write_f64(&slot.theta, 0.0);

    AssetSlot::write_f64(&slot.yes_last_trade_price, 0.0);
    AssetSlot::write_f64(&slot.yes_last_trade_size, 0.0);
    AssetSlot::write_f64(&slot.no_last_trade_price, 0.0);
    AssetSlot::write_f64(&slot.no_last_trade_size, 0.0);

    slot.market_resolved.store(0, Ordering::Relaxed);
    AssetSlot::write_string(&slot.outcome, "");

    slot.end_write();
}

/// Write market identity fields to a SHM slot (on init or rotation).
fn write_slot_identity(
    slot: &AssetSlot,
    market: &gamma::MarketInfo,
    asset: &str,
    timeframe_secs: i64,
) {
    slot.begin_write();
    AssetSlot::write_string(&slot.asset, asset);
    slot.timeframe_secs
        .store(timeframe_secs as u64, Ordering::Relaxed);
    AssetSlot::write_string(&slot.yes_token_id, &market.yes_token_id);
    AssetSlot::write_string(&slot.no_token_id, &market.no_token_id);
    AssetSlot::write_string(&slot.condition_id, &market.condition_id);
    AssetSlot::write_string(&slot.slug, &market.unique_id);
    slot.window_start
        .store(market.window_start, Ordering::Relaxed);
    slot.window_end
        .store(market.window_end, Ordering::Relaxed);
    slot.market_resolved.store(0, Ordering::Relaxed);
    AssetSlot::write_string(&slot.outcome, "");
    slot.end_write();
}

// ============================================================================
// PRE-WARM STATE — tracks next market discovery and pre-subscription
// ============================================================================

const PRE_WARM_THRESHOLD: f64 = 0.85;

fn window_progress(window_start: i64, window_end: i64, now_ts: i64) -> f64 {
    let duration = window_end - window_start;
    if duration <= 0 {
        return 1.0;
    }
    let elapsed = now_ts - window_start;
    (elapsed as f64 / duration as f64).clamp(0.0, 1.0)
}

struct PreWarmState {
    discovery_launched: bool,
    pending_market: Arc<Mutex<Option<gamma::MarketInfo>>>,
    next_market: Option<gamma::MarketInfo>,
    ws_pre_subscribed: bool,
    pending_strike: Arc<Mutex<Option<f64>>>,
    strike_fetch_in_progress: Arc<AtomicBool>,
}

impl PreWarmState {
    fn new() -> Self {
        Self {
            discovery_launched: false,
            pending_market: Arc::new(Mutex::new(None)),
            next_market: None,
            ws_pre_subscribed: false,
            pending_strike: Arc::new(Mutex::new(None)),
            strike_fetch_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    fn reset(&mut self) {
        self.discovery_launched = false;
        *self.pending_market.lock().unwrap() = None;
        self.next_market = None;
        self.ws_pre_subscribed = false;
        *self.pending_strike.lock().unwrap() = None;
        self.strike_fetch_in_progress.store(false, Ordering::SeqCst);
    }

    fn poll_pending(&mut self) -> bool {
        if self.next_market.is_some() {
            return false;
        }
        let pending = self.pending_market.lock().unwrap().take();
        if let Some(market) = pending {
            self.next_market = Some(market);
            true
        } else {
            false
        }
    }

    fn take(&mut self) -> Option<(gamma::MarketInfo, Option<f64>)> {
        let market = self.next_market.take()?;
        let strike = self.pending_strike.lock().unwrap().take();
        self.ws_pre_subscribed = false;
        Some((market, strike))
    }
}

// ============================================================================
// PER-MARKET RUNTIME STATE
// ============================================================================

struct MarketState {
    config_idx: usize,
    market: gamma::MarketInfo,
    /// SHM slot index for this market
    shm_slot_index: usize,
    /// CSV writer (only active when --also-csv is set)
    csv_writer: Option<MarketCsvWriter>,
    gamma_outcome: Arc<Mutex<Option<String>>>,
    gamma_fetch_in_progress: Arc<AtomicBool>,
    gamma_generation: Arc<AtomicU64>,
    consecutive_fetch_failures: u32,
    last_fetch_attempt_ms: i64,
    gamma_last_outcome_fetch_ms: i64,
    resolution_grace_start_ms: i64,
    tag: String,
    pre_warm: PreWarmState,
}

// ============================================================================
// READER THREAD — self-monitoring status printer
// ============================================================================

fn spawn_reader_thread(shm_path: String, slot_count: usize, shutdown: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        // Brief delay to let the writer initialize all slots
        std::thread::sleep(Duration::from_secs(5));

        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_secs(30));
            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            let reader = match SignalReader::open(&shm_path) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[MONITOR] Failed to open SHM: {}", e);
                    continue;
                }
            };

            let alive = reader.engine_alive();
            let now_us_val = now_us();

            println!(
                "\n[MONITOR] === Signal Status (engine {}) ===",
                if alive { "ALIVE" } else { "DEAD" }
            );

            for i in 0..slot_count {
                match reader.read_slot(i) {
                    Some(snap) => {
                        if snap.asset.is_empty() {
                            println!("  Slot {}: (empty)", i);
                            continue;
                        }
                        let binance_age_ms =
                            now_us_val.saturating_sub(snap.binance_update_us) / 1000;
                        let poly_age_ms =
                            now_us_val.saturating_sub(snap.poly_yes_update_us) / 1000;
                        let stale = SignalReader::is_stale(&snap, 5_000_000);
                        println!(
                            "  Slot {}: {}-{}s | seq={} epoch={} | BTC=${:.0} | Y:{:.2}/{:.2} N:{:.2}/{:.2} | TTX={}s | strike=${:.0} | age B:{}ms P:{}ms {}{}",
                            i,
                            snap.asset,
                            snap.timeframe_secs,
                            snap.seq,
                            snap.market_epoch,
                            snap.binance_mid,
                            snap.yes_bid,
                            snap.yes_ask,
                            snap.no_bid,
                            snap.no_ask,
                            snap.ttx_seconds,
                            snap.strike,
                            binance_age_ms,
                            poly_age_ms,
                            if stale { "[STALE]" } else { "" },
                            if snap.market_resolved {
                                format!(" [RESOLVED:{}]", snap.outcome)
                            } else {
                                String::new()
                            },
                        );
                    }
                    None => {
                        println!("  Slot {}: (torn read)", i);
                    }
                }
            }
        }
    });
}

// ============================================================================
// MAIN
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("signal_engine=info".parse().unwrap())
                .add_directive("combine_fetcher=info".parse().unwrap()),
        )
        .init();

    let args = Args::parse();
    let duration = args.duration.unwrap_or(28800);

    // --- Load config ---
    let markets: Vec<MarketConfig> = if std::path::Path::new(&args.config).exists() {
        load_config(&args.config)?
    } else {
        info!(
            "Config '{}' not found, using default (BTC-15m)",
            args.config
        );
        default_markets()
    };

    println!("=====================================================================");
    println!("  Signal Engine - SHM Output");
    println!(
        "  {} market(s) | Config: {}",
        markets.len(),
        args.config
    );
    println!("  SHM: {}", args.shm_path);
    println!(
        "  Pre-warm at {:.0}% window progress | Zero cold-start rotation",
        PRE_WARM_THRESHOLD * 100.0
    );
    if args.also_csv {
        println!("  CSV dual-output: ENABLED (dir: {})", args.csv_dir);
    }
    println!("=====================================================================\n");

    println!(
        "Duration: {}s ({:.1}h)",
        duration,
        duration as f64 / 3600.0
    );
    println!("Markets:");
    for cfg in &markets {
        println!(
            "  - {}-{} ({})",
            cfg.asset.to_uppercase(),
            timeframe_label(cfg.timeframe_secs),
            cfg.binance_symbol
        );
    }
    println!();

    // Ensure CSV directory exists (if dual-output)
    if args.also_csv && args.csv_dir != "." {
        std::fs::create_dir_all(&args.csv_dir)?;
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
                println!(
                    " {} YES:{}... NO:{}...",
                    m.unique_id,
                    truncate_id(&m.yes_token_id, 8, 0),
                    truncate_id(&m.no_token_id, 8, 0)
                );

                let csv_writer = if args.also_csv {
                    let tf = timeframe_label(cfg.timeframe_secs);
                    let market_dir = if args.csv_dir == "." {
                        format!("{}/{}", cfg.asset, tf)
                    } else {
                        format!("{}/{}/{}", args.csv_dir, cfg.asset, tf)
                    };
                    std::fs::create_dir_all(&market_dir)?;
                    let base_stem = format!("{}/{}_{}_data", market_dir, cfg.asset, tf);
                    Some(MarketCsvWriter::new(base_stem))
                } else {
                    None
                };

                let shm_slot_index = market_states.len();
                market_states.push(MarketState {
                    config_idx: i,
                    market: m,
                    shm_slot_index,
                    csv_writer,
                    gamma_outcome: Arc::new(Mutex::new(None)),
                    gamma_fetch_in_progress: Arc::new(AtomicBool::new(false)),
                    gamma_generation: Arc::new(AtomicU64::new(0)),
                    consecutive_fetch_failures: 0,
                    last_fetch_attempt_ms: 0,
                    gamma_last_outcome_fetch_ms: 0,
                    resolution_grace_start_ms: 0,
                    tag,
                    pre_warm: PreWarmState::new(),
                });
            }
            Err(e) => {
                println!(" FAILED: {}", e);
                warn!("[{}] Skipping -- market fetch failed: {}", tag, e);
            }
        }
    }

    if market_states.is_empty() {
        error!("No markets could be fetched. Exiting.");
        return Err(anyhow::anyhow!("No markets available"));
    }

    println!(
        "Fetched {}/{} markets\n",
        market_states.len(),
        markets.len()
    );

    // --- Create SHM ---
    let shm_writer = SignalWriter::create(&args.shm_path, market_states.len())?;
    println!(
        "SHM created: {} ({} slots)",
        args.shm_path,
        market_states.len()
    );

    // Initialize SHM slot identities
    for ms in &market_states {
        let cfg = &markets[ms.config_idx];
        write_slot_identity(
            shm_writer.slot(ms.shm_slot_index),
            &ms.market,
            &cfg.asset,
            cfg.timeframe_secs,
        );
    }

    // --- Collect unique Binance symbols ---
    let unique_symbols: Vec<String> = {
        let mut seen = HashSet::new();
        market_states
            .iter()
            .filter_map(|ms| {
                let sym = &markets[ms.config_idx].binance_symbol;
                if seen.insert(sym.clone()) {
                    Some(sym.clone())
                } else {
                    None
                }
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

    // --- Fetch strikes sequentially ---
    println!("\nFetching strike prices (sequential)...");
    for ms in &mut market_states {
        print!("  [{}] ...", ms.tag);
        match strike::fetch_strike_with_retry(&http, &ms.market, None).await {
            Ok(s) => {
                ms.market.strike_price = Some(s);
                ms.market.strike_fetched = true;
                println!(" ${:.2}", s);
            }
            Err(e) => {
                println!(" FAILED: {} (will use 0.0 until backfilled)", e);
            }
        }
    }

    // --- Setup Polymarket ---
    let poly_stream = Arc::new(PolymarketOrderbookStream::new());
    let all_token_ids: Vec<String> = market_states
        .iter()
        .flat_map(|ms| {
            vec![
                ms.market.yes_token_id.clone(),
                ms.market.no_token_id.clone(),
            ]
        })
        .collect();

    let mut rx = poly_stream.clone().stream(all_token_ids.clone()).await?;
    let mut poly_drain_handle: tokio::task::JoinHandle<()> = tokio::spawn(async move {
        while let Some(_update) = rx.recv().await {}
    });
    println!(
        "Polymarket connected ({} tokens)",
        market_states.len() * 2
    );
    tokio::time::sleep(Duration::from_secs(2)).await;

    // --- Setup RTDS ---
    let unique_assets: Vec<String> = {
        let mut seen = HashSet::new();
        markets
            .iter()
            .filter_map(|c| {
                if seen.insert(c.asset.clone()) {
                    Some(c.asset.clone())
                } else {
                    None
                }
            })
            .collect()
    };
    let asset_refs: Vec<&str> = unique_assets.iter().map(|s| s.as_str()).collect();
    let mut multi_rtds = MultiAssetRtdsTracker::new(&asset_refs);
    multi_rtds.start();
    println!(
        "RTDS tracker connected ({})",
        unique_assets
            .iter()
            .map(|a| a.to_uppercase())
            .collect::<Vec<_>>()
            .join("/")
    );
    tokio::time::sleep(Duration::from_secs(2)).await;

    // --- Setup Market Event Stream ---
    let market_events = MarketEventStream::new();
    market_events.start(all_token_ids.clone());
    println!(
        "Market event stream connected ({} tokens)",
        all_token_ids.len()
    );

    // --- Initialize CSV files (if dual-output) ---
    if args.also_csv {
        let now = Utc::now();
        for ms in &mut market_states {
            if let Some(ref mut w) = ms.csv_writer {
                w.check_rotation(&now)?;
            }
        }
        println!("CSV files created");
    }
    println!();

    // --- Signal handling ---
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("Received SIGINT, shutting down...");
        shutdown_signal.store(true, Ordering::SeqCst);
    });

    // --- Spawn self-monitoring reader thread ---
    spawn_reader_thread(
        args.shm_path.clone(),
        market_states.len(),
        shutdown.clone(),
    );

    let start_time = SystemTime::now();
    let end_time = start_time + Duration::from_secs(duration);
    let mut tick: u64 = 0;
    let mut last_log_ms: i64 = 0;
    let mut last_heartbeat_ms: i64 = 0;

    const MAX_CONSECUTIVE_FETCH_FAILURES: u32 = 10;
    const FETCH_RETRY_COOLDOWN_MS: i64 = 5000;
    const RESOLUTION_GRACE_SECS: i64 = 30;
    const RESOLUTION_GRACE_MS: i64 = RESOLUTION_GRACE_SECS * 1000;

    info!("Starting signal engine with SHM output...\n");

    // ========================================================================
    // MAIN LOOP
    // ========================================================================

    loop {
        if SystemTime::now() >= end_time || shutdown.load(Ordering::SeqCst) {
            break;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;

        let now = Utc::now();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // Heartbeat update (every second)
        if now_ms - last_heartbeat_ms >= 1000 {
            shm_writer.heartbeat();
            last_heartbeat_ms = now_ms;
        }

        // Hourly CSV rotation (if dual-output)
        if args.also_csv {
            for ms in &mut market_states {
                if let Some(ref mut w) = ms.csv_writer {
                    let _ = w.check_rotation(&now);
                }
            }
        }

        // Frequency control: minimum 10ms between ticks
        let should_record = now_ms - last_log_ms >= 10;
        if !should_record {
            continue;
        }
        last_log_ms = now_ms;

        let now_ts = now.timestamp();
        let now_us_val = now_us();

        // Cache Binance snapshots per symbol
        let mut binance_snapshots: HashMap<&str, _> = HashMap::new();
        let mut binance_bid_asks: HashMap<&str, (f64, f64)> = HashMap::new();
        for sym in &unique_symbols {
            if let Some(snap) = binance_manager.get_market_snapshot(sym).await {
                let ba = binance_manager
                    .get_bid_ask(sym)
                    .await
                    .unwrap_or((snap.spot_price, snap.spot_price));
                binance_bid_asks.insert(sym.as_str(), ba);
                binance_snapshots.insert(sym.as_str(), snap);
            }
        }

        // Cache Binance depth data per symbol
        let mut binance_depths: HashMap<&str, _> = HashMap::new();
        for sym in &unique_symbols {
            if let Some(depth) = binance_manager.get_depth_data(sym).await {
                binance_depths.insert(sym.as_str(), depth);
            }
        }

        // RTDS snapshots per asset
        let mut rtds_per_asset: HashMap<&str, RtdsSnapshot> = HashMap::new();
        for cfg in &markets {
            if rtds_per_asset.contains_key(cfg.asset.as_str()) {
                continue;
            }
            if let Some(snap) = binance_snapshots.get(cfg.binance_symbol.as_str()) {
                rtds_per_asset.insert(
                    &cfg.asset,
                    multi_rtds.get_snapshot(&cfg.asset, snap.spot_price, snap.timestamp),
                );
            }
        }

        tick += 1;

        // ====================================================================
        // PRE-WARM PHASE 1: Launch discovery at 85% window progress
        // ====================================================================
        for ms in &mut market_states {
            if ms.pre_warm.discovery_launched {
                continue;
            }

            let progress = window_progress(ms.market.window_start, ms.market.window_end, now_ts);
            if progress < PRE_WARM_THRESHOLD {
                continue;
            }

            ms.pre_warm.discovery_launched = true;
            let cfg = &markets[ms.config_idx];
            let tag = ms.tag.clone();
            let asset = cfg.asset.clone();
            let timeframe_secs = cfg.timeframe_secs;
            let current_window_start = ms.market.window_start;
            let http_clone = http.clone();
            let pending_slot = ms.pre_warm.pending_market.clone();
            let strike_slot = ms.pre_warm.pending_strike.clone();
            let strike_in_progress = ms.pre_warm.strike_fetch_in_progress.clone();

            info!(
                "[{}] PRE-WARM: {:.0}% window -- launching background discovery",
                tag,
                progress * 100.0,
            );

            tokio::spawn(async move {
                match gamma::fetch_next_market_for(
                    &http_clone,
                    &asset,
                    timeframe_secs,
                    current_window_start,
                )
                .await
                {
                    Ok(next_market) => {
                        info!(
                            "[{}] PRE-WARM: Discovered {} YES:{}... NO:{}...",
                            tag,
                            next_market.unique_id,
                            truncate_id(&next_market.yes_token_id, 8, 0),
                            truncate_id(&next_market.no_token_id, 8, 0),
                        );
                        let market_for_strike = next_market.clone();
                        *pending_slot.lock().unwrap() = Some(next_market);

                        let retry_cfg = strike::RetryConfig {
                            max_attempts: Some(30),
                            initial_backoff_ms: 1000,
                            max_backoff_ms: 3000,
                        };
                        strike_in_progress.store(true, Ordering::SeqCst);
                        match strike::fetch_strike_with_retry(
                            &http_clone,
                            &market_for_strike,
                            Some(retry_cfg),
                        )
                        .await
                        {
                            Ok(s) => {
                                info!("[{}] PRE-WARM: Strike fetched: ${:.2}", tag, s);
                                *strike_slot.lock().unwrap() = Some(s);
                            }
                            Err(e) => {
                                warn!("[{}] PRE-WARM: Strike fetch failed: {}", tag, e);
                            }
                        }
                        strike_in_progress.store(false, Ordering::SeqCst);
                    }
                    Err(e) => {
                        warn!(
                            "[{}] PRE-WARM: Discovery failed: {} (will fall back at rotation)",
                            tag, e,
                        );
                    }
                }
            });
        }

        // ====================================================================
        // PRE-WARM PHASE 2: Poll for completed discovery + pre-subscribe WS
        // ====================================================================
        let mut needs_presub_resubscribe = false;
        for ms in &mut market_states {
            if ms.pre_warm.ws_pre_subscribed {
                continue;
            }
            if !ms.pre_warm.discovery_launched {
                continue;
            }

            if ms.pre_warm.poll_pending() {
                if let Some(ref next) = ms.pre_warm.next_market {
                    info!(
                        "[{}] PRE-WARM: Ready -- will pre-subscribe tokens for {}",
                        ms.tag, next.unique_id,
                    );
                    ms.pre_warm.ws_pre_subscribed = true;
                    needs_presub_resubscribe = true;
                }
            }
        }

        if needs_presub_resubscribe {
            let mut all_tokens: Vec<String> = Vec::new();
            for ms in &market_states {
                all_tokens.push(ms.market.yes_token_id.clone());
                all_tokens.push(ms.market.no_token_id.clone());
                if let Some(ref next) = ms.pre_warm.next_market {
                    all_tokens.push(next.yes_token_id.clone());
                    all_tokens.push(next.no_token_id.clone());
                }
            }
            let unique_tokens: Vec<String> = {
                let mut seen = HashSet::new();
                all_tokens
                    .into_iter()
                    .filter(|t| seen.insert(t.clone()))
                    .collect()
            };
            info!(
                "[POLY] PRE-WARM: Re-subscribing {} tokens (current + pre-warm)",
                unique_tokens.len(),
            );
            poly_drain_handle.abort();
            let mut new_rx = poly_stream.clone().stream(unique_tokens).await?;
            poly_drain_handle = tokio::spawn(async move {
                while let Some(_) = new_rx.recv().await {}
            });
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // ====================================================================
        // Process each market: write SHM + optional CSV + handle rotation
        // ====================================================================
        let mut needs_resubscribe = false;
        for ms in &mut market_states {
            let cfg = &markets[ms.config_idx];
            let sym = cfg.binance_symbol.as_str();

            let snapshot = match binance_snapshots.get(sym) {
                Some(s) => s,
                None => continue,
            };
            let (binance_bid, binance_ask) = binance_bid_asks
                .get(sym)
                .copied()
                .unwrap_or((snapshot.spot_price, snapshot.spot_price));

            // Polymarket prices
            let yes_prices = poly_stream.get_prices(&ms.market.yes_token_id);
            let no_prices = poly_stream.get_prices(&ms.market.no_token_id);

            let poly_yes_update_ms = yes_prices
                .as_ref()
                .map(|p| {
                    if p.last_update_ms > 0 {
                        p.last_update_ms
                    } else {
                        now_ms
                    }
                })
                .unwrap_or(now_ms);
            let poly_no_update_ms = no_prices
                .as_ref()
                .map(|p| {
                    if p.last_update_ms > 0 {
                        p.last_update_ms
                    } else {
                        now_ms
                    }
                })
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
            let ttx_secs = ms.market.window_end - now_ts;

            let strike_val = ms.market.strike_price.unwrap_or(0.0);
            let greeks_strike = if strike_val > 0.0 {
                strike_val
            } else {
                snapshot.spot_price
            };
            let greeks = calculate_greeks(
                snapshot.spot_price,
                greeks_strike,
                ttx,
                snapshot.realized_volatility,
            );

            let yes_bid = yes_prices
                .as_ref()
                .and_then(|p| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0)))
                .unwrap_or(0.0);
            let yes_ask = yes_prices
                .as_ref()
                .and_then(|p| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0)))
                .unwrap_or(0.0);
            let no_bid = no_prices
                .as_ref()
                .and_then(|p| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0)))
                .unwrap_or(0.0);
            let no_ask = no_prices
                .as_ref()
                .and_then(|p| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0)))
                .unwrap_or(0.0);

            let yes_bid_size = yes_prices
                .as_ref()
                .map(|p| p.best_bid_size)
                .unwrap_or(0.0);
            let yes_ask_size = yes_prices
                .as_ref()
                .map(|p| p.best_ask_size)
                .unwrap_or(0.0);
            let no_bid_size = no_prices
                .as_ref()
                .map(|p| p.best_bid_size)
                .unwrap_or(0.0);
            let no_ask_size = no_prices
                .as_ref()
                .map(|p| p.best_ask_size)
                .unwrap_or(0.0);

            let yes_spread = calculate_spread_bps(yes_bid, yes_ask);
            let no_spread = calculate_spread_bps(no_bid, no_ask);

            // Resolution detection: WS -> price-based -> Gamma API fallback
            let outcome =
                if let Some(resolved_event) = market_events.take_resolved_for(&ms.market.slug) {
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
                    let cached = ms.gamma_outcome.lock().unwrap().clone();
                    if let Some(result) = cached {
                        result
                    } else if !ms.gamma_fetch_in_progress.load(Ordering::SeqCst)
                        && (now_ms - ms.gamma_last_outcome_fetch_ms >= 100)
                    {
                        ms.gamma_last_outcome_fetch_ms = now_ms;
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
                                    if gen.load(Ordering::SeqCst) == gen_at_spawn {
                                        info!("Gamma API outcome for {}: {}", slug, result);
                                        *cache.lock().unwrap() = Some(result);
                                    } else {
                                        info!(
                                            "Gamma API outcome for {} arrived after rotation, discarding",
                                            slug
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!("Gamma outcome failed for {}: {}", slug, e);
                                }
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

            // --- Order Book Depth ---
            let yes_bid_depth_3 = yes_prices.as_ref().map(|p| p.bid_depth_3).unwrap_or(0.0);
            let yes_ask_depth_3 = yes_prices.as_ref().map(|p| p.ask_depth_3).unwrap_or(0.0);
            let no_bid_depth_3 = no_prices.as_ref().map(|p| p.bid_depth_3).unwrap_or(0.0);
            let no_ask_depth_3 = no_prices.as_ref().map(|p| p.ask_depth_3).unwrap_or(0.0);
            let yes_bid_depth_all = yes_prices.as_ref().map(|p| p.bid_depth_all).unwrap_or(0.0);
            let yes_ask_depth_all = yes_prices.as_ref().map(|p| p.ask_depth_all).unwrap_or(0.0);
            let no_bid_depth_all = no_prices.as_ref().map(|p| p.bid_depth_all).unwrap_or(0.0);
            let no_ask_depth_all = no_prices.as_ref().map(|p| p.ask_depth_all).unwrap_or(0.0);
            let yes_book_imbalance = yes_prices.as_ref().map(|p| p.book_imbalance).unwrap_or(0.0);
            let no_book_imbalance = no_prices.as_ref().map(|p| p.book_imbalance).unwrap_or(0.0);
            let all_bids = yes_bid_depth_all + no_bid_depth_all;
            let all_asks = yes_ask_depth_all + no_ask_depth_all;
            let combined_obi = if (all_bids + all_asks) > 0.0 {
                (all_bids - all_asks) / (all_bids + all_asks)
            } else {
                0.0
            };

            // --- Trade Flow ---
            let yes_last_trade_price = yes_prices
                .as_ref()
                .map(|p| p.last_trade_price)
                .unwrap_or(0.0);
            let yes_last_trade_size = yes_prices
                .as_ref()
                .map(|p| p.last_trade_size)
                .unwrap_or(0.0);
            let yes_last_trade_side = yes_prices
                .as_ref()
                .map(|p| p.last_trade_side.clone())
                .unwrap_or_default();
            let no_last_trade_price = no_prices
                .as_ref()
                .map(|p| p.last_trade_price)
                .unwrap_or(0.0);
            let no_last_trade_size = no_prices
                .as_ref()
                .map(|p| p.last_trade_size)
                .unwrap_or(0.0);
            let no_last_trade_side = no_prices
                .as_ref()
                .map(|p| p.last_trade_side.clone())
                .unwrap_or_default();
            let yes_taker_buy = yes_prices
                .as_ref()
                .map(|p| p.taker_buy_volume)
                .unwrap_or(0.0);
            let yes_taker_sell = yes_prices
                .as_ref()
                .map(|p| p.taker_sell_volume)
                .unwrap_or(0.0);
            let no_taker_buy = no_prices
                .as_ref()
                .map(|p| p.taker_buy_volume)
                .unwrap_or(0.0);
            let no_taker_sell = no_prices
                .as_ref()
                .map(|p| p.taker_sell_volume)
                .unwrap_or(0.0);
            let total_buy = yes_taker_buy + no_taker_buy;
            let total_sell = yes_taker_sell + no_taker_sell;
            let trade_flow_imbalance = if (total_buy + total_sell) > 0.0 {
                (total_buy - total_sell) / (total_buy + total_sell)
            } else {
                0.0
            };

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

            let rtds_snap = rtds_per_asset
                .get(cfg.asset.as_str())
                .cloned()
                .unwrap_or_default();

            // === Write SHM slot ===
            write_slot_signals(
                shm_writer.slot(ms.shm_slot_index),
                now_us_val,
                snapshot,
                binance_bid,
                binance_ask,
                yes_bid,
                yes_ask,
                yes_bid_size,
                yes_ask_size,
                no_bid,
                no_ask,
                no_bid_size,
                no_ask_size,
                strike_val,
                ttx_secs,
                ms.market.window_start,
                ms.market.window_end,
                &greeks,
                yes_book_imbalance,
                combined_obi,
                binance_book_imbalance,
                yes_bid_depth_3,
                yes_ask_depth_3,
                no_bid_depth_3,
                no_ask_depth_3,
                yes_bid_depth_all,
                yes_ask_depth_all,
                no_bid_depth_all,
                no_ask_depth_all,
                &rtds_snap,
                yes_last_trade_price,
                yes_last_trade_size,
                no_last_trade_price,
                no_last_trade_size,
                resolved,
                &outcome,
                binance_update_ms,
                poly_yes_update_ms,
                poly_no_update_ms,
            );

            // === Write CSV (if dual-output) ===
            if args.also_csv {
                let vol_data_source = if snapshot.volatility_regime == "BOOTSTRAP" {
                    "BOOTSTRAP".to_string()
                } else {
                    snapshot.vol_source_path.clone()
                };

                let record = CsvRecord {
                    timestamp: now.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
                    market_slug: ms.market.slug.clone(),
                    binance_price: snapshot.spot_price,
                    binance_volume: sanitize(snapshot.volume_1min),
                    yes_bid,
                    yes_bid_size,
                    yes_ask,
                    yes_ask_size,
                    no_bid,
                    no_bid_size,
                    no_ask,
                    no_ask_size,
                    time_to_open,
                    time_to_expiry_minutes: ttx,
                    strike_price: strike_val,
                    realized_volatility: sanitize(snapshot.realized_volatility),
                    delta_yes: greeks.delta,
                    gamma: greeks.gamma,
                    vega: greeks.vega,
                    theta: greeks.theta,
                    market_resolved: resolved,
                    outcome: outcome.clone(),
                    timestamp_unix_ms: now_ms,
                    binance_update_ms,
                    poly_yes_update_ms,
                    poly_no_update_ms,
                    latency_binance_poly_ms: latency_ms,
                    binance_bid,
                    binance_ask,
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
                    vol_data_source,
                    yes_spread_bps: sanitize(yes_spread),
                    no_spread_bps: sanitize(no_spread),
                    rtds_binance_price: sanitize(rtds_snap.rtds_binance_price),
                    rtds_chainlink_price: sanitize(rtds_snap.rtds_chainlink_price),
                    rtds_binance_dev_bps: sanitize(rtds_snap.rtds_binance_dev_bps),
                    rtds_chainlink_dev_bps: sanitize(rtds_snap.rtds_chainlink_dev_bps),
                    rtds_price_advantage_bps: sanitize(rtds_snap.rtds_price_advantage_bps),
                    rtds_chainlink_binance_spread_bps: sanitize(
                        rtds_snap.rtds_chainlink_binance_spread_bps,
                    ),
                    rtds_binance_lag_ms: rtds_snap.rtds_binance_lag_ms,
                    rtds_chainlink_lag_ms: rtds_snap.rtds_chainlink_lag_ms,
                    rtds_binance_stale_ms: rtds_snap.rtds_binance_stale_ms,
                    rtds_chainlink_stale_ms: rtds_snap.rtds_chainlink_stale_ms,
                    yes_bid_depth_3: sanitize(yes_bid_depth_3),
                    yes_ask_depth_3: sanitize(yes_ask_depth_3),
                    no_bid_depth_3: sanitize(no_bid_depth_3),
                    no_ask_depth_3: sanitize(no_ask_depth_3),
                    yes_bid_depth_all: sanitize(yes_bid_depth_all),
                    yes_ask_depth_all: sanitize(yes_ask_depth_all),
                    no_bid_depth_all: sanitize(no_bid_depth_all),
                    no_ask_depth_all: sanitize(no_ask_depth_all),
                    yes_bid_levels: yes_prices
                        .as_ref()
                        .map(|p| p.bid_levels_count)
                        .unwrap_or(0),
                    yes_ask_levels: yes_prices
                        .as_ref()
                        .map(|p| p.ask_levels_count)
                        .unwrap_or(0),
                    no_bid_levels: no_prices
                        .as_ref()
                        .map(|p| p.bid_levels_count)
                        .unwrap_or(0),
                    no_ask_levels: no_prices
                        .as_ref()
                        .map(|p| p.ask_levels_count)
                        .unwrap_or(0),
                    yes_book_imbalance: sanitize(yes_book_imbalance),
                    no_book_imbalance: sanitize(no_book_imbalance),
                    combined_obi: sanitize(combined_obi),
                    yes_last_trade_price: sanitize(yes_last_trade_price),
                    yes_last_trade_size: sanitize(yes_last_trade_size),
                    yes_last_trade_side,
                    no_last_trade_price: sanitize(no_last_trade_price),
                    no_last_trade_size: sanitize(no_last_trade_size),
                    no_last_trade_side,
                    trade_flow_imbalance: sanitize(trade_flow_imbalance),
                    binance_bid_depth_3: sanitize(binance_bid_depth_3),
                    binance_ask_depth_3: sanitize(binance_ask_depth_3),
                    binance_bid_depth_all: sanitize(binance_bid_depth_all),
                    binance_ask_depth_all: sanitize(binance_ask_depth_all),
                    binance_bid_levels,
                    binance_ask_levels,
                    binance_book_imbalance: sanitize(binance_book_imbalance),
                    binance_poly_obi_divergence: sanitize(binance_poly_obi_divergence),
                };

                if let Some(ref mut w) = ms.csv_writer {
                    w.write_record(&record)?;
                }
            }

            // --- Compact logging (every 100 ticks) ---
            if tick % 100 == 0 {
                let lag_display = {
                    let yes_has_data = yes_prices
                        .as_ref()
                        .map(|p| p.last_update_ms > 0)
                        .unwrap_or(false);
                    let no_has_data = no_prices
                        .as_ref()
                        .map(|p| p.last_update_ms > 0)
                        .unwrap_or(false);
                    if yes_has_data && no_has_data {
                        format!("{}ms", latency_ms)
                    } else {
                        "N/A".to_string()
                    }
                };
                let prewarm_status = if ms.pre_warm.next_market.is_some() {
                    " [PREWARM READY]"
                } else if ms.pre_warm.discovery_launched {
                    " [PREWARM PENDING]"
                } else {
                    ""
                };
                info!(
                    "[{}] #{} {} TTX={:.1}m | ${:.0} | Y:{:.2}@{:.0} N:{:.2}@{:.0} | Vol:{:.1}% D:{:.4} | Lag={} | {} | SHM:slot{}{}",
                    ms.tag,
                    tick,
                    ms.market.unique_id,
                    ttx,
                    snapshot.spot_price,
                    yes_bid,
                    yes_bid_size,
                    no_bid,
                    no_bid_size,
                    snapshot.realized_volatility * 100.0,
                    greeks.delta,
                    lag_display,
                    snapshot.volatility_regime,
                    ms.shm_slot_index,
                    prewarm_status,
                );
            }

            // --- Market rotation ---
            if resolved && ms.resolution_grace_start_ms == 0 {
                ms.resolution_grace_start_ms = now_ms;
                info!(
                    "[{}] Market {} expired -- starting {}s resolution grace period",
                    ms.tag, ms.market.unique_id, RESOLUTION_GRACE_SECS,
                );
            }

            let grace_expired = ms.resolution_grace_start_ms > 0
                && (now_ms - ms.resolution_grace_start_ms) >= RESOLUTION_GRACE_MS;
            let has_outcome = !outcome.is_empty();

            if resolved && (has_outcome || grace_expired) {
                let time_since_last = if ms.last_fetch_attempt_ms > 0 {
                    now_ms.saturating_sub(ms.last_fetch_attempt_ms)
                } else {
                    i64::MAX
                };

                let should_attempt = ms.consecutive_fetch_failures == 0
                    || (time_since_last >= FETCH_RETRY_COOLDOWN_MS
                        && ms.consecutive_fetch_failures < MAX_CONSECUTIVE_FETCH_FAILURES);

                if !should_attempt {
                    continue;
                }

                if grace_expired && !has_outcome {
                    warn!(
                        "[{}] Resolution grace period expired with no outcome -- forcing rotation",
                        ms.tag
                    );
                } else if has_outcome {
                    info!("[{}] Outcome '{}' detected -- rotating", ms.tag, outcome);
                }

                {
                    ms.last_fetch_attempt_ms = now_ms;
                    info!(
                        "[{}] Market {} expired (TTX={:.1}m) -- rotating...",
                        ms.tag, ms.market.unique_id, ttx
                    );

                    // --- Ensure outcome is captured before rotation ---
                    if outcome.is_empty() {
                        info!(
                            "[{}] No outcome yet -- blocking Gamma fetch before rotation",
                            ms.tag
                        );
                        let final_outcome = match tokio::time::timeout(
                            Duration::from_secs(3),
                            gamma::fetch_outcome(&http, &ms.market.slug),
                        )
                        .await
                        {
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
                            // Write final outcome to SHM
                            let slot = shm_writer.slot(ms.shm_slot_index);
                            slot.begin_write();
                            slot.market_resolved.store(1, Ordering::Relaxed);
                            AssetSlot::write_string(&slot.outcome, &final_outcome);
                            slot.end_write();

                            // Write final CSV record if dual-output
                            if args.also_csv {
                                if let Some(ref mut w) = ms.csv_writer {
                                    // Build a minimal final record with outcome
                                    let vol_data_source =
                                        if snapshot.volatility_regime == "BOOTSTRAP" {
                                            "BOOTSTRAP".to_string()
                                        } else {
                                            snapshot.vol_source_path.clone()
                                        };
                                    let final_record = CsvRecord {
                                        timestamp: now
                                            .format("%Y-%m-%dT%H:%M:%S%.3f")
                                            .to_string(),
                                        market_slug: ms.market.slug.clone(),
                                        binance_price: snapshot.spot_price,
                                        binance_volume: sanitize(snapshot.volume_1min),
                                        yes_bid,
                                        yes_bid_size,
                                        yes_ask,
                                        yes_ask_size,
                                        no_bid,
                                        no_bid_size,
                                        no_ask,
                                        no_ask_size,
                                        time_to_open,
                                        time_to_expiry_minutes: ttx,
                                        strike_price: strike_val,
                                        realized_volatility: sanitize(
                                            snapshot.realized_volatility,
                                        ),
                                        delta_yes: greeks.delta,
                                        gamma: greeks.gamma,
                                        vega: greeks.vega,
                                        theta: greeks.theta,
                                        market_resolved: true,
                                        outcome: final_outcome.clone(),
                                        timestamp_unix_ms: now_ms,
                                        binance_update_ms,
                                        poly_yes_update_ms,
                                        poly_no_update_ms,
                                        latency_binance_poly_ms: latency_ms,
                                        binance_bid,
                                        binance_ask,
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
                                        vol_data_source,
                                        yes_spread_bps: sanitize(yes_spread),
                                        no_spread_bps: sanitize(no_spread),
                                        rtds_binance_price: sanitize(
                                            rtds_snap.rtds_binance_price,
                                        ),
                                        rtds_chainlink_price: sanitize(
                                            rtds_snap.rtds_chainlink_price,
                                        ),
                                        rtds_binance_dev_bps: sanitize(
                                            rtds_snap.rtds_binance_dev_bps,
                                        ),
                                        rtds_chainlink_dev_bps: sanitize(
                                            rtds_snap.rtds_chainlink_dev_bps,
                                        ),
                                        rtds_price_advantage_bps: sanitize(
                                            rtds_snap.rtds_price_advantage_bps,
                                        ),
                                        rtds_chainlink_binance_spread_bps: sanitize(
                                            rtds_snap.rtds_chainlink_binance_spread_bps,
                                        ),
                                        rtds_binance_lag_ms: rtds_snap.rtds_binance_lag_ms,
                                        rtds_chainlink_lag_ms: rtds_snap.rtds_chainlink_lag_ms,
                                        rtds_binance_stale_ms: rtds_snap.rtds_binance_stale_ms,
                                        rtds_chainlink_stale_ms: rtds_snap
                                            .rtds_chainlink_stale_ms,
                                        yes_bid_depth_3: sanitize(yes_bid_depth_3),
                                        yes_ask_depth_3: sanitize(yes_ask_depth_3),
                                        no_bid_depth_3: sanitize(no_bid_depth_3),
                                        no_ask_depth_3: sanitize(no_ask_depth_3),
                                        yes_bid_depth_all: sanitize(yes_bid_depth_all),
                                        yes_ask_depth_all: sanitize(yes_ask_depth_all),
                                        no_bid_depth_all: sanitize(no_bid_depth_all),
                                        no_ask_depth_all: sanitize(no_ask_depth_all),
                                        yes_bid_levels: yes_prices
                                            .as_ref()
                                            .map(|p| p.bid_levels_count)
                                            .unwrap_or(0),
                                        yes_ask_levels: yes_prices
                                            .as_ref()
                                            .map(|p| p.ask_levels_count)
                                            .unwrap_or(0),
                                        no_bid_levels: no_prices
                                            .as_ref()
                                            .map(|p| p.bid_levels_count)
                                            .unwrap_or(0),
                                        no_ask_levels: no_prices
                                            .as_ref()
                                            .map(|p| p.ask_levels_count)
                                            .unwrap_or(0),
                                        yes_book_imbalance: sanitize(yes_book_imbalance),
                                        no_book_imbalance: sanitize(no_book_imbalance),
                                        combined_obi: sanitize(combined_obi),
                                        yes_last_trade_price: sanitize(yes_last_trade_price),
                                        yes_last_trade_size: sanitize(yes_last_trade_size),
                                        yes_last_trade_side: yes_prices
                                            .as_ref()
                                            .map(|p| p.last_trade_side.clone())
                                            .unwrap_or_default(),
                                        no_last_trade_price: sanitize(no_last_trade_price),
                                        no_last_trade_size: sanitize(no_last_trade_size),
                                        no_last_trade_side: no_prices
                                            .as_ref()
                                            .map(|p| p.last_trade_side.clone())
                                            .unwrap_or_default(),
                                        trade_flow_imbalance: sanitize(trade_flow_imbalance),
                                        binance_bid_depth_3: sanitize(binance_bid_depth_3),
                                        binance_ask_depth_3: sanitize(binance_ask_depth_3),
                                        binance_bid_depth_all: sanitize(binance_bid_depth_all),
                                        binance_ask_depth_all: sanitize(binance_ask_depth_all),
                                        binance_bid_levels,
                                        binance_ask_levels,
                                        binance_book_imbalance: sanitize(
                                            binance_book_imbalance,
                                        ),
                                        binance_poly_obi_divergence: sanitize(
                                            binance_poly_obi_divergence,
                                        ),
                                    };
                                    w.write_record(&final_record)?;
                                    info!(
                                        "[{}] Final resolution record written: {}",
                                        ms.tag, final_outcome
                                    );
                                }
                            }
                        }
                    }

                    // === ROTATION: Use pre-discovered market or fall back ===
                    let next_market_result = if let Some((pre_market, pre_strike)) =
                        ms.pre_warm.take()
                    {
                        info!(
                            "[{}] PRE-WARM ROTATION: Using pre-discovered {} (zero cold start)",
                            ms.tag, pre_market.unique_id,
                        );
                        let mut market = pre_market;
                        if let Some(s) = pre_strike {
                            market.strike_price = Some(s);
                            market.strike_fetched = true;
                        }
                        Ok(market)
                    } else {
                        info!("[{}] No pre-warm -- standard rotation", ms.tag);
                        let slug_prefix = format!(
                            "{}-updown-{}-",
                            cfg.asset,
                            timeframe_label(cfg.timeframe_secs),
                        );
                        match market_events.take_new_market_for(&slug_prefix) {
                            Some(nm)
                                if nm.outcomes.len() == 2 && nm.asset_ids.len() == 2 =>
                            {
                                info!(
                                    "[{}] WS new_market: {} (instant, no HTTP)",
                                    ms.tag, nm.slug
                                );
                                let next_ws = ms.market.window_start + cfg.timeframe_secs;
                                let up_idx = nm.outcomes.iter().position(|o| o == "Up");
                                let down_idx = nm.outcomes.iter().position(|o| o == "Down");
                                let (yes_id, no_id) = match (up_idx, down_idx) {
                                    (Some(ui), Some(di)) => {
                                        (nm.asset_ids[ui].clone(), nm.asset_ids[di].clone())
                                    }
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
                                gamma::fetch_next_market_for(
                                    &http,
                                    &cfg.asset,
                                    cfg.timeframe_secs,
                                    ms.market.window_start,
                                )
                                .await
                            }
                        }
                    };

                    match next_market_result {
                        Ok(next_market) => {
                            info!("[{}] Rotated to: {}", ms.tag, next_market.unique_id);

                            // Remove old tokens from price cache
                            let old_tokens = vec![
                                ms.market.yes_token_id.clone(),
                                ms.market.no_token_id.clone(),
                            ];
                            poly_stream.remove_tokens(&old_tokens);

                            let strike_already_fetched = next_market.strike_fetched;
                            ms.market = next_market;
                            ms.gamma_generation.fetch_add(1, Ordering::SeqCst);
                            *ms.gamma_outcome.lock().unwrap() = None;
                            ms.gamma_fetch_in_progress.store(false, Ordering::SeqCst);
                            ms.consecutive_fetch_failures = 0;
                            ms.gamma_last_outcome_fetch_ms = 0;
                            ms.resolution_grace_start_ms = 0;

                            ms.pre_warm.reset();

                            // Update SHM slot: new identity + reset signals + bump epoch
                            let slot = shm_writer.slot(ms.shm_slot_index);

                            // Reset all signal fields
                            reset_slot_signals(slot);

                            // Write new identity
                            write_slot_identity(
                                slot,
                                &ms.market,
                                &cfg.asset,
                                cfg.timeframe_secs,
                            );

                            // Bump epoch
                            slot.market_epoch.fetch_add(1, Ordering::Relaxed);

                            // Strike: if not pre-fetched, fetch in background
                            if !strike_already_fetched {
                                let http_clone = http.clone();
                                let market_clone = ms.market.clone();
                                let tag_clone = ms.tag.clone();
                                let strike_slot = ms.pre_warm.pending_strike.clone();

                                info!(
                                    "[{}] Strike fetch in background (strike_price=0.0 until ready)",
                                    tag_clone,
                                );
                                ms.pre_warm
                                    .strike_fetch_in_progress
                                    .store(true, Ordering::SeqCst);

                                tokio::spawn(async move {
                                    let retry_cfg = strike::RetryConfig {
                                        max_attempts: None,
                                        initial_backoff_ms: 500,
                                        max_backoff_ms: 2000,
                                    };
                                    match strike::fetch_strike_with_retry(
                                        &http_clone,
                                        &market_clone,
                                        Some(retry_cfg),
                                    )
                                    .await
                                    {
                                        Ok(s) => {
                                            info!(
                                                "[{}] Background strike: ${:.2}",
                                                tag_clone, s
                                            );
                                            *strike_slot.lock().unwrap() = Some(s);
                                        }
                                        Err(e) => {
                                            warn!(
                                                "[{}] Background strike failed: {}",
                                                tag_clone, e
                                            );
                                        }
                                    }
                                });
                            } else {
                                info!(
                                    "[{}] Strike: ${:.2} (pre-fetched)",
                                    ms.tag,
                                    ms.market.strike_price.unwrap_or(0.0),
                                );
                            }

                            needs_resubscribe = true;
                        }
                        Err(e) => {
                            ms.consecutive_fetch_failures += 1;
                            warn!(
                                "[{}] Rotation failed ({}/{}): {}",
                                ms.tag,
                                ms.consecutive_fetch_failures,
                                MAX_CONSECUTIVE_FETCH_FAILURES,
                                e
                            );
                        }
                    }
                }
            }
        }

        // Re-subscribe ALL active tokens after rotation
        if needs_resubscribe {
            let all_tokens: Vec<String> = market_states
                .iter()
                .flat_map(|ms| {
                    vec![
                        ms.market.yes_token_id.clone(),
                        ms.market.no_token_id.clone(),
                    ]
                })
                .collect();
            info!(
                "[POLY] Re-subscribing all {} tokens after rotation",
                all_tokens.len()
            );
            poly_drain_handle.abort();
            let mut new_rx = poly_stream.clone().stream(all_tokens).await?;
            poly_drain_handle =
                tokio::spawn(async move { while let Some(_) = new_rx.recv().await {} });
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        // ====================================================================
        // BACKFILL: Check if background strike fetch completed for any market
        // ====================================================================
        for ms in &mut market_states {
            if !ms.market.strike_fetched
                && ms
                    .pre_warm
                    .strike_fetch_in_progress
                    .load(Ordering::SeqCst)
            {
                if let Some(s) = ms.pre_warm.pending_strike.lock().unwrap().take() {
                    ms.market.strike_price = Some(s);
                    ms.market.strike_fetched = true;
                    ms.pre_warm
                        .strike_fetch_in_progress
                        .store(false, Ordering::SeqCst);
                    info!("[{}] Strike backfilled: ${:.2}", ms.tag, s);
                }
            }
        }

        // RTDS summary (every 500 ticks)
        if tick % 500 == 0 {
            for asset in &unique_assets {
                if let Some(rtds) = rtds_per_asset.get(asset.as_str()) {
                    let (asset_b, asset_c) = multi_rtds.update_counts(asset);
                    info!(
                        "[RTDS] {:<3} price=B:{:.4}/C:{:.4} dev=B:{:+.1}/C:{:+.1}bps stale=B:{}ms/C:{}ms n=B:{}/C:{}",
                        asset.to_uppercase(),
                        rtds.rtds_binance_price,
                        rtds.rtds_chainlink_price,
                        rtds.rtds_binance_dev_bps,
                        rtds.rtds_chainlink_dev_bps,
                        rtds.rtds_binance_stale_ms,
                        rtds.rtds_chainlink_stale_ms,
                        asset_b,
                        asset_c,
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

    // Flush CSV files (if dual-output)
    if args.also_csv {
        for ms in &mut market_states {
            if let Some(ref mut w) = ms.csv_writer {
                w.flush()?;
            }
        }
    }

    info!(
        "Signal engine complete! {} markets tracked, {} ticks",
        market_states.len(),
        tick
    );
    println!("\nDone!");

    Ok(())
}
