# Hourly Asset Logger Changes

## Files Created

| New File | Copied From |
|----------|-------------|
| `src/bin/hourly_asset_logger.rs` | `src/bin/multi_asset_logger_fixed.rs` |
| `config/hourly_asset_logger.yml` | `config/multi_asset_logger.yml` |

`Cargo.toml` was updated to add the new `[[bin]]` target.

## Key Changes from 15-Minute Logger

### 1. Market Discovery (slug matching)

**15-minute markets** use epoch-based slugs constructed deterministically:
```
btc-updown-15m-1773637200
```
The logger calculates `window_start = (now / 900) * 900` and builds the slug directly.

**Hourly markets** use human-readable slugs with variable format:
```
bitcoin-up-or-down-march-17-2026-8am-et
ethereum-up-or-down-march-17-5am-et
solana-up-or-down-march-16-2026-10am-et
xrp-up-or-down-march-17-2026-3pm-et
```

The hourly logger cannot construct slugs from timestamps. Instead, it uses the Gamma API
search endpoint with `slug_contains` filter:
```
GET /events?slug_contains=bitcoin-up-or-down-&active=true&closed=false&limit=10
```

Asset code mapping for slug prefixes:
- `btc` -> `bitcoin-up-or-down-`
- `eth` -> `ethereum-up-or-down-`
- `sol` -> `solana-up-or-down-`
- `xrp` -> `xrp-up-or-down-`

Slug validation checks that the slug ends with `am-et` or `pm-et`.

### 2. Market Rotation

**15-minute logger**: Tries WS `new_market` event first (filtered by `-updown-` pattern), falls back to `gamma::fetch_next_market_for()` which constructs the next epoch-based slug.

**Hourly logger**: WS `new_market` events do NOT match hourly slugs (the `MarketEventStream` module filters for `-updown-` pattern). Market rotation uses Gamma API search exclusively via `fetch_next_hourly_market()`. The WS `market_resolved` events still work for resolution detection since they match by exact slug.

### 3. Timeframe Constants

| Parameter | 15-minute | Hourly |
|-----------|-----------|--------|
| `window_seconds` | 300 or 900 | 3600 |
| `RESOLUTION_GRACE_SECS` | 30 | 60 |
| `FETCH_RETRY_COOLDOWN_MS` | 5000 | 10000 |
| Config `timeframe` field | `"5m"` or `"15m"` | `"1h"` |
| Market tag format | `BTC-15M` | `BTC-1H` |

### 4. Kline Interval and Volatility

**No change.** Both loggers use 1-minute Binance klines for volatility bootstrap (70 candles). The `vol_1sec`, `vol_5sec`, `vol_13sec`, `vol_34sec`, `vol_89sec` columns are computed from real-time tick data (via the `VolatilityCalculator`), not from klines. The klines only seed the initial state. No adjustment is needed for the hourly timeframe.

### 5. Strike Price Fetching

The existing `strike::variant_for_window()` maps `window_seconds` to the Polymarket crypto-price API variant parameter: 300 -> `"five"`, anything else -> `"fifteen"`. For hourly markets (3600s), this falls through to `"fifteen"`, which may not return the correct strike.

The hourly logger uses a **limited retry** (max 10 attempts) instead of infinite retry, and falls back to the current Binance spot price if the strike API does not support hourly markets.

### 6. Output Directory Structure

**15-minute logger**: `{csv_dir}/{asset}/{timeframe}/` (e.g., `./btc/15m/`)

**Hourly logger**: `{csv_dir}/{asset}/1h/` (e.g., `./hourly/btc/1h/`). Default csv_dir is `./hourly` instead of `.`.

### 7. CSV Schema

**Identical.** Same 80+ columns, same header, same format string. CSV files from the hourly logger are fully compatible with the 15-minute logger's backtest pipeline.

### 8. Config Format

The hourly config uses `timeframe: 1h` instead of `timeframe: 5m` or `timeframe: 15m`. Only `1h` is accepted by the hourly logger. The same `asset` and `binance_symbol` fields are used.
