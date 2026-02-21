//! Combine-fetcher library
//!
//! This library provides modules for fetching and combining data from
//! various sources including Binance and Polymarket.

pub mod api;
pub mod constants;

pub use api::{BinanceDataManager, BinancePrice, PolymarketOrderbookStream};
pub use api::{MarketInfo, fetch_current_market, fetch_next_market, fetch_outcome};
pub use api::{RetryConfig, fetch_strike, fetch_strike_with_retry};
pub use constants::*;

pub mod volatility;

pub use volatility::{
    FibonacciVolatilityCalculator,
    VolumeTracker,
    MarketSnapshot,
    PriceTick,
    VolumeTick,
};


pub use api::{RtdsTracker, RtdsSnapshot};
