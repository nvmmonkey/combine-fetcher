# Health Monitoring System - Detailed Analysis

## Quick Reference

**File**: `src/api/polymarket.rs`
**Binary Entry**: `src/bin/multi_market_logger_hourly.rs`
**Main Culprit**: Lines 917-985 (Health Monitor Task)

---

## 1. The [HEALTH] Warning Message

### Location
**File**: `src/api/polymarket.rs:961-974`

```rust
warn!(
    "[HEALTH] 🔍 ACTIVE_WS:{} QUIET_WS:{} STALE:{} DISC:{} UNHEALTHY:{} | {}",
    healthy_ws, quiet_ws, stale_ws, disconnected, unhealthy,
    stream_clone.connection_status()
);
```

### When It Fires
- **Line 959**: `if tick_count % 3 == 0 || has_issues`
- Logs every 30 seconds normally, BUT
- Logs **EVERY 10 SECONDS** when `has_issues == true` (line 957)

---

## 2. Root Cause: The `has_issues` Trigger

### Code
**File**: `src/api/polymarket.rs:957`

```rust
let has_issues = disconnected > 0 || unhealthy > 0 || stale_ws > 0;
```

### The Problem
Any token in STALE, DISCONNECTED, or UNHEALTHY state triggers warnings **every 10 seconds**.

---

## 3. Health Status Classification

### Token State Machine
**File**: `src/api/polymarket.rs:129-202` (TokenPrices methods)

```
┌─────────────────────────────────────────┐
│ connection_detail() - Lines 167-202     │
└─────────────────────────────────────────┘
        ↓
    Is ws_connected? (line 176)
        ├─ NO → "DISCONNECTED" ❌ TRIGGERS WARNING
        │
        └─ YES → Check consecutive_errors (line 181)
           ├─ errors ≥ 3 → "UNHEALTHY" ❌ TRIGGERS WARNING
           │
           └─ errors < 3 → Check ws_age (lines 186-201)
              ├─ < 10s → "HEALTHY_WS" ✅ NO WARNING
              ├─ 10-90s → "QUIET_WS" ✅ NO WARNING
              └─ > 90s → "STALE_WS" ❌ TRIGGERS WARNING
```

### The Key Thresholds

| Threshold | Variable | Value | Reason |
|-----------|----------|-------|--------|
| Staleness check | `DEFAULT_STALENESS_MS` (line 33) | 5000ms | For `is_stale()` method |
| WS age for API fallback | `WS_CONNECTED_AGE_MS` (line 36) | 5000ms | If no WS data for 5s, use REST |
| Silent disconnect threshold | `SILENT_DISCONNECT_THRESHOLD_MS` (line 54) | 90000ms | >90s without updates = very stale |
| Health check interval | `HEALTH_CHECK_INTERVAL_SECS` (line 57) | 10 seconds | How often to run health check |
| Stream error threshold | `STREAM_ERROR_THRESHOLD` (line 60) | 3 errors | Mark unhealthy after 3 consecutive errors |

---

## 4. Health Monitor Task Details

### Location
**File**: `src/api/polymarket.rs:917-985`

### Flow
```rust
let mut health_ticker = interval(Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS));  // 10s
let mut tick_count = 0u32;

loop {
    health_ticker.tick().await;  // LINE 927 - Waits 10s
    tick_count += 1;

    let token_statuses = stream_clone.detailed_status();  // LINE 931

    // Count tokens by status (lines 934-952)
    let mut healthy_ws = 0;
    let mut quiet_ws = 0;
    let mut stale_ws = 0;
    let mut disconnected = 0;
    let mut unhealthy = 0;

    for (_short_id, detail) in &token_statuses {
        if detail.contains("HEALTHY_WS") { healthy_ws += 1; }
        else if detail.contains("QUIET_WS") { quiet_ws += 1; }
        else if detail.contains("STALE_WS") { stale_ws += 1; }
        else if detail.contains("DISCONNECTED") { disconnected += 1; }
        else if detail.contains("UNHEALTHY") { unhealthy += 1; }
    }

    // LINE 957 - THE TRIGGER
    let has_issues = disconnected > 0 || unhealthy > 0 || stale_ws > 0;

    // LINE 959 - LOGS WHEN: every 30s OR immediately when issues exist
    if tick_count % 3 == 0 || has_issues {
        if has_issues {
            warn!("[HEALTH] 🔍 ACTIVE_WS:{} QUIET_WS:{} STALE:{} DISC:{} UNHEALTHY:{} | {}",
                healthy_ws, quiet_ws, stale_ws, disconnected, unhealthy,
                stream_clone.connection_status()
            );
            // Also logs details for problematic tokens (lines 967-973)
            for (short_id, detail) in &token_statuses {
                if detail.contains("STALE") || detail.contains("DISCONNECTED") || detail.contains("UNHEALTHY") {
                    warn!("[HEALTH]   └─ {}: {}", short_id, detail);
                }
            }
        }
    }
}
```

### Timing Breakdown
- **Ticker fires**: Every 10 seconds
- **Throttled logs**: Every 30 seconds (when tick_count % 3 == 0)
- **Immediate logs**: **EVERY 10 SECONDS** when any token shows problems

---

## 5. WebSocket Connection Management

### Two Parallel Streams

#### PRICE_STREAM
- **Location**: Lines 493-701
- **Purpose**: Subscribes to price changes
- **Spawned**: Line 493
- **Handles**:
  - WebSocket reconnection with exponential backoff (1s → 2s → 4s ... → 60s max)
  - MAX_RECONNECT_DELAY_MS (line 43): 60000ms
  - MAX_RECONNECT_ATTEMPTS (line 49): 0 (infinite retries)

#### BOOK_STREAM
- **Location**: Lines 711-910
- **Purpose**: Subscribes to orderbook (bid/ask sizes)
- **Spawned**: Line 711
- **Handles**: Same reconnection logic as PRICE_STREAM

#### REST_FALLBACK
- **Location**: Lines 993-1137
- **Purpose**: Polls REST API when WebSocket data is stale
- **Interval**: 3 seconds (line 1002)
- **Fallback condition** (lines 1027-1035):
  ```rust
  let should_fallback = !prices.ws_connected
                     || prices.is_unhealthy()
                     || (now_ms - prices.last_update_ms) > WS_CONNECTED_AGE_MS;
  ```

---

## 6. Real-World Scenario: 48-Hour Run

### Market Characteristics
- 15-minute options (expiry every 15m)
- ~6-8 markets tracked simultaneously
- Each market has quiet periods (especially in dead hours)

### Typical Timeline
```
t=0s
├─ Market opens
├─ YES/NO token prices stream active
└─ ws_connected = true, last_update_ms = now

t=10s → t=85s
├─ Prices updated regularly
├─ age < 10s
└─ Status: HEALTHY_WS (no warnings)

t=95s (PROBLEM BEGINS)
├─ No updates for 95 seconds (market is quiet/illiquid)
├─ is_very_stale() returns true (line 136: age > 90000ms)
└─ Status: STALE_WS

t=105s
├─ Health check runs (tick_count=11, tick_count % 3 != 0)
├─ has_issues = true (because stale_ws > 0)
├─ Triggers line 961: warn!("[HEALTH]...")
└─ ⚠️ WARNING #1

t=115s, t=125s, t=135s ... t=300s
├─ Health check runs every 10s
├─ has_issues still true (still no updates, still STALE)
├─ Triggers line 961 every time
└─ ⚠️ WARNING every 10 seconds (15+ warnings during 2-3 minute quiet window)

t=305s
├─ REST fallback kicks in (updates mid_price)
├─ But ws_connected still true, last_ws_update_ms still old
├─ Status remains: STALE_WS
└─ Warnings continue
```

### Volume Impact Across 48 Hours

**For 6 markets with these characteristics:**
- Each market has ~4 trading windows per hour (15-min market rotation)
- Each market dies for 5-10 minutes during low-volume windows
- **Per dead period**: 30-60 warnings (one every 10s × 3-6 minutes)
- **Per market per hour**: ~120-240 warnings
- **Total for 48 hours**:
  - 6 markets × 48 hours × ~180 warnings/hour = **51,840 warnings**

**Reality Check**: The logging is probably not that aggressive in practice, but you can see why it gets excessive.

---

## 7. The Core Design Issue

### What the Code Does Right
- ✅ Detects genuine disconnections
- ✅ Handles stream errors robustly
- ✅ Falls back to REST API when needed
- ✅ Reconnects with exponential backoff

### What the Code Does Wrong
- ❌ Conflates "connection is bad" with "no data for >90 seconds"
- ❌ Treats legitimate quiet market periods as failures
- ❌ No distinction between "connection died" vs "market dried up"
- ❌ Logs **every 10 seconds** instead of throttling for repeated issues
- ❌ 90-second threshold is too low for 15-minute options markets

### The Fix Options (Not Implemented)
1. **Increase staleness threshold** from 90s to 300s (5 minutes)
2. **Separate logging for different issue types** (real connection problems vs. natural quiet)
3. **Implement better throttling** (log first issue once, then only if state changes)
4. **Change "STALE_WS" definition** to only trigger on actual connection problems, not time-based staleness

---

## 8. How Data Flows

```
Polymarket WebSocket
    ↓
PRICE_STREAM (async task)
    ├─ Receives price changes
    ├─ Updates prices_clone DashMap (line 571-603)
    ├─ Sets: ws_connected = true, last_ws_update_ms = timestamp
    └─ Sends to mpsc channel

BOOK_STREAM (async task)
    ├─ Receives orderbook updates
    ├─ Updates bid/ask sizes (line 801-828)
    ├─ Sets: last_update_ms = now
    └─ Sends to mpsc channel

REST_FALLBACK (async task)
    ├─ Checks every 3s if should_fallback (line 1027-1035)
    ├─ Calls midpoint() and last_trade_price() APIs
    ├─ Updates mid_price (line 1092-1097)
    ├─ Sets: last_update_ms = now (but NOT last_ws_update_ms)
    └─ Sends to mpsc channel

HEALTH_MONITOR (async task)
    ├─ Checks every 10s: detailed_status() for all tokens
    ├─ Counts by: HEALTHY_WS, QUIET_WS, STALE_WS, DISCONNECTED, UNHEALTHY
    ├─ Logs if: has_issues OR tick_count % 3 == 0
    └─ ⚠️ WARNING if stale_ws > 0
```

---

## 9. Key Variables and Their Meanings

### In TokenPrices struct (lines 70-87)
```rust
pub ws_connected: bool              // WebSocket connection active (line 79)
pub last_update_ms: i64             // Last ANY update (WS or API) (line 78)
pub last_ws_update_ms: i64          // Last WEBSOCKET update (line 84)
pub consecutive_errors: u32         // Errors since last successful update (line 83)
pub ws_message_count: u64           // Total WS messages received (line 85)
pub api_fallback_count: u64         // Total API fallback calls (line 86)
```

### Status Determination
- **HEALTHY_WS**: `ws_connected && ws_age < 10s`
- **QUIET_WS**: `ws_connected && ws_age >= 10s && ws_age < 90s`
- **STALE_WS**: `ws_connected && ws_age >= 90s` ← **THIS TRIGGERS WARNINGS**
- **DISCONNECTED**: `!ws_connected`
- **UNHEALTHY**: `consecutive_errors >= 3`

---

## 10. Important Comments in Code

### Line 51-54: Silent Disconnect Threshold
```rust
/// Silent disconnect threshold: if no data received for this long while "connected", log warning
/// Increased to 90s to accommodate natural quiet periods in 15-min options markets
/// This is now only used for logging, not for triggering reconnections
```

The comment admits this was increased to 90s for markets like yours, but it's still too aggressive.

### Line 159: Data Source Logic
```rust
pub fn data_source(&self) -> &str {
    if self.ws_connected && self.age_ms() < WS_CONNECTED_AGE_MS {
        "WS"
    } else {
        "API"
    }
}
```

The system is smart about fallback but doesn't prevent warning logs.

---

## Summary Table

| Aspect | Value | Location |
|--------|-------|----------|
| Health check interval | 10 seconds | Line 57 |
| Warning log interval | Every 10s when issues exist | Lines 959-974 |
| Staleness threshold | 90 seconds | Line 54 |
| Stream error threshold | 3 consecutive errors | Line 60 |
| REST fallback interval | 3 seconds | Line 39 |
| WS fallback trigger | 5 seconds without data | Line 36 |
| Entry point | multi_market_logger_hourly | Line 746 (stream call) |
| Stream spawning | Line 462 (async fn stream) | |

---

## Files to Review

1. **Main health logic**: `src/api/polymarket.rs` - Lines 917-985
2. **Status classification**: `src/api/polymarket.rs` - Lines 129-202 (TokenPrices methods)
3. **Entry point**: `src/bin/multi_market_logger_hourly.rs` - Line 752 (`.stream()` call)
4. **Connection detail**: `src/api/polymarket.rs` - Lines 167-202 (connection_detail method)

---

## Reproduction Steps

To reproduce excessive warnings:
1. Run: `cargo run --bin multi_market_logger_hourly -- 172800 btc_data.csv 0`
2. During quiet market windows (5-10 minute periods), observe warnings every 10 seconds
3. Expected: 50-100+ warning lines per quiet window
4. Expected total for 48h run: 500-1000+ warnings

