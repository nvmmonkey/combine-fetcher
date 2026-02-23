#!/usr/bin/env python3
"""
Supplementary analysis: sell timing, halt context, DN mechanics.
"""

import csv
from collections import defaultdict
from datetime import datetime, timezone
from statistics import mean, median

CSV_PATH = "/home/lehash/development/poly-trader/trader1/filtered_events/btc_15min_trades.csv"

def parse_csv():
    rows = []
    with open(CSV_PATH, 'r') as f:
        reader = csv.DictReader(f)
        for row in reader:
            row['timestamp'] = int(row['timestamp'])
            row['size'] = float(row['size'])
            row['usdc_size'] = float(row['usdc_size'])
            row['price'] = float(row['price'])
            row['outcome_index'] = int(row['outcome_index'])
            row['dt'] = datetime.fromisoformat(row['timestamp_readable'].replace('.000', '+00:00'))
            row['date'] = row['dt'].strftime('%Y-%m-%d')
            slug_parts = row['event_slug'].split('-')
            try:
                row['market_start_ts'] = int(slug_parts[-1])
            except (ValueError, IndexError):
                row['market_start_ts'] = 0
            row['entry_delay_s'] = row['timestamp'] - row['market_start_ts']
            rows.append(row)
    return rows

def main():
    rows = parse_csv()
    rows.sort(key=lambda r: r['timestamp'])

    # =========================================================================
    # 1. SELL TIMING — when in the 15min window do sells happen?
    # =========================================================================
    print("=== SELL TIMING ANALYSIS ===")
    for date in ['2026-02-20', '2026-02-21', '2026-02-22']:
        day_sells = [r for r in rows if r['date'] == date and r['side'] == 'SELL']
        if not day_sells:
            continue

        delays = [r['entry_delay_s'] for r in day_sells]
        # Bucket sells by delay
        early_sells = [d for d in delays if d < 120]      # < 2min
        mid_sells = [d for d in delays if 120 <= d < 600]  # 2-10 min
        late_sells = [d for d in delays if d >= 600]       # > 10min

        print(f"\n--- {date} ---")
        print(f"  Total sells: {len(day_sells)}")
        print(f"  Avg sell delay: {mean(delays):.0f}s ({mean(delays)/60:.1f} min)")
        print(f"  Median sell delay: {median(delays):.0f}s ({median(delays)/60:.1f} min)")
        print(f"  Early (<2min): {len(early_sells)} ({100*len(early_sells)/len(delays):.1f}%)")
        print(f"  Mid (2-10min): {len(mid_sells)} ({100*len(mid_sells)/len(delays):.1f}%)")
        print(f"  Late (>10min): {len(late_sells)} ({100*len(late_sells)/len(delays):.1f}%)")

        # Sell prices vs buy prices comparison
        day_buys = [r for r in rows if r['date'] == date and r['side'] == 'BUY']
        avg_buy_price = mean(r['price'] for r in day_buys)
        avg_sell_price = mean(r['price'] for r in day_sells)
        print(f"  Avg BUY price: ${avg_buy_price:.4f}")
        print(f"  Avg SELL price: ${avg_sell_price:.4f}")
        print(f"  Sell-Buy spread: ${avg_sell_price - avg_buy_price:.4f}")

        # Are sells on the same outcome as buys (reducing position) or opposite?
        # Group by market, check if sells are on the side they bought
        by_market = defaultdict(list)
        for r in [x for x in rows if x['date'] == date]:
            by_market[r['condition_id']].append(r)

        sell_reduces_position = 0
        sell_on_other_side = 0
        for cid, trades in by_market.items():
            buys_up = sum(t['size'] for t in trades if t['side'] == 'BUY' and t['outcome'] == 'Up')
            buys_down = sum(t['size'] for t in trades if t['side'] == 'BUY' and t['outcome'] == 'Down')
            sells_up = [t for t in trades if t['side'] == 'SELL' and t['outcome'] == 'Up']
            sells_down = [t for t in trades if t['side'] == 'SELL' and t['outcome'] == 'Down']

            # Sell on Up when they hold Up = reducing position
            for s in sells_up:
                if buys_up > 0:
                    sell_reduces_position += 1
                else:
                    sell_on_other_side += 1
            for s in sells_down:
                if buys_down > 0:
                    sell_reduces_position += 1
                else:
                    sell_on_other_side += 1

        print(f"  Sells reducing own position: {sell_reduces_position}")
        print(f"  Sells on unbought side: {sell_on_other_side}")

    # =========================================================================
    # 2. FEB 21 HALT CONTEXT — what happened around the gap
    # =========================================================================
    print("\n\n=== FEB 21 HALT CONTEXT ===")
    feb21 = [r for r in rows if r['date'] == '2026-02-21']
    feb21.sort(key=lambda r: r['timestamp'])

    # Gap 1: 02:09 → 03:00 UTC
    # Gap 2: 04:41 → 05:15 UTC
    # Show activity around these times
    print("\n--- Activity around Gap 1 (02:09-03:00 UTC) ---")
    pre_gap1 = [r for r in feb21 if r['dt'].hour == 1 or (r['dt'].hour == 2 and r['dt'].minute <= 10)]
    post_gap1 = [r for r in feb21 if r['dt'].hour == 3 and r['dt'].minute <= 15]

    if pre_gap1:
        last_pre = pre_gap1[-1]
        print(f"  Last trade before gap: {last_pre['dt'].strftime('%H:%M:%S')} - {last_pre['side']} {last_pre['outcome']} @ ${last_pre['price']:.4f}")
        # What market were they trading?
        pre_markets = set(r['event_slug'] for r in pre_gap1)
        print(f"  Markets traded before gap: {pre_markets}")

    if post_gap1:
        first_post = post_gap1[0]
        print(f"  First trade after gap: {first_post['dt'].strftime('%H:%M:%S')} - {first_post['side']} {first_post['outcome']} @ ${first_post['price']:.4f}")
        post_markets = set(r['event_slug'] for r in post_gap1)
        print(f"  Markets traded after gap: {post_markets}")

    print("\n--- Activity around Gap 2 (04:41-05:15 UTC) ---")
    pre_gap2 = [r for r in feb21 if r['dt'].hour == 4 and r['dt'].minute >= 30]
    post_gap2 = [r for r in feb21 if r['dt'].hour == 5 and r['dt'].minute <= 30]

    if pre_gap2:
        last_pre2 = pre_gap2[-1]
        print(f"  Last trade before gap: {last_pre2['dt'].strftime('%H:%M:%S')} - {last_pre2['side']} {last_pre2['outcome']} @ ${last_pre2['price']:.4f}")
        pre_markets2 = set(r['event_slug'] for r in pre_gap2)
        print(f"  Markets traded before gap: {pre_markets2}")

    if post_gap2:
        first_post2 = post_gap2[0]
        print(f"  First trade after gap: {first_post2['dt'].strftime('%H:%M:%S')} - {first_post2['side']} {first_post2['outcome']} @ ${first_post2['price']:.4f}")
        post_markets2 = set(r['event_slug'] for r in post_gap2)
        print(f"  Markets traded after gap: {post_markets2}")

    # =========================================================================
    # 3. DN MECHANICS — How does the trader BUILD the DN position?
    #    Does cheap-up come first, then expensive-down? Or simultaneous?
    # =========================================================================
    print("\n\n=== DN BUILD SEQUENCE (Feb 21 guaranteed-profit markets) ===")
    feb21_markets = defaultdict(list)
    for r in feb21:
        feb21_markets[r['condition_id']].append(r)

    for cid, trades in sorted(feb21_markets.items(), key=lambda x: x[1][0]['market_start_ts']):
        trades.sort(key=lambda r: r['timestamp'])
        buys = [t for t in trades if t['side'] == 'BUY']
        up_buys = [t for t in buys if t['outcome'] == 'Up']
        down_buys = [t for t in buys if t['outcome'] == 'Down']

        if not up_buys or not down_buys:
            continue

        # Check if guaranteed
        up_cost = sum(t['usdc_size'] for t in up_buys)
        down_cost = sum(t['usdc_size'] for t in down_buys)
        up_shares = sum(t['size'] for t in up_buys)
        down_shares = sum(t['size'] for t in down_buys)
        sell_rev = sum(t['usdc_size'] for t in trades if t['side'] == 'SELL')

        net_up = up_shares - sum(t['size'] for t in trades if t['side'] == 'SELL' and t['outcome'] == 'Up')
        net_down = down_shares - sum(t['size'] for t in trades if t['side'] == 'SELL' and t['outcome'] == 'Down')

        total_cost = up_cost + down_cost
        pnl_up = net_up - total_cost + sell_rev
        pnl_down = net_down - total_cost + sell_rev

        if pnl_up > 0 and pnl_down > 0:
            market_start = datetime.fromtimestamp(trades[0]['market_start_ts'], tz=timezone.utc).strftime('%H:%M')
            # Which side was bought first?
            first_up = up_buys[0]['timestamp']
            first_down = down_buys[0]['timestamp']

            up_first = first_up < first_down
            simultaneous = first_up == first_down

            # Time to build both sides
            all_buy_times = [t['timestamp'] for t in buys]
            build_duration = max(all_buy_times) - min(all_buy_times)

            up_avg_price = mean(t['price'] for t in up_buys)
            down_avg_price = mean(t['price'] for t in down_buys)

            print(f"\n  Market {market_start}:")
            print(f"    Up: {len(up_buys)} buys, avg ${up_avg_price:.4f}, cost ${up_cost:.2f}, shares {up_shares:.1f}")
            print(f"    Down: {len(down_buys)} buys, avg ${down_avg_price:.4f}, cost ${down_cost:.2f}, shares {down_shares:.1f}")
            print(f"    {'Up' if up_first else 'Down' if not simultaneous else 'Simultaneous'} bought first ({abs(first_up - first_down)}s apart)")
            print(f"    Build duration: {build_duration}s ({build_duration/60:.1f} min)")
            print(f"    Guaranteed PnL: up=${pnl_up:.2f} down=${pnl_down:.2f}")

            # Show first 5 trades chronologically
            print(f"    First 5 trades:")
            for t in trades[:5]:
                print(f"      +{t['entry_delay_s']}s: {t['side']} {t['outcome']} {t['size']:.1f} shares @ ${t['price']:.4f} (${t['usdc_size']:.2f})")

    # =========================================================================
    # 4. CAPITAL DEPLOYMENT COMPARISON — Feb 20 was massive vs Feb 21/22
    # =========================================================================
    print("\n\n=== CAPITAL DEPLOYMENT PATTERN ===")
    for date in ['2026-02-20', '2026-02-21', '2026-02-22']:
        day = [r for r in rows if r['date'] == date]
        buys = [r for r in day if r['side'] == 'BUY']

        # Group by market
        by_market = defaultdict(list)
        for r in buys:
            by_market[r['condition_id']].append(r)

        sizes = sorted([sum(t['usdc_size'] for t in trades) for trades in by_market.values()], reverse=True)

        print(f"\n--- {date} ---")
        print(f"  Markets: {len(sizes)}")
        print(f"  Top 10 market sizes: {[f'${s:.0f}' for s in sizes[:10]]}")
        print(f"  Bottom 5 market sizes: {[f'${s:.0f}' for s in sizes[-5:]]}")
        print(f"  Top 5 markets account for: ${sum(sizes[:5]):.0f} / ${sum(sizes):.0f} = {100*sum(sizes[:5])/sum(sizes):.1f}%")
        if len(sizes) >= 10:
            print(f"  Top 10 markets account for: ${sum(sizes[:10]):.0f} / ${sum(sizes):.0f} = {100*sum(sizes[:10])/sum(sizes):.1f}%")

    # =========================================================================
    # 5. COMBINED AVG DISTRIBUTION — how many markets achieve < $1 combined?
    # =========================================================================
    print("\n\n=== COMBINED AVG COST DISTRIBUTION ===")
    for date in ['2026-02-20', '2026-02-21', '2026-02-22']:
        day = [r for r in rows if r['date'] == date]
        by_market = defaultdict(list)
        for r in day:
            by_market[r['condition_id']].append(r)

        combined_avgs = []
        for cid, trades in by_market.items():
            buys = [t for t in trades if t['side'] == 'BUY']
            sells = [t for t in trades if t['side'] == 'SELL']
            total_cost = sum(t['usdc_size'] for t in buys)
            sell_rev = sum(t['usdc_size'] for t in sells)

            up_shares = sum(t['size'] for t in buys if t['outcome'] == 'Up') - sum(t['size'] for t in sells if t['outcome'] == 'Up')
            down_shares = sum(t['size'] for t in buys if t['outcome'] == 'Down') - sum(t['size'] for t in sells if t['outcome'] == 'Down')
            net_shares = up_shares + down_shares

            if net_shares > 0:
                # Net cost after sell revenue
                net_cost = total_cost - sell_rev
                combined_avg = net_cost / net_shares if net_shares > 0 else 0
                combined_avgs.append(combined_avg)

        if combined_avgs:
            buckets = {
                '<$0.30': sum(1 for c in combined_avgs if c < 0.30),
                '$0.30-0.40': sum(1 for c in combined_avgs if 0.30 <= c < 0.40),
                '$0.40-0.50': sum(1 for c in combined_avgs if 0.40 <= c < 0.50),
                '$0.50-0.60': sum(1 for c in combined_avgs if 0.50 <= c < 0.60),
                '$0.60-0.70': sum(1 for c in combined_avgs if 0.60 <= c < 0.70),
                '$0.70-0.80': sum(1 for c in combined_avgs if 0.70 <= c < 0.80),
                '$0.80-1.00': sum(1 for c in combined_avgs if 0.80 <= c < 1.00),
                '>=$1.00': sum(1 for c in combined_avgs if c >= 1.00),
            }
            print(f"\n--- {date} ---")
            print(f"  Combined avg distribution (net of sells):")
            for k, v in buckets.items():
                pct = 100 * v / len(combined_avgs) if combined_avgs else 0
                bar = '#' * int(pct / 2)
                print(f"    {k:>10}: {v:>3} ({pct:>5.1f}%) {bar}")
            under_50 = sum(1 for c in combined_avgs if c < 0.50)
            print(f"  Markets with combined avg < $0.50: {under_50}/{len(combined_avgs)} ({100*under_50/len(combined_avgs):.1f}%)")
            under_1 = sum(1 for c in combined_avgs if c < 1.00)
            print(f"  Markets with combined avg < $1.00: {under_1}/{len(combined_avgs)} ({100*under_1/len(combined_avgs):.1f}%)")

    # =========================================================================
    # 6. BUY-BOTH-SIDES SEQUENCING — simultaneous or staggered?
    # =========================================================================
    print("\n\n=== BUY SEQUENCING: UP vs DOWN timing ===")
    for date in ['2026-02-21']:
        day = [r for r in rows if r['date'] == date]
        by_market = defaultdict(list)
        for r in day:
            by_market[r['condition_id']].append(r)

        up_first_count = 0
        down_first_count = 0
        simultaneous_count = 0
        avg_gap = []

        for cid, trades in by_market.items():
            buys = [t for t in trades if t['side'] == 'BUY']
            up_buys = [t for t in buys if t['outcome'] == 'Up']
            down_buys = [t for t in buys if t['outcome'] == 'Down']

            if not up_buys or not down_buys:
                continue

            first_up = min(t['timestamp'] for t in up_buys)
            first_down = min(t['timestamp'] for t in down_buys)

            gap = abs(first_up - first_down)
            avg_gap.append(gap)

            if first_up < first_down:
                up_first_count += 1
            elif first_down < first_up:
                down_first_count += 1
            else:
                simultaneous_count += 1

        print(f"\n--- {date} (both-sides markets only) ---")
        total = up_first_count + down_first_count + simultaneous_count
        print(f"  Up bought first: {up_first_count} ({100*up_first_count/total:.1f}%)")
        print(f"  Down bought first: {down_first_count} ({100*down_first_count/total:.1f}%)")
        print(f"  Simultaneous: {simultaneous_count} ({100*simultaneous_count/total:.1f}%)")
        if avg_gap:
            print(f"  Avg gap between first Up/Down buy: {mean(avg_gap):.1f}s")
            print(f"  Median gap: {median(avg_gap):.1f}s")
            print(f"  Max gap: {max(avg_gap)}s ({max(avg_gap)/60:.1f} min)")

    # =========================================================================
    # 7. FEB 21 LOSS ANALYSIS — what made it a losing day?
    # =========================================================================
    print("\n\n=== FEB 21 LOSS ANALYSIS — WORST MARKETS ===")
    feb21 = [r for r in rows if r['date'] == '2026-02-21']
    by_market = defaultdict(list)
    for r in feb21:
        by_market[r['condition_id']].append(r)

    market_results = []
    for cid, trades in by_market.items():
        buys = [t for t in trades if t['side'] == 'BUY']
        sells = [t for t in trades if t['side'] == 'SELL']

        up_shares = sum(t['size'] for t in buys if t['outcome'] == 'Up')
        down_shares = sum(t['size'] for t in buys if t['outcome'] == 'Down')
        up_cost = sum(t['usdc_size'] for t in buys if t['outcome'] == 'Up')
        down_cost = sum(t['usdc_size'] for t in buys if t['outcome'] == 'Down')
        sell_rev = sum(t['usdc_size'] for t in sells)

        total_cost = up_cost + down_cost
        net_up = up_shares - sum(t['size'] for t in sells if t['outcome'] == 'Up')
        net_down = down_shares - sum(t['size'] for t in sells if t['outcome'] == 'Down')

        pnl_up = net_up - total_cost + sell_rev
        pnl_down = net_down - total_cost + sell_rev

        market_start = datetime.fromtimestamp(trades[0]['market_start_ts'], tz=timezone.utc).strftime('%H:%M')

        market_results.append({
            'market_start': market_start,
            'title': trades[0]['title'][:50],
            'total_cost': total_cost,
            'pnl_if_up': pnl_up,
            'pnl_if_down': pnl_down,
            'worst_pnl': min(pnl_up, pnl_down),
            'best_pnl': max(pnl_up, pnl_down),
            'net_up_shares': net_up,
            'net_down_shares': net_down,
            'up_cost': up_cost,
            'down_cost': down_cost,
            'num_sells': len(sells),
        })

    # Sort by worst P&L
    market_results.sort(key=lambda x: x['worst_pnl'])

    print(f"\nTop 15 worst markets (by worst-case P&L):")
    print(f"{'Start':>6} | {'Cost':>8} | {'PnL↑':>8} | {'PnL↓':>8} | {'Worst':>8} | {'UpCost':>8} | {'DnCost':>8} | Sells")
    print("-" * 85)
    for m in market_results[:15]:
        print(f"{m['market_start']:>6} | ${m['total_cost']:>7.0f} | ${m['pnl_if_up']:>7.0f} | ${m['pnl_if_down']:>7.0f} | ${m['worst_pnl']:>7.0f} | ${m['up_cost']:>7.0f} | ${m['down_cost']:>7.0f} | {m['num_sells']:>3}")

    # What fraction of markets have both scenarios losing?
    both_lose = [m for m in market_results if m['pnl_if_up'] < 0 and m['pnl_if_down'] < 0]
    print(f"\nMarkets where BOTH outcomes lose money: {len(both_lose)}/{len(market_results)}")
    for m in both_lose:
        print(f"  {m['market_start']} | cost=${m['total_cost']:.0f} | pnl_up=${m['pnl_if_up']:.0f} | pnl_dn=${m['pnl_if_down']:.0f}")

    # Markets where worst case > -$200
    big_losers = [m for m in market_results if m['worst_pnl'] < -200]
    print(f"\nMarkets with worst-case loss > $200: {len(big_losers)}/{len(market_results)}")
    total_worst_exposure = sum(m['worst_pnl'] for m in big_losers)
    print(f"  Combined worst-case exposure: ${total_worst_exposure:.0f}")

    print("\n\n=== ANALYSIS COMPLETE ===")

if __name__ == '__main__':
    main()
