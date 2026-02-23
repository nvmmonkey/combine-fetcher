# Blushing-Fine Trader SELL Order Analysis

**Generated**: 2026-02-23
**Data**: Feb 21, 2026 -- 28 overlapping BTC 15-min up/down markets
**Trader**: Blushing-Fine (proxy wallet 0x63ce...9a)
**Method**: Mapped trader SELL orders to our RTDS market data (sub-second resolution)

## Executive Summary

Analysis of **37 sell decisions** across **28 markets** on Feb 21, 2026,
mapped against our RTDS z-score and market data.

The trader (Blushing-Fine) follows a **delta-neutral-to-directional pivot** strategy:

1. **Open**: BUY both Up and Down at market open (taker)
2. **Pivot**: SELL the losing side mid-window (maker) -- the directional decision
3. **Hold**: Ride the remaining directional position to expiry

### Key Findings at a Glance

| Question | Answer |
|----------|--------|
| At what dev_bps does the trader sell? | Median |dev_bps| = **3.13 bps** (P25=2.27, P75=7.15) |
| When in the window? | Median **5.1 min** into market (34% of window) |
| Sells the losing side? | **75.7%** of the time |
| Typical loss on sell | **21.6%** per share ($0.101 drop) |
| How much of holding sold? | Median **21%** of losing-side position |
| Correlation (dev_bps vs timing) | r = **0.0539** |

---

## 1. Z-Score / Dev_bps at Sell Time

`dev_bps = (binance_price - strike_price) / strike_price * 10000` -- how far BTC has moved from the strike.
Positive = above strike (Up favored). Negative = below strike (Down favored).

Note: This is NOT `rtds_binance_dev_bps` from our CSV (which measures inter-feed deviation, near zero).
This is the actual price-vs-strike deviation that determines market outcome.

### Distribution Statistics

| Metric | Value |
|--------|-------|
| Mean dev_bps (signed) | +2.17 bps |
| Median dev_bps (signed) | +3.05 bps |
| Std Dev | 6.02 bps |
| Range | [-18.91, 13.30] bps |
| **|dev_bps| Mean** | **4.79 bps** |
| **|dev_bps| Median** | **3.13 bps** |
| |dev_bps| P10 | 0.52 bps |
| |dev_bps| P25 | 2.27 bps |
| |dev_bps| P75 | 7.15 bps |
| |dev_bps| P90 | 11.06 bps |

### Dev_bps Bucket Distribution

| |dev_bps| range | Count | Pct | Histogram |
|-----------------|-------|-----|-----------|
| < 1 bps | 6 | 16.2% | ######## |
| 1-2 bps | 2 | 5.4% | ## |
| 2-3 bps | 5 | 13.5% | ###### |
| 3-5 bps | 12 | 32.4% | ################ |
| 5-7 bps | 2 | 5.4% | ## |
| 7-10 bps | 5 | 13.5% | ###### |
| 10-15 bps | 4 | 10.8% | ##### |
| 15-20 bps | 1 | 2.7% | # |
| 20-30 bps | 0 | 0.0% |  |
| > 30 bps | 0 | 0.0% |  |

### Interpretation

The trader sells at remarkably small deviations (median 3.1 bps).
They do NOT wait for large z-score moves. They sell into small deviations,
suggesting they are reading the price trajectory/momentum rather than waiting for a threshold.

## 2. Sell Timing

### Time-to-Expiry at Sell

| Metric | Value |
|--------|-------|
| Mean TTE | 9.16 min |
| Median TTE | 9.93 min |
| Std Dev | 3.29 min |
| Range | [3.40, 14.40] min |

### Time Into Market (since open)

| Metric | Value |
|--------|-------|
| Mean time in | 5.84 min |
| Median time in | 5.07 min |
| Std Dev | 3.29 min |

### Timing Windows

| Window | TTE | Count | Pct |
|--------|-----|-------|-----|
| Early | > 10 min | 17 | 45.9% |
| Mid | 5-10 min | 13 | 35.1% |
| Late | < 5 min | 7 | 18.9% |

### Time-Into-Market Histogram

| Time since open | Count | Pct | Histogram |
|-----------------|-------|-----|-----------|
| 0-2 min | 5 | 13.5% | ###### |
| 2-4 min | 7 | 18.9% | ######### |
| 4-6 min | 10 | 27.0% | ############# |
| 6-8 min | 5 | 13.5% | ###### |
| 8-10 min | 3 | 8.1% | #### |
| 10-12 min | 7 | 18.9% | ######### |
| 12-15 min | 0 | 0.0% |  |

### Interpretation

The trader's median first-sell is at **5.1 min** (34% of window).
Sells are distributed across the window, suggesting condition-driven rather than timer-driven behavior.

## 3. Directional Analysis: Selling the Losing Side

| Metric | Value |
|--------|-------|
| Selling the LOSING side | **75.7%** (28/37) |
| Selling AGAINST z-score direction | 75.7% |
| Up sells | 18 |
| Down sells | 19 |

### Losing vs Winning Side Sells (|dev_bps| comparison)

| | Losing Side Sells | Winning Side Sells |
|--|---|---|
| Mean |dev_bps| | 5.69 bps | 2.00 bps |
| Median |dev_bps| | 4.57 bps | 2.27 bps |

### Interpretation

The trader overwhelmingly sells the **losing side** (76%).
This confirms classic loss-cutting: they exit the unfavored outcome to ride the winner.

**Both-side sells**: In 9 markets, the trader sold BOTH Up and Down:
  - `btc-updown-15m-1771632000`
  - `btc-updown-15m-1771633800`
  - `btc-updown-15m-1771634700`
  - `btc-updown-15m-1771688700`
  - `btc-updown-15m-1771694100`
  - `btc-updown-15m-1771695900`
  - `btc-updown-15m-1771698600`
  - `btc-updown-15m-1771706700`
  - `btc-updown-15m-1771711200`

This indicates mid-market direction changes or active position rebalancing.

## 4. Price Drop from Buy to Sell (Loss Taken)

A positive price drop means the trader sold BELOW their buy VWAP (taking a loss).
A negative price drop means they sold ABOVE their buy VWAP (a gain).

| Metric | Value |
|--------|-------|
| Mean price drop (buy_vwap - sell_price) | $0.1024 |
| Median price drop | $0.1011 |
| Std Dev | $0.0877 |
| Mean price drop % | 20.1% |
| Median price drop % | 21.6% |
| Sells at a LOSS (drop > 0) | 33 |
| Sells at a GAIN (drop < 0) | 4 |
| Sells at EVEN | 0 |
| Mean loss per sell event (USDC) | $-27.41 |
| Median loss per sell event (USDC) | $-11.78 |
| **Total P&L from all sells (USDC)** | **$-1014.15** |

### Interpretation

The trader typically takes a **$0.101 loss per share** (21.6%) on sells.
This is the cost of pivoting from delta-neutral to directional.
Total USDC loss from selling: **$-1014.15** across 37 events.

## 5. Combined Cost Before vs After Sells

The 'combined cost' represents the effective per-share-pair cost of the DN position.
Selling one side recovers capital and reduces the effective cost per remaining share.

| Metric | Value |
|--------|-------|
| Mean combined cost BEFORE sells | $1.9688 |
| Mean combined cost AFTER sells | $1.8884 |
| Median BEFORE | $1.2371 |
| Median AFTER | $1.1845 |
| Mean improvement | $0.0804 |
| Markets analyzed | 28 |

### Per-Market Breakdown

| Market (epoch) | Before | After | Improvement | Net Cost | Remaining Up | Remaining Down |
|----------------|--------|-------|-------------|----------|-------------|---------------|
| 1771701300 | $19.174 | $15.985 | $+3.189 | $391.58 | 1008 | 24 |
| 1771685100 | $1.245 | $0.957 | $+0.288 | $350.28 | 479 | 366 |
| 1771687800 | $1.317 | $1.046 | $+0.271 | $461.56 | 441 | 693 |
| 1771711200 | $1.354 | $1.100 | $+0.253 | $540.72 | 491 | 633 |
| 1771709400 | $1.663 | $1.423 | $+0.241 | $418.66 | 746 | 294 |
| 1771699500 | $1.248 | $1.022 | $+0.225 | $1188.71 | 1163 | 1560 |
| 1771705800 | $1.977 | $1.771 | $+0.207 | $255.42 | 365 | 144 |
| 1771686000 | $1.221 | $1.090 | $+0.131 | $400.45 | 367 | 488 |
| 1771698600 | $1.081 | $0.980 | $+0.100 | $639.01 | 811 | 652 |
| 1771635600 | $1.334 | $1.257 | $+0.078 | $621.80 | 495 | 935 |
| 1771682400 | $1.250 | $1.177 | $+0.073 | $540.93 | 460 | 512 |
| 1771714800 | $1.316 | $1.259 | $+0.057 | $456.42 | 362 | 670 |
| 1771697700 | $0.933 | $0.893 | $+0.040 | $741.67 | 831 | 930 |
| 1771717500 | $1.513 | $1.477 | $+0.036 | $361.37 | 878 | 245 |
| 1771707600 | $3.266 | $3.236 | $+0.030 | $349.63 | 108 | 613 |
| 1771686900 | $0.983 | $0.956 | $+0.027 | $885.80 | 1100 | 926 |
| 1771716600 | $1.361 | $1.337 | $+0.024 | $258.97 | 194 | 439 |
| 1771679700 | $0.990 | $0.974 | $+0.016 | $388.49 | 399 | 538 |
| 1771632900 | $0.996 | $1.007 | $-0.011 | $1437.35 | 1428 | 1556 |
| 1771632000 | $0.941 | $0.962 | $-0.021 | $1440.13 | 2310 | 1497 |
| 1771694100 | $1.229 | $1.283 | $-0.054 | $459.18 | 654 | 358 |
| 1771690500 | $1.183 | $1.240 | $-0.057 | $1010.63 | 815 | 1286 |
| 1771633800 | $1.156 | $1.267 | $-0.111 | $1842.20 | 2682 | 1454 |
| 1771700400 | $0.935 | $1.063 | $-0.128 | $365.49 | 469 | 344 |
| 1771634700 | $0.890 | $1.036 | $-0.146 | $2380.72 | 2752 | 2297 |
| 1771688700 | $0.958 | $1.192 | $-0.234 | $677.86 | 920 | 569 |
| 1771695900 | $1.179 | $1.636 | $-0.457 | $581.43 | 669 | 355 |
| 1771706700 | $2.435 | $4.248 | $-1.813 | $798.22 | 188 | 1455 |

## 6. Sell Size as Percentage of Pre-Sell Holdings

Sell % is computed as: total_shares_sold / shares_owned_before_first_sell.
Values >100% indicate interleaved buy-sell patterns (trader re-buys and re-sells within the same window).

| Metric | Value |
|--------|-------|
| Mean sell % | 42.9% |
| Median sell % | 20.9% |
| Std Dev | 53.4% |
| Range | [1.3%, 278.9%] |
| Full sells (>=90%) | 4 (10.8%) |
| Partial sells (<90%) | 33 |

### Interpretation

The trader sells only a **portion** (21%) of their pre-sell position.
This suggests the directional pivot is **incremental**, not all-at-once.
The trader sells a fraction, observes market reaction, and may sell more later or re-buy.

**Important caveat**: Some markets show values >100% because the trader interleaves
buy-sell-buy-sell sequences within the same window (active market-making mixed with
directional pivoting). The simple pre-first-sell denominator understates their true activity.

## 7. Volatility Regime at Sell Time

| Regime | Count | Pct |
|--------|-------|-----|
| EXTREME | 18 | 48.6% |
| LOW | 4 | 10.8% |
| NORMAL | 15 | 40.5% |

## 8. Early vs Late Sells (Subgroup Comparison)

Comparing sells that happen in the first 5 minutes vs after 5 minutes:

| Metric | Early (<5 min) | Late (>=5 min) |
|--------|---------------|----------------|
| Count | 17 | 20 |
| Mean |dev_bps| | 4.76 bps | 4.82 bps |
| Median |dev_bps| | 3.45 bps | 3.11 bps |
| % selling losing side | 64.7% | 85.0% |
| Median price drop % | 23.9% | 18.4% |

### Interpretation

Early sells happen at LARGER deviations -- the trader reacts quickly to big moves.
Late sells have a higher rate of selling the losing side.

## 9. Correlations

| Relationship | Pearson r |
|-------------|-----------|
| |dev_bps| vs time_into_market | +0.0539 |
| |dev_bps| vs price_drop_pct | +0.0897 |

### Interpretation

**dev_bps vs timing** (r=+0.0539): Weak -- sell timing is not primarily driven by deviation magnitude.

**dev_bps vs loss** (r=+0.0897): Weak -- the loss taken does not strongly depend on deviation magnitude.

## 10. Detailed Sell Event Table

Sorted by |dev_bps| ascending.

| # | Market (epoch) | Sold | dev_bps | Time In | TTE | Pre-Sell VWAP | Sell Avg | Drop % | Sell % | Losing? | Regime |
|---|----------------|------|---------|---------|-----|----------|----------|--------|--------|---------|--------|
| 1 | 1771711200 | Up | -0.21 | 8.1m | 6.9m | $0.534 | $0.436 | +18.3% | 7% | YES | EXTREME |
| 2 | 1771682400 | Up | +0.22 | 5.6m | 9.4m | $0.384 | $0.343 | +10.7% | 40% | no | LOW |
| 3 | 1771706700 | Up | -0.44 | 8.8m | 6.2m | $0.418 | $0.341 | +18.4% | 57% | YES | NORMAL |
| 4 | 1771709400 | Up | -0.52 | 6.1m | 8.9m | $0.532 | $0.398 | +25.2% | 46% | YES | EXTREME |
| 5 | 1771688700 | Up | +0.80 | 3.7m | 11.3m | $0.469 | $0.368 | +21.6% | 55% | no | EXTREME |
| 6 | 1771632900 | Up | +0.96 | 5.1m | 9.9m | $0.327 | $0.270 | +17.4% | 4% | no | EXTREME |
| 7 | 1771634700 | Up | -1.29 | 2.6m | 12.4m | $0.403 | $0.369 | +8.3% | 21% | YES | EXTREME |
| 8 | 1771632000 | Up | -1.45 | 3.2m | 11.8m | $0.486 | $0.321 | +34.0% | 75% | YES | EXTREME |
| 9 | 1771701300 | Up | +2.19 | 5.1m | 9.9m | $0.380 | $0.317 | +16.6% | 28% | no | EXTREME |
| 10 | 1771695900 | Up | +2.27 | 4.9m | 10.1m | $0.560 | $0.435 | +22.4% | 18% | no | EXTREME |
| 11 | 1771714800 | Down | +2.30 | 10.2m | 4.8m | $0.466 | $0.440 | +5.5% | 8% | YES | NORMAL |
| 12 | 1771686900 | Up | +2.44 | 1.3m | 13.7m | $0.474 | $0.420 | +11.4% | 21% | no | EXTREME |
| 13 | 1771633800 | Up | +2.55 | 2.6m | 12.4m | $0.498 | $0.322 | +35.4% | 117% | no | EXTREME |
| 14 | 1771679700 | Down | +3.01 | 7.9m | 7.1m | $0.455 | $0.230 | +49.4% | 9% | YES | NORMAL |
| 15 | 1771716600 | Down | +3.05 | 11.6m | 3.4m | $0.322 | $0.390 | -21.1% | 3% | YES | NORMAL |
| 16 | 1771706700 | Down | +3.07 | 4.9m | 10.1m | $0.489 | $0.351 | +28.3% | 7% | YES | EXTREME |
| 17 | 1771694100 | Down | +3.10 | 7.3m | 7.7m | $0.576 | $0.394 | +31.6% | 13% | YES | NORMAL |
| 18 | 1771698600 | Up | +3.11 | 4.3m | 10.7m | $0.472 | $0.417 | +11.6% | 20% | no | NORMAL |
| 19 | 1771694100 | Up | -3.13 | 11.5m | 3.5m | $0.380 | $0.390 | -2.5% | 6% | YES | NORMAL |
| 20 | 1771695900 | Down | +3.37 | 10.0m | 5.0m | $0.471 | $0.394 | +16.2% | 139% | YES | EXTREME |
| 21 | 1771685100 | Up | +3.45 | 0.6m | 14.4m | $0.398 | $0.403 | -1.2% | 55% | no | NORMAL |
| 22 | 1771634700 | Down | +3.57 | 5.1m | 9.9m | $0.597 | $0.307 | +48.5% | 49% | YES | EXTREME |
| 23 | 1771707600 | Down | +4.15 | 1.8m | 13.2m | $0.447 | $0.400 | +10.5% | 1% | YES | NORMAL |
| 24 | 1771711200 | Down | +4.99 | 1.9m | 13.1m | $0.446 | $0.324 | +27.3% | 129% | YES | NORMAL |
| 25 | 1771633800 | Down | +5.00 | 1.2m | 13.8m | $0.487 | $0.421 | +13.6% | 55% | YES | NORMAL |
| 26 | 1771686000 | Down | +5.91 | 11.2m | 3.8m | $0.442 | $0.373 | +15.7% | 21% | YES | EXTREME |
| 27 | 1771632000 | Down | +6.31 | 10.9m | 4.1m | $0.517 | $0.384 | +25.8% | 18% | YES | EXTREME |
| 28 | 1771700400 | Down | +7.15 | 11.3m | 3.7m | $0.480 | $0.377 | +21.4% | 21% | YES | NORMAL |
| 29 | 1771687800 | Down | +7.17 | 3.0m | 12.0m | $0.518 | $0.339 | +34.5% | 87% | YES | NORMAL |
| 30 | 1771699500 | Down | +7.91 | 2.2m | 12.8m | $0.661 | $0.390 | +41.0% | 279% | YES | NORMAL |
| 31 | 1771635600 | Down | +8.02 | 9.1m | 5.9m | $0.355 | $0.234 | +34.2% | 24% | YES | LOW |
| 32 | 1771688700 | Down | +8.65 | 4.8m | 10.2m | $0.561 | $0.357 | +36.4% | 69% | YES | LOW |
| 33 | 1771698600 | Down | +10.75 | 6.2m | 8.8m | $0.481 | $0.361 | +24.9% | 6% | YES | LOW |
| 34 | 1771717500 | Up | -11.06 | 3.8m | 11.2m | $0.416 | $0.250 | +39.9% | 8% | YES | EXTREME |
| 35 | 1771690500 | Up | -11.62 | 4.9m | 10.1m | $0.394 | $0.300 | +23.9% | 7% | YES | NORMAL |
| 36 | 1771697700 | Down | +13.30 | 5.6m | 9.4m | $0.219 | $0.369 | -68.6% | 22% | YES | EXTREME |
| 37 | 1771705800 | Up | -18.91 | 7.9m | 7.1m | $0.357 | $0.147 | +58.9% | 45% | YES | EXTREME |

## 11. Parameter Mapping to Our Strategy

Mapping the trader's empirical sell behavior to our backtest parameter space.

### A. Sell Timing --> `dir_no_trade_before_pct`

The trader's median first-sell is at **5.1 min** into the 15-min window (34%).
Standard deviation: 3.3 min.

**Recommended**: `dir_no_trade_before_pct = 34`

However, the trader reacts to conditions, not a fixed timer. This should be a minimum gate,
not the sole trigger. The actual trigger is the dev_bps signal (see below).

### B. Dev_bps at Sell --> `dir_min_z_mid`, `dir_emergency_min_z`

| Percentile | |dev_bps| |
|------------|----------|
| P10 (rare small sells) | 0.52 bps |
| P25 (lower quartile) | 2.27 bps |
| P50 (median) | 3.13 bps |
| P75 (upper quartile) | 7.15 bps |
| P90 (aggressive sells) | 11.06 bps |

**Recommended**:
  - `dir_min_z_mid = 2.3` bps (minimum signal to consider a directional pivot)
  - `dir_emergency_min_z = 7.2` bps (strong signal for aggressive position change)

To convert to z-scores: divide by your volatility normalization factor.
For typical 15-min BTC vol (~0.3%), 1 bps ~ 0.03 z-score.

### C. Loss Tolerance --> `dn_abandon_pct`

The trader takes a median loss of **21.6%** per share when selling.
Mean loss: 20.1%.

**Recommended**: `dn_abandon_pct = 21`
(Abandon the DN hedge when the losing side has dropped this percentage from entry)

### D. Position Tilt --> Emergency Hedge Sizing

The trader sells a median **21%** of the losing side.

The trader keeps most of the position hedged.
**Recommended**: `emergency_sell_pct = 21`

### E. Combined Parameter Set

```toml
# Derived from Blushing-Fine sell behavior analysis (Feb 21, 2026)
# Based on 37 sell events across 28 markets

dir_no_trade_before_pct = 34       # Minimum 5.1min into 15min window before directional pivot
dir_min_z_mid = 2.3                # Dev_bps threshold for directional consideration (P25)
dir_emergency_min_z = 7.2           # Dev_bps threshold for aggressive selling (P75)
dn_abandon_pct = 21                    # Abandon DN when side loses this % from entry
emergency_sell_pct = 21             # Partial exit of losing side

# Supporting data:
# Trader's median sell price: $0.101 below buy VWAP
# Trader's total sell P&L: $-1014.15 across 37 events
# Correlation (dev_bps vs timing): r = 0.0539
# Losing-side sell rate: 75.7%
# Volatility regimes at sell: {'EXTREME': 18, 'LOW': 4, 'NORMAL': 15}
```

---

## Methodology

1. **Data join**: Trader's `event_slug` matched to our market CSV filenames
2. **Timestamp matching**: Binary search on ISO timestamps (sub-second market data vs 1-second trader data)
3. **VWAP calculation**: Volume-weighted average price across all fills per side per market
4. **Sell aggregation**: All sell orders for the same slug+outcome aggregated into one 'sell event'
5. **Coverage**: 28 of the trader's 56 sell-active markets overlap with our 50 market CSVs
6. **Total sell events**: 37 (some markets have sells on both sides)

### Limitations

- Market data has sub-second resolution; trader timestamps have 1-second resolution
- Only 28/56 sell-active markets overlap with our RTDS data
- Single day of data (Feb 21) -- strategy may vary across sessions
- dev_bps is raw deviation, not a vol-normalized z-score
- 9 markets had sells on BOTH sides, complicating the simple 'sell loser' model
- Buy VWAP includes all buys (early + mid-market), not just initial position