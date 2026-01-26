# Historical Bootstrap Implementation for Volatility

## API_ENDPOINT

### Primary: Binance K-lines REST API

**Endpoint:** `GET /api/v3/klines`

**Base URL:** `https://api.binance.com`

**Parameters:**
```
symbol: BTCUSDT (or configured symbol)
interval: 1m (1-minute candles)
limit: 1000 (maximum allowed per request)
```

**Rationale:**
- Binance K-lines provide OHLCV data ideal for volatility calculation
- 1-minute interval matches the calculator's 1-minute buffer (`price_buffer_1min`)
- 1000 candles = 16.67 hours of historical data (sufficient for warm-up)
- Free, no API key required for public endpoints
- High reliability (99.9% uptime SLA)

**Alternative Endpoints (Fallback):**
1. `GET /api/v3/aggTrades` - Individual trades (higher resolution, slower)
2. `GET /api/v1/klines` - Legacy endpoint (deprecated, avoid)

**Request Flow:**
```rust
GET https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=1m&limit=1000
```

**Response Format:**
```json
[
  [1499040000000, "0.01634790", "0.80000000", "0.01575800", "0.01577100", "148976.11427815", 1499644799999, "2434.85503045", 308, "1756410.54748708", "17928899.62484339"],
  // ... 999 more candles
  // Index 0: Open time (ms)
  // Index 1: Open price
  // Index 2: High price
  // Index 3: Low price
  // Index 4: Close price  <-- USE THIS for volatility
  // Index 5: Volume
  // Index 6: Close time (ms)
]
```

### Rate Limits
- 1200 request weight per minute
- This endpoint = 1 weight
- 1000 candles in 1 request is optimal

---

## DATA_FLOW

### Phase 1: Bootstrap (Initialization)

```
                     +---------------------+
                     |  Application Start  |
                     +---------------------+
                               |
                               v
                     +---------------------+
                     | Create Calculator   |
                     | (empty buffers)     |
                     +---------------------+
                               |
                               v
                     +---------------------+
                     | Fetch Historical    |
                     | K-lines (1000)      |
                     +---------------------+
                               |
                +--------------+--------------+
                |                             |
                v                             v
      +-------------------+         +------------------+
      | API Success       |         | API Failure      |
      | (candles array)   |         | (network/parse)  |
      +-------------------+         +------------------+
                |                             |
                v                             v
      +-------------------+         +------------------+
      | Validate Data     |         | Fallback to      |
      | - len >= 60       |         | Default Vol      |
      | - prices > 0      |         | (symbol-based)   |
      +-------------------+         +------------------+
                |
    +-----------+-----------+
    |                       |
    v                       v
+---------+           +-------------+
| Populate|           | Populate    |
| unified |           | 1min buffer |
| buffer  |           | (close only)|
+---------+           +-------------+
    |                       |
    +-----------+-----------+
                |
                v
      +-------------------+
      | Start WebSocket   |
      | (real-time feed)  |
      +-------------------+
```

### Phase 2: Runtime (After Bootstrap)

```
WebSocket Feed -> price_buffer_unified (millisecond precision)
                  |
                  |-- calculate_vol_from_window() uses unified buffer
                  |-- Fibonacci blend uses all timeframes
                  |-- Historical 1min data provides baseline stability
```

### Buffer Pre-population Strategy

**Two-Tier Buffer Approach:**

1. **`price_buffer_unified`** (millisecond ticks)
   - Populated with: Close price from each 1-minute candle
   - Timestamp: Candle close time
   - Purpose: Immediate volatility availability on startup
   - Quantity: Last 90 seconds of candles (most recent)

2. **`price_buffer_1min`** (f64 prices only)
   - Populated with: All 1000 close prices
   - Purpose: Long-term volatility baseline
   - Retained: 60 minutes max (sliding window)

**Welford Initialization:**
- Calculator does NOT store running mean/variance
- Historical data populates buffers only
- First `calculate_all()` call runs Welford on populated buffers
- Result: Immediate valid volatility on startup

---

## ERROR_HANDLING

### Error Categories & Responses

| Error Type | Detection | Response | Fallback |
|------------|-----------|----------|----------|
| Network Failure | `reqwest::Error` | Retry once (1s delay) | Default volatility |
| Parse Error | JSON decode failure | Abort bootstrap | Default volatility |
| Empty Response | `candles.is_empty()` | Abort bootstrap | Default volatility |
| Invalid Data | Any price <= 0 | Abort bootstrap | Default volatility |
| Insufficient Data | `len() < 60` | Use partial data | Log warning |
| Rate Limit | HTTP 429 | Wait 60s, retry | Default volatility |

### Retry Strategy

```
1. Initial API call (timeout: 5s)
   |
   |-- Success --> Continue to validation
   |
   |-- Failure (network/timeout)
       |
       v
   2. Wait 1 second (exponential backoff: 1s)
   |
   v
   3. Retry once
   |
   |-- Success --> Continue to validation
   |
   |-- Failure --> FALLBACK to default

Total max wait: 1 second
```

### State Transitions

```
            +---------+
            |  START  |
            +---------+
                |
                v
         [API CALL #1]
                |
        +-------+-------+
        |               |
    SUCCESS          FAILURE
        |               |
        v               v
   [VALIDATE]    [WAIT 1s]
        |               |
        v               v
   [POPULATE]    [API CALL #2]
        |               |
        |         +-----+-----+
        |         |           |
        |     SUCCESS     FAILURE
        |         |           |
        |         v           v
        |    [VALIDATE]   [FALLBACK]
        |         |
        +---->----+
                |
                v
           [READY STATE]
```

### Logging Strategy

- **ERROR level**: Bootstrap failure, using fallback
- **WARN level**: Partial data (< 1000 candles), using available
- **INFO level**: Bootstrap success, candles count
- **DEBUG level**: API request details, individual candle processing

---

## FALLBACK_RECOMMENDATION

### Critique: 0 vs 0.5 Default Volatility

| Aspect | vol=0 | vol=0.5 |
|--------|-------|---------|
| **BS Formula Impact** | d1 = (ln(S/K) + (0+sigma^2/2)T) / (sigma*sqrt(T)) | d1 = (ln(S/K) + (0+0.25/2)T) / (0.5*sqrt(T)) |
| **ITM Probability** | Always 50% (no drift assumption) | Varies with moneyness |
| **Vega Exposure** | Zero (vol insensitive) | Normal sensitivity |
| **Trading Behavior** | May buy underpriced options | Market-based pricing |
| **Risk Profile** | Undefined risk | Moderate risk assumption |

### Analysis: vol=0 is SAFER

**Reasoning:**

1. **Conservative Pricing**: vol=0 produces the minimum theoretical option value. If the system prices with vol=0 and then discovers true volatility > 0, option prices will increase. This is preferable to:

2. **Overpricing Risk**: vol=0.5 may OVERPRICE options if true volatility is lower. Trading on overpriced options leads to losses.

3. **Bootstrap Recovery**: When real-time data flows in, volatility will converge from 0 to true value. This is a natural "warm-up" that the Fibonacci blending handles gracefully.

4. **Regime Detection**: The calculator's `get_volatility_regime()` uses `vol_1sec / vol_89sec`. With vol=0, ratio is undefined (0/0 = handled by max(0.01) denominator). The system correctly enters "warming up" state.

### Final Recommendation: **Use vol=0 as safer fallback**

**However**, the current implementation uses symbol-based defaults (BTC: 0.55, ETH: 0.65, SOL: 0.75). These are REASONABLE estimates based on historical crypto volatility.

**Hybrid Strategy (Recommended):**

```
1. Primary fallback: Symbol-based default (current behavior)
   - BTC: 0.55 (55% annual vol is historically accurate)
   - ETH: 0.65
   - SOL: 0.75
   - Unknown: 0.65

2. Bootstrap failure handling:
   - Log ERROR with details
   - Continue with symbol-based default
   - Re-attempt bootstrap every 5 minutes (background retry)

3. Gradual convergence:
   - As real-time data accumulates, calculated vol replaces default
   - Fibonacci weighting handles smooth transition
```

**Do NOT change defaults to 0.** The symbol-based defaults are empirically grounded and provide safer pricing than 0 while the system warms up.

---

## IMPLEMENTATION_STEPS

### Step 1: Add Bootstrap Method to FibonacciVolatilityCalculator

**Location:** `/home/lehash/development/combine-fetcher/src/volatility.rs`

```rust
impl FibonacciVolatilityCalculator {
    /// Bootstrap with historical K-line data from Binance
    ///
    /// # Arguments
    /// * `symbol` - Trading pair (e.g., "BTCUSDT")
    ///
    /// # Returns
    /// * `Ok(())` - Bootstrap successful
    /// * `Err(e)` - Bootstrap failed, will use default volatility
    pub async fn bootstrap_historical(&mut self, symbol: &str) -> Result<()> {
        const API_URL: &str = "https://api.binance.com/api/v3/klines";
        const LIMIT: u32 = 1000;

        let url = format!(
            "{}?symbol={}&interval=1m&limit={}",
            API_URL, symbol, LIMIT
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;

        let response = client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("API returned status: {}", response.status()));
        }

        let candles: Vec<Vec<serde_json::Value>> = response.json().await?;

        if candles.is_empty() {
            return Err(anyhow::anyhow!("Empty response from Binance"));
        }

        // Populate buffers with close prices
        for candle in &candles {
            if candle.len() < 7 {
                continue;
            }

            let close_time = candle[6].as_i64().unwrap_or(0);
            let close_price = candle[4].as_str().and_then(|s| s.parse::<f64>().ok());

            if let Some(price) = close_price {
                if price > 0.0 {
                    self.add_price(close_time, price);
                    self.add_price_1min(price);
                }
            }
        }

        info!(
            "Bootstrapped {} calculator with {} candles from {}",
            symbol,
            candles.len(),
            candles.first().and_then(|c| c[6].as_i64()).unwrap_or(0)
        );

        Ok(())
    }
}
```

### Step 2: Add Retry Wrapper with Fallback

```rust
/// Bootstrap with retry and safe fallback
pub async fn bootstrap_with_fallback(
    calculator: &mut FibonacciVolatilityCalculator,
    symbol: &str,
) -> BootstrapResult {
    match calculator.bootstrap_historical(symbol).await {
        Ok(()) => BootstrapResult::Success,
        Err(e) => {
            warn!("Bootstrap attempt 1 failed for {}: {}", symbol, e);

            // Retry once after 1 second
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            match calculator.bootstrap_historical(symbol).await {
                Ok(()) => BootstrapResult::SuccessAfterRetry,
                Err(e2) => {
                    error!("Bootstrap failed for {} after 2 attempts: {}. Using default volatility: {}",
                        symbol, e2, calculator.default_volatility);
                    BootstrapResult::UsingDefault
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum BootstrapResult {
    Success,
    SuccessAfterRetry,
    UsingDefault,
}
```

### Step 3: Update BinanceDataManager.connect()

**Location:** `/home/lehash/development/combine-fetcher/src/api/binance.rs`

After creating volatility calculators (line 166), bootstrap them:

```rust
// In BinanceDataManager::connect(), after line 172:

// Bootstrap historical data for each calculator
for (symbol, vol_calculator) in self.vol_calculators.iter_mut() {
    let mut calc = vol_calculator.lock().await;
    match bootstrap_with_fallback(&mut calc, symbol).await {
        BootstrapResult::Success => {
            info!("Bootstrap successful for {}", symbol);
        }
        BootstrapResult::SuccessAfterRetry => {
            warn!("Bootstrap succeeded after retry for {}", symbol);
        }
        BootstrapResult::UsingDefault => {
            warn!("Bootstrap failed for {}, using default volatility", symbol);
        }
    }
}
```

### Step 4: Add Dependencies (if not present)

**Check `/home/lehash/development/combine-fetcher/Cargo.toml`:**

```toml
[dependencies]
reqwest = { version = "0.12", features = ["json"] }
```

### Step 5: Testing Strategy

```rust
#[cfg(test)]
mod bootstrap_tests {
    use super::*;

    #[tokio::test]
    async fn test_bootstrap_historical() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        let result = calc.bootstrap_historical("BTCUSDT").await;

        assert!(result.is_ok());
        assert!(calc.is_ready());

        let (vol_1sec, _, _, _, _, vol_blended) = calc.calculate_all(chrono::Utc::now().timestamp_millis());
        assert!(vol_blended > 0.0);
    }

    #[tokio::test]
    async fn test_bootstrap_invalid_symbol() {
        let mut calc = FibonacciVolatilityCalculator::new("INVALID");
        let result = calc.bootstrap_historical("INVALID").await;

        assert!(result.is_err());
        // Calculator should still function with defaults
        let (_, _, _, _, _, vol_blended) = calc.calculate_all(0);
        assert_eq!(vol_blended, 0.65); // Default for unknown symbols
    }

    #[tokio::test]
    async fn test_bootstrap_fallback_flow() {
        let mut calc = FibonacciVolatilityCalculator::new("BTCUSDT");
        let result = bootstrap_with_fallback(&mut calc, "BTCUSDT").await;

        // Should always return a valid result (success or fallback)
        match result {
            BootstrapResult::UsingDefault => {
                assert_eq!(calc.default_volatility, 0.55);
            }
            _ => {
                assert!(calc.is_ready());
            }
        }
    }
}
```

### Step 6: Integration Checklist

- [ ] Add `reqwest` dependency if missing
- [ ] Implement `bootstrap_historical()` in `volatility.rs`
- [ ] Implement `bootstrap_with_fallback()` wrapper
- [ ] Update `BinanceDataManager::connect()` to call bootstrap
- [ ] Add unit tests for bootstrap
- [ ] Add integration test with mock Binance API
- [ ] Update logging to include bootstrap status
- [ ] Document bootstrap behavior in module docs

---

## SUMMARY

| Component | Specification |
|-----------|---------------|
| **API** | Binance `/api/v3/klines` |
| **Interval** | 1 minute |
| **Quantity** | 1000 candles (16.7 hours) |
| **Timeout** | 5 seconds |
| **Retry** | 1 attempt after 1 second |
| **Fallback** | Symbol-based default (BTC: 0.55, ETH: 0.65, SOL: 0.75) |
| **Buffers Populated** | Both `price_buffer_unified` and `price_buffer_1min` |
| **Safe Startup** | Yes - always has valid volatility data |
