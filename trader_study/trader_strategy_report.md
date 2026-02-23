# Blushing-Fine (0x63ce…) BTC 15-Min Trader Strategy Report

**Date**: 2026-02-23
**Data**: 19,605 trades across Feb 20–22, 200 unique markets
**Focus**: Feb 21 deep-dive with cross-day comparison

---

## Executive Summary

Blushing-Fine runs a **delta-neutral (DN) market-making strategy** on Polymarket's BTC 15-minute Up/Down markets. The trader buys **both sides** of virtually every market, accumulating shares at a combined average cost below $1.00, aiming to guarantee profit regardless of outcome. The strategy is supplemented by mid-window **position rebalancing sells** that improve the combined average.

**Key findings**:
- The trader buys both Up and Down in **87–94% of markets** — this is overwhelmingly a DN strategy
- Only **14–24% of markets achieve guaranteed profit** (combined avg < $1 before any resolution), but **100% of markets have combined avg < $1 after accounting for sell revenue**
- Sells are always **position reductions on owned sides** (never naked), timed at 6–7 min into the window at lower prices — likely selling into favorable mid-window price moves
- Feb 20 was a **high-conviction, concentrated deployment** ($131K across 34 markets, avg $3,843/market)
- Feb 21–22 switched to **diversified, lower-size deployment** ($50K across 91 markets on Feb 21, avg $550/market)
- Feb 21 had 2 trading gaps (51 min and 34 min) in the early UTC hours, but trading was otherwise continuous across 24 hours
- The worst-case Feb 21 exposure was **-$9,682** across all markets, but the best-case was **+$19,515**

---

## 1. Position Structure: Overwhelmingly Both-Sides

The trader buys **both Up and Down** in the vast majority of markets:

| Date | Markets | Both Sides | Up-Heavy | Down-Heavy | Single-Side |
|------|---------|-----------|----------|------------|-------------|
| Feb 20 | 34 | 32 (94%) | 0 | 2 (6%) | 0 |
| Feb 21 | 91 | 79 (87%) | 5 (5%) | 4 (4%) | 3 (3%) |
| Feb 22 | 75 | 69 (92%) | 1 (1%) | 2 (3%) | 3 (4%) |

**Up/Down USDC Ratio** (for both-sides markets):
- Feb 20: avg 1.15, median 0.99 — nearly equal allocation
- Feb 21: avg 1.24, median 1.12 — slight Up bias
- Feb 22: avg 1.16, median 0.99 — back to balanced

**Takeaway**: This is a **delta-neutral strategy**. The trader doesn't predict direction — they accumulate both sides cheaply. The slight Up bias on Feb 21 (1.24 ratio) suggests the trader may have had marginal directional conviction on some markets, or Up shares were more available cheaply.

---

## 2. Entry Timing: Fast Opener, Then Accumulates

The trader enters markets **extremely quickly** after open, then spends the full 15 minutes accumulating:

| Metric | Feb 20 | Feb 21 | Feb 22 |
|--------|--------|--------|--------|
| Avg first entry | 38s | 24s | 38s |
| Median first entry | 10s | 8s | 12s |
| Within 60s | 91% | 88% | 79% |
| Within 120s | 94% | 98% | 92% |
| Avg trade delay (all) | 296s | 308s | 328s |

**Entry pattern**: The trader places the first buy within **4–12 seconds** of market open, then continues buying through the full 15-minute window. The average delay of ~5 minutes across all trades shows sustained accumulation, not front-loaded buying.

**Build sequence** (from DN guaranteed-profit markets on Feb 21):
- **Up bought first** in 56% of both-sides markets
- **Down bought first** in 44%
- Average gap between first Up and first Down buy: **132 seconds** (2.2 min)
- Full position build takes **8–12 minutes** typically

This suggests the trader starts buying whichever side has better pricing, then adds the opposite side ~2 minutes later as orders fill.

---

## 3. Price Levels: Buying the Mid-Range

The trader is **not** primarily sniping extreme cheap shares. Most capital goes into the $0.20–$0.60 range:

### Feb 21 Buy Price Distribution

| Price Bucket | # Buys | USDC Spent | % of Capital |
|-------------|--------|-----------|-------------|
| < $0.05 | 58 | $36 | 0.1% |
| $0.05–0.10 | 88 | $124 | 0.2% |
| $0.10–0.20 | 363 | $1,000 | 2.0% |
| $0.20–0.40 | 1,897 | $10,304 | 20.6% |
| **$0.40–0.60** | **2,834** | **$25,730** | **51.4%** |
| $0.60–0.80 | 761 | $10,974 | 21.9% |
| $0.80–0.95 | 93 | $1,916 | 3.8% |

**Average buy prices**:
- Up side: $0.42 avg
- Down side: $0.43 avg

This is consistent across all three days. The trader's strategy is to buy both sides at approximately **$0.40–0.50 each**, giving a combined cost of ~$0.85–$0.95 per share-pair — right at the edge of guaranteed profitability.

### How DN Profit Works

In guaranteed-profit DN markets, the pattern is:
- Buy **cheap side** (the perceived loser) at $0.03–$0.30
- Buy **expensive side** (the perceived winner) at $0.45–$0.65
- Combined: $0.28–$0.50 per share-pair → $0.50–$0.72 profit per pair guaranteed

Example from Feb 21 14:30 (best DN market):
- Up: avg $0.45, 1,861 shares, cost $866
- Down: avg $0.28, 1,727 shares, cost $443
- Combined avg: $0.36 → **$552 min profit, $418 max profit** (guaranteed)

---

## 4. Position Sizing: Feb 20 Was Massive, Then Scaled Down

| Metric | Feb 20 | Feb 21 | Feb 22 |
|--------|--------|--------|--------|
| Total deployed | $130,661 | $50,083 | $32,946 |
| Markets | 34 | 91 | 75 |
| Avg per market | $3,843 | $550 | $439 |
| Median per market | $2,689 | $422 | $366 |
| Max single market | $16,558 | $2,764 | $1,735 |
| Markets > $1,000 | 28 (82%) | 9 (10%) | 2 (3%) |

**Capital concentration**:
- Feb 20: Top 5 markets = 41% of capital (very concentrated)
- Feb 21: Top 5 markets = 19% of capital (much more diversified)
- Feb 22: Top 5 markets = 17% of capital (continued diversification)

**The trader dramatically reduced per-market exposure after Feb 20.** This could indicate:
1. Feb 20 was a high-confidence day (or the trader was ramping up)
2. The trader shifted to a diversification strategy after Feb 20
3. Available liquidity/edge decreased on Feb 21–22

---

## 5. Sell Behavior: Mid-Window Position Rebalancing

The trader actively sells shares — **always reducing existing positions**, never selling naked:

| Metric | Feb 20 | Feb 21 | Feb 22 |
|--------|--------|--------|--------|
| Total sells | 1,363 | 836 | 646 |
| Sell/Buy ratio | 21% | 14% | 15% |
| Total sell USDC | $20,020 | $5,143 | $4,047 |
| Avg sell price | $0.38 | $0.36 | $0.37 |
| Avg sell timing | 6.3 min | 6.6 min | 7.5 min |

**Sell timing pattern**:
- 10% of sells within 2 min (very early — possibly cancellations or immediate rebalancing)
- **70% of sells at 2–10 minutes** (mid-window, the core rebalancing zone)
- 20–30% of sells after 10 minutes (late window, near resolution)

**All sells reduce owned positions** (0 naked sells across all 3 days). The avg sell price ($0.36–0.38) is **lower than the avg buy price** ($0.43–0.45), meaning the trader sells shares at a loss per-share but improves the combined average of the remaining position.

**Interpretation**: The trader sells shares that have become **less likely to win** (price dropped from entry), accepting a per-share loss, but this improves the combined cost basis of the remaining portfolio. This is active **DN position management** — when one side's price drops, sell those shares and keep the opposite side which is now more likely to pay out.

---

## 6. P&L Analysis: Edge Is Thin

### Combined Average Cost (Net of Sells)

| Combined Avg | Feb 20 | Feb 21 | Feb 22 |
|-------------|--------|--------|--------|
| < $0.30 | 3 (9%) | 2 (2%) | 5 (7%) |
| $0.30–0.40 | 2 (6%) | 14 (15%) | 9 (12%) |
| **$0.40–0.50** | **14 (41%)** | **50 (55%)** | **44 (59%)** |
| $0.50–0.60 | 13 (38%) | 24 (26%) | 14 (19%) |
| $0.60–0.70 | 2 (6%) | 1 (1%) | 2 (3%) |
| < $1.00 | **100%** | **100%** | **100%** |

Every single market across all 3 days has a combined average below $1.00 after accounting for sell revenue. However, the gross cost (before sells) tells a different story:

### Guaranteed Profit Markets (Gross, Before Sells)

| Date | Guaranteed Profit | % | Avg Combined Avg | Total Min P&L | Total Max P&L |
|------|-----------------|---|-----------------|---------------|---------------|
| Feb 20 | 4/34 | 12% | $0.53 | -$22,430 | +$21,474 |
| Feb 21 | 13/91 | 14% | $0.51 | -$9,682 | +$19,515 |
| Feb 22 | 18/75 | 24% | $0.52 | -$5,063 | +$11,812 |

**Feb 21 had the best DN hit rate** (14% vs 12% on Feb 20), but the overall strategy depends heavily on which outcome resolves. The total P&L swings from -$9.7K to +$19.5K depending on market outcomes.

### Feb 21 Markets Where Both Outcomes Lose

5 out of 91 markets had **guaranteed losses** (both pnl_if_up and pnl_if_down negative):

| Market | Cost | PnL if Up | PnL if Down | Issue |
|--------|------|-----------|-------------|-------|
| 06:45 | $376 | -$19 | -$67 | Combined avg $1.39 (!) |
| 12:45 | $543 | -$3 | -$107 | Overpaid on both sides |
| 13:45 | $174 | -$6 | -$49 | Small but still losing |
| 14:00 | $640 | -$81 | -$29 | Large position, both lose |
| 17:00 | $300 | -$68 | -$15 | Moderate loss either way |

The 06:45 market is notable — combined avg of $1.39 means the trader badly overpaid, likely due to rapid price changes or poor fill quality.

---

## 7. Trading Halt Analysis: Feb 21 Gaps

Two gaps were detected on Feb 21 (vs zero on Feb 20):

### Gap 1: 02:09 → 03:00 UTC (51 minutes)
- **Last trade before gap**: BUY Down @ $0.31 (market btc-updown-15m-1771639200)
- **Was active across 5 markets** before halting
- **First trade after gap**: BUY Up @ $0.48 (new market btc-updown-15m-1771642800)
- **Skipped ~3 market windows** during the gap

### Gap 2: 04:41 → 05:15 UTC (34 minutes)
- **Last trade before gap**: SELL Down @ $0.41 (was rebalancing)
- **First trade after gap**: BUY Up @ $0.47 (new market)
- **Skipped ~2 market windows**

**Context**: Both gaps occurred during low-liquidity UTC hours (2–5 AM). The trader was active across 3–5 markets per hour otherwise. A similar 33-min gap occurred on Feb 22 at the exact same time (04:41–05:15 UTC), suggesting this may be a **scheduled maintenance window** rather than a reaction to losses.

**Note**: Feb 22 also shows a gap at the same time, and Feb 20 trading only started at 15:40 UTC (no early morning data). The "halt" may simply be when the trader's bot restarts or a scheduled downtime.

---

## 8. Feb 21 Hourly Activity Timeline

| Hour (UTC) | Trades | Buy USDC | Sell USDC | Markets |
|-----------|--------|----------|-----------|---------|
| 00:00 | 869 | $7,949 | $848 | 4 |
| 01:00 | 150 | $2,748 | $38 | 4 |
| 02:00 | 64 | $787 | $127 | 1 |
| 03:00 | 151 | $1,209 | $60 | 4 |
| 04:00 | 235 | $1,715 | $138 | 3 |
| 05:00 | 263 | $1,591 | $292 | 3 |
| 06:00 | 266 | $1,347 | $261 | 4 |
| 07:00 | 431 | $1,819 | $341 | 4 |
| 08:00 | 517 | $2,522 | $498 | 4 |
| 09:00 | 354 | $2,172 | $521 | 4 |
| 10:00 | 249 | $1,543 | $71 | 4 |
| 11:00 | 284 | $1,304 | $100 | 4 |
| 12:00 | 287 | $2,146 | $205 | 4 |
| 13:00 | 183 | $1,218 | $6 | 4 |
| 14:00 | 361 | $3,144 | $205 | 4 |
| 15:00 | 355 | $2,800 | $374 | 4 |
| 16:00 | 222 | $2,116 | $16 | 4 |
| 17:00 | 183 | $1,685 | $178 | 4 |
| 18:00 | 430 | $3,194 | $386 | 4 |
| 19:00 | 183 | $1,144 | $108 | 4 |
| 20:00 | 291 | $1,809 | $103 | 4 |
| 21:00 | 199 | $1,413 | $74 | 4 |
| 22:00 | 236 | $1,501 | $159 | 4 |
| 23:00 | 167 | $1,208 | $34 | 4 |

**Pattern**: The trader operates **24 hours**, processing exactly 4 markets per hour (=4 × 15-min windows). The midnight hour (00:00) was notably active with $7,949 in buys — potentially the trader was running with Feb 20's higher sizing before scaling down. Activity peaks again at 07:00–09:00 and 14:00–18:00 UTC.

---

## 9. Cross-Day Comparison: Strategy Evolution

| Metric | Feb 20 | Feb 21 | Feb 22 |
|--------|--------|--------|--------|
| **Markets** | 34 | 91 | 75 |
| **Trades** | 7,842 | 6,930 | 4,833 |
| **Total deployed** | $130,661 | $50,083 | $32,946 |
| **Per-market avg** | $3,843 | $550 | $439 |
| **DN guaranteed** | 4 (12%) | 13 (14%) | 18 (24%) |
| **Both-sides** | 94% | 87% | 92% |
| **Sells** | 1,363 (21%) | 836 (14%) | 646 (15%) |
| **Sell revenue** | $20,020 | $5,143 | $4,047 |
| **Worst-case P&L** | -$22,430 | -$9,682 | -$5,063 |
| **Best-case P&L** | +$21,474 | +$19,515 | +$11,812 |

### Key Differences

1. **Feb 20 → Feb 21: 7x reduction in per-market sizing** ($3,843 → $550). This is the single biggest change. The trader went from concentrated, high-conviction positions to diversified, lower-risk positions.

2. **Sell activity dropped**: Feb 20 had 21% sell ratio vs 14% on Feb 21. Less rebalancing may indicate:
   - Smaller positions need less management
   - The trader was less engaged in active management
   - Tighter initial positioning reduced need for rebalancing

3. **DN success rate improved**: 12% → 14% → 24% across the three days. The trader may have been learning and improving fill quality, or market conditions favored DN accumulation.

4. **Feb 21 had the widest P&L swing asymmetry**: -$9.7K downside vs +$19.5K upside. This asymmetry exists because the trader tends to accumulate more of the cheaper side, so when that side wins, the payout is larger.

---

## 10. Strategy Characterization

### What Blushing-Fine Is Doing

**Core strategy: Automated Delta-Neutral Market Making**

1. **At market open** (~4–12s): Place initial buys on whichever side has better pricing
2. **During first 2 minutes**: Add opposite side, begin building balanced position
3. **Minutes 2–10**: Continue accumulating both sides, buying dips; sell shares whose probability has dropped
4. **Minutes 10–15**: Final accumulation, mostly passive
5. **At resolution**: Collect $1 per winning share; lose all losing shares

**The edge**: By buying both sides at a combined average below $1.00, the trader guarantees profit if they can achieve this consistently. The sell activity (at lower prices) is the rebalancing mechanism — selling shares that have become less likely to win, accepting a per-share loss, but improving the overall combined average.

### Strategy Classification

| Attribute | Classification |
|-----------|---------------|
| **Type** | Delta-Neutral / Market Making |
| **Direction** | Non-directional (buys both sides) |
| **Timing** | Early entry (4–12s), full-window accumulation |
| **Sizing** | Uniform per-market (Feb 21–22), concentrated (Feb 20) |
| **Rebalancing** | Active sells at mid-window, always reducing owned positions |
| **Automation** | Almost certainly automated (24h operation, 4–12s entry, consistent patterns) |
| **Edge source** | Accumulating combined avg < $1.00 through patient multi-order building |

### Risks

1. **Slippage/overpay**: 5 markets on Feb 21 had both outcomes losing — combined avg exceeded $1.00
2. **Concentration risk**: Feb 20's $16.5K single-market positions are very exposed
3. **Outcome dependence**: Despite DN intent, most markets are **not** guaranteed profit — P&L depends heavily on which side resolves. The total swing is $29K on Feb 21 (-$9.7K to +$19.5K)
4. **Liquidity**: In 15-min markets, available order book depth constrains how much can be accumulated cheaply

### Estimated Edge Per Market (Feb 21)

- Median combined avg (net of sells): ~$0.45
- This implies ~$0.55 gross profit per share-pair at resolution
- With median market cost of $422 and median combined avg of $0.45:
  - Estimated shares per market: ~940 share-pairs
  - Expected profit per market: ~$520 if combined avg holds
  - But only 14% of markets achieve guaranteed profit without resolution luck
  - **The remaining 86% of P&L depends on the market outcome**

---

## 11. Implications for Our Strategy

### What We Can Learn

1. **Both-sides accumulation works** — the trader has been active for at least 3 days with significant capital, suggesting positive overall P&L
2. **Speed matters** — entering within 4–12s of market open is critical to getting good prices
3. **Rebalancing sells improve P&L** — selling shares that have depreciated accepts a small per-share loss but improves the overall position
4. **Diversification over concentration** — the Feb 20 → Feb 21 shift suggests the trader learned that concentration risk is dangerous
5. **24-hour operation is required** — markets run continuously, and the trader participates in every single 15-minute window

### How This Relates to Our DN Book

Our DN strategy targets `combined_avg < $1.00` as the guaranteed-profit threshold. Blushing-Fine achieves this on 100% of markets (net of sells), but only 14% achieve it on gross buy cost alone. The sell-rebalancing mechanism is therefore **critical** to the strategy's success and should be incorporated into our approach.

### Key Metric to Track

The single most important metric is **combined average cost net of sell revenue**. Blushing-Fine targets the $0.40–$0.50 range, which provides a healthy $0.50–$0.60 profit margin per share-pair.

---

*Report generated from analysis of btc_15min_trades.csv (19,605 rows, Feb 20–22 2026)*
*Analysis scripts: analyze_trader.py, analyze_supplementary.py*
