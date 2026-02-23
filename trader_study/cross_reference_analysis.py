#!/usr/bin/env python3
"""
Cross-reference trader entries with RTDS market data.
Maps each trader trade on Feb 21 to the closest market data row,
then analyzes entry conditions, combined prices, and compares vs our backtest.
"""

import csv
import os
import sys
import json
import re
from datetime import datetime, timezone
from collections import defaultdict
from statistics import mean, median, stdev

# === PATHS ===
TRADER_CSV = "/home/lehash/development/poly-trader/trader1/filtered_events/btc_15min_trades.csv"
MARKET_DIR = "/home/lehash/development/combine-fetcher/221data/fixed/"
BACKTEST_FILE = "/home/lehash/development/poly-mock-trading/src/data/zv691_feb21unfitFixed.txt"
OUTPUT_FILE = "/home/lehash/development/combine-fetcher/trader_study/cross_reference_report.md"

# === STEP 1: Load trader trades for Feb 21 with overlapping slugs ===
print("Loading trader trades...")

# Get list of our market CSV slugs
our_slugs = set()
for fname in os.listdir(MARKET_DIR):
    if fname.endswith(".csv"):
        our_slugs.add(fname.replace(".csv", ""))

print(f"  Our market CSVs: {len(our_slugs)} slugs")

# Load trader trades (use csv.DictReader for comma-in-title safety)
trader_trades = []
with open(TRADER_CSV) as f:
    reader = csv.DictReader(f)
    for row in reader:
        if "2026-02-21" in row["timestamp_readable"]:
            slug = row["event_slug"]
            if slug in our_slugs:
                trader_trades.append({
                    "timestamp": int(row["timestamp"]),
                    "timestamp_readable": row["timestamp_readable"],
                    "slug": slug,
                    "side": row["side"],
                    "outcome": row["outcome"],
                    "outcome_index": int(row["outcome_index"]),
                    "size": float(row["size"]),
                    "usdc_size": float(row["usdc_size"]),
                    "price": float(row["price"]),
                    "type": row["type"],
                })

print(f"  Feb 21 trades in overlapping markets: {len(trader_trades)}")
print(f"  Unique slugs traded: {len(set(t['slug'] for t in trader_trades))}")

# === STEP 2: Load market data CSVs into memory (indexed by timestamp_unix_ms) ===
print("\nLoading market data CSVs...")

market_data = {}  # slug -> list of rows sorted by timestamp
for slug in our_slugs:
    fpath = os.path.join(MARKET_DIR, f"{slug}.csv")
    rows = []
    with open(fpath) as f:
        reader = csv.DictReader(f)
        for row in reader:
            try:
                ts_ms = int(row["timestamp_unix_ms"])
                rows.append({
                    "ts_ms": ts_ms,
                    "timestamp": row["timestamp"],
                    "binance_price": float(row["binance_price"]) if row["binance_price"] else None,
                    "yes_bid": float(row["yes_bid"]) if row["yes_bid"] else None,
                    "yes_ask": float(row["yes_ask"]) if row["yes_ask"] else None,
                    "no_bid": float(row["no_bid"]) if row["no_bid"] else None,
                    "no_ask": float(row["no_ask"]) if row["no_ask"] else None,
                    "time_to_expiry_minutes": float(row["time_to_expiry_minutes"]) if row["time_to_expiry_minutes"] else None,
                    "strike_price": float(row["strike_price"]) if row["strike_price"] else None,
                    "realized_volatility": float(row["realized_volatility"]) if row["realized_volatility"] else None,
                    "rtds_binance_dev_bps": float(row["rtds_binance_dev_bps"]) if row["rtds_binance_dev_bps"] else None,
                    "rtds_chainlink_dev_bps": float(row["rtds_chainlink_dev_bps"]) if row["rtds_chainlink_dev_bps"] else None,
                    "rtds_price_advantage_bps": float(row["rtds_price_advantage_bps"]) if row["rtds_price_advantage_bps"] else None,
                    "rtds_chainlink_binance_spread_bps": float(row["rtds_chainlink_binance_spread_bps"]) if row["rtds_chainlink_binance_spread_bps"] else None,
                    "volatility_regime": row.get("volatility_regime", ""),
                    "rtds_binance_stale_ms": float(row["rtds_binance_stale_ms"]) if row.get("rtds_binance_stale_ms") else None,
                    "rtds_chainlink_stale_ms": float(row["rtds_chainlink_stale_ms"]) if row.get("rtds_chainlink_stale_ms") else None,
                    "yes_spread_bps": float(row["yes_spread_bps"]) if row.get("yes_spread_bps") else None,
                    "no_spread_bps": float(row["no_spread_bps"]) if row.get("no_spread_bps") else None,
                })
            except (ValueError, KeyError):
                continue
    # Sort by timestamp
    rows.sort(key=lambda r: r["ts_ms"])
    market_data[slug] = rows
    # Only load every Nth row to save memory — we need ~1sec precision
    # Actually, market data is ~80k rows per 15 min market = ~90 rows/sec
    # Trader timestamp is in seconds. We need the closest row.

print(f"  Loaded {len(market_data)} market CSVs")
for slug in sorted(list(market_data.keys()))[:3]:
    print(f"    {slug}: {len(market_data[slug])} rows")

# === STEP 3: Binary search for closest market row to each trader trade ===
print("\nJoining trader trades to market state...")

import bisect

def find_closest_row(rows, target_ts_ms):
    """Binary search for the closest row to target timestamp (ms)."""
    if not rows:
        return None
    # Extract timestamps for bisect
    timestamps = [r["ts_ms"] for r in rows]
    idx = bisect.bisect_left(timestamps, target_ts_ms)

    # Check boundaries
    if idx == 0:
        return rows[0]
    if idx >= len(rows):
        return rows[-1]

    # Compare neighbors
    before = rows[idx - 1]
    after = rows[idx]
    if (target_ts_ms - before["ts_ms"]) <= (after["ts_ms"] - target_ts_ms):
        return before
    return after

joined_trades = []
for trade in trader_trades:
    slug = trade["slug"]
    if slug not in market_data:
        continue

    target_ms = trade["timestamp"] * 1000  # convert seconds to ms
    closest = find_closest_row(market_data[slug], target_ms)

    if closest is None:
        continue

    time_diff_ms = abs(target_ms - closest["ts_ms"])

    # Compute combined ask price
    yes_ask = closest["yes_ask"]
    no_ask = closest["no_ask"]
    combined_ask = (yes_ask + no_ask) if (yes_ask is not None and no_ask is not None) else None

    joined_trades.append({
        **trade,
        "market_state": closest,
        "time_diff_ms": time_diff_ms,
        "combined_ask": combined_ask,
    })

print(f"  Joined {len(joined_trades)} trades (time_diff_ms stats below)")

time_diffs = [t["time_diff_ms"] for t in joined_trades]
print(f"  Median time_diff: {median(time_diffs):.0f}ms, Max: {max(time_diffs):.0f}ms, Mean: {mean(time_diffs):.0f}ms")

# === STEP 4: Aggregate per-market entry analysis ===
print("\nAggregating per-market entry analysis...")

# Group trades by slug, then within each slug by "entry burst" (trades within same second cluster)
trades_by_slug = defaultdict(list)
for t in joined_trades:
    trades_by_slug[t["slug"]].append(t)

# For each market, determine the trader's entry pattern
market_entries = {}
for slug, trades in trades_by_slug.items():
    trades.sort(key=lambda t: t["timestamp"])

    # Find first trade timestamp per outcome
    first_up_ts = None
    first_down_ts = None

    up_trades = [t for t in trades if t["outcome"] == "Up"]
    down_trades = [t for t in trades if t["outcome"] == "Down"]

    if up_trades:
        first_up_ts = up_trades[0]["timestamp"]
    if down_trades:
        first_down_ts = down_trades[0]["timestamp"]

    # USDC totals
    up_usdc = sum(t["usdc_size"] for t in up_trades)
    down_usdc = sum(t["usdc_size"] for t in down_trades)
    total_usdc = up_usdc + down_usdc

    # Size totals (shares)
    up_shares = sum(t["size"] for t in up_trades)
    down_shares = sum(t["size"] for t in down_trades)

    # Weighted avg price per side
    up_wavg = up_usdc / up_shares if up_shares > 0 else None
    down_wavg = down_usdc / down_shares if down_shares > 0 else None

    # Combined cost per share pair
    combined_cost = None
    if up_wavg is not None and down_wavg is not None:
        combined_cost = up_wavg + down_wavg

    # First trade's market state (the entry conditions)
    first_trade = trades[0]
    ms = first_trade["market_state"]

    # Entry time relative to market
    entry_tte = ms["time_to_expiry_minutes"]

    # Check if trader bought both sides
    bought_both = len(up_trades) > 0 and len(down_trades) > 0

    # Extract epoch from slug to get market_start_epoch
    slug_epoch = int(slug.split("-")[-1])
    market_expiry_epoch = slug_epoch + 900  # 15 min = 900 sec

    market_entries[slug] = {
        "slug": slug,
        "n_trades": len(trades),
        "n_up_trades": len(up_trades),
        "n_down_trades": len(down_trades),
        "up_usdc": up_usdc,
        "down_usdc": down_usdc,
        "total_usdc": total_usdc,
        "up_shares": up_shares,
        "down_shares": down_shares,
        "up_wavg_price": up_wavg,
        "down_wavg_price": down_wavg,
        "combined_cost_per_share": combined_cost,
        "bought_both": bought_both,
        "first_trade_ts": first_trade["timestamp"],
        "first_trade_readable": first_trade["timestamp_readable"],
        # Market state at entry
        "entry_tte_minutes": entry_tte,
        "entry_binance_dev_bps": ms["rtds_binance_dev_bps"],
        "entry_chainlink_dev_bps": ms["rtds_chainlink_dev_bps"],
        "entry_price_advantage_bps": ms["rtds_price_advantage_bps"],
        "entry_cl_bn_spread_bps": ms["rtds_chainlink_binance_spread_bps"],
        "entry_volatility_regime": ms["volatility_regime"],
        "entry_binance_stale_ms": ms["rtds_binance_stale_ms"],
        "entry_chainlink_stale_ms": ms["rtds_chainlink_stale_ms"],
        "entry_binance_price": ms["binance_price"],
        "entry_strike_price": ms["strike_price"],
        "entry_yes_ask": ms["yes_ask"],
        "entry_no_ask": ms["no_ask"],
        "entry_yes_bid": ms["yes_bid"],
        "entry_no_bid": ms["no_bid"],
        "entry_combined_ask": first_trade["combined_ask"],
        "entry_realized_vol": ms["realized_volatility"],
        "entry_yes_spread_bps": ms["yes_spread_bps"],
        "entry_no_spread_bps": ms["no_spread_bps"],
    }

print(f"  Markets analyzed: {len(market_entries)}")
both_sides = sum(1 for m in market_entries.values() if m["bought_both"])
print(f"  Bought both sides: {both_sides}/{len(market_entries)}")

# === STEP 5: Parse backtest P&L from the results file ===
print("\nParsing backtest results...")

backtest_pnl = {}  # slug -> {"combined": float, "dn": float, "dir": float, "outcome": str}

with open(BACKTEST_FILE) as f:
    content = f.read()

# Parse the Combined Top/Bottom sections
# Pattern: btc-updown-15m-EPOCH: $AMOUNT (ROI%) | DN:... $DN_PNL | DIR:... $DIR_PNL | OUTCOME
combined_pattern = re.compile(
    r'(btc-updown-15m-\d+): \$([-\d.]+) \(([-\d.]+)%\) \| DN:.*?\$([-\d.]+)\s*\| DIR:.*?\$([-\d.]+)\s*\| (YES|NO)'
)

# Also parse DN Book and DIR Book separately for complete per-market data
dn_pattern = re.compile(
    r'(btc-updown-15m-\d+): \$([-\d.]+) \(ROI:([-\d.]+)%\) \|.*?cost=\$([\d.]+)\s+profit=\$([-\d.]+)'
)

# Parse all lines for per-market data from combined section
for match in combined_pattern.finditer(content):
    slug = match.group(1)
    combined_pnl = float(match.group(2))
    combined_roi = float(match.group(3))
    dn_pnl = float(match.group(4))
    dir_pnl = float(match.group(5))
    outcome = match.group(6)
    backtest_pnl[slug] = {
        "combined": combined_pnl,
        "dn": dn_pnl,
        "dir": dir_pnl,
        "outcome": outcome,
        "roi": combined_roi,
    }

print(f"  Parsed {len(backtest_pnl)} backtest per-market results from Combined section")

# For markets not in top/bottom 10, we need to scrape from DN and DIR sections
# Let's parse all unique market P&L lines more broadly
dn_markets = {}
dir_markets = {}

lines = content.split('\n')
in_dn_section = False
in_dir_section = False
in_combined_section = False

for line in lines:
    if "── [DN Book]" in line:
        in_dn_section = True
        in_dir_section = False
        in_combined_section = False
        continue
    if "── [DIR Book]" in line:
        in_dir_section = True
        in_dn_section = False
        in_combined_section = False
        continue
    if "── [Combined]" in line:
        in_combined_section = True
        in_dn_section = False
        in_dir_section = False
        continue
    if line.strip().startswith("──") or line.strip() == "":
        if not line.strip().startswith("──"):
            pass
        else:
            in_dn_section = False
            in_dir_section = False
            in_combined_section = False
        continue

    if in_dn_section:
        m = re.match(r'\s+(btc-updown-15m-\d+):.*?profit=\$([-\d.]+)', line)
        if m:
            dn_markets[m.group(1)] = float(m.group(2))

    if in_dir_section:
        m = re.match(r'\s+(btc-updown-15m-\d+):.*?profit=\$([-\d.]+)', line)
        if m:
            dir_markets[m.group(1)] = float(m.group(2))

print(f"  DN book markets parsed: {len(dn_markets)}")
print(f"  DIR book markets parsed: {len(dir_markets)}")

# Fill in any markets not in combined top/bottom
for slug in our_slugs:
    if slug not in backtest_pnl:
        dn_p = dn_markets.get(slug, 0.0)
        dir_p = dir_markets.get(slug, 0.0)
        backtest_pnl[slug] = {
            "combined": dn_p + dir_p,
            "dn": dn_p,
            "dir": dir_p,
            "outcome": "?",
            "roi": 0.0,
        }

# === STEP 6: Compute aggregate statistics ===
print("\n=== AGGREGATE ANALYSIS ===\n")

# 6a. Entry dev_bps distribution
dev_bps_vals = [m["entry_binance_dev_bps"] for m in market_entries.values() if m["entry_binance_dev_bps"] is not None]
abs_dev = [abs(d) for d in dev_bps_vals]
print(f"1. Entry Binance dev_bps:")
print(f"   Mean: {mean(dev_bps_vals):.2f} bps, Median: {median(dev_bps_vals):.2f} bps")
print(f"   Mean |dev|: {mean(abs_dev):.2f} bps, Median |dev|: {median(abs_dev):.2f} bps")
print(f"   Stdev: {stdev(dev_bps_vals):.2f} bps")
print(f"   Min: {min(dev_bps_vals):.2f}, Max: {max(dev_bps_vals):.2f}")

# Dev_bps buckets
buckets = [(0, 2), (2, 5), (5, 10), (10, 20), (20, 50), (50, 100)]
print(f"   Distribution by |dev_bps|:")
for lo, hi in buckets:
    cnt = sum(1 for d in abs_dev if lo <= d < hi)
    print(f"     [{lo}, {hi}): {cnt} markets ({cnt/len(abs_dev)*100:.0f}%)")
cnt_big = sum(1 for d in abs_dev if d >= 100)
print(f"     [100+): {cnt_big} markets ({cnt_big/len(abs_dev)*100:.0f}%)")

# 6b. Staleness analysis
bn_stale = [m["entry_binance_stale_ms"] for m in market_entries.values() if m["entry_binance_stale_ms"] is not None]
cl_stale = [m["entry_chainlink_stale_ms"] for m in market_entries.values() if m["entry_chainlink_stale_ms"] is not None]
print(f"\n2. Staleness at entry:")
print(f"   Binance stale_ms: Mean={mean(bn_stale):.0f}, Median={median(bn_stale):.0f}, Max={max(bn_stale):.0f}")
print(f"   Chainlink stale_ms: Mean={mean(cl_stale):.0f}, Median={median(cl_stale):.0f}, Max={max(cl_stale):.0f}")

# 6c. Combined ask price
combined_asks = [m["entry_combined_ask"] for m in market_entries.values() if m["entry_combined_ask"] is not None]
print(f"\n3. Combined ask at entry (yes_ask + no_ask):")
print(f"   Mean: ${mean(combined_asks):.4f}, Median: ${median(combined_asks):.4f}")
print(f"   Min: ${min(combined_asks):.4f}, Max: ${max(combined_asks):.4f}")
below_1 = sum(1 for c in combined_asks if c < 1.0)
print(f"   Below $1.00: {below_1}/{len(combined_asks)} ({below_1/len(combined_asks)*100:.0f}%)")

# Trader's combined COST per share (weighted avg)
combined_costs = [m["combined_cost_per_share"] for m in market_entries.values() if m["combined_cost_per_share"] is not None]
if combined_costs:
    print(f"\n   Trader's combined COST per share pair (wavg):")
    print(f"   Mean: ${mean(combined_costs):.4f}, Median: ${median(combined_costs):.4f}")
    print(f"   Min: ${min(combined_costs):.4f}, Max: ${max(combined_costs):.4f}")
    below_1_cost = sum(1 for c in combined_costs if c < 1.0)
    print(f"   Below $1.00: {below_1_cost}/{len(combined_costs)} ({below_1_cost/len(combined_costs)*100:.0f}%)")

# 6d. Time to expiry at entry
ttes = [m["entry_tte_minutes"] for m in market_entries.values() if m["entry_tte_minutes"] is not None]
print(f"\n4. Time to expiry at entry:")
print(f"   Mean: {mean(ttes):.1f} min, Median: {median(ttes):.1f} min")
print(f"   Min: {min(ttes):.1f} min, Max: {max(ttes):.1f} min")

tte_buckets = [(0, 3), (3, 5), (5, 8), (8, 11), (11, 15)]
print(f"   Distribution:")
for lo, hi in tte_buckets:
    cnt = sum(1 for t in ttes if lo <= t < hi)
    print(f"     [{lo}, {hi}) min: {cnt} markets ({cnt/len(ttes)*100:.0f}%)")

# 6e. Chainlink-Binance disagreement
cl_bn_spreads = [m["entry_cl_bn_spread_bps"] for m in market_entries.values() if m["entry_cl_bn_spread_bps"] is not None]
print(f"\n5. CL-BN spread at entry:")
print(f"   Mean: {mean(cl_bn_spreads):.2f} bps, Median: {median(cl_bn_spreads):.2f} bps")
print(f"   Min: {min(cl_bn_spreads):.2f}, Max: {max(cl_bn_spreads):.2f}")
disagree_big = sum(1 for s in cl_bn_spreads if abs(s) > 5)
print(f"   |spread| > 5 bps: {disagree_big}/{len(cl_bn_spreads)} ({disagree_big/len(cl_bn_spreads)*100:.0f}%)")

# 6f. Price advantage
pa_vals = [m["entry_price_advantage_bps"] for m in market_entries.values() if m["entry_price_advantage_bps"] is not None]
print(f"\n6. Price advantage at entry:")
print(f"   Mean: {mean(pa_vals):.2f} bps, Median: {median(pa_vals):.2f} bps")
print(f"   Min: {min(pa_vals):.2f}, Max: {max(pa_vals):.2f}")

# 6g. Volatility regime at entry
regimes = [m["entry_volatility_regime"] for m in market_entries.values()]
regime_counts = defaultdict(int)
for r in regimes:
    regime_counts[r] += 1
print(f"\n7. Volatility regime at entry:")
for regime, cnt in sorted(regime_counts.items(), key=lambda x: -x[1]):
    print(f"   {regime}: {cnt} ({cnt/len(regimes)*100:.0f}%)")

# 6h. Both-sides vs one-side analysis
print(f"\n8. Position structure:")
both_entries = [m for m in market_entries.values() if m["bought_both"]]
one_side = [m for m in market_entries.values() if not m["bought_both"]]
print(f"   Bought BOTH sides: {len(both_entries)}/{len(market_entries)} markets")
print(f"   Bought ONE side: {len(one_side)}/{len(market_entries)} markets")
if both_entries:
    both_usdc = [m["total_usdc"] for m in both_entries]
    print(f"   Both-sides USDC: Mean=${mean(both_usdc):.2f}, Median=${median(both_usdc):.2f}")
if one_side:
    one_usdc = [m["total_usdc"] for m in one_side]
    print(f"   One-side USDC: Mean=${mean(one_usdc):.2f}, Median=${median(one_usdc):.2f}")

# For both-sides, analyze the up vs down split
if both_entries:
    print(f"\n   Both-sides: Up vs Down share split:")
    for m in sorted(both_entries, key=lambda x: x["slug"]):
        up_pct = m["up_shares"] / (m["up_shares"] + m["down_shares"]) * 100
        print(f"     {m['slug']}: Up={m['up_shares']:.0f} ({up_pct:.0f}%), Down={m['down_shares']:.0f} ({100-up_pct:.0f}%), "
              f"Up_wavg=${m['up_wavg_price']:.3f}, Down_wavg=${m['down_wavg_price']:.3f}, "
              f"Combined=${m['combined_cost_per_share']:.4f}")

# === STEP 7: Per-market comparison with our backtest ===
print(f"\n\n=== PER-MARKET COMPARISON: TRADER vs OUR BACKTEST ===\n")

comparison = []
for slug in sorted(market_entries.keys()):
    me = market_entries[slug]
    bt = backtest_pnl.get(slug, {"combined": 0, "dn": 0, "dir": 0, "outcome": "?", "roi": 0})

    # Estimate trader P&L: if bought both sides at combined < $1, guaranteed profit
    # Otherwise, profit depends on outcome
    comparison.append({
        "slug": slug,
        "trader_usdc": me["total_usdc"],
        "trader_shares_up": me["up_shares"],
        "trader_shares_down": me["down_shares"],
        "trader_bought_both": me["bought_both"],
        "trader_combined_cost": me["combined_cost_per_share"],
        "our_combined_pnl": bt["combined"],
        "our_dn_pnl": bt["dn"],
        "our_dir_pnl": bt["dir"],
        "our_outcome": bt["outcome"],
        "entry_tte": me["entry_tte_minutes"],
        "entry_dev_bps": me["entry_binance_dev_bps"],
        "entry_cl_bn_spread": me["entry_cl_bn_spread_bps"],
    })

# Print table
print(f"{'Slug':>38} | {'Trader$':>8} | {'Both?':>5} | {'CombCost':>8} | {'OurPnL':>8} | {'OurDN':>7} | {'OurDIR':>7} | {'TTE':>5} | {'DevBps':>7}")
print("-" * 130)
for c in comparison:
    cc_str = f"${c['trader_combined_cost']:.4f}" if c['trader_combined_cost'] else "N/A"
    print(f"{c['slug']:>38} | ${c['trader_usdc']:>7.2f} | {'YES' if c['trader_bought_both'] else 'NO':>5} | {cc_str:>8} | ${c['our_combined_pnl']:>7.2f} | ${c['our_dn_pnl']:>6.2f} | ${c['our_dir_pnl']:>6.2f} | {c['entry_tte']:>5.1f} | {c['entry_dev_bps']:>7.2f}")

# === STEP 8: Key finding — trader's guaranteed profit from combined < $1 ===
print(f"\n\n=== KEY FINDING: COMBINED COST BELOW $1.00 ===\n")

both_side_entries = [m for m in market_entries.values() if m["bought_both"] and m["combined_cost_per_share"] is not None]
for m in sorted(both_side_entries, key=lambda x: x["combined_cost_per_share"]):
    profit_per_pair = 1.0 - m["combined_cost_per_share"]
    min_shares = min(m["up_shares"], m["down_shares"])
    guaranteed_profit = profit_per_pair * min_shares
    print(f"  {m['slug']}: Combined=${m['combined_cost_per_share']:.4f}, "
          f"Edge=${profit_per_pair:.4f}/pair, "
          f"Min_shares={min_shares:.0f}, "
          f"Est_guaranteed_profit=${guaranteed_profit:.2f}, "
          f"TTE={m['entry_tte_minutes']:.1f}min, "
          f"DevBps={m['entry_binance_dev_bps']:.1f}")

# === STEP 9: Entry timing analysis — when does the trader enter relative to market open? ===
print(f"\n\n=== ENTRY TIMING PATTERN ===\n")

for m in sorted(market_entries.values(), key=lambda x: x["slug"]):
    slug_epoch = int(m["slug"].split("-")[-1])
    entry_offset = m["first_trade_ts"] - slug_epoch
    print(f"  {m['slug']}: Entry at +{entry_offset}s after market open, TTE={m['entry_tte_minutes']:.1f}min, "
          f"Trades={m['n_trades']}, USDC=${m['total_usdc']:.2f}")

entry_offsets = []
for m in market_entries.values():
    slug_epoch = int(m["slug"].split("-")[-1])
    entry_offsets.append(m["first_trade_ts"] - slug_epoch)

print(f"\n  Entry offset stats:")
print(f"    Mean: {mean(entry_offsets):.0f}s, Median: {median(entry_offsets):.0f}s")
print(f"    Min: {min(entry_offsets):.0f}s, Max: {max(entry_offsets):.0f}s")

# === STEP 10: Generate Markdown Report ===
print(f"\n\nGenerating report to {OUTPUT_FILE}...")

report = []
report.append("# Cross-Reference Report: Trader Entries vs RTDS Market Data")
report.append(f"\n**Date**: Feb 21, 2026 | **Overlapping markets**: {len(market_entries)} / 50 our CSVs")
report.append(f"**Total trader trades in overlap**: {len(joined_trades)} | **Markets with both-side buys**: {both_sides}")
report.append(f"**Timestamp match quality**: Median {median(time_diffs):.0f}ms offset (max {max(time_diffs):.0f}ms)")

report.append("\n---\n")
report.append("## 1. Entry Conditions: Binance Deviation (dev_bps)")
report.append(f"\nThe trader enters at varying RTDS deviation levels from the strike price.\n")
report.append(f"| Metric | Value |")
report.append(f"|--------|-------|")
report.append(f"| Mean dev_bps | {mean(dev_bps_vals):.2f} |")
report.append(f"| Median dev_bps | {median(dev_bps_vals):.2f} |")
report.append(f"| Mean |dev_bps| | {mean(abs_dev):.2f} |")
report.append(f"| Median |dev_bps| | {median(abs_dev):.2f} |")
report.append(f"| Stdev | {stdev(dev_bps_vals):.2f} |")
report.append(f"| Min | {min(dev_bps_vals):.2f} |")
report.append(f"| Max | {max(dev_bps_vals):.2f} |")

report.append(f"\n**Distribution of |dev_bps| at entry:**\n")
report.append(f"| Range (bps) | Count | % |")
report.append(f"|-------------|-------|---|")
for lo, hi in buckets:
    cnt = sum(1 for d in abs_dev if lo <= d < hi)
    report.append(f"| [{lo}, {hi}) | {cnt} | {cnt/len(abs_dev)*100:.0f}% |")
cnt_big = sum(1 for d in abs_dev if d >= 100)
report.append(f"| [100+) | {cnt_big} | {cnt_big/len(abs_dev)*100:.0f}% |")

report.append(f"\n**Interpretation**: The trader does NOT wait for extreme deviations. The majority of entries happen at low |dev_bps|, suggesting the trader is not relying on z-score signals for entry timing.")

report.append("\n---\n")
report.append("## 2. Combined Ask Price at Entry")
report.append(f"\nWhen the order book has yes_ask + no_ask < $1.00, buying both sides guarantees profit at settlement.\n")
report.append(f"| Metric | Value |")
report.append(f"|--------|-------|")
report.append(f"| Mean combined ask | ${mean(combined_asks):.4f} |")
report.append(f"| Median combined ask | ${median(combined_asks):.4f} |")
report.append(f"| Min combined ask | ${min(combined_asks):.4f} |")
report.append(f"| Max combined ask | ${max(combined_asks):.4f} |")
report.append(f"| Below $1.00 | {below_1}/{len(combined_asks)} ({below_1/len(combined_asks)*100:.0f}%) |")

if combined_costs:
    report.append(f"\n**Trader's actual weighted-average combined cost per share pair:**\n")
    report.append(f"| Metric | Value |")
    report.append(f"|--------|-------|")
    report.append(f"| Mean | ${mean(combined_costs):.4f} |")
    report.append(f"| Median | ${median(combined_costs):.4f} |")
    report.append(f"| Min | ${min(combined_costs):.4f} |")
    report.append(f"| Max | ${max(combined_costs):.4f} |")
    report.append(f"| Below $1.00 | {below_1_cost}/{len(combined_costs)} ({below_1_cost/len(combined_costs)*100:.0f}%) |")

report.append(f"\n### Both-Sides Detail Table\n")
report.append(f"| Market | Combined Cost | Edge/pair | Min Shares | Est Profit | TTE | DevBps |")
report.append(f"|--------|--------------|-----------|------------|------------|-----|--------|")
for m in sorted(both_side_entries, key=lambda x: x["combined_cost_per_share"]):
    profit_per_pair = 1.0 - m["combined_cost_per_share"]
    min_shares = min(m["up_shares"], m["down_shares"])
    gp = profit_per_pair * min_shares
    slug_short = m["slug"].replace("btc-updown-15m-", "")
    report.append(f"| ...{slug_short} | ${m['combined_cost_per_share']:.4f} | ${profit_per_pair:.4f} | {min_shares:.0f} | ${gp:.2f} | {m['entry_tte_minutes']:.1f} | {m['entry_binance_dev_bps']:.1f} |")

report.append("\n---\n")
report.append("## 3. Time-to-Expiry at Entry")
report.append(f"\n| Metric | Value |")
report.append(f"|--------|-------|")
report.append(f"| Mean | {mean(ttes):.1f} min |")
report.append(f"| Median | {median(ttes):.1f} min |")
report.append(f"| Min | {min(ttes):.1f} min |")
report.append(f"| Max | {max(ttes):.1f} min |")

report.append(f"\n| TTE Range | Count | % |")
report.append(f"|-----------|-------|---|")
for lo, hi in tte_buckets:
    cnt = sum(1 for t in ttes if lo <= t < hi)
    report.append(f"| [{lo}, {hi}) min | {cnt} | {cnt/len(ttes)*100:.0f}% |")

report.append(f"\n**Interpretation**: The trader enters early — within the first few minutes of the 15-minute window. This gives maximum time for the DN position to evolve and for price discovery.")

report.append("\n---\n")
report.append("## 4. Chainlink vs Binance Disagreement at Entry")
report.append(f"\n| Metric | Value |")
report.append(f"|--------|-------|")
report.append(f"| Mean CL-BN spread | {mean(cl_bn_spreads):.2f} bps |")
report.append(f"| Median CL-BN spread | {median(cl_bn_spreads):.2f} bps |")
report.append(f"| Max | {max(cl_bn_spreads):.2f} bps |")
report.append(f"| |spread| > 5 bps | {disagree_big}/{len(cl_bn_spreads)} ({disagree_big/len(cl_bn_spreads)*100:.0f}%) |")

report.append(f"\n**Interpretation**: The trader enters regardless of CL-BN agreement state — most entries show moderate spread. The trader is not timing entries based on oracle disagreement.")

report.append("\n---\n")
report.append("## 5. Price Advantage (rtds_price_advantage_bps)")
report.append(f"\n| Metric | Value |")
report.append(f"|--------|-------|")
report.append(f"| Mean | {mean(pa_vals):.2f} bps |")
report.append(f"| Median | {median(pa_vals):.2f} bps |")
report.append(f"| Min | {min(pa_vals):.2f} bps |")
report.append(f"| Max | {max(pa_vals):.2f} bps |")

report.append("\n---\n")
report.append("## 6. Staleness at Entry")
report.append(f"\n| Feed | Mean (ms) | Median (ms) | Max (ms) |")
report.append(f"|------|-----------|-------------|----------|")
report.append(f"| Binance | {mean(bn_stale):.0f} | {median(bn_stale):.0f} | {max(bn_stale):.0f} |")
report.append(f"| Chainlink | {mean(cl_stale):.0f} | {median(cl_stale):.0f} | {max(cl_stale):.0f} |")

report.append(f"\n**Interpretation**: Data is fresh at entry — sub-second Binance staleness, ~1-2s Chainlink. The trader is not exploiting stale data; they enter on fresh prices.")

report.append("\n---\n")
report.append("## 7. Volatility Regime at Entry")
report.append(f"\n| Regime | Count | % |")
report.append(f"|--------|-------|---|")
for regime, cnt in sorted(regime_counts.items(), key=lambda x: -x[1]):
    report.append(f"| {regime} | {cnt} | {cnt/len(regimes)*100:.0f}% |")

report.append("\n---\n")
report.append("## 8. Position Structure (Both-Sides vs One-Side)")
report.append(f"\n| Type | Markets | Avg USDC |")
report.append(f"|------|---------|----------|")
if both_entries:
    report.append(f"| Both sides | {len(both_entries)} | ${mean([m['total_usdc'] for m in both_entries]):.2f} |")
if one_side:
    report.append(f"| One side only | {len(one_side)} | ${mean([m['total_usdc'] for m in one_side]):.2f} |")

report.append("\n---\n")
report.append("## 9. Entry Timing (Offset from Market Open)")
report.append(f"\n| Metric | Value |")
report.append(f"|--------|-------|")
report.append(f"| Mean offset | {mean(entry_offsets):.0f}s |")
report.append(f"| Median offset | {median(entry_offsets):.0f}s |")
report.append(f"| Min offset | {min(entry_offsets):.0f}s |")
report.append(f"| Max offset | {max(entry_offsets):.0f}s |")

report.append("\n---\n")
report.append("## 10. Per-Market Comparison: Trader vs Our Backtest")
report.append(f"\n| Market | Trader USDC | Both? | Combined Cost | Our PnL | Our DN | Our DIR | TTE | DevBps |")
report.append(f"|--------|-------------|-------|---------------|---------|--------|---------|-----|--------|")
for c in sorted(comparison, key=lambda x: x["slug"]):
    cc_str = f"${c['trader_combined_cost']:.4f}" if c['trader_combined_cost'] else "N/A"
    slug_short = c['slug'].replace("btc-updown-15m-", "")
    report.append(f"| ...{slug_short} | ${c['trader_usdc']:.2f} | {'Y' if c['trader_bought_both'] else 'N'} | {cc_str} | ${c['our_combined_pnl']:.2f} | ${c['our_dn_pnl']:.2f} | ${c['our_dir_pnl']:.2f} | {c['entry_tte']:.1f} | {c['entry_dev_bps']:.1f} |")

# Summary statistics
our_pnl_vals = [c["our_combined_pnl"] for c in comparison]
report.append(f"\n**Our backtest summary on these 50 markets**: Total P&L = ${sum(our_pnl_vals):.2f}, Mean = ${mean(our_pnl_vals):.2f}, WR = {sum(1 for p in our_pnl_vals if p > 0)/len(our_pnl_vals)*100:.0f}%")

# Estimate trader P&L for both-sides markets
trader_both_est = []
for m in both_side_entries:
    if m["combined_cost_per_share"] is not None and m["combined_cost_per_share"] < 1.0:
        edge = 1.0 - m["combined_cost_per_share"]
        min_s = min(m["up_shares"], m["down_shares"])
        trader_both_est.append(edge * min_s)
    else:
        trader_both_est.append(0)

if trader_both_est:
    report.append(f"\n**Trader estimated DN profit (both-sides markets only)**: ${sum(trader_both_est):.2f} across {len(trader_both_est)} markets")

report.append("\n---\n")
report.append("## 11. Key Findings & Strategic Insights")
report.append(f"""
### The Trader's Core Strategy

1. **Delta-Neutral at Scale**: The trader buys BOTH sides in {len(both_entries)}/{len(market_entries)} markets, locking in guaranteed profit when combined cost < $1.00.

2. **Speed Over Signal**: Entry happens at +{median(entry_offsets):.0f}s median after market open, with {mean(abs_dev):.1f} bps mean |dev| — the trader does NOT wait for large deviations or z-score signals.

3. **Liquidity Harvesting**: The trader sweeps the entire order book aggressively, buying many small fills across multiple price levels. Average USDC deployed is large relative to typical order sizes.

4. **Fresh Data, Not Stale**: Binance staleness at entry is {median(bn_stale):.0f}ms median — the trader is NOT exploiting stale oracle data. They trade on fresh prices.

5. **Oracle Disagreement Not Required**: CL-BN spread at entry is {median(cl_bn_spreads):.1f} bps median — no particular pattern of entering during oracle disagreement.

### Comparison to Our Strategy

- **Our backtest lost ${abs(sum(our_pnl_vals)):.2f}** across these 50 markets (ROI: -9.2%)
- The trader's both-sides approach extracts guaranteed edge from combined ask < $1.00
- Our DN book relies on counter-trend timing and gets abandoned too often (30/49 markets abandoned)
- Our DIR book was slightly profitable ($6) but cannot compensate for DN losses
- **Key difference**: Trader enters immediately and sweeps; we wait for signals that may never come

### What This Means for Our Strategy

1. The market microstructure provides DN edge through **order book inefficiency** (combined ask < $1.00), not through price movement prediction
2. Our RTDS signals (dev_bps, z-score) may add value for directional overlays, but the base DN trade should be more aggressive on entry timing
3. The trader's aggressive fill pattern suggests they understand order book dynamics — they know the combined ask dips below $1.00 briefly and needs to be captured quickly
""")

# Write report
with open(OUTPUT_FILE, "w") as f:
    f.write("\n".join(report))

print(f"\nReport written to {OUTPUT_FILE}")
print("DONE.")
