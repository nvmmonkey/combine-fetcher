#!/usr/bin/env python3
"""
Comprehensive analysis of Blushing-Fine's BTC 15min trading on Polymarket.
Focus: Feb 21 deep-dive with Feb 20/22 comparison.
"""

import csv
import json
import sys
from collections import defaultdict
from datetime import datetime, timezone, timedelta
from statistics import mean, median, stdev

CSV_PATH = "/home/lehash/development/poly-trader/trader1/filtered_events/btc_15min_trades.csv"

def parse_csv():
    """Parse CSV with proper csv.DictReader to handle commas in title field."""
    rows = []
    with open(CSV_PATH, 'r') as f:
        reader = csv.DictReader(f)
        for row in reader:
            # Parse key fields
            row['timestamp'] = int(row['timestamp'])
            row['size'] = float(row['size'])
            row['usdc_size'] = float(row['usdc_size'])
            row['price'] = float(row['price'])
            row['outcome_index'] = int(row['outcome_index'])
            # Parse readable timestamp
            row['dt'] = datetime.fromisoformat(row['timestamp_readable'].replace('.000', '+00:00'))
            row['date'] = row['dt'].strftime('%Y-%m-%d')
            # Extract market start time from event_slug (e.g., btc-updown-15m-1771601400)
            slug_parts = row['event_slug'].split('-')
            try:
                row['market_start_ts'] = int(slug_parts[-1])
                row['market_start_dt'] = datetime.fromtimestamp(row['market_start_ts'], tz=timezone.utc)
            except (ValueError, IndexError):
                row['market_start_ts'] = 0
                row['market_start_dt'] = row['dt']
            # Time into market window (seconds after market start)
            row['entry_delay_s'] = row['timestamp'] - row['market_start_ts']
            rows.append(row)
    return rows

def group_by_market(rows):
    """Group trades by condition_id (unique market)."""
    markets = defaultdict(list)
    for r in rows:
        markets[r['condition_id']].append(r)
    return markets

def group_by_date(rows):
    """Group trades by date."""
    dates = defaultdict(list)
    for r in rows:
        dates[r['date']].append(r)
    return dates

def analyze_position_structure(market_trades):
    """Analyze whether trader buys both sides, ratio, DN vs directional."""
    buys = [t for t in market_trades if t['side'] == 'BUY']
    sells = [t for t in market_trades if t['side'] == 'SELL']

    up_buys = [t for t in buys if t['outcome'] == 'Up']
    down_buys = [t for t in buys if t['outcome'] == 'Down']
    up_sells = [t for t in sells if t['outcome'] == 'Up']
    down_sells = [t for t in sells if t['outcome'] == 'Down']

    up_buy_usdc = sum(t['usdc_size'] for t in up_buys)
    down_buy_usdc = sum(t['usdc_size'] for t in down_buys)
    up_sell_usdc = sum(t['usdc_size'] for t in up_sells)
    down_sell_usdc = sum(t['usdc_size'] for t in down_sells)

    up_buy_shares = sum(t['size'] for t in up_buys)
    down_buy_shares = sum(t['size'] for t in down_buys)
    up_sell_shares = sum(t['size'] for t in up_sells)
    down_sell_shares = sum(t['size'] for t in down_sells)

    total_buy_usdc = up_buy_usdc + down_buy_usdc

    # Classify position type
    has_both = up_buy_usdc > 0 and down_buy_usdc > 0
    if has_both:
        ratio = up_buy_usdc / down_buy_usdc if down_buy_usdc > 0 else float('inf')
        if 0.2 < ratio < 5.0:
            pos_type = "BOTH_SIDES"
        elif ratio <= 0.2:
            pos_type = "DOWN_HEAVY"
        else:
            pos_type = "UP_HEAVY"
    elif up_buy_usdc > 0:
        pos_type = "UP_ONLY"
        ratio = float('inf')
    elif down_buy_usdc > 0:
        pos_type = "DOWN_ONLY"
        ratio = 0
    else:
        pos_type = "NO_BUYS"
        ratio = 0

    return {
        'pos_type': pos_type,
        'up_buy_usdc': up_buy_usdc,
        'down_buy_usdc': down_buy_usdc,
        'up_sell_usdc': up_sell_usdc,
        'down_sell_usdc': down_sell_usdc,
        'up_buy_shares': up_buy_shares,
        'down_buy_shares': down_buy_shares,
        'up_sell_shares': up_sell_shares,
        'down_sell_shares': down_sell_shares,
        'total_buy_usdc': total_buy_usdc,
        'total_sell_usdc': up_sell_usdc + down_sell_usdc,
        'has_both': has_both,
        'ratio_up_down': ratio,
        'num_buys': len(buys),
        'num_sells': len(sells),
        'num_up_buys': len(up_buys),
        'num_down_buys': len(down_buys),
    }

def analyze_entry_timing(market_trades):
    """Analyze when trader enters relative to market open."""
    delays = [t['entry_delay_s'] for t in market_trades if t['side'] == 'BUY']
    if not delays:
        return {'first_entry_s': None, 'last_entry_s': None, 'avg_entry_s': None, 'entry_spread_s': None}

    return {
        'first_entry_s': min(delays),
        'last_entry_s': max(delays),
        'avg_entry_s': mean(delays),
        'entry_spread_s': max(delays) - min(delays),
        'entries_within_60s': sum(1 for d in delays if d <= 60),
        'entries_within_120s': sum(1 for d in delays if d <= 120),
        'entries_after_5min': sum(1 for d in delays if d > 300),
    }

def analyze_prices(market_trades):
    """Analyze price distribution of buys."""
    buys = [t for t in market_trades if t['side'] == 'BUY']
    if not buys:
        return {}

    up_prices = [t['price'] for t in buys if t['outcome'] == 'Up']
    down_prices = [t['price'] for t in buys if t['outcome'] == 'Down']

    cheap_buys = [t for t in buys if t['price'] < 0.10]
    mid_buys = [t for t in buys if 0.10 <= t['price'] <= 0.80]
    expensive_buys = [t for t in buys if t['price'] > 0.80]

    return {
        'up_price_min': min(up_prices) if up_prices else None,
        'up_price_max': max(up_prices) if up_prices else None,
        'up_price_avg': mean(up_prices) if up_prices else None,
        'down_price_min': min(down_prices) if down_prices else None,
        'down_price_max': max(down_prices) if down_prices else None,
        'down_price_avg': mean(down_prices) if down_prices else None,
        'cheap_count': len(cheap_buys),
        'mid_count': len(mid_buys),
        'expensive_count': len(expensive_buys),
        'cheap_usdc': sum(t['usdc_size'] for t in cheap_buys),
        'mid_usdc': sum(t['usdc_size'] for t in mid_buys),
        'expensive_usdc': sum(t['usdc_size'] for t in expensive_buys),
    }

def detect_halt_gaps(rows_sorted, gap_threshold_min=30):
    """Find gaps in trading activity > threshold minutes."""
    if len(rows_sorted) < 2:
        return []

    gaps = []
    for i in range(1, len(rows_sorted)):
        delta = rows_sorted[i]['timestamp'] - rows_sorted[i-1]['timestamp']
        if delta > gap_threshold_min * 60:
            gaps.append({
                'gap_start': rows_sorted[i-1]['dt'].strftime('%Y-%m-%d %H:%M:%S UTC'),
                'gap_end': rows_sorted[i]['dt'].strftime('%Y-%m-%d %H:%M:%S UTC'),
                'gap_minutes': delta / 60,
                'gap_start_ts': rows_sorted[i-1]['timestamp'],
                'gap_end_ts': rows_sorted[i]['timestamp'],
            })
    return gaps

def compute_pnl_for_market(market_trades, market_outcome_resolved=None):
    """
    Estimate P&L for a market position.
    In binary markets, winning shares pay $1, losing shares pay $0.
    P&L = (shares_won * $1) - total_usdc_spent_buying + total_usdc_from_selling

    We don't have resolution data here, so we compute for both scenarios.
    """
    buys = [t for t in market_trades if t['side'] == 'BUY']
    sells = [t for t in market_trades if t['side'] == 'SELL']

    up_buy_shares = sum(t['size'] for t in buys if t['outcome'] == 'Up')
    down_buy_shares = sum(t['size'] for t in buys if t['outcome'] == 'Down')
    up_sell_shares = sum(t['size'] for t in sells if t['outcome'] == 'Up')
    down_sell_shares = sum(t['size'] for t in sells if t['outcome'] == 'Down')

    up_buy_cost = sum(t['usdc_size'] for t in buys if t['outcome'] == 'Up')
    down_buy_cost = sum(t['usdc_size'] for t in buys if t['outcome'] == 'Down')
    up_sell_revenue = sum(t['usdc_size'] for t in sells if t['outcome'] == 'Up')
    down_sell_revenue = sum(t['usdc_size'] for t in sells if t['outcome'] == 'Down')

    total_cost = up_buy_cost + down_buy_cost
    total_sell_revenue = up_sell_revenue + down_sell_revenue

    # Net shares held after sells
    net_up_shares = up_buy_shares - up_sell_shares
    net_down_shares = down_buy_shares - down_sell_shares

    # P&L if "Up" wins: Up shares pay $1 each, Down = $0
    pnl_if_up = net_up_shares * 1.0 - total_cost + total_sell_revenue
    # P&L if "Down" wins: Down shares pay $1 each, Up = $0
    pnl_if_down = net_down_shares * 1.0 - total_cost + total_sell_revenue

    # Combined avg cost per share (relevant for DN)
    total_shares = net_up_shares + net_down_shares
    combined_avg = total_cost / total_shares if total_shares > 0 else 0

    return {
        'net_up_shares': net_up_shares,
        'net_down_shares': net_down_shares,
        'total_cost': total_cost,
        'total_sell_revenue': total_sell_revenue,
        'pnl_if_up': pnl_if_up,
        'pnl_if_down': pnl_if_down,
        'combined_avg': combined_avg,
        'guaranteed_profit': pnl_if_up > 0 and pnl_if_down > 0,
        'max_pnl': max(pnl_if_up, pnl_if_down),
        'min_pnl': min(pnl_if_up, pnl_if_down),
    }

def main():
    print("Parsing CSV...")
    rows = parse_csv()
    print(f"Total rows: {len(rows)}")

    by_date = group_by_date(rows)
    print(f"Dates found: {sorted(by_date.keys())}")
    for d in sorted(by_date.keys()):
        print(f"  {d}: {len(by_date[d])} trades")

    # Sort all rows by timestamp
    rows.sort(key=lambda r: r['timestamp'])

    output = {}

    # =========================================================================
    # SECTION 1: Overall statistics per day
    # =========================================================================
    print("\n=== PER-DAY OVERVIEW ===")
    day_stats = {}
    for date in sorted(by_date.keys()):
        day_rows = by_date[date]
        day_markets = group_by_market(day_rows)

        buys = [t for t in day_rows if t['side'] == 'BUY']
        sells = [t for t in day_rows if t['side'] == 'SELL']

        total_buy_usdc = sum(t['usdc_size'] for t in buys)
        total_sell_usdc = sum(t['usdc_size'] for t in sells)

        # Side breakdown
        up_buys = sum(t['usdc_size'] for t in buys if t['outcome'] == 'Up')
        down_buys = sum(t['usdc_size'] for t in buys if t['outcome'] == 'Down')

        stats = {
            'num_trades': len(day_rows),
            'num_markets': len(day_markets),
            'num_buys': len(buys),
            'num_sells': len(sells),
            'total_buy_usdc': round(total_buy_usdc, 2),
            'total_sell_usdc': round(total_sell_usdc, 2),
            'up_buy_usdc': round(up_buys, 2),
            'down_buy_usdc': round(down_buys, 2),
            'avg_usdc_per_market': round(total_buy_usdc / len(day_markets), 2) if day_markets else 0,
            'first_trade': day_rows[0]['timestamp_readable'] if day_rows else None,
            'last_trade': day_rows[-1]['timestamp_readable'] if day_rows else None,
        }
        day_stats[date] = stats
        print(f"\n--- {date} ---")
        for k, v in stats.items():
            print(f"  {k}: {v}")

    output['day_stats'] = day_stats

    # =========================================================================
    # SECTION 2: Position structure analysis (per market, per day)
    # =========================================================================
    print("\n=== POSITION STRUCTURE ===")
    pos_structure_by_day = {}
    for date in sorted(by_date.keys()):
        day_markets = group_by_market(by_date[date])
        structures = {}
        type_counts = defaultdict(int)
        for cid, trades in day_markets.items():
            ps = analyze_position_structure(trades)
            structures[cid] = ps
            type_counts[ps['pos_type']] += 1

        pos_structure_by_day[date] = {
            'type_distribution': dict(type_counts),
            'both_sides_count': type_counts.get('BOTH_SIDES', 0),
            'up_only_count': type_counts.get('UP_ONLY', 0),
            'down_only_count': type_counts.get('DOWN_ONLY', 0),
        }

        # For BOTH_SIDES markets, analyze the ratio distributions
        both_sides = {cid: s for cid, s in structures.items() if s['pos_type'] == 'BOTH_SIDES'}
        if both_sides:
            ratios = [s['ratio_up_down'] for s in both_sides.values() if s['ratio_up_down'] != float('inf')]
            if ratios:
                pos_structure_by_day[date]['ratio_up_down_avg'] = round(mean(ratios), 4)
                pos_structure_by_day[date]['ratio_up_down_median'] = round(median(ratios), 4)

        print(f"\n--- {date} ---")
        print(f"  Position types: {dict(type_counts)}")
        if both_sides:
            print(f"  Both-sides markets: {len(both_sides)}")
            if ratios:
                print(f"  Up/Down USDC ratio avg: {mean(ratios):.4f}, median: {median(ratios):.4f}")

    output['position_structure'] = pos_structure_by_day

    # =========================================================================
    # SECTION 3: Entry timing analysis
    # =========================================================================
    print("\n=== ENTRY TIMING ===")
    timing_by_day = {}
    for date in sorted(by_date.keys()):
        day_markets = group_by_market(by_date[date])
        all_first_entries = []
        all_delays = []

        for cid, trades in day_markets.items():
            timing = analyze_entry_timing(trades)
            if timing['first_entry_s'] is not None:
                all_first_entries.append(timing['first_entry_s'])
                all_delays.extend([t['entry_delay_s'] for t in trades if t['side'] == 'BUY'])

        if all_first_entries:
            timing_by_day[date] = {
                'avg_first_entry_s': round(mean(all_first_entries), 1),
                'median_first_entry_s': round(median(all_first_entries), 1),
                'min_first_entry_s': min(all_first_entries),
                'max_first_entry_s': max(all_first_entries),
                'pct_entering_within_60s': round(100 * sum(1 for d in all_first_entries if d <= 60) / len(all_first_entries), 1),
                'pct_entering_within_120s': round(100 * sum(1 for d in all_first_entries if d <= 120) / len(all_first_entries), 1),
                'pct_entering_after_5min': round(100 * sum(1 for d in all_first_entries if d > 300) / len(all_first_entries), 1),
                'all_trades_avg_delay_s': round(mean(all_delays), 1),
            }
            print(f"\n--- {date} ---")
            for k, v in timing_by_day[date].items():
                print(f"  {k}: {v}")

    output['entry_timing'] = timing_by_day

    # =========================================================================
    # SECTION 4: Price level analysis
    # =========================================================================
    print("\n=== PRICE LEVELS ===")
    price_analysis_by_day = {}
    for date in sorted(by_date.keys()):
        day_buys = [t for t in by_date[date] if t['side'] == 'BUY']

        up_prices = [t['price'] for t in day_buys if t['outcome'] == 'Up']
        down_prices = [t['price'] for t in day_buys if t['outcome'] == 'Down']
        all_prices = [t['price'] for t in day_buys]

        # Price buckets
        buckets = {'<0.05': 0, '0.05-0.10': 0, '0.10-0.20': 0, '0.20-0.40': 0,
                    '0.40-0.60': 0, '0.60-0.80': 0, '0.80-0.95': 0, '>0.95': 0}
        bucket_usdc = {'<0.05': 0, '0.05-0.10': 0, '0.10-0.20': 0, '0.20-0.40': 0,
                       '0.40-0.60': 0, '0.60-0.80': 0, '0.80-0.95': 0, '>0.95': 0}
        for t in day_buys:
            p = t['price']
            if p < 0.05:
                b = '<0.05'
            elif p < 0.10:
                b = '0.05-0.10'
            elif p < 0.20:
                b = '0.10-0.20'
            elif p < 0.40:
                b = '0.20-0.40'
            elif p < 0.60:
                b = '0.40-0.60'
            elif p < 0.80:
                b = '0.60-0.80'
            elif p < 0.95:
                b = '0.80-0.95'
            else:
                b = '>0.95'
            buckets[b] += 1
            bucket_usdc[b] += t['usdc_size']

        price_analysis_by_day[date] = {
            'up_price_avg': round(mean(up_prices), 4) if up_prices else None,
            'up_price_median': round(median(up_prices), 4) if up_prices else None,
            'down_price_avg': round(mean(down_prices), 4) if down_prices else None,
            'down_price_median': round(median(down_prices), 4) if down_prices else None,
            'overall_price_avg': round(mean(all_prices), 4) if all_prices else None,
            'price_buckets_count': {k: v for k, v in buckets.items() if v > 0},
            'price_buckets_usdc': {k: round(v, 2) for k, v in bucket_usdc.items() if v > 0},
        }

        print(f"\n--- {date} ---")
        print(f"  Up avg price: {mean(up_prices):.4f}" if up_prices else "  No Up buys")
        print(f"  Down avg price: {mean(down_prices):.4f}" if down_prices else "  No Down buys")
        print(f"  Price buckets (count): {dict((k,v) for k,v in buckets.items() if v > 0)}")
        print(f"  Price buckets (USDC):  {dict((k,round(v,2)) for k,v in bucket_usdc.items() if v > 0)}")

    output['price_analysis'] = price_analysis_by_day

    # =========================================================================
    # SECTION 5: Position sizing analysis
    # =========================================================================
    print("\n=== POSITION SIZING ===")
    sizing_by_day = {}
    for date in sorted(by_date.keys()):
        day_markets = group_by_market(by_date[date])
        market_sizes = []
        for cid, trades in day_markets.items():
            buys = [t for t in trades if t['side'] == 'BUY']
            market_usdc = sum(t['usdc_size'] for t in buys)
            market_sizes.append(market_usdc)

        if market_sizes:
            sizing_by_day[date] = {
                'avg_usdc_per_market': round(mean(market_sizes), 2),
                'median_usdc_per_market': round(median(market_sizes), 2),
                'min_usdc_per_market': round(min(market_sizes), 2),
                'max_usdc_per_market': round(max(market_sizes), 2),
                'stdev_usdc_per_market': round(stdev(market_sizes), 2) if len(market_sizes) > 1 else 0,
                'total_deployed': round(sum(market_sizes), 2),
                'markets_over_500': sum(1 for s in market_sizes if s > 500),
                'markets_over_1000': sum(1 for s in market_sizes if s > 1000),
                'markets_under_100': sum(1 for s in market_sizes if s < 100),
            }
            print(f"\n--- {date} ---")
            for k, v in sizing_by_day[date].items():
                print(f"  {k}: {v}")

    output['position_sizing'] = sizing_by_day

    # =========================================================================
    # SECTION 6: Sell behavior analysis
    # =========================================================================
    print("\n=== SELL BEHAVIOR ===")
    sell_analysis_by_day = {}
    for date in sorted(by_date.keys()):
        sells = [t for t in by_date[date] if t['side'] == 'SELL']
        buys = [t for t in by_date[date] if t['side'] == 'BUY']

        if sells:
            sell_prices = [t['price'] for t in sells]
            sell_outcomes = defaultdict(int)
            for s in sells:
                sell_outcomes[s['outcome']] += 1

            sell_analysis_by_day[date] = {
                'num_sells': len(sells),
                'num_buys': len(buys),
                'sell_to_buy_ratio': round(len(sells) / len(buys), 4) if buys else 0,
                'total_sell_usdc': round(sum(t['usdc_size'] for t in sells), 2),
                'avg_sell_price': round(mean(sell_prices), 4),
                'sell_outcome_breakdown': dict(sell_outcomes),
                'sell_price_range': f"{min(sell_prices):.4f} - {max(sell_prices):.4f}",
            }
        else:
            sell_analysis_by_day[date] = {
                'num_sells': 0,
                'num_buys': len(buys),
                'note': 'NO SELLS on this day'
            }

        print(f"\n--- {date} ---")
        for k, v in sell_analysis_by_day[date].items():
            print(f"  {k}: {v}")

    output['sell_behavior'] = sell_analysis_by_day

    # =========================================================================
    # SECTION 7: P&L analysis per market
    # =========================================================================
    print("\n=== P&L ANALYSIS ===")
    pnl_by_day = {}
    for date in sorted(by_date.keys()):
        day_markets = group_by_market(by_date[date])
        guaranteed_profits = 0
        guaranteed_losses = 0
        total_min_pnl = 0
        total_max_pnl = 0
        market_pnls = []

        for cid, trades in day_markets.items():
            pnl = compute_pnl_for_market(trades)
            market_pnls.append({
                'condition_id': cid,
                'title': trades[0]['title'],
                'market_start': trades[0].get('market_start_dt', trades[0]['dt']).strftime('%H:%M'),
                **pnl
            })
            if pnl['guaranteed_profit']:
                guaranteed_profits += 1
            if pnl['max_pnl'] < 0:
                guaranteed_losses += 1
            total_min_pnl += pnl['min_pnl']
            total_max_pnl += pnl['max_pnl']

        # Sort by min_pnl to find worst markets
        market_pnls.sort(key=lambda x: x['min_pnl'])

        combined_avgs = [m['combined_avg'] for m in market_pnls if m['combined_avg'] > 0]

        pnl_by_day[date] = {
            'num_markets': len(day_markets),
            'guaranteed_profits': guaranteed_profits,
            'guaranteed_losses': guaranteed_losses,
            'total_min_pnl': round(total_min_pnl, 2),
            'total_max_pnl': round(total_max_pnl, 2),
            'avg_combined_avg': round(mean(combined_avgs), 4) if combined_avgs else None,
            'median_combined_avg': round(median(combined_avgs), 4) if combined_avgs else None,
            'worst_5_markets': [(m['market_start'], m['title'][:50], round(m['min_pnl'], 2), round(m['max_pnl'], 2), round(m['combined_avg'], 4)) for m in market_pnls[:5]],
            'best_5_markets': [(m['market_start'], m['title'][:50], round(m['min_pnl'], 2), round(m['max_pnl'], 2), round(m['combined_avg'], 4)) for m in market_pnls[-5:]],
        }

        print(f"\n--- {date} ---")
        print(f"  Markets: {len(day_markets)}")
        print(f"  Guaranteed profits: {guaranteed_profits}")
        print(f"  Guaranteed losses: {guaranteed_losses}")
        print(f"  Total min P&L: ${total_min_pnl:.2f}")
        print(f"  Total max P&L: ${total_max_pnl:.2f}")
        if combined_avgs:
            print(f"  Avg combined avg cost: ${mean(combined_avgs):.4f}")
            print(f"  Median combined avg cost: ${median(combined_avgs):.4f}")
        print(f"  Worst 5 markets (by min P&L):")
        for m in market_pnls[:5]:
            print(f"    {m['market_start']} | min=${m['min_pnl']:.2f} max=${m['max_pnl']:.2f} | comb_avg=${m['combined_avg']:.4f}")
        print(f"  Best 5 markets (by min P&L):")
        for m in market_pnls[-5:]:
            print(f"    {m['market_start']} | min=${m['min_pnl']:.2f} max=${m['max_pnl']:.2f} | comb_avg=${m['combined_avg']:.4f}")

    output['pnl_analysis'] = pnl_by_day

    # =========================================================================
    # SECTION 8: Halt/Gap detection (Feb 21 focus)
    # =========================================================================
    print("\n=== HALT/GAP DETECTION ===")
    gap_analysis = {}
    for date in sorted(by_date.keys()):
        day_rows = sorted(by_date[date], key=lambda r: r['timestamp'])
        gaps = detect_halt_gaps(day_rows, gap_threshold_min=20)

        gap_analysis[date] = {
            'num_gaps_20min': len(gaps),
            'gaps': gaps,
            'trading_window': f"{day_rows[0]['dt'].strftime('%H:%M')} - {day_rows[-1]['dt'].strftime('%H:%M')} UTC" if day_rows else None,
        }

        print(f"\n--- {date} ---")
        print(f"  Trading window: {gap_analysis[date]['trading_window']}")
        print(f"  Gaps > 20min: {len(gaps)}")
        for g in gaps:
            print(f"    {g['gap_start']} → {g['gap_end']} ({g['gap_minutes']:.0f} min)")

    output['gap_analysis'] = gap_analysis

    # =========================================================================
    # SECTION 9: Feb 21 deep-dive — hourly activity pattern
    # =========================================================================
    print("\n=== FEB 21 HOURLY ACTIVITY ===")
    if '2026-02-21' in by_date:
        feb21 = sorted(by_date['2026-02-21'], key=lambda r: r['timestamp'])
        hourly = defaultdict(lambda: {'trades': 0, 'buy_usdc': 0, 'sell_usdc': 0, 'markets': set()})

        for t in feb21:
            hour = t['dt'].strftime('%H:00')
            hourly[hour]['trades'] += 1
            if t['side'] == 'BUY':
                hourly[hour]['buy_usdc'] += t['usdc_size']
            else:
                hourly[hour]['sell_usdc'] += t['usdc_size']
            hourly[hour]['markets'].add(t['condition_id'])

        print(f"{'Hour':>6} | {'Trades':>6} | {'Buy USDC':>10} | {'Sell USDC':>10} | {'Markets':>7}")
        print("-" * 55)
        hourly_data = []
        for hour in sorted(hourly.keys()):
            h = hourly[hour]
            print(f"{hour:>6} | {h['trades']:>6} | ${h['buy_usdc']:>9.2f} | ${h['sell_usdc']:>9.2f} | {len(h['markets']):>7}")
            hourly_data.append({
                'hour': hour,
                'trades': h['trades'],
                'buy_usdc': round(h['buy_usdc'], 2),
                'sell_usdc': round(h['sell_usdc'], 2),
                'markets': len(h['markets'])
            })

        output['feb21_hourly'] = hourly_data

    # =========================================================================
    # SECTION 10: DN pattern detection — cheap up + expensive down (or vice versa)
    # =========================================================================
    print("\n=== DELTA-NEUTRAL PATTERN DETECTION ===")
    dn_analysis_by_day = {}
    for date in sorted(by_date.keys()):
        day_markets = group_by_market(by_date[date])
        dn_markets = 0
        dir_markets = 0
        mixed_markets = 0

        dn_details = []

        for cid, trades in day_markets.items():
            buys = [t for t in trades if t['side'] == 'BUY']
            up_buys = [t for t in buys if t['outcome'] == 'Up']
            down_buys = [t for t in buys if t['outcome'] == 'Down']

            if not up_buys or not down_buys:
                dir_markets += 1
                continue

            avg_up_price = mean(t['price'] for t in up_buys)
            avg_down_price = mean(t['price'] for t in down_buys)

            up_usdc = sum(t['usdc_size'] for t in up_buys)
            down_usdc = sum(t['usdc_size'] for t in down_buys)

            # Classic DN: cheap on one side, expensive on the other
            # Combined avg < $1 means guaranteed profit
            pnl = compute_pnl_for_market(trades)

            if pnl['guaranteed_profit']:
                dn_markets += 1
                dn_details.append({
                    'market_start': trades[0].get('market_start_dt', trades[0]['dt']).strftime('%H:%M'),
                    'avg_up_price': round(avg_up_price, 4),
                    'avg_down_price': round(avg_down_price, 4),
                    'combined_avg': round(pnl['combined_avg'], 4),
                    'guaranteed_min_pnl': round(pnl['min_pnl'], 2),
                })
            else:
                mixed_markets += 1

        dn_analysis_by_day[date] = {
            'dn_guaranteed_profit': dn_markets,
            'directional_only': dir_markets,
            'both_sides_not_guaranteed': mixed_markets,
            'total_markets': len(day_markets),
            'dn_pct': round(100 * dn_markets / len(day_markets), 1) if day_markets else 0,
        }

        if dn_details:
            dn_analysis_by_day[date]['sample_dn_markets'] = dn_details[:10]

        print(f"\n--- {date} ---")
        print(f"  DN (guaranteed profit): {dn_markets}")
        print(f"  Directional (one side only): {dir_markets}")
        print(f"  Both sides but NOT guaranteed: {mixed_markets}")
        print(f"  DN %: {dn_analysis_by_day[date]['dn_pct']}%")
        if dn_details:
            print(f"  Sample DN markets:")
            for d in dn_details[:5]:
                print(f"    {d['market_start']} | up_avg=${d['avg_up_price']:.4f} down_avg=${d['avg_down_price']:.4f} | comb_avg=${d['combined_avg']:.4f} | min_pnl=${d['guaranteed_min_pnl']:.2f}")

    output['dn_analysis'] = dn_analysis_by_day

    # =========================================================================
    # SECTION 11: Feb 21 specifically — market-by-market timeline
    # =========================================================================
    print("\n=== FEB 21 MARKET-BY-MARKET TIMELINE ===")
    if '2026-02-21' in by_date:
        feb21_markets = group_by_market(by_date['2026-02-21'])
        market_timeline = []

        for cid, trades in sorted(feb21_markets.items(), key=lambda x: x[1][0]['market_start_ts']):
            ps = analyze_position_structure(trades)
            pnl = compute_pnl_for_market(trades)
            timing = analyze_entry_timing(trades)

            entry = {
                'market_start': trades[0].get('market_start_dt', trades[0]['dt']).strftime('%H:%M'),
                'title': trades[0]['title'][:60],
                'num_trades': len(trades),
                'total_cost': round(pnl['total_cost'], 2),
                'combined_avg': round(pnl['combined_avg'], 4),
                'pnl_if_up': round(pnl['pnl_if_up'], 2),
                'pnl_if_down': round(pnl['pnl_if_down'], 2),
                'guaranteed': pnl['guaranteed_profit'],
                'pos_type': ps['pos_type'],
                'first_entry_delay': timing['first_entry_s'],
                'up_buy_usdc': round(ps['up_buy_usdc'], 2),
                'down_buy_usdc': round(ps['down_buy_usdc'], 2),
                'sells_usdc': round(ps['total_sell_usdc'], 2),
            }
            market_timeline.append(entry)

        output['feb21_market_timeline'] = market_timeline

        print(f"{'Start':>6} | {'Type':>12} | {'Cost':>8} | {'CombAvg':>8} | {'PnL↑':>8} | {'PnL↓':>8} | {'DN?':>3} | {'#Tr':>4}")
        print("-" * 80)
        for m in market_timeline:
            dn_mark = 'YES' if m['guaranteed'] else 'no'
            print(f"{m['market_start']:>6} | {m['pos_type']:>12} | ${m['total_cost']:>7.2f} | ${m['combined_avg']:>7.4f} | ${m['pnl_if_up']:>7.2f} | ${m['pnl_if_down']:>7.2f} | {dn_mark:>3} | {m['num_trades']:>4}")

    # =========================================================================
    # SECTION 12: Cross-day comparison summary
    # =========================================================================
    print("\n=== CROSS-DAY COMPARISON ===")
    comparison = {}
    for date in sorted(by_date.keys()):
        day_markets = group_by_market(by_date[date])
        all_combined_avgs = []
        dn_count = 0
        total_cost = 0
        total_min_pnl = 0
        total_max_pnl = 0

        for cid, trades in day_markets.items():
            pnl = compute_pnl_for_market(trades)
            if pnl['combined_avg'] > 0:
                all_combined_avgs.append(pnl['combined_avg'])
            if pnl['guaranteed_profit']:
                dn_count += 1
            total_cost += pnl['total_cost']
            total_min_pnl += pnl['min_pnl']
            total_max_pnl += pnl['max_pnl']

        comparison[date] = {
            'markets': len(day_markets),
            'trades': len(by_date[date]),
            'total_deployed_usdc': round(total_cost, 2),
            'dn_markets': dn_count,
            'dn_pct': round(100 * dn_count / len(day_markets), 1) if day_markets else 0,
            'avg_combined_avg': round(mean(all_combined_avgs), 4) if all_combined_avgs else None,
            'total_min_pnl': round(total_min_pnl, 2),
            'total_max_pnl': round(total_max_pnl, 2),
            'sells': sum(1 for t in by_date[date] if t['side'] == 'SELL'),
        }

        print(f"\n{date}:")
        for k, v in comparison[date].items():
            print(f"  {k}: {v}")

    output['cross_day_comparison'] = comparison

    # Save raw output for report generation
    # Convert sets and non-serializable types
    with open('/home/lehash/development/combine-fetcher/trader_study/analysis_output.json', 'w') as f:
        json.dump(output, f, indent=2, default=str)

    print("\n\nAnalysis complete. Output saved to analysis_output.json")
    return output

if __name__ == '__main__':
    main()
