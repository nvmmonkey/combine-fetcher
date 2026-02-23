# Order Book Depth/Flow Dynamics Integration Plan

**Date**: 2026-02-23
**Motivation**: Top trader study (Blushing-Fine) reveals 75% directional accuracy from
order book intelligence that our RTDS logger doesn't capture.

---

## WHY: The Missing Signal

The trader ignores z-score entirely for sell timing decisions. 96% of his sells occur
at |dev_bps| < 2 bps. His directional accuracy comes from **order book depth/flow dynamics**:
- Bid/ask volume imbalance (depth behind best bid vs best ask)
- Queue position and maker fill patterns
- Order flow imbalance (aggressive buy vs sell volume)
- Book pressure changes over time

**Our current RTDS logger only tracks best bid/ask prices** (yes_bid, yes_ask, no_bid, no_ask).
It does NOT capture order book depth, volume at each price level, or fill rate patterns.

**zv694 backtest proved this**: Lowering z-thresholds to match the trader's timing without
adding his signal source caused a -62% regression (-$151 over 5 days). The z-thresholds
exist because z-score is our ONLY signal. We cannot lower them until we have supplementary
signals from the order book.

---

## POLYMARKET CLOB API: What Is Available

### WebSocket Streams (Real-Time, Low Latency)

The Polymarket SDK (`polymarket-client-sdk 0.4.1`) provides via `wss://ws-subscriptions-clob.polymarket.com/ws/market`:

**1. `subscribe_orderbook(asset_ids)` -- Full L2 Order Book (ALREADY SUBSCRIBED)**
Returns `BookUpdate` containing:
- `bids: Vec<OrderBookLevel>` -- ALL bid levels, each with `price` and `size`
- `asks: Vec<OrderBookLevel>` -- ALL ask levels, each with `price` and `size`
- `asset_id`, `market`, `timestamp`

**We already receive full depth but DISCARD everything except best-level size.**

**2. `subscribe_last_trade_price(asset_ids)` -- Trade-by-Trade Feed (NOT SUBSCRIBED)**
Returns `LastTradePrice` containing:
- `price` -- execution price
- `side` -- BUY or SELL (taker aggression direction)
- `size` -- trade size
- `fee_rate_bps` -- fee rate

**This is the trade flow data we need for CVD/taker analysis. Currently NOT subscribed.**

**3. `subscribe_best_bid_ask(asset_ids)` -- Top-of-Book Only (ALREADY SUBSCRIBED)**
No depth or size info. Already used for best_bid/best_ask.

---

## CURRENT LOGGER: What We Capture vs What's Available

### The Critical Gap

The `subscribe_orderbook` handler at `src/api/polymarket.rs` lines 855-884 builds
`bid_levels: Vec<OrderBookLevel>` and `ask_levels: Vec<OrderBookLevel>` containing ALL
depth levels, but only extracts:

```rust
let bid_size = find_size_at_price(&bid_levels, target_bid_price);
let ask_size = find_size_at_price(&ask_levels, target_ask_price);
prices.best_bid_size = bid_size;
prices.best_ask_size = ask_size;
```

**All depth data beyond best level is discarded.** The vectors go out of scope. This is
the single highest-value fix -- data already arrives and is parsed, we just need to extract
metrics before dropping it.

### Current Dashboard Limitations

- **OBI (calc_obi at dashboard.rs:741)**: Uses only best-level size. Misses entire book depth.
- **CVD (dashboard.rs:1838)**: Synthetic proxy from Binance BTC volume, NOT actual Polymarket trades.

---

## NEW FIELDS TO LOG (31 total)

### Group A: Full Book Depth (15 columns, ZERO new connections)

Computed from `bid_levels`/`ask_levels` vectors already in BOOK_STREAM handler.

| CSV Column | Formula |
|------------|---------|
| `yes_bid_depth_3` | Sum of sizes at top 3 YES bid levels |
| `yes_ask_depth_3` | Sum of sizes at top 3 YES ask levels |
| `no_bid_depth_3` | Sum of sizes at top 3 NO bid levels |
| `no_ask_depth_3` | Sum of sizes at top 3 NO ask levels |
| `yes_bid_depth_all` | Sum of sizes at ALL YES bid levels |
| `yes_ask_depth_all` | Sum of sizes at ALL YES ask levels |
| `no_bid_depth_all` | Sum of sizes at ALL NO bid levels |
| `no_ask_depth_all` | Sum of sizes at ALL NO ask levels |
| `yes_bid_levels` | Count of distinct YES bid price levels |
| `yes_ask_levels` | Count of distinct YES ask price levels |
| `no_bid_levels` | Count of distinct NO bid price levels |
| `no_ask_levels` | Count of distinct NO ask price levels |
| `yes_book_imbalance` | `(yes_bid_all - yes_ask_all) / (yes_bid_all + yes_ask_all)` |
| `no_book_imbalance` | `(no_bid_all - no_ask_all) / (no_bid_all + no_ask_all)` |
| `combined_obi` | Cross-token: `(all_bids - all_asks) / total` |

### Group B: Trade Flow (11 columns, 1 new WS connection)

From new `subscribe_last_trade_price` subscription.

| CSV Column | Source |
|------------|--------|
| `yes_last_trade_price` | LastTradePrice.price for YES token |
| `yes_last_trade_size` | LastTradePrice.size for YES token |
| `yes_last_trade_side` | "BUY" or "SELL" from LastTradePrice.side |
| `no_last_trade_price` | Same for NO token |
| `no_last_trade_size` | Same for NO token |
| `no_last_trade_side` | Same for NO token |
| `yes_taker_buy_vol` | Cumulative taker-buy volume (USDC) for YES |
| `yes_taker_sell_vol` | Cumulative taker-sell volume (USDC) for YES |
| `no_taker_buy_vol` | Same for NO |
| `no_taker_sell_vol` | Same for NO |
| `trade_flow_imbalance` | `(all_taker_buy - all_taker_sell) / total` |

### Group C: Depth Deltas (5 columns, zero connections)

Computed from consecutive ticks.

| CSV Column | Formula |
|------------|---------|
| `yes_bid_depth_delta` | current - previous `yes_bid_depth_all` |
| `yes_ask_depth_delta` | current - previous `yes_ask_depth_all` |
| `no_bid_depth_delta` | current - previous `no_bid_depth_all` |
| `no_ask_depth_delta` | current - previous `no_ask_depth_all` |
| `book_pressure_shift` | current - previous `combined_obi` |

---

## IMPLEMENTATION ARCHITECTURE

### Current: 6 WebSocket Connections

| # | Stream | Data |
|---|--------|------|
| 1 | BestBidAsk (Poly YES+NO) | best_bid, best_ask |
| 2 | OrderBook (Poly YES+NO) | best_bid_size, best_ask_size **(depth discarded)** |
| 3 | NewMarkets (Poly events) | market detection |
| 4 | MarketResolutions (Poly events) | resolution detection |
| 5 | RTDS Binance | Binance price relay |
| 6 | RTDS Chainlink | Chainlink oracle relay |

### Proposed: 7 Connections (or 6 with refactor)

**Option A (Simple)**: Add connection #7 for `subscribe_last_trade_price`
**Option B (Optimized)**: Refactor connection #2 to share WsClient, consuming raw
`WsMessage` stream and routing `Book` + `LastTradePrice` in same handler.

### Data Flow

```
BOOK_STREAM (connection #2, exists)
  BookUpdate { bids, asks }
  +---> [EXISTING] best_bid_size, best_ask_size
  +---> [NEW] depth_3, depth_all, levels, imbalance --> TokenPrices

TRADE_STREAM (connection #7, new)
  LastTradePrice { price, side, size }
  +---> [NEW] last_trade, taker_buy_vol, taker_sell_vol --> TokenPrices

Logger Main Loop
  +---> Combined OBI, trade flow imbalance, depth deltas
  +---> CsvRecord { 53 existing + 31 new = 84 columns }
```

### Latency Impact

| Component | Added Latency |
|-----------|--------------|
| Group A depth extraction | <1 microsecond (in-memory Vec sum) |
| Group B trade stream | ~0ms on read path (async WS task) |
| Group C delta computation | <1 microsecond (f64 subtraction) |
| CSV write overhead | ~5 microseconds (~200 extra bytes) |
| **Total** | **<10 microseconds** (well within <250ms budget) |

---

## IMPLEMENTATION STEPS

### Step 1: Extract Full Depth from Existing BOOK_STREAM (Group A)

**File**: `src/api/polymarket.rs`
- Expand `TokenPrices` struct with 9 new depth fields
- In BOOK_STREAM handler (lines 855-884), compute depth sums from `bid_levels`/`ask_levels`
  BEFORE the vectors go out of scope
- **Risk**: Minimal. No new connections, pure struct expansion.

### Step 2: Add Depth Columns to CSV Logger (Group A)

**File**: `src/bin/multi_market_logger_hourly.rs`
- Add 15 Group A fields to `CsvRecord`
- Extend `csv_header()` and `to_csv_line()`
- Wire in main loop from `yes_prices`/`no_prices`
- **Risk**: Low. Follows exact pattern of previous column expansions.

### Step 3: Add LastTradePrice Stream (Group B)

**File**: `src/api/polymarket.rs`
- Add `taker_buy_volume`/`taker_sell_volume` to `TokenPrices`
- Spawn 3rd task in `stream()` for `subscribe_last_trade_price`
- Accumulate taker volumes by side, reset on market rotation
- **Risk**: Moderate. New WS connection but proven pattern.

### Step 4: Add Trade Flow + Delta Columns (Groups B + C)

**File**: `src/bin/multi_market_logger_hourly.rs`
- Add remaining 16 columns to CsvRecord
- Store previous-tick state for delta computation
- Reset on market rotation
- **Risk**: Low. Delta pattern already used in dashboard.

### Step 5 (Optional): Upgrade Dashboard

**File**: `src/bin/dashboard.rs`
- Replace shallow `calc_obi` with full-depth version
- Replace synthetic CVD with actual trade flow
- **Risk**: Low, independent concern.

---

## BACKTESTER INTEGRATION

New columns are immediately available to Python backtest scripts via `pandas.read_csv()`.
New strategy parameters enabled:

| Parameter | Gate/Filter | Data Source |
|-----------|-------------|-------------|
| `depth_obi_sell_trigger` | Sell when `combined_obi < -0.3` | Group A |
| `depth_thinning_gate` | Block entry when bid depth < threshold | Group A |
| `depth_delta_exit` | Sell when bid depth draining fast | Group C |
| `taker_flow_sell` | Sell when net taker selling heavy | Group B |
| `queue_depletion_exit` | Sell when bid size drops 50%+ at same price | Group A |

**Validation approach**: Log new columns for 1-2 days without trading changes. Run
correlation analysis between new metrics and market outcomes before integrating into strategy.

---

## CRITICAL FILES

| File | Changes |
|------|---------|
| `src/api/polymarket.rs` | Expand TokenPrices, extract full depth, add trade stream |
| `src/bin/multi_market_logger_hourly.rs` | Expand CsvRecord, wire new fields, compute deltas |
| `src/bin/dashboard.rs` | Replace shallow OBI + synthetic CVD (optional) |

## SDK REFERENCE

| Type | SDK Location |
|------|-------------|
| `BookUpdate` | `polymarket-client-sdk/src/clob/ws/types/response.rs:69-85` |
| `OrderBookLevel` | Same file, lines 90-95 |
| `LastTradePrice` | Same file, lines 157-173 |
| `subscribe_last_trade_price` | `polymarket-client-sdk/src/clob/ws/client.rs:182-196` |
