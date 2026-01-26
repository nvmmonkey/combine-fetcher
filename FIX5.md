# Fix Plan: time_to_open Field Semantics - Verification

## Problem Summary

**User Question:**

The CSV output shows negative `time_to_open` values (e.g., -788, -789, -790). User questioned whether this is correct.

**Current vs Expected Behavior:**

| Aspect | Current | Expected |
|--------|---------|----------|
| time_to_open when market is open | Negative (e.g., -788) | Negative (seconds since open) |
| time_to_open when market is pending | Positive | Positive (seconds until open) |
| time_to_open at market open | 0 | 0 |

**Conclusion:** The current behavior is **CORRECT**. No code changes are needed.

---

## Root Cause Analysis

### Semantics of time_to_open

**Definition:**
```
time_to_open = window_start - now_ts
```

**Value Meanings:**
- **Positive (> 0)**: Market opens in X seconds (pre-market phase)
- **Zero (= 0)**: Market is opening now
- **Negative (< 0)**: Market opened X seconds ago (active trading)

**Why This Is Correct:**

1. **Backtesting Compatibility:**
   - Matches mock data format in `/development/poly-mock-trading/data/option-mock-data.py`
   - The backtesting strategy uses `if record.time_to_open <= 0` to detect active markets
   - Negative values indicate "market is open for trading"

2. **Example from output.csv:**
   ```
   timestamp: 2026-01-17T14:43:08.287
   market_slug: btc-updown-15m-1768660200
   time_to_open: -788
   time_to_expiry_minutes: 1.87
   ```
   Interpretation:
   - Market window started at timestamp 1768660200
   - Current time is 788 seconds (~13 minutes) after window start
   - Market is ACTIVE (trading enabled)
   - Market closes in ~1.87 minutes

---

## Implementation Status

| Component | Status | Notes |
|-----------|--------|-------|
| time_to_open calculation | VERIFIED CORRECT | No changes needed |
| CSV output format | VERIFIED CORRECT | Matches mock data format |
| Backtesting compatibility | VERIFIED CORRECT | Compatible with main-maker-taker.rs |
| Code changes | NONE REQUIRED | Implementation is sound |

---

## Files Modified

| File | Changes |
|------|---------|
| _NONE_ | No code changes required - behavior is correct as-is |

---

## Testing Instructions

### Validation

To verify the time_to_open values are correct:

```bash
# Run the data logger
cd /home/lehash/development/combine-fetcher
cargo build --release
./target/release/data_logger 60 test_output.csv
```

### Expected CSV Output

For a market that is already open:
```
timestamp,market_slug,...,time_to_open,time_to_expiry_minutes,...
2026-01-17T14:43:08.287,btc-updown-15m-1768660200,...,-788,1.87,...
```

### Validation Checklist

1. **Check time_to_open is negative when market is active:**
   ```bash
   awk -F',' 'NR>1 {print $13}' test_output.csv | head -5
   # Should show negative values like: -788, -789, -790...
   ```

2. **Check time_to_expiry_minutes decreases over time:**
   ```bash
   awk -F',' 'NR>1 {print $14}' test_output.csv | head -5
   # Should show decreasing values: 1.87, 1.85, 1.83...
   ```

3. **Verify market_resolved is false when time_to_expiry_minutes > 0:**
   ```bash
   awk -F',' 'NR>1 {print $13 "," $14 "," $20}' test_output.csv | head -5
   # Should show: -788,1.87,false
   ```

---

## Semantics Reference Table

| time_to_open Value | Meaning | Trading State |
|--------------------|---------|---------------|
| > 0 | Market opens in X seconds | PRE-MARKET (no trading) |
| = 0 | Market is opening now | MARKET OPEN |
| < 0 | Market opened X seconds ago | ACTIVE TRADING |

---

## Code Reference

### Current Implementation (src/main.rs:417)

```rust
let time_to_open = market.window_start - now_ts;
```

This is correct and should NOT be changed.

### Backtesting Strategy Reference

The mock trading strategy correctly uses this field:

```rust
// From /development/poly-mock-trading/src/main-maker-taker.rs:1026
if record.time_to_open <= 0 && !record.market_resolved {
    strategy.execute_trade(&record);
}
```

Negative `time_to_open` triggers trading, which is the intended behavior.

---

## Known Limitations

1. **Human Readability:**
   - Negative values may be confusing for users expecting "time until open"
   - For human-readable reports, consider adding a derived field like `market_state` with values "PRE-MARKET", "OPEN", "ACTIVE"

2. **No Absolute Time in CSV:**
   - The CSV contains relative time but not the absolute window_start timestamp
   - Users must derive window_start from market_slug (e.g., btc-updown-15m-1768660200)

---

## Next Steps

**RECOMMENDED: Enhanced Documentation**

Consider adding a CSV header comment or README section explaining time_to_open semantics:

```csv
# time_to_open: Seconds relative to market window start
#   > 0 : Market opens in N seconds (pre-market)
#   = 0 : Market is opening now
#   < 0 : Market opened N seconds ago (active trading)
```

---

## Related Documentation

- [FIX4.md](./FIX4.md) - Previous fix: CSV Recording Frequency Control
- [FIX3.md](./FIX3.md) - Previous fix: Option Data Calculation
- [FIX2.md](./FIX2.md) - Previous fix: Data Streaming Issues
- [FIX1.md](./FIX1.md) - Previous fix: Import Resolution
- [README.md](./README.md) - Project overview
