# INTEGRATION ANALYSIS: Bootstrap Function Integration

## CURRENT_INIT_FLOW

### Entry Point: `multi_market_logger.rs` (main function)

1. **Tracing Setup** (lines 349-355)
   - Configures logging with env filters

2. **HTTP Client Creation** (lines 381-385)
   ```rust
   let http = reqwest::Client::builder()
       .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
       .connect_timeout(Duration::from_secs(3))
       .pool_idle_timeout(Duration::from_secs(90))
       .build()?;
   ```

3. **Market Fetching** (lines 387-395)
   - Calls `fetch_current_market(&http)` to get initial BTC market
   - Market includes: unique_id, slug, condition_id, yes_token_id, no_token_id, window_start/end

4. **Binance Initialization** (lines 397-400)
   ```rust
   let mut binance_manager = BinanceDataManager::new();
   binance_manager.connect(&["BTCUSDT"], None).await?;
   ```
   - Spawns two background tasks: `bookticker_connection_loop` and `aggtrade_connection_loop`
   - Initializes volatility calculator and volume tracker per symbol

5. **Polymarket Stream Creation** (lines 402-403)
   ```rust
   let poly_stream = Arc::new(PolymarketOrderbookStream::new());
   ```

6. **Market Subscription** (lines 406-427)
   - Calls `subscribe_to_market()` helper which invokes `poly_stream.stream(token_ids).await`
   - Spawns 4 background tasks: price_stream, book_stream, health_monitor, rest_fallback
   - Message receiver spawned in background task (messages consumed but not used)

7. **Connection Warmup** (line 432)
   - Sleeps 3 seconds for connections to establish

8. **CSV File Setup** (lines 434-445)

9. **Main Event Loop** (lines 456-638)
   - Polls Binance prices via `binance_manager.get_price()` and `get_market_snapshot()`
   - Retrieves Polymarket prices via `poly_stream.get_prices()`
   - Handles market rotation: calls `fetch_next_market_after()` and re-subscribes

---

## BOOTSTRAP_INJECTION_POINT

### Recommended: One-Time Startup Call (BEFORE Market Operations)

**Location:** After Binance connection, before Polymarket subscription
**File:** `/home/lehash/development/combine-fetcher/src/bin/multi_market_logger.rs`
**Insert between lines 400-402**

```rust
// Setup Binance (shared across all markets)
let mut binance_manager = BinanceDataManager::new();
binance_manager.connect(&["BTCUSDT"], None).await?;
println!("Binance connected");

// *** BOOTSTRAP INJECTION POINT ***
// Initialize historical volatility calculator with warm-start data
// This must complete before market operations begin
match bootstrap_binance_volatility(&http, "BTCUSDT").await {
    Ok(warmed_calcs) => {
        println!("Bootstrap complete: warmed {} volatility calculators", warmed_calcs.len());
        // Merge warmed data into binance_manager's internal calculators
    }
    Err(e) => {
        warn!("Bootstrap failed, using cold-start: {}", e);
        // Continue with empty calculators (default behavior)
    }
}
// ********************************

// Setup Polymarket stream (will re-subscribe when market changes)
let poly_stream = Arc::new(PolymarketOrderbookStream::new());
```

### Alternative: Per-Market Bootstrap (NOT RECOMMENDED)

**Reasons against:**
- Volatility data is symbol-specific, not market-specific
- BTC volatility is the same across all 15-minute windows
- Would cause redundant API calls for identical data
- Introduces latency during market rotation

### Critical Dependencies on Bootstrap Completion

1. **BinanceDataManager internal state**
   - `vol_calculators: HashMap<String, Arc<Mutex<FibonacciVolatilityCalculator>>>`
   - Bootstrap must populate these BEFORE the main loop queries them
   - The `FibonacciVolatilityCalculator` needs historical price points for accurate realized_vol

2. **Timing requirement**
   - Must complete before first `get_market_snapshot()` call (line 469)
   - Otherwise `realized_vol` defaults to `VOLATILITY_BTC` constant

3. **Error handling**
   - Bootstrap failure should be NON-FATAL
   - Fall back to cold-start (empty calculators) with warning log

---

## DEPENDENCIES

### On Binance API (`binance.rs`)

1. **`BinanceDataManager` struct** (lines 93-108)
   - Field: `vol_calculators: HashMap<String, Arc<Mutex<FibonacciVolatilityCalculator>>>`
   - This is the target for bootstrap data injection

2. **`FibonacciVolatilityCalculator` (from `volatility.rs`)**
   - Needs method to accept historical price points
   - Requires: timestamp (i64) and mid_price (f64) pairs
   - Current interface (implied from usage at line 599):
     ```rust
     vol_calc.add_price(current_time, mid_price);
     ```

3. **`BinanceDataManager::new()` and `connect()`**
   - `connect()` already creates empty `FibonacciVolatilityCalculator` instances (lines 162-172)
   - Bootstrap will need to either:
     a) Call a new method on `BinanceDataManager` to inject data into existing calculators
     b) Access the calculators directly via a public getter/merger

### On Polymarket API (`polymarket.rs`)

**No direct dependency.** The bootstrap function:
- Uses `reqwest::Client` for HTTP requests (already created in main)
- Fetches from `https://api.binance.com` (independent endpoint)
- Does not touch Polymarket WebSocket infrastructure

### On Volatility Module (`volatility.rs`)

**Requires public access to:**
- `FibonacciVolatilityCalculator::add_price()` or bulk insert method
- Potential new method: `hydrate_historical(Vec<PricePoint>)`

---

## RISK_ASSESSMENT

### High Risk Areas

1. **Race Condition: Background Task Startup**
   - `BinanceDataManager::connect()` spawns background tasks IMMEDIATELY (lines 212-241)
   - These tasks call `vol_calc.add_price()` as WebSocket messages arrive
   - If bootstrap runs AFTER spawn, may have data collision:
     - Background task: inserts price at t=1000
     - Bootstrap: inserts historical prices at t=0, t=900...
     - Order of insertion affects volatility calculations

   **MITIGATION:** Bootstrap BEFORE `connect()` call, or add a synchronization primitive

2. **Missing Data During Warmup**
   - Current code has 3-second sleep (line 432)
   - WebSocket messages may arrive slowly
   - Early `get_market_snapshot()` calls may return incomplete volatility
   - Bootstrap eliminates this by pre-loading data

3. **Market Rotation During Bootstrap**
   - If bootstrap is called PER MARKET, the rotation window (line 607-636)
     could trigger while bootstrap is in-flight
   - This creates a race: new market fetch vs bootstrap completion

   **MITIGATION:** Call bootstrap ONCE at startup (see BOOTSTRAP_INJECTION_POINT)

### Medium Risk Areas

4. **API Rate Limits**
   - Bootstrap will make historical klines requests to Binance REST API
   - If multiple markets bootstrapped simultaneously, could hit rate limits
   - Current design (one-time call) mitigates this

5. **Memory Allocation**
   - Historical klines for 89-second window (fibonacci sequence) ~200 data points
   - Negligible memory impact

### Low Risk Areas

6. **Network Latency**
   - Bootstrap runs synchronously at startup
   - Could delay application start by 100-500ms
   - Acceptable tradeoff for accurate initial volatility

7. **Data Freshness**
   - Historical klines may be 1-2 seconds behind real-time
   - First WebSocket update will correct this
   - Transition should be seamless

---

## INTEGRATION RECOMMENDATIONS

### 1. Add Method to `BinanceDataManager`

```rust
// In binance.rs, impl BinanceDataManager
pub async fn warm_start_volatility(&mut self, symbol: &str, historical_data: Vec<(i64, f64)>) -> Result<()> {
    let upper_symbol = symbol.to_uppercase();
    if let Some(vol_calculator) = self.vol_calculators.get(&upper_symbol) {
        let mut vol_calc = vol_calculator.lock().await;
        for (timestamp, price) in historical_data {
            vol_calc.add_price(timestamp, price);
        }
        Ok(())
    } else {
        Err(anyhow!("No volatility calculator for symbol {}", symbol))
    }
}
```

### 2. Bootstrap Function Signature

```rust
/// Fetch historical klines from Binance and warm-start volatility calculators
///
/// # Arguments
/// * `client` - HTTP client for REST API calls
/// * `symbol` - Trading pair (e.g., "BTCUSDT")
///
/// # Returns
/// Map of symbol -> Vec<(timestamp_ms, mid_price)> for injection into BinanceDataManager
pub async fn bootstrap_binance_volatility(
    client: &reqwest::Client,
    symbol: &str,
) -> Result<HashMap<String, Vec<(i64, f64)>>> {
    // Implementation fetches klines from:
    // GET https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=1s&limit=100
}
```

### 3. Integration Flow

```rust
// In main(), BEFORE connect():
let bootstrap_data = bootstrap_binance_volatility(&http, "BTCUSDT").await?;

let mut binance_manager = BinanceDataManager::new();
binance_manager.connect(&["BTCUSDT"], None).await?;

// Inject bootstrap data AFTER connect creates the calculators
for (symbol, data) in bootstrap_data {
    binance_manager.warm_start_volatility(&symbol, data).await?;
}
```

### 4. Error Handling Strategy

- Log warning on bootstrap failure
- Continue with cold-start (empty calculators)
- Application remains functional (realized_vol defaults to `VOLATILITY_BTC`)
- Consider retry mechanism for production

---

## SUMMARY TABLE

| Aspect | Finding |
|--------|---------|
| **Injection Point** | After `BinanceDataManager::new()`, before `connect()` |
| **Call Frequency** | ONCE at startup (not per-market) |
| **Binance Dependency** | Requires new method: `warm_start_volatility()` |
| **Polymarket Dependency** | None |
| **Race Condition Risk** | MEDIUM: if called after `connect()`, background tasks may collide |
| **Recommended Mitigation** | Call before `connect()`, add internal locking to `add_price()` |
| **Failure Mode** | Non-fatal: fall back to cold-start with warning |
| **Performance Impact** | +100-500ms startup latency for 89-second historical data |
