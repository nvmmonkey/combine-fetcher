# Order Book Depth/Flow Dynamics Integration Plan

**Date**: 2026-02-23
**Motivation**: Top Polymarket trader achieves 75% directional accuracy using order book signals we don't capture.
**Critical finding**: Full L2 order book data is ALREADY received via WebSocket but immediately discarded.

## 1. Current State

### What We Already Receive (and discard)

**BOOK_STREAM** (`subscribe_orderbook`): Delivers `BookUpdate { bids: Vec<OrderBookLevel>, asks: Vec<OrderBookLevel> }` with the **full L2 order book** at every level. Current code at `src/api/polymarket.rs:855-884` iterates all levels but only extracts `best_bid_size` and `best_ask_size`.

**PRICE_STREAM** (`subscribe_best_bid_ask`): Best bid/ask/spread/timestamp per token. Stored in `TokenPrices`.

**Dashboard**: Already computes `obi` and `cvd` but only from best-level sizes (not full depth).

### SDK Capabilities (confirmed from source)

- `subscribe_orderbook` (WS): Full L2 book — **ALREADY CONNECTED, data discarded**
- `subscribe_last_trade_price` (WS): Trade-by-trade with price, size, side, fee_rate_bps — **NOT subscribed**
- `order_book` / `order_books` (REST): Full book snapshot — available as fallback
- SDK multiplexes all market subscriptions over single `ChannelType::Market` connection

## 2. New Data Fields

### Primary — From existing BOOK_STREAM (zero new connections)

| Field | Type | Description |
|-------|------|-------------|
| `yes_bid_depth_3` | f64 | Total bid size across top 3 price levels (YES) |
| `yes_ask_depth_3` | f64 | Total ask size across top 3 price levels (YES) |
| `no_bid_depth_3` | f64 | Total bid size across top 3 price levels (NO) |
| `no_ask_depth_3` | f64 | Total ask size across top 3 price levels (NO) |
| `yes_bid_depth_all` | f64 | Total bid size across ALL levels (YES) |
| `yes_ask_depth_all` | f64 | Total ask size across ALL levels (YES) |
| `no_bid_depth_all` | f64 | Total bid size across ALL levels (NO) |
| `no_ask_depth_all` | f64 | Total ask size across ALL levels (NO) |
| `yes_bid_levels` | u16 | Number of distinct bid price levels (YES) |
| `yes_ask_levels` | u16 | Number of distinct ask price levels (YES) |
| `no_bid_levels` | u16 | Number of distinct bid price levels (NO) |
| `no_ask_levels` | u16 | Number of distinct ask price levels (NO) |
| `yes_book_imbalance` | f64 | `(bid_all - ask_all) / (bid_all + ask_all)` for YES |
| `no_book_imbalance` | f64 | Same for NO |
| `combined_obi` | f64 | Cross-token imbalance |

### Secondary — From new `subscribe_last_trade_price` (reuses existing WS connection)

| Field | Type | Description |
|-------|------|-------------|
| `yes_last_trade_price` | f64 | Last executed trade price (YES) |
| `yes_last_trade_size` | f64 | Size of last trade |
| `yes_last_trade_side` | String | BUY or SELL (taker aggression) |
| `no_last_trade_price` | f64 | Same for NO |
| `no_last_trade_size` | f64 | Size of last NO trade |
| `no_last_trade_side` | String | BUY or SELL |
| `trade_flow_imbalance` | f64 | Rolling: `(buy_vol - sell_vol) / total_vol` |

### Tertiary — Computed deltas (previous-tick state)

| Field | Type | Description |
|-------|------|-------------|
| `yes_bid_depth_delta` | f64 | Change in yes_bid_depth_all since last tick |
| `yes_ask_depth_delta` | f64 | Change in yes_ask_depth_all since last tick |
| `book_pressure_shift` | f64 | `imbalance_now - imbalance_prev` |

## 3. Implementation Phases

### Phase 1: Extract Depth from Existing Book Stream

**Risk**: Zero. **New connections**: Zero. **Latency impact**: Zero.

**File**: `src/api/polymarket.rs`
- Expand `TokenPrices` struct (line 70) with depth fields
- Modify BOOK_STREAM handler (lines 855-884) to compute depth sums from already-received `BookUpdate` vectors

```rust
// After building bid_levels and ask_levels vectors:
let bid_depth_3: f64 = bid_levels.iter().take(3)
    .map(|l| l.size.to_f64().unwrap_or(0.0)).sum();
let bid_depth_all: f64 = bid_levels.iter()
    .map(|l| l.size.to_f64().unwrap_or(0.0)).sum();
// ... store in prices struct
```

### Phase 2: Wire to CSV Logger

**File**: `src/bin/multi_market_logger_hourly.rs`
- Add 15 new columns to `CsvRecord` (appended at end for backward compatibility)
- Update `csv_header()` and `to_csv_line()`
- Wire from `poly_stream.get_prices()` in main loop

### Phase 3: Add LastTradePrice Stream

**File**: `src/api/polymarket.rs`
- Add third stream task in `stream()` method (same pattern as BOOK_STREAM)
- Uses `subscribe_last_trade_price` — reuses existing WS connection via SDK multiplexing
- Store trade flow data in `TokenPrices`

### Phase 4: Delta Computation + Full CSV

- Track previous-tick state in logger main loop
- Add remaining 11 columns (trade flow + deltas)
- Total new columns: 26 (53 → 79)

### Phase 5: Dashboard + Backtester

- Upgrade dashboard OBI from best-level to full-depth
- Replace synthetic CVD with actual trade flow imbalance
- New backtest parameters: `obi_sell_trigger`, `depth_delta_gate`, `trade_flow_gate`

## 4. Key Files

| File | Lines | What to Change |
|------|-------|---------------|
| `src/api/polymarket.rs` | 70-109 | Expand `TokenPrices` struct |
| `src/api/polymarket.rs` | 855-884 | Extract depth from BOOK_STREAM |
| `src/api/polymarket.rs` | `stream()` | Add trade stream subscription |
| `src/bin/multi_market_logger_hourly.rs` | 33-107 | Add CsvRecord columns |
| `src/bin/multi_market_logger_hourly.rs` | 814-873 | Wire depth in main loop |
| `src/bin/dashboard.rs` | 741 | Upgrade OBI to full-depth |

## 5. Impact Assessment

- **Memory**: ~240 bytes additional per token pair (trivial)
- **Latency**: Phase 1-2 zero impact. Phase 3 reuses existing WS.
- **CSV size**: ~200 bytes/row additional (~50% increase)
- **Backward compat**: All columns appended at end, existing scripts unaffected

## 6. Strategy Value

Once we have 1-2 weeks of logged depth data:
1. Correlate OBI shifts with trader's sell timing from the Feb 21 dataset
2. If confirmed: add `obi_trigger` as supplementary DIR signal alongside z-score
3. Build composite: `combined_score = f(z_score, book_imbalance, flow_direction, depth_shift)`
4. THEN lower z-thresholds (zv694 showed this fails without the supplementary signal)
