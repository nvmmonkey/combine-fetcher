# Multi-Market Slug Analysis Report

> Generated: 2026-02-22 13:46:05
> Data source: `/home/lehash/development/combine-fetcher/221data/selected`
> Markets analyzed: 6
> Total data rows: 476,017
> Fee model: `poly_fee_rate(p) = 25.0 * (p*(1-p))^2`

## Market Classification Summary

| Classification | Count |
|---------------|-------|
| DIR_FAVORABLE | 2 |
| MIXED | 4 |

| Outcome | Count |
|---------|-------|
| NO | 6 |


## Per-Market Detailed Analysis

### `btc-updown-15m-1771632000`
- **Class**: MIXED | **Outcome**: NO | **Resolved**: True
- **Strike**: $67,990.16 | **RV**: 0.306522
- **Price**: $68,035.12 → $68,017.35
- **Rows**: 78,959 | **Z mean**: +0.2964 | **Z std**: 1.8168
- **Price above strike**: 62.0%

**A) Z-Score Trajectory**

Stats: mean=0.2964, std=1.8168, min=-1.7131, max=399.9108
Positive: 62.0% | >1.0: 7.2% | >2.0: 2.3% | <-1.0: 2.1% | <-2.0: 0.0%

| Time% | TTE(min) | Z-score | Price | Dev(bps) | YES ask | NO ask |
|-------|----------|---------|-------|----------|---------|--------|
| 0% | 14.87 | +0.4057 | 68,035.12 | +6.6 | 0.5700 | 0.4500 |
| 10% | 13.38 | +0.3010 | 68,002.01 | +1.7 | 0.4100 | 0.6000 |
| 20% | 11.9 | +0.2627 | 68,016.18 | +3.8 | 0.5100 | 0.5100 |
| 30% | 10.4 | +0.3486 | 68,000.70 | +1.6 | 0.4200 | 0.6000 |
| 40% | 8.92 | +0.0009 | 67,990.24 | +0.0 | 0.3500 | 0.6600 |
| 50% | 7.43 | -0.0040 | 67,989.85 | -0.1 | 0.3100 | 0.7000 |
| 60% | 5.95 | -1.4606 | 67,956.76 | -4.9 | 0.1500 | 0.8600 |
| 70% | 4.47 | +1.0005 | 68,009.99 | +2.9 | 0.4400 | 0.5700 |
| 80% | 2.97 | +0.4854 | 68,013.40 | +3.4 | 0.3600 | 0.6500 |
| 90% | 1.48 | +0.4568 | 68,005.60 | +2.3 | 0.2200 | 0.8100 |
| 95% | 0.75 | +0.2016 | 67,995.01 | +0.7 | 0.0800 | 0.9300 |
| 99% | 0.15 | -0.0753 | 67,989.35 | -0.1 | 0.0200 | 0.9900 |

**B) Z-Flip Analysis**: 12 raw | 12 filtered (>50 rows) | 12 sustained (>200 rows)

| # | Time% | TTE(min) | From→To | Z at flip | Run |
|---|-------|----------|---------|-----------|-----|
| 1 | 20.4% | 11.83 | YES→NO | -0.1597 | 16068 |
| 2 | 29.6% | 10.47 | NO→YES | +0.0109 | 7335 |
| 3 | 37.6% | 9.28 | YES→NO | -0.2230 | 6213 |
| 4 | 38.1% | 9.2 | NO→YES | +0.0009 | 508 |
| 5 | 41.5% | 8.7 | YES→NO | -0.0357 | 2614 |
| 6 | 45.2% | 8.15 | NO→YES | +0.0374 | 2882 |
| 7 | 45.9% | 8.05 | YES→NO | -0.0117 | 577 |
| 8 | 48.1% | 7.72 | NO→YES | +0.1344 | 1783 |
| 9 | 49.8% | 7.47 | YES→NO | -0.0040 | 1245 |
| 10 | 68.2% | 4.73 | NO→YES | +0.0296 | 14537 |

**C) RTDS Staleness**

Stale book %: >3bps=95.7%, >5bps=5.6%, >10bps=0.0%
Gate would block: @3bps=4.3%, @5bps=94.4%, @10bps=100.0%
Price advantage: mean=4.1778, p95=5.1039
Early advantage: 4.1991 bps | Late advantage: 4.1229 bps
Binance stale >1s: 37.9% | >5s: 0.0%
Chainlink stale >1s: 98.7% | >5s: 0.5%

**D) Volatility Regime**: {'NORMAL': 77.1, 'EXTREME': 19.9, 'HIGH': 2.1, 'LOW': 0.9} | 75 transitions
First EXTREME at: 0.8% time

| Time% | TTE | From → To |
|-------|-----|-----------|
| 0.8% | 14.75m | NORMAL → EXTREME |
| 1.3% | 14.67m | EXTREME → NORMAL |
| 2.5% | 14.5m | NORMAL → EXTREME |
| 4.0% | 14.28m | EXTREME → NORMAL |
| 6.1% | 13.97m | NORMAL → EXTREME |
| 6.7% | 13.88m | EXTREME → NORMAL |
| 9.7% | 13.43m | NORMAL → EXTREME |
| 10.1% | 13.37m | EXTREME → NORMAL |

**E) Ask Prices (Fee-Aware)**

YES ask: mean=0.3642, min=0.01 | Fee cost: mean=0.369, min=0.01
NO ask: mean=0.6493, min=0.2 | Fee cost: mean=0.6559, min=0.2013
Combined (w/fees): mean=1.0248, min=1.01 | <$1.00: 0.0% | <$0.95: 0.0%

Fee schedule: @0.30→0.3033, @0.40→0.4058, @0.45→0.4569, @0.50→0.5078

YES <0.48: 71.2% | <0.45: 67.6% | <0.42: 62.5% | <0.40: 57.6%
NO  <0.48: 16.4% | <0.45: 11.4% | <0.42: 5.7% | <0.40: 4.7%

**F) DN Entry Opportunities**

Both sides <0.48: 0 rows | <0.45: 0 | <0.42: 0
Seeding imbalance: 0.7701 (cheap=YES, bottleneck=NO)
No rows where both sides < seed_price simultaneously
Simulated one-sided abandon at: 0.0% time

**G) DIR Commit Timing**

|z|>3.0: commit at 99.5% (z=3.2639, tte=0.07m, side=YES)
|z|>2.0: commit at 71.6% (z=2.9519, tte=4.22m, side=YES)
|z|>1.5: commit at 2.5% (z=1.7085, tte=14.5m, side=YES)
|z|>1.0: commit at 0.8% (z=1.4226, tte=14.75m, side=YES)
|z|>0.5: commit at 0.8% (z=1.4226, tte=14.75m, side=YES)
Tiered mid: 3.5% qualified, first at 71.6% (z=2.9519, side=YES)
Tiered late: 57.6% qualified, first at 80.3% (z=0.588, side=YES)

**H) Emergency Hedge Analysis**

Emergency would trigger at 58.0% (z=-1.5573, side=NO)
Post-abandon z: mean=0.3671, std=2.3243, range=[-1.7131, 399.9108]

**I) Late Z-Reversal**: STABLE
Early (0-50%): z_mean=0.1424, YES=67.8%
Mid (50-80%): z_mean=0.1486, YES=39.5%
Late (80-100%): z_mean=0.9008, YES=81.5%

**J) Price Deviation Velocity**

Window 50: mean=-0.0, std=0.0062, max=0.109, p95=0.0091
Window 100: mean=-0.0, std=0.0044, max=0.0545, p95=0.0091
Window 500: mean=-0.0001, std=0.0021, max=0.0109, p95=0.0047
Acceleration: max=0.0005, p95=0.0001

**K) Chainlink Deviation Near Expiry**

last_20pct: |dev| mean=4.1229 bps, max=9.6775 bps, >5bps=3.9%, >10bps=0.0%
  Above tiered threshold: 0.0%
last_10pct: |dev| mean=4.0721 bps, max=9.6775 bps, >5bps=3.6%, >10bps=0.0%
  Above tiered threshold: 3.6%
last_5pct: |dev| mean=4.2499 bps, max=9.6775 bps, >5bps=7.4%, >10bps=0.0%
  Above tiered threshold: 99.5%

**K0) Chainlink-Binance Z-Score Disagreement** (CRITICAL)

Overall disagreement: **38.0%** of ticks
  early: disagree=52.4% | z_bn=0.2115, z_cl=-0.1995 | bn_YES=73.5%, cl_YES=21.0%
  commit: disagree=25.7% | z_bn=-0.0163, z_cl=-0.6447 | bn_YES=37.5%, cl_YES=11.9%
  late: disagree=40.5% | z_bn=0.8629, z_cl=-0.0543 | bn_YES=100.0%, cl_YES=59.5%
  expiry: disagree=25.1% | z_bn=1.0233, z_cl=-3.4944 | bn_YES=25.1%, cl_YES=0.0%

| Time% | Z Binance | Z Chainlink | BN says | CL says | Agree? |
|-------|-----------|-------------|---------|---------|--------|
| 50% | -0.0040 | -0.2355 | NO | NO | YES |
| 75% | +0.8012 | +0.2659 | YES | YES | YES |
| 90% | +0.4568 | -0.3441 | YES | NO | **NO** |
| 95% | +0.2016 | -0.8379 | YES | NO | **NO** |
| 99% | -0.0753 | -2.6179 | NO | NO | YES |

Chainlink margin from strike at expiry: mean |z_cl|=3.6387, min |z_cl|=0.8557
Late CL-BN momentum: mean=0.0183, std=0.3888 (WIDENING)

**M) Confidence Scoring**: **BROKEN** — constant at 0.5 across all 78,959 ticks
  The multi-signal confidence system is NOT producing differentiated scores. `dir_min_confidence` filtering is ineffective.

**N) DN Achievability**: 0.0% of time combined < $1.00
  First 25%: 0.0% achievable, 0.0% near break-even (<$1.005)
  Best combined 0-25%: $1.0213
  Best combined 25-50%: $1.0175
  Best combined 50-75%: $1.0125
  Best combined 75-100%: $1.0100

**O) Regime Stability**: score=0.999 (76 transitions, 1.0/1k rows)
  Longest stable run: 5907 rows, mean run: 1038.9 rows

**P) Binance Staleness**: severity=LOW (mean=887ms, p95=1440ms, max=3440ms)
  >1s: 37.9% | >5s: 0.0% | >10s: 0.0%

---

### `btc-updown-15m-1771634700`
- **Class**: MIXED | **Outcome**: NO | **Resolved**: True
- **Strike**: $67,818.89 | **RV**: 0.254278
- **Price**: $67,844.95 → $67,840.99
- **Rows**: 79,659 | **Z mean**: +0.5386 | **Z std**: 0.8547
- **Price above strike**: 87.5%

**A) Z-Score Trajectory**

Stats: mean=0.5386, std=0.8547, min=-0.8734, max=124.4559
Positive: 87.5% | >1.0: 14.3% | >2.0: 2.1% | <-1.0: 0.0% | <-2.0: 0.0%

| Time% | TTE(min) | Z-score | Price | Dev(bps) | YES ask | NO ask |
|-------|----------|---------|-------|----------|---------|--------|
| 0% | 14.93 | +0.2836 | 67,844.95 | +3.8 | 0.5600 | 0.4500 |
| 10% | 13.43 | -0.2013 | 67,811.99 | -1.0 | 0.3500 | 0.6600 |
| 20% | 11.95 | -0.1124 | 67,810.15 | -1.3 | 0.4100 | 0.6000 |
| 30% | 10.45 | +0.7364 | 67,841.15 | +3.3 | 0.5500 | 0.4700 |
| 40% | 8.95 | +0.2306 | 67,834.40 | +2.3 | 0.4800 | 0.5300 |
| 50% | 7.47 | +0.6468 | 67,860.51 | +6.1 | 0.6500 | 0.3700 |
| 60% | 5.97 | +1.1887 | 67,846.05 | +4.0 | 0.5000 | 0.5100 |
| 70% | 4.48 | +0.6212 | 67,849.85 | +4.6 | 0.5800 | 0.4300 |
| 80% | 2.98 | +0.3814 | 67,835.01 | +2.4 | 0.4400 | 0.5700 |
| 90% | 1.5 | +0.7793 | 67,842.26 | +3.5 | 0.5500 | 0.4600 |
| 95% | 0.75 | +1.3799 | 67,848.15 | +4.3 | 0.2300 | 0.8500 |
| 99% | 0.15 | +6.1020 | 67,840.99 | +3.3 | 0.0300 | 0.9900 |

**B) Z-Flip Analysis**: 6 raw | 6 filtered (>50 rows) | 6 sustained (>200 rows)

| # | Time% | TTE(min) | From→To | Z at flip | Run |
|---|-------|----------|---------|-----------|-----|
| 1 | 9.6% | 13.5 | YES→NO | -0.1164 | 7667 |
| 2 | 12.7% | 13.03 | NO→YES | +0.0207 | 2466 |
| 3 | 16.1% | 12.53 | YES→NO | -0.1735 | 2708 |
| 4 | 23.1% | 11.48 | NO→YES | +0.0118 | 5575 |
| 5 | 24.0% | 11.35 | YES→NO | -0.0079 | 754 |
| 6 | 26.5% | 10.98 | NO→YES | +0.3276 | 1948 |

**C) RTDS Staleness**

Stale book %: >3bps=92.3%, >5bps=1.6%, >10bps=0.0%
Gate would block: @3bps=7.7%, @5bps=98.4%, @10bps=100.0%
Price advantage: mean=3.5613, p95=4.3661
Early advantage: 3.7096 bps | Late advantage: 3.5104 bps
Binance stale >1s: 45.6% | >5s: 0.0%
Chainlink stale >1s: 98.9% | >5s: 0.2%

**D) Volatility Regime**: {'NORMAL': 83.7, 'EXTREME': 12.4, 'LOW': 3.2, 'HIGH': 0.6} | 60 transitions
First EXTREME at: 0.9% time

| Time% | TTE | From → To |
|-------|-----|-----------|
| 0.9% | 14.8m | NORMAL → EXTREME |
| 1.3% | 14.73m | EXTREME → LOW |
| 1.4% | 14.72m | LOW → NORMAL |
| 2.7% | 14.52m | NORMAL → EXTREME |
| 3.4% | 14.42m | EXTREME → NORMAL |
| 9.6% | 13.5m | NORMAL → EXTREME |
| 10.2% | 13.4m | EXTREME → NORMAL |
| 10.2% | 13.4m | NORMAL → EXTREME |

**E) Ask Prices (Fee-Aware)**

YES ask: mean=0.4786, min=0.01 | Fee cost: mean=0.4854, min=0.01
NO ask: mean=0.5338, min=0.23 | Fee cost: mean=0.5409, min=0.2318
Combined (w/fees): mean=1.0262, min=1.01 | <$1.00: 0.0% | <$0.95: 0.0%

Fee schedule: @0.30→0.3033, @0.40→0.4058, @0.45→0.4569, @0.50→0.5078

YES <0.48: 46.0% | <0.45: 38.3% | <0.42: 28.9% | <0.40: 25.3%
NO  <0.48: 33.1% | <0.45: 22.1% | <0.42: 16.9% | <0.40: 14.9%

**F) DN Entry Opportunities**

Both sides <0.48: 0 rows | <0.45: 0 | <0.42: 0
Seeding imbalance: 0.2795 (cheap=YES, bottleneck=NO)
No rows where both sides < seed_price simultaneously
Simulated one-sided abandon at: 0.0% time

**G) DIR Commit Timing**

|z|>3.0: commit at 97.8% (z=3.1665, tte=0.33m, side=YES)
|z|>2.0: commit at 66.3% (z=2.4365, tte=5.03m, side=YES)
|z|>1.5: commit at 47.6% (z=1.5649, tte=7.83m, side=YES)
|z|>1.0: commit at 0.9% (z=1.093, tte=14.8m, side=YES)
|z|>0.5: commit at 0.9% (z=0.8542, tte=14.8m, side=YES)
Tiered mid: 0.7% qualified, first at 66.3% (z=2.4365, side=YES)
Tiered late: 77.1% qualified, first at 84.6% (z=0.5022, side=YES)

**H) Emergency Hedge Analysis**

Emergency would trigger at 47.6% (z=1.5649, side=YES)
Post-abandon z: mean=0.7914, std=1.0003, range=[0.1067, 124.4559]

**I) Late Z-Reversal**: STABLE
Early (0-50%): z_mean=0.2305, YES=74.9%
Mid (50-80%): z_mean=0.5916, YES=100.0%
Late (80-100%): z_mean=1.2293, YES=100.0%

**J) Price Deviation Velocity**

Window 50: mean=-0.0, std=0.0058, max=0.1213, p95=0.0053
Window 100: mean=-0.0, std=0.004, max=0.064, p95=0.0075
Window 500: mean=-0.0, std=0.0018, max=0.0151, p95=0.004
Acceleration: max=0.001, p95=0.0001

**K) Chainlink Deviation Near Expiry**

last_20pct: |dev| mean=3.5104 bps, max=4.5146 bps, >5bps=0.0%, >10bps=0.0%
  Above tiered threshold: 0.0%
last_10pct: |dev| mean=3.6389 bps, max=4.1774 bps, >5bps=0.0%, >10bps=0.0%
  Above tiered threshold: 0.0%
last_5pct: |dev| mean=3.5851 bps, max=4.0616 bps, >5bps=0.0%, >10bps=0.0%
  Above tiered threshold: 91.2%

**K0) Chainlink-Binance Z-Score Disagreement** (CRITICAL)

Overall disagreement: **49.2%** of ticks
  early: disagree=49.9% | z_bn=0.1603, z_cl=-0.2595 | bn_YES=64.2%, cl_YES=14.2%
  commit: disagree=46.4% | z_bn=0.5639, z_cl=0.0866 | bn_YES=100.0%, cl_YES=53.6%
  late: disagree=49.0% | z_bn=0.6276, z_cl=-0.0619 | bn_YES=100.0%, cl_YES=51.0%
  expiry: disagree=67.8% | z_bn=2.6292, z_cl=-0.3292 | bn_YES=100.0%, cl_YES=32.2%

| Time% | Z Binance | Z Chainlink | BN says | CL says | Agree? |
|-------|-----------|-------------|---------|---------|--------|
| 50% | +0.6468 | +0.2985 | YES | YES | YES |
| 75% | +0.1561 | -0.3002 | YES | NO | **NO** |
| 90% | +0.7793 | +0.0814 | YES | YES | YES |
| 95% | +1.3799 | +0.1671 | YES | YES | YES |
| 99% | +6.1020 | -0.9377 | YES | NO | **NO** |

Chainlink margin from strike at expiry: mean |z_cl|=0.439, min |z_cl|=0.0158
Late CL-BN momentum: mean=0.0007, std=0.1569 (WIDENING)

**M) Confidence Scoring**: **BROKEN** — constant at 0.5 across all 79,659 ticks
  The multi-signal confidence system is NOT producing differentiated scores. `dir_min_confidence` filtering is ineffective.

**N) DN Achievability**: 0.0% of time combined < $1.00
  First 25%: 0.0% achievable, 0.0% near break-even (<$1.005)
  Best combined 0-25%: $1.0228
  Best combined 25-50%: $1.0228
  Best combined 50-75%: $1.0175
  Best combined 75-100%: $1.0100

**O) Regime Stability**: score=0.9992 (61 transitions, 0.8/1k rows)
  Longest stable run: 9424 rows, mean run: 1305.9 rows

**P) Binance Staleness**: severity=MODERATE (mean=1034ms, p95=2108ms, max=4711ms)
  >1s: 45.6% | >5s: 0.0% | >10s: 0.0%

---

### `btc-updown-15m-1771685100`
- **Class**: MIXED | **Outcome**: NO | **Resolved**: True
- **Strike**: $68,526.64 | **RV**: 0.408073
- **Price**: $68,541.68 → $68,450.01
- **Rows**: 79,385 | **Z mean**: -0.7901 | **Z std**: 2.2129
- **Price above strike**: 35.0%

**A) Z-Score Trajectory**

Stats: mean=-0.7901, std=2.2129, min=-248.198, max=2.2676
Positive: 35.0% | >1.0: 4.2% | >2.0: 0.6% | <-1.0: 26.3% | <-2.0: 12.2%

| Time% | TTE(min) | Z-score | Price | Dev(bps) | YES ask | NO ask |
|-------|----------|---------|-------|----------|---------|--------|
| 0% | 14.97 | +0.1008 | 68,541.68 | +2.2 | 0.3900 | 0.6200 |
| 10% | 13.47 | -0.1523 | 68,505.09 | -3.1 | 0.2600 | 0.7500 |
| 20% | 11.98 | +0.1641 | 68,548.54 | +3.2 | 0.4400 | 0.5700 |
| 30% | 10.48 | -1.7928 | 68,471.80 | -8.0 | 0.2100 | 0.8000 |
| 40% | 8.98 | -0.0349 | 68,522.54 | -0.6 | 0.4000 | 0.6100 |
| 50% | 7.48 | +0.3985 | 68,569.32 | +6.2 | 0.6300 | 0.3800 |
| 60% | 5.98 | -0.1786 | 68,509.15 | -2.5 | 0.2400 | 0.7700 |
| 70% | 4.5 | -0.8531 | 68,454.16 | -10.6 | 0.0900 | 0.9200 |
| 80% | 3.0 | -1.0369 | 68,454.71 | -10.5 | 0.0300 | 0.9800 |
| 90% | 1.5 | -1.1625 | 68,469.62 | -8.3 | 0.0200 | 0.9900 |
| 95% | 0.75 | -1.6768 | 68,464.82 | -9.0 | 0.0100 | 1.0000 |
| 99% | 0.15 | -4.8156 | 68,447.24 | -11.6 | 0.0100 | 1.0000 |

**B) Z-Flip Analysis**: 5 raw | 5 filtered (>50 rows) | 5 sustained (>200 rows)

| # | Time% | TTE(min) | From→To | Z at flip | Run |
|---|-------|----------|---------|-----------|-----|
| 1 | 7.1% | 13.9 | YES→NO | -0.1360 | 5747 |
| 2 | 13.8% | 12.9 | NO→YES | +0.0452 | 5241 |
| 3 | 23.8% | 11.4 | YES→NO | -0.1360 | 7956 |
| 4 | 40.0% | 8.98 | NO→YES | +0.0081 | 12830 |
| 5 | 57.7% | 6.33 | YES→NO | -0.0648 | 14102 |

**C) RTDS Staleness**

Stale book %: >3bps=24.5%, >5bps=2.0%, >10bps=0.0%
Gate would block: @3bps=75.5%, @5bps=98.0%, @10bps=100.0%
Price advantage: mean=2.6678, p95=3.821
Early advantage: 2.7702 bps | Late advantage: 2.7587 bps
Binance stale >1s: 43.8% | >5s: 0.0%
Chainlink stale >1s: 99.1% | >5s: 0.5%

**D) Volatility Regime**: {'NORMAL': 72.2, 'EXTREME': 18.5, 'LOW': 5.0, 'HIGH': 4.3} | 160 transitions
First EXTREME at: 1.9% time

| Time% | TTE | From → To |
|-------|-----|-----------|
| 1.3% | 14.77m | NORMAL → HIGH |
| 1.9% | 14.68m | HIGH → EXTREME |
| 2.0% | 14.67m | EXTREME → NORMAL |
| 2.3% | 14.62m | NORMAL → EXTREME |
| 2.6% | 14.58m | EXTREME → HIGH |
| 2.9% | 14.53m | HIGH → NORMAL |
| 3.1% | 14.5m | NORMAL → HIGH |
| 3.3% | 14.48m | HIGH → NORMAL |

**E) Ask Prices (Fee-Aware)**

YES ask: mean=0.2609, min=0.0055 | Fee cost: mean=0.2641, min=0.0055
NO ask: mean=0.7514, min=0.25 | Fee cost: mean=0.7561, min=0.2522
Combined (w/fees): mean=1.0202, min=1.001 | <$1.00: 0.0% | <$0.95: 0.0%

Fee schedule: @0.30→0.3033, @0.40→0.4058, @0.45→0.4569, @0.50→0.5078

YES <0.48: 85.1% | <0.45: 79.1% | <0.42: 74.8% | <0.40: 70.4%
NO  <0.48: 8.7% | <0.45: 6.3% | <0.42: 5.1% | <0.40: 4.1%

**F) DN Entry Opportunities**

Both sides <0.48: 0 rows | <0.45: 0 | <0.42: 0
Seeding imbalance: 0.8973 (cheap=YES, bottleneck=NO)
No rows where both sides < seed_price simultaneously
Simulated one-sided abandon at: 0.0% time

**G) DIR Commit Timing**

|z|>3.0: commit at 69.3% (z=-3.5765, tte=4.6m, side=NO)
|z|>2.0: commit at 47.2% (z=2.0107, tte=7.9m, side=YES)
|z|>1.5: commit at 28.0% (z=-1.8899, tte=10.78m, side=NO)
|z|>1.0: commit at 26.4% (z=-1.1678, tte=11.02m, side=NO)
|z|>0.5: commit at 2.6% (z=0.607, tte=14.58m, side=YES)
Tiered mid: 15.1% qualified, first at 61.5% (z=-2.5563, side=NO)
Tiered late: 100.0% qualified, first at 80.1% (z=-1.0468, side=NO)

**H) Emergency Hedge Analysis**

Emergency would trigger at 47.0% (z=1.9257, side=YES)
Post-abandon z: mean=-1.2301, std=2.745, range=[-248.198, 2.2676]

**I) Late Z-Reversal**: YES→NO
Early (0-50%): z_mean=0.0292, YES=54.5%
Mid (50-80%): z_mean=-0.779, YES=25.8%
Late (80-100%): z_mean=-2.8725, YES=0.0%

**J) Price Deviation Velocity**

Window 50: mean=-0.0002, std=0.006, max=0.078, p95=0.012
Window 100: mean=-0.0002, std=0.0045, max=0.039, p95=0.0103
Window 500: mean=-0.0002, std=0.0023, max=0.0095, p95=0.0053
Acceleration: max=0.0006, p95=0.0001

**K) Chainlink Deviation Near Expiry**

last_20pct: |dev| mean=2.7577 bps, max=5.0985 bps, >5bps=0.1%, >10bps=0.0%
  Above tiered threshold: 0.0%
last_10pct: |dev| mean=2.7331 bps, max=3.686 bps, >5bps=0.0%, >10bps=0.0%
  Above tiered threshold: 0.0%
last_5pct: |dev| mean=2.4711 bps, max=3.3013 bps, >5bps=0.0%, >10bps=0.0%
  Above tiered threshold: 16.5%

**K0) Chainlink-Binance Z-Score Disagreement** (CRITICAL)

Overall disagreement: **13.3%** of ticks
  early: disagree=29.8% | z_bn=-0.1187, z_cl=-0.361 | bn_YES=49.4%, cl_YES=19.6%
  commit: disagree=7.1% | z_bn=-0.314, z_cl=-0.6423 | bn_YES=44.2%, cl_YES=37.1%
  late: disagree=0.0% | z_bn=-1.4549, z_cl=-1.9393 | bn_YES=0.0%, cl_YES=0.0%
  expiry: disagree=0.0% | z_bn=-6.7498, z_cl=-8.2345 | bn_YES=0.0%, cl_YES=0.0%

| Time% | Z Binance | Z Chainlink | BN says | CL says | Agree? |
|-------|-----------|-------------|---------|---------|--------|
| 50% | +0.3985 | +0.2446 | YES | YES | YES |
| 75% | -3.4070 | -5.0412 | NO | NO | YES |
| 90% | -1.1625 | -1.5486 | NO | NO | YES |
| 95% | -1.6768 | -2.2407 | NO | NO | YES |
| 99% | -4.8156 | -5.8030 | NO | NO | YES |

Chainlink margin from strike at expiry: mean |z_cl|=8.312, min |z_cl|=2.6331
Late CL-BN momentum: mean=-0.0012, std=0.2544 (NARROWING)

**M) Confidence Scoring**: **BROKEN** — constant at 0.5 across all 79,385 ticks
  The multi-signal confidence system is NOT producing differentiated scores. `dir_min_confidence` filtering is ineffective.

**N) DN Achievability**: 0.0% of time combined < $1.00
  First 25%: 0.0% achievable, 0.0% near break-even (<$1.005)
  Best combined 0-25%: $1.0175
  Best combined 25-50%: $1.0142
  Best combined 50-75%: $1.0101
  Best combined 75-100%: $1.0010

**O) Regime Stability**: score=0.998 (161 transitions, 2.0/1k rows)
  Longest stable run: 4446 rows, mean run: 493.1 rows

**P) Binance Staleness**: severity=LOW (mean=969ms, p95=1736ms, max=3247ms)
  >1s: 43.8% | >5s: 0.0% | >10s: 0.0%

---

### `btc-updown-15m-1771686900`
- **Class**: MIXED | **Outcome**: NO | **Resolved**: True
- **Strike**: $68,475.70 | **RV**: 0.292269
- **Price**: $68,496.40 → $68,453.60
- **Rows**: 79,261 | **Z mean**: +0.6241 | **Z std**: 1.0282
- **Price above strike**: 80.9%

**A) Z-Score Trajectory**

Stats: mean=0.6241, std=1.0282, min=-118.0135, max=2.9642
Positive: 80.9% | >1.0: 22.3% | >2.0: 7.5% | <-1.0: 3.9% | <-2.0: 1.2%

| Time% | TTE(min) | Z-score | Price | Dev(bps) | YES ask | NO ask |
|-------|----------|---------|-------|----------|---------|--------|
| 0% | 14.97 | +0.1939 | 68,496.40 | +3.0 | 0.4500 | 0.5600 |
| 10% | 13.47 | +0.1688 | 68,492.43 | +2.4 | 0.4700 | 0.5400 |
| 20% | 11.98 | +1.4997 | 68,524.71 | +7.2 | 0.5800 | 0.4300 |
| 30% | 10.48 | +1.9895 | 68,536.51 | +8.9 | 0.7200 | 0.2900 |
| 40% | 8.98 | +2.0608 | 68,534.01 | +8.5 | 0.7600 | 0.2500 |
| 50% | 7.48 | +2.3731 | 68,536.98 | +8.9 | 0.7100 | 0.3000 |
| 60% | 5.98 | +1.0610 | 68,546.82 | +10.4 | 0.8100 | 0.2000 |
| 70% | 4.5 | +0.9007 | 68,528.62 | +7.7 | 0.7700 | 0.2400 |
| 80% | 3.0 | +0.1219 | 68,481.55 | +0.8 | 0.3400 | 0.6700 |
| 90% | 1.5 | -0.5351 | 68,457.55 | -2.6 | 0.1000 | 0.9100 |
| 95% | 0.75 | -0.8288 | 68,455.82 | -2.9 | 0.0200 | 0.9900 |
| 99% | 0.15 | -2.2098 | 68,453.60 | -3.2 | 0.0100 | 1.0000 |

**B) Z-Flip Analysis**: 3 raw | 2 filtered (>50 rows) | 2 sustained (>200 rows)

| # | Time% | TTE(min) | From→To | Z at flip | Run |
|---|-------|----------|---------|-----------|-----|
| 1 | 80.3% | 2.95 | YES→NO | -0.0446 | 63703 |
| 2 | 80.8% | 2.87 | YES→NO | -0.1469 | 390 |

**C) RTDS Staleness**

Stale book %: >3bps=6.4%, >5bps=0.3%, >10bps=0.0%
Gate would block: @3bps=93.6%, @5bps=99.7%, @10bps=100.0%
Price advantage: mean=2.1716, p95=3.1379
Early advantage: 2.2643 bps | Late advantage: 1.8219 bps
Binance stale >1s: 40.6% | >5s: 0.0%
Chainlink stale >1s: 98.9% | >5s: 0.6%

**D) Volatility Regime**: {'NORMAL': 82.4, 'EXTREME': 12.1, 'LOW': 3.2, 'HIGH': 2.2} | 91 transitions
First EXTREME at: 0.0% time

| Time% | TTE | From → To |
|-------|-----|-----------|
| 0.3% | 14.92m | EXTREME → LOW |
| 0.6% | 14.88m | LOW → HIGH |
| 0.6% | 14.88m | HIGH → LOW |
| 0.8% | 14.85m | LOW → EXTREME |
| 1.3% | 14.78m | EXTREME → HIGH |
| 1.3% | 14.78m | HIGH → EXTREME |
| 2.3% | 14.63m | EXTREME → NORMAL |
| 7.0% | 13.92m | NORMAL → HIGH |

**E) Ask Prices (Fee-Aware)**

YES ask: mean=0.5557, min=0.01 | Fee cost: mean=0.5615, min=0.01
NO ask: mean=0.4561, min=0.17 | Fee cost: mean=0.4601, min=0.1708
Combined (w/fees): mean=1.0215, min=1.01 | <$1.00: 0.0% | <$0.95: 0.0%

Fee schedule: @0.30→0.3033, @0.40→0.4058, @0.45→0.4569, @0.50→0.5078

YES <0.48: 25.7% | <0.45: 21.4% | <0.42: 20.6% | <0.40: 20.6%
NO  <0.48: 61.4% | <0.45: 59.4% | <0.42: 55.9% | <0.40: 52.8%

**F) DN Entry Opportunities**

Both sides <0.48: 0 rows | <0.45: 0 | <0.42: 0
Seeding imbalance: 0.582 (cheap=NO, bottleneck=YES)
No rows where both sides < seed_price simultaneously
Simulated one-sided abandon at: 0.0% time

**G) DIR Commit Timing**

|z|>3.0: commit at 99.5% (z=-3.026, tte=0.08m, side=NO)
|z|>2.0: commit at 23.5% (z=2.073, tte=11.45m, side=YES)
|z|>1.5: commit at 14.2% (z=1.5659, tte=12.85m, side=YES)
|z|>1.0: commit at 0.8% (z=1.0238, tte=14.85m, side=YES)
|z|>0.5: commit at 0.8% (z=1.0238, tte=14.85m, side=YES)
Tiered mid: 8.7% qualified, first at 50.0% (z=2.3731, side=YES)
Tiered late: 54.6% qualified, first at 85.8% (z=-0.8527, side=NO)

**H) Emergency Hedge Analysis**

Emergency would trigger at 40.0% (z=2.0608, side=YES)
Post-abandon z: mean=0.4807, std=1.1867, range=[-118.0135, 2.8627]

**I) Late Z-Reversal**: YES→NO
Early (0-50%): z_mean=0.9122, YES=100.0%
Mid (50-80%): z_mean=1.0285, YES=100.0%
Late (80-100%): z_mean=-0.7102, YES=3.9%

**J) Price Deviation Velocity**

Window 50: mean=-0.0001, std=0.0047, max=0.0731, p95=0.0043
Window 100: mean=-0.0001, std=0.0035, max=0.0404, p95=0.0072
Window 500: mean=-0.0001, std=0.0019, max=0.0123, p95=0.004
Acceleration: max=0.0003, p95=0.0001

**K) Chainlink Deviation Near Expiry**

last_20pct: |dev| mean=1.8208 bps, max=2.6259 bps, >5bps=0.0%, >10bps=0.0%
  Above tiered threshold: 0.0%
last_10pct: |dev| mean=1.7456 bps, max=2.441 bps, >5bps=0.0%, >10bps=0.0%
  Above tiered threshold: 0.0%
last_5pct: |dev| mean=1.688 bps, max=1.8482 bps, >5bps=0.0%, >10bps=0.0%
  Above tiered threshold: 0.0%

**K0) Chainlink-Binance Z-Score Disagreement** (CRITICAL)

Overall disagreement: **3.2%** of ticks
  early: disagree=5.0% | z_bn=0.8214, z_cl=0.5627 | bn_YES=100.0%, cl_YES=95.0%
  commit: disagree=0.0% | z_bn=1.0562, z_cl=0.7804 | bn_YES=100.0%, cl_YES=100.0%
  late: disagree=7.2% | z_bn=-0.0131, z_cl=-0.3906 | bn_YES=28.9%, cl_YES=21.7%
  expiry: disagree=0.0% | z_bn=-1.6932, z_cl=-2.619 | bn_YES=0.0%, cl_YES=0.0%

| Time% | Z Binance | Z Chainlink | BN says | CL says | Agree? |
|-------|-----------|-------------|---------|---------|--------|
| 50% | +2.3731 | +2.1080 | YES | YES | YES |
| 75% | +0.9867 | +0.7092 | YES | YES | YES |
| 90% | -0.5351 | -0.9120 | NO | NO | YES |
| 95% | -0.8288 | -1.2976 | NO | NO | YES |
| 99% | -2.2098 | -3.2093 | NO | NO | YES |

Chainlink margin from strike at expiry: mean |z_cl|=2.6655, min |z_cl|=1.3152
Late CL-BN momentum: mean=-0.0027, std=0.1386 (NARROWING)

**M) Confidence Scoring**: **BROKEN** — constant at 0.5 across all 79,261 ticks
  The multi-signal confidence system is NOT producing differentiated scores. `dir_min_confidence` filtering is ineffective.

**N) DN Achievability**: 0.0% of time combined < $1.00
  First 25%: 0.0% achievable, 0.0% near break-even (<$1.005)
  Best combined 0-25%: $1.0150
  Best combined 25-50%: $1.0175
  Best combined 50-75%: $1.0146
  Best combined 75-100%: $1.0100

**O) Regime Stability**: score=0.9988 (92 transitions, 1.2/1k rows)
  Longest stable run: 11968 rows, mean run: 861.5 rows

**P) Binance Staleness**: severity=LOW (mean=926ms, p95=1585ms, max=3885ms)
  >1s: 40.6% | >5s: 0.0% | >10s: 0.0%

---

### `btc-updown-15m-1771706700`
- **Class**: DIR_FAVORABLE | **Outcome**: NO | **Resolved**: True
- **Strike**: $68,369.78 | **RV**: 0.291392
- **Price**: $68,385.29 → $68,211.20
- **Rows**: 79,355 | **Z mean**: -1.6360 | **Z std**: 11.4193
- **Price above strike**: 35.4%

**A) Z-Score Trajectory**

Stats: mean=-1.636, std=11.4193, min=-2319.4458, max=1.6847
Positive: 35.4% | >1.0: 0.0% | >2.0: 0.0% | <-1.0: 11.6% | <-2.0: 6.3%

| Time% | TTE(min) | Z-score | Price | Dev(bps) | YES ask | NO ask |
|-------|----------|---------|-------|----------|---------|--------|
| 0% | 14.97 | +0.1459 | 68,385.29 | +2.3 | 0.5700 | 0.4400 |
| 10% | 13.47 | -0.0180 | 68,367.85 | -0.3 | 0.5200 | 0.4900 |
| 20% | 11.98 | -0.2154 | 68,347.96 | -3.2 | 0.4400 | 0.5700 |
| 30% | 10.48 | -0.0406 | 68,365.93 | -0.6 | 0.4800 | 0.5300 |
| 40% | 8.98 | +0.1902 | 68,386.46 | +2.4 | 0.6000 | 0.4100 |
| 50% | 7.48 | +0.3007 | 68,393.15 | +3.4 | 0.6200 | 0.3900 |
| 60% | 5.98 | -0.0435 | 68,366.76 | -0.4 | 0.3800 | 0.6300 |
| 70% | 4.5 | -0.1788 | 68,358.90 | -1.6 | 0.3800 | 0.6300 |
| 80% | 3.0 | -0.7713 | 68,331.99 | -5.5 | 0.1100 | 0.9000 |
| 90% | 1.5 | -0.6154 | 68,348.46 | -3.1 | 0.0800 | 0.9300 |
| 95% | 0.75 | -18.4058 | 68,219.51 | -22.0 | 0.0100 | 1.0000 |
| 99% | 0.15 | -37.7768 | 68,231.85 | -20.2 | 0.0100 | 1.0000 |

**B) Z-Flip Analysis**: 5 raw | 5 filtered (>50 rows) | 5 sustained (>200 rows)

| # | Time% | TTE(min) | From→To | Z at flip | Run |
|---|-------|----------|---------|-----------|-----|
| 1 | 8.5% | 13.7 | YES→NO | -0.0141 | 6714 |
| 2 | 13.7% | 12.92 | NO→YES | +0.0152 | 4209 |
| 3 | 15.2% | 12.7 | YES→NO | -0.0188 | 1094 |
| 4 | 31.9% | 10.2 | NO→YES | +0.0336 | 13279 |
| 5 | 57.4% | 6.38 | YES→NO | -0.1919 | 20289 |

**C) RTDS Staleness**

Stale book %: >3bps=3.0%, >5bps=0.7%, >10bps=0.2%
Gate would block: @3bps=97.0%, @5bps=99.3%, @10bps=99.8%
Price advantage: mean=1.6988, p95=2.6361
Early advantage: 1.6286 bps | Late advantage: 1.7909 bps
Binance stale >1s: 41.5% | >5s: 0.0%
Chainlink stale >1s: 99.0% | >5s: 0.5%

**D) Volatility Regime**: {'NORMAL': 81.8, 'EXTREME': 15.7, 'LOW': 2.1, 'HIGH': 0.4} | 77 transitions
First EXTREME at: 5.8% time

| Time% | TTE | From → To |
|-------|-----|-----------|
| 3.3% | 14.48m | NORMAL → LOW |
| 3.8% | 14.4m | LOW → NORMAL |
| 5.8% | 14.1m | NORMAL → EXTREME |
| 6.3% | 14.02m | EXTREME → NORMAL |
| 8.5% | 13.7m | NORMAL → LOW |
| 8.5% | 13.7m | LOW → NORMAL |
| 8.7% | 13.67m | NORMAL → EXTREME |
| 9.0% | 13.62m | EXTREME → NORMAL |

**E) Ask Prices (Fee-Aware)**

YES ask: mean=0.3907, min=0.0055 | Fee cost: mean=0.3958, min=0.0055
NO ask: mean=0.6215, min=0.2 | Fee cost: mean=0.6269, min=0.2013
Combined (w/fees): mean=1.0227, min=1.001 | <$1.00: 0.0% | <$0.95: 0.0%

Fee schedule: @0.30→0.3033, @0.40→0.4058, @0.45→0.4569, @0.50→0.5078

YES <0.48: 55.7% | <0.45: 52.1% | <0.42: 49.2% | <0.40: 42.9%
NO  <0.48: 30.2% | <0.45: 27.4% | <0.42: 20.8% | <0.40: 14.9%

**F) DN Entry Opportunities**

Both sides <0.48: 0 rows | <0.45: 0 | <0.42: 0
Seeding imbalance: 0.4574 (cheap=YES, bottleneck=NO)
No rows where both sides < seed_price simultaneously
Simulated one-sided abandon at: 0.0% time

**G) DIR Commit Timing**

|z|>3.0: commit at 92.9% (z=-3.1615, tte=1.07m, side=NO)
|z|>2.0: commit at 64.4% (z=-2.1677, tte=5.33m, side=NO)
|z|>1.5: commit at 61.8% (z=-1.5568, tte=5.72m, side=NO)
|z|>1.0: commit at 57.4% (z=-1.0772, tte=6.38m, side=NO)
|z|>0.5: commit at 14.0% (z=0.6405, tte=12.88m, side=YES)
Tiered mid: 0.9% qualified, first at 64.4% (z=-2.1677, side=NO)
Tiered late: 85.7% qualified, first at 80.1% (z=-0.6129, side=NO)

**H) Emergency Hedge Analysis**

Emergency would trigger at 57.2% (z=1.6847, side=YES)
Post-abandon z: mean=-2.7182, std=14.6417, range=[-2319.4458, 1.6847]

**I) Late Z-Reversal**: YES→NO
Early (0-50%): z_mean=0.0464, YES=55.9%
Mid (50-80%): z_mean=-0.3819, YES=24.7%
Late (80-100%): z_mean=-7.7616, YES=0.0%

**J) Price Deviation Velocity**

Window 50: mean=-0.0003, std=0.0105, max=0.3634, p95=0.0063
Window 100: mean=-0.0003, std=0.0075, max=0.1817, p95=0.0094
Window 500: mean=-0.0003, std=0.0034, max=0.0376, p95=0.0059
Acceleration: max=0.0022, p95=0.0001

**K) Chainlink Deviation Near Expiry**

last_20pct: |dev| mean=1.7654 bps, max=17.5419 bps, >5bps=1.8%, >10bps=1.0%
  Above tiered threshold: 1.0%
last_10pct: |dev| mean=1.8476 bps, max=17.5419 bps, >5bps=3.7%, >10bps=2.0%
  Above tiered threshold: 3.7%
last_5pct: |dev| mean=1.5609 bps, max=9.8756 bps, >5bps=1.1%, >10bps=0.0%
  Above tiered threshold: 2.5%

**K0) Chainlink-Binance Z-Score Disagreement** (CRITICAL)

Overall disagreement: **6.2%** of ticks
  early: disagree=13.3% | z_bn=-0.0391, z_cl=-0.1931 | bn_YES=37.0%, cl_YES=24.9%
  commit: disagree=3.8% | z_bn=-0.0914, z_cl=-0.2468 | bn_YES=56.0%, cl_YES=53.6%
  late: disagree=0.0% | z_bn=-1.1691, z_cl=-1.2626 | bn_YES=0.0%, cl_YES=0.0%
  expiry: disagree=0.0% | z_bn=-27.1897, z_cl=-28.5442 | bn_YES=0.0%, cl_YES=0.0%

| Time% | Z Binance | Z Chainlink | BN says | CL says | Agree? |
|-------|-----------|-------------|---------|---------|--------|
| 50% | +0.3007 | +0.1908 | YES | YES | YES |
| 75% | -0.6550 | -0.7842 | NO | NO | YES |
| 90% | -0.6154 | -0.9623 | NO | NO | YES |
| 95% | -18.4058 | -10.1533 | NO | NO | YES |
| 99% | -37.7768 | -42.3184 | NO | NO | YES |

Chainlink margin from strike at expiry: mean |z_cl|=29.1654, min |z_cl|=6.9666
Late CL-BN momentum: mean=-0.0033, std=1.121 (NARROWING)

**M) Confidence Scoring**: **BROKEN** — constant at 0.5 across all 79,355 ticks
  The multi-signal confidence system is NOT producing differentiated scores. `dir_min_confidence` filtering is ineffective.

**N) DN Achievability**: 0.0% of time combined < $1.00
  First 25%: 0.0% achievable, 0.0% near break-even (<$1.005)
  Best combined 0-25%: $1.0232
  Best combined 25-50%: $1.0054
  Best combined 50-75%: $1.0020
  Best combined 75-100%: $1.0010

**O) Regime Stability**: score=0.999 (78 transitions, 1.0/1k rows)
  Longest stable run: 16070 rows, mean run: 1017.4 rows

**P) Binance Staleness**: severity=LOW (mean=927ms, p95=1535ms, max=2929ms)
  >1s: 41.5% | >5s: 0.0% | >10s: 0.0%

---

### `btc-updown-15m-1771713900`
- **Class**: DIR_FAVORABLE | **Outcome**: NO | **Resolved**: True
- **Strike**: $68,093.13 | **RV**: 0.1
- **Price**: $68,091.85 → $67,972.01
- **Rows**: 79,398 | **Z mean**: -1.2861 | **Z std**: 8.1869
- **Price above strike**: 68.5%

**A) Z-Score Trajectory**

Stats: mean=-1.2861, std=8.1869, min=-1596.6289, max=2.7899
Positive: 68.5% | >1.0: 15.8% | >2.0: 3.0% | <-1.0: 22.4% | <-2.0: 10.8%

| Time% | TTE(min) | Z-score | Price | Dev(bps) | YES ask | NO ask |
|-------|----------|---------|-------|----------|---------|--------|
| 0% | 14.97 | -0.0352 | 68,091.85 | -0.2 | 0.5400 | 0.4700 |
| 10% | 13.47 | +1.7127 | 68,152.15 | +8.7 | 0.7000 | 0.3100 |
| 20% | 11.98 | +0.5951 | 68,154.82 | +9.1 | 0.7100 | 0.3000 |
| 30% | 10.48 | +1.8450 | 68,149.21 | +8.2 | 0.6900 | 0.3200 |
| 40% | 8.98 | +0.2445 | 68,116.51 | +3.4 | 0.6200 | 0.3900 |
| 50% | 7.48 | +0.3026 | 68,119.54 | +3.9 | 0.6200 | 0.3900 |
| 60% | 5.98 | +0.6535 | 68,143.05 | +7.3 | 0.8000 | 0.2100 |
| 70% | 4.5 | +0.4112 | 68,101.32 | +1.2 | 0.5600 | 0.4500 |
| 80% | 3.0 | -0.5208 | 68,064.95 | -4.1 | 0.2600 | 0.7500 |
| 90% | 1.5 | -1.6699 | 68,029.24 | -9.4 | 0.0300 | 0.9800 |
| 95% | 0.75 | -16.1279 | 67,961.99 | -19.3 | 0.0100 | 1.0000 |
| 99% | 0.15 | -25.6077 | 68,000.01 | -13.7 | 0.0100 | 1.0000 |

**B) Z-Flip Analysis**: 8 raw | 8 filtered (>50 rows) | 7 sustained (>200 rows)

| # | Time% | TTE(min) | From→To | Z at flip | Run |
|---|-------|----------|---------|-----------|-----|
| 1 | 0.1% | 14.95 | NO→YES | +0.0207 | 112 |
| 2 | 45.4% | 8.18 | YES→NO | -0.0110 | 35953 |
| 3 | 48.2% | 7.75 | NO→YES | +0.1909 | 2321 |
| 4 | 66.5% | 5.02 | YES→NO | -0.1112 | 14468 |
| 5 | 66.7% | 4.98 | NO→YES | +0.0143 | 203 |
| 6 | 68.1% | 4.78 | YES→NO | -0.1047 | 992 |
| 7 | 69.1% | 4.62 | NO→YES | +0.0010 | 915 |
| 8 | 72.9% | 4.05 | YES→NO | -0.1138 | 2976 |

**C) RTDS Staleness**

Stale book %: >3bps=16.7%, >5bps=7.8%, >10bps=2.7%
Gate would block: @3bps=83.3%, @5bps=92.2%, @10bps=97.3%
Price advantage: mean=2.452, p95=8.5494
Early advantage: 1.72 bps | Late advantage: 4.7441 bps
Binance stale >1s: 59.5% | >5s: 27.6%
Chainlink stale >1s: 99.3% | >5s: 28.4%

**D) Volatility Regime**: {'NORMAL': 64.5, 'EXTREME': 28.4, 'HIGH': 4.4, 'LOW': 2.8} | 154 transitions
First EXTREME at: 0.0% time

| Time% | TTE | From → To |
|-------|-----|-----------|
| 0.9% | 14.83m | EXTREME → NORMAL |
| 4.9% | 14.23m | NORMAL → EXTREME |
| 4.9% | 14.23m | EXTREME → NORMAL |
| 5.0% | 14.22m | NORMAL → LOW |
| 5.1% | 14.2m | LOW → NORMAL |
| 5.1% | 14.2m | NORMAL → LOW |
| 5.3% | 14.17m | LOW → EXTREME |
| 5.5% | 14.15m | EXTREME → NORMAL |

**E) Ask Prices (Fee-Aware)**

YES ask: mean=0.4982, min=0.01 | Fee cost: mean=0.504, min=0.01
NO ask: mean=0.513, min=0.19 | Fee cost: mean=0.5172, min=0.1911
Combined (w/fees): mean=1.0212, min=0.9888 | <$1.00: 0.0% | <$0.95: 0.0%

Fee schedule: @0.30→0.3033, @0.40→0.4058, @0.45→0.4569, @0.50→0.5078

YES <0.48: 30.4% | <0.45: 28.3% | <0.42: 27.4% | <0.40: 27.3%
NO  <0.48: 63.9% | <0.45: 57.5% | <0.42: 48.5% | <0.40: 43.9%

**F) DN Entry Opportunities**

Both sides <0.48: 0 rows | <0.45: 0 | <0.42: 0
Seeding imbalance: 0.5246 (cheap=NO, bottleneck=YES)
No rows where both sides < seed_price simultaneously
Simulated one-sided abandon at: 0.0% time

**G) DIR Commit Timing**

|z|>3.0: commit at 76.5% (z=-3.9232, tte=3.52m, side=NO)
|z|>2.0: commit at 10.8% (z=2.0061, tte=13.35m, side=YES)
|z|>1.5: commit at 9.6% (z=1.7089, tte=13.53m, side=YES)
|z|>1.0: commit at 8.3% (z=1.2684, tte=13.73m, side=YES)
|z|>0.5: commit at 7.9% (z=0.6711, tte=13.78m, side=YES)
Tiered mid: 3.7% qualified, first at 76.4% (z=-2.2817, side=NO)
Tiered late: 100.0% qualified, first at 80.1% (z=-0.5226, side=NO)

**H) Emergency Hedge Analysis**

Emergency would trigger at 56.6% (z=1.7972, side=YES)
Post-abandon z: mean=-2.7274, std=10.3032, range=[-1596.6289, 1.9803]

**I) Late Z-Reversal**: YES→NO
Early (0-50%): z_mean=0.7284, YES=93.9%
Mid (50-80%): z_mean=0.1153, YES=71.6%
Late (80-100%): z_mean=-8.475, YES=0.0%

**J) Price Deviation Velocity**

Window 50: mean=-0.0002, std=0.0123, max=0.3796, p95=0.0152
Window 100: mean=-0.0002, std=0.0087, max=0.1898, p95=0.0134
Window 500: mean=-0.0002, std=0.0036, max=0.038, p95=0.0079
Acceleration: max=0.0028, p95=0.0002

**K) Chainlink Deviation Near Expiry**

last_20pct: |dev| mean=4.1621 bps, max=16.3753 bps, >5bps=33.3%, >10bps=3.6%
  Above tiered threshold: 3.6%
last_10pct: |dev| mean=6.1816 bps, max=16.3753 bps, >5bps=67.3%, >10bps=7.2%
  Above tiered threshold: 67.3%
last_5pct: |dev| mean=5.9487 bps, max=9.0491 bps, >5bps=71.7%, >10bps=0.0%
  Above tiered threshold: 78.4%

**K0) Chainlink-Binance Z-Score Disagreement** (CRITICAL)

Overall disagreement: **9.5%** of ticks
  early: disagree=6.7% | z_bn=0.8754, z_cl=0.6717 | bn_YES=99.6%, cl_YES=92.9%
  commit: disagree=17.8% | z_bn=0.4103, z_cl=0.168 | bn_YES=83.9%, cl_YES=81.8%
  late: disagree=0.0% | z_bn=-3.6717, z_cl=-3.0902 | bn_YES=0.0%, cl_YES=0.0%
  expiry: disagree=0.0% | z_bn=-20.5762, z_cl=-17.1768 | bn_YES=0.0%, cl_YES=0.0%

| Time% | Z Binance | Z Chainlink | BN says | CL says | Agree? |
|-------|-----------|-------------|---------|---------|--------|
| 50% | +0.3026 | +0.0387 | YES | YES | YES |
| 75% | -0.1195 | -0.3130 | NO | NO | YES |
| 90% | -1.6699 | -2.1415 | NO | NO | YES |
| 95% | -16.1279 | -10.0767 | NO | NO | YES |
| 99% | -25.6077 | -33.6076 | NO | NO | YES |

Chainlink margin from strike at expiry: mean |z_cl|=17.5785, min |z_cl|=2.8447
Late CL-BN momentum: mean=-0.009, std=1.2263 (NARROWING)

**M) Confidence Scoring**: **BROKEN** — constant at 0.5 across all 79,398 ticks
  The multi-signal confidence system is NOT producing differentiated scores. `dir_min_confidence` filtering is ineffective.

**N) DN Achievability**: 0.0% of time combined < $1.00
  First 25%: 0.0% achievable, 0.0% near break-even (<$1.005)
  Best combined 0-25%: $0.9888
  Best combined 25-50%: $1.0118
  Best combined 50-75%: $1.0156
  Best combined 75-100%: $1.0100

**O) Regime Stability**: score=0.998 (155 transitions, 2.0/1k rows)
  Longest stable run: 5289 rows, mean run: 512.2 rows

**P) Binance Staleness**: severity=CRITICAL (mean=19259ms, p95=106765ms, max=130433ms)
  >1s: 59.5% | >5s: 27.6% | >10s: 26.5%

---

## L) Cross-Market Patterns

### Pattern 1: Z-Score Directional Bias

| Slug | Z mean | Z std | YES% | Outcome | Reversal | Class |
|------|--------|-------|------|---------|----------|-------|
| `pdown-15m-1771632000` | +0.2964 | 1.8168 | 62% | NO | STABLE | MIXED |
| `pdown-15m-1771634700` | +0.5386 | 0.8547 | 87% | NO | STABLE | MIXED |
| `pdown-15m-1771685100` | -0.7901 | 2.2129 | 35% | NO | YES→NO | MIXED |
| `pdown-15m-1771686900` | +0.6241 | 1.0282 | 81% | NO | YES→NO | MIXED |
| `pdown-15m-1771706700` | -1.6360 | 11.4193 | 35% | NO | YES→NO | DIR_FAVORABLE |
| `pdown-15m-1771713900` | -1.2861 | 8.1869 | 69% | NO | YES→NO | DIR_FAVORABLE |

### Pattern 2: DN Feasibility (Fee-Adjusted)

| Slug | Combined Mean | Combined Min | <$1 % | Imbalance | Bottleneck |
|------|-------------- |------------- |-------|-----------|------------|
| `pdown-15m-1771632000` | 1.0248 | 1.0100 | 0.0% | 0.77 | NO |
| `pdown-15m-1771634700` | 1.0262 | 1.0100 | 0.0% | 0.28 | NO |
| `pdown-15m-1771685100` | 1.0202 | 1.0010 | 0.0% | 0.90 | NO |
| `pdown-15m-1771686900` | 1.0215 | 1.0100 | 0.0% | 0.58 | YES |
| `pdown-15m-1771706700` | 1.0227 | 1.0010 | 0.0% | 0.46 | NO |
| `pdown-15m-1771713900` | 1.0212 | 0.9888 | 0.0% | 0.52 | YES |

### Pattern 3: RTDS Stale Book Availability

| Slug | Stale>3bps | Stale>5bps | Stale>10bps | Gate Block @3 | Gate Block @10 |
|------|-----------|-----------|------------|---------------|----------------|
| `pdown-15m-1771632000` | 95.7% | 5.6% | 0.0% | 4.3% | 100.0% |
| `pdown-15m-1771634700` | 92.3% | 1.6% | 0.0% | 7.7% | 100.0% |
| `pdown-15m-1771685100` | 24.5% | 2.0% | 0.0% | 75.5% | 100.0% |
| `pdown-15m-1771686900` | 6.4% | 0.3% | 0.0% | 93.6% | 100.0% |
| `pdown-15m-1771706700` | 3.0% | 0.7% | 0.2% | 97.0% | 99.8% |
| `pdown-15m-1771713900` | 16.7% | 7.8% | 2.7% | 83.3% | 97.3% |

### Pattern 4: Volatility Regime Transitions

| Slug | NORMAL% | EXTREME% | Transitions | First EXTREME |
|------|---------|----------|-------------|---------------|
| `pdown-15m-1771632000` | 77.1% | 19.9% | 75 | 0.8% |
| `pdown-15m-1771634700` | 83.7% | 12.4% | 60 | 0.9% |
| `pdown-15m-1771685100` | 72.2% | 18.5% | 160 | 1.9% |
| `pdown-15m-1771686900` | 82.4% | 12.1% | 91 | never% |
| `pdown-15m-1771706700` | 81.8% | 15.7% | 77 | 5.8% |
| `pdown-15m-1771713900` | 64.5% | 28.4% | 154 | never% |

### Pattern 5: Z-Flip Severity

| Slug | Raw Flips | Sustained | Z std | DIR Would Commit | Correct? |
|------|-----------|-----------|-------|------------------|----------|
| `pdown-15m-1771632000` | 12 | 12 | 1.8168 | z>2.0 at 71.6% (YES) | ✗ |
| `pdown-15m-1771634700` | 6 | 6 | 0.8547 | z>2.0 at 66.3% (YES) | ✗ |
| `pdown-15m-1771685100` | 5 | 5 | 2.2129 | z>2.0 at 47.2% (YES) | ✓ |
| `pdown-15m-1771686900` | 3 | 2 | 1.0282 | z>2.0 at 23.5% (YES) | ✗ |
| `pdown-15m-1771706700` | 5 | 5 | 11.4193 | z>2.0 at 64.4% (NO) | ✓ |
| `pdown-15m-1771713900` | 8 | 7 | 8.1869 | z>2.0 at 10.8% (YES) | ✗ |

### Pattern 6: Chainlink Arb Near Expiry

| Slug | Last 20% |dev|>10bps | Last 10% |dev|>5bps | Last 5% |dev|>3bps |
|------|----------------------|---------------------|---------------------|
| `pdown-15m-1771632000` | 0.0% | 3.6% | 99.5% |
| `pdown-15m-1771634700` | 0.0% | 0.0% | 91.2% |
| `pdown-15m-1771685100` | 0.0% | 0.0% | 16.5% |
| `pdown-15m-1771686900` | 0.0% | 0.0% | 0.0% |
| `pdown-15m-1771706700` | 1.0% | 3.7% | 2.5% |
| `pdown-15m-1771713900` | 3.6% | 67.3% | 78.4% |

### Pattern 7: Emergency Hedge Trigger Analysis

Markets where emergency hedge would trigger: 6/6

| Slug | Trigger? | Trigger Time% | Z at Trigger | Side | Outcome Match? |
|------|----------|---------------|------------- |------|----------------|
| `pdown-15m-1771632000` | YES | 58.0% | -1.5573 | NO | ✓ |
| `pdown-15m-1771634700` | YES | 47.6% | +1.5649 | YES | ✗ |
| `pdown-15m-1771685100` | YES | 47.0% | +1.9257 | YES | ✗ |
| `pdown-15m-1771686900` | YES | 40.0% | +2.0608 | YES | ✗ |
| `pdown-15m-1771706700` | YES | 57.2% | +1.6847 | YES | ✗ |
| `pdown-15m-1771713900` | YES | 56.6% | +1.7972 | YES | ✗ |

### Pattern 8: Chainlink-Binance Z-Score Disagreement (CRITICAL)

Polymarket resolves on Chainlink. Backtest uses Binance z-score. Disagreement = DIR commits wrong side.

| Slug | Overall Disagree | Commit Window | Late (75-95%) | Expiry (95-100%) | Outcome |
|------|-----------------|---------------|---------------|------------------|---------|
| `pdown-15m-1771632000` | 38.0% | 25.7% | 40.5% | 25.1% | NO |
| `pdown-15m-1771634700` | 49.2% | 46.4% | 49.0% | 67.8% | NO |
| `pdown-15m-1771685100` | 13.3% | 7.1% | 0.0% | 0.0% | NO |
| `pdown-15m-1771686900` | 3.2% | 0.0% | 7.2% | 0.0% | NO |
| `pdown-15m-1771706700` | 6.2% | 3.8% | 0.0% | 0.0% | NO |
| `pdown-15m-1771713900` | 9.5% | 17.8% | 0.0% | 0.0% | NO |

### Pattern 9: Confidence Scoring Health

**WARNING**: 6/6 markets have CONSTANT trade_confidence. The multi-signal system is broken.
  `dir_min_confidence` filtering cannot work with constant scores.

### Pattern 10: DN Achievability

| Slug | Achievable % | First 25% | Near Break-Even | Best Combined |
|------|-------------|-----------|-----------------|---------------|
| `pdown-15m-1771632000` | 0.0% | 0.0% | 0.0% | $1.0100 |
| `pdown-15m-1771634700` | 0.0% | 0.0% | 0.0% | $1.0100 |
| `pdown-15m-1771685100` | 0.0% | 0.0% | 0.0% | $1.0010 |
| `pdown-15m-1771686900` | 0.0% | 0.0% | 0.0% | $1.0100 |
| `pdown-15m-1771706700` | 0.0% | 0.0% | 0.0% | $1.0010 |
| `pdown-15m-1771713900` | 0.0% | 0.0% | 0.0% | $0.9888 |

### Pattern 11: Data Quality (Regime Stability + Binance Staleness)

| Slug | Stability | Transitions | Mean Stale (ms) | >5s % | Severity |
|------|-----------|-------------|----------------|-------|----------|
| `pdown-15m-1771632000` | 0.999 | 76 | 887 | 0.0% | LOW |
| `pdown-15m-1771634700` | 0.9992 | 61 | 1034 | 0.0% | MODERATE |
| `pdown-15m-1771685100` | 0.998 | 161 | 969 | 0.0% | LOW |
| `pdown-15m-1771686900` | 0.9988 | 92 | 926 | 0.0% | LOW |
| `pdown-15m-1771706700` | 0.999 | 78 | 927 | 0.0% | LOW |
| `pdown-15m-1771713900` | 0.998 | 155 | 19259 | 27.6% | CRITICAL |

## Parameter Tuning Suggestions

> Based on observed patterns across all analyzed markets.
> Reference: BACKTEST_TUNING_GUIDE.md parameter tiers.

### 1. DN Combined Average Threshold (`dn_max_combined`)
**Current default**: 0.95
**Observed**: avg combined (w/fees) = 1.0228, min = 0.9888
**DN profitable (<$1) time**: 0.0% avg across markets
**Suggestion**: Combined avg consistently >$1.00 with fees. DN opportunities are rare. Consider:
  - Tightening `dn_max_combined` from 0.95 to 0.93 to only enter high-quality DN windows
  - OR relaxing seed prices to capture more entry windows

### 2. One-Sided Cost Cap (`dn_max_one_sided_cost`)
**Current default**: $20.0
**Observed**: avg seeding imbalance = 0.59 (1/6 markets >0.8 imbalance)
**Simulated abandon**: triggers at 0% time avg (earliest: 0%)
**Suggestion**: Imbalance is moderate. Current cap $20.0 is appropriate.

### 3. Stale Gate Threshold (`stale_gate_threshold_bps`)
**Current default**: 3.0 bps
**Observed stale %**: >3bps avg=39.8%, >10bps avg=0.5%
**Gate would block**: @3bps=60.2%, @10bps=99.5%
**Suggestion**: Gate at 10bps blocks >80% of trades — too strict.
  - Lower to 3.0 bps (current) or try 5.0 bps as compromise

### 4. DIR Z-Flip Management (`dir_max_postcommit_flips`)
**Current default**: 5
**Observed**: avg sustained flips = 6.2, 5/6 markets with 3+ sustained flips
**Suggestion**: High flip activity. Consider:
  - Lowering `dir_max_postcommit_flips` from 5 to 3
  - Increasing `dir_flip_hedge_ratio` from 0.25 to 0.50 for stronger hedging

### 5. Late Reversal Protection
**Observed**: 4/6 markets show z-reversal (early vs late direction change)
**Suggestion**: Significant reversal rate. Consider:
  - Raising `dir_no_trade_before_pct` from 0.35 to 0.50 — wait longer before committing
  - Raising `dir_min_z_early` from 3.0 to 4.0 — only early-commit on very strong signal

### 6. DIR Direction Accuracy
**Observed**: 2/6 (33%) markets where z-score majority side matches outcome
**Suggestion**: DIR accuracy below 50% — worse than random. Consider:
  - Raising `dir_min_confidence` from 0.7 to 0.80
  - Raising `dir_min_z_mid` from 2.0 to 2.5 — stricter mid-window entry
  - Consider DN-only mode (disable DIR) until accuracy improves

### 7. Fee-Aware Seed Pricing
**Current**: dn_seed_price=0.48, dn_early_seed_price=0.45
**Fee impact**: @0.48→0.4875, @0.45→0.4569, @0.42→0.4262
**DN break-even**: needs combined (w/fees) < $1.00
  If both sides at seed_price 0.48: combined = 0.9750
  If both sides at early_seed_price 0.45: combined = 0.9138

### 8. Emergency Hedge Configuration
**Current**: dir_emergency_min_z=1.5
**Observed**: Emergency would trigger in 6/6 markets
  Correct side: 1/6 (17%)
**Suggestion**: Emergency hedge predicts wrong side >50%. Consider:
  - Raising `dir_emergency_min_z` from 1.5 to 2.0

### 9. Volatility Regime Gating
**Observed**: 1/6 markets with >20% time in EXTREME regime
  DIR accuracy in EXTREME markets: 0/1
**Suggestion**: Consider strengthening `dn_regime_gate` to skip markets where EXTREME appears before 50% time

### 10. Chainlink Resolution Arb (Exploit 4)
**Observed**: 5/6 markets with Chainlink arb opportunity (>3bps dev in last 5%)
  Avg |chainlink_dev| in last 5%: 3.56 bps
**Suggestion**: Chainlink arb opportunities exist. Ensure `chainlink_arb_enabled=True`.

### 11. NEW: Chainlink-Binance Agreement Gate (CRITICAL)
**Observed**: avg Chainlink-Binance disagreement = 19.9%
  1/6 markets with >30% disagreement near expiry
**Suggestion**: Implement `dir_require_chainlink_agreement`:
  - Only commit DIR when z_binance and z_chainlink agree on direction
  - NEW param: `dir_min_chainlink_margin_bps = 3.0` — don't commit when Chainlink within 3bps of strike
  - NEW param: `dir_late_exit_threshold = 0.85` — force hedge if z_chainlink disagrees at 85%+ time

### 12. Confidence Scoring Column (NOT a bug — logger placeholder)
**Observed**: 6/6 markets with CONSTANT trade_confidence = 0.5
**Root cause**: The CSV `trade_confidence` column is a **logger placeholder** in `src/api/binance.rs:753`:
  `trade_confidence: if realized_vol > 0.5 { 0.8 } else { 0.5 }`
  Since all Feb 21 markets have realized_vol < 0.5, every tick gets 0.5.
**Impact**: NONE on backtest. The backtest computes its own confidence internally via
  `compute_directional_confidence()` (4/5 signal voting system) — it does NOT use this CSV column.
  The `dir_min_confidence=0.70` gate operates on the backtest's internal calculation, not this field.
**Action**: No fix needed. This column is informational only and can be ignored for analysis.

### 13. NEW: DN Early Skip Threshold
**Observed**: avg DN achievable in first 25%: 0.0%
  Near break-even (<$1.005) in first 25%: 0.0%
  Markets where DN is NEVER achievable in first 25%: 6/6
**Suggestion**: DN almost never reaches break-even early. Implement:
  - NEW: `dn_skip_threshold` — skip DN if combined_ask never < $1.005 in first 25% of market
  - This prevents accumulating one-sided cost on structurally unprofitable markets

### 14. Stale Gate Recalibration
**Observed**: mean |rtds_binance_dev| = 0.11 bps (negligible)
  mean |rtds_chainlink_dev| = 2.50 bps (real signal)
**Suggestion**: Binance deviation is negligible — stale gate based on binance_dev almost never triggers.
  - Recalibrate stale_gate to use `chainlink_dev` instead of `binance_dev`
  - Chainlink deviation has actual signal content

### 15. NEW: Binance Staleness Gate for DIR
**Observed**: avg Binance stale mean = 4000ms
  1/6 markets with >10% of ticks stale >5s
**Suggestion**: Chronic Binance staleness degrades z-score reliability.
  - NEW: `max_binance_stale_ms_for_commit = 5000` — don't commit DIR when Binance data is >5s stale
  - Scale `dir_max_shares` inversely with Chainlink margin at commit time

### 16. Late Reversal Check Timing
**Current**: `dir_late_reversal_pct = 0.60` (checks at 60% time)
**Problem**: Actual reversals happen at 75-90% based on observed data
**Observed**: 4/6 markets reverse direction late
**Suggestion**: Raise `dir_late_reversal_pct` from 0.60 to 0.75
  - Or implement continuous monitoring instead of single check

### Parameter Summary Table

| Parameter | Current | Suggested | Tier | Rationale |
|-----------|---------|-----------|------|-----------|
| `dn_max_combined` | 0.95 | **0.93** | T1 | Combined avg >$1 with fees |
| `stale_gate_threshold_bps` | 3.0 | **5.0** | T2 | 10bps gate too restrictive |
| `dir_max_postcommit_flips` | 5 | **3** | T2 | High flip count |
| `dir_flip_hedge_ratio` | 0.25 | **0.50** | T2 | Needs stronger hedge |
| `dir_no_trade_before_pct` | 0.35 | **0.50** | T2 | Frequent late reversals |
| `dir_min_confidence` | 0.7 | **0.80** | T1 | DIR accuracy 33% |
| `dir_emergency_min_z` | 1.5 | **2.0** | T1 | Emergency hedge wrong-sided |
| `dir_require_chainlink_agreement` | N/A (NEW) | **True** | T1 | CL-BN disagree 20% |
| `dir_min_chainlink_margin_bps` | N/A (NEW) | **3.0** | T1 | Prevent commit near CL strike |
| `dir_late_exit_threshold` | N/A (NEW) | **0.85** | T1 | Force hedge on CL disagree late |
| `trade_confidence` | constant=0.5 | No action | — | Logger placeholder, backtest uses internal confidence |
| `dn_skip_threshold` | N/A (NEW) | **1.005** | T2 | Skip DN if never near break-even |
| `stale_gate_source` | binance_dev | **chainlink_dev** | T2 | BN dev negligible, CL has signal |
| `max_binance_stale_ms_for_commit` | N/A (NEW) | **5000** | T2 | Chronic BN staleness |
| `dir_late_reversal_pct` | 0.60 | **0.75** | T2 | Reversals happen at 75-90% |
