# Multi-Market Logger

Binary application for collecting data from **FUTURE ONLY** 15m BTC markets on Polymarket simultaneously.

## Features

- **Future-Only Discovery**: Only discovers upcoming 15m BTC markets (historical/expired markets skipped - no streaming data)
- **Delayed Strike Fetch**: Strike prices fetched 30 seconds AFTER market starts (when API data is available)
- **Unique Identifiers**: Each market has a unique ID (`btc-updown-15m-{window_start}`) for tracking
- **Shared Infrastructure**: Single BinanceDataManager and PolymarketOrderbookStream for all markets
- **Sticky Price Fetching**: Retry logic with fallback values for failed strike fetches (20 attempts, 500ms interval)
- **CSV Output**: Extended format with `market_unique_id` column for market identification
- **Overnight Operation**: 8-hour runtime (28800 seconds) with auto-restart capability

## Usage

### Direct Execution

```bash
# Run for 8 hours (default), output to multi_btc_data.csv, collect 10 markets
cargo run --bin multi_market_logger --release

# Custom duration (in seconds), output file, and number of markets
cargo run --bin multi_market_logger --release -- 28800 custom_data.csv 15
```

### Parameters

1. **duration_secs** (optional): Runtime in seconds (default: 28800 = 8 hours)
2. **output.csv** (optional): CSV output file path (default: `multi_btc_data.csv`)
3. **num_markets** (optional): Number of markets to discover (default: 10)

### Overnight Runner Script

Use the provided shell script for automated overnight collection with auto-restart:

```bash
# Run for 8 hours, default output file, 10 markets
./run_overnight.sh

# Custom configuration
./run_overnight.sh 8 overnight_data.csv 15
```

The script provides:
- Automatic restart on failure
- Backup of existing CSV files
- Detailed logging to timestamped log files
- Progress tracking with timestamps

## CSV Format

Output includes extended format with market identification:

```csv
timestamp,market_unique_id,market_slug,binance_price,binance_volume,yes_bid,yes_bid_size,yes_ask,yes_ask_size,no_bid,no_bid_size,no_ask,no_ask_size,time_to_open,time_to_expiry_minutes,strike_price,realized_volatility,delta_yes,gamma,vega,theta,market_resolved,outcome
2026-01-19T10:15:23.456,btc-updown-15m-1737260400,btc-updown-15m-1737260400,99500.50,1234567.89,0.52,1000,0.53,500,0.47,800,0.48,1200,-300,5.2,99400.00,0.55,0.0234,0.0012,0.0045,-0.0008,false,
```

### New Fields

- **market_unique_id**: Unique identifier for each market (`btc-updown-15m-{window_start}`)
- All other fields match the single-market logger format

## Implementation Details

### Market Discovery (Future-Only)

The logger searches for markets by:
1. Starting from the **next** 15-minute window (not current)
2. Looking **forward only** for future markets
3. Skipping historical/expired markets (they won't stream data)
4. Continuing until `num_markets` upcoming markets are found

Each discovered market gets a unique ID based on its window start timestamp.

### Strike Price Fetching (Delayed)

Strike prices are fetched dynamically during data collection:
- **Trigger**: 30 seconds AFTER market window starts
- **Reasoning**: API data is not available until market is active
- **Retry Logic**: 20 attempts with 500ms intervals
- **Fallback**: Uses current BTC spot price if fetch fails

### Data Collection

- **Single Binance Connection**: One `BinanceDataManager` for BTCUSDT shared across all markets
- **Multi-Token Subscription**: All YES/NO token IDs subscribed via single `PolymarketOrderbookStream`
- **Per-Market Recording**: Each market's data recorded to same CSV with unique identifier
- **Dynamic Strike Updates**: Strike prices fetched and updated when markets become active

### Error Handling

- Individual market fetch failures don't stop collection
- Failed strike prices use fallback value (0.0 initially, then current spot)
- Warnings logged but collection continues for remaining markets
- Auto-restart script handles crashes and network issues

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Multi-Market Logger                       │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────┐    ┌──────────────────────────────────┐  │
│  │ Market       │    │ PolymarketOrderbookStream        │  │
│  │ Discovery    ├───▶│ (all tokens from all markets)    │  │
│  │ (10 markets) │    └──────────────────────────────────┘  │
│  └──────────────┘                                        │
│         │                                                 │
│         ▼                                                 │
│  ┌──────────────────────────────────────────────────┐    │
│  │              MarketInfo Vec                       │    │
│  │  - unique_id, yes_token_id, no_token_id          │    │
│  │  - window_start, window_end, strike_price        │    │
│  └──────────────────────────────────────────────────┘    │
│                           │                                │
│                           ▼                                │
│  ┌──────────────────────────────────────────────────┐    │
│  │              Main Collection Loop                 │    │
│  │  For each market:                                │    │
│  │    1. Get shared Binance price                   │    │
│  │    2. Get YES/NO prices from stream              │    │
│  │    3. Calculate Greeks                           │    │
│  │    4. Write CSV record with market_unique_id     │    │
│  └──────────────────────────────────────────────────┘    │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

## Monitoring

During execution, the logger logs progress every 100 ticks:

```
#100 BTC=99500.50 Active markets: 8
#200 BTC=99510.25 Active markets: 8
```

The `Active markets` count shows how many markets have not yet expired.

## Troubleshooting

### No Markets Discovered

If the logger finds no markets:
- Check network connectivity to Gamma API
- Verify Polymarket is online
- Try increasing `num_markets` to search more windows

### Strike Price Fetch Failures

Markets with failed strike fetches will show warnings:
```
⚠️ Strike fetch failed for btc-updown-15m-1737260400: ...
```

These markets will use the current BTC spot price as the strike (fallback).

### Connection Issues

The PolymarketOrderbookStream has built-in:
- Auto-reconnection with exponential backoff
- REST API fallback when WebSocket is disconnected
- Health monitoring with silent disconnect detection

Check the logs for reconnection events and health status.

## File Locations

- **Binary**: `/home/lehash/development/combine-fetcher/src/bin/multi_market_logger.rs`
- **Runner Script**: `/home/lehash/development/combine-fetcher/run_overnight.sh`
- **Documentation**: `/home/lehash/development/combine-fetcher/MULTI_MARKET_LOGGER.md`

## Example Output

After running overnight (8 hours) with 10 markets:
- Duration: 8.0 hours
- Markets tracked: 10
- Records per market: ~2,400 (assuming 10ms interval)
- Total records: ~24,000
- Output: `overnight_btc_data_YYYYMMDD_HHMMSS.csv`
