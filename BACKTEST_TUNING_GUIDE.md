# Backtest Tuning Guide — zv68 with Corrected Outcome Data

> **Last updated**: 2026-02-22
> **Backtest version**: zv68 (v6.7 — Exact Combinatorics + Conformal Prediction)
> **Data pipeline**: fix_and_slugify.py → backtest-zv68.py

---

## Honest Assessment Up Front

This guide reflects reality, not optimism:

- **22+ backtest versions** have been iterated through. Many A/B tests failed.
- **Data before Feb 18 is unusable** — no RTDS columns (`rtds_latency`, `rtds_staleness`, `rtds_binance_dev_bps`, etc.). Only `218data/` through `221data/` works with zv68.
- **The outcome bug inflated results**. The empty-outcome fallback (`min(yes_shares, no_shares)`) gave DN "profits" on markets with no real resolution. With corrected outcomes from `fix_and_slugify.py`, **DIR showed negative ROI** — that's real performance, not a bug.
- **You need more data**. Four days (Feb 18-21) is a tiny sample. Continue collecting through Feb 25 minimum before trusting any sweep result.
- **DN is the safer book**. If DIR stays negative after more data and tuning, the fallback is DN-only mode. That's a valid strategy.

---

## Step 1: Data Preparation

### What fix_and_slugify.py does

Located at: `/home/lehash/development/combine-fetcher/fix_and_slugify.py`

The script solves a critical problem: hourly CSV logs split the same market slug across multiple files. It:

1. **Concatenates** all hourly `.csv` files from input directories
2. **Groups** by `market_slug` (reunites rows split across files)
3. **Filters out** incomplete slugs:
   - Fewer than 10,000 rows → skip (incomplete data)
   - Never reached near-expiry (`time_to_expiry_minutes > 2.0`) → skip (market didn't finish)
4. **Preserves** all original rows untouched
5. **Appends** one synthetic resolution row per slug:
   - Copy of last row with `market_resolved=True`
   - `outcome` set to API-fetched result from Gamma API (`YES`/`NO`)
6. **Logs wrong outcomes** found in source data vs API truth

### Run it

```bash
cd /home/lehash/development/combine-fetcher

python3 fix_and_slugify.py ./218data/ ./219data/ ./220data/ ./221data/ -o ./all_slugified
```

### What to expect

- API calls to `gamma-api.polymarket.com` for each slug (rate-limited at 0.2s)
- Output: one `<slug>.csv` per market in `./all_slugified/`
- Summary: how many slugs output, how many skipped, outcome cache stats
- `[FIX]` warnings if source data had wrong outcomes (these are corrected in the resolution row)

### Important: the resolution row

The backtest reads outcome from the **last row** of each slug's data:

```python
# backtest-zv68.py line 2314
outcome = market_df.iloc[-1].get('outcome', '')
```

If outcome is empty/missing, the fallback kicks in:

```python
# line 2325 — the problematic fallback
payout = float(min(total_yes, total_no)) if total_yes > 0 and total_no > 0 else 0
```

This `min(yes, no)` fallback **artificially inflates DN results** because it always pays out as if the DN position were balanced. With `fix_and_slugify.py`, every slug gets a proper `YES`/`NO` outcome, so the fallback never triggers on valid data.

---

## Step 2: Baseline Run

### Command

```bash
cd /home/lehash/development/poly-mock-trading/src/data

python3 backtest-zv68.py --dir /home/lehash/development/combine-fetcher/all_slugified/ --no-monte-carlo
```

### Flags

| Flag | Purpose |
|------|---------|
| `--dir DIR` | Directory of per-slug CSVs |
| `--file FILE` | Single CSV (for debugging one market) |
| `--no-monte-carlo` | Skip Monte Carlo modules (faster, ~60% less runtime) |
| `--sweep` | Run parameter sweep |
| `--capital AMOUNT` | Production capital scaling (e.g., 200, 500, 1000 USDC) |
| `--sample N` | Random sample of N markets (for quick iteration) |
| `--output FILE` | Export results to Excel |

### What the baseline report shows

The report has 8 sections:

1. **Market Regime Distribution** — oscillating vs trending breakdown
2. **Multi-Leg Delta Neutral Opportunity** — how many markets have combined_avg < $1.00
3. **Same-Tick vs Multi-Leg Arb** — how multi-leg unlocks more markets
4. **Latency x Multi-Leg Interaction** — impact of Binance-Poly latency
5. **Fee Impact by Price Level** — fee formula: `25.0 * (p * (1-p))^2`
6. **Deviation x Volatility x Time → Directional Accuracy** — z-score accuracy tables
7. **Hybrid Backtest** — the main results: DN + DIR book P&L
8. **Parameter Sweep** (only with `--sweep`)

### Key metrics to check in baseline

- **Total profit** and **ROI%** (combined)
- **DN profit** vs **DIR profit** (separated)
- **DN EW count** — how many markets achieved "profitable either way" (combined_avg < $1.00)
- **Direction accuracy** — % of DIR commits that predicted the correct outcome
- **Win rate** — % of markets with positive profit
- **Exploit diagnostics** — which exploits are firing, which are gated

### Quick iteration tip

Use `--sample 5` for fast iteration during manual param changes:

```bash
python3 backtest-zv68.py --dir /path/to/csvs/ --sample 5 --no-monte-carlo
```

---

## Step 3: Focused Sweep (~1,000 Combos)

### What it is

`create_focused_sweep()` generates ~1,000 parameter combinations with **locked exploit config** (the exploit settings that tested best) and varies critical trading parameters. This is the recommended starting sweep.

### Run it

```bash
python3 backtest-zv68.py --dir /path/to/csvs/ --sweep --no-monte-carlo
```

Note: `--sweep` currently calls `parameter_sweep()` (81K combos) by default. To use `create_focused_sweep()` instead, you'll need to modify the `generate_report()` function to call it. Check which sweep function is wired up in the code first.

### The 7 phases of create_focused_sweep()

| Phase | What it varies | Combos | Purpose |
|-------|---------------|--------|---------|
| 1. DN Emergency Hedge | `dir_emergency_min_z` (0.5-1.2), `dir_emergency_max_cost` (50-100), `dn_max_one_sided_cost` (15-25) | 36 | How aggressively to hedge abandoned DN positions |
| 2. DN Seeding | `dn_seed_price` (0.40-0.55), `dn_early_seed_price` (0.40-0.50), `dn_early_window_pct` (0.15-0.30) | 36 | Early positioning for DN book |
| 3. DIR Z-Flip | `dir_max_postcommit_flips` (3-12), `dir_flip_hedge_ratio` (0.1-1.0), `dir_flip_hedge_max_cost` (30-70) | 48 | Managing z-score direction changes after DIR commits |
| 4. Position Sizing | `dn_trade_size` (15-30), `dir_trade_size` (20-40), `edge_scale_min/max` | 81 | How big each trade is |
| 5. Combined DN+DIR | `dn_max_one_sided_cost`, `dir_min_confidence`, `dir_emergency_min_z`, `dir_emergency_max_cost` | 36 | Joint tuning of both books |
| 6. DIR-Only Fallback | DN disabled, vary DIR cost/confidence | 12 | What if DN is the problem? |
| 7. Balanced High-ROI | Multi-dimensional: emergency+flip, seed+emergency, size+emergency, comprehensive, edge-case, z-flip management, seeding+sizing | ~750 | Broad search of promising regions |

### Locked exploit config in focused sweep

These are **fixed** across all 1,000 combos (proven best settings):

```
exploit3 (Stale-Only Gate): ON, threshold=10.0 bps (ultra-strict)
exploit1 (Price Sniping): OFF (disabled — hurts bad markets)
exploit2 (Edge-Scaled Sizing): ON, reference=5.0 bps, min=20, max=80
exploit4 (Chainlink Resolution Arb): ON, multiplier=1.5x, max_cost=60
```

### Recommended tuning sequence

1. **Focused sweep first** (~1,000 combos, 1-2 hours on CPU) — identifies the winning parameter quadrant
2. **GPU sweep on best quadrant** (81,000 combos, 20-30 min on RTX 3090) — finds exact sweet spots within that region
3. **Test DIR-only** (Phase 6 in focused sweep) — isolate whether DN or DIR is the bottleneck
4. **Validate on held-out data** — run best config on newest day's data that wasn't in the sweep

### Chainlink Arb tiered thresholds (Exploit 4)

Exploit 4 uses tiered Chainlink deviation thresholds by time remaining:

| Time Elapsed | Min Chainlink Dev | Rationale |
|--------------|-------------------|-----------|
| 80% | 10 bps | Conservative — still 20% of window remaining |
| 90% | 5 bps | Moderate — resolution approaching |
| 95% | 3 bps | Aggressive — very close to expiry, small dev is decisive |

This is the only exploit with **isolated P&L** — it creates its own trades separate from DN/DIR, so its profit is directly attributable.

---

## Step 4: Parameter Priority Tiers

### Tier 1 — High Impact (tune these first)

These parameters have the most influence on overall P&L:

| Parameter | Default | Book | What it controls |
|-----------|---------|------|-----------------|
| `dn_max_one_sided_cost` | 20.0 | DN | Max $ spent on one side before abandoning — **single biggest loss limiter** |
| `dn_max_combined` | 0.95 | DN | Max combined_avg to accept. Lower = more profitable but fewer markets qualify |
| `dir_emergency_min_z` | 1.5 | DIR | Min z-score to trigger emergency hedge on abandoned DN. Lower = more triggers |
| `dir_emergency_max_cost` | 30.0 | DIR | Budget for emergency hedges. Higher = more protection but more capital at risk |
| `dir_min_confidence` | 0.70 | DIR | Multi-signal confidence threshold (4/5 independent signals) |
| `dn_abandon_pct` | 0.40 | DN | Time fraction at which one-sided DN positions are abandoned |

### Tier 2 — Medium Impact (tune after Tier 1 stabilizes)

| Parameter | Default | Book | What it controls |
|-----------|---------|------|-----------------|
| `dn_seed_price` | 0.48 | DN | Max price to pay during seeding phase |
| `dn_early_seed_price` | 0.45 | DN | Max price in early window (first 20%) — higher = more aggressive positioning |
| `dir_max_postcommit_flips` | 5 | DIR | Z-flips tolerated after commit before freezing |
| `dir_flip_hedge_ratio` | 0.25 | DIR | Size of hedge buys on z-flip (25% of committed size) |
| `dir_flip_hedge_max_cost` | 40.0 | DIR | Total budget for DIR flip-hedging |
| `dn_trade_size` | 40 | DN | Shares per DN trade |
| `dir_trade_size` | 30 | DIR | Shares per DIR trade |
| `stale_gate_threshold_bps` | 3.0 | Both | RTDS deviation threshold for "book is stale" — lower = stricter |
| `edge_scale_reference_bps` | 5.0 | Both | Baseline BPS for 1x edge-scaled sizing |

### Tier 2.5 — Hedge Mechanics (tune if DN abandonment is frequent)

| Parameter | Default | Book | What it controls |
|-----------|---------|------|-----------------|
| `enable_dir_emergency_hedge` | True | Both | Recover capital from DN abandon via DIR |
| `enable_dn_emergency_hedge` | True | DN | Aggressive pair completion before DN abandons |
| `dn_emergency_price_bonus` | 0.10 | DN | Extra price tolerance for emergency pair completion |
| `dn_flip_hedge_enabled` | True | DN | Buy opposite side when z flips during DN accumulation |
| `dir_flip_hedge_enabled` | True | DIR | Hedge instead of freeze on post-commit z-flips |

### Tier 3 — Low Impact / Safety (rarely need to change)

| Parameter | Default | Book | Notes |
|-----------|---------|------|-------|
| `dn_cheap_threshold` | 0.42 | DN | Already well-tuned |
| `dn_max_imbalance` | 0.65 | DN | Balance constraint — too strict kills volume |
| `max_exposure` | 200.0 | Both | Hard cap per market — match to capital budget |
| `dir_min_z_early/mid/late` | 3.0/2.0/0.5 | DIR | Z thresholds by time period — already researched |
| `dir_max_price` | 0.65 | DIR | Never pay more than 65c for DIR |
| `dn_flip_hedge_ratio` | 1.0 | DN | 1:1 ratio on DN z-flip hedging — match committed size |
| `dir_require_z_flip` | True | DIR | Critical safety feature — don't disable |
| `dn_counter_trend` | True | DN | Buys against z-score for cheaper fills — keep on |
| `dn_regime_gate` | True | DN | Prevents starting DN in extreme regimes — keep on |

### Tier 4 — Exploit Config (already locked by focused sweep)

| Parameter | Default | Notes |
|-----------|---------|-------|
| `exploit3_dn_enabled/dir_enabled` | True/True | Stale gate — proven winner |
| `snipe_enabled_dn/dir` | True/True | Price sniping — focused sweep disables this |
| `edge_scale_enabled` | True | Edge-scaled sizing — keep on |
| `chainlink_arb_enabled` | True | Resolution arb — the only exploit with isolated P&L |

### Parameters to NOT touch

- `dn_conditional_seed`: **False** — was tested, causes DN regression
- `dn_early_dev_gate`: **False** — was tested, causes DN regression
- `rtds_dir_require_stale`: **False** — too restrictive, blocks legitimate DIR trades

---

## Step 5: Interpreting Sweep Results

### Sweep output format

The sweep runs each parameter config across all markets and reports:

```
ROI:  44.6% WR:  82% EW: 14/18 | DN:$  142 DIR:$  -23 | osc<$15 comb<0.95 | E3=10bps E2=5ref E4=1.5x
```

| Field | Meaning |
|-------|---------|
| `ROI` | Total return on investment (profit / cost × 100) |
| `WR` | Win rate — % of markets with positive combined profit |
| `EW` | "Either Way" — DN markets where combined_avg < $1.00 (guaranteed profit) |
| `DN:$` | Total DN book profit across all markets |
| `DIR:$` | Total DIR book profit across all markets |
| `osc<$X` | `dn_max_one_sided_cost` value |
| `comb<X` | `dn_max_combined` value |
| `E3=Xbps` | Stale gate threshold |
| `E2=Xref` | Edge scale reference BPS |
| `E4=Xx` | Chainlink arb size multiplier |

### What to look for

1. **DN profit should be positive and stable** across top configs. If DN is negative, something is fundamentally wrong with the data or parameters.

2. **DIR profit will likely be negative** with corrected outcomes. This is expected. Look for configs where DIR loss is minimized (least negative), not where it's magically positive.

3. **EW count matters more than ROI** for DN reliability. High EW = more markets where DN achieved guaranteed profit.

4. **Watch for overfit**: If the #1 config has 60% ROI but configs #2-#10 have 10-15%, the #1 is likely overfit to a specific market. Look for **clusters** of similar ROI in the top 10.

5. **The saved JSON**: Top 50 configs are saved to `best_parameters_v6.json` with the parameters that differed. Use this to identify which parameter ranges consistently appear in top results.

### Red flags in sweep results

- Top config ROI >> average top-10 ROI → overfit
- DN profit negative in most configs → data issue or outcomes still wrong
- All top configs disable DN (`dn_max_one_sided_cost=0`) → DN book broken
- Very few markets traded → parameters too restrictive
- DIR showing positive ROI on 4 days of data → likely overfit (needs more data to validate)

---

## Step 6: DN-Only Fallback

If DIR consistently shows negative ROI after tuning, shift to DN-only mode.

### How to test DN-only

The focused sweep already includes Phase 6 (DIR-Only Fallback) with 12 combos. But for DN-only, set in the sweep or manually:

```python
'dir_enabled': False,      # Disable DIR entirely
'dir_max_cost': 0.0,       # Zero DIR budget
```

Or equivalently, focus on configs where:
- DN profit > 0
- DIR trades = 0 or DIR cost = 0

### DN-only viability

DN book makes money when:
1. Both YES and NO sides get bought cheap enough that `combined_avg < $1.00`
2. The market resolves (either side pays $1.00 per winning share)
3. Profit = payout - total_cost (guaranteed positive when combined_avg < $1.00)

The four exploits still apply in DN-only mode:
- **Stale Gate** (Exploit 3): Only buy when book is stale (better fills)
- **Edge-Scaled Sizing** (Exploit 2): Buy more when the edge is larger
- **Price Sniping** (Exploit 1): Only buy when ask < theoretical fair price
- **Chainlink Arb** (Exploit 4): Still works near expiry for last-minute profits

### DN-only checklist

- [ ] Run baseline with `dir_enabled=False`
- [ ] Compare DN profit vs DN+DIR combined profit
- [ ] Check if DN EW count drops (it shouldn't — DIR doesn't affect DN)
- [ ] Verify `dn_max_one_sided_cost` is set low enough to limit losses on abandoned positions

---

## Step 7: Production Readiness Checklist

### Before going live

- [ ] **Data**: At least 7 days of RTDS-enabled data (Feb 18+). Current: ~4 days. **Not enough yet.**
- [ ] **Sweep stability**: Top-5 configs show consistent ROI (within 10% of each other)
- [ ] **DN EW rate**: >50% of markets achieve either-way profitability
- [ ] **DIR accuracy**: If DIR enabled, direction accuracy >70%
- [ ] **No overfit signals**: Performance doesn't collapse with `--sample` subsets
- [ ] **Monte Carlo passed**: Run with `--monte-carlo` (remove `--no-monte-carlo`):
  - Bankroll survival >95%
  - Ruin probability <5%
  - 5th-percentile terminal bankroll > 0
- [ ] **Capital scaling**: Test with `--capital 200` to match real deployment budget
- [ ] **Fee model verified**: Fee formula `25.0 * (p * (1-p))^2` matches Polymarket's actual fees
- [ ] **WebSocket lifecycle**: Token eviction implemented (see `WS_LIFECYCLE_ANALYSIS.md`) — stale entries won't accumulate
- [ ] **RTDS operational**: All 6 WebSocket streams connected and healthy

### Production parameters to set via `--capital`

```bash
# Recommended starting point
python3 backtest-zv68.py --dir ./all_slugified/ --capital 200

# This auto-scales:
# - dn_trade_size, dir_trade_size
# - max_exposure
# - dn_max_one_sided_cost
# - dir_max_cost, dir_emergency_max_cost
# - chainlink_arb_max_cost
```

### Risk controls for production

The two-book system shares a $200/market exposure budget:
- `max_exposure`: 200.0 (hard cap per market)
- `dn_initial_capital`: 1000.0 (cross-market DN cap)
- `dir_initial_capital`: 1000.0 (cross-market DIR cap)

---

## Step 8: Timeline and Data Requirements

### Current state (Feb 22)

| Data Dir | Date | Usable? | Notes |
|----------|------|---------|-------|
| Pre-218 | Before Feb 18 | **NO** | No RTDS columns — crashes zv68 |
| `218data/` | Feb 18 | Yes | First day with RTDS |
| `219data/` | Feb 19 | Yes | |
| `220data/` | Feb 20 | Yes | |
| `221data/` | Feb 21 | Yes | |

### Minimum requirements

- **For initial tuning**: 4 days (current) — enough to see patterns, NOT enough to trust
- **For sweep validation**: 7+ days (through Feb 25) — reduces overfit risk
- **For production confidence**: 14+ days — validates across different market conditions

### Data collection plan

1. Continue running `multi_market_logger_hourly` with RTDS enabled
2. Each new day adds `2XX data/` directory
3. Re-run `fix_and_slugify.py` with all available data dirs each time:

```bash
# When Feb 22 data is ready:
python3 fix_and_slugify.py ./218data/ ./219data/ ./220data/ ./221data/ ./222data/ -o ./all_slugified

# When Feb 25 data is ready (minimum for sweep trust):
python3 fix_and_slugify.py ./218data/ ./219data/ ./220data/ ./221data/ ./222data/ ./223data/ ./224data/ ./225data/ -o ./all_slugified
```

4. Re-run sweep each time new data is added to check stability

### What "stability" means

- Top-5 configs should **not dramatically reshuffle** when new data is added
- ROI should converge (narrow range), not diverge (wide swings)
- If adding 1 day of data flips the best config, the sample is still too small

---

## Step 9: Full Parameter Reference

### DN Book Parameters (23 params)

| Parameter | Default | Type | Description |
|-----------|---------|------|-------------|
| `dn_cheap_threshold` | 0.42 | float | Max price to consider "cheap" for accumulation |
| `dn_max_combined` | 0.95 | float | Max combined_avg to accept (lower = more profit per pair) |
| `dn_latency_boost_ms` | 150 | int | Latency threshold for bonus pricing |
| `dn_latency_price_bonus` | 0.08 | float | Extra price tolerance during high latency |
| `dn_min_volume_latency` | 3.0 | float | Min volume for latency boost to apply |
| `dn_min_seed` | 20 | int | Min shares needed per side to consider "seeded" |
| `dn_seed_price` | 0.48 | float | Max price during seeding phase |
| `dn_max_shares` | 300 | int | Max shares per side |
| `dn_trade_size` | 40 | int | Shares per trade |
| `dn_max_imbalance` | 0.65 | float | Max allowed imbalance ratio |
| `dn_max_one_sided_cost` | 20.0 | float | **Critical**: Max $ on one side before stop |
| `dn_max_spread_bps` | 500.0 | float | Max bid-ask spread to trade through |
| `dn_early_window_pct` | 0.20 | float | First 20% = early aggressive seeding window |
| `dn_early_seed_price` | 0.45 | float | Max price in early window |
| `dn_counter_trend` | True | bool | Buy against z-score for cheaper fills |
| `dn_regime_gate` | True | bool | Skip extreme regimes |
| `dn_abandon_pct` | 0.40 | float | Time fraction to abandon one-sided positions |
| `dn_urgent_bonus` | 0.05 | float | Extra price tolerance for underweight side |
| `dn_conditional_seed` | False | bool | **Disabled** — causes regression |
| `dn_early_dev_gate` | False | bool | **Disabled** — causes regression |
| `dn_flip_hedge_enabled` | True | bool | Hedge when z-score flips during DN |
| `dn_flip_hedge_ratio` | 1.0 | float | Hedge size relative to committed position |
| `dn_flip_hedge_price_bonus` | 0.05 | float | Extra price tolerance for flip hedges |

### DIR Book Parameters (28 params)

| Parameter | Default | Type | Description |
|-----------|---------|------|-------------|
| `dir_enabled` | True | bool | Master switch for directional book |
| `dir_min_z_early` | 3.0 | float | Min z-score in early period |
| `dir_min_z_mid` | 2.0 | float | Min z-score in mid period (87%+ accuracy) |
| `dir_min_z_late` | 0.5 | float | Min z-score in late period |
| `dir_max_price` | 0.65 | float | Never pay more than this |
| `dir_min_price` | 0.04 | float | Floor price |
| `dir_trade_size` | 30 | int | Shares per DIR trade |
| `dir_max_shares` | 200 | int | Max shares |
| `dir_max_cost` | 80.0 | float | Max total DIR spend per market |
| `dir_min_ticks_between` | 5 | int | Cooldown between trades |
| `dir_no_trade_before_pct` | 0.35 | float | No DIR before 35% of window elapsed |
| `dir_early_budget_pct` | 0.25 | float | Budget cap for early DIR trades |
| `dir_min_dev_pct` | 0.05 | float | Min deviation to consider |
| `dir_consistency_window` | 50 | int | Ticks of z-direction history for consistency |
| `dir_max_spread_bps` | 500.0 | float | Max spread |
| `dir_require_z_flip` | True | bool | **Critical**: Require z to flip before committing |
| `dir_min_confidence` | 0.70 | float | Multi-signal confidence threshold (4/5 signals) |
| `enable_dir_emergency_hedge` | True | bool | Hedge abandoned DN with DIR |
| `dir_emergency_min_z` | 1.5 | float | Z threshold for emergency hedge trigger |
| `dir_emergency_max_cost` | 30.0 | float | Emergency hedge budget |
| `dir_max_precommit_flips` | 5 | int | Max z-flips before skipping DIR |
| `dir_flip_confidence_boost` | 0.10 | float | Extra confidence needed at 3+ flips |
| `dir_no_confidence_boost` | 0.10 | float | Extra confidence for NO-side commits |
| `dir_max_postcommit_flips` | 5 | int | Max z-flips after commit before freeze |
| `dir_late_reversal_check` | True | bool | Check for late z-reversal |
| `dir_late_reversal_pct` | 0.60 | float | Time threshold for late check |
| `dir_flip_hedge_enabled` | True | bool | Hedge on post-commit z-flips |
| `dir_flip_hedge_ratio` | 0.25 | float | Hedge 25% of committed position |
| `dir_flip_hedge_max_cost` | 40.0 | float | Budget for DIR flip-hedging |

### Shared / Global Parameters (6 params)

| Parameter | Default | Description |
|-----------|---------|-------------|
| `dn_initial_capital` | 1000.0 | Cross-market DN budget |
| `dir_initial_capital` | 1000.0 | Cross-market DIR budget |
| `max_exposure` | 200.0 | Hard cap per market |
| `min_ticks_between` | 2 | Global trade cooldown |
| `no_entry_after_pct` | 0.90 | No new entries in last 10% of window |
| `vol_source` | 'realized_volatility' | Which column for z-score volatility |

### RTDS Parameters (12 params)

| Parameter | Default | Description |
|-----------|---------|-------------|
| `rtds_enabled` | True | Master RTDS switch |
| `rtds_min_advantage_bps` | 3.0 | Min BPS edge to consider stale |
| `rtds_chainlink_min_dev` | 5.0 | Chainlink deviation for high confidence |
| `rtds_min_latency_ms` | 100 | Timestamp-mode latency threshold |
| `rtds_dn_bonus` | 0.05 | DN price bonus when stale |
| `rtds_dir_conf_bonus` | 0.15 | DIR confidence boost when stale |
| `rtds_dir_require_stale` | False | Hard gate on stale-only DIR |
| `rtds_dn_seed_enabled` | True | RTDS-directed seeding |
| `rtds_dn_seed_threshold` | 3.0 | Min BPS for RTDS seeding |
| `rtds_dn_seed_early_mult` | 1.5 | Early period threshold multiplier |
| `rtds_dn_max_bonus` | 0.10 | Max RTDS bonus cap |
| `rtds_double_stale_bonus` | 0.03 | Bonus when both feeds agree |

### Exploit Parameters (16 params)

| Parameter | Default | Exploit | Description |
|-----------|---------|---------|-------------|
| `exploit3_dn_enabled` | True | Stale Gate | Gate DN on stale book |
| `exploit3_dir_enabled` | True | Stale Gate | Gate DIR on stale book |
| `stale_gate_threshold_bps` | 3.0 | Stale Gate | Min BPS to consider stale |
| `snipe_enabled_dn` | True | Price Snipe | Apply to DN |
| `snipe_enabled_dir` | True | Price Snipe | Apply to DIR |
| `snipe_safety_margin` | 0.02 | Price Snipe | Buy only if ask < fair - 2c |
| `snipe_max_z_for_dn` | 2.0 | Price Snipe | Skip snipe for DN when z extreme |
| `snipe_max_ask_dir` | 0.85 | Price Snipe | Max price for DIR snipe |
| `edge_scale_enabled` | True | Edge Scale | Master switch |
| `edge_scale_reference_bps` | 5.0 | Edge Scale | Baseline for 1x sizing |
| `edge_scale_min_size` | 20 | Edge Scale | Floor shares |
| `edge_scale_max_size` | 80 | Edge Scale | Ceiling shares |
| `chainlink_arb_enabled` | True | Chainlink Arb | Master switch |
| `chainlink_arb_size_multiplier` | 2.0 | Chainlink Arb | Trade size multiplier |
| `chainlink_arb_max_cost` | 60.0 | Chainlink Arb | Budget |
| `chainlink_arb_max_price` | 0.75 | Chainlink Arb | Max price |

---

## Step 10: Quick Reference — Common Workflows

### "I just want to see if this data works"

```bash
python3 backtest-zv68.py --dir ./all_slugified/ --sample 3 --no-monte-carlo
```

### "Run a proper baseline"

```bash
python3 backtest-zv68.py --dir ./all_slugified/ --no-monte-carlo
```

### "Run the full sweep"

```bash
python3 backtest-zv68.py --dir ./all_slugified/ --sweep --no-monte-carlo
```

### "Test with real capital constraints"

```bash
python3 backtest-zv68.py --dir ./all_slugified/ --capital 200 --no-monte-carlo
```

### "Full production validation"

```bash
python3 backtest-zv68.py --dir ./all_slugified/ --capital 200
# (includes Monte Carlo: bankroll survival, z-flip threshold, conformal robustness)
```

### "Export for analysis"

```bash
python3 backtest-zv68.py --dir ./all_slugified/ --output results.xlsx --no-monte-carlo
```

### "Add new data and revalidate"

```bash
# 1. Re-run data prep with new directories
python3 fix_and_slugify.py ./218data/ ./219data/ ... ./225data/ -o ./all_slugified

# 2. Re-run baseline to see if results hold
python3 backtest-zv68.py --dir ./all_slugified/ --no-monte-carlo

# 3. Re-run sweep to check stability
python3 backtest-zv68.py --dir ./all_slugified/ --sweep --no-monte-carlo
```

---

## Step 11: Historical Log Analysis (Pre-Correction Baseline)

> All logs in `src/data/*.txt` were generated **before** outcome correction and before filtering incomplete slugs. These numbers are inflated by the `min(yes, no)` fallback bug. Use them as relative comparisons between versions, not as absolute targets.

### 37 Log Files Analyzed (zv51 → zv67)

### Performance Trajectory (Pre-Correction)

| Version | Date | Data | Markets | ROI | DN Profit | DIR Profit | Notes |
|---------|------|------|---------|-----|-----------|------------|-------|
| zv51 | Feb 16 | 216 | 34 | 8.8% | -$107 | +$268 | Pre-RTDS baseline |
| zv52 | Feb 18 | 218 | 48 | 0.9% | — | — | RTDS timestamp test, stale gate bottleneck found |
| zv53 | Feb 18 | 218 | 42 | 7.9% | -$112 | +$306 | Emergency hedge added |
| zv55 | Feb 18-19 | 218-219 | 42 | 8.0% | -$111 | +$306 | Timestamp vs BPS negligible diff |
| zv56 | Feb 18-20 | 218 | 42 | 8.7% | -$116 | +$337 | Flip-hedge mechanics added |
| zv62 | Feb 19 | 219 | 83 | 24.8% | — | — | Multi-market scaling (5.5x jump) |
| zv65 | Feb 18 | 218 | 48 | 36.3% | — | — | RTDS exploits introduced |
| zv66 | Feb 20 | MC250 | 69 | **40.4%** | -21% | +67% | Best pre-correction result |
| zv67 | Feb 19-20 | 219-220 | 83 | 32-40% | — | — | Latest, sustained 30%+ range |

### What the Logs Reveal

**1. DIR book was the profit driver (pre-correction)**
- DIR consistently showed 20-70% ROI across all versions
- DN consistently showed -4% to -21% ROI (expected: DN sacrifices profit for low-vol guarantee)
- With corrected outcomes, DIR went negative — the `min(yes,no)` fallback was giving DIR free wins

**2. RTDS-directed seeding never fired**
- `note.txt` confirms: `abs(rtds_binance_dev_bps)` was always below 3.0 bps at seed time
- The stale gate at 10.0 bps was blocking too aggressively (600k-800k DN ticks gated per run)
- Lowering `stale_gate_threshold_bps` to 3.0 could unlock 8-10% more DN trading capacity

**3. Exploit 4 (Chainlink Arb) was the most reliable**
- 8 trades per run, 17-20% ROI consistently
- Only exploit with isolated P&L — others are filters/modifiers

**4. Late-window entries were most accurate**
- 80-100% of market window: 92-96% direction accuracy
- Sweet spot for DIR entries confirmed across all versions

**5. NO-side prediction was weak**
- 0-50% win rate on NO-outcome markets
- System consistently biased toward YES predictions
- This matters more now with corrected outcomes

**6. Multi-market breadth improved results**
- 18 markets → ~13% ROI; 60+ markets → 30-40% ROI
- More markets = more DN opportunities + DIR diversification

### Key Parameter Configs From Logs

**Best historical sweep result (zv51, pre-correction):**
```
z_early=5.0, z_mid=1.0, z_late=0.5, confidence=0.70
dn_max_one_sided_cost=15, dn_max_combined=0.95
stale_gate=10.0 bps (too strict — identified as bottleneck)
```

**Converged config by zv66-67:**
```
dn_max_one_sided_cost=20, dn_abandon_pct=0.40
dir_emergency_min_z=1.5, dir_emergency_max_cost=30
stale_gate=3.0 bps (lowered from 10.0)
exploit4 (Chainlink): ON, 1.5-2.0x multiplier
exploit1 (Snipe): OFF in focused sweep (hurts bad markets)
```

### What This Means for Re-Tuning

1. **Expect lower ROI with corrected outcomes.** The 30-40% ROI numbers were inflated. Realistic target: 5-15% ROI if DN works, 0-5% if DN-only.

2. **DIR needs re-validation.** It was the profit driver but relied on the outcome bug. With real outcomes, it may need tighter entry gates or should be disabled.

3. **DN parameters are already well-converged.** The logs show `dn_max_one_sided_cost=20`, `dn_max_combined=0.95`, `dn_abandon_pct=0.40` were stable across zv56-zv67. Start here.

4. **Stale gate was the biggest bottleneck.** 10.0 bps blocked too many trades. The focused sweep locks it at 10.0 (conservative) — try 3.0-5.0 if DN capacity is too low.

5. **NO-side bias is a risk.** With corrected outcomes, markets that resolved NO will now correctly penalize wrong YES predictions. Watch direction accuracy by outcome side.

### Data Directories Used in Historical Logs

| Dir | Date | Rows | Markets | RTDS? |
|-----|------|------|---------|-------|
| `216data/` | Feb 16 | 2.6M | 34 | No |
| `218data/` | Feb 18-19 | 3.2M | 48 | Yes |
| `219data/` | Feb 19 | 3.5M | 83 | Yes |
| `220data/` | Feb 20 | ~3M | 69 | Yes |
| `221data/` | Feb 21 | ~3M | — | Yes |

Only `218data/` through `221data/` are usable with zv68. The `216data/` logs are useful only for understanding the parameter evolution, not for comparison.

### Recovering Deleted "Selected" Folder

The `selected/` folder (manually curated slugs filtered by RTDS quality on Feb 18) was deleted. However, **all raw hourly CSV logs still exist** in the `2XXdata/` directories. To recover:

```bash
# Re-run fix_and_slugify on all raw data — this recreates ALL slugs including
# the "bad" ones that were previously filtered out of selected/
python3 fix_and_slugify.py ./218data/ ./219data/ ./220data/ ./221data/ -o ./all_slugified
```

To specifically isolate the "bad" slugs (markets with poor RTDS data, high spread, extreme oscillation) for separate analysis:

```bash
# Step 1: Generate all slugs
python3 fix_and_slugify.py ./218data/ -o ./218data/all_slugs

# Step 2: Run backtest on ALL slugs to identify which ones the RTDS/quality
# filters would reject. The zv68 DynamicBacktestAdapter health score (<30 = bad)
# will flag them in the report. Compare against the historical log results
# to see which markets were in "selected" vs filtered out.
python3 backtest-zv68.py --dir ./218data/all_slugs/ --no-monte-carlo
```

The historical logs in `src/data/*.txt` show exactly which markets were traded and their per-market results — cross-reference these to identify which slugs were in the deleted `selected/` folder vs which were filtered out as "bad days."

**Why this matters for tuning:** The bad slugs are valuable for validating that the RTDS quality gates (stale gate, spread filter, health score) correctly reject unprofitable markets. If the bot makes money on good slugs but loses on bad ones, the filtering is working. If it loses on both, the parameters need more work.

---

## Step 12: Overfitting Analysis — Parameter-by-Parameter

> **Overall overfitting risk: LOW-MEDIUM**. Most parameters are deliberate conservative choices for an asymmetric payoff strategy, not data-fitted. The few genuinely uncertain parameters are tied to features that barely activate.

### Strategy Design: Asymmetric Payoff (Not Overfitting)

The parameter choices are designed for a specific distribution shape:
- **25th percentile**: Small loss (defensive caps keep downside tight)
- **50th percentile**: Positive (strict entry gates ensure most trades profit)
- **75th percentile**: Much fatter than 25th (let winners run)

This is a **positive skew strategy**. The "conservative" parameters aren't leaving money on the table — they're shaping the left tail to be shallow while allowing a fat right tail.

### Z-Score Signal Quality

The z-score is not a simple static calculation:
- **1-minute volatility** base, bootstrapped from **1-second** data
- **Fibonacci-weighted** blending across timeframes
- Falls back to **70k+ lines** of Binance spot for 1-min vol estimation
- Continuously re-estimated as new data arrives

This makes the z-score **adaptive**, not statically distributed. A `dir_min_z_mid = 2.0` threshold on an adaptive bootstrapped z-score is much more meaningful than 2.0 on a fixed-window calculation.

### Parameter Changes: zv51→zv53 (Deliberate, Not Data-Fitted)

| Parameter | Before | After | Rationale |
|---|---|---|---|
| `dir_min_z_mid` | 1.0 | 2.0 | Stricter entry = fewer trades but higher confidence; shapes median positive |
| `dn_max_one_sided_cost` | 40.0 | 20.0 | Lower cap = tighter 25th percentile loss; defensive by design |
| `dn_early_seed_price` | 0.50 | 0.45 | Fee-aware: Polymarket fees `25*(p*(1-p))^2` eat DN margin at 0.50 |
| `dn_max_combined` | 0.97 | 0.95 | Stricter DN entry = guaranteed 5c+ profit per pair instead of 3c |
| `dir_max_postcommit_flips` | 3 | 5 | Allow more z-score evolution before freezing position |

These are **risk management decisions**, not curve-fitting.

### SAFE Parameters (Deliberate Conservative Choices)

| Parameter | Default | Why It's Safe |
|---|---|---|
| `dir_min_z_mid` | 2.0 | Higher threshold = fewer but more confident trades; positive skew shaping |
| `dir_min_z_early` | 3.0 | Very strict early gate; adaptive z-score makes this meaningful |
| `dir_min_z_late` | 0.5 | Principled: late markets, low bar is sound |
| `dn_max_one_sided_cost` | 20.0 | Caps left tail; safety over upside is the strategy |
| `dn_early_seed_price` | 0.45 | Fee-aware pricing, not data-fitted |
| `dn_max_combined` | 0.95 | Stricter DN entry for guaranteed margin |
| `dn_seed_price` | 0.48 | Fee-conscious seeding threshold |
| `dn_early_window_pct` | 0.20 | Round number, conceptually sound |
| `dir_require_z_flip` | True | Safety feature, principled |
| `dn_counter_trend` | True | Market-neutral concept |
| Capital/exposure params | — | Config, not strategy |

### UNCERTAIN Parameters (Features That Barely Activate)

These are the genuinely unknown parameters — not because they're overfit, but because the features they control haven't been adequately tested:

| Parameter | Default | Issue |
|---|---|---|
| `rtds_dn_seed_threshold` | 3.0 | **Dead feature**: note.txt confirms deviation never hits threshold at seed time. Never fires. |
| `stale_gate_threshold_bps` | 3.0 or 10.0 | Mismatch between code (3.0) and focused sweep (10.0). Exact value unvalidated — need to determine if the feature just needs to be ON vs specific threshold. |
| `snipe_safety_margin` | 0.02 | Focused sweep disables sniping entirely. If the feature is OFF in best configs, this value doesn't matter. |
| `edge_scale_reference_bps` | 5.0 | Calibrated to Feb 18 RTDS advantage range; if advantage distribution shifts, scaling miscalibrates. |
| `rtds_chainlink_min_dev` | 5.0 | Rarely fires on low-vol data; untested on high-vol conditions. |
| `chainlink_arb_size_multiplier` | 2.0 | Feature barely activates; value is reasonable but unvalidated. |

### Low-Volatility Context

Current BTC is low-vol post 120k→68k crash. In this regime:
- Fewer z-flips → defensive parameters cost less (fewer events they need to block)
- Tighter spreads → RTDS deviations smaller → stale gate features may not trigger
- The adaptive z-score recalculates constantly, so it adjusts to the current vol regime automatically
- The defensive parameter set should perform **as designed**: small 25th pct loss, positive median, fat 75th pct

The main risk is not that parameters are wrong, but that **RTDS-dependent features may not activate** if spreads and deviations are too tight in calm conditions.

### Parameter Structure: 1 Random Variable, 88+ Controls

The z-score is the **only unpredictive random variable** in the system. Market direction at any moment is genuinely random — you can't control where z goes.

Every other parameter is a **predictive control lever** that bounds the loss regardless of z-score path:

| Type | Parameters | Purpose |
|---|---|---|
| **Random (uncontrollable)** | z-score direction | Market direction; accepted as unpredictable |
| **Entry controls** | `dn_max_combined`, `dir_min_z_*`, `dir_min_confidence`, `dn_seed_price`, `dn_early_seed_price` | Tighten entry quality — only enter when edge is clear |
| **Loss caps** | `dn_max_one_sided_cost`, `dir_max_cost`, `max_exposure`, `dir_emergency_max_cost` | Hard bounds on downside — loss can't exceed these regardless of z path |
| **Timing gates** | `stale_gate`, `dn_abandon_pct`, `no_entry_after_pct`, `dir_no_trade_before_pct` | Control when you trade — only when conditions are favorable |
| **Hedge mechanics** | `dir_flip_hedge_ratio`, `dn_flip_hedge_ratio`, `enable_dir_emergency_hedge` | Limit damage when z reverses against you |
| **Position limits** | `dn_max_shares`, `dir_max_shares`, `dn_max_imbalance`, `dn_trade_size` | Cap exposure per position |

**Why overfitting is not the real risk**: You're not trying to predict z better. You're tightening the envelope around what happens when z goes against you. The 88+ control parameters don't need to generalize to "predict better on new data" — they need to hold the loss bound across different z-score paths. As long as the loss caps and entry gates work mechanically, they work on any data.

### Tuning Philosophy: Defensive-First on Worst-Case Data

Parameters were tuned on the **top losing market slug from Feb 18** — the single worst-case market in the dataset. This is deliberate:

- The goal is **not** max ROI
- The goal is **bounded loss on the worst day** while letting normal/good days compound
- If the parameters survive the worst slug on the worst day, they survive everything

**Sweep interpretation changes with this philosophy:**
- Don't pick the config with highest ROI
- Pick the config where the **worst-performing market still has a bounded, small loss**
- A config with 10% ROI and max single-market loss of -$8 is better than 25% ROI with a -$40 outlier
- Check the **25th percentile** of per-market P&L, not the mean

**Why ROI doesn't matter:**
- Defensive parameters on bad days → small loss
- Same parameters on normal days → modest positive
- Same parameters on good days → fat positive (caps don't bind when markets behave)
- Over time, the positive skew compounds. You don't need any single day to be great.

### How to Validate on New Data

1. **Collect more data**: Every additional day reduces uncertainty. 7+ days enables train/test split.
2. **Check RTDS activation**: On new data, verify stale gate and edge-scaling are actually triggering. If not, the exact threshold values are moot.
3. **Monitor distribution shape**: Confirm the 25th/50th/75th percentile targets hold on new data. If the skew flattens, investigate which trades changed.
4. **Regime test**: Run separately on volatile vs calm days. The adaptive z-score should handle regime shifts, but RTDS thresholds may need adjustment.
5. **Worst-case validation**: On each new day of data, find the worst-performing slug. If its loss is within the bounds you tuned for on Feb 18's worst slug, the parameters hold.
