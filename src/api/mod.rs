//! API modules for external data sources
//!
//! This module contains WebSocket clients for various external APIs
//! including cryptocurrency exchanges and prediction markets.

pub mod binance;
pub mod gamma;
pub mod market_events;
pub mod polymarket;
pub mod rtds_tracker;
pub mod strike;

pub use binance::{BinanceDataManager, BinancePrice};
pub use gamma::{MarketInfo, fetch_current_market, fetch_next_market, fetch_outcome};
pub use market_events::{MarketEventStream, NewMarketEvent, MarketResolvedEvent};
pub use polymarket::{PolymarketOrderbookStream, PolyPriceUpdate, TokenPrices};
pub use rtds_tracker::{RtdsTracker, RtdsSnapshot};
pub use strike::{RetryConfig, fetch_strike, fetch_strike_with_retry};
