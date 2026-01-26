# Fix Plan: Data Streaming Issues - Volume and Orderbook Updates

## Problem Summary

**Current vs Expected Behavior:**

The `market_data.csv` shows incorrect streaming data:
- `binance_volume=0.00` (no volume data being captured)
- `yes_bid=0.01` and `yes_ask=0.99` (stuck at default/min-max values)
- Data appears to be using fallback/default values instead of real-time updates

**Expected Behavior:**
- Real-time Binance volume data from @aggTrade WebSocket stream
- Dynamic Polymarket YES/NO bid/ask prices updating based on actual orderbook
- Price spread and size data reflecting real market liquidity

---

## Root Cause Analysis

### 1. Missing Volume Stream (FIXED)

**Problem:** The combine-fetcher only implemented `@bookTicker` stream but NOT `@aggTrade` stream for volume.

**Fix Applied:**
- Added `BinanceAggTradeEvent` struct to parse trade messages
- Added second WebSocket connection for `@aggTrade` stream
- Added `volume` field to `BinancePrice` struct
- Implemented `aggtrade_connection_loop()` for volume updates

### 2. Incomplete Polymarket Integration (FIXED)

**Problem:** Polymarket orderbook was not using dual-feed architecture (price + orderbook streams).

**Fix Applied:**
- Replaced simple orderbook-only stream with dual-feed architecture
- Added `subscribe_prices()` for best bid/ask prices
- Maintained `subscribe_orderbook()` for liquidity sizes
- Added health monitoring and REST API fallback

### 3. Borrow Errors in binance.rs (FIXED)

**Problem:** After adding volume tracking, `symbol` was moved before being used in debug! macro.

**Fix Applied:**
- Changed `prices_guard.insert(symbol, price)` to `prices_guard.insert(symbol.clone(), price)`
- Same fix for aggTrade message handler

---

## Implementation Status

| Component | Status | Notes |
|-----------|--------|-------|
| Binance @aggTrade stream | DONE | Captures real-time volume |
| Binance volume field | DONE | Added to BinancePrice struct |
| Polymarket dual stream | DONE | Price + orderbook feeds |
| REST API fallback | DONE | For stale WebSocket data |
| Health monitoring | DONE | Silent disconnect detection |
| Compilation | FIXED | All borrow errors resolved |

---

## Files Modified

| File | Changes |
|------|---------|
| `src/api/binance.rs` | Added @aggTrade stream, volume tracking |
| `src/api/polymarket.rs` | Replaced with dual-feed architecture |
| `src/api/mod.rs` | Updated exports (PolymarketMessage → PolyPriceUpdate) |

---

## Testing Command

```bash
cd /home/lehash/development/combine-fetcher
cargo build --release
./target/release/data_logger 60 test_output.csv
```

Expected: CSV should now show real volume values (not 0.00) and dynamic bid/ask prices.
