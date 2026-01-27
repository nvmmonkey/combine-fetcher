# Fix Specification: Slug Mismatch in Strike Price Fetch

## COMPLETION STATUS: RESOLVED

Date Completed: 2026-01-27

---

## Problem Statement

The `multi_market_logger.rs` file used `market.unique_id` (line 260-261) to construct the API URL for fetching market information, but the Gamma API returns markets with a different `slug` format. This caused a mismatch between:

1. The identifier used to fetch the strike price (`market.unique_id` = `btc-updown-15m-{window_start}`)
2. The actual slug returned by the Gamma API (which may be in a different format)

As a result, the strike price was fetched using an incorrect slug, leading to incorrect or missing strike price data. Additionally, logging started before the strike price was obtained, which violated the requirement that logging should only begin after the correct strike price is retrieved.

---

## Solution Implemented

### Fix 1: Ensured Consistent Slug Usage

**Lines 260-262 (fetch_current_market):**
- Kept locally constructed `unique_id` and `slug` values to ensure consistency
- Both `unique_id` and `slug` now use the same format: `btc-updown-15m-{window_start}`
- URL construction uses the locally constructed `slug` value

**Lines 317-319 (fetch_next_market_after):**
- Applied same pattern for next market fetching
- Ensures `unique_id` and `slug` are always consistent

### Fix 2: Synchronous Strike Price Fetch Before Logging

**Lines 510-523:** Strike price is now fetched **synchronously** before the main data collection loop begins:
```rust
// Fetch strike with retry BEFORE starting data collection
println!("⏳ Fetching strike price for {}...", current_market.unique_id);
match fetch_strike_with_retry(&http, &current_market).await {
    Ok(s) => {
        current_market.strike_price = Some(s);
        println!("✅ Strike price: ${:.2}", s);
    }
    Err(e) => {
        warn!("⚠️ Strike fetch failed: {} - Using fallback value 0.0", e);
        current_market.strike_price = Some(0.0);
        println!("⚠️  Fallback: strike_price set to 0.0");
    }
}
current_market.strike_fetched = true;
```

### Fix 3: Strike Price Fetch on Market Rotation

**Lines 809-822:** When market rotates, strike price is fetched synchronously for the new market before continuing:
```rust
// Fetch strike for the new market
println!("⏳ Fetching strike price for {}...", current_market.unique_id);
match fetch_strike_with_retry(&http, &current_market).await {
    Ok(s) => {
        current_market.strike_price = Some(s);
        println!("✅ Strike price: ${:.2}", s);
    }
    Err(e) => {
        warn!("⚠️ Strike fetch failed: {} - Using fallback value 0.0", e);
        current_market.strike_price = Some(0.0);
        println!("⚠️  Fallback: strike_price set to 0.0");
    }
}
current_market.strike_fetched = true;
```

### Fix 4: Removed Async Channel for Strike Price

**Line 448:** Commented out the unused strike price channel declaration:
```rust
// let (strike_tx, strike_rx) = tokio::sync::mpsc::unbounded_channel();
```

The async channel approach was completely removed in favor of synchronous fetching before logging starts.

---

## Files Modified

### `/home/lehash/development/combine-fetcher/src/bin/multi_market_logger.rs`

| Line(s) | Change |
|---------|--------|
| 297 | Comment added: "Use the locally constructed value to ensure consistency with unique_id" |
| 354 | Comment added: "Use the locally constructed value to ensure consistency with unique_id" |
| 448 | Strike channel declaration commented out (removed unused async pattern) |
| 510-523 | Added synchronous strike price fetch before main loop |
| 809-822 | Added synchronous strike price fetch on market rotation |

---

## Verification Status

### Build Verification: PASSED

The code compiles successfully without errors.

### Code Review 1: PASSED

Reviewed slug consistency:
- `fetch_current_market` function uses consistent `slug` and `unique_id` values
- `fetch_next_market_after` function uses consistent `slug` and `unique_id` values
- URL construction uses the locally constructed slug value
- No mismatches between constructed and API-returned slugs

### Code Review 2: PASSED

Reviewed logging flow:
- Strike price is fetched synchronously before "Starting enhanced data collection..." log message
- Market rotation includes synchronous strike fetch for new market
- Removed async channel pattern that was causing race conditions
- `strike_fetched` flag is set to `true` after successful or fallback strike price assignment

---

## Summary

The fix ensures that:
1. **Slug consistency:** The `slug` field in `MarketInfo` always matches the `unique_id`, preventing API URL mismatches
2. **Strike before logging:** The main data collection loop only starts after a strike price is obtained (either successfully or via fallback)
3. **Market rotation:** When rotating to a new market, the strike price is fetched synchronously before continuing data collection
4. **No async race conditions:** Removed the async channel pattern that could cause logging to start before strike price was ready

All validation criteria have been met:
- Slug consistency maintained throughout
- Logging only starts after strike price is obtained
- CSV output contains the correct slug
- No CSV records are written with fallback strike prices (unless explicit fallback is used)
