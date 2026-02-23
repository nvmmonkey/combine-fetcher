# Order Book Depth Upgrade — Implementation Prompt

## Mission

Upgrade the Polymarket WebSocket integration in `src/api/polymarket.rs` to extract **full L2 order book depth** from data that is **already received but currently discarded**, then wire the new fields through to the CSV logger. WebSocket subscriptions only — no REST polling.

**Why**: Our av1 maker strategy buys at best-bid-only and achieves combined cost ~$0.97/pair. The top Polymarket trader achieves $0.87/pair by placing limit orders deeper in the book. To simulate multi-depth buying in our backtester, we first need to LOG the full depth data. This upgrade closes a $950/day gap at 10,000 pairs/day.

**Branch**: `orderbook-depth-upgrade` (based on `mutimarket`)

---

## Important: WebSocket Subscription History & Research Directive

### What Changed Between `main` and `mutimarket`

On the `main` branch, the PRICE_STREAM used `subscribe_prices()` which returned **`PriceChange`** events — order book delta updates containing per-level `price`, `size`, `side` changes plus `best_bid`/`best_ask`. This was a richer feed.

On the current `mutimarket` branch, this was switched to `subscribe_best_bid_ask()` which returns only **`BestBidAsk`** — top-of-book only (`best_bid`, `best_ask`, `spread`). This was done for simplicity and reliability.

**Do NOT change the current `subscribe_best_bid_ask` PRICE_STREAM.** Keep it as-is. For depth data, use the **existing BOOK_STREAM** (`subscribe_orderbook`) which already receives the full L2 book.

### WebSocket Subscription `level` Parameter

The Polymarket WebSocket subscription request schema includes a `level` parameter (integer) that may control order book depth. The full subscription schema is:

```json
{
  "type": "market",
  "assets_ids": ["TOKEN_ID"],
  "level": <integer>,                  // MAY control depth — research this
  "custom_feature_enabled": true       // enables best_bid_ask, new_market, market_resolved
}
```

**RESEARCH DIRECTIVE**: Before implementing, read the actual Polymarket developer docs at `/home/lehash/development/polymarket-dev-docs/polymarket_developer_docs.md` and the SDK docs at `/home/lehash/development/polymarket-dev-docs/polymarket_rust_sdk_docs.md` to:
1. Confirm the `level` parameter behavior — does it control book depth levels returned?
2. Verify the `subscribe_last_trade_price` method signature and response fields
3. Check if the SDK's `subscribe_orderbook` passes `level` through or if it needs to be set separately
4. Confirm whether each `subscribe_*` call creates a separate WS connection or multiplexes

### All WebSocket Event Types (from Polymarket docs)

| Event | Trigger | Key Fields |
|-------|---------|-----------|
| `book` | On subscribe + when trade affects book | `bids[]`, `asks[]`, `hash`, `timestamp` |
| `price_change` | New order placed or cancelled | `price_changes[]` with `price`, `size`, `side`, `best_bid`, `best_ask` |
| `last_trade_price` | Trade executed | `price`, `side` (required), `size` (required), `fee_rate_bps`, `transaction_hash` |
| `tick_size_change` | Market tick size updated | `old_tick_size`, `new_tick_size` |
| `best_bid_ask` | Top-of-book changes (requires `custom_feature_enabled`) | `best_bid`, `best_ask`, `spread` |
| `new_market` | Market created (requires `custom_feature_enabled`) | `question`, `assets_ids`, `outcomes` |
| `market_resolved` | Market resolved (requires `custom_feature_enabled`) | `winning_asset_id`, `winning_outcome` |

**Note**: The actual WebSocket docs show `side` and `size` as **required** in `last_trade_price` events, but the SDK struct marks them as `Option`. Handle both cases gracefully.

---

## Phase 1: Extract Depth from Existing BOOK_STREAM (polymarket.rs)

**Risk: ZERO. No new connections. No new subscriptions. Data already arrives and is parsed.**

### Current State

The `subscribe_orderbook` handler in `src/api/polymarket.rs` (BOOK_STREAM, spawned around line 776) receives `BookUpdate` messages containing the **full L2 order book**:

```rust
// SDK type (from polymarket-client-sdk 0.4.1):
pub struct BookUpdate {
    pub asset_id: U256,
    pub market: B256,
    pub timestamp: i64,
    pub bids: Vec<OrderBookLevel>,  // ALL bid levels, price descending
    pub asks: Vec<OrderBookLevel>,  // ALL ask levels, price ascending
    pub hash: Option<String>,
}

pub struct OrderBookLevel {
    pub price: Decimal,
    pub size: Decimal,
}
```

The handler builds `bid_levels: Vec<OrderBookLevel>` and `ask_levels: Vec<OrderBookLevel>` from `book.bids` and `book.asks`, but then ONLY extracts:

```rust
let bid_size = find_size_at_price(&bid_levels, target_bid_price);
let ask_size = find_size_at_price(&ask_levels, target_ask_price);
prices.best_bid_size = bid_size;
prices.best_ask_size = ask_size;
// bid_levels and ask_levels go out of scope — ALL depth data discarded
```

### What to Do

**1a. Expand `TokenPrices` struct (around line 69-109)** — add these depth fields:

```rust
// --- ORDER BOOK DEPTH (from existing BOOK_STREAM, zero new connections) ---
pub bid_depth_3: f64,       // Total bid size across top 3 price levels
pub ask_depth_3: f64,       // Total ask size across top 3 price levels
pub bid_depth_all: f64,     // Total bid size across ALL levels
pub ask_depth_all: f64,     // Total ask size across ALL levels
pub bid_levels_count: u16,  // Number of distinct bid price levels
pub ask_levels_count: u16,  // Number of distinct ask price levels
pub book_imbalance: f64,    // (bid_all - ask_all) / (bid_all + ask_all), range [-1, 1]
```

Initialize all to `0.0` / `0` in the constructor.

**1b. Compute depth metrics in BOOK_STREAM handler** — AFTER building `bid_levels` and `ask_levels` vectors, BEFORE they go out of scope, add:

```rust
// --- Depth extraction (Phase 1) ---
let bid_depth_3: f64 = bid_levels.iter().take(3)
    .filter_map(|l| l.size.to_f64())
    .sum();
let ask_depth_3: f64 = ask_levels.iter().take(3)
    .filter_map(|l| l.size.to_f64())
    .sum();
let bid_depth_all: f64 = bid_levels.iter()
    .filter_map(|l| l.size.to_f64())
    .sum();
let ask_depth_all: f64 = ask_levels.iter()
    .filter_map(|l| l.size.to_f64())
    .sum();

prices.bid_depth_3 = bid_depth_3;
prices.ask_depth_3 = ask_depth_3;
prices.bid_depth_all = bid_depth_all;
prices.ask_depth_all = ask_depth_all;
prices.bid_levels_count = bid_levels.len() as u16;
prices.ask_levels_count = ask_levels.len() as u16;

let total = bid_depth_all + ask_depth_all;
prices.book_imbalance = if total > 0.0 {
    (bid_depth_all - ask_depth_all) / total
} else {
    0.0
};
```

**1c. Expose via `get_prices()`** — No changes needed. The existing `get_prices()` method returns a clone of `TokenPrices`, so the new fields are automatically available to callers.

### How to Test Phase 1

After modifying polymarket.rs, write a small test binary (or modify an existing one) that:
1. Creates a `PolymarketStream` instance
2. Calls `stream()` with a known active BTC up/down market's YES and NO token IDs
3. Waits ~5 seconds for WebSocket data to arrive
4. Calls `get_prices()` for both tokens
5. Prints the depth fields: `bid_depth_3`, `ask_depth_3`, `bid_depth_all`, `ask_depth_all`, `bid_levels_count`, `ask_levels_count`, `book_imbalance`
6. Verifies: `bid_depth_3 > 0`, `bid_depth_all >= bid_depth_3`, `bid_levels_count > 0`

You can find active market token IDs from the Gamma API or from the logger's market discovery code. The existing `multi_market_logger_hourly.rs` already discovers these automatically.

**Expected output**: Typical BTC 15-min markets have 5-20 bid levels and 5-20 ask levels, with total depth of 500-5000 shares per side. `book_imbalance` should be in [-1, 1] range.

---

## Phase 2: Add LastTradePrice Stream (polymarket.rs)

**Risk: LOW. Uses `subscribe_last_trade_price` — same SDK pattern as existing streams. Reuses the existing WS connection via SDK multiplexing (single `ChannelType::Market` connection).**

### SDK Method

```rust
// From polymarket-client-sdk WS client:
pub fn subscribe_last_trade_price(
    &self,
    asset_ids: Vec<U256>
) -> Result<impl Stream<Item = Result<LastTradePrice>>>

pub struct LastTradePrice {
    pub asset_id: U256,
    pub market: B256,
    pub price: Decimal,
    pub side: Option<Side>,     // BUY or SELL (taker aggression direction)
    pub size: Option<Decimal>,  // Trade size in shares
    pub fee_rate_bps: Option<Decimal>,
    pub timestamp: i64,         // Unix ms
}
```

The WebSocket server sends `last_trade_price` events whenever a trade is executed on the given asset. The `side` field indicates taker aggression: `BUY` means a taker bought (price pressure up), `SELL` means a taker sold (price pressure down).

### What to Do

**2a. Add trade flow fields to `TokenPrices`:**

```rust
// --- TRADE FLOW (from subscribe_last_trade_price) ---
pub last_trade_price: f64,
pub last_trade_size: f64,
pub last_trade_side: String,        // "BUY", "SELL", or ""
pub taker_buy_volume: f64,          // Cumulative taker-buy volume (shares)
pub taker_sell_volume: f64,         // Cumulative taker-sell volume (shares)
pub trade_count: u64,               // Total trades observed
```

Initialize to `0.0` / `0` / `""` in constructor.

**2b. Spawn a TRADE_STREAM task** in the `stream()` method, following the exact same pattern as the existing PRICE_STREAM and BOOK_STREAM:

- Clone the same `asset_ids` vec
- Clone the `prices` DashMap Arc
- Clone the `generation` counter for supersession detection
- Create a new `ws::Client::default()`
- Call `ws_client.subscribe_last_trade_price(asset_ids_trade.clone())`
- Process `LastTradePrice` messages in a loop:
  - Look up token in `prices` DashMap
  - Update `last_trade_price`, `last_trade_size`, `last_trade_side`
  - Accumulate: if `side == Some(Side::Buy)` → `taker_buy_volume += size`, else `taker_sell_volume += size`
  - Increment `trade_count`
- Include the same generation check, reconnection logic, and error handling as the existing streams

**2c. Reset trade flow state on market rotation:**

In `remove_tokens()` (polymarket.rs line ~488), the token is already removed from the DashMap entirely, so cumulative volumes reset automatically when a new `TokenPrices` is created. No additional changes needed.

### How to Test Phase 2

Same test as Phase 1 but also verify trade flow:
1. After waiting ~10-15 seconds (trades are less frequent than book updates)
2. Print `last_trade_price`, `last_trade_size`, `last_trade_side`, `taker_buy_volume`, `taker_sell_volume`, `trade_count`
3. Verify: `trade_count > 0`, `last_trade_price > 0`, `taker_buy_volume + taker_sell_volume > 0`

**Important**: Trade events are less frequent than book updates. During low-volume periods, you may need to wait 30+ seconds to see a trade. This is normal.

---

## Phase 3: Wire Depth + Trade Flow to CSV Logger (multi_market_logger_hourly.rs)

**Risk: LOW. Follows the exact same pattern as the existing 45 columns.**

### What to Do

**3a. Add fields to `CsvRecord` struct (around line 33-107):**

Append these new fields at the END of the struct (backward compatibility — existing CSV parsers ignore extra columns):

```rust
// --- ORDER BOOK DEPTH ---
pub yes_bid_depth_3: f64,
pub yes_ask_depth_3: f64,
pub no_bid_depth_3: f64,
pub no_ask_depth_3: f64,
pub yes_bid_depth_all: f64,
pub yes_ask_depth_all: f64,
pub no_bid_depth_all: f64,
pub no_ask_depth_all: f64,
pub yes_bid_levels: u16,
pub yes_ask_levels: u16,
pub no_bid_levels: u16,
pub no_ask_levels: u16,
pub yes_book_imbalance: f64,
pub no_book_imbalance: f64,
pub combined_obi: f64,           // Cross-token: (all_bids - all_asks) / total

// --- TRADE FLOW ---
pub yes_last_trade_price: f64,
pub yes_last_trade_size: f64,
pub yes_last_trade_side: String,
pub no_last_trade_price: f64,
pub no_last_trade_size: f64,
pub no_last_trade_side: String,
pub trade_flow_imbalance: f64,   // (buy_vol - sell_vol) / total_vol
```

**3b. Extend `csv_header()` (around line 110-127):**

Append the new column names to the header string in the exact same order as the struct fields above.

**3c. Extend `to_csv_line()` (around line 129-204):**

Append the new fields with appropriate precision:
- Depth values: `{:.0}` (integer shares)
- Level counts: `{}` (integer)
- Imbalance values: `{:.4}` (4 decimal places)
- Trade prices: `{:.4}`
- Trade sizes: `{:.0}`
- Trade side: `{}` (string)

**3d. Wire in the main loop (around lines 673-873):**

After extracting `yes_prices` and `no_prices` via `poly_stream.get_prices()`, extract the new fields:

```rust
// --- Depth extraction ---
let yes_bid_depth_3 = yes_prices.as_ref().map(|p| p.bid_depth_3).unwrap_or(0.0);
let yes_ask_depth_3 = yes_prices.as_ref().map(|p| p.ask_depth_3).unwrap_or(0.0);
// ... same pattern for all depth fields ...

// --- Combined OBI (cross-token) ---
let all_bids = yes_bid_depth_all + no_bid_depth_all;
let all_asks = yes_ask_depth_all + no_ask_depth_all;
let combined_obi = if (all_bids + all_asks) > 0.0 {
    (all_bids - all_asks) / (all_bids + all_asks)
} else { 0.0 };

// --- Trade flow ---
let yes_last_trade_price = yes_prices.as_ref().map(|p| p.last_trade_price).unwrap_or(0.0);
// ... same pattern ...

// --- Trade flow imbalance (cross-token) ---
let total_buy = yes_taker_buy + no_taker_buy;
let total_sell = yes_taker_sell + no_taker_sell;
let trade_flow_imbalance = if (total_buy + total_sell) > 0.0 {
    (total_buy - total_sell) / (total_buy + total_sell)
} else { 0.0 };
```

### How to Test Phase 3

Run the hourly logger for one 15-minute market cycle:
```bash
cargo run --bin multi_market_logger_hourly -- --duration-minutes 20
```

Then verify the output CSV:
1. Check that the new columns appear in the header
2. Verify depth values are non-zero for most rows
3. Verify `yes_bid_depth_all >= yes_bid_depth_3` (all >= top-3)
4. Verify `combined_obi` is in [-1, 1] range
5. Verify trade flow columns populate (may be sparse — trades are event-driven, not every tick)

---

## Critical Implementation Notes

### 1. WebSocket Only — No REST Polling

All data must come from WebSocket subscriptions. The three subscription methods are:
- `subscribe_best_bid_ask(asset_ids)` → `BestBidAsk` (EXISTING — PRICE_STREAM)
- `subscribe_orderbook(asset_ids)` → `BookUpdate` (EXISTING — BOOK_STREAM)
- `subscribe_last_trade_price(asset_ids)` → `LastTradePrice` (NEW — TRADE_STREAM)

All three use the same WS endpoint: `wss://ws-subscriptions-clob.polymarket.com/ws/market`

The SDK multiplexes subscriptions over a single `ChannelType::Market` connection. Each `subscribe_*` call creates a new `ws::Client` internally, which connects to the same endpoint.

### 2. SDK Version and Imports

The project uses `polymarket-client-sdk = "0.4.1"` with features `["ws", "gamma", "rtds", "clob", "data"]`.

Key imports already used in `polymarket.rs`:
```rust
use polymarket_client_sdk::clob::ws::Client as WsClient;
use polymarket_client_sdk::clob::ws::{BookUpdate, BestBidAsk, OrderBookLevel};
```

For the new stream, add:
```rust
use polymarket_client_sdk::clob::ws::LastTradePrice;
// Side is already imported from: polymarket_client_sdk::clob::types::Side
```

### 3. DashMap Shared State

`TokenPrices` instances are stored in a `DashMap<String, TokenPrices>` shared across all stream tasks via `Arc`. Each stream task accesses the same DashMap:

```rust
let prices_clone = self.prices.clone(); // Arc<DashMap<String, TokenPrices>>
```

The TRADE_STREAM should follow the exact same pattern — get a mutable reference via `prices_clone.entry()` or `prices_clone.get_mut()`, update fields, and drop the lock.

### 4. Generation Counter (Supersession)

Each call to `stream()` increments a generation counter. Background tasks check this counter periodically and exit if superseded:

```rust
if current_gen.load(Ordering::Relaxed) != generation {
    info!("Stream generation changed, exiting old task");
    break;
}
```

The TRADE_STREAM must include this same check.

### 5. Reconnection Pattern

Both existing streams have reconnection logic: on disconnect, they wait with exponential backoff, then re-subscribe. The TRADE_STREAM should follow the same pattern. Look at the BOOK_STREAM handler (around lines 776-993) as the template.

### 6. Token ID Format

The SDK's `subscribe_*` methods accept `Vec<U256>` (not `Vec<String>`). The existing code converts string token IDs to U256 before subscribing. Follow the same conversion pattern.

### 7. Backward Compatibility

New CSV columns MUST be appended at the END. Never reorder or remove existing columns. Existing Python backtest scripts use `pandas.read_csv()` and access columns by name, so new columns won't break them.

---

## File Reference

| File | What to Change |
|------|---------------|
| `src/api/polymarket.rs` | Expand `TokenPrices` struct (~line 69), extract depth in BOOK_STREAM handler (~line 841-915), add TRADE_STREAM task in `stream()` |
| `src/bin/multi_market_logger_hourly.rs` | Expand `CsvRecord` (~line 33), extend `csv_header()` (~line 110), extend `to_csv_line()` (~line 129), wire new fields in main loop (~line 673-873) |

### Do NOT Modify

- `src/bin/dashboard.rs` — Dashboard upgrade is a separate task
- Any Python backtest files
- The SDK itself (`polymarket-client-sdk`)
- Other loggers or binaries

---

## Polymarket Developer Docs Reference

Local copies available at:
- `/home/lehash/development/polymarket-dev-docs/polymarket_developer_docs.md` — Full API docs including WebSocket section, orderbook format, fee model, event types
- `/home/lehash/development/polymarket-dev-docs/polymarket_rust_sdk_docs.md` — Full Rust SDK docs including all struct definitions, subscription methods, builder patterns

### Key SDK Types (from polymarket_rust_sdk_docs.md)

**BookUpdate** (already used):
```rust
pub struct BookUpdate {
    pub asset_id: U256,
    pub market: B256,
    pub timestamp: i64,
    pub bids: Vec<OrderBookLevel>,  // Price descending
    pub asks: Vec<OrderBookLevel>,  // Price ascending
    pub hash: Option<String>,
}
```

**OrderBookLevel** (already used):
```rust
pub struct OrderBookLevel {
    pub price: Decimal,
    pub size: Decimal,
}
```

**LastTradePrice** (NEW — subscribe via `subscribe_last_trade_price`):
```rust
pub struct LastTradePrice {
    pub asset_id: U256,
    pub market: B256,
    pub price: Decimal,
    pub side: Option<Side>,          // Some(Side::Buy) or Some(Side::Sell)
    pub size: Option<Decimal>,       // Trade size in shares
    pub fee_rate_bps: Option<Decimal>,
    pub timestamp: i64,              // Unix ms
}
// NOTE: SDK marks side/size as Option, but Polymarket WS docs show them as required.
// Handle None gracefully (default side="" and size=0.0) but expect them to always be present.
```

**BestBidAsk** (already used):
```rust
pub struct BestBidAsk {
    pub asset_id: U256,
    pub market: B256,
    pub best_bid: Decimal,
    pub best_ask: Decimal,
    pub spread: Decimal,
    pub timestamp: i64,
}
```

### WebSocket Subscription Methods (SDK)

All connect to `wss://ws-subscriptions-clob.polymarket.com/ws/market`:

| SDK Method | WS Event | Response Type | Used In |
|-----------|----------|--------------|---------|
| `subscribe_best_bid_ask()` | `best_bid_ask` | `BestBidAsk` | PRICE_STREAM (existing, do NOT change) |
| `subscribe_orderbook()` | `book` | `BookUpdate` | BOOK_STREAM (existing, extract depth here) |
| `subscribe_last_trade_price()` | `last_trade_price` | `LastTradePrice` | TRADE_STREAM (NEW) |
| `subscribe_prices()` | `price_change` | `PriceChange` | Was used on `main`, replaced by `subscribe_best_bid_ask` on `mutimarket` |

---

## Strategy Context (Why This Matters)

### Current av1 Strategy Results (5-day backtest, 0% maker fee)

| Day | Profit | WR | Combined Avg |
|-----|--------|----|-------------|
| Feb 18 | +$268 | 70.7% | $0.9684 |
| Feb 19 | -$34 | 63.0% | $0.9699 |
| Feb 20 | +$210 | 68.7% | $0.9693 |
| Feb 21 | +$71 | 64.0% | $0.9623 |
| Feb 22 | -$75 | 61.1% | $0.9638 |
| **5-DAY** | **+$440** | 65.3% | $0.9667 |

### The Gap

| Metric | Our Sim | Trader Actual | Gap |
|--------|---------|--------------|-----|
| Combined cost | $0.9667 | $0.8721 | **$0.095/pair** |
| Buy source | Best bid only | Multi-depth limits | Order book depth |
| Buy range | $0.49 ± $0.05 | $0.02 to $0.94 | Full spectrum |

### What Depth Data Enables

Once we log depth data for 1-2 days, the backtester can simulate:
1. **Multi-depth buying** — Place limit orders at multiple book levels (not just best bid)
2. **Fill probability at depth** — Estimate how likely fills are at each price level
3. **OBI-based sell signals** — Use book imbalance to time sells (replace pure z-score)
4. **Queue position estimation** — Model where our orders sit in the book

Full analysis: `/home/lehash/development/poly-mock-trading/src/data/av1_analysis.md`
Integration plans: `/home/lehash/development/combine-fetcher/trader_study/orderbook_integration_plan.md`
