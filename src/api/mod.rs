//! API modules for external data sources
//!
//! This module contains WebSocket clients for various external APIs
//! including cryptocurrency exchanges and prediction markets.

pub mod binance;
pub mod polymarket;

pub use binance::{BinanceDataManager, BinancePrice};
pub use polymarket::{PolymarketOrderbookStream, PolyPriceUpdate, TokenPrices};
