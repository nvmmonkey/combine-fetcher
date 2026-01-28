//! Constants for the combine-fetcher project.

/// Gamma API base URL for Polymarket events
pub const GAMMA_API_URL: &str = "https://gamma-api.polymarket.com";

/// Polymarket crypto API base URL
pub const POLY_CRYPTO_API_URL: &str = "https://polymarket.com/api/crypto";

/// Default volatility for BTC (0.55 = 55% annualized)
pub const VOLATILITY_BTC: f64 = 0.55;

/// Market window size in seconds (15 minutes)
pub const MARKET_WINDOW_SECONDS: i64 = 900;

/// Minutes per year for time-to-expiry calculations
pub const MINUTES_PER_YEAR: f64 = 525_600.0;

/// Maximum retry attempts for fetching strike price
pub const STRIKE_RETRY_ATTEMPTS: u32 = 20;

/// Retry interval in milliseconds for strike price fetching
pub const STRIKE_RETRY_INTERVAL_MS: u64 = 200;

/// HTTP request timeout in seconds
pub const HTTP_TIMEOUT_SECS: u64 = 5;

/// Minimum logging interval in milliseconds (rate limiting)
pub const MIN_LOG_INTERVAL_MS: i64 = 100;

/// CSV recording interval in seconds.
/// - Set to 0 for event-driven mode (record on every update with duplicate guard)
/// - Set to positive value for time-interval mode (record at fixed interval)
pub const CSV_RECORD_INTERVAL_SEC: u64 = 0;
