# signal-shm API Reference

Shared-memory signal engine for Polymarket crypto binary option trading.
Single writer process, multiple reader bots. Lock-free reads via seqlock protocol.

## 1. Overview

```
                        SIGNAL ENGINE (single process)
                        ==============================
  Binance SBE WS ──┐
    (bookTicker,    │   ┌──────────────────────────────┐
     depth20,       ├──>│                              │
     aggTrade)      │   │   subscribe + compute        │
                    │   │                              │     ┌──────────────────────┐
  Polymarket WS ────┤   │   binance mid/bid/ask/vol    │     │  /dev/shm/           │
    (book, trades)  ├──>│   poly BBA + depth + OBI     ├────>│  poly-signals.shm    │
                    │   │   RTDS deviations            │     │                      │
  RTDS Chainlink ───┤   │   greeks, strike, TTX        │     │  [header]            │
    (crypto_prices, ├──>│   z-score, trade flow        │     │  [slot 0: btc-15m]   │
     crypto_prices_ │   │                              │     │  [slot 1: btc-5m]    │
     chainlink)     │   └──────────────────────────────┘     │  [slot 2: eth-15m]   │
                    │                                        │  ...                 │
  Gamma API ────────┘                                        └─────────┬────────────┘
    (strike, tokens,                                                   │
     resolution)                                          mmap read (zero-copy)
                                                                       │
                                                          ┌────────────┼────────────┐
                                                          │            │            │
                                                       ┌──▼──┐     ┌──▼──┐     ┌──▼──┐
                                                       │ bot │     │ bot │     │ mon │
                                                       │ btc │     │ eth │     │ CLI │
                                                       └─────┘     └─────┘     └─────┘
```

### Performance

| Operation | Latency | Notes |
|-----------|---------|-------|
| Writer: update one field | <1 us | Single atomic store + seqlock bump |
| Writer: full slot update | <1 us | ~40 field writes between begin/end |
| Reader: snapshot read | <100 ns | Seqlock retry on torn read (rare) |
| Reader: staleness check | <10 ns | Single u64 compare |
| SHM file open | ~50 us | One-time mmap at startup |

### Guarantees

- **Consistency**: Seqlock ensures readers never see partially-written slots.
- **No blocking**: Readers never block the writer. Writers never block readers.
- **Crash safety**: Readers detect writer crash via heartbeat + PID. SHM file survives engine restart.
- **Single writer**: Only one engine process may write. Enforced by PID check in header.


## 2. Shared Memory Layout

**File**: `/dev/shm/poly-signals.shm` (configurable, default `DEFAULT_SHM_PATH`)

**Total size**: `size_of::<ShmHeader>() + slot_count * size_of::<AssetSlot>()`

### ShmHeader

Occupies the first bytes of the file. All fields are naturally aligned.

```rust
#[repr(C)]
pub struct ShmHeader {
    pub magic: u64,          // SHM_MAGIC = 0x504F4C5953494731 ("POLYSIG1")
    pub version: u32,        // SHM_VERSION = 1
    pub slot_count: u32,     // Number of asset slots allocated
    pub engine_pid: u32,     // PID of the writer process
    pub _pad0: u32,          // Alignment padding
    pub heartbeat_us: u64,   // Microsecond timestamp, updated every engine tick
    pub _reserved: [u64; 8], // Future use (64 bytes)
}
```

| Field | Type | Description |
|-------|------|-------------|
| `magic` | `u64` | Must equal `SHM_MAGIC`. Readers reject file if mismatch. |
| `version` | `u32` | Protocol version. Readers reject if `!= SHM_VERSION`. |
| `slot_count` | `u32` | Number of `AssetSlot` entries following the header. Max `MAX_SLOTS` (64). |
| `engine_pid` | `u32` | Writer's PID. Readers check `/proc/{pid}/` to confirm liveness. |
| `heartbeat_us` | `u64` | Writer updates this every tick. Readers detect stale engine if `now - heartbeat > threshold`. |

### AssetSlot

One slot per asset/timeframe pair (e.g., BTC-15m, ETH-5m). Fixed size, cache-line padded.

```rust
#[repr(C, align(64))]
pub struct AssetSlot {
    // --- Seqlock ---
    pub seq: AtomicU64,              // Even = idle, odd = write in progress

    // --- Identity ---
    pub asset: [u8; 8],             // e.g. b"btc\0\0\0\0\0"
    pub timeframe_secs: u32,        // 300 (5m) or 900 (15m)
    pub _pad_id: u32,

    // --- Binance (SBE bookTicker + depth20 + aggTrade) ---
    pub binance_mid: f64,           // (bid + ask) / 2, USD
    pub binance_bid: f64,           // Best bid, USD
    pub binance_ask: f64,           // Best ask, USD
    pub binance_spread_bps: f64,    // (ask - bid) / mid * 10000, bps
    pub binance_volume_1s: f64,     // 1-second aggTrade volume, USDT
    pub binance_volume_spike: u8,   // 1 = volume spike detected, 0 = normal
    pub _pad_vol: [u8; 7],

    // --- Binance L2 Depth (depth20@100ms SBE) ---
    pub binance_bid_depth_3: f64,   // Top-3 bid depth, USDT
    pub binance_ask_depth_3: f64,   // Top-3 ask depth, USDT
    pub binance_bid_depth_all: f64, // Full-book bid depth (20 levels), USDT
    pub binance_ask_depth_all: f64, // Full-book ask depth (20 levels), USDT
    pub binance_obi: f64,           // Order book imbalance: (bid-ask)/(bid+ask)

    // --- Volatility (Fibonacci multi-timeframe, Welford's) ---
    pub vol_1s: f64,                // 1-second realized vol, annualized
    pub vol_5s: f64,                // 5-second realized vol, annualized
    pub vol_13s: f64,               // 13-second realized vol, annualized
    pub vol_34s: f64,               // 34-second realized vol, annualized
    pub vol_89s: f64,               // 89-second realized vol, annualized
    pub realized_vol: f64,          // Fibonacci-blended vol (BS input), annualized
    pub vol_source: u8,             // 0=REALTIME_FIBONACCI, 1=1MIN_FALLBACK, 2=DEFAULT
    pub volatility_regime: u8,      // 0=low, 1=normal, 2=high, 3=extreme
    pub _pad_vol2: [u8; 6],

    // --- Polymarket Book (WS book stream) ---
    pub yes_bid: f64,               // YES best bid, 0.00-1.00
    pub yes_ask: f64,               // YES best ask, 0.00-1.00
    pub yes_bid_size: f64,          // YES best bid size, shares
    pub yes_ask_size: f64,          // YES best ask size, shares
    pub no_bid: f64,                // NO best bid, 0.00-1.00
    pub no_ask: f64,                // NO best ask, 0.00-1.00
    pub no_bid_size: f64,           // NO best bid size, shares
    pub no_ask_size: f64,           // NO best ask size, shares
    pub yes_spread_bps: f64,        // (yes_ask - yes_bid) / 1.0 * 10000, bps
    pub no_spread_bps: f64,         // (no_ask - no_bid) / 1.0 * 10000, bps

    // --- Polymarket Depth (WS book stream, full book) ---
    pub yes_bid_depth_3: f64,       // YES top-3 bid depth, shares
    pub yes_ask_depth_3: f64,       // YES top-3 ask depth, shares
    pub no_bid_depth_3: f64,        // NO top-3 bid depth, shares
    pub no_ask_depth_3: f64,        // NO top-3 ask depth, shares
    pub yes_bid_depth_all: f64,     // YES full bid depth, shares
    pub yes_ask_depth_all: f64,     // YES full ask depth, shares
    pub no_bid_depth_all: f64,      // NO full bid depth, shares
    pub no_ask_depth_all: f64,      // NO full ask depth, shares
    pub yes_obi: f64,               // YES OBI: (bid_d - ask_d)/(bid_d + ask_d)
    pub no_obi: f64,                // NO OBI
    pub combined_obi: f64,          // Combined OBI (always ~0 on binary, prefer yes_obi)

    // --- Polymarket Trade Flow (WS trade stream) ---
    pub yes_last_trade_price: f64,  // Last YES trade price, 0.00-1.00
    pub yes_last_trade_size: f64,   // Last YES trade size, shares
    pub yes_last_trade_side: u8,    // 0=buy, 1=sell
    pub no_last_trade_side: u8,     // 0=buy, 1=sell
    pub _pad_trade: [u8; 6],
    pub no_last_trade_price: f64,   // Last NO trade price, 0.00-1.00
    pub no_last_trade_size: f64,    // Last NO trade size, shares
    pub trade_flow_imbalance: f64,  // Net buy-sell imbalance, signed

    // --- RTDS (Polymarket's price feed relay) ---
    pub rtds_binance_price: f64,    // Polymarket's view of Binance, USD
    pub rtds_chainlink_price: f64,  // Chainlink oracle price, USD
    pub rtds_binance_dev_bps: f64,  // (direct - rtds_binance) / direct * 10000
    pub rtds_chainlink_dev_bps: f64,// (direct - rtds_chainlink) / direct * 10000
    pub rtds_price_adv_bps: f64,    // max(|binance_dev|, |chainlink_dev|)
    pub rtds_cl_bn_spread_bps: f64, // (rtds_binance - rtds_chainlink) / rtds_binance * 10000
    pub rtds_binance_stale_ms: i64, // Age of RTDS Binance data, ms (-1 = no data)
    pub rtds_chainlink_stale_ms: i64,// Age of RTDS Chainlink data, ms (-1 = no data)

    // --- Market Lifecycle (Gamma API + derived) ---
    pub strike: f64,                // Binary option strike price, USD
    pub ttx_seconds: i64,           // Time to expiry, seconds (counts down)
    pub time_to_open: i64,          // Seconds until market opens (0 = live)
    pub market_resolved: u8,        // 1 = market has resolved
    pub _pad_lc: [u8; 7],

    // --- Greeks (Black-Scholes derived) ---
    pub delta_yes: f64,             // YES delta, 0.0-1.0
    pub gamma_greek: f64,           // Gamma
    pub vega: f64,                  // Vega
    pub theta: f64,                 // Theta, per-second

    // --- Token IDs (for Polymarket WS subscriptions) ---
    pub yes_token_id: [u8; TOKEN_ID_LEN], // 78-char hex token ID, null-padded
    pub no_token_id: [u8; TOKEN_ID_LEN],  // 78-char hex token ID, null-padded
    pub condition_id: [u8; TOKEN_ID_LEN], // Condition ID, null-padded
    pub market_slug: [u8; 128],           // e.g. "btc-updown-15m-1711900800"

    // --- Timestamps ---
    pub binance_update_us: u64,     // Last Binance update, microseconds since epoch
    pub poly_update_us: u64,        // Last Polymarket book update, microseconds since epoch
    pub rtds_update_us: u64,        // Last RTDS update, microseconds since epoch
    pub gamma_update_us: u64,       // Last Gamma API fetch, microseconds since epoch
}
```

### Constants

```rust
pub const SHM_MAGIC: u64 = 0x504F4C5953494731;  // "POLYSIG1"
pub const SHM_VERSION: u32 = 1;
pub const MAX_SLOTS: u32 = 64;
pub const TOKEN_ID_LEN: usize = 80;             // 78 chars + null + pad
pub const DEFAULT_SHM_PATH: &str = "/dev/shm/poly-signals.shm";
```

### Seqlock Protocol

The seqlock ensures readers get a consistent snapshot without blocking the writer.

**Writer side**:
```
seq.store(seq + 1, Release)   // seq becomes ODD → write in progress
... write all fields ...
seq.store(seq + 1, Release)   // seq becomes EVEN → write complete
```

**Reader side**:
```
loop {
    let s1 = seq.load(Acquire)
    if s1 & 1 != 0 { continue }   // ODD = writer active, retry
    ... copy all fields ...
    let s2 = seq.load(Acquire)
    if s1 == s2 { break }         // consistent snapshot
    // else: writer touched slot during read, retry
}
```

Retries are extremely rare in practice (<0.01% of reads) because a full slot read
completes in ~50ns and the writer updates each slot only once per engine tick (~10ms).


## 3. Reader API

### `SignalReader::open(path: &str) -> Result<SignalReader>`

Opens an existing SHM file and validates the header.

```rust
let reader = SignalReader::open("/dev/shm/poly-signals.shm")?;
```

**Errors**:
- File does not exist (engine not started)
- Magic mismatch (corrupted or wrong file)
- Version mismatch (incompatible engine version)
- File too small (truncated)

### `SignalReader::read(asset: &str, timeframe_secs: u32) -> Option<SignalSnapshot>`

Reads a consistent snapshot for the given asset and timeframe. Scans slots by matching
`asset` (case-insensitive) and `timeframe_secs`. Returns `None` if no matching slot.

```rust
if let Some(btc) = reader.read("btc", 900) {
    println!("BTC mid: ${:.2}", btc.binance_mid);
}
```

### `SignalReader::read_slot(index: usize) -> Option<SignalSnapshot>`

Reads a consistent snapshot by slot index. Returns `None` if index is out of bounds
or the slot has never been written (seq == 0).

```rust
let slot0 = reader.read_slot(0)?;
```

### `SignalReader::engine_alive() -> bool`

Returns `true` if:
1. The PID in the header corresponds to a running process
2. The heartbeat timestamp is less than 5 seconds old

```rust
if !reader.engine_alive() {
    error!("Signal engine is dead — using stale data");
}
```

### `SignalSnapshot`

Immutable copy of all fields from an `AssetSlot` at a single consistent point in time.
All fields match the `AssetSlot` layout, with string types converted from fixed-length
byte arrays to `String`.

```rust
pub struct SignalSnapshot {
    // Identity
    pub asset: String,              // "btc", "eth", "sol", "xrp"
    pub timeframe_secs: u32,        // 300 or 900

    // Binance
    pub binance_mid: f64,           // USD
    pub binance_bid: f64,           // USD
    pub binance_ask: f64,           // USD
    pub binance_spread_bps: f64,    // bps
    pub binance_volume_1s: f64,     // USDT
    pub binance_volume_spike: bool,

    // Binance L2 Depth
    pub binance_bid_depth_3: f64,   // USDT
    pub binance_ask_depth_3: f64,   // USDT
    pub binance_bid_depth_all: f64, // USDT
    pub binance_ask_depth_all: f64, // USDT
    pub binance_obi: f64,           // ratio, -1.0..1.0

    // Volatility
    pub vol_1s: f64,                // annualized
    pub vol_5s: f64,
    pub vol_13s: f64,
    pub vol_34s: f64,
    pub vol_89s: f64,
    pub realized_vol: f64,          // Fibonacci blend
    pub vol_source: String,         // "REALTIME_FIBONACCI" | "1MIN_FALLBACK" | "DEFAULT"
    pub volatility_regime: String,  // "low" | "normal" | "high" | "extreme"

    // Polymarket Book
    pub yes_bid: f64,               // 0.00-1.00
    pub yes_ask: f64,
    pub yes_bid_size: f64,          // shares
    pub yes_ask_size: f64,
    pub no_bid: f64,
    pub no_ask: f64,
    pub no_bid_size: f64,
    pub no_ask_size: f64,
    pub yes_spread_bps: f64,        // bps
    pub no_spread_bps: f64,

    // Polymarket Depth
    pub yes_bid_depth_3: f64,       // shares
    pub yes_ask_depth_3: f64,
    pub no_bid_depth_3: f64,
    pub no_ask_depth_3: f64,
    pub yes_bid_depth_all: f64,
    pub yes_ask_depth_all: f64,
    pub no_bid_depth_all: f64,
    pub no_ask_depth_all: f64,
    pub yes_obi: f64,               // ratio, -1.0..1.0
    pub no_obi: f64,
    pub combined_obi: f64,          // ~0 on binary (use yes_obi)

    // Trade Flow
    pub yes_last_trade_price: f64,
    pub yes_last_trade_size: f64,
    pub yes_last_trade_side: String,// "buy" | "sell"
    pub no_last_trade_price: f64,
    pub no_last_trade_size: f64,
    pub no_last_trade_side: String,
    pub trade_flow_imbalance: f64,

    // RTDS
    pub rtds_binance_price: f64,    // USD
    pub rtds_chainlink_price: f64,  // USD
    pub rtds_binance_dev_bps: f64,  // bps (clock-safe)
    pub rtds_chainlink_dev_bps: f64,// bps (clock-safe)
    pub rtds_price_adv_bps: f64,    // bps (clock-safe)
    pub rtds_cl_bn_spread_bps: f64, // bps (clock-safe)
    pub rtds_binance_stale_ms: i64, // ms, -1 = no data
    pub rtds_chainlink_stale_ms: i64,

    // Market Lifecycle
    pub strike: f64,                // USD
    pub ttx_seconds: i64,           // seconds remaining
    pub time_to_open: i64,          // seconds until open
    pub market_resolved: bool,

    // Greeks
    pub delta_yes: f64,
    pub gamma_greek: f64,
    pub vega: f64,
    pub theta: f64,

    // Token IDs
    pub yes_token_id: String,
    pub no_token_id: String,
    pub condition_id: String,
    pub market_slug: String,

    // Timestamps
    pub binance_update_us: u64,     // microseconds since epoch
    pub poly_update_us: u64,
    pub rtds_update_us: u64,
    pub gamma_update_us: u64,
}
```

### Staleness Detection

```rust
impl SignalReader {
    /// Check if a snapshot's data source is stale.
    /// `threshold_us`: maximum acceptable age in microseconds.
    pub fn is_stale(snap: &SignalSnapshot, threshold_us: u64) -> bool {
        let now_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;
        // Check all source timestamps — stale if ANY critical source is old
        let binance_age = now_us.saturating_sub(snap.binance_update_us);
        let poly_age = now_us.saturating_sub(snap.poly_update_us);
        binance_age > threshold_us || poly_age > threshold_us
    }
}
```


## 4. Writer API

### `SignalWriter::create(path: &str, slot_count: u32) -> Result<SignalWriter>`

Creates a new SHM file (or overwrites existing), writes the header, and returns a writer.
Sets `engine_pid` to the current process PID.

```rust
let mut writer = SignalWriter::create("/dev/shm/poly-signals.shm", 8)?;
```

**Behavior**:
- Allocates `header_size + slot_count * slot_size` bytes
- Writes magic, version, slot_count, PID
- Zeros all slots
- File permissions: `0o644` (owner rw, group/other r)

### `SignalWriter::slot(index: usize) -> &AssetSlot`

Returns a reference to the slot at the given index for writing. Panics if out of bounds.

### `AssetSlot::begin_write()` / `end_write()`

Seqlock write bracket. All field writes MUST occur between these calls.

```rust
let slot = writer.slot(0);
slot.begin_write();
slot.write_f64(offset_of!(AssetSlot, binance_mid), 87432.50);
slot.write_f64(offset_of!(AssetSlot, yes_bid), 0.48);
slot.write_f64(offset_of!(AssetSlot, yes_ask), 0.52);
slot.end_write();
```

**begin_write**: Increments `seq` to odd (signals "write in progress").
**end_write**: Increments `seq` to even (signals "write complete"). Also updates the header heartbeat.

### Atomic Field Helpers

```rust
impl AssetSlot {
    /// Write an f64 at the given byte offset within the slot.
    pub fn write_f64(&self, offset: usize, value: f64);

    /// Read an f64 at the given byte offset (used internally by reader).
    pub fn read_f64(&self, offset: usize) -> f64;

    /// Write a string into a fixed-length byte array field.
    /// Truncates if longer than the field, null-pads if shorter.
    pub fn write_string(&self, offset: usize, len: usize, value: &str);

    /// Read a string from a fixed-length byte array field (strips null padding).
    pub fn read_string(&self, offset: usize, len: usize) -> String;

    /// Write a u64/i64/u32/u8 at the given byte offset.
    pub fn write_u64(&self, offset: usize, value: u64);
    pub fn write_i64(&self, offset: usize, value: i64);
    pub fn write_u32(&self, offset: usize, value: u32);
    pub fn write_u8(&self, offset: usize, value: u8);
}
```

### Writer Heartbeat

The writer must call `writer.heartbeat()` periodically (typically every engine tick).
This updates `ShmHeader::heartbeat_us` so readers can detect a dead engine.

```rust
// In the engine's main loop:
loop {
    // ... compute signals, write slots ...
    writer.heartbeat(); // updates header.heartbeat_us = now
    sleep(tick_interval);
}
```


## 5. Signal Fields Reference

| Field | Type | Unit | Source | Update Freq | Notes |
|-------|------|------|--------|-------------|-------|
| **Identity** | | | | | |
| `asset` | string | — | Config | once | "btc", "eth", "sol", "xrp" |
| `timeframe_secs` | u32 | seconds | Config | once | 300 (5m) or 900 (15m) |
| `yes_token_id` | string | — | Gamma API | per-rotation | 78-char hex, changes every market window |
| `no_token_id` | string | — | Gamma API | per-rotation | 78-char hex, changes every market window |
| `condition_id` | string | — | Gamma API | per-rotation | CTF condition ID |
| `market_slug` | string | — | Gamma API | per-rotation | e.g. "btc-updown-15m-1711900800" |
| **Binance Price** | | | | | |
| `binance_mid` | f64 | USD | Binance SBE | per-tick ~10ms | (bid + ask) / 2 |
| `binance_bid` | f64 | USD | Binance SBE | per-tick ~10ms | Best bid price |
| `binance_ask` | f64 | USD | Binance SBE | per-tick ~10ms | Best ask price |
| `binance_spread_bps` | f64 | bps | Derived | per-tick ~10ms | (ask - bid) / mid * 10000 |
| `binance_volume_1s` | f64 | USDT | Binance SBE | per-second | Rolling 1s aggTrade volume |
| `binance_volume_spike` | bool | — | Derived | per-second | Volume exceeds spike threshold |
| **Binance L2 Depth** | | | | | |
| `binance_bid_depth_3` | f64 | USDT | Binance SBE | ~100ms | Sum of top 3 bid levels |
| `binance_ask_depth_3` | f64 | USDT | Binance SBE | ~100ms | Sum of top 3 ask levels |
| `binance_bid_depth_all` | f64 | USDT | Binance SBE | ~100ms | Sum of all 20 bid levels |
| `binance_ask_depth_all` | f64 | USDT | Binance SBE | ~100ms | Sum of all 20 ask levels |
| `binance_obi` | f64 | ratio | Derived | ~100ms | (bid_d - ask_d) / (bid_d + ask_d), range -1..1 |
| **Volatility** | | | | | |
| `vol_1s` | f64 | annualized | Derived | per-tick | Welford's 1-second window |
| `vol_5s` | f64 | annualized | Derived | per-tick | Welford's 5-second window |
| `vol_13s` | f64 | annualized | Derived | per-tick | Welford's 13-second window |
| `vol_34s` | f64 | annualized | Derived | per-tick | Welford's 34-second window |
| `vol_89s` | f64 | annualized | Derived | per-tick | Welford's 89-second window |
| `realized_vol` | f64 | annualized | Derived | per-tick | Fibonacci-weighted blend (BS model input) |
| `vol_source` | enum | — | Derived | per-tick | 0=REALTIME_FIBONACCI, 1=1MIN_FALLBACK, 2=DEFAULT(0.55) |
| `volatility_regime` | enum | — | Derived | per-tick | 0=low, 1=normal, 2=high, 3=extreme |
| **Polymarket Book** | | | | | |
| `yes_bid` | f64 | price | Polymarket WS | per-tick ~50ms | YES best bid (0.00-1.00) |
| `yes_ask` | f64 | price | Polymarket WS | per-tick ~50ms | YES best ask (0.00-1.00) |
| `yes_bid_size` | f64 | shares | Polymarket WS | per-tick ~50ms | Size at YES best bid |
| `yes_ask_size` | f64 | shares | Polymarket WS | per-tick ~50ms | Size at YES best ask |
| `no_bid` | f64 | price | Polymarket WS | per-tick ~50ms | NO best bid (0.00-1.00) |
| `no_ask` | f64 | price | Polymarket WS | per-tick ~50ms | NO best ask (0.00-1.00) |
| `no_bid_size` | f64 | shares | Polymarket WS | per-tick ~50ms | Size at NO best bid |
| `no_ask_size` | f64 | shares | Polymarket WS | per-tick ~50ms | Size at NO best ask |
| `yes_spread_bps` | f64 | bps | Derived | per-tick ~50ms | (yes_ask - yes_bid) * 10000 |
| `no_spread_bps` | f64 | bps | Derived | per-tick ~50ms | (no_ask - no_bid) * 10000 |
| **Polymarket Depth** | | | | | |
| `yes_bid_depth_3` | f64 | shares | Polymarket WS | per-tick ~50ms | YES top-3 bid levels |
| `yes_ask_depth_3` | f64 | shares | Polymarket WS | per-tick ~50ms | YES top-3 ask levels |
| `no_bid_depth_3` | f64 | shares | Polymarket WS | per-tick ~50ms | NO top-3 bid levels |
| `no_ask_depth_3` | f64 | shares | Polymarket WS | per-tick ~50ms | NO top-3 ask levels |
| `yes_bid_depth_all` | f64 | shares | Polymarket WS | per-tick ~50ms | YES full book bid |
| `yes_ask_depth_all` | f64 | shares | Polymarket WS | per-tick ~50ms | YES full book ask |
| `no_bid_depth_all` | f64 | shares | Polymarket WS | per-tick ~50ms | NO full book bid |
| `no_ask_depth_all` | f64 | shares | Polymarket WS | per-tick ~50ms | NO full book ask |
| `yes_obi` | f64 | ratio | Derived | per-tick ~50ms | YES-side OBI (this is the useful one) |
| `no_obi` | f64 | ratio | Derived | per-tick ~50ms | NO-side OBI |
| `combined_obi` | f64 | ratio | Derived | per-tick ~50ms | Always ~0 on binary markets (mirror book) |
| **Trade Flow** | | | | | |
| `yes_last_trade_price` | f64 | price | Polymarket WS | per-trade | Last YES trade (0.00-1.00) |
| `yes_last_trade_size` | f64 | shares | Polymarket WS | per-trade | Last YES trade size |
| `yes_last_trade_side` | enum | — | Polymarket WS | per-trade | 0=buy, 1=sell |
| `no_last_trade_price` | f64 | price | Polymarket WS | per-trade | Last NO trade (0.00-1.00) |
| `no_last_trade_size` | f64 | shares | Polymarket WS | per-trade | Last NO trade size |
| `no_last_trade_side` | enum | — | Polymarket WS | per-trade | 0=buy, 1=sell |
| `trade_flow_imbalance` | f64 | signed | Derived | per-trade | Net buy-sell imbalance |
| **RTDS** | | | | | |
| `rtds_binance_price` | f64 | USD | RTDS | ~100ms | Polymarket's view of Binance price |
| `rtds_chainlink_price` | f64 | USD | RTDS | ~1s | Chainlink oracle price (~60% Binance weight) |
| `rtds_binance_dev_bps` | f64 | bps | Derived | ~100ms | Clock-safe: (direct - rtds) / direct * 10000 |
| `rtds_chainlink_dev_bps` | f64 | bps | Derived | ~1s | Clock-safe: deviation from Chainlink |
| `rtds_price_adv_bps` | f64 | bps | Derived | ~100ms | Clock-safe: max(\|binance_dev\|, \|chainlink_dev\|) |
| `rtds_cl_bn_spread_bps` | f64 | bps | Derived | ~1s | Clock-safe: Chainlink vs RTDS Binance deviation |
| `rtds_binance_stale_ms` | i64 | ms | Derived | ~100ms | Age of RTDS Binance data (-1 = never received) |
| `rtds_chainlink_stale_ms` | i64 | ms | Derived | ~1s | Age of RTDS Chainlink data (-1 = never received) |
| **Market Lifecycle** | | | | | |
| `strike` | f64 | USD | Gamma API | per-rotation | Binary option strike price |
| `ttx_seconds` | i64 | seconds | Derived | per-second | Time to expiry (countdown) |
| `time_to_open` | i64 | seconds | Derived | per-second | Seconds until market opens (0 = live) |
| `market_resolved` | bool | — | Gamma API | per-rotation | True after settlement |
| **Greeks** | | | | | |
| `delta_yes` | f64 | — | Derived | per-tick | BS delta of YES token (0.0-1.0) |
| `gamma_greek` | f64 | — | Derived | per-tick | BS gamma |
| `vega` | f64 | — | Derived | per-tick | BS vega |
| `theta` | f64 | per-second | Derived | per-tick | BS theta (negative = time decay) |
| **Timestamps** | | | | | |
| `binance_update_us` | u64 | microseconds | Internal | per-tick | Last Binance write to this slot |
| `poly_update_us` | u64 | microseconds | Internal | per-tick | Last Polymarket write to this slot |
| `rtds_update_us` | u64 | microseconds | Internal | per-update | Last RTDS write to this slot |
| `gamma_update_us` | u64 | microseconds | Internal | per-rotation | Last Gamma API write to this slot |

### Source Legend

| Source | Description | Connection |
|--------|-------------|------------|
| **Binance SBE** | Direct Binance WebSocket, SBE binary encoding | Authenticated WS, bestBidAsk + depth20@100ms + aggTrade streams |
| **Polymarket WS** | Polymarket CLOB WebSocket | Book stream (L2) + trade stream per token ID |
| **RTDS** | Polymarket Real-Time Data Stream | `crypto_prices` (Binance relay) + `crypto_prices_chainlink` |
| **Gamma API** | Polymarket event metadata REST API | HTTP, polled at market rotation (~5min/15min) |
| **Derived** | Computed from other fields | In-engine calculation |


## 6. Configuration

### Engine YAML Config

```yaml
# signal-engine.yml
shm:
  path: /dev/shm/poly-signals.shm
  slot_count: 8

markets:
  - asset: btc
    timeframe: 15m
    binance_symbol: BTCUSDT

  - asset: btc
    timeframe: 5m
    binance_symbol: BTCUSDT

  - asset: eth
    timeframe: 15m
    binance_symbol: ETHUSDT

  - asset: eth
    timeframe: 5m
    binance_symbol: ETHUSDT

  - asset: sol
    timeframe: 15m
    binance_symbol: SOLUSDT

  - asset: sol
    timeframe: 5m
    binance_symbol: SOLUSDT

  - asset: xrp
    timeframe: 15m
    binance_symbol: XRPUSDT

# Binance SBE WebSocket
binance:
  use_sbe: true              # SBE binary encoding (vs JSON fallback)
  depth_levels: 20           # depth20@100ms stream
  reconnect_delay_ms: 1000
  max_reconnect_delay_ms: 60000

# Volatility
volatility:
  staleness_threshold_s: 120 # Fall back to 1MIN kline vol after 120s no updates
  default_vol: 0.55          # DEFAULT regime when no data available
  kline_bootstrap_count: 70  # Klines to fetch at startup for vol seed
  kline_refresh_count: 10    # Klines to refresh every 5 minutes
  cache_ttl_ms: 500          # Vol cache TTL (avoids recomputation)
  dedup_threshold_bps: 0.1   # Skip identical price updates (0.1 bps tolerance)

# RTDS
rtds:
  enabled: true
  reconnect_delay_ms: 1000
  max_reconnect_delay_ms: 60000
```

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `BINANCE_API_KEY` | Yes (SBE) | Binance API key for authenticated SBE WebSocket |
| `BINANCE_SECRET_KEY` | No | Only needed if using signed REST endpoints |
| `RUST_LOG` | No | Tracing filter, e.g. `signal_engine=info,signal_shm=debug` |


## 7. Usage Examples

### Strategy Bot (Reader)

```rust
use signal_shm::{SignalReader, SignalSnapshot};

fn main() -> anyhow::Result<()> {
    let reader = SignalReader::open("/dev/shm/poly-signals.shm")?;

    // Check engine is alive before trading
    if !reader.engine_alive() {
        anyhow::bail!("Signal engine not running");
    }

    // Read BTC 15-minute signals
    let btc = reader.read("btc", 900).expect("BTC 15m slot not found");

    println!("BTC: ${:.2} vs strike ${:.2}, TTX={}s",
        btc.binance_mid, btc.strike, btc.ttx_seconds);
    println!("BBA: YES {:.2}/{:.2}  NO {:.2}/{:.2}",
        btc.yes_bid, btc.yes_ask, btc.no_bid, btc.no_ask);
    println!("Vol: {:.4} ({}), Regime: {}",
        btc.realized_vol, btc.vol_source, btc.volatility_regime);
    println!("RTDS edge: {:.1}bps, Chainlink spread: {:.1}bps",
        btc.rtds_price_adv_bps, btc.rtds_cl_bn_spread_bps);
    println!("YES OBI: {:.3}, Depth: {:.0}/{:.0}",
        btc.yes_obi, btc.yes_bid_depth_all, btc.yes_ask_depth_all);

    // Staleness check (5 second threshold)
    if SignalReader::is_stale(&btc, 5_000_000) {
        eprintln!("WARNING: signal data is stale!");
    }

    // Token IDs for Polymarket order submission
    println!("YES token: {}", btc.yes_token_id);
    println!("NO token:  {}", btc.no_token_id);
    println!("Slug:      {}", btc.market_slug);

    Ok(())
}
```

### Strategy Loop (Hot Path)

```rust
use signal_shm::SignalReader;
use std::time::Duration;

fn run_strategy(reader: &SignalReader) {
    loop {
        // Zero-copy read — <100ns per snapshot
        if let Some(btc) = reader.read("btc", 900) {
            // Skip if data is stale
            if SignalReader::is_stale(&btc, 2_000_000) {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }

            // Combined ask = cheap side indicator
            let combined_ask = btc.yes_ask + btc.no_ask;
            let cheap_side = if btc.yes_ask <= btc.no_ask { "YES" } else { "NO" };

            // RTDS edge detection (clock-safe signals only)
            if btc.rtds_price_adv_bps > 3.0 {
                // We see a Binance move the book hasn't priced yet
                // ... front-run logic ...
            }

            // ... rest of strategy ...
        }

        std::thread::sleep(Duration::from_millis(300)); // 300ms cycle
    }
}
```

### Signal Engine (Writer)

```rust
use signal_shm::SignalWriter;

fn main() -> anyhow::Result<()> {
    let mut writer = SignalWriter::create("/dev/shm/poly-signals.shm", 8)?;

    // Configure slot 0 for BTC 15m
    let slot = writer.slot(0);
    slot.begin_write();
    slot.write_string(offset_of!(AssetSlot, asset), 8, "btc");
    slot.write_u32(offset_of!(AssetSlot, timeframe_secs), 900);
    slot.end_write();

    // Main engine loop
    loop {
        // ... receive Binance tick ...

        let slot = writer.slot(0);
        slot.begin_write();
        slot.write_f64(offset_of!(AssetSlot, binance_mid), 87432.50);
        slot.write_f64(offset_of!(AssetSlot, binance_bid), 87432.00);
        slot.write_f64(offset_of!(AssetSlot, binance_ask), 87433.00);
        slot.write_f64(offset_of!(AssetSlot, binance_spread_bps), 0.114);
        slot.write_u64(offset_of!(AssetSlot, binance_update_us), now_us());
        slot.end_write();

        writer.heartbeat();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

### Monitoring CLI

```rust
// signal-reader --path /dev/shm/poly-signals.shm --dump
use signal_shm::SignalReader;

fn dump(reader: &SignalReader) {
    println!("Engine PID: {}, alive: {}", reader.engine_pid(), reader.engine_alive());

    for i in 0..reader.slot_count() {
        if let Some(s) = reader.read_slot(i) {
            let age_ms = (now_us() - s.binance_update_us) / 1000;
            println!("[{}] {}-{}s  BTC=${:.2}  YES {:.2}/{:.2}  NO {:.2}/{:.2}  vol={:.4}  age={}ms",
                i, s.asset, s.timeframe_secs,
                s.binance_mid, s.yes_bid, s.yes_ask, s.no_bid, s.no_ask,
                s.realized_vol, age_ms);
        }
    }
}
```


## 8. Operational Notes

### Startup Order

1. Start the signal engine first: `cargo run --bin signal-engine -- --config signal-engine.yml`
2. Wait for "SHM ready" log message
3. Start strategy bots — they open the existing SHM file

If a bot starts before the engine, `SignalReader::open()` returns an error (file not found).
Bots should retry with backoff.

### Single Writer Invariant

Only ONE writer process may exist at a time. The engine writes its PID to the header.
If a second engine starts, it overwrites the SHM file — the first engine's writes go
to the old mmap and readers see the new engine's data. This is safe but wasteful.
Use process supervisors (systemd) to ensure single-instance.

### Engine Crash Detection

Readers detect a dead engine via two signals:

1. **PID check**: `reader.engine_alive()` checks if `/proc/{pid}/` exists
2. **Heartbeat staleness**: `header.heartbeat_us` stops updating when engine dies

Strategy bots should check `engine_alive()` on every cycle and enter a safe mode
(cancel open orders, stop placing new ones) if the engine is down.

### SHM File Persistence

`/dev/shm/` is a tmpfs — files persist across process restarts but not across reboots.
When the engine restarts, it overwrites the file with a fresh header. Readers that
already have the file mmap'd will see the new data after the next read (the mmap
points to the same tmpfs inode if the path is the same).

If the engine is restarted while bots are running, bots should:
1. Detect heartbeat gap (engine down period)
2. Re-validate header magic/version after heartbeat resumes
3. Resume normal operation

### Monitoring

```bash
# Live signal dump (refreshes every second)
watch -n1 'cargo run --bin signal-reader -- --path /dev/shm/poly-signals.shm --dump'

# Check SHM file exists and size
ls -la /dev/shm/poly-signals.shm

# Check engine process
ps aux | grep signal-engine

# Inspect raw bytes (debugging)
hexdump -C /dev/shm/poly-signals.shm | head -20
```

### Performance Tuning

- Pin the engine to a dedicated CPU core (`taskset -c 0`) to avoid context-switch jitter
- Bots reading at 300ms intervals will never contend with the writer (10ms tick)
- The seqlock retry loop has no backoff — retries are <0.01% of reads and complete in nanoseconds
- SHM file is always hot in page cache (tmpfs) — no disk I/O
