// multi_market_logger_hourly_fixed.rs - HOURLY CSV ROTATION (FIXED BINANCE EVENT TIME)
//! Enhanced Multi-Market Data Logger with Hourly CSV Rotation
//! //  cargo run --bin multi_market_logger_hourly_fixed -- 172800 btc_data_fixed.csv 1
//!
//! IDENTICAL to multi_market_logger_hourly.rs except uses binance_fixed module
//! which preserves Binance exchange-level event_time for RTDS edge detection.
//! Run both loggers side-by-side to compare RTDS lag/stale data quality.

use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use rust_decimal::prelude::*;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use combine_fetcher::api::binance_fixed::BinanceDataManager;
use combine_fetcher::api::gamma;
use combine_fetcher::api::market_events::MarketEventStream;
use combine_fetcher::api::polymarket::PolymarketOrderbookStream;
use combine_fetcher::api::rtds_tracker::RtdsTracker;
use combine_fetcher::api::strike;
use combine_fetcher::{CSV_RECORD_INTERVAL_SEC, HTTP_TIMEOUT_SECS, MINUTES_PER_YEAR};

// ============================================================================
// CSV RECORD - BACKWARD COMPATIBLE + ENHANCED
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct CsvRecord {
    // === ORIGINAL 23 COLUMNS (PRESERVED ORDER) ===
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

    // === NEW COLUMNS (APPENDED FOR BACKWARD COMPATIBILITY) ===

    // Latency tracking
    pub timestamp_unix_ms: i64,
    pub binance_update_ms: i64,
    pub poly_yes_update_ms: i64,
    pub poly_no_update_ms: i64,
    pub latency_binance_poly_ms: i64,

    // Binance orderbook detail
    pub binance_bid: f64,
    pub binance_ask: f64,
    pub binance_spread_bps: f64,

    // Multi-timeframe volatility
    pub vol_1sec: f64,
    pub vol_5sec: f64,
    pub vol_13sec: f64,
    pub vol_34sec: f64,
    pub vol_89sec: f64,
    pub volatility_regime: String,

    // Granular volume
    pub volume_1sec: f64,
    pub volume_spike: bool,

    // Signal quality
    pub signal_quality: String,
    pub trade_confidence: f64,
    pub vol_data_source: String,

    // Polymarket spreads
    pub yes_spread_bps: f64,
    pub no_spread_bps: f64,

    // === RTDS PRICE FEED COLUMNS (v2, clock-safe) ===
    pub rtds_binance_price: f64,
    pub rtds_chainlink_price: f64,
    // ✅ Clock-safe (pure price math — USE FOR TRADING)
    pub rtds_binance_dev_bps: f64,
    pub rtds_chainlink_dev_bps: f64,
    pub rtds_price_advantage_bps: f64,
    pub rtds_chainlink_binance_spread_bps: f64,
    // ⚠️ Approximate (~50ms NTP error — USE FOR MONITORING)
    pub rtds_binance_lag_ms: i64,
    pub rtds_chainlink_lag_ms: i64,
    pub rtds_binance_stale_ms: i64,
    pub rtds_chainlink_stale_ms: i64,
}

impl CsvRecord {
    pub fn csv_header() -> &'static str {
        // ORIGINAL 23 columns first (exact order preserved)
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
        rtds_binance_stale_ms,rtds_chainlink_stale_ms"
    }

    pub fn to_csv_line(&self) -> String {
        format!(
            // ORIGINAL 23 columns (exact format preserved)
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
            {:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{},{}",
            // Original columns
            self.timestamp,
            self.market_slug,
            self.binance_price,
            self.binance_volume,
            self.yes_bid,
            self.yes_bid_size,
            self.yes_ask,
            self.yes_ask_size,
            self.no_bid,
            self.no_bid_size,
            self.no_ask,
            self.no_ask_size,
            self.time_to_open,
            self.time_to_expiry_minutes,
            self.strike_price,
            self.realized_volatility,
            self.delta_yes,
            self.gamma,
            self.vega,
            self.theta,
            self.market_resolved,
            self.outcome,
            // New latency columns
            self.timestamp_unix_ms,
            self.binance_update_ms,
            self.poly_yes_update_ms,
            self.poly_no_update_ms,
            self.latency_binance_poly_ms,
            // New Binance orderbook columns
            self.binance_bid,
            self.binance_ask,
            self.binance_spread_bps,
            // New volatility columns
            self.vol_1sec,
            self.vol_5sec,
            self.vol_13sec,
            self.vol_34sec,
            self.vol_89sec,
            self.volatility_regime,
            // New volume columns
            self.volume_1sec,
            self.volume_spike,
            // New signal quality columns
            self.signal_quality,
            self.trade_confidence,
            self.vol_data_source,
            // New spread columns
            self.yes_spread_bps,
            self.no_spread_bps,
            self.rtds_binance_price,
            self.rtds_chainlink_price,
            self.rtds_binance_dev_bps,
            self.rtds_chainlink_dev_bps,
            self.rtds_price_advantage_bps,
            self.rtds_chainlink_binance_spread_bps,
            self.rtds_binance_lag_ms,
            self.rtds_chainlink_lag_ms,
            self.rtds_binance_stale_ms,
            self.rtds_chainlink_stale_ms,
        )
    }
}

// ============================================================================
// HOURLY FILE ROTATION
// ============================================================================

/// Generates hourly CSV filename with timestamp
/// Example: "output.csv" -> "output_2024-01-15_10.csv"
fn generate_hourly_filename(base_path: &str, hour_time: DateTime<Utc>) -> String {
    let path = Path::new(base_path);

    // Extract file stem and extension
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("data");
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("csv");

    // Format: {stem}_{YYYY-MM-DD}_{HH}.{ext}
    let timestamp = hour_time.format("%Y-%m-%d_%H");
    format!("{}_{}.{}", stem, timestamp, extension)
}

/// Creates or re-opens an hourly CSV file.
///
/// SAFETY FIXES (v5.2):
///   1. Uses append mode — restarts continue writing, no data overwritten.
///      With O_APPEND, each write() syscall atomically seeks to EOF first,
///      so even if two processes write simultaneously, records won't fuse.
///   2. Only writes header if file is new/empty — no duplicate headers
///   3. Uses .lock file to detect concurrent writers
fn open_new_hourly_file(path: &str) -> Result<BufWriter<std::fs::File>> {
    // Concurrent-writer detection via lock file
    let lock_path = format!("{}.lock", path);
    let my_pid = std::process::id();

    if let Ok(existing_pid) = std::fs::read_to_string(&lock_path) {
        let existing_pid = existing_pid.trim();
        // Check if the PID is still alive
        let pid_alive = existing_pid
            .parse::<u32>()
            .ok()
            .map(|pid| std::path::Path::new(&format!("/proc/{}", pid)).exists())
            .unwrap_or(false);

        if pid_alive && existing_pid != my_pid.to_string() {
            warn!(
                "⚠️  Lock file {} exists with PID {} (still alive). \
                 Another logger may be writing to {}. Proceeding with append mode \
                 (O_APPEND prevents corruption, but data may be duplicated).",
                lock_path, existing_pid, path
            );
        }
    }

    // Write our PID to lock file
    let _ = std::fs::write(&lock_path, my_pid.to_string());

    // Check if file already has content (from a previous run this hour)
    let file_exists_with_data = std::fs::metadata(path)
        .map(|m| m.len() > 0)
        .unwrap_or(false);

    let file = OpenOptions::new()
        .create(true)
        .append(true) // KEY FIX: O_APPEND makes each write() atomic to EOF
        .open(path)?;

    let mut writer = BufWriter::with_capacity(64 * 1024, file);

    // Only write header if file is new or empty
    if !file_exists_with_data {
        writeln!(writer, "{}", CsvRecord::csv_header())?;
        writer.flush()?;
    } else {
        info!("Appending to existing file: {} (header already present)", path);
    }

    Ok(writer)
}

/// Tracks current CSV file and hour for rotation
struct HourlyCsvWriter {
    base_path: String,
    current_file: Option<BufWriter<std::fs::File>>,
    current_file_path: String,
    current_hour: u32,
    records_since_flush: u32,
    last_flush_ms: i64,
}

impl HourlyCsvWriter {
    fn new(base_path: String) -> Self {
        Self {
            base_path,
            current_file: None,
            current_file_path: String::new(),
            current_hour: u32::MAX, // Sentinel value indicating uninitialized
            records_since_flush: 0,
            last_flush_ms: 0,
        }
    }

    /// Check if hour has changed and rotate if needed
    /// Returns true if file was rotated
    fn check_rotation(&mut self, now: &DateTime<Utc>) -> Result<bool> {
        let hour = now.hour();

        // First time initialization (using u32::MAX as sentinel)
        if self.current_hour == u32::MAX {
            self.rotate(now)?;
            return Ok(true);
        }

        // Check if hour changed
        if hour != self.current_hour {
            self.rotate(now)?;
            return Ok(true);
        }

        Ok(false)
    }

    /// Rotate to new hourly file
    fn rotate(&mut self, now: &DateTime<Utc>) -> Result<()> {
        // Flush and close current file if exists
        if let Some(mut file) = self.current_file.take() {
            file.flush()?;
            // Remove lock file for the old hour
            let _ = std::fs::remove_file(format!("{}.lock", self.current_file_path));
            info!("Rotated CSV file: {}", self.current_file_path);
        }

        // Generate new filename
        let new_path = generate_hourly_filename(&self.base_path, *now);
        self.current_file_path = new_path.clone();
        self.current_hour = now.hour();

        // Reset flush counters for new file
        self.records_since_flush = 0;
        self.last_flush_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // Open new file
        self.current_file = Some(open_new_hourly_file(&new_path)?);

        info!("New hourly CSV file: {}", new_path);

        Ok(())
    }

    /// Write a record to the current file
    ///
    /// Builds the full line (including newline) as a single String, then writes
    /// it via write_all so the BufWriter flushes it as one write() syscall.
    /// Combined with O_APPEND, this guarantees records are never fused even
    /// if two processes write to the same file.
    ///
    /// Flushes every 100 records OR every 1 second, whichever comes first.
    /// This avoids 100+ flush syscalls/sec in event-driven mode (interval=0).
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

    /// Flush current file before exit
    fn flush(&mut self) -> Result<()> {
        if let Some(mut file) = self.current_file.take() {
            file.flush()?;
            let _ = std::fs::remove_file(format!("{}.lock", self.current_file_path));
            info!("Final CSV file flushed: {}", self.current_file_path);
        }
        Ok(())
    }
}

// ============================================================================
// HELPER FUNCTIONS
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

// ============================================================================
// GREEKS
// ============================================================================

#[derive(Debug, Clone, Copy, Default)]
pub struct Greeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub theta: f64,
}

fn calculate_greeks(spot: f64, strike: f64, ttx_min: f64, vol: f64) -> Greeks {
    // NaN guard: IEEE 754 NaN comparisons are always false, so
    // `vol <= 0.0` passes when vol is NaN. Check explicitly.
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

    let norm_pdf = |x: f64| -> f64 { (-x * x / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt() };

    let n_prime_d2 = norm_pdf(d2);

    // Clamp to finite values — if d1/d2 overflow, norm_pdf returns ~0 but
    // intermediate products can still be Inf/NaN
    let delta = (n_prime_d2 / (spot * vol * sqrt_t)).clamp(0.0, 1.0);
    let gamma = (n_prime_d2 * d1 / (spot * spot * vol * vol * t)).clamp(-1e6, 1e6);
    let vega = (n_prime_d2 * sqrt_t * 0.01).clamp(-1e6, 1e6);
    let theta = (-(spot * n_prime_d2 * vol) / (2.0 * sqrt_t) / 365.0).clamp(-1e6, 1e6);

    Greeks {
        delta: if delta.is_finite() { delta } else { 0.0 },
        gamma: if gamma.is_finite() { gamma } else { 0.0 },
        vega: if vega.is_finite() { vega } else { 0.0 },
        theta: if theta.is_finite() { theta } else { 0.0 },
    }
}

// ============================================================================
// HELPER: Truncate token ID for display
// ============================================================================

pub fn truncate_id(id: &str, first: usize, last: usize) -> String {
    let len = id.len();
    if len <= first + last {
        return id.to_string();
    }
    format!("{}...{}", &id[..first], &id[len - last..])
}

// ============================================================================
// MAIN - ENHANCED DATA COLLECTION LOOP WITH HOURLY ROTATION
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("multi_market_logger_hourly=info".parse().unwrap())
                .add_directive("combine_fetcher=info".parse().unwrap()),
        )
        .init();

    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║ Enhanced Multi-Market Data Logger (Hourly Rotation)       ║");
    println!("║ Original 23 cols + 22 new cols + HOURLY FILE ROTATION    ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");

    let args: Vec<String> = std::env::args().collect();
    let duration: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(28800);
    let csv_path = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "overnight_btc_data.csv".into());
    let csv_record_interval_sec: u64 = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(CSV_RECORD_INTERVAL_SEC);

    println!(
        "Duration: {}s ({:.1}h) | Output Base: {} | Interval: {}s\n",
        duration,
        duration as f64 / 3600.0,
        csv_path,
        csv_record_interval_sec
    );
    println!("ℹ️  CSV files will rotate hourly with format: <base>_YYYY-MM-DD_HH.csv");

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(3))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?;

    println!("⏳ Fetching current market...");
    let mut current_market = gamma::fetch_current_market(&http).await?;
    println!(
        "✅ Market: {} | YES: {}... | NO: {}...",
        current_market.unique_id,
        truncate_id(&current_market.yes_token_id, 10, 0),
        truncate_id(&current_market.no_token_id, 10, 0)
    );

    // Setup Binance
    let mut binance_manager = BinanceDataManager::new();
    binance_manager.connect(&["BTCUSDT"], None).await?;
    println!("✅ Binance connected");

    // Bootstrap volatility with historical data
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("⏳ Bootstrapping volatility with historical data...");
    match binance_manager.bootstrap_volatility("BTCUSDT", 70).await {
        Ok(count) => println!("✅ Bootstrapped with {} 1-min candles", count),
        Err(e) => warn!("⚠️ Bootstrap failed: {} - will use defaults", e),
    }

    // Fetch strike with infinite retry BEFORE starting data collection
    println!(
        "⏳ Fetching strike price for {}...",
        current_market.unique_id
    );
    println!("ℹ️  NOTE: Will retry indefinitely until strike price is available (exponential backoff, max 30s)");
    let strike = strike::fetch_strike_with_retry(&http, &current_market, None).await?;
    current_market.strike_price = Some(strike);
    current_market.strike_fetched = true;
    println!("✅ Strike price: ${:.2} - ready to start logging", strike);

    // Setup Polymarket stream
    let poly_stream = Arc::new(PolymarketOrderbookStream::new());
    let token_ids = vec![
        current_market.yes_token_id.clone(),
        current_market.no_token_id.clone(),
    ];

    let mut rx = poly_stream.clone().stream(token_ids).await?;

    // Spawn task to consume Polymarket updates (they update internal cache)
    tokio::spawn(async move {
        while let Some(_update) = rx.recv().await {
            // Updates are handled internally by PolymarketOrderbookStream
        }
    });

    println!("✅ Polymarket connected");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Setup RTDS price feed tracker
    let mut rtds_tracker = RtdsTracker::new();
    rtds_tracker.start();
    println!("✅ RTDS tracker connected (Binance relay + Chainlink oracle)");
    tokio::time::sleep(Duration::from_secs(2)).await; // Wait for first RTDS updates

    // Setup market lifecycle event stream (new_market + market_resolved via WS)
    let market_events = MarketEventStream::new();
    market_events.start(vec![
        current_market.yes_token_id.clone(),
        current_market.no_token_id.clone(),
    ]);
    println!("✅ Market event stream connected (new_market + market_resolved)");

    // Initialize hourly CSV writer
    let mut csv_writer = HourlyCsvWriter::new(csv_path.clone());

    // Initial file creation
    let now = Utc::now();
    csv_writer.check_rotation(&now)?;
    println!("✅ Initial CSV file created");

    let start_time = SystemTime::now();
    let end_time = start_time + Duration::from_secs(duration);
    let mut tick: u64 = 0;
    let mut last_log_ms: i64 = 0;
    let mut markets_tracked: usize = 1;

    // Track consecutive market fetch failures to prevent tight retry loops
    let mut consecutive_fetch_failures: u32 = 0;
    const MAX_CONSECUTIVE_FETCH_FAILURES: u32 = 10;
    let mut last_fetch_attempt_ms: i64 = 0;
    const FETCH_RETRY_COOLDOWN_MS: i64 = 5000;

    // Grace period: keep polling for outcome after TTX <= 0 before rotating
    const RESOLUTION_GRACE_SECS: i64 = 30;
    const RESOLUTION_GRACE_MS: i64 = RESOLUTION_GRACE_SECS * 1000;
    let mut resolution_grace_start_ms: i64 = 0;

    // H3 FIX: Signal handling — graceful shutdown on SIGINT
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("🛑 Received SIGINT, shutting down gracefully...");
        shutdown_signal.store(true, Ordering::SeqCst);
    });

    // C6: Gamma API outcome cache (non-blocking resolution detection)
    let gamma_outcome: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let gamma_fetch_in_progress = Arc::new(AtomicBool::new(false));
    // Generation counter: prevents stale outcome from previous market leaking into new one
    let gamma_generation = Arc::new(AtomicU64::new(0));
    let mut gamma_last_outcome_fetch_ms: i64 = 0;

    info!("Starting enhanced data collection with hourly rotation...");

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

        // === HOURLY ROTATION CHECK ===
        // Check if hour has changed and rotate CSV file if needed
        if csv_writer.check_rotation(&now)? {
            info!("🔄 Hourly CSV rotation completed");
        }

        // Get MarketSnapshot (contains all Binance data)
        let snapshot = match binance_manager.get_market_snapshot("BTCUSDT").await {
            Some(s) => s,
            None => continue,
        };

        // CSV recording frequency control
        let should_record = if csv_record_interval_sec == 0 {
            const MIN_EVENT_INTERVAL_MS: i64 = 10;
            now_ms - last_log_ms >= MIN_EVENT_INTERVAL_MS
        } else {
            let interval_ms = (csv_record_interval_sec * 1000) as i64;
            now_ms - last_log_ms >= interval_ms
        };

        if !should_record {
            continue;
        }

        last_log_ms = now_ms;
        let now_ts = now.timestamp();

        // Get Polymarket prices (from internal cache)
        let yes_prices = poly_stream.get_prices(&current_market.yes_token_id);
        let no_prices = poly_stream.get_prices(&current_market.no_token_id);

        // Extract Polymarket update timestamps
        let poly_yes_update_ms = yes_prices
            .as_ref()
            .map(|p| p.last_update_ms)
            .unwrap_or(now_ms);
        let poly_no_update_ms = no_prices
            .as_ref()
            .map(|p| p.last_update_ms)
            .unwrap_or(now_ms);

        // Binance update time from snapshot
        let binance_update_ms = snapshot.timestamp;

        // Calculate latency differential
        let poly_avg_update = (poly_yes_update_ms + poly_no_update_ms) / 2;
        let latency_ms = binance_update_ms - poly_avg_update; //depreciated

        // Get Binance bid/ask
        let (binance_bid, binance_ask) = binance_manager
            .get_bid_ask("BTCUSDT")
            .await
            .unwrap_or((snapshot.spot_price, snapshot.spot_price));

        let time_to_open = current_market.window_start - now_ts;
        let ttx = if time_to_open > 0 {
            (current_market.window_end - now_ts) as f64 / 60.0
        } else {
            ((current_market.window_end - now_ts) as f64 / 60.0).max(0.0)
        };

        let resolved = ttx <= 0.0;
        let strike = current_market.strike_price.unwrap_or(snapshot.spot_price);
        let greeks = calculate_greeks(
            snapshot.spot_price,
            strike,
            ttx,
            snapshot.realized_volatility,
        );

        // Extract Polymarket prices (moved up for C6 resolution detection)
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

        let yes_spread = calculate_spread_bps(yes_bid, yes_ask);
        let no_spread = calculate_spread_bps(no_bid, no_ask);

        // Resolution detection: WS market_resolved → price-based → Gamma API fallback
        let outcome = if let Some(resolved_event) = market_events.take_resolved_for(&current_market.slug) {
            // Primary: WS market_resolved event (instant, no HTTP call)
            info!(
                "✅ WS resolution for {}: {}",
                resolved_event.slug, resolved_event.winning_outcome
            );
            // Map Up/Down outcomes to YES/NO
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
            let cached = gamma_outcome.lock().unwrap().clone();
            if let Some(result) = cached {
                result
            } else if !gamma_fetch_in_progress.load(Ordering::SeqCst)
                && (now_ms - gamma_last_outcome_fetch_ms >= 100) {
                gamma_last_outcome_fetch_ms = now_ms;
                // Spawn non-blocking Gamma API fetch with generation guard
                gamma_fetch_in_progress.store(true, Ordering::SeqCst);
                let cache = gamma_outcome.clone();
                let in_progress = gamma_fetch_in_progress.clone();
                let gen = gamma_generation.clone();
                let gen_at_spawn = gen.load(Ordering::SeqCst);
                let slug = current_market.slug.clone();
                let client = http.clone();
                tokio::spawn(async move {
                    match gamma::fetch_outcome(&client, &slug).await {
                        Ok(result) => {
                            if gen.load(Ordering::SeqCst) == gen_at_spawn {
                                info!("✅ Gamma API outcome for {}: {}", slug, result);
                                *cache.lock().unwrap() = Some(result);
                            } else {
                                info!("Gamma outcome for {} arrived after rotation, discarding", slug);
                            }
                        }
                        Err(e) => {
                            warn!("Gamma outcome fetch failed for {}: {}", slug, e);
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

        // Determine volatility data source - read from snapshot (computed on the clone)
        let vol_data_source = if snapshot.volatility_regime == "BOOTSTRAP" {
            "BOOTSTRAP".to_string()
        } else {
            snapshot.vol_source_path.clone()
        };

        // Sanitize f64 values — prevent NaN/Inf from reaching CSV
        // (NaN formats as "NaN" which breaks pandas numeric parsing)
        fn sanitize(v: f64) -> f64 {
            if v.is_finite() { v } else { 0.0 }
        }

        // (Polymarket price extraction moved above outcome determination for C6)

        // Get RTDS snapshot (Polymarket's price feed view)
        let rtds = rtds_tracker.get_snapshot(snapshot.spot_price, binance_update_ms);

        // Build record (backward compatible order)
        let record = CsvRecord {
            // ORIGINAL 23 columns
            timestamp: now.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
            market_slug: current_market.slug.clone(),
            binance_price: snapshot.spot_price,
            binance_volume: sanitize(snapshot.volume_1min),
            yes_bid,
            yes_bid_size: yes_prices.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0),
            yes_ask,
            yes_ask_size: yes_prices.as_ref().map(|p| p.best_ask_size).unwrap_or(0.0),
            no_bid,
            no_bid_size: no_prices.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0),
            no_ask,
            no_ask_size: no_prices.as_ref().map(|p| p.best_ask_size).unwrap_or(0.0),
            time_to_open,
            time_to_expiry_minutes: ttx,
            strike_price: strike,
            realized_volatility: sanitize(snapshot.realized_volatility),
            delta_yes: greeks.delta,
            gamma: greeks.gamma,
            vega: greeks.vega,
            theta: greeks.theta,
            market_resolved: resolved,
            outcome: outcome.to_string(),

            // NEW 22 columns (appended)
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
            vol_data_source: vol_data_source.clone(),
            yes_spread_bps: sanitize(yes_spread),
            no_spread_bps: sanitize(no_spread),

            // RTDS price feed columns
            rtds_binance_price: sanitize(rtds.rtds_binance_price),
            rtds_chainlink_price: sanitize(rtds.rtds_chainlink_price),
            rtds_binance_dev_bps: sanitize(rtds.rtds_binance_dev_bps),
            rtds_chainlink_dev_bps: sanitize(rtds.rtds_chainlink_dev_bps),
            rtds_price_advantage_bps: sanitize(rtds.rtds_price_advantage_bps),
            rtds_chainlink_binance_spread_bps: sanitize(rtds.rtds_chainlink_binance_spread_bps),
            rtds_binance_lag_ms: rtds.rtds_binance_lag_ms,
            rtds_chainlink_lag_ms: rtds.rtds_chainlink_lag_ms,
            rtds_binance_stale_ms: rtds.rtds_binance_stale_ms,
            rtds_chainlink_stale_ms: rtds.rtds_chainlink_stale_ms,
        };

        // Write to current hourly file
        csv_writer.write_record(&record)?;

        tick += 1;

        // P0 FIX: Periodic 1min buffer refresh (every ~5 minutes)
        if tick % (300 * 90) == 0 {
            // Every ~5 minutes at 90 ticks/sec
            let _ = binance_manager.bootstrap_volatility("BTCUSDT", 10).await;
        }

        if tick % 100 == 0 {
            info!(
                "#{} Market={} TTX={:.1}m | Hour={:02}",
                tick,
                current_market.unique_id,
                ttx,
                now.hour()
            );
            info!(
                "yesIDX={} | noIDX={}",
                truncate_id(&current_market.yes_token_id, 4, 2),
                truncate_id(&current_market.no_token_id, 4, 2)
            );
            info!(
                "BTC=${:.0}@${:.0} | Yes:{:.2}@{:.0} | No:{:.2}@{:.0} | Vol:{:.4}% | Δ:{:.6} | Γ:{:.6} | v:{:.6} | Θ:{:.6}",
                record.binance_price, record.binance_volume, record.yes_bid, record.yes_bid_size,
                record.no_bid, record.no_bid_size, record.realized_volatility*100.0,
                record.delta_yes, record.gamma, record.vega, record.theta
            );
            info!(
                "Lag={}ms | BASpr={:.2}bps | VolRege={} | VolSrc={} | Vol[1s/5s/89s]={:.4}/{:.4}/{:.4}",
                latency_ms, snapshot.spread_bps, snapshot.volatility_regime, vol_data_source,
                snapshot.vol_1sec, snapshot.vol_5sec, snapshot.vol_89sec
            );
            info!(
                "Volume[1s/1m]={:.0}/{:.0} | Spike={} | YesSpr={:.2}bps | NoSpr={:.2}bps | Q*={} | Conf={:.2}",
                snapshot.volume_1sec, snapshot.volume_1min, snapshot.volume_spike,
                yes_spread, no_spread, snapshot.signal_quality, snapshot.trade_confidence
            );

            // RTDS feed health
            let (rtds_b_count, rtds_c_count) = rtds_tracker.update_counts();
            info!(
                "RTDS: B_dev={:.1}bps C_dev={:.1}bps adv={:.1}bps C-B_spread={:.1}bps | stale=B:{}ms/C:{}ms | n=B:{}/C:{}",
                rtds.rtds_binance_dev_bps,
                rtds.rtds_chainlink_dev_bps,
                rtds.rtds_price_advantage_bps,
                rtds.rtds_chainlink_binance_spread_bps,
                rtds.rtds_binance_stale_ms,
                rtds.rtds_chainlink_stale_ms,
                rtds_b_count,
                rtds_c_count,
            );
        }

        // Check if market expired - start grace period, then rotate
        // Start grace period timer when market first expires
        if resolved && resolution_grace_start_ms == 0 {
            resolution_grace_start_ms = now_ms;
            info!("⏳ Market {} expired — starting {}s resolution grace period", current_market.unique_id, RESOLUTION_GRACE_SECS);
        }

        let grace_expired = resolution_grace_start_ms > 0
            && (now_ms - resolution_grace_start_ms) >= RESOLUTION_GRACE_MS;
        let has_outcome = !outcome.is_empty();

        if resolved && (has_outcome || grace_expired) {
            if grace_expired && !has_outcome {
                warn!("⚠️ Resolution grace period expired with no outcome for {} — forcing rotation", current_market.unique_id);
            } else if has_outcome {
                info!("✅ Outcome '{}' detected during grace period — rotating", outcome);
            }

            // Check if we need to cool down before retrying (prevent tight retry loops)
            let time_since_last_fetch_attempt = if last_fetch_attempt_ms > 0 {
                now_ms.saturating_sub(last_fetch_attempt_ms)
            } else {
                i64::MAX
            };

            let should_attempt_fetch = consecutive_fetch_failures == 0
                || (time_since_last_fetch_attempt >= FETCH_RETRY_COOLDOWN_MS
                    && consecutive_fetch_failures < MAX_CONSECUTIVE_FETCH_FAILURES);

            if should_attempt_fetch {
                last_fetch_attempt_ms = now_ms;

                info!(
                    "🔄 Market {} expired (TTX={:.1}m) - rotating to next market...",
                    current_market.unique_id, ttx
                );

                // Ensure outcome is captured before rotation — blocking Gamma fetch
                // prevents empty outcome rows when async fetch hasn't returned yet.
                if outcome.is_empty() {
                    info!("[OUTCOME] No outcome yet — blocking Gamma fetch before rotation");
                    let final_outcome = match tokio::time::timeout(
                        Duration::from_secs(3),
                        gamma::fetch_outcome(&http, &current_market.slug),
                    ).await {
                        Ok(Ok(result)) => {
                            info!("[OUTCOME] Blocking Gamma outcome: {}", result);
                            result
                        }
                        Ok(Err(e)) => {
                            warn!("[OUTCOME] Blocking Gamma fetch failed: {}", e);
                            String::new()
                        }
                        Err(_) => {
                            warn!("[OUTCOME] Blocking Gamma fetch timed out (3s)");
                            String::new()
                        }
                    };
                    if !final_outcome.is_empty() {
                        let mut final_record = record.clone();
                        final_record.outcome = final_outcome;
                        csv_writer.write_record(&final_record)?;
                        info!("[OUTCOME] Final resolution record written: {}", final_record.outcome);
                    }
                }

                // Try WS new_market event first, fall back to Gamma API
                let next_market_result = match market_events.take_new_market_for("btc-updown-15m-") {
                    Some(nm) if nm.outcomes.len() == 2 && nm.asset_ids.len() == 2 => {
                        info!("✅ WS new_market: {} (instant, no HTTP)", nm.slug);
                        let next_ws = current_market.window_start + combine_fetcher::MARKET_WINDOW_SECONDS;
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
                            window_end: next_ws + combine_fetcher::MARKET_WINDOW_SECONDS,
                            strike_price: None,
                            strike_fetched: false,
                            asset: "btc".to_string(),
                            window_seconds: combine_fetcher::MARKET_WINDOW_SECONDS,
                        })
                    }
                    _ => {
                        // No WS event or bad shape — Gamma API fallback
                        gamma::fetch_next_market(&http, current_market.window_start).await
                    }
                };

                match next_market_result {
                    Ok(next_market) => {
                        info!("✅ Rotated to: {}", next_market.unique_id);

                        // Clean up old token entries before subscribing to new ones
                        // This prevents stale entries from accumulating in the DashMap
                        // and triggering perpetual health warnings
                        let old_token_ids = vec![
                            current_market.yes_token_id.clone(),
                            current_market.no_token_id.clone(),
                        ];
                        poly_stream.remove_tokens(&old_token_ids);

                        current_market = next_market;
                        markets_tracked += 1;

                        // Reset Gamma outcome cache for new market
                        // Increment generation BEFORE clearing — any in-flight
                        // Gamma fetch from the old market will see a stale generation
                        // and discard its result.
                        gamma_generation.fetch_add(1, Ordering::SeqCst);
                        *gamma_outcome.lock().unwrap() = None;
                        gamma_fetch_in_progress.store(false, Ordering::SeqCst);

                        // Reset failure counter and grace timer on successful rotation
                        consecutive_fetch_failures = 0;
                        resolution_grace_start_ms = 0;
                        gamma_last_outcome_fetch_ms = 0;

                        // Fetch strike for the new market (infinite retry, no fallback)
                        println!(
                            "⏳ Fetching strike price for {}...",
                            current_market.unique_id
                        );
                        let strike = strike::fetch_strike_with_retry(&http, &current_market, None).await?;
                        current_market.strike_price = Some(strike);
                        current_market.strike_fetched = true;
                        println!("✅ Strike price: ${:.2} - resuming data collection", strike);

                        // Re-subscribe to new tokens
                        let new_token_ids = vec![
                            current_market.yes_token_id.clone(),
                            current_market.no_token_id.clone(),
                        ];
                        let mut new_rx = poly_stream.clone().stream(new_token_ids).await?;

                        tokio::spawn(
                            async move { while let Some(_update) = new_rx.recv().await {} },
                        );

                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Err(e) => {
                        consecutive_fetch_failures += 1;
                        warn!(
                            "⚠️ Failed to fetch next market (attempt {}/{}): {}",
                            consecutive_fetch_failures, MAX_CONSECUTIVE_FETCH_FAILURES, e
                        );

                        if consecutive_fetch_failures >= MAX_CONSECUTIVE_FETCH_FAILURES {
                            error!("❌ Too many consecutive fetch failures ({}). Exiting to prevent tight loop.",
                                MAX_CONSECUTIVE_FETCH_FAILURES);
                            break;
                        }

                        warn!("⏳ Will retry in {}s. Continuing with current market until cooldown expires...",
                            FETCH_RETRY_COOLDOWN_MS / 1000);
                    }
                }
            } else if time_since_last_fetch_attempt < FETCH_RETRY_COOLDOWN_MS {
                // In cooldown period - skip fetch attempt this iteration
                debug!(
                    "⏸️  In cooldown period ({}ms remaining until retry)",
                    FETCH_RETRY_COOLDOWN_MS - time_since_last_fetch_attempt
                );
            }
        }
    }

    // Flush final file
    csv_writer.flush()?;

    info!("✅ Collection complete!");
    info!("   Duration: {:.1} hours", duration as f64 / 3600.0);
    info!("   Markets tracked: {}", markets_tracked);
    info!("   Total ticks: {}", tick);
    info!("   Output base: {}", csv_path);
    info!("   Hourly files created in pattern: <base>_YYYY-MM-DD_HH.csv");
    println!("\n✅ Complete!");

    Ok(())
}