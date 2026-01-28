# FIX2: Volatility Frozen/Duplicated Issue

## COMPLETION STATUS: RESOLVED

Date Completed: 2026-01-27

---

## Problem Statement

The volatility values in `custom_data.csv` are frozen and highly duplicated. All volatility windows (`vol_1sec` through `vol_89sec`) show identical values (e.g., 0.177532), and the values do not change over time despite `vol_data_source` showing "REALTIME".

### Observed Symptoms
1. All volatility columns (vol_1sec, vol_5sec, vol_13sec, vol_34sec, vol_89sec) contain identical values
2. The volatility_regime shows "REALTIME" but values remain static
3. The bootstrap volatility value appears to persist instead of being replaced by real-time calculations

## Root Cause Analysis

The deduplication threshold in `/home/lehash/development/combine-fetcher/src/volatility.rs` at line 176 is too restrictive:

```rust
min_price_change_bps: 0.1, // 0.1 bps = 0.001% (very sensitive for arb)
```

This 0.1 basis point threshold (0.001%) filters out small price changes. In a stable or low-volatility market:

1. Small price movements below 0.001% are rejected as "duplicates"
2. Few or no new distinct prices enter the `price_buffer_unified`
3. All volatility calculations fall back to the bootstrap/default value
4. The system reports "REALTIME" but uses stale/bootstrap values

The deduplication logic at lines 228-233:
```rust
let price_change_bps = ((price - self.last_accepted_price).abs() / self.last_accepted_price) * 10_000.0;
if self.last_accepted_price > 0.0 {
    if price_change_bps < self.min_price_change_bps {
        return; // Duplicate price, skip
    }
}
```

### Note on FIX1 (Strike Price)

After verification, FIX1's strike price implementation is **working correctly**. The `calculate_greeks()` function at `multi_market_logger.rs:645-650` properly uses the `strike` parameter derived from `current_market.strike_price`. The strike price is not causing this volatility issue.

## Solution Specification

### Primary Fix: Reduce Deduplication Threshold

**File:** `/home/lehash/development/combine-fetcher/src/volatility.rs`

**Location:** Line 176

**Change:**
```rust
// Before:
min_price_change_bps: 0.1, // 0.1 bps = 0.001% (very sensitive for arb)

// After:
min_price_change_bps: 0.01, // 0.01 bps = 0.0001% (allow more updates)
```

**Rationale:**
- Reducing from 0.1 bps to 0.01 bps (10x more sensitive) allows smaller price movements to be captured
- This enables the unified buffer to receive more frequent price updates
- More price data points allow proper volatility calculation across all time windows
- Still provides some deduplication to avoid excessive processing of identical prices

### Implementation Steps

1. Open `/home/lehash/development/combine-fetcher/src/volatility.rs`
2. Locate line 176 in the `VolatilityEngine::new()` function
3. Change `min_price_change_bps: 0.1` to `min_price_change_bps: 0.01`
4. Save the file
5. Rebuild the project: `cargo build --release`
6. Restart the application

### Verification Steps

After applying the fix, verify the following:

1. **Check CSV output volatility columns:**
   - `vol_1sec`, `vol_5sec`, `vol_13sec`, `vol_34sec`, `vol_89sec` should now show **different values**
   - Values should change over time as new price data arrives

2. **Confirm volatility_regime transitions:**
   - After sufficient real-time data accumulation (3+ samples per volatility.rs:170), volatility_regime should change from "BOOTSTRAP" to "REALTIME"
   - Once in REALTIME regime, values should continue to update

3. **Monitor unified buffer size:**
   - The unified buffer should accumulate more price ticks
   - Buffer size can be checked via `get_unified_buffer_size()`

4. **Compare against Binance price movements:**
   - Volatility changes should correlate with actual BTC price movements from Binance websocket feeds

## Files Modified

| File | Line | Change |
|------|------|--------|
| `/home/lehash/development/combine-fetcher/src/volatility.rs` | 176 | `min_price_change_bps: 0.1` -> `min_price_change_bps: 0.01` |

## Related Files (Reference Only)

| File | Purpose |
|------|---------|
| `/home/lehash/development/combine-fetcher/src/bin/multi_market_logger.rs` | CSV logging, uses volatility values |
| `/home/lehash/development/combine-fetcher/FIX1.md` | Strike price fix (verified working) |
