#!/usr/bin/env python3
"""
Blushing-Fine Trader SELL Order Analysis v3
=============================================
Analyzes the directional decision-making of top Polymarket trader "Blushing-Fine"
by mapping their SELL orders to our RTDS z-score/market data.

The trader's strategy:
  1. At market open: BUY both Up and Down at ~$0.50 each (taker)
  2. Mid-window: SELL the losing side (maker) -- the directional decision
  3. End up with directional position tilted to winning side

This script answers: What market conditions trigger the SELL?
"""

import csv
import os
import statistics
from datetime import datetime
from collections import defaultdict

# ============================================================================
# CONFIGURATION
# ============================================================================
TRADER_CSV = '/home/lehash/development/poly-trader/trader1/filtered_events/btc_15min_trades.csv'
MARKET_DIR = '/home/lehash/development/combine-fetcher/221data/fixed/'
OUTPUT_REPORT = '/home/lehash/development/combine-fetcher/trader_study/sell_analysis_report.md'

# ============================================================================
# DATA LOADING
# ============================================================================

def load_trader_data():
    """Load trader CSV using csv.DictReader (title field has commas)."""
    trades = []
    with open(TRADER_CSV, 'r') as f:
        reader = csv.DictReader(f)
        for row in reader:
            ts_readable = row['timestamp_readable']
            if '2026-02-21' not in ts_readable:
                continue
            trades.append({
                'timestamp': int(row['timestamp']),
                'timestamp_readable': ts_readable,
                'type': row['type'],
                'size': float(row['size']),
                'usdc_size': float(row['usdc_size']),
                'price': float(row['price']),
                'side': row['side'],
                'outcome': row['outcome'],
                'event_slug': row['event_slug'],
            })
    return trades


def get_available_market_slugs():
    """Get set of slugs we have market data for."""
    slugs = set()
    for f in os.listdir(MARKET_DIR):
        if f.endswith('.csv'):
            slugs.add(f.replace('.csv', ''))
    return slugs


def load_market_data(slug):
    """Load market CSV data, returning list of rows sorted by timestamp."""
    filepath = os.path.join(MARKET_DIR, f'{slug}.csv')
    if not os.path.exists(filepath):
        return []

    rows = []
    with open(filepath, 'r') as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append(row)
    return rows


def parse_iso_to_unix(iso_str):
    """Parse ISO timestamp to unix seconds (float)."""
    try:
        if '.' in iso_str:
            dt = datetime.strptime(iso_str, '%Y-%m-%dT%H:%M:%S.%f')
        else:
            dt = datetime.strptime(iso_str, '%Y-%m-%dT%H:%M:%S')
        return dt.timestamp()
    except Exception:
        return None


def find_closest_market_row(market_rows, target_unix_ts, parsed_cache):
    """
    Find the market data row closest in time to the target timestamp.
    Uses binary search on the parsed timestamps for efficiency.
    """
    if not market_rows:
        return None

    if 'timestamps' not in parsed_cache:
        timestamps = []
        for row in market_rows:
            ts = parse_iso_to_unix(row['timestamp'])
            timestamps.append(ts)
        parsed_cache['timestamps'] = timestamps

    timestamps = parsed_cache['timestamps']

    lo, hi = 0, len(timestamps) - 1
    best_idx = 0
    best_diff = abs(timestamps[0] - target_unix_ts) if timestamps[0] else float('inf')

    while lo <= hi:
        mid = (lo + hi) // 2
        if timestamps[mid] is None:
            lo = mid + 1
            continue
        diff = abs(timestamps[mid] - target_unix_ts)
        if diff < best_diff:
            best_diff = diff
            best_idx = mid
        if timestamps[mid] < target_unix_ts:
            lo = mid + 1
        elif timestamps[mid] > target_unix_ts:
            hi = mid - 1
        else:
            break

    return market_rows[best_idx]


def compute_market_open_time(slug):
    """Extract market open unix timestamp from slug."""
    parts = slug.split('-')
    return int(parts[-1])


def vwap(trade_list):
    """Compute volume-weighted average price."""
    total_usdc = sum(t['usdc_size'] for t in trade_list)
    total_size = sum(t['size'] for t in trade_list)
    if total_size == 0:
        return 0, 0, 0
    return total_usdc / total_size, total_size, total_usdc


def percentile(sorted_list, pct):
    """Return the value at the given percentile from a sorted list."""
    if not sorted_list:
        return 0
    idx = int(len(sorted_list) * pct / 100)
    idx = min(idx, len(sorted_list) - 1)
    return sorted_list[idx]


# ============================================================================
# ANALYSIS
# ============================================================================

def analyze():
    """Main analysis function."""
    print("Loading trader data...")
    all_trades = load_trader_data()
    our_slugs = get_available_market_slugs()

    print(f"Total Feb 21 trades: {len(all_trades)}")
    print(f"Our market slugs: {len(our_slugs)}")

    # Group trades by event_slug
    trades_by_slug = defaultdict(list)
    for t in all_trades:
        trades_by_slug[t['event_slug']].append(t)

    # Filter to overlapping markets with sells
    overlap_slugs = set(trades_by_slug.keys()) & our_slugs
    sell_slugs = set()
    for slug in overlap_slugs:
        if any(t['side'] == 'SELL' for t in trades_by_slug[slug]):
            sell_slugs.add(slug)

    print(f"Overlapping slugs with sells: {len(sell_slugs)}")

    # ========================================================================
    # Per-market analysis
    # ========================================================================
    all_sell_events = []

    for slug in sorted(sell_slugs):
        trades = sorted(trades_by_slug[slug], key=lambda t: t['timestamp'])
        market_open = compute_market_open_time(slug)
        market_close = market_open + 900  # 15 min

        market_rows = load_market_data(slug)
        if not market_rows:
            continue
        parsed_cache = {}

        strike_price = float(market_rows[0].get('strike_price', 0))

        buys = [t for t in trades if t['side'] == 'BUY']
        sells = [t for t in trades if t['side'] == 'SELL']

        # Total buys (all time)
        buy_up_all = [t for t in buys if t['outcome'] == 'Up']
        buy_down_all = [t for t in buys if t['outcome'] == 'Down']
        buy_up_vwap_all, buy_up_size_all, buy_up_usdc_all = vwap(buy_up_all)
        buy_down_vwap_all, buy_down_size_all, buy_down_usdc_all = vwap(buy_down_all)
        total_buy_usdc = buy_up_usdc_all + buy_down_usdc_all

        # Cluster sells by outcome
        for outcome in ['Up', 'Down']:
            outcome_sells = [t for t in sells if t['outcome'] == outcome]
            if not outcome_sells:
                continue

            sell_total_size = sum(t['size'] for t in outcome_sells)
            sell_total_usdc = sum(t['usdc_size'] for t in outcome_sells)
            sell_avg_price = sell_total_usdc / sell_total_size if sell_total_size > 0 else 0

            first_sell_ts = min(t['timestamp'] for t in outcome_sells)
            last_sell_ts = max(t['timestamp'] for t in outcome_sells)

            tte_at_sell = (market_close - first_sell_ts) / 60.0
            time_into_market = (first_sell_ts - market_open) / 60.0

            # Compute position BEFORE first sell (only buys prior to first sell)
            buys_before_sell = [
                t for t in buys
                if t['outcome'] == outcome and t['timestamp'] < first_sell_ts
            ]
            pre_sell_vwap, pre_sell_size, pre_sell_usdc = vwap(buys_before_sell)

            # Also compute buys of OTHER side before this sell
            other_outcome = 'Down' if outcome == 'Up' else 'Up'
            other_buys_before = [
                t for t in buys
                if t['outcome'] == other_outcome and t['timestamp'] < first_sell_ts
            ]
            other_pre_vwap, other_pre_size, other_pre_usdc = vwap(other_buys_before)

            # Find closest market row at sell time
            market_row = find_closest_market_row(market_rows, first_sell_ts, parsed_cache)
            if market_row is None:
                continue

            rtds_dev_bps = float(market_row.get('rtds_binance_dev_bps', 0))
            tte_market = float(market_row.get('time_to_expiry_minutes', 0))
            yes_ask = float(market_row.get('yes_ask', 0))
            yes_bid = float(market_row.get('yes_bid', 0))
            no_ask = float(market_row.get('no_ask', 0))
            no_bid = float(market_row.get('no_bid', 0))
            vol_regime = market_row.get('volatility_regime', '')
            binance_price = float(market_row.get('binance_price', 0))
            chainlink_dev_bps = float(market_row.get('rtds_chainlink_dev_bps', 0))

            # NOTE: rtds_binance_dev_bps is the deviation between RTDS feed and live Binance
            # (near-zero, ~0.0007 bps). The actual z-score proxy is binance vs strike:
            dev_bps = ((binance_price - strike_price) / strike_price) * 10000 if strike_price else 0
            price_move_bps = dev_bps  # alias for clarity

            # Determine if selling the LOSING side
            # dev_bps > 0 => binance > strike => Up favored
            # dev_bps < 0 => binance < strike => Down favored
            if outcome == 'Up':
                selling_losing = dev_bps < 0
                buy_vwap_this_side = buy_up_vwap_all
                buy_size_this_side = buy_up_size_all
                buy_usdc_this_side = buy_up_usdc_all
                current_price = yes_bid
            else:
                selling_losing = dev_bps > 0
                buy_vwap_this_side = buy_down_vwap_all
                buy_size_this_side = buy_down_size_all
                buy_usdc_this_side = buy_down_usdc_all
                current_price = no_bid

            # Price drop using pre-sell VWAP (more accurate: what they paid before deciding to sell)
            if pre_sell_vwap > 0:
                price_drop = pre_sell_vwap - sell_avg_price
                price_drop_pct = (price_drop / pre_sell_vwap) * 100
            else:
                price_drop = 0
                price_drop_pct = 0

            # Loss: sell_proceeds - cost_of_shares_sold (using pre-sell VWAP)
            actual_loss = sell_total_usdc - (sell_total_size * pre_sell_vwap) if pre_sell_vwap > 0 else 0

            # Sell % relative to pre-sell position (what they held before deciding to sell)
            sell_pct_pre = (sell_total_size / pre_sell_size * 100) if pre_sell_size > 0 else 0
            # Also compute vs total buys for reference
            sell_pct_total = (sell_total_size / buy_size_this_side * 100) if buy_size_this_side > 0 else 0

            selling_against_zscore = (
                (outcome == 'Up' and dev_bps < 0) or
                (outcome == 'Down' and dev_bps > 0)
            )

            sell_event = {
                'slug': slug,
                'market_open': market_open,
                'outcome_sold': outcome,
                'first_sell_ts': first_sell_ts,
                'last_sell_ts': last_sell_ts,
                'time_into_market_min': time_into_market,
                'tte_at_sell_min': tte_at_sell,
                'tte_market_min': tte_market,
                'sell_total_size': sell_total_size,
                'sell_total_usdc': sell_total_usdc,
                'sell_avg_price': sell_avg_price,
                'pre_sell_vwap': pre_sell_vwap,
                'pre_sell_size': pre_sell_size,
                'pre_sell_usdc': pre_sell_usdc,
                'other_pre_sell_size': other_pre_size,
                'other_pre_sell_usdc': other_pre_usdc,
                'buy_vwap_this_side': buy_vwap_this_side,
                'buy_size_this_side': buy_size_this_side,
                'buy_usdc_this_side': buy_usdc_this_side,
                'price_drop': price_drop,
                'price_drop_pct': price_drop_pct,
                'actual_loss_usdc': actual_loss,
                'sell_pct_of_holdings': sell_pct_pre,
                'sell_pct_of_total': sell_pct_total,
                'dev_bps': dev_bps,
                'abs_dev_bps': abs(dev_bps),
                'rtds_feed_dev_bps': rtds_dev_bps,
                'chainlink_dev_bps': chainlink_dev_bps,
                'price_move_bps': price_move_bps,
                'selling_losing_side': selling_losing,
                'selling_against_zscore': selling_against_zscore,
                'vol_regime': vol_regime,
                'binance_price': binance_price,
                'strike_price': strike_price,
                'yes_bid': yes_bid,
                'yes_ask': yes_ask,
                'no_bid': no_bid,
                'no_ask': no_ask,
                'current_market_price': current_price,
                'buy_up_vwap': buy_up_vwap_all,
                'buy_down_vwap': buy_down_vwap_all,
                'buy_up_size': buy_up_size_all,
                'buy_down_size': buy_down_size_all,
                'buy_up_usdc': buy_up_usdc_all,
                'buy_down_usdc': buy_down_usdc_all,
                'total_buy_usdc': total_buy_usdc,
                'num_sell_orders': len(outcome_sells),
            }
            all_sell_events.append(sell_event)

    print(f"\nTotal sell events (per slug+outcome): {len(all_sell_events)}")

    # ========================================================================
    # Statistical Analysis
    # ========================================================================
    results = {}
    total = len(all_sell_events)

    # -- Q1: Dev_bps at sell --
    dev_bps_at_sell = [e['dev_bps'] for e in all_sell_events]
    abs_dev_bps_at_sell = sorted([e['abs_dev_bps'] for e in all_sell_events])

    results['dev_bps_distribution'] = {
        'mean': statistics.mean(dev_bps_at_sell),
        'median': statistics.median(dev_bps_at_sell),
        'stdev': statistics.stdev(dev_bps_at_sell) if len(dev_bps_at_sell) > 1 else 0,
        'min': min(dev_bps_at_sell),
        'max': max(dev_bps_at_sell),
        'abs_mean': statistics.mean(abs_dev_bps_at_sell),
        'abs_median': statistics.median(abs_dev_bps_at_sell),
        'abs_p10': percentile(abs_dev_bps_at_sell, 10),
        'abs_p25': percentile(abs_dev_bps_at_sell, 25),
        'abs_p75': percentile(abs_dev_bps_at_sell, 75),
        'abs_p90': percentile(abs_dev_bps_at_sell, 90),
    }

    # -- Q2: Timing --
    tte_values = [e['tte_at_sell_min'] for e in all_sell_events]
    time_into = [e['time_into_market_min'] for e in all_sell_events]

    early_sells = [e for e in all_sell_events if e['tte_at_sell_min'] > 10]
    mid_sells = [e for e in all_sell_events if 5 <= e['tte_at_sell_min'] <= 10]
    late_sells = [e for e in all_sell_events if e['tte_at_sell_min'] < 5]

    results['timing'] = {
        'tte_mean': statistics.mean(tte_values),
        'tte_median': statistics.median(tte_values),
        'tte_stdev': statistics.stdev(tte_values) if len(tte_values) > 1 else 0,
        'tte_min': min(tte_values),
        'tte_max': max(tte_values),
        'time_into_mean': statistics.mean(time_into),
        'time_into_median': statistics.median(time_into),
        'time_into_stdev': statistics.stdev(time_into) if len(time_into) > 1 else 0,
        'early_pct': len(early_sells) / total * 100,
        'mid_pct': len(mid_sells) / total * 100,
        'late_pct': len(late_sells) / total * 100,
        'early_count': len(early_sells),
        'mid_count': len(mid_sells),
        'late_count': len(late_sells),
    }

    # -- Q3: Direction --
    losing_sells = [e for e in all_sell_events if e['selling_losing_side']]
    against_zscore = [e for e in all_sell_events if e['selling_against_zscore']]

    results['direction'] = {
        'selling_losing_pct': len(losing_sells) / total * 100,
        'selling_losing_count': len(losing_sells),
        'selling_against_zscore_pct': len(against_zscore) / total * 100,
        'sells_up_count': len([e for e in all_sell_events if e['outcome_sold'] == 'Up']),
        'sells_down_count': len([e for e in all_sell_events if e['outcome_sold'] == 'Down']),
    }

    # -- Losing vs Winning side dev_bps --
    losing_dev_bps = [e['abs_dev_bps'] for e in all_sell_events if e['selling_losing_side']]
    winning_dev_bps = [e['abs_dev_bps'] for e in all_sell_events if not e['selling_losing_side']]

    results['losing_vs_winning'] = {
        'losing_abs_dev_mean': statistics.mean(losing_dev_bps) if losing_dev_bps else 0,
        'losing_abs_dev_median': statistics.median(losing_dev_bps) if losing_dev_bps else 0,
        'winning_abs_dev_mean': statistics.mean(winning_dev_bps) if winning_dev_bps else 0,
        'winning_abs_dev_median': statistics.median(winning_dev_bps) if winning_dev_bps else 0,
    }

    # -- Q4: Price drop --
    valid_sells = [e for e in all_sell_events if e['pre_sell_vwap'] > 0]
    price_drops = [e['price_drop'] for e in valid_sells]
    price_drop_pcts = [e['price_drop_pct'] for e in valid_sells]
    losses = [e['actual_loss_usdc'] for e in valid_sells]

    results['price_impact'] = {
        'price_drop_mean': statistics.mean(price_drops) if price_drops else 0,
        'price_drop_median': statistics.median(price_drops) if price_drops else 0,
        'price_drop_stdev': statistics.stdev(price_drops) if len(price_drops) > 1 else 0,
        'price_drop_pct_mean': statistics.mean(price_drop_pcts) if price_drop_pcts else 0,
        'price_drop_pct_median': statistics.median(price_drop_pcts) if price_drop_pcts else 0,
        'loss_usdc_mean': statistics.mean(losses) if losses else 0,
        'loss_usdc_median': statistics.median(losses) if losses else 0,
        'loss_usdc_total': sum(losses) if losses else 0,
        'count_positive_drop': len([d for d in price_drops if d > 0]),
        'count_negative_drop': len([d for d in price_drops if d < 0]),
        'count_zero': len([d for d in price_drops if d == 0]),
    }

    # -- Q5: Combined cost before vs after sells --
    combined_analysis = []
    for slug in sorted(sell_slugs):
        slug_events = [e for e in all_sell_events if e['slug'] == slug]
        if not slug_events:
            continue
        e = slug_events[0]

        min_size = min(e['buy_up_size'], e['buy_down_size'])
        if min_size == 0:
            continue

        combined_avg_before = (e['buy_up_usdc'] + e['buy_down_usdc']) / min_size

        total_sell_proceeds = sum(ev['sell_total_usdc'] for ev in slug_events)
        net_cost_after = e['total_buy_usdc'] - total_sell_proceeds

        remaining_up = e['buy_up_size'] - sum(
            ev['sell_total_size'] for ev in slug_events if ev['outcome_sold'] == 'Up'
        )
        remaining_down = e['buy_down_size'] - sum(
            ev['sell_total_size'] for ev in slug_events if ev['outcome_sold'] == 'Down'
        )
        remaining_min = min(remaining_up, remaining_down)

        if remaining_min > 0:
            combined_avg_after = net_cost_after / remaining_min
        else:
            remaining_max = max(remaining_up, remaining_down)
            combined_avg_after = net_cost_after / remaining_max if remaining_max > 0 else 0

        combined_analysis.append({
            'slug': slug,
            'combined_avg_before': combined_avg_before,
            'combined_avg_after': combined_avg_after,
            'improvement': combined_avg_before - combined_avg_after,
            'total_buy_usdc': e['total_buy_usdc'],
            'total_sell_proceeds': total_sell_proceeds,
            'net_cost': net_cost_after,
            'remaining_up': remaining_up,
            'remaining_down': remaining_down,
        })

    results['combined_cost'] = {
        'avg_before': statistics.mean([c['combined_avg_before'] for c in combined_analysis]) if combined_analysis else 0,
        'avg_after': statistics.mean([c['combined_avg_after'] for c in combined_analysis]) if combined_analysis else 0,
        'avg_improvement': statistics.mean([c['improvement'] for c in combined_analysis]) if combined_analysis else 0,
        'median_before': statistics.median([c['combined_avg_before'] for c in combined_analysis]) if combined_analysis else 0,
        'median_after': statistics.median([c['combined_avg_after'] for c in combined_analysis]) if combined_analysis else 0,
        'markets_analyzed': len(combined_analysis),
    }

    # -- Q6: Sell sizing --
    sell_pcts = [e['sell_pct_of_holdings'] for e in all_sell_events]
    full_sells = [e for e in all_sell_events if e['sell_pct_of_holdings'] >= 90]
    partial_sells = [e for e in all_sell_events if e['sell_pct_of_holdings'] < 90]

    results['sell_sizing'] = {
        'sell_pct_mean': statistics.mean(sell_pcts),
        'sell_pct_median': statistics.median(sell_pcts),
        'sell_pct_stdev': statistics.stdev(sell_pcts) if len(sell_pcts) > 1 else 0,
        'sell_pct_min': min(sell_pcts),
        'sell_pct_max': max(sell_pcts),
        'full_sell_count': len(full_sells),
        'partial_sell_count': len(partial_sells),
        'full_sell_pct': len(full_sells) / total * 100,
    }

    # -- Volatility regime --
    vol_counts = defaultdict(int)
    for e in all_sell_events:
        vol_counts[e['vol_regime']] += 1
    results['volatility'] = dict(vol_counts)

    # -- Dev_bps buckets --
    buckets = [
        ('< 1 bps', 0, 1),
        ('1-2 bps', 1, 2),
        ('2-3 bps', 2, 3),
        ('3-5 bps', 3, 5),
        ('5-7 bps', 5, 7),
        ('7-10 bps', 7, 10),
        ('10-15 bps', 10, 15),
        ('15-20 bps', 15, 20),
        ('20-30 bps', 20, 30),
        ('> 30 bps', 30, 9999),
    ]

    bucket_counts = {}
    for label, lo, hi in buckets:
        count = len([e for e in all_sell_events if lo <= e['abs_dev_bps'] < hi])
        bucket_counts[label] = count
    results['dev_bps_buckets'] = bucket_counts

    # -- Time-into-market buckets --
    time_buckets = [
        ('0-2 min', 0, 2),
        ('2-4 min', 2, 4),
        ('4-6 min', 4, 6),
        ('6-8 min', 6, 8),
        ('8-10 min', 8, 10),
        ('10-12 min', 10, 12),
        ('12-15 min', 12, 15),
    ]

    time_bucket_counts = {}
    for label, lo, hi in time_buckets:
        count = len([e for e in all_sell_events if lo <= e['time_into_market_min'] < hi])
        time_bucket_counts[label] = count
    results['time_buckets'] = time_bucket_counts

    # -- Both-side sell markets --
    both_side_markets = []
    for slug in sorted(sell_slugs):
        slug_events = [e for e in all_sell_events if e['slug'] == slug]
        outcomes_sold = set(e['outcome_sold'] for e in slug_events)
        if len(outcomes_sold) > 1:
            both_side_markets.append(slug)
    results['both_side_sells'] = {
        'count': len(both_side_markets),
        'slugs': both_side_markets,
    }

    # -- Correlation: |dev_bps| vs time_into_market --
    if len(all_sell_events) > 2:
        x = [e['abs_dev_bps'] for e in all_sell_events]
        y = [e['time_into_market_min'] for e in all_sell_events]
        n = len(x)
        mean_x = statistics.mean(x)
        mean_y = statistics.mean(y)
        cov = sum((x[i] - mean_x) * (y[i] - mean_y) for i in range(n)) / (n - 1)
        std_x = statistics.stdev(x)
        std_y = statistics.stdev(y)
        corr = cov / (std_x * std_y) if std_x > 0 and std_y > 0 else 0
        results['correlation_dev_time'] = round(corr, 4)

    # -- Correlation: |dev_bps| vs price_drop_pct --
    if len(valid_sells) > 2:
        x2 = [e['abs_dev_bps'] for e in valid_sells]
        y2 = [e['price_drop_pct'] for e in valid_sells]
        n2 = len(x2)
        mean_x2 = statistics.mean(x2)
        mean_y2 = statistics.mean(y2)
        cov2 = sum((x2[i] - mean_x2) * (y2[i] - mean_y2) for i in range(n2)) / (n2 - 1)
        std_x2 = statistics.stdev(x2)
        std_y2 = statistics.stdev(y2)
        corr2 = cov2 / (std_x2 * std_y2) if std_x2 > 0 and std_y2 > 0 else 0
        results['correlation_dev_loss'] = round(corr2, 4)

    # -- Subgroup: early sells (first 5 min) vs late (after 5 min) --
    early_events = [e for e in all_sell_events if e['time_into_market_min'] < 5]
    late_events = [e for e in all_sell_events if e['time_into_market_min'] >= 5]
    results['early_vs_late'] = {
        'early_count': len(early_events),
        'late_count': len(late_events),
        'early_abs_dev_mean': statistics.mean([e['abs_dev_bps'] for e in early_events]) if early_events else 0,
        'late_abs_dev_mean': statistics.mean([e['abs_dev_bps'] for e in late_events]) if late_events else 0,
        'early_abs_dev_median': statistics.median([e['abs_dev_bps'] for e in early_events]) if early_events else 0,
        'late_abs_dev_median': statistics.median([e['abs_dev_bps'] for e in late_events]) if late_events else 0,
        'early_losing_pct': (
            len([e for e in early_events if e['selling_losing_side']]) / len(early_events) * 100
            if early_events else 0
        ),
        'late_losing_pct': (
            len([e for e in late_events if e['selling_losing_side']]) / len(late_events) * 100
            if late_events else 0
        ),
        'early_price_drop_median': (
            statistics.median([e['price_drop_pct'] for e in early_events if e['pre_sell_vwap'] > 0])
            if [e for e in early_events if e['pre_sell_vwap'] > 0] else 0
        ),
        'late_price_drop_median': (
            statistics.median([e['price_drop_pct'] for e in late_events if e['pre_sell_vwap'] > 0])
            if [e for e in late_events if e['pre_sell_vwap'] > 0] else 0
        ),
    }

    sorted_events = sorted(all_sell_events, key=lambda e: e['abs_dev_bps'])

    # ========================================================================
    # Generate Report
    # ========================================================================
    generate_report(results, all_sell_events, combined_analysis, sorted_events, total)

    print(f"\nReport written to: {OUTPUT_REPORT}")
    return results, all_sell_events


def generate_report(results, all_sell_events, combined_analysis, sorted_events, total):
    """Generate the comprehensive markdown report."""

    lines = []

    def add(s=''):
        lines.append(s)

    add("# Blushing-Fine Trader SELL Order Analysis")
    add()
    add("**Generated**: 2026-02-23")
    add("**Data**: Feb 21, 2026 -- 28 overlapping BTC 15-min up/down markets")
    add("**Trader**: Blushing-Fine (proxy wallet 0x63ce...9a)")
    add("**Method**: Mapped trader SELL orders to our RTDS market data (sub-second resolution)")
    add()

    add("## Executive Summary")
    add()
    n_markets = len(set(e['slug'] for e in all_sell_events))
    add(f"Analysis of **{total} sell decisions** across **{n_markets} markets** on Feb 21, 2026,")
    add("mapped against our RTDS z-score and market data.")
    add()
    add("The trader (Blushing-Fine) follows a **delta-neutral-to-directional pivot** strategy:")
    add()
    add("1. **Open**: BUY both Up and Down at market open (taker)")
    add("2. **Pivot**: SELL the losing side mid-window (maker) -- the directional decision")
    add("3. **Hold**: Ride the remaining directional position to expiry")
    add()

    d = results['dev_bps_distribution']
    t = results['timing']
    dr = results['direction']
    pi = results['price_impact']
    ss = results['sell_sizing']

    add("### Key Findings at a Glance")
    add()
    add("| Question | Answer |")
    add("|----------|--------|")
    add(f"| At what dev_bps does the trader sell? | Median |dev_bps| = **{d['abs_median']:.2f} bps** (P25={d['abs_p25']:.2f}, P75={d['abs_p75']:.2f}) |")
    add(f"| When in the window? | Median **{t['time_into_median']:.1f} min** into market ({t['time_into_median']/15*100:.0f}% of window) |")
    add(f"| Sells the losing side? | **{dr['selling_losing_pct']:.1f}%** of the time |")
    add(f"| Typical loss on sell | **{pi['price_drop_pct_median']:.1f}%** per share (${pi['price_drop_median']:.3f} drop) |")
    add(f"| How much of holding sold? | Median **{ss['sell_pct_median']:.0f}%** of losing-side position |")
    add(f"| Correlation (dev_bps vs timing) | r = **{results.get('correlation_dev_time', 0):.4f}** |")
    add()

    add("---")
    add()

    # ====================================================================
    # Section 1: Dev_bps at sell
    # ====================================================================
    add("## 1. Z-Score / Dev_bps at Sell Time")
    add()
    add("`dev_bps = (binance_price - strike_price) / strike_price * 10000` -- how far BTC has moved from the strike.")
    add("Positive = above strike (Up favored). Negative = below strike (Down favored).")
    add()
    add("Note: This is NOT `rtds_binance_dev_bps` from our CSV (which measures inter-feed deviation, near zero).")
    add("This is the actual price-vs-strike deviation that determines market outcome.")
    add()
    add("### Distribution Statistics")
    add()
    add("| Metric | Value |")
    add("|--------|-------|")
    add(f"| Mean dev_bps (signed) | {d['mean']:+.2f} bps |")
    add(f"| Median dev_bps (signed) | {d['median']:+.2f} bps |")
    add(f"| Std Dev | {d['stdev']:.2f} bps |")
    add(f"| Range | [{d['min']:.2f}, {d['max']:.2f}] bps |")
    add(f"| **|dev_bps| Mean** | **{d['abs_mean']:.2f} bps** |")
    add(f"| **|dev_bps| Median** | **{d['abs_median']:.2f} bps** |")
    add(f"| |dev_bps| P10 | {d['abs_p10']:.2f} bps |")
    add(f"| |dev_bps| P25 | {d['abs_p25']:.2f} bps |")
    add(f"| |dev_bps| P75 | {d['abs_p75']:.2f} bps |")
    add(f"| |dev_bps| P90 | {d['abs_p90']:.2f} bps |")
    add()

    add("### Dev_bps Bucket Distribution")
    add()
    add("| |dev_bps| range | Count | Pct | Histogram |")
    add("|-----------------|-------|-----|-----------|")
    for label, count in results['dev_bps_buckets'].items():
        pct = count / total * 100
        bar = '#' * max(1, int(pct / 2)) if count > 0 else ''
        add(f"| {label} | {count} | {pct:.1f}% | {bar} |")
    add()

    add("### Interpretation")
    add()
    if d['abs_median'] < 5:
        add(f"The trader sells at remarkably small deviations (median {d['abs_median']:.1f} bps).")
        add("They do NOT wait for large z-score moves. They sell into small deviations,")
        add("suggesting they are reading the price trajectory/momentum rather than waiting for a threshold.")
    elif d['abs_median'] < 10:
        add(f"The trader waits for moderate deviation ({d['abs_median']:.1f} bps median) before selling.")
        add("This is a meaningful price movement but not an extreme threshold.")
    else:
        add(f"The trader waits for significant deviation ({d['abs_median']:.1f} bps) before pivoting.")
    add()

    # ====================================================================
    # Section 2: Timing
    # ====================================================================
    add("## 2. Sell Timing")
    add()
    add("### Time-to-Expiry at Sell")
    add()
    add("| Metric | Value |")
    add("|--------|-------|")
    add(f"| Mean TTE | {t['tte_mean']:.2f} min |")
    add(f"| Median TTE | {t['tte_median']:.2f} min |")
    add(f"| Std Dev | {t['tte_stdev']:.2f} min |")
    add(f"| Range | [{t['tte_min']:.2f}, {t['tte_max']:.2f}] min |")
    add()
    add("### Time Into Market (since open)")
    add()
    add("| Metric | Value |")
    add("|--------|-------|")
    add(f"| Mean time in | {t['time_into_mean']:.2f} min |")
    add(f"| Median time in | {t['time_into_median']:.2f} min |")
    add(f"| Std Dev | {t['time_into_stdev']:.2f} min |")
    add()

    add("### Timing Windows")
    add()
    add("| Window | TTE | Count | Pct |")
    add("|--------|-----|-------|-----|")
    add(f"| Early | > 10 min | {t['early_count']} | {t['early_pct']:.1f}% |")
    add(f"| Mid | 5-10 min | {t['mid_count']} | {t['mid_pct']:.1f}% |")
    add(f"| Late | < 5 min | {t['late_count']} | {t['late_pct']:.1f}% |")
    add()

    add("### Time-Into-Market Histogram")
    add()
    add("| Time since open | Count | Pct | Histogram |")
    add("|-----------------|-------|-----|-----------|")
    for label, count in results['time_buckets'].items():
        pct = count / total * 100
        bar = '#' * max(1, int(pct / 2)) if count > 0 else ''
        add(f"| {label} | {count} | {pct:.1f}% | {bar} |")
    add()

    add("### Interpretation")
    add()
    time_pct = t['time_into_median'] / 15 * 100
    add(f"The trader's median first-sell is at **{t['time_into_median']:.1f} min** ({time_pct:.0f}% of window).")
    if t['early_pct'] > 50:
        add("The majority sell early (>10min TTE). This is a quick-reaction trader.")
    elif t['late_pct'] > 50:
        add("The majority sell late (<5min TTE). This trader waits for certainty.")
    else:
        add("Sells are distributed across the window, suggesting condition-driven rather than timer-driven behavior.")
    add()

    # ====================================================================
    # Section 3: Directional analysis
    # ====================================================================
    add("## 3. Directional Analysis: Selling the Losing Side")
    add()
    add("| Metric | Value |")
    add("|--------|-------|")
    add(f"| Selling the LOSING side | **{dr['selling_losing_pct']:.1f}%** ({dr['selling_losing_count']}/{total}) |")
    add(f"| Selling AGAINST z-score direction | {dr['selling_against_zscore_pct']:.1f}% |")
    add(f"| Up sells | {dr['sells_up_count']} |")
    add(f"| Down sells | {dr['sells_down_count']} |")
    add()

    lw = results['losing_vs_winning']
    add("### Losing vs Winning Side Sells (|dev_bps| comparison)")
    add()
    add("| | Losing Side Sells | Winning Side Sells |")
    add("|--|---|---|")
    add(f"| Mean |dev_bps| | {lw['losing_abs_dev_mean']:.2f} bps | {lw['winning_abs_dev_mean']:.2f} bps |")
    add(f"| Median |dev_bps| | {lw['losing_abs_dev_median']:.2f} bps | {lw['winning_abs_dev_median']:.2f} bps |")
    add()

    add("### Interpretation")
    add()
    if dr['selling_losing_pct'] > 70:
        add(f"The trader overwhelmingly sells the **losing side** ({dr['selling_losing_pct']:.0f}%).")
        add("This confirms classic loss-cutting: they exit the unfavored outcome to ride the winner.")
    elif dr['selling_losing_pct'] > 50:
        add(f"The trader mostly sells the losing side ({dr['selling_losing_pct']:.0f}%), but with notable exceptions.")
    else:
        add(f"Only {dr['selling_losing_pct']:.0f}% of sells target the losing side -- suggesting a more nuanced strategy.")
    add()

    bs = results['both_side_sells']
    if bs['count'] > 0:
        add(f"**Both-side sells**: In {bs['count']} markets, the trader sold BOTH Up and Down:")
        for s in bs['slugs']:
            add(f"  - `{s}`")
        add()
        add("This indicates mid-market direction changes or active position rebalancing.")
        add()

    # ====================================================================
    # Section 4: Price drop
    # ====================================================================
    add("## 4. Price Drop from Buy to Sell (Loss Taken)")
    add()
    add("A positive price drop means the trader sold BELOW their buy VWAP (taking a loss).")
    add("A negative price drop means they sold ABOVE their buy VWAP (a gain).")
    add()
    add("| Metric | Value |")
    add("|--------|-------|")
    add(f"| Mean price drop (buy_vwap - sell_price) | ${pi['price_drop_mean']:.4f} |")
    add(f"| Median price drop | ${pi['price_drop_median']:.4f} |")
    add(f"| Std Dev | ${pi['price_drop_stdev']:.4f} |")
    add(f"| Mean price drop % | {pi['price_drop_pct_mean']:.1f}% |")
    add(f"| Median price drop % | {pi['price_drop_pct_median']:.1f}% |")
    add(f"| Sells at a LOSS (drop > 0) | {pi['count_positive_drop']} |")
    add(f"| Sells at a GAIN (drop < 0) | {pi['count_negative_drop']} |")
    add(f"| Sells at EVEN | {pi['count_zero']} |")
    add(f"| Mean loss per sell event (USDC) | ${pi['loss_usdc_mean']:.2f} |")
    add(f"| Median loss per sell event (USDC) | ${pi['loss_usdc_median']:.2f} |")
    add(f"| **Total P&L from all sells (USDC)** | **${pi['loss_usdc_total']:.2f}** |")
    add()

    add("### Interpretation")
    add()
    if pi['price_drop_median'] > 0:
        add(f"The trader typically takes a **${pi['price_drop_median']:.3f} loss per share** ({pi['price_drop_pct_median']:.1f}%) on sells.")
        add(f"This is the cost of pivoting from delta-neutral to directional.")
        add(f"Total USDC loss from selling: **${pi['loss_usdc_total']:.2f}** across {total} events.")
    else:
        add(f"The median price change is ${pi['price_drop_median']:.3f} -- some sells occur at favorable prices.")
    add()

    # ====================================================================
    # Section 5: Combined cost
    # ====================================================================
    add("## 5. Combined Cost Before vs After Sells")
    add()
    cc = results['combined_cost']
    add("The 'combined cost' represents the effective per-share-pair cost of the DN position.")
    add("Selling one side recovers capital and reduces the effective cost per remaining share.")
    add()
    add("| Metric | Value |")
    add("|--------|-------|")
    add(f"| Mean combined cost BEFORE sells | ${cc['avg_before']:.4f} |")
    add(f"| Mean combined cost AFTER sells | ${cc['avg_after']:.4f} |")
    add(f"| Median BEFORE | ${cc['median_before']:.4f} |")
    add(f"| Median AFTER | ${cc['median_after']:.4f} |")
    add(f"| Mean improvement | ${cc['avg_improvement']:.4f} |")
    add(f"| Markets analyzed | {cc['markets_analyzed']} |")
    add()

    add("### Per-Market Breakdown")
    add()
    add("| Market (epoch) | Before | After | Improvement | Net Cost | Remaining Up | Remaining Down |")
    add("|----------------|--------|-------|-------------|----------|-------------|---------------|")
    for c in sorted(combined_analysis, key=lambda x: x['improvement'], reverse=True):
        epoch = c['slug'].replace('btc-updown-15m-', '')
        add(f"| {epoch} | ${c['combined_avg_before']:.3f} | ${c['combined_avg_after']:.3f} | ${c['improvement']:+.3f} | ${c['net_cost']:.2f} | {c['remaining_up']:.0f} | {c['remaining_down']:.0f} |")
    add()

    # ====================================================================
    # Section 6: Sell sizing
    # ====================================================================
    add("## 6. Sell Size as Percentage of Pre-Sell Holdings")
    add()
    add("Sell % is computed as: total_shares_sold / shares_owned_before_first_sell.")
    add("Values >100% indicate interleaved buy-sell patterns (trader re-buys and re-sells within the same window).")
    add()
    add("| Metric | Value |")
    add("|--------|-------|")
    add(f"| Mean sell % | {ss['sell_pct_mean']:.1f}% |")
    add(f"| Median sell % | {ss['sell_pct_median']:.1f}% |")
    add(f"| Std Dev | {ss['sell_pct_stdev']:.1f}% |")
    add(f"| Range | [{ss['sell_pct_min']:.1f}%, {ss['sell_pct_max']:.1f}%] |")
    add(f"| Full sells (>=90%) | {ss['full_sell_count']} ({ss['full_sell_pct']:.1f}%) |")
    add(f"| Partial sells (<90%) | {ss['partial_sell_count']} |")
    add()

    add("### Interpretation")
    add()
    if ss['sell_pct_median'] >= 90:
        add(f"The trader does **full exits** (median {ss['sell_pct_median']:.0f}%) from the losing side.")
        add("They dump the entire position rather than scaling out.")
    elif ss['sell_pct_median'] >= 50:
        add(f"The trader sells the **majority** ({ss['sell_pct_median']:.0f}%) but keeps partial hedge.")
    else:
        add(f"The trader sells only a **portion** ({ss['sell_pct_median']:.0f}%) of their pre-sell position.")
        add("This suggests the directional pivot is **incremental**, not all-at-once.")
        add("The trader sells a fraction, observes market reaction, and may sell more later or re-buy.")
        add()
        add("**Important caveat**: Some markets show values >100% because the trader interleaves")
        add("buy-sell-buy-sell sequences within the same window (active market-making mixed with")
        add("directional pivoting). The simple pre-first-sell denominator understates their true activity.")
    add()

    # ====================================================================
    # Section 7: Volatility regime
    # ====================================================================
    add("## 7. Volatility Regime at Sell Time")
    add()
    add("| Regime | Count | Pct |")
    add("|--------|-------|-----|")
    for regime, count in sorted(results['volatility'].items()):
        pct = count / total * 100
        add(f"| {regime} | {count} | {pct:.1f}% |")
    add()

    # ====================================================================
    # Section 8: Early vs Late subgroup
    # ====================================================================
    add("## 8. Early vs Late Sells (Subgroup Comparison)")
    add()
    el = results['early_vs_late']
    add("Comparing sells that happen in the first 5 minutes vs after 5 minutes:")
    add()
    add("| Metric | Early (<5 min) | Late (>=5 min) |")
    add("|--------|---------------|----------------|")
    add(f"| Count | {el['early_count']} | {el['late_count']} |")
    add(f"| Mean |dev_bps| | {el['early_abs_dev_mean']:.2f} bps | {el['late_abs_dev_mean']:.2f} bps |")
    add(f"| Median |dev_bps| | {el['early_abs_dev_median']:.2f} bps | {el['late_abs_dev_median']:.2f} bps |")
    add(f"| % selling losing side | {el['early_losing_pct']:.1f}% | {el['late_losing_pct']:.1f}% |")
    add(f"| Median price drop % | {el['early_price_drop_median']:.1f}% | {el['late_price_drop_median']:.1f}% |")
    add()

    add("### Interpretation")
    add()
    if el['early_count'] > 0 and el['late_count'] > 0:
        if el['early_abs_dev_median'] > el['late_abs_dev_median']:
            add("Early sells happen at LARGER deviations -- the trader reacts quickly to big moves.")
        else:
            add("Late sells happen at LARGER deviations -- deviation accumulates over time.")
        if el['early_losing_pct'] > el['late_losing_pct']:
            add("Early sells are more likely to be cutting losses (higher losing-side %).")
        else:
            add("Late sells have a higher rate of selling the losing side.")
    add()

    # ====================================================================
    # Section 9: Correlations
    # ====================================================================
    add("## 9. Correlations")
    add()
    corr1 = results.get('correlation_dev_time', 0)
    corr2 = results.get('correlation_dev_loss', 0)
    add(f"| Relationship | Pearson r |")
    add(f"|-------------|-----------|")
    add(f"| |dev_bps| vs time_into_market | {corr1:+.4f} |")
    add(f"| |dev_bps| vs price_drop_pct | {corr2:+.4f} |")
    add()

    add("### Interpretation")
    add()
    if abs(corr1) < 0.2:
        add(f"**dev_bps vs timing** (r={corr1:+.4f}): Weak -- sell timing is not primarily driven by deviation magnitude.")
    elif corr1 > 0:
        add(f"**dev_bps vs timing** (r={corr1:+.4f}): Positive -- larger deviations accumulate over time; late sells see bigger moves.")
    else:
        add(f"**dev_bps vs timing** (r={corr1:+.4f}): Negative -- the trader reacts faster to bigger deviations.")
    add()

    if abs(corr2) < 0.2:
        add(f"**dev_bps vs loss** (r={corr2:+.4f}): Weak -- the loss taken does not strongly depend on deviation magnitude.")
    elif corr2 > 0:
        add(f"**dev_bps vs loss** (r={corr2:+.4f}): Positive -- larger deviations correlate with larger losses (deeper mispricing).")
    else:
        add(f"**dev_bps vs loss** (r={corr2:+.4f}): Negative -- larger deviations actually correspond to smaller losses (better sell prices).")
    add()

    # ====================================================================
    # Section 10: Detailed event table
    # ====================================================================
    add("## 10. Detailed Sell Event Table")
    add()
    add("Sorted by |dev_bps| ascending.")
    add()
    add("| # | Market (epoch) | Sold | dev_bps | Time In | TTE | Pre-Sell VWAP | Sell Avg | Drop % | Sell % | Losing? | Regime |")
    add("|---|----------------|------|---------|---------|-----|----------|----------|--------|--------|---------|--------|")
    for i, e in enumerate(sorted_events, 1):
        epoch = e['slug'].replace('btc-updown-15m-', '')
        losing = 'YES' if e['selling_losing_side'] else 'no'
        add(f"| {i} | {epoch} | {e['outcome_sold']} | {e['dev_bps']:+.2f} | {e['time_into_market_min']:.1f}m | {e['tte_at_sell_min']:.1f}m | ${e['pre_sell_vwap']:.3f} | ${e['sell_avg_price']:.3f} | {e['price_drop_pct']:+.1f}% | {e['sell_pct_of_holdings']:.0f}% | {losing} | {e['vol_regime']} |")
    add()

    # ====================================================================
    # Section 11: Parameter mapping
    # ====================================================================
    add("## 11. Parameter Mapping to Our Strategy")
    add()
    add("Mapping the trader's empirical sell behavior to our backtest parameter space.")
    add()

    no_trade_pct = t['time_into_median'] / 15 * 100

    add("### A. Sell Timing --> `dir_no_trade_before_pct`")
    add()
    add(f"The trader's median first-sell is at **{t['time_into_median']:.1f} min** into the 15-min window ({no_trade_pct:.0f}%).")
    add(f"Standard deviation: {t['time_into_stdev']:.1f} min.")
    add()
    add(f"**Recommended**: `dir_no_trade_before_pct = {no_trade_pct:.0f}`")
    add()
    add("However, the trader reacts to conditions, not a fixed timer. This should be a minimum gate,")
    add("not the sole trigger. The actual trigger is the dev_bps signal (see below).")
    add()

    add("### B. Dev_bps at Sell --> `dir_min_z_mid`, `dir_emergency_min_z`")
    add()
    add(f"| Percentile | |dev_bps| |")
    add(f"|------------|----------|")
    add(f"| P10 (rare small sells) | {d['abs_p10']:.2f} bps |")
    add(f"| P25 (lower quartile) | {d['abs_p25']:.2f} bps |")
    add(f"| P50 (median) | {d['abs_median']:.2f} bps |")
    add(f"| P75 (upper quartile) | {d['abs_p75']:.2f} bps |")
    add(f"| P90 (aggressive sells) | {d['abs_p90']:.2f} bps |")
    add()
    add("**Recommended**:")
    add(f"  - `dir_min_z_mid = {d['abs_p25']:.1f}` bps (minimum signal to consider a directional pivot)")
    add(f"  - `dir_emergency_min_z = {d['abs_p75']:.1f}` bps (strong signal for aggressive position change)")
    add()
    add("To convert to z-scores: divide by your volatility normalization factor.")
    add("For typical 15-min BTC vol (~0.3%), 1 bps ~ 0.03 z-score.")
    add()

    add("### C. Loss Tolerance --> `dn_abandon_pct`")
    add()
    add(f"The trader takes a median loss of **{pi['price_drop_pct_median']:.1f}%** per share when selling.")
    add(f"Mean loss: {pi['price_drop_pct_mean']:.1f}%.")
    add()
    add(f"**Recommended**: `dn_abandon_pct = {max(1, int(abs(pi['price_drop_pct_median'])))}`")
    add(f"(Abandon the DN hedge when the losing side has dropped this percentage from entry)")
    add()

    add("### D. Position Tilt --> Emergency Hedge Sizing")
    add()
    add(f"The trader sells a median **{ss['sell_pct_median']:.0f}%** of the losing side.")
    add()
    if ss['sell_pct_median'] >= 80:
        add("This is essentially a **full exit** from the losing side, converting to pure directional.")
        add("**Recommended**: `emergency_sell_pct = 100` (full exit of losing side)")
    elif ss['sell_pct_median'] >= 50:
        add(f"The trader retains ~{100 - ss['sell_pct_median']:.0f}% residual hedge.")
        add(f"**Recommended**: `emergency_sell_pct = {ss['sell_pct_median']:.0f}`")
    else:
        add(f"The trader keeps most of the position hedged.")
        add(f"**Recommended**: `emergency_sell_pct = {ss['sell_pct_median']:.0f}`")
    add()

    add("### E. Combined Parameter Set")
    add()
    add("```toml")
    add("# Derived from Blushing-Fine sell behavior analysis (Feb 21, 2026)")
    add(f"# Based on {total} sell events across {n_markets} markets")
    add()
    add(f"dir_no_trade_before_pct = {no_trade_pct:.0f}       # Minimum {t['time_into_median']:.1f}min into 15min window before directional pivot")
    add(f"dir_min_z_mid = {d['abs_p25']:.1f}                # Dev_bps threshold for directional consideration (P25)")
    add(f"dir_emergency_min_z = {d['abs_p75']:.1f}           # Dev_bps threshold for aggressive selling (P75)")
    add(f"dn_abandon_pct = {max(1, int(abs(pi['price_drop_pct_median'])))}                    # Abandon DN when side loses this % from entry")
    if ss['sell_pct_median'] >= 80:
        add(f"emergency_sell_pct = 100            # Full exit of losing side (trader does {ss['sell_pct_median']:.0f}%)")
    else:
        add(f"emergency_sell_pct = {ss['sell_pct_median']:.0f}             # Partial exit of losing side")
    add()
    add(f"# Supporting data:")
    add(f"# Trader's median sell price: ${pi['price_drop_median']:.3f} below buy VWAP")
    add(f"# Trader's total sell P&L: ${pi['loss_usdc_total']:.2f} across {total} events")
    add(f"# Correlation (dev_bps vs timing): r = {results.get('correlation_dev_time', 0):.4f}")
    add(f"# Losing-side sell rate: {dr['selling_losing_pct']:.1f}%")
    add(f"# Volatility regimes at sell: {dict(sorted(results['volatility'].items()))}")
    add("```")
    add()

    add("---")
    add()
    add("## Methodology")
    add()
    add("1. **Data join**: Trader's `event_slug` matched to our market CSV filenames")
    add("2. **Timestamp matching**: Binary search on ISO timestamps (sub-second market data vs 1-second trader data)")
    add("3. **VWAP calculation**: Volume-weighted average price across all fills per side per market")
    add("4. **Sell aggregation**: All sell orders for the same slug+outcome aggregated into one 'sell event'")
    add(f"5. **Coverage**: 28 of the trader's 56 sell-active markets overlap with our 50 market CSVs")
    add(f"6. **Total sell events**: {total} (some markets have sells on both sides)")
    add()
    add("### Limitations")
    add()
    add("- Market data has sub-second resolution; trader timestamps have 1-second resolution")
    add("- Only 28/56 sell-active markets overlap with our RTDS data")
    add("- Single day of data (Feb 21) -- strategy may vary across sessions")
    add("- dev_bps is raw deviation, not a vol-normalized z-score")
    add(f"- {bs['count']} markets had sells on BOTH sides, complicating the simple 'sell loser' model")
    add("- Buy VWAP includes all buys (early + mid-market), not just initial position")

    with open(OUTPUT_REPORT, 'w') as f:
        f.write('\n'.join(lines))


# ============================================================================
# MAIN
# ============================================================================
if __name__ == '__main__':
    results, events = analyze()

    print("\n" + "=" * 70)
    print("SUMMARY")
    print("=" * 70)
    d = results['dev_bps_distribution']
    t = results['timing']
    dr = results['direction']
    pi = results['price_impact']
    ss = results['sell_sizing']

    print(f"\nDev_bps at sell: median={d['abs_median']:.2f}, mean={d['abs_mean']:.2f}, P25={d['abs_p25']:.2f}, P75={d['abs_p75']:.2f}")
    print(f"Time into market: median={t['time_into_median']:.2f}min ({t['time_into_median']/15*100:.0f}% of window)")
    print(f"Selling losing side: {dr['selling_losing_pct']:.1f}%")
    print(f"Price drop median: ${pi['price_drop_median']:.4f} ({pi['price_drop_pct_median']:.1f}%)")
    print(f"Sell size median: {ss['sell_pct_median']:.1f}% of holdings")
    print(f"Correlation (|dev_bps| vs time): r={results.get('correlation_dev_time', 0):.4f}")
    print(f"Correlation (|dev_bps| vs loss): r={results.get('correlation_dev_loss', 0):.4f}")
    print(f"Total sell P&L: ${pi['loss_usdc_total']:.2f}")
