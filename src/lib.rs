//! Combine-fetcher library
//!
//! This library provides modules for fetching and combining data from
//! various sources including Binance and Polymarket.

pub mod api;
pub mod constants;

pub use api::{BinanceDataManager, BinancePrice, PolymarketOrderbookStream};
pub use constants::*;

pub mod volatility;

pub use volatility::{
    FibonacciVolatilityCalculator,
    VolumeTracker,
    MarketSnapshot,
    PriceTick,
    VolumeTick,
};
