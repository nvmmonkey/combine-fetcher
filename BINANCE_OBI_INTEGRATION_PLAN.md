# Binance OBI & L2 Depth Integration Plan

> **Purpose**: Implementation prompt for Claude Opus 4.6 team to add Binance orderbook depth and OBI (Order Book Imbalance) to `combine-fetcher`, replicating the pattern used for Polymarket OBI but for the Binance BTC/USDT book.
>
> **Why**: Binance OBI is a **leading indicator** — book pressure shifts BEFORE price moves. Combined with existing RTDS (which detects price moves after they happen on Binance), Binance OBI lets us **predict** Polymarket price moves before they occur. Signal: `binance_obi < -0.20 AND polymarket_obi > -0.05` → Poly hasn't reacted yet → frontrun.

---

## 1. CONTEXT: What Already Exists

### Current Binance Integration (`src/api/binance.rs`, 1201 lines)
- **2 WebSocket connections** spawned in `connect()`:
  - `bookTicker` → `BinanceBookTicker` → updates `BinancePrice { bid, ask, mid, volume }` in `Arc<RwLock<HashMap<String, BinancePrice>>>`
  - `aggTrade` → `BinanceAggTradeEvent` → feeds `VolumeTracker` for rolling volume
- **Public API**: `get_price()`, `get_bid_ask()`, `get_market_snapshot()` (returns `MarketSnapshot` from `volatility.rs`)
- **`MarketSnapshot`** (`src/volatility.rs:24-56`): 16 fields (spot_price, vol_1sec-89sec, spread_bps, volume_1sec/1min, signal_quality, etc.)
- **Connection pattern**: Each loop has exponential backoff reconnect, reconnecting flag guard, ping/pong handling

### Current Logger (`src/bin/multi_market_logger_hourly.rs`, 1249 lines)
- **66 CSV columns** in `CsvRecord` struct (lines 34-133)
- **Column groups**: Original 23 + Latency 5 + Binance OB 3 + Volatility 6 + Volume 2 + Signal 3 + Spreads 2 + RTDS 10 + Polymarket Depth 15 + Trade Flow 7
- **Data flow**: `binance.get_market_snapshot("BTCUSDT")` → snapshot fields → `CsvRecord` → CSV line
- **Polymarket depth extraction** (lines 875-915): Reads `TokenPrices.bid_depth_3/ask_depth_3/bid_depth_all/ask_depth_all/bid_levels_count/ask_levels_count/book_imbalance` then computes `combined_obi`

### Polymarket OBI Pattern (the reference implementation)
Polymarket OBI was added in commits `2b49edf` and `49dd53f` using 4 layers:
1. **Struct fields** on `TokenPrices` (7 depth + 6 trade flow fields)
2. **WS handler** — depth extracted from existing `subscribe_orderbook` BOOK_STREAM data that was previously discarded
3. **Logger CsvRecord** — 23 new columns (YES/NO × depth fields + derived OBI + trade flow)
4. **Derived metrics** — `combined_obi` and `trade_flow_imbalance` computed cross-token in the logger

### Binance WebSocket Docs (from `/development/binance-ws-doc/`)
- **Partial Book Depth stream**: `<symbol>@depth<levels>@<speed>` (e.g., `btcusdt@depth5@100ms`)
- **Levels**: 5, 10, or 20
- **Speed**: 1000ms (default) or 100ms
- **Payload format** (single stream, no wrapper):
  ```json
  {
    "lastUpdateId": 160,
    "bids": [["price", "qty"], ...],
    "asks": [["price", "qty"], ...]
  }
  ```
- **Connection limits**: 300 connections/5min/IP, 1024 streams/connection, 5 incoming msgs/sec
- **Same connection CAN multiplex** ticker + depth, but requires combined stream URL format with message wrapping — NOT compatible with current single-stream handlers

---

## 2. ARCHITECTURE DECISION

### Use a 3rd Separate WebSocket Connection for Depth
**Rationale**: Follow existing pattern (bookTicker = conn 1, aggTrade = conn 2, depth = conn 3). This:
- Avoids changing existing WS handlers (no message-unwrapping refactor)
- Follows the proven reconnection/backoff pattern
- Only adds 1 connection (well within Binance's 300/5min limit)
- Keeps depth updates at 100ms without affecting bookTicker latency

### Stream to Subscribe
```
wss://stream.binance.com:9443/ws/btcusdt@depth5@100ms
```
- **depth5**: Top 5 bid + top 5 ask levels (sufficient for OBI — matches Polymarket's top-3 + all approach)
- **100ms**: Fastest available, needed for leading indicator edge

### Data Storage
Add a **separate** `BinanceDepthData` struct behind its own `Arc<RwLock<>>` (NOT mixed into `BinancePrice`). This:
- Avoids lock contention with the high-frequency bookTicker updates
- Follows the RTDS tracker pattern (`RtdsPriceData` behind `RwLock`)
- Is clock-safe (pure quantity math, no timestamp comparison)

---

## 3. IMPLEMENTATION PHASES

### Phase 1: `src/api/binance.rs` — Add Depth Stream (~250 lines)

#### 1A. New Structs (after `BinanceAggTradeEvent`, ~line 117)

```rust
/// Binance partial book depth snapshot from @depth5@100ms stream
#[derive(Debug, Deserialize)]
struct BinanceDepthEvent {
    /// Last update ID for this snapshot
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    /// Bid levels: [[price, qty], ...]
    bids: Vec<[String; 2]>,
    /// Ask levels: [[price, qty], ...]
    asks: Vec<[String; 2]>,
}

/// Processed Binance depth data (thread-safe snapshot)
#[derive(Debug, Clone, Default)]
pub struct BinanceDepthData {
    /// Sum of qty for top 3 bid levels
    pub bid_depth_3: f64,
    /// Sum of qty for top 3 ask levels
    pub ask_depth_3: f64,
    /// Sum of qty for ALL bid levels (up to 5)
    pub bid_depth_all: f64,
    /// Sum of qty for ALL ask levels (up to 5)
    pub ask_depth_all: f64,
    /// Number of bid price levels with qty > 0
    pub bid_levels: u16,
    /// Number of ask price levels with qty > 0
    pub ask_levels: u16,
    /// OBI: (bid_depth_all - ask_depth_all) / (bid_depth_all + ask_depth_all), range [-1, 1]
    pub book_imbalance: f64,
    /// Binance event receive timestamp (local clock, for staleness monitoring only)
    pub last_update_ms: u64,
}
```

#### 1B. New Fields on `BinanceDataManager` (in the struct definition, ~line 178)

```rust
/// Thread-safe depth data storage: symbol -> BinanceDepthData
depth_data: Arc<RwLock<HashMap<String, BinanceDepthData>>>,
/// Whether a reconnection is in progress (depth stream)
reconnecting_depth: Arc<RwLock<bool>>,
```

Initialize in `new()` and `with_url()`:
```rust
depth_data: Arc::new(RwLock::new(HashMap::new())),
reconnecting_depth: Arc::new(RwLock::new(false)),
```

#### 1C. New Public Accessor (after `get_market_snapshot()`, ~line 757)

```rust
/// Get the latest depth data (OBI + book levels) for a symbol
///
/// # Arguments
/// * `symbol` - Trading pair symbol (case-insensitive, e.g., "BTCUSDT")
///
/// # Returns
/// * `Some(BinanceDepthData)` - If depth data is available
/// * `None` - If symbol not found or no depth data received yet
pub async fn get_depth_data(&self, symbol: &str) -> Option<BinanceDepthData> {
    let depth = self.depth_data.read().await;
    depth.get(&symbol.to_uppercase()).cloned()
}
```

#### 1D. Depth Connection Spawned in `connect()` (after aggTrade spawn, ~line 537)

```rust
// Build depth streams URL (3rd connection)
let depth_url = {
    let streams: Vec<String> = self
        .symbols
        .iter()
        .map(|s| format!("{}@depth5@100ms", s.to_lowercase()))
        .collect();
    format!("{}/{}", self.ws_url, streams.join("/"))
};

info!("Connecting to Binance WebSocket (depth): {}", depth_url);

let depth_data_clone = self.depth_data.clone();
let reconnecting_depth_clone = self.reconnecting_depth.clone();

tokio::spawn(async move {
    Self::depth_connection_loop(
        depth_url,
        depth_data_clone,
        reconnecting_depth_clone,
    )
    .await
});
```

#### 1E. `depth_connection_loop()` (after `aggtrade_connection_loop()`, ~line 962)

Follow the EXACT same pattern as `aggtrade_connection_loop()`:
- Same exponential backoff (1s to 30s max)
- Same `reconnecting` flag guard with sleep+continue (FIX H2 pattern)
- Same ping/pong handling
- Same Close/Binary/Frame/Error match arms
- On `Message::Text` → call `handle_depth_message()`

#### 1F. `handle_depth_message()` (after `handle_aggtrade_message()`, ~line 1114)

```rust
/// Handle incoming partial book depth WebSocket message
async fn handle_depth_message(
    text: &str,
    depth_data: &Arc<RwLock<HashMap<String, BinanceDepthData>>>,
    symbol: &str,  // passed from connection context (single-symbol stream)
) -> Result<()> {
    let event: BinanceDepthEvent =
        serde_json::from_str(text).map_err(|e| anyhow!("Failed to parse depth: {}", e))?;

    let upper_symbol = symbol.to_uppercase();

    // Parse and sum bid depths
    let bid_quantities: Vec<f64> = event.bids.iter()
        .filter_map(|level| level[1].parse::<f64>().ok())
        .filter(|qty| *qty > 0.0)
        .collect();
    let ask_quantities: Vec<f64> = event.asks.iter()
        .filter_map(|level| level[1].parse::<f64>().ok())
        .filter(|qty| *qty > 0.0)
        .collect();

    let bid_depth_3: f64 = bid_quantities.iter().take(3).sum();
    let ask_depth_3: f64 = ask_quantities.iter().take(3).sum();
    let bid_depth_all: f64 = bid_quantities.iter().sum();
    let ask_depth_all: f64 = ask_quantities.iter().sum();

    let total = bid_depth_all + ask_depth_all;
    let book_imbalance = if total > 0.0 {
        (bid_depth_all - ask_depth_all) / total
    } else {
        0.0
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let data = BinanceDepthData {
        bid_depth_3,
        ask_depth_3,
        bid_depth_all,
        ask_depth_all,
        bid_levels: bid_quantities.len() as u16,
        ask_levels: ask_quantities.len() as u16,
        book_imbalance,
        last_update_ms: now_ms,
    };

    {
        let mut depth_guard = depth_data.write().await;
        depth_guard.insert(upper_symbol.clone(), data);
    }

    debug!(
        "Updated depth for {}: OBI={:.4}, bids={}, asks={}",
        upper_symbol, book_imbalance, bid_quantities.len(), ask_quantities.len()
    );

    Ok(())
}
```

**IMPORTANT NOTE on symbol routing**: The `@depth5@100ms` stream payload does NOT include a symbol field (unlike bookTicker/aggTrade which have `"s": "BTCUSDT"`). For single-symbol use (BTCUSDT only in `multi_market_logger_hourly.rs`), pass the symbol from the connection context. For multi-symbol (future `multi_asset_logger.rs`), either:
- Use separate connections per symbol (simplest), OR
- Use combined stream URL and extract symbol from the `"stream"` wrapper field

For this implementation, use single-symbol approach since the logger only tracks one Binance pair at a time.

#### 1G. Tests (append to existing `mod tests` block)

```rust
#[test]
fn test_binance_depth_data_default() {
    let data = BinanceDepthData::default();
    assert_eq!(data.bid_depth_3, 0.0);
    assert_eq!(data.ask_depth_3, 0.0);
    assert_eq!(data.book_imbalance, 0.0);
    assert_eq!(data.bid_levels, 0);
    assert_eq!(data.ask_levels, 0);
}

#[test]
fn test_binance_depth_event_parse() {
    let json = r#"{
        "lastUpdateId": 160,
        "bids": [["97000.00", "1.5"], ["96999.00", "0.8"], ["96998.00", "2.1"]],
        "asks": [["97001.00", "0.9"], ["97002.00", "1.2"], ["97003.00", "0.5"]]
    }"#;
    let event: BinanceDepthEvent = serde_json::from_str(json).unwrap();
    assert_eq!(event.bids.len(), 3);
    assert_eq!(event.asks.len(), 3);
    assert_eq!(event.last_update_id, 160);
}

#[tokio::test]
async fn test_get_depth_data_none_before_connect() {
    let manager = BinanceDataManager::new();
    assert!(manager.get_depth_data("BTCUSDT").await.is_none());
}
```

---

### Phase 2: `src/bin/multi_market_logger_hourly.rs` — Add CSV Columns (~80 lines)

#### 2A. Extend `CsvRecord` (after `trade_flow_imbalance` field, line 132)

Append 8 new fields at the END of the struct:

```rust
    // === BINANCE L2 DEPTH (from Binance depth5@100ms stream) ===
    pub binance_bid_depth_3: f64,
    pub binance_ask_depth_3: f64,
    pub binance_bid_depth_all: f64,
    pub binance_ask_depth_all: f64,
    pub binance_bid_levels: u16,
    pub binance_ask_levels: u16,
    pub binance_book_imbalance: f64,
    pub binance_poly_obi_divergence: f64,
```

#### 2B. Extend `csv_header()` (line 136-159)

Append to the end of the header string (after `trade_flow_imbalance`):

```
,binance_bid_depth_3,binance_ask_depth_3,\
binance_bid_depth_all,binance_ask_depth_all,\
binance_bid_levels,binance_ask_levels,\
binance_book_imbalance,binance_poly_obi_divergence
```

#### 2C. Extend `to_csv_line()` (at the end of the format string)

Append the 8 new fields matching the header order.

#### 2D. Extract Depth Data in Main Loop (after the trade flow section, ~line 915)

```rust
// --- Binance L2 Depth ---
let binance_depth = binance.get_depth_data("BTCUSDT").await;
let binance_bid_depth_3 = binance_depth.as_ref().map(|d| d.bid_depth_3).unwrap_or(0.0);
let binance_ask_depth_3 = binance_depth.as_ref().map(|d| d.ask_depth_3).unwrap_or(0.0);
let binance_bid_depth_all = binance_depth.as_ref().map(|d| d.bid_depth_all).unwrap_or(0.0);
let binance_ask_depth_all = binance_depth.as_ref().map(|d| d.ask_depth_all).unwrap_or(0.0);
let binance_bid_levels = binance_depth.as_ref().map(|d| d.bid_levels).unwrap_or(0);
let binance_ask_levels = binance_depth.as_ref().map(|d| d.ask_levels).unwrap_or(0);
let binance_book_imbalance = binance_depth.as_ref().map(|d| d.book_imbalance).unwrap_or(0.0);

// Binance vs Polymarket OBI divergence (leading indicator signal)
// Positive = Binance more bullish than Poly (Binance bids > asks, Poly hasn't reacted)
// Negative = Binance more bearish than Poly
let binance_poly_obi_divergence = binance_book_imbalance - combined_obi;
```

#### 2E. Populate CsvRecord (in the record construction, after `trade_flow_imbalance`)

```rust
    // Binance L2 Depth
    binance_bid_depth_3: sanitize(binance_bid_depth_3),
    binance_ask_depth_3: sanitize(binance_ask_depth_3),
    binance_bid_depth_all: sanitize(binance_bid_depth_all),
    binance_ask_depth_all: sanitize(binance_ask_depth_all),
    binance_bid_levels,
    binance_ask_levels,
    binance_book_imbalance: sanitize(binance_book_imbalance),
    binance_poly_obi_divergence: sanitize(binance_poly_obi_divergence),
```

---

## 4. COLUMN DEFINITIONS (for analysis scripts)

| Column | Type | Range | Description |
|--------|------|-------|-------------|
| `binance_bid_depth_3` | f64 | 0+ | Sum of BTC qty across top 3 bid levels |
| `binance_ask_depth_3` | f64 | 0+ | Sum of BTC qty across top 3 ask levels |
| `binance_bid_depth_all` | f64 | 0+ | Sum of BTC qty across all 5 bid levels |
| `binance_ask_depth_all` | f64 | 0+ | Sum of BTC qty across all 5 ask levels |
| `binance_bid_levels` | u16 | 0-5 | Count of active bid price levels |
| `binance_ask_levels` | u16 | 0-5 | Count of active ask price levels |
| `binance_book_imbalance` | f64 | -1.0 to +1.0 | Binance OBI: (bid_all - ask_all) / (bid_all + ask_all) |
| `binance_poly_obi_divergence` | f64 | -2.0 to +2.0 | binance_obi - polymarket_combined_obi (leading signal) |

**Trading Signal**: When `binance_book_imbalance < -0.20` AND `combined_obi > -0.05`:
- Binance sellers dominating (book pressure building)
- Polymarket hasn't reacted yet
- **Action**: Buy NO side cheaply before Polymarket adjusts

---

## 5. TESTING STRATEGY

### Unit Tests (Phase 1)
- `test_binance_depth_data_default` — Default values are zero
- `test_binance_depth_event_parse` — JSON deserialization of depth payload
- `test_get_depth_data_none_before_connect` — Returns None before any data

### Integration Test (manual)
1. `cargo build --bin multi_market_logger_hourly`
2. Run logger for 60 seconds
3. Verify CSV has 74 columns (66 existing + 8 new)
4. Verify `binance_book_imbalance` values are in [-1.0, 1.0] range
5. Verify depth values are non-zero after ~1 second of streaming
6. Verify `binance_poly_obi_divergence` = `binance_book_imbalance` - `combined_obi`

### Smoke Test
```bash
# Run logger briefly and check output
cargo run --bin multi_market_logger_hourly -- 30 /tmp/test_obi.csv 1

# Check column count
head -1 /tmp/test_obi.csv | tr ',' '\n' | wc -l
# Expected: 74

# Check OBI values exist
tail -5 /tmp/test_obi.csv | cut -d',' -f67-74
```

---

## 6. FILES TO MODIFY

| File | Changes | Lines Added |
|------|---------|-------------|
| `src/api/binance.rs` | Add `BinanceDepthEvent`, `BinanceDepthData` structs; add `depth_data`/`reconnecting_depth` fields; add `get_depth_data()` accessor; add `depth_connection_loop()` + `handle_depth_message()`; spawn depth task in `connect()`; add tests | ~250 |
| `src/bin/multi_market_logger_hourly.rs` | Add 8 fields to `CsvRecord`; extend `csv_header()`; extend `to_csv_line()`; add depth extraction + divergence calc in main loop | ~40 |

**NO changes needed to**: `volatility.rs`, `polymarket.rs`, `rtds_tracker.rs`, `lib.rs`, `Cargo.toml`

---

## 7. CRITICAL IMPLEMENTATION NOTES

1. **Do NOT modify existing WebSocket connections** — depth is a 3rd independent connection
2. **Do NOT add depth fields to `MarketSnapshot`** — use separate `get_depth_data()` accessor to avoid lock contention
3. **Depth payload has NO symbol field** — for single-symbol, pass symbol from connection context; for multi-symbol (future), use combined stream URL
4. **OBI is clock-safe** — pure quantity math, no timestamp comparison across clocks
5. **Column order matters** — new columns MUST be appended at the END of CsvRecord to maintain backward compatibility with existing CSV parsers
6. **The `depth5` stream returns exactly 5 levels** — no need for level counting beyond checking qty > 0
7. **100ms update rate** — depth updates 10x per second; this is fine since the logger samples at `CSV_RECORD_INTERVAL_SEC` (typically 1s)
8. **sanitize()** all f64 values before CSV output — prevents NaN/Inf from breaking pandas
9. **For future multi_asset_logger.rs**: `get_depth_data()` already takes symbol as parameter, so it naturally supports multi-token. The depth_connection_loop already handles multi-symbol URLs via `streams.join("/")`. The only thing multi_asset_logger would need is to call `get_depth_data("ETHUSDT")` etc.

---

## 8. EXECUTION ORDER

1. **First**: Implement all Phase 1 changes in `src/api/binance.rs`
2. **Run**: `cargo test` to verify unit tests pass
3. **Run**: `cargo build` to verify compilation
4. **Then**: Implement Phase 2 changes in `src/bin/multi_market_logger_hourly.rs`
5. **Run**: `cargo build --bin multi_market_logger_hourly`
6. **Test**: Run smoke test with 30-second logger run
7. **Verify**: CSV has 74 columns, OBI values are sane
