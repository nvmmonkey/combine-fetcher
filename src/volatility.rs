//! Volatility calculation module for latency-arbitrage trading
//!
//! Provides:
//! - Fibonacci multi-timeframe volatility calculator using Welford's algorithm
//! - Volume tracker with 1-second and 1-minute aggregation
//! - Market snapshot struct for combined market data
//! - Fast volatility estimates for sub-second arbitrage decisions
//!
//! OPTIMIZED FOR: Low-latency arbitrage with event-driven price sampling
//! - Deduplicates identical prices without time-based rate limiting
//! - Provides fast volatility (5s window) + accurate volatility (Fibonacci blend)
//! - Falls back to 1-min kline volatility when real-time data is stale

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tracing;

// ============================================================================
// MARKET SNAPSHOT
// ============================================================================

/// Combined market data snapshot for pricing and analysis
#[derive(Debug, Clone, serde::Serialize)]
pub struct MarketSnapshot {
    pub timestamp: i64,
    pub symbol: String,
    // pub spread_bps: f64,

    // For BinaryOptionPricer
    pub spot_price: f64,          // S: Current spot price
    pub realized_volatility: f64, // σ: Blended Fibonacci volatility for BS

    // Multi-timeframe volatility
    pub vol_1sec: f64,
    pub vol_5sec: f64,
    pub vol_13sec: f64,
    pub vol_34sec: f64,
    pub vol_89sec: f64,

    // Order book
    pub spread_bps: f64,

    // Volume (for momentum/filtering)
    pub volume_1sec: f64,
    pub volume_1min: f64,
    pub volume_spike: bool,

    // Signal quality
    pub signal_quality: String,
    pub trade_confidence: f64,
    pub volatility_regime: String,
}

// ============================================================================
// PRICE TICK
// ============================================================================

/// Single price tick with timestamp
#[derive(Debug, Clone)]
pub struct PriceTick {
    pub timestamp: i64,
    pub price: f64,
}

// ============================================================================
// VOLUME TICK
// ============================================================================

/// Single volume tick with timestamp
#[derive(Debug, Clone)]
pub struct VolumeTick {
    pub timestamp: i64,
    pub volume_usdt: f64,
}

// ============================================================================
// FIBONACCI MULTI-TIMEFRAME VOLATILITY CALCULATOR
// ============================================================================

/// Multi-timeframe volatility calculator using Fibonacci-weighted windows
///
/// Uses Welford's one-pass algorithm for numerically stable variance calculation.
/// Implements 500ms TTL caching to optimize repeated calculations.
///
/// ## Data Sources
/// - `price_buffer_unified`: Real-time tick data from WebSocket (event-driven, deduplicated)
/// - `price_buffer_1min`: Historical 1-minute candle closes from REST API bootstrap
///
/// ## Volatility Calculation Strategy
/// 1. If real-time buffer has enough samples → use real-time Fibonacci windows
/// 2. If real-time is stale (>3s) or sparse → fall back to 1-min volatility
/// 3. If neither has enough data → return default volatility
///
/// ## Latency Arbitrage Optimization
/// - NO time-based rate limiting (preserves sub-second reaction time)
/// - Deduplicates identical prices only (filters orderbook noise)
/// - Provides fast volatility (5s window, 3+ samples) for trading decisions
/// - Provides accurate volatility (Fibonacci blend) for Greeks/pricing
#[derive(Clone)]
pub struct FibonacciVolatilityCalculator {
    // Real-time tick buffer (from WebSocket bookTicker)
    price_buffer_unified: VecDeque<PriceTick>,

    // Historical 1-minute candle closes (from REST API bootstrap)
    price_buffer_1min: VecDeque<f64>,

    // Configuration
    window_minutes: usize,
    min_samples_fast: usize,     // Minimum samples for fast volatility (3)
    min_samples_realtime: usize, // Minimum samples for standard windows (5-10)
    min_samples_1min: usize,     // Minimum samples for 1-min calculation (10)
    default_volatility: f64,

    // Deduplication (NO time-based rate limiting)
    last_accepted_price: f64,
    min_price_change_bps: f64, // Minimum price change to accept sample

    // Staleness detection
    stale_threshold_ms: i64, // Time without updates to consider data stale

    // Precomputed annualization factors
    // Formula: sqrt(periods_per_year) where periods_per_year = seconds_per_year / interval_seconds
    annualization_1sec: f64,
    annualization_5sec: f64,
    annualization_13sec: f64,
    annualization_34sec: f64,
    annualization_89sec: f64,
    annualization_1min: f64,

    // Fibonacci weights (1:2:3:5:8) - favors longer timeframes for stability
    fib_weights: [f64; 5],
    fib_weight_sum: f64,

    // Cache (optimization: recalculate only every 500ms)
    cached_vol_fast: f64, // Fast estimate (5s window) for arbitrage
    cached_vol_1sec: f64,
    cached_vol_5sec: f64,
    cached_vol_13sec: f64,
    cached_vol_34sec: f64,
    cached_vol_89sec: f64,
    cached_vol_1min: f64,    // Cached 1-min volatility
    cached_vol_blended: f64, // Fibonacci-weighted blend
    cache_timestamp: i64,
    cache_ttl_ms: i64,

    // Symbol for logging
    symbol: String,

    // Volatility source path tracking
    // Tracks which calculation path was actually used: "REALTIME_FIBONACCI" | "1MIN_FALLBACK" | "DEFAULT"
    vol_source_path: String,
}

impl FibonacciVolatilityCalculator {
    /// Create a new volatility calculator for the given symbol
    pub fn new(symbol: &str) -> Self {
        let default_vol = match symbol.to_uppercase().as_str() {
            "BTCUSDT" => 0.55,
            "ETHUSDT" => 0.65,
            "SOLUSDT" => 0.75,
            _ => 0.65,
        };

        let fib_weights = [1.0, 2.0, 3.0, 5.0, 8.0];
        let fib_weight_sum: f64 = fib_weights.iter().sum();

        // Correct annualization: periods_per_year = (365.25 * 24 * 60 * 60) / interval_seconds
        const SECONDS_PER_YEAR: f64 = 365.25 * 24.0 * 60.0 * 60.0;

        Self {
            price_buffer_unified: VecDeque::with_capacity(500),
            price_buffer_1min: VecDeque::with_capacity(120),
            window_minutes: 60,
            min_samples_fast: 3,     // Fast vol needs 3+ price changes
            min_samples_realtime: 5, // Standard windows need 5+ samples
            min_samples_1min: 10,    // 1-min needs 10+ candles
            default_volatility: default_vol,

            // Deduplication settings (NO time-based rate limiting)
            last_accepted_price: 0.0,
            min_price_change_bps: 0.1, // 0.1 bps = 0.0001% (ultra-sensitive to capture micro-movements)

            // Staleness threshold
            stale_threshold_ms: 120_000, // P1 FIX: 2 minutes - matches 1-min granularity

            // Corrected annualization factors
            annualization_1sec: (SECONDS_PER_YEAR / 1.0).sqrt(), // ≈ 5,617
            annualization_5sec: (SECONDS_PER_YEAR / 5.0).sqrt(), // ≈ 2,512
            annualization_13sec: (SECONDS_PER_YEAR / 13.0).sqrt(), // ≈ 1,558
            annualization_34sec: (SECONDS_PER_YEAR / 34.0).sqrt(), // ≈ 963
            annualization_89sec: (SECONDS_PER_YEAR / 89.0).sqrt(), // ≈ 595
            annualization_1min: (SECONDS_PER_YEAR / 60.0).sqrt(), // ≈ 725

            fib_weights,
            fib_weight_sum,

            cached_vol_fast: default_vol,
            cached_vol_1sec: default_vol,
            cached_vol_5sec: default_vol,
            cached_vol_13sec: default_vol,
            cached_vol_34sec: default_vol,
            cached_vol_89sec: default_vol,
            cached_vol_1min: default_vol,
            cached_vol_blended: default_vol,
            cache_timestamp: 0,
            cache_ttl_ms: 500,

            symbol: symbol.to_uppercase(),

            vol_source_path: "DEFAULT".to_string(),
        }
    }

    // ========================================================================
    // DATA INPUT METHODS
    // ========================================================================

    /// Add real-time price tick with deduplication (NO time-based rate limiting)
    ///
    /// Optimized for latency arbitrage:
    /// - Accepts every meaningful price change immediately
    /// - Filters duplicate prices to prevent variance pollution
    /// - NO artificial delays (preserves sub-second reaction time)
    ///
    /// Called by WebSocket handler for each bookTicker update.
    /// Add real-time price tick with improved deduplication
    pub fn add_price(&mut self, timestamp: i64, price: f64) {
        if price <= 0.0 {
            return;
        }

        // Improved deduplication: accept if meaningful change OR 60+ seconds passed
        if self.last_accepted_price > 0.0 {
            let price_change_bps =
                ((price - self.last_accepted_price).abs() / self.last_accepted_price) * 10000.0;

            let time_since_last = self
                .price_buffer_unified
                .back()
                .map_or(i64::MAX, |last| timestamp - last.timestamp);

            // Accept if: meaningful change OR 60+ seconds passed (fixes sparse ticks)
            if price_change_bps < self.min_price_change_bps && time_since_last < 60_000 {
                return; // Skip duplicate
            }
        }

        // Accept tick
        self.price_buffer_unified
            .push_back(PriceTick { timestamp, price });
        self.last_accepted_price = price;

        // Cleanup: Keep only last 90 minutes (not seconds)
        let cutoff = timestamp - 90 * 60_000; // CHANGED: 90 minutes
        while let Some(oldest) = self.price_buffer_unified.front() {
            if oldest.timestamp < cutoff {
                self.price_buffer_unified.pop_front();
            } else {
                break;
            }
        }
    }

    /// NEW: Sync 1-min rolling data into real-time buffer for accurate volatility
    /// Call this every ~60 seconds from your main loop
    pub fn sync_1min_to_realtime(&mut self, current_time: i64) {
        // Take most recent 1-min candle and add to real-time buffer
        if let Some(&last_1min_price) = self.price_buffer_1min.back() {
            let minute_timestamp = (current_time / 60_000) * 60_000; // Snap to minute

            // Only add if it's a new minute bucket
            let should_add = self.price_buffer_unified.back().map_or(true, |last_tick| {
                let last_minute = (last_tick.timestamp / 60_000) * 60_000;
                minute_timestamp > last_minute
            });

            if should_add {
                self.price_buffer_unified.push_back(PriceTick {
                    timestamp: minute_timestamp,
                    price: last_1min_price,
                });

                tracing::debug!(
                    "{} synced 1-min price {:.2} to realtime buffer (buffer_size={})",
                    self.symbol,
                    last_1min_price,
                    self.price_buffer_unified.len()
                );
            }
        }

        // Cleanup old data (90 minutes)
        let cutoff = current_time - 90 * 60_000;
        while let Some(oldest) = self.price_buffer_unified.front() {
            if oldest.timestamp < cutoff {
                self.price_buffer_unified.pop_front();
            } else {
                break;
            }
        }
    }

    /// Add 1-minute candle close price (from REST API bootstrap)
    ///
    /// Called during initialization to seed historical volatility.
    pub fn add_price_1min(&mut self, price: f64) {
        if price <= 0.0 {
            return;
        }

        self.price_buffer_1min.push_back(price);

        // Keep at most window_minutes + 10 candles
        let max_size = self.window_minutes + 10;
        while self.price_buffer_1min.len() > max_size {
            self.price_buffer_1min.pop_front();
        }
    }

    // ========================================================================
    // BUFFER STATUS METHODS
    // ========================================================================

    /// Get the size of the 1-minute price buffer
    pub fn get_1min_buffer_size(&self) -> usize {
        self.price_buffer_1min.len()
    }

    /// Get the size of the unified (real-time) price buffer
    pub fn get_unified_buffer_size(&self) -> usize {
        self.price_buffer_unified.len()
    }

    /// Check if calculator has been bootstrapped with historical data
    pub fn is_bootstrapped(&self) -> bool {
        self.price_buffer_1min.len() >= self.min_samples_1min
    }

    /// Check if calculator has enough real-time data for fast estimates
    pub fn has_realtime_data(&self) -> bool {
        self.price_buffer_unified.len() >= self.min_samples_fast
    }

    /// Check if real-time data is stale (>2 minutes without updates)
    pub fn is_realtime_stale(&self, current_time: i64) -> bool {
        if self.price_buffer_unified.is_empty() {
            return true;
        }

        // P1 FIX: Use the configured threshold instead of hard-coded value
        if let Some(latest) = self.price_buffer_unified.back() {
            return (current_time - latest.timestamp) > self.stale_threshold_ms;
        }

        true
    }

    /// Check if calculator is ready for reliable calculations
    /// (either real-time OR bootstrapped)
    pub fn is_ready(&self) -> bool {
        self.has_realtime_data() || self.is_bootstrapped()
    }

    // ========================================================================
    // VOLATILITY CALCULATION METHODS
    // ========================================================================

    /// Calculate volatility from real-time tick window using Welford's algorithm
    ///
    /// Returns None if insufficient data, otherwise returns annualized volatility.
    #[inline]
    fn calculate_vol_from_realtime_window(
        &self,
        window_ms: i64,
        annualization: f64,
        current_time: i64,
        min_samples: usize,
    ) -> Option<f64> {
        let cutoff = current_time - window_ms;
        let filtered: Vec<f64> = self
            .price_buffer_unified
            .iter()
            .filter(|tick| tick.timestamp >= cutoff)
            .map(|tick| tick.price)
            .collect();

        if filtered.len() < min_samples {
            return None; // Not enough price changes in this window
        }

        self.welford_volatility(&filtered, annualization)
    }

    /// Calculate volatility from 1-minute candle buffer
    ///
    /// Uses all available candles in the buffer.
    /// Returns None if insufficient data.
    fn calculate_vol_from_1min_buffer(&self) -> Option<f64> {
        if self.price_buffer_1min.len() < self.min_samples_1min {
            return None;
        }

        let prices: Vec<f64> = self.price_buffer_1min.iter().copied().collect();
        self.welford_volatility(&prices, self.annualization_1min)
    }

    /// Calculate volatility from 1-minute buffer using last N candles
    fn calculate_vol_from_1min_window(&self, candles: usize) -> Option<f64> {
        let available = self.price_buffer_1min.len();
        if available < self.min_samples_1min {
            return None;
        }

        let take_count = candles.min(available);
        let prices: Vec<f64> = self
            .price_buffer_1min
            .iter()
            .skip(available - take_count)
            .copied()
            .collect();

        if prices.len() < self.min_samples_1min {
            return None;
        }

        self.welford_volatility(&prices, self.annualization_1min)
    }

    /// Welford's one-pass algorithm for numerically stable variance calculation
    ///
    /// Returns annualized volatility, clamped to [0.001, 3.0]
    /// Lower clamp reduced to 0.1% to allow real low-volatility periods
    #[inline]
    fn welford_volatility(&self, prices: &[f64], annualization: f64) -> Option<f64> {
        if prices.len() < 2 {
            return None;
        }

        let mut count = 0;
        let mut mean = 0.0;
        let mut m2 = 0.0;

        for i in 1..prices.len() {
            if prices[i - 1] > 0.0 && prices[i] > 0.0 {
                let log_return = (prices[i] / prices[i - 1]).ln();
                count += 1;
                let delta = log_return - mean;
                mean += delta / count as f64;
                let delta2 = log_return - mean;
                m2 += delta * delta2;
            }
        }

        if count < 2 {
            return None;
        }

        let variance = m2 / (count as f64 - 1.0);
        let std_dev = variance.sqrt();
        let annualized = std_dev * annualization;

        // Debug: Log raw calculation before clamping
        tracing::trace!(
            "{} welford: count={}, variance={:.12}, std_dev={:.8}, annualization={:.2}, raw_annualized={:.6}",
            self.symbol, count, variance, std_dev, annualization, annualized
        );

        // Clamp to reasonable range (0.1% to 300% annualized)
        // Lower clamp reduced from 5% to 0.1% to allow real low-vol periods
        // H5 FIX: Prevent NaN propagation to MarketSnapshot
        let result = annualized.clamp(0.001, 3.0);
        if !result.is_finite() {
            return None;
        }
        Some(result)
    }

    /// Calculate all Fibonacci timeframes with caching
    ///
    /// Returns: (vol_1sec, vol_5sec, vol_13sec, vol_34sec, vol_89sec, vol_blended)
    ///
    /// ## Fallback Logic
    /// - If real-time data available and not stale → use real-time Fibonacci windows
    /// - If real-time sparse or stale → fall back to 1-min volatility
    /// - If neither → return default volatility
    ///
    /// ## Fast Volatility
    /// - Also calculates fast volatility (5s window, 3+ samples) for arbitrage decisions
    /// - Available via get_fast_volatility()
    /// Calculate all Fibonacci timeframes with caching
    ///
    /// Returns: (vol_1sec, vol_5sec, vol_13sec, vol_34sec, vol_89sec, vol_blended)
    ///
    /// ## Fallback Logic
    /// - If real-time data available and not stale → use real-time Fibonacci windows
    /// - If real-time vol suspiciously low (< threshold) → fall back to 1-min vol
    /// - If real-time sparse or stale → fall back to 1-min volatility
    /// - If neither → return default volatility
    ///
    /// ## Fast Volatility
    /// - Also calculates fast volatility (5s window, 3+ samples) for arbitrage decisions
    /// - Available via get_fast_volatility()
    /// Calculate all Fibonacci timeframes with 1-min sync fallback
    ///
    /// AUTO-SYNC: Automatically feeds 1-min data into real-time buffer when stale
    /// All existing logic preserved + sparse tick fix
    pub fn calculate_all(&mut self, current_time: i64) -> (f64, f64, f64, f64, f64, f64) {
        // Check cache validity (500ms TTL)
        if current_time - self.cache_timestamp < self.cache_ttl_ms {
            return (
                self.cached_vol_1sec,
                self.cached_vol_5sec,
                self.cached_vol_13sec,
                self.cached_vol_34sec,
                self.cached_vol_89sec,
                self.cached_vol_blended,
            );
        }

        // NEW: Auto-sync 1-min data into real-time buffer if stale
        // This fixes sparse millisecond ticks automatically
        if self.is_realtime_stale(current_time) {
            tracing::debug!(
                "{} auto-syncing 1-min data to realtime (stale={}, realtime_size={})",
                self.symbol,
                self.is_realtime_stale(current_time),
                self.price_buffer_unified.len()
            );
            self.sync_1min_to_realtime(current_time);
        }

        // Calculate 1-min volatility first (used as fallback)
        let vol_1min_raw = self.calculate_vol_from_1min_buffer();
        self.cached_vol_1min = vol_1min_raw.unwrap_or(self.default_volatility);

        tracing::debug!(
            "{} 1min_vol: raw={:?} -> cached={:.4} (buffer_size={}, default={:.2})",
            self.symbol,
            vol_1min_raw,
            self.cached_vol_1min,
            self.price_buffer_1min.len(),
            self.default_volatility
        );

        const MIN_REALTIME_VOL_THRESHOLD: f64 = 0.02; // 2% annualized

        let get_vol_with_fallback = |realtime_vol: Option<f64>, fallback: f64| -> f64 {
            match realtime_vol {
                Some(v) if v >= MIN_REALTIME_VOL_THRESHOLD => v,
                Some(v) => {
                    tracing::trace!(
                        "{} real-time vol {:.4} below threshold {:.4}, using fallback {:.4}",
                        self.symbol,
                        v,
                        MIN_REALTIME_VOL_THRESHOLD,
                        fallback
                    );
                    fallback
                }
                None => fallback,
            }
        };

        // Calculate FAST volatility estimate (5s window, 3+ samples)
        let fast_vol_raw = self.calculate_vol_from_realtime_window(
            5_000,
            self.annualization_1sec,
            current_time,
            self.min_samples_fast,
        );

        let is_stale = self.is_realtime_stale(current_time);
        let use_realtime = fast_vol_raw.is_some() && !is_stale;

        if use_realtime {
            self.vol_source_path = "REALTIME_FIBONACCI".to_string();
            self.cached_vol_fast = get_vol_with_fallback(fast_vol_raw, self.cached_vol_1min);

            self.cached_vol_1sec = self.cached_vol_fast;
            self.cached_vol_5sec = get_vol_with_fallback(
                self.calculate_vol_from_realtime_window(
                    10_000,
                    self.annualization_5sec,
                    current_time,
                    self.min_samples_realtime,
                ),
                self.cached_vol_1min,
            );
            self.cached_vol_13sec = get_vol_with_fallback(
                self.calculate_vol_from_realtime_window(
                    20_000,
                    self.annualization_13sec,
                    current_time,
                    self.min_samples_realtime,
                ),
                self.cached_vol_5sec,
            );
            self.cached_vol_34sec = get_vol_with_fallback(
                self.calculate_vol_from_realtime_window(
                    45_000,
                    self.annualization_34sec,
                    current_time,
                    self.min_samples_realtime + 2,
                ),
                self.cached_vol_13sec,
            );
            self.cached_vol_89sec = get_vol_with_fallback(
                self.calculate_vol_from_realtime_window(
                    90_000,
                    self.annualization_89sec,
                    current_time,
                    self.min_samples_realtime + 5,
                ),
                self.cached_vol_34sec,
            );

            tracing::trace!(
            "{} using realtime vol: fast={:.4}, 1s={:.4}, 5s={:.4}, 13s={:.4}, 34s={:.4}, 89s={:.4}",
            self.symbol,
            self.cached_vol_fast,
            self.cached_vol_1sec,
            self.cached_vol_5sec,
            self.cached_vol_13sec,
            self.cached_vol_34sec,
            self.cached_vol_89sec
        );
        } else {
            self.vol_source_path = "1MIN_FALLBACK".to_string();
            tracing::debug!(
                "{} using 1-min fallback (stale={}, fast_vol={:?}, buffer_size={})",
                self.symbol,
                is_stale,
                fast_vol_raw,
                self.price_buffer_unified.len()
            );

            // P0 FIX: Apply production floor to fallback values
            const MIN_PRODUCTION_VOL: f64 = 0.10; // 10% annualized floor for BTCUSDT
            let vol_with_floor = self.cached_vol_1min.max(MIN_PRODUCTION_VOL);
            self.cached_vol_fast = vol_with_floor;
            self.cached_vol_1sec = vol_with_floor;
            self.cached_vol_5sec = vol_with_floor;
            self.cached_vol_13sec = vol_with_floor;
            self.cached_vol_34sec = vol_with_floor;
            self.cached_vol_89sec = vol_with_floor;
        }

        // Fibonacci-weighted blend for accurate estimate
        self.cached_vol_blended = (self.cached_vol_1sec * self.fib_weights[0]
            + self.cached_vol_5sec * self.fib_weights[1]
            + self.cached_vol_13sec * self.fib_weights[2]
            + self.cached_vol_34sec * self.fib_weights[3]
            + self.cached_vol_89sec * self.fib_weights[4])
            / self.fib_weight_sum;

        // P0 FIX: Production floor on blended volatility (prevents z-score explosion)
        const MIN_PRODUCTION_VOL: f64 = 0.10; // 10% annualized floor for BTCUSDT
        self.cached_vol_blended = self.cached_vol_blended.max(MIN_PRODUCTION_VOL);

        self.cache_timestamp = current_time;

        (
            self.cached_vol_1sec,
            self.cached_vol_5sec,
            self.cached_vol_13sec,
            self.cached_vol_34sec,
            self.cached_vol_89sec,
            self.cached_vol_blended,
        )
    }

    /// Get the best available volatility estimate
    ///
    /// Tries in order:
    /// 1. Blended Fibonacci volatility (if real-time data available)
    /// 2. 1-minute historical volatility (if bootstrapped)
    /// 3. Default volatility
    pub fn get_best_volatility(&self) -> f64 {
        if self.has_realtime_data()
            && !self.is_realtime_stale(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64,
            )
        {
            self.cached_vol_blended
        } else if self.is_bootstrapped() {
            self.cached_vol_1min
        } else {
            self.default_volatility
        }
    }

    /// Get fast volatility estimate for arbitrage decisions
    ///
    /// Fast estimate uses 5-second window with minimum 3 price changes.
    /// Falls back to 1-min volatility if real-time data is stale.
    pub fn get_fast_volatility(&self) -> f64 {
        self.cached_vol_fast
    }

    /// Get the current volatility regime based on short/long-term ratio
    pub fn get_volatility_regime(&self) -> String {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        if !self.has_realtime_data() || self.is_realtime_stale(current_time) {
            return "BOOTSTRAP".to_string(); // Indicate we're using historical data
        }

        let ratio = self.cached_vol_1sec / self.cached_vol_89sec.max(0.01);
        if ratio > 2.0 {
            "EXTREME".to_string()
        } else if ratio > 1.5 {
            "HIGH".to_string()
        } else if ratio < 0.7 {
            "LOW".to_string()
        } else {
            "NORMAL".to_string()
        }
    }

    /// Get the volatility source path that was used in the last calculate_all() call
    ///
    /// Returns one of:
    /// - "REALTIME_FIBONACCI" - Using real-time WebSocket tick data
    /// - "1MIN_FALLBACK" - Using 1-minute historical kline data
    /// - "DEFAULT" - Using default volatility (insufficient data)
    pub fn get_vol_source_path(&self) -> &str {
        &self.vol_source_path
    }

    /// Get diagnostic information about the calculator state
    pub fn get_diagnostics(&self) -> String {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        format!(
            "{}[rt:{}/1m:{}/ready:{}/stale:{}]",
            self.symbol,
            self.price_buffer_unified.len(),
            self.price_buffer_1min.len(),
            if self.is_ready() { "Y" } else { "N" },
            if self.is_realtime_stale(current_time) {
                "Y"
            } else {
                "N"
            }
        )
    }
}

// ============================================================================
// VOLUME TRACKER
// ============================================================================

/// Volume tracker with rolling window aggregation
///
/// Tracks trade volume in 1-second and 1-minute windows,
/// with automatic detection of volume spikes.
pub struct VolumeTracker {
    volume_buffer: VecDeque<VolumeTick>,
    average_volume_1min: f64,
    sample_count: usize,
}

impl VolumeTracker {
    /// Create a new volume tracker
    pub fn new() -> Self {
        Self {
            volume_buffer: VecDeque::with_capacity(500),
            average_volume_1min: 0.0,
            sample_count: 0,
        }
    }

    /// Add a trade to the volume tracker
    pub fn add_trade(&mut self, timestamp: i64, volume_usdt: f64) {
        self.volume_buffer.push_back(VolumeTick {
            timestamp,
            volume_usdt,
        });

        // Cleanup: Keep only last 65 seconds
        let cutoff_1min = timestamp - 65_000;
        while let Some(oldest) = self.volume_buffer.front() {
            if oldest.timestamp < cutoff_1min {
                self.volume_buffer.pop_front();
            } else {
                break;
            }
        }

        // Update rolling average
        self.sample_count += 1;
        if self.sample_count > 60 {
            self.average_volume_1min = self.get_volume_1min();
        }
    }

    /// Calculate 1-second volume on-demand (prevents negative values)
    #[inline]
    pub fn get_volume_1sec(&self, current_time: i64) -> f64 {
        let cutoff = current_time - 1_000;
        let sum = self
            .volume_buffer
            .iter()
            .filter(|v| v.timestamp >= cutoff)
            .map(|v| v.volume_usdt)
            .sum::<f64>();

        // Normalize -0.0 to 0.0
        if sum == 0.0 {
            0.0
        } else {
            sum.max(0.0)
        }
    }

    /// Get total volume in the 1-minute window
    #[inline]
    pub fn get_volume_1min(&self) -> f64 {
        self.volume_buffer
            .iter()
            .map(|v| v.volume_usdt)
            .sum::<f64>()
            .max(0.0)
    }

    /// Check if current volume represents a spike (2x baseline)
    #[inline]
    pub fn is_volume_spike(&self, current_time: i64) -> bool {
        if self.average_volume_1min <= 0.0 {
            return false;
        }

        let current = self.get_volume_1sec(current_time) * 60.0;
        current > self.average_volume_1min * 2.0
    }

    /// Get the ratio of current volume to baseline
    #[inline]
    pub fn get_volume_ratio(&self, current_time: i64) -> f64 {
        if self.average_volume_1min <= 0.0 {
            return 1.0;
        }

        let current = self.get_volume_1sec(current_time) * 60.0;
        current / self.average_volume_1min
    }
}

impl Default for VolumeTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// THREAD-SAFE WRAPPERS
// ============================================================================

/// Thread-safe wrapper for FibonacciVolatilityCalculator
pub type SharedVolatilityCalculator = Arc<Mutex<FibonacciVolatilityCalculator>>;

/// Thread-safe wrapper for VolumeTracker
pub type SharedVolumeTracker = Arc<Mutex<VolumeTracker>>;

/// Create a new shared volatility calculator
pub fn create_shared_volatility_calculator(symbol: &str) -> SharedVolatilityCalculator {
    Arc::new(Mutex::new(FibonacciVolatilityCalculator::new(symbol)))
}

/// Create a new shared volume tracker
pub fn create_shared_volume_tracker() -> SharedVolumeTracker {
    Arc::new(Mutex::new(VolumeTracker::new()))
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_tick_creation() {
        let tick = PriceTick {
            timestamp: 1234567890,
            price: 50000.0,
        };
        assert_eq!(tick.timestamp, 1234567890);
        assert_eq!(tick.price, 50000.0);
    }

    #[test]
    fn test_volume_tick_creation() {
        let tick = VolumeTick {
            timestamp: 1234567890,
            volume_usdt: 1000.0,
        };
        assert_eq!(tick.timestamp, 1234567890);
        assert_eq!(tick.volume_usdt, 1000.0);
    }

    #[test]
    fn test_volatility_calculator_creation() {
        let calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        assert_eq!(calc.default_volatility, 0.55);
        assert!(!calc.is_ready());
        assert!(!calc.is_bootstrapped());
        assert!(!calc.has_realtime_data());
        assert_eq!(calc.fib_weights, [1.0, 2.0, 3.0, 5.0, 8.0]);
        assert_eq!(calc.min_price_change_bps, 0.1);
        assert_eq!(calc.stale_threshold_ms, 120_000); // P1 FIX: Updated to 2 minutes
    }

    #[test]
    fn test_deduplication_filters_identical_prices() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        let base_time = 1234567890000_i64;

        // Add same price multiple times (should deduplicate)
        for i in 0..100 {
            calc.add_price(base_time + i * 10, 50000.0);
        }

        // Should only have 1 sample (all duplicates filtered)
        assert_eq!(calc.get_unified_buffer_size(), 1);

        // Add slightly different price (should accept)
        calc.add_price(base_time + 1000, 50003.0); // 0.006% change = 0.6 bps > 0.5 bps threshold
        assert_eq!(calc.get_unified_buffer_size(), 2);
    }

    #[test]
    fn test_volatility_calculator_add_price() {
        let mut calc = FibonacciVolatilityCalculator::new("ETHUSDT");
        let base_time = 1234567890000_i64;

        // Add real-time prices with meaningful changes
        for i in 0..25 {
            calc.add_price(base_time + i * 100, 1000.0 + i as f64 * 2.0);
        }

        assert_eq!(calc.get_unified_buffer_size(), 25);
        assert!(calc.has_realtime_data());
        assert!(!calc.is_bootstrapped());
        assert!(calc.is_ready()); // Ready because has real-time
    }

    #[test]
    fn test_volatility_calculator_add_1min() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");

        // Add 1-min candles
        for i in 0..15 {
            calc.add_price_1min(50000.0 + i as f64 * 10.0);
        }

        assert_eq!(calc.get_1min_buffer_size(), 15);
        assert!(calc.is_bootstrapped());
        assert!(!calc.has_realtime_data());
        assert!(calc.is_ready()); // Ready because bootstrapped
    }

    #[test]
    fn test_1min_volatility_calculation() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");

        // Add enough 1-min candles with realistic price movement
        let base = 50000.0;
        let prices: Vec<f64> = (0..60)
            .map(|i| {
                let offset = ((i as f64 * 0.7).sin() * 25.0) + ((i as f64 * 1.3).cos() * 15.0);
                base + offset
            })
            .collect();

        for price in &prices {
            calc.add_price_1min(*price);
        }

        let vol = calc.calculate_vol_from_1min_buffer();
        assert!(vol.is_some());
        let vol_value = vol.unwrap();
        println!("1-min volatility: {:.4}", vol_value);
        assert!(vol_value > 0.01, "Vol should be > 1%, got {:.4}", vol_value);
        assert!(vol_value <= 3.0);
    }

    #[test]
    fn test_fallback_to_1min_vol() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        let current_time = 1234567890000_i64;

        // Add 1-min data with oscillating prices
        let prices = [
            50000.0, 50050.0, 49980.0, 50100.0, 49950.0, 50080.0, 50020.0, 50150.0, 49900.0,
            50200.0, 50100.0, 50250.0, 49850.0, 50300.0, 50150.0, 50350.0, 50200.0, 50400.0,
            50100.0, 50450.0, 50300.0, 50500.0, 50200.0, 50550.0, 50350.0, 50600.0, 50400.0,
            50650.0, 50300.0, 50700.0, 50500.0, 50750.0, 50400.0, 50800.0, 50550.0, 50850.0,
            50600.0, 50900.0, 50500.0, 50950.0, 50700.0, 51000.0, 50600.0, 51050.0, 50750.0,
            51100.0, 50800.0, 51150.0, 50700.0, 51200.0, 50900.0, 51250.0, 50800.0, 51300.0,
            50950.0, 51350.0, 51000.0, 51400.0, 50900.0, 51450.0,
        ];

        for price in &prices {
            calc.add_price_1min(*price);
        }

        // Real-time buffer is empty, should fall back to 1-min
        let (v1, v5, v13, _v34, _v89, vb) = calc.calculate_all(current_time);

        // All should be equal to the 1-min volatility (fallback)
        assert!((v1 - v5).abs() < 0.0001);
        assert!((v5 - v13).abs() < 0.0001);
        println!("Fallback vol: {:.4}", vb);

        // Should NOT be the minimum clamp value (0.001)
        assert!(vb > 0.01, "Blended vol should be > 1%, got {:.4}", vb);
    }

    #[test]
    fn test_staleness_detection() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        let base_time = 1234567890000_i64;

        // Add some recent data
        calc.add_price(base_time, 50000.0);
        calc.add_price(base_time + 100, 50010.0);

        // Not stale immediately
        assert!(!calc.is_realtime_stale(base_time + 200));

        // Stale after 2+ minutes (P1 FIX: updated threshold)
        assert!(calc.is_realtime_stale(base_time + 130_000));
    }

    #[test]
    fn test_fast_volatility() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        let base_time = 1234567890000_i64;

        // Add enough price changes for fast vol (3+ samples in 5s window)
        calc.add_price(base_time, 50000.0);
        calc.add_price(base_time + 1000, 50010.0);
        calc.add_price(base_time + 2000, 49995.0);
        calc.add_price(base_time + 3000, 50015.0);

        calc.calculate_all(base_time + 3000);
        let fast_vol = calc.get_fast_volatility();

        println!("Fast volatility: {:.4}", fast_vol);
        assert!(fast_vol > 0.0);
    }

    #[test]
    fn test_volatility_calculator_cache() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        let base_time = 1234567890000_i64;

        // Add real-time prices
        for i in 0..30 {
            calc.add_price(base_time + i * 100, 50000.0 + i as f64 * 10.0);
        }

        // First call should calculate
        let (v1, _v5, _v13, _v34, _v89, _vb) = calc.calculate_all(base_time + 2900);
        let cached_ts = calc.cache_timestamp;
        assert!(cached_ts > 0);

        // Second call within 500ms should use cache
        let (v1_cached, _, _, _, _, _) = calc.calculate_all(base_time + 3200);
        assert_eq!(v1, v1_cached);
        assert_eq!(calc.cache_timestamp, cached_ts); // Timestamp unchanged
    }

    #[test]
    fn test_volume_tracker_creation() {
        let tracker = VolumeTracker::new();
        assert_eq!(tracker.volume_buffer.len(), 0);
        assert_eq!(tracker.average_volume_1min, 0.0);
    }

    #[test]
    fn test_volume_tracker_add_trade() {
        let mut tracker = VolumeTracker::new();
        let timestamp = 1234567890000_i64;

        tracker.add_trade(timestamp, 100.0);
        tracker.add_trade(timestamp + 100, 200.0);
        assert_eq!(tracker.volume_buffer.len(), 2);
        assert_eq!(tracker.get_volume_1sec(timestamp + 200), 300.0);
    }

    #[test]
    fn test_volume_tracker_cleanup() {
        let mut tracker = VolumeTracker::new();
        let timestamp = 1234567890000_i64;

        // Add trades over time
        for i in 0..100 {
            tracker.add_trade(timestamp + i * 1000, 100.0);
        }

        // Add a trade 70 seconds later (should trigger cleanup)
        tracker.add_trade(timestamp + 70000, 100.0);

        // Old trades should be removed
        assert!(tracker.volume_buffer.len() < 100);
    }

    #[test]
    fn test_fibonacci_weights() {
        let calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        assert_eq!(calc.fib_weights, [1.0, 2.0, 3.0, 5.0, 8.0]);
        assert_eq!(calc.fib_weight_sum, 19.0);
    }

    #[test]
    fn test_volatility_regime_bootstrap() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");

        // Only 1-min data, no real-time
        for i in 0..20 {
            calc.add_price_1min(50000.0 + i as f64);
        }

        let regime = calc.get_volatility_regime();
        assert_eq!(regime, "BOOTSTRAP");
    }

    #[test]
    fn test_get_diagnostics() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        let base_time = 1234567890000_i64;

        for i in 0..10 {
            calc.add_price(base_time + i * 100, 50000.0 + i as f64 * 5.0);
        }

        for _ in 0..15 {
            calc.add_price_1min(50000.0);
        }

        let diag = calc.get_diagnostics();
        assert!(diag.contains("BTCUSDT"));
        assert!(diag.contains("rt:10"));
        assert!(diag.contains("1m:15"));
    }

    #[test]
    fn test_shared_calculator_creation() {
        let shared = create_shared_volatility_calculator("SOLUSDT");
        let calc = shared.lock().unwrap();
        assert_eq!(calc.default_volatility, 0.75);
    }

    #[test]
    fn test_shared_volume_tracker_creation() {
        let shared = create_shared_volume_tracker();
        assert_eq!(shared.lock().unwrap().volume_buffer.len(), 0);
    }

    #[test]
    fn test_welford_accuracy() {
        let calc = FibonacciVolatilityCalculator::new("BTCUSDT");

        // Known prices with calculable volatility
        let prices = vec![100.0, 101.0, 99.0, 100.5, 100.0];
        let vol = calc.welford_volatility(&prices, 1.0); // No annualization

        assert!(vol.is_some());
        let v = vol.unwrap();
        println!("Welford vol (no annualization): {:.6}", v);
        assert!(v > 0.0);
        assert!(v < 0.1); // Should be small for small price moves
    }

    #[test]
    fn test_zero_and_negative_prices_filtered() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");

        calc.add_price(1000, 0.0); // Should be ignored
        calc.add_price(2000, -100.0); // Should be ignored
        calc.add_price(3000, 50000.0); // Should be added

        assert_eq!(calc.get_unified_buffer_size(), 1);

        calc.add_price_1min(0.0); // Should be ignored
        calc.add_price_1min(-1.0); // Should be ignored
        calc.add_price_1min(50000.0); // Should be added

        assert_eq!(calc.get_1min_buffer_size(), 1);
    }
}
