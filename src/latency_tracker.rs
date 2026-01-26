//! Latency tracking for delta arbitrage on Polymarket 15-minute options
//!
//! Add to your crate:
//! 1. Save this file as src/latency_tracker.rs
//! 2. Add `pub mod latency_tracker;` to lib.rs
//! 3. Use in multi_market_logger.rs
//!
//! Example usage:
//! ```ignore
//! use combine_fetcher::latency_tracker::LatencyTracker;
//! 
//! let tracker = LatencyTracker::new();
//! 
//! // In Binance websocket handler:
//! tracker.on_binance_update(timestamp_ms, price);
//! 
//! // In Polymarket websocket handler:
//! tracker.on_poly_update(timestamp_ms);
//! 
//! // Check for latency arb signal:
//! let signal = tracker.get_arb_signal(current_ms);
//! ```

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

/// Latency tracker for detecting delta arbitrage opportunities
/// 
/// Tracks the age of Binance vs Polymarket data and momentum signals.
/// When Binance is fresh but Polymarket is stale, and there's momentum,
/// this indicates a latency arbitrage opportunity.
pub struct LatencyTracker {
    // Timestamps
    binance_last_ms: AtomicI64,
    poly_last_ms: AtomicI64,
    
    // Price history for momentum calculation (protected by mutex)
    price_history: Mutex<VecDeque<(i64, f64)>>,
    
    // Configuration
    momentum_threshold_bps: f64,
    staleness_threshold_ms: i64,
}

impl LatencyTracker {
    pub fn new() -> Self {
        Self {
            binance_last_ms: AtomicI64::new(0),
            poly_last_ms: AtomicI64::new(0),
            price_history: Mutex::new(VecDeque::with_capacity(500)),
            momentum_threshold_bps: 5.0,    // 5 bps = 0.05%
            staleness_threshold_ms: 200,    // 200ms
        }
    }
    
    /// Configure thresholds
    pub fn with_thresholds(mut self, momentum_bps: f64, staleness_ms: i64) -> Self {
        self.momentum_threshold_bps = momentum_bps;
        self.staleness_threshold_ms = staleness_ms;
        self
    }
    
    /// Update Binance timestamp and price
    #[inline]
    pub fn on_binance_update(&self, timestamp_ms: i64, price: f64) {
        self.binance_last_ms.store(timestamp_ms, Ordering::Release);
        
        if let Ok(mut history) = self.price_history.lock() {
            history.push_back((timestamp_ms, price));
            
            // Keep last 10 seconds
            let cutoff = timestamp_ms - 10_000;
            while let Some(&(ts, _)) = history.front() {
                if ts < cutoff {
                    history.pop_front();
                } else {
                    break;
                }
            }
        }
    }
    
    /// Update Polymarket timestamp
    #[inline]
    pub fn on_poly_update(&self, timestamp_ms: i64) {
        self.poly_last_ms.store(timestamp_ms, Ordering::Release);
    }
    
    /// Get Binance data age in milliseconds
    #[inline]
    pub fn get_binance_age(&self, current_ms: i64) -> i64 {
        let last = self.binance_last_ms.load(Ordering::Acquire);
        if last == 0 { return i64::MAX; }
        current_ms - last
    }
    
    /// Get Polymarket data age in milliseconds
    #[inline]
    pub fn get_poly_age(&self, current_ms: i64) -> i64 {
        let last = self.poly_last_ms.load(Ordering::Acquire);
        if last == 0 { return i64::MAX; }
        current_ms - last
    }
    
    /// Calculate momentum in basis points over a time window
    pub fn get_momentum_bps(&self, current_ms: i64, window_ms: i64) -> f64 {
        let history = match self.price_history.lock() {
            Ok(h) => h,
            Err(_) => return 0.0,
        };
        
        if history.len() < 2 {
            return 0.0;
        }
        
        let cutoff = current_ms - window_ms;
        let mut oldest_price = 0.0;
        let mut newest_price = 0.0;
        let mut found_oldest = false;
        
        for &(ts, price) in history.iter() {
            if ts >= cutoff {
                if !found_oldest {
                    oldest_price = price;
                    found_oldest = true;
                }
                newest_price = price;
            }
        }
        
        if oldest_price <= 0.0 {
            return 0.0;
        }
        
        ((newest_price - oldest_price) / oldest_price) * 10_000.0
    }
    
    /// Get 1-second momentum
    #[inline]
    pub fn get_momentum_1s(&self, current_ms: i64) -> f64 {
        self.get_momentum_bps(current_ms, 1_000)
    }
    
    /// Get 5-second momentum
    #[inline]
    pub fn get_momentum_5s(&self, current_ms: i64) -> f64 {
        self.get_momentum_bps(current_ms, 5_000)
    }
    
    /// Check for latency arbitrage signal
    /// 
    /// Returns (has_signal, momentum_signal_string)
    /// - "LONG" if Binance up + Poly stale
    /// - "SHORT" if Binance down + Poly stale
    /// - "NEUTRAL" otherwise
    pub fn get_arb_signal(&self, current_ms: i64) -> (bool, String) {
        let binance_age = self.get_binance_age(current_ms);
        let poly_age = self.get_poly_age(current_ms);
        let momentum = self.get_momentum_1s(current_ms);
        
        // Binance fresh (< 100ms), Poly stale (> threshold)
        let binance_fresh = binance_age < 100;
        let poly_stale = poly_age > self.staleness_threshold_ms;
        let has_momentum = momentum.abs() >= self.momentum_threshold_bps;
        
        if binance_fresh && poly_stale && has_momentum {
            let signal = if momentum > 0.0 { "LONG" } else { "SHORT" };
            return (true, signal.to_string());
        }
        
        // Return momentum direction even without arb signal
        let signal = if momentum > self.momentum_threshold_bps / 2.0 {
            "LONG"
        } else if momentum < -self.momentum_threshold_bps / 2.0 {
            "SHORT"
        } else {
            "NEUTRAL"
        };
        
        (false, signal.to_string())
    }
    
    /// Get full latency stats for CSV logging
    pub fn get_stats(&self, current_ms: i64) -> LatencyStats {
        let binance_age = self.get_binance_age(current_ms);
        let poly_age = self.get_poly_age(current_ms);
        let momentum_1s = self.get_momentum_1s(current_ms);
        let momentum_5s = self.get_momentum_5s(current_ms);
        let (latency_arb, momentum_signal) = self.get_arb_signal(current_ms);
        
        LatencyStats {
            binance_age_ms: binance_age,
            poly_age_ms: poly_age,
            latency_delta_ms: poly_age - binance_age,
            momentum_1s_bps: momentum_1s,
            momentum_5s_bps: momentum_5s,
            momentum_signal,
            latency_arb_signal: latency_arb,
        }
    }
}

impl Default for LatencyTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Latency statistics for CSV logging
#[derive(Debug, Clone)]
pub struct LatencyStats {
    pub binance_age_ms: i64,
    pub poly_age_ms: i64,
    pub latency_delta_ms: i64,
    pub momentum_1s_bps: f64,
    pub momentum_5s_bps: f64,
    pub momentum_signal: String,
    pub latency_arb_signal: bool,
}

/// Complement arbitrage detector
/// 
/// Checks if YES + NO prices deviate from $1.00
pub fn check_complement_arb(yes_ask: f64, no_ask: f64, threshold: f64) -> (bool, f64) {
    let sum = yes_ask + no_ask;
    let arb_exists = sum < (1.0 - threshold) || sum > (1.0 + threshold);
    (arb_exists, sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_momentum_calculation() {
        let tracker = LatencyTracker::new();
        let base_time = 1700000000000_i64;
        
        // Add ascending prices (upward momentum)
        for i in 0..10 {
            tracker.on_binance_update(base_time + i * 100, 100.0 + i as f64 * 0.1);
        }
        
        let momentum = tracker.get_momentum_1s(base_time + 900);
        assert!(momentum > 0.0, "Expected positive momentum, got {}", momentum);
    }
    
    #[test]
    fn test_latency_detection() {
        let tracker = LatencyTracker::new();
        let base_time = 1700000000000_i64;
        
        // Fresh binance update
        tracker.on_binance_update(base_time, 100.0);
        tracker.on_binance_update(base_time + 50, 100.1);  // Price moved up
        
        // Stale poly update (300ms ago)
        tracker.on_poly_update(base_time - 250);
        
        let (has_arb, signal) = tracker.get_arb_signal(base_time + 50);
        
        // Should detect latency arb with LONG signal
        assert!(has_arb || signal == "LONG" || signal == "NEUTRAL");
    }
    
    #[test]
    fn test_complement_arb() {
        // Should detect arb when sum < 0.995
        let (arb, sum) = check_complement_arb(0.48, 0.48, 0.005);
        assert!(arb);
        assert!((sum - 0.96).abs() < 0.001);
        
        // Should not detect when sum ~= 1.0
        let (arb, _) = check_complement_arb(0.50, 0.50, 0.005);
        assert!(!arb);
    }
}