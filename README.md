# Combine Fetcher

Real-time data logger streaming from Binance and Polymarket WebSockets to CSV.

## Current Status

| Component | Status |
|-----------|--------|
| Binance WebSocket | Implemented |
| Polymarket CLOB WebSocket | Implemented |
| Constants Module | Implemented |
| Volatility Calculator | Implemented (FIX3) |
| Greeks Calculation | Fixed (FIX3) |
| Volume Tracking | Fixed (FIX3) |
| Build | PASS (26/26 tests) |
| All Critical Issues | RESOLVED |

## Recent Changes

**FIX6 (January 2026)**: Multi-Market Data Logger

- Added `multi_market_logger` binary for collecting data from all 15m BTC markets simultaneously
- Market discovery across multiple time windows with unique identifiers
- Extended CSV format with `market_unique_id` column
- Overnight runner script with auto-restart capability

**FIX3 (January 2026)**: Option Data Calculation - Volatility and Greeks Formula

- Created `src/volatility.rs` - Fibonacci volatility calculator (525 lines) with Welford's algorithm
- Fixed Greeks formulas (Delta, Gamma, Vega) for accurate options pricing
- Resolved deadlock risk in `get_market_snapshot()` with proper lock ordering
- Implemented multi-symbol volatility tracking via HashMap
- Fixed volume reporting to use rolling window (volume_1min) instead of cumulative

**Status:** All critical and medium priority issues resolved. 26/26 tests passing.

See [FIX6.md](./FIX6.md) for detailed implementation notes, or [LASTFIX.md](./LASTFIX.md) for a summary.

**FIX1 (January 2025)**: Resolved import dependencies on external `poly_crypto_arb` crate. All modules are now self-contained:

- Created `src/api/binance.rs` - Binance data manager with WebSocket connection
- Created `src/api/polymarket.rs` - Polymarket orderbook stream using vendor SDK
- Created `src/constants.rs` - API URLs and volatility constants
- Fixed imports in `src/main.rs` - Resolved Arc wrapper issues

See [FIX1.md](./FIX1.md) for detailed implementation notes.

## Architecture

```
┌─────────────────┐     ┌──────────────────────┐
│  Binance WS     │     │  Polymarket CLOB WS  │
│  @bookTicker    │     │  orderbook updates   │
└────────┬────────┘     └──────────┬───────────┘
         │                         │
         ▼                         ▼
    ┌────────────────────────────────────┐
    │         SharedState (Arc<RwLock>)  │
    │  - BinanceSnapshot                 │
    │  - PolymarketSnapshot              │
    │  - MarketInfo                      │
    └────────────────┬───────────────────┘
                     │
                     ▼
              ┌──────────────┐
              │  CSV Logger  │
              │ (on update)  │
              └──────────────┘
```

## Features

- **Real-time streaming** - logs on every Binance price update
- **Dual WebSocket** - Binance + Polymarket CLOB
- **No SDK required** - uses raw WebSocket connections
- **Strike price retry** - 20 attempts, 500ms interval, with fallback to 0.0
- **Greeks calculation** - delta, gamma, vega, theta
- **Rate limiting** - minimum 100ms between logs to avoid duplicates

## Build & Run

```bash
cargo build --release

# Run the data_logger binary
./target/release/data_logger

# Custom duration and output
./target/release/data_logger 1800 btc_data.csv

# With debug logging
RUST_LOG=debug ./target/release/data_logger
```

## CSV Output

Same format as your Python mock data generator:

| Field | Description |
|-------|-------------|
| timestamp | ISO timestamp with milliseconds |
| market_slug | `btc-updown-15m-{window_start}` |
| binance_price | Real-time BTC mid price |
| yes_bid/ask | Polymarket YES orderbook |
| no_bid/ask | Polymarket NO orderbook |
| time_to_expiry_minutes | Minutes until resolution |
| strike_price | Market creation price |
| delta_yes, gamma, vega, theta | Greeks |

## Project Structure

```
combine-fetcher/
├── src/
│   ├── main.rs              # Binary entry point (data_logger)
│   ├── bin/
│   │   └── multi_market_logger.rs  # Multi-market collector (NEW in FIX6)
│   ├── lib.rs               # Library exports
│   ├── volatility.rs        # Fibonacci volatility calculator (NEW in FIX3)
│   ├── api/
│   │   ├── binance.rs       # Binance WebSocket client
│   │   └── polymarket.rs    # Polymarket CLOB streamer
│   └── constants.rs         # API endpoints and volatility constants
├── Cargo.toml               # Package configuration
├── vendor/
│   └── rs-clob-client/      # Polymarket SDK (vendored)
├── run_overnight.sh         # Overnight collection script (NEW in FIX6)
├── FIX1.md                  # Initial fix: Import resolution
├── FIX2.md                  # WebSocket connection improvements
├── FIX3.md                  # Option data calculation fixes
├── FIX6.md                  # Multi-market logger
├── LASTFIX.md               # Summary of latest fixes
├── MULTI_MARKET_LOGGER.md   # Multi-market documentation
└── README.md                # This file
```

## WebSocket Endpoints

- **Binance**: `wss://stream.binance.com:9443/ws/btcusdt@bookTicker`
- **Polymarket**: `wss://ws-subscriptions-clob.polymarket.com/ws/market`

## Example Output

```
╔═══════════════════════════════════════════════════════════╗
║  Combine Fetcher - Data Logger                          ║
╚═══════════════════════════════════════════════════════════╝

Duration: 900s (15.0m) | Output: market_data.csv

⏳ Fetching market...
Market: btc-updown-15m-1768647600 | YES: 1234567890... | NO: 0987654321...
⏳ Fetching strike price (20 retries)...
✅ Strike fetched on attempt 3: $95432.50
⏳ Waiting for connections...
✅ Connected to Binance @bookTicker
✅ Connected to Polymarket WebSocket

#50 BTC=$95445.25 Y=0.480/0.490 N=0.500/0.510 Σ=1.000~ TTX=12.5m K=$95432.50
#100 BTC=$95467.50 Y=0.485/0.495 N=0.495/0.505 Σ=1.000~ TTX=12.0m K=$95432.50
...
✅ Logged 1800 ticks to market_data.csv
```

## Compared to REST Polling Version

| Feature | REST Version | WebSocket Version |
|---------|--------------|-------------------|
| Update rate | 5s interval | Real-time (~10ms) |
| Rows per 15min | ~180 | ~9000+ |
| Polymarket latency | 5s+ | <100ms |
| API rate limits | Risk of hitting | None |

## Dependencies

Key runtime dependencies:
- `polymarket-client-sdk` - Vendored Polymarket CLOB SDK
- `tokio` - Async runtime
- `tokio-tungstenite` - WebSocket client
- `reqwest` - HTTP client
- `rust_decimal` - Precise decimal arithmetic
- `dashmap` - Concurrent hashmap for shared state

## Multi-Market Data Logger

The `multi_market_logger` binary collects data from all 15m BTC markets simultaneously with unique identifiers.

### Usage

```bash
# Run for 8 hours (default), output to multi_btc_data.csv
cargo run --bin multi_market_logger --release

# Custom duration (seconds), output file, and number of markets
cargo run --bin multi_market_logger --release -- 28800 custom_data.csv 15
```

### Overnight Runner Script

```bash
# Automated 8-hour collection with auto-restart
./run_overnight.sh

# Custom configuration
./run_overnight.sh 8 overnight_data.csv 15
```

### Features

- Multi-market discovery across time windows
- Extended CSV format with `market_unique_id` column
- Single shared Binance connection for all markets
- Auto-restart on failure

See [MULTI_MARKET_LOGGER.md](./MULTI_MARKET_LOGGER.md) for full documentation.