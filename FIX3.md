# FIX3: Verify Real-time Volatility Transition After Bootstrap

## Problem Statement

While FIX2 addresses the deduplication threshold issue for real-time price acceptance, we need to verify that the bootstrap-to-realtime transition is working correctly. Specifically, we need to confirm that:

1. Bootstrap data from REST API is being properly loaded
2. Real-time WebSocket data is arriving and being accepted into the unified buffer
3. The system correctly transitions from "BOOTSTRAP" to "REALTIME" volatility regime
4. Once in REALTIME mode, the bootstrap values are fully phased out

### Observed Context

The `volatility_regime` field in `custom_data.csv` shows "REALTIME", but we need to verify:
- Is the unified buffer actually growing with real-time data?
- Is the staleness detection working correctly?
- Are the bootstrap values being replaced or is there a hybrid state?

## Investigation Scope

### Primary Files to Examine

| File | Purpose | Key Areas |
|------|---------|-----------|
| `/home/lehash/development/combine-fetcher/src/volatility.rs` | Volatility calculation engine | Buffer management, regime detection, staleness checks |
| `/home/lehash/development/combine-fetcher/src/api/binance.rs` | Binance WebSocket & REST API | Bootstrap loading, real-time price injection |

### Key Data Structures

**Unified Buffer (`price_buffer_unified`):**
- Holds real-time price ticks from WebSocket
- Deduplicated by `min_price_change_bps` threshold
- 90-second rolling window (cleaned in `add_price()`)

**1-Minute Buffer (`price_buffer_1min`):**
- Holds historical candle close prices from REST API
- Used as fallback when real-time data is sparse or stale
- Updated via `bootstrap_volatility()` in binance.rs

## Diagnostic Enhancement Specification

### Objective

Add targeted logging to track buffer state transitions and verify that real-time data is replacing bootstrap data over time.

### Logging Requirements

#### 1. Bootstrap Completion Logging

**File:** `/home/lehash/development/combine-fetcher/src/api/binance.rs`
**Location:** End of `bootstrap_volatility()` method (after line 386)

**Add:**
```rust
// Log bootstrap completion with buffer state summary
info!(
    "[BOOTSTRAP_COMPLETE] {} | 1min_buffer={} | unified_buffer={} | default_vol={:.4} | ready={}",
    upper_symbol,
    calc.get_1min_buffer_size(),
    calc.get_unified_buffer_size(),
    calc.default_volatility,
    calc.is_ready()
);
```

#### 2. Real-time Price Acceptance Logging

**File:** `/home/lehash/development/combine-fetcher/src/volatility.rs`
**Location:** In `add_price()` method, after successful price insertion (after line 238)

**Add:**
```rust
// Log successful price acceptance with buffer state
tracing::debug!(
    "[RT_PRICE_ACCEPTED] {} | price={:.2} | buffer_size={} | last_accepted={:.2} | change_bps={:.4}",
    self.symbol,
    price,
    self.price_buffer_unified.len(),
    self.last_accepted_price,
    price_change_bps
);
```

#### 3. Deduplication Filter Logging

**File:** `/home/lehash/development/combine-fetcher/src/volatility.rs`
**Location:** In `add_price()` method, when price is rejected as duplicate (inside the `if` block at line 230-232)

**Add:**
```rust
// Log rejected duplicate prices (trace level to avoid spam)
tracing::trace!(
    "[RT_PRICE_FILTERED] {} | price={:.2} | change_bps={:.4} < threshold {:.4}",
    self.symbol,
    price,
    price_change_bps,
    self.min_price_change_bps
);
```

#### 4. Volatility Regime Transition Logging

**File:** `/home/lehash/development/combine-fetcher/src/volatility.rs`
**Location:** In `calculate_all()` method, at the decision point between real-time and fallback (around line 507-567)

**Add:**
```rust
// Log regime decision
if use_realtime {
    tracing::info!(
        "[REGIME_REALTIME] {} | buffer_size={} | fast_vol={:.4} | 1min_fallback={:.4}",
        self.symbol,
        self.price_buffer_unified.len(),
        self.cached_vol_fast,
        self.cached_vol_1min
    );
} else {
    tracing::warn!(
        "[REGIME_BOOTSTRAP] {} | buffer_size={} | is_stale={} | fast_vol={:?} | using_1min_fallback",
        self.symbol,
        self.price_buffer_unified.len(),
        is_stale,
        fast_vol_raw
    );
}
```

#### 5. Buffer State Summary Logging

**File:** `/home/lehash/development/combine-fetcher/src/volatility.rs`
**Location:** New method or enhanced `get_diagnostics()`

**Add enhanced diagnostics:**
```rust
pub fn get_buffer_diagnostics(&self, current_time: i64) -> String {
    let age_ms = if let Some(latest) = self.price_buffer_unified.back() {
        current_time - latest.timestamp
    } else {
        i64::MAX
    };

    format!(
        "{}[rt:{}/1m:{}/age_ms:{}/stale:{}/ready:{}/regime:{}]",
        self.symbol,
        self.price_buffer_unified.len(),
        self.price_buffer_1min.len(),
        age_ms,
        if self.is_realtime_stale(current_time) { "Y" } else { "N" },
        if self.is_ready() { "Y" } else { "N" },
        self.get_volatility_regime()
    )
}
```

### Verification Steps

After implementing the diagnostic logging:

1. **Start the application** and observe bootstrap completion:
   - Look for `[BOOTSTRAP_COMPLETE]` log entry
   - Verify `1min_buffer` has 10+ candles
   - Verify `unified_buffer` has 5 initial synthetic ticks

2. **Monitor real-time price acceptance:**
   - Look for `[RT_PRICE_ACCEPTED]` messages (should appear frequently)
   - If not appearing, check `[RT_PRICE_FILTERED]` trace logs
   - High filter rate indicates deduplication is too aggressive

3. **Observe regime transitions:**
   - Initially should see `[REGIME_BOOTSTRAP]` warnings
   - After 3-5 seconds of real-time data, should see `[REGIME_REALTIME]`
   - Buffer size should be growing steadily

4. **Check buffer diagnostics:**
   - Call `get_buffer_diagnostics()` periodically from main loop
   - Monitor `age_ms` - should be < 3000ms for non-stale data
   - `regime` should transition from "BOOTSTRAP" to "NORMAL"/"HIGH"/"LOW"/"EXTREME"

### Expected Behavior Timeline

| Time | Event | Expected Log | Expected Buffer State |
|------|-------|--------------|----------------------|
| T+0s | Bootstrap completes | `[BOOTSTRAP_COMPLETE]` | 1min: 70, unified: 5 |
| T+1s | First WebSocket prices | `[RT_PRICE_ACCEPTED]` | unified: 5-10 |
| T+3s | Staleness check passes | `[REGIME_REALTIME]` | unified: 10+, age_ms < 3000 |
| T+10s | Steady state | Continuous `[RT_PRICE_ACCEPTED]` | unified: 20-100 (depends on volatility) |

### Failure Indicators

| Symptom | Log Pattern | Likely Cause |
|---------|-------------|--------------|
| Stuck in BOOTSTRAP | Continuous `[REGIME_BOOTSTRAP]` | No real-time data arriving |
| High filter rate | Many `[RT_PRICE_FILTERED]` | Deduplication threshold too high (see FIX2) |
| Stale data | `stale=Y`, `age_ms > 3000` | WebSocket connection issues |
| Frozen buffer | `buffer_size` constant | No new prices accepted |

## Related Documentation

| Document | Topic |
|----------|-------|
| `/home/lehash/development/combine-fetcher/FIX1.md` | Strike price implementation (verified working) |
| `/home/lehash/development/combine-fetcher/FIX2.md` | Deduplication threshold fix |

## Implementation Notes

- All logging should use appropriate levels: `info!` for state changes, `debug!` for price acceptance, `trace!` for filtered duplicates
- Log format uses `[PREFIX]` for easy filtering: `grep '\[REGIME_'` or `grep '\[RT_PRICE'`
- No code behavior changes - this is purely a diagnostic enhancement
- After verification, logs can be reduced in frequency or removed if desired
