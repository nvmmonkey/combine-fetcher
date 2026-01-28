# Combine Fetcher - Current State Summary

This document describes the current state of the combine-fetcher program after all fixes and improvements.

---

## Overview

The `combine-fetcher` is a Rust application that combines real-time market data from Binance and Polymarket for options pricing analysis. It supports two modes of operation:

1. **Single Market Logger** (`data_logger`) - Logs data for a single market
2. **Multi-Market Logger** (`multi_market_logger`) - Automatically rotates through markets as they expire

---

## Architecture

### Data Sources

| Source | WebSocket Streams | Data Provided |
|--------|------------------|---------------|
| **Binance** | `@aggTrade`, `@bookTicker` | Price, volume, volatility, bid/ask |
| **Polymarket** | Price feed, Orderbook feed (via SDK v0.4.1) | YES/NO bid/ask, sizes |

### Core Modules

| Module | Purpose |
|--------|---------|
| `src/api/binance.rs` | Binance WebSocket + REST, volatility tracking, volume tracking |
| `src/api/polymarket.rs` | Polymarket SDK v0.4.1 wrapper, dual-feed architecture, health monitoring |
| `src/volatility.rs` | Fibonacci multi-timeframe volatility calculator with Welford's algorithm |
| `src/latency_tracker.rs` | WebSocket latency tracking |
| `src/bin/multi_market_logger.rs` | Multi-market data logger with auto-rotation |
| `src/main.rs` | Single-market data logger |

### File Structure

```
src/
├── lib.rs                    # Library exports
├── main.rs                   # Single-market data logger
├── constants.rs              # Configuration constants
├── volatility.rs             # Fibonacci volatility calculator
├── latency_tracker.rs        # Latency tracking
├── api/
│   ├── mod.rs                # API module exports
│   ├── binance.rs            # Binance WebSocket + REST
│   └── polymarket.rs         # Polymarket SDK wrapper
└── bin/
    ├── multi_market_logger.rs  # Multi-market logger (main binary)
    └── test_binance_bootstrap.rs  # Testing utility
```

---

## Key Features

### 1. Real-Time Volatility Calculation

- **Fibonacci multi-timeframe**: 1s, 5s, 13s, 34s, 89s windows
- **Welford's algorithm**: Online variance calculation for numerical stability
- **Historical bootstrap**: Fetches 1000 candles (~16.7 hours) on startup via REST API
- **Volatility regimes**: LOW (<15%), NORMAL (15-30%), HIGH (30-60%), EXTREME (>60%)
- **Blended volatility**: Weighted average using Fibonacci weights (1:2:3:5:8)

### 2. Volume Tracking

- **1-second and 1-minute rolling windows**
- **Spike detection**: Alerts when volume exceeds 2x baseline
- **Real-time updates**: From Binance `@aggTrade` stream

### 3. Options Greeks Calculation

Binary/cash-or-nothing option Greeks using Black-Scholes:

- **Delta**: `n'(d2)` - Price sensitivity (clamped 0-1)
- **Gamma**: Second derivative - Can be negative for OTM binary options
- **Vega**: Volatility sensitivity (positive for vanilla options)
- **Theta**: Time decay (negative, per day)

### 4. Market Rotation (Multi-Market Logger)

- Automatic detection of market expiry (`resolved = true`)
- Fetches next active market via Polymarket API
- **Cooldown mechanism**: 5-second delay between failed fetch attempts
- **Max retry limit**: Exits after 3 consecutive failures
- **Infinite strike retry**: Keeps retrying strike fetch until market expires with exponential backoff

---

## CSV Output Format

### Quick Reference Table (45 Columns)

| # | Column Name | Type | Units | Section |
|---|-------------|------|-------|---------|
| 1 | `timestamp` | String (ISO 8601) | UTC | Market Identity |
| 2 | `market_slug` | String | - | Market Identity |
| 3 | `binance_price` | Float | USD (2 decimals) | Binance Data |
| 4 | `binance_volume` | Float | BTC units (2 decimals) | Binance Data |
| 5-8 | `yes_bid`, `yes_bid_size`, `yes_ask`, `yes_ask_size` | Float | Price (0-1), token count | Polymarket YES |
| 9-12 | `no_bid`, `no_bid_size`, `no_ask`, `no_ask_size` | Float | Price (0-1), token count | Polymarket NO |
| 13 | `time_to_open` | Integer | Seconds | Timing |
| 14 | `time_to_expiry_minutes` | Float | Minutes (2 decimals) | Timing |
| 15 | `strike_price` | Float | USD (2 decimals) | Option Parameters |
| 16 | `realized_volatility` | Float | Annual (6 decimals) | Option Parameters |
| 17-20 | `delta_yes`, `gamma`, `vega`, `theta` | Float | Dimensionless/USD/day | Greeks |
| 21 | `market_resolved` | Boolean | true/false | Market State |
| 22 | `outcome` | String | "YES"/"NO"/"" | Market State |
| 23-27 | `timestamp_unix_ms`, `binance_update_ms`, `poly_yes_update_ms`, `poly_no_update_ms`, `latency_binance_poly_ms` | Integer | Milliseconds | Latency |
| 28-30 | `binance_bid`, `binance_ask`, `binance_spread_bps` | Float | USD, basis points | Binance |
| 31-36 | `vol_1sec`, `vol_5sec`, `vol_13sec`, `vol_34sec`, `vol_89sec`, `volatility_regime` | Float/String | Annual, enum | Volatility |
| 37-38 | `volume_1sec`, `volume_spike` | Float/Boolean | BTC units, boolean | Volume |
| 39-41 | `signal_quality`, `trade_confidence`, `vol_data_source` | String/Float | Enum, 0-1, enum | Signal |
| 42-43 | `yes_spread_bps`, `no_spread_bps` | Float | Basis points | Polymarket |

### time_to_open Semantics

| Value | Meaning | Trading State |
|-------|---------|---------------|
| > 0 | Market opens in N seconds | PRE-MARKET (no trading) |
| = 0 | Market is opening now | MARKET OPEN |
| < 0 | Market opened N seconds ago | ACTIVE TRADING |

**Important:** Negative values are **correct behavior** - they indicate the market is active.

---

## Usage

### Build

```bash
cd /home/lehash/development/combine-fetcher
cargo build --release
```

### Multi-Market Logger (Recommended)

```bash
# Default: 8 hours, 1-second interval
./target/release/multi_market_logger

# Custom duration (seconds), output file, interval (seconds)
./target/release/multi_market_logger 28800 overnight_data.csv 10
```

### Single Market Logger

```bash
# Duration (seconds), output file, recording interval (optional)
./target/release/data_logger 60 output.csv 1
```

### Recording Intervals

| Interval | Records/Second | Use Case |
|----------|----------------|----------|
| `0` (default) | 10-100 | Event-driven, high-frequency |
| `1` | ~1 | Standard time-series logging |
| `5` | ~0.2 | Reduced file size |
| `60` | ~0.017 | Long-term monitoring |

---

## Configuration Constants

### Volatility (`src/volatility.rs`)

| Constant | Value | Description |
|----------|-------|-------------|
| `BUFFER_SIZE_1SEC` | 100 | 1-second buffer capacity |
| `BUFFER_SIZE_89SEC` | 8900 | 89-second buffer capacity |
| `MIN_VOLATILITY` | 0.05 | Minimum annualized volatility |
| `MAX_VOLATILITY` | 3.0 | Maximum annualized volatility |
| `STALE_THRESHOLD_MS` | 5000 | Data staleness threshold |

### Default Volatility by Symbol

| Symbol | Default |
|--------|---------|
| BTCUSDT | 0.55 (55%) |
| ETHUSDT | 0.65 (65%) |
| SOLUSDT | 0.75 (75%) |
| Other | 0.65 (65%) |

### Polymarket (`src/api/polymarket.rs`)

| Constant | Value | Description |
|----------|-------|-------------|
| `SILENT_DISCONNECT_THRESHOLD_MS` | 30000 | Silent disconnect detection (30s) |
| `HEALTH_CHECK_INTERVAL_SECS` | 5 | Health check frequency |

### Multi-Market Logger

| Constant | Value | Description |
|----------|-------|-------------|
| `MAX_CONSECUTIVE_FETCH_FAILURES` | 3 | Exit after N failed market fetches |
| `FETCH_RETRY_COOLDOWN_MS` | 5000 | Cooldown between failed fetch attempts |
| `STRIKE_FETCH_DELAY_SEC` | 30 | Delay before fetching strike price |

---

## Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| `polymarket-client-sdk` | 0.4.1 | Polymarket CLOB + WebSocket |
| `tokio` | 1.35 | Async runtime |
| `reqwest` | 0.11 | HTTP client |
| `dashmap` | 6.1 | Concurrent HashMap |
| `tracing` | 0.1 | Structured logging |

---

## Important Behaviors

### Strike Price Fetching
- Strike price is fetched **30 seconds after market start**
- Uses **infinite retry** with exponential backoff (500ms -> 30s max)
- Stops retrying if market expires
- Result is sent via channel and updated in main loop
- Falls back to spot price if fetch fails

### Health Monitoring
- Checks WebSocket connection health every 5 seconds
- Detects silent disconnects (no updates for >30s)
- Logs warnings but continues operating
- SDK handles automatic reconnection

### Market Rotation
- When market expires, fetches next market with `end_tick_gt` parameter
- 5-second cooldown between failed attempts
- Exits gracefully after 3 consecutive failures
- Cleans up old subscription tasks automatically

---

## Known Limitations

1. **Strike Price API**: May fail repeatedly; falls back to spot price after market expiry
2. **Quiet Markets**: Health monitor may show warnings during low-volume periods
3. **Volatility Warm-up**: Requires ~90 seconds to fill all buffers after bootstrap
4. **Single Symbol**: Current implementation tracks one symbol at a time (BTCUSDT default)

---

## Testing

Run unit tests:
```bash
cargo test
```

Expected: 26 passed tests (volatility calculator, volume tracker, bootstrap)

---

## Fix History Summary

| Date | Fix | One-Line Summary |
|------|-----|------------------|
| 2025-01-17 | FIX1 | Initial import resolution and option data structure fixes |
| 2025-01-17 | FIX2 | Added Binance @aggTrade stream for volume, implemented Polymarket dual-feed architecture |
| 2025-01-17 | FIX3 | Implemented Fibonacci volatility calculator, corrected Greeks formulas, fixed deadlock risk |
| 2025-01-17 | FIX4 | Added configurable CSV recording frequency via CLI argument |
| 2025-01-17 | FIX5 | Verified time_to_open semantics (negative values = active trading) |
| 2026-01-26 | FIX6 | Verified Polymarket SDK v0.4.1 official usage |
| 2026-01-26 | FIX7 | Fixed strike price channel communication, updated silent disconnect threshold to 30s |
| 2026-01-26 | FIX8 | Code quality refactoring |
| 2026-01-26 | FIX9 | Fixed market rotation race condition, added cooldown and max retry limit, infinite strike retry |
| 2026-01-26 | FIX10 | Documentation consolidation |
| 2026-01-27 | FIX11 | Fixed strike price to use blocking fetch before logging starts, ensuring data consistency with main.rs pattern |
| 2026-01-27 | FIX12 | Implemented infinite retry for strike price fetch with exponential backoff (500ms→30s), removed fallback to 0.0 |

---

## Related Documentation

- [README.md](./README.md) - Project overview
- [CSV_HEADER_DOCUMENTATION.md](./CSV_HEADER_DOCUMENTATION.md) - Complete CSV column definitions (45 fields)
- [polymarket-client-sdk docs](https://docs.rs/polymarket-client-sdk/0.4.1/polymarket_client_sdk/) - Official SDK documentation
