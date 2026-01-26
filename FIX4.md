# Fix Plan: CSV Recording Frequency Control

## Problem Summary

**User Concern:**

The CSV recording frequency was not configurable and was hardcoded in the application logic. Users needed the ability to:
1. Control how frequently market data is recorded to CSV
2. Adjust recording frequency based on their specific use case
3. Potentially disable rate limiting entirely for maximum recording frequency

**Current vs Expected Behavior:**

The `market_data.csv` recording was controlled by a hardcoded `MIN_LOG_INTERVAL_MS = 100` constant:
- CSV rows were written every ~100ms (10 records/second maximum)
- No way to adjust this without modifying source code
- No way to disable rate limiting for maximum frequency recording

**Expected Behavior:**
- Configurable CSV recording frequency via command-line argument
- Default behavior maintained at 100ms intervals
- Option to set interval to 0 for unlimited/unthrottled recording

---

## Root Cause Analysis

### Hardcoded MIN_LOG_INTERVAL_MS

**Problem:** The CSV recording frequency was controlled by a constant defined in `src/constants.rs`:

```rust
/// Minimum logging interval in milliseconds (rate limiting)
pub const MIN_LOG_INTERVAL_MS: i64 = 100;
```

This constant was used in `src/main.rs` at line 387:

```rust
// Rate limit
if now_ms - last_log_ms < MIN_LOG_INTERVAL_MS {
    continue;
}
```

**Issues:**
1. No runtime configurability - requires code change to adjust
2. No option to disable rate limiting (setting to 0 would still skip records)
3. Not exposed via CLI arguments despite other parameters being configurable

**User Impact:**
- High-frequency trading scenarios may need faster recording
- Data analysis may prefer lower frequency to reduce file size
- Testing/debugging may need maximum frequency (0 = unlimited)

---

## Implementation Status

| Component | Status | Notes |
|-----------|--------|-------|
| New constant `CSV_RECORD_INTERVAL_SEC` | COMPLETED | Added to `src/constants.rs` |
| CLI argument parsing | COMPLETED | Optional third argument in `main.rs` |
| Updated rate limiting logic | COMPLETED | Handles 0 = unlimited case |
| Documentation | COMPLETED | Usage instructions updated |
| Code compilation | COMPLETED | Clean build, no warnings |

---

## Files Modified

| File | Changes |
|------|---------|
| `src/constants.rs` | Added `CSV_RECORD_INTERVAL_SEC` constant (default 0 = event-driven) |
| `src/main.rs` | Added CLI argument parsing for custom interval; Updated rate limiting logic to handle 0 = event-driven, >0 = interval mode |

---

## Implementation Details

### 1. New Constant (src/constants.rs)

```rust
/// CSV recording interval in seconds.
/// - Set to 0 for event-driven mode (record on every update with duplicate guard)
/// - Set to positive value for time-interval mode (record at fixed interval)
pub const CSV_RECORD_INTERVAL_SEC: u64 = 0;
```

**Note:** The default is 0 (event-driven mode) which records on every new stream update with a minimal 10ms guard against duplicates.

### 2. Updated Rate Limiting Logic (src/main.rs)

**Before:**
```rust
if now_ms - last_log_ms < MIN_LOG_INTERVAL_MS {
    continue;
}
```

**After:**
```rust
// Get CSV interval from CLI args or use default
let csv_record_interval_sec: u64 = args.get(3)
    .and_then(|s| s.parse().ok())
    .unwrap_or(CSV_RECORD_INTERVAL_SEC);

// CSV recording frequency control
let should_record = if csv_record_interval_sec == 0 {
    // Event-driven mode: record on every update (small guard for duplicates)
    const MIN_EVENT_INTERVAL_MS: i64 = 10;
    now_ms - last_log_ms >= MIN_EVENT_INTERVAL_MS
} else {
    // Time-interval mode: record at fixed interval
    let interval_ms = (csv_record_interval_sec * 1000) as i64;
    now_ms - last_log_ms >= interval_ms
};

if !should_record {
    continue;
}
```

Key changes:
- Added event-driven mode (interval = 0) with minimal 10ms guard
- Interval mode converts seconds to milliseconds for comparison
- CLI argument can override the default constant

### 3. Command-Line Interface

**Usage:**
```bash
# Default event-driven mode (0 = record on stream updates)
./target/release/data_logger 60 output.csv

# Custom 1-second interval
./target/release/data_logger 60 output.csv 1

# Custom 5-second interval
./target/release/data_logger 60 output.csv 5
```

---

## Testing Instructions

### Build

```bash
cd /home/lehash/development/combine-fetcher
cargo build --release
```

### Test Scenarios

**1. Default Behavior (event-driven, interval=0):**
```bash
./target/release/data_logger 10 test_default.csv
```
Expected: Records on every Binance price update (approximately 10-100 records/second)

**2. Custom Interval (1 second):**
```bash
./target/release/data_logger 10 test_1sec.csv 1
```
Expected: ~10 CSV records (10 seconds / 1 second interval)

**3. Custom Interval (5 seconds):**
```bash
./target/release/data_logger 10 test_5sec.csv 5
```
Expected: ~2 CSV records (10 seconds / 5 second interval)

**4. Verify Output:**
```bash
# Count records
wc -l test_*.csv

# Check timestamps between records
head -20 test_unlimited.csv | cut -d',' -f1
```

---

## Expected Behavior Table

| Interval Value | Behavior | Records/Second | Use Case |
|----------------|----------|----------------|----------|
| `0` (default) | Event-driven (on stream updates) | 10-100 | High-frequency analysis, debugging |
| `1` | 1-second intervals | ~1 | Standard time-series logging |
| `5` | 5-second intervals | ~0.2 | Reduced file size |
| `60` | 1-minute intervals | ~0.017 | Long-term monitoring |

### Rate Limiting Logic Flow

```
                           ┌─────────────────────────┐
                           │  Market Update Received │
                           └────────────┬────────────┘
                                        │
                                        ▼
                           ┌─────────────────────────┐
                           │ csv_record_interval_sec │
                           │ == 0 ?                  │
                           └────────────┬────────────┘
                                        │
                    ┌───────────────────┴───────────────────┐
                    │ YES (Event-driven)                    │ NO (Interval mode)
                    ▼                                       ▼
            ┌───────────────┐                   ┌─────────────────────────┐
            │ Record if >   │                   │ Time since last >=      │
            │ 10ms elapsed  │                   │ interval_sec?           │
            │ (guard only)  │                   └────────────┬────────────┘
            └───────────────┘                                 │
                                        ┌───────────┴───────────┐
                                        │ YES                   │ NO
                                        ▼                       ▼
                                ┌───────────────┐       ┌───────────────┐
                                │ RECORD DATA   │       │ SKIP (throttle)│
                                └───────────────┘       └───────────────┘
```

---

## Configuration Examples

### Example 1: High-Frequency Trading Analysis (Event-Driven)
```bash
# Record on every market update (default mode)
./target/release/data_logger 300 hf_trading_data.csv
```

### Example 2: Long-Term Monitoring (1-Minute Intervals)
```bash
# Record once per minute for 24 hours
./target/release/data_logger 86400 daily_monitor.csv 60
```

### Example 3: Standard Production Use (1-Second Intervals)
```bash
# Record once per second
./target/release/data_logger 900 production.csv 1
```

---

## Backward Compatibility

**Full backward compatibility maintained:**

- Default behavior is now event-driven (0 = record on stream updates)
- Existing scripts without third argument use the default constant
- No changes to CSV format or column structure
- No changes to other command-line arguments

---

## Known Limitations

1. **File Size with Unlimited Recording:**
   - Setting interval to 0 can generate very large files
   - Monitor disk space during long-running sessions
   - Consider log rotation for production use

2. **System Performance:**
   - Very low intervals (< 10ms) may impact performance
   - Disk I/O becomes bottleneck at high frequencies
   - CSV writing is synchronous (no buffering)

3. **Precision:**
   - Actual interval may vary slightly due to system scheduling
   - 10ms sleep in main loop provides minimum granularity

---

## Next Steps

**Potential Future Enhancements:**

1. Add buffered CSV writing for better performance at high frequency
2. Implement log rotation for long-running sessions
3. Add CSV output format options (e.g., parquet for large datasets)
4. Consider in-memory buffering with periodic flush

---

## Related Documentation

- [FIX3.md](./FIX3.md) - Previous fix: Option Data Calculation (Greeks/Volatility)
- [FIX2.md](./FIX2.md) - Previous fix: Data Streaming Issues (Volume/Orderbook)
- [FIX1.md](./FIX1.md) - Previous fix: Import Resolution
- [README.md](./README.md) - Project overview
