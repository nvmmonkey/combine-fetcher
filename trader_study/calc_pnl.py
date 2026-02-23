#!/usr/bin/env python3
"""
Calculate Blushing-Fine trader P&L per market on Feb 21, 2026
and compare to our zv691 backtest results.

Fee formula: fee_rate = 6.25 * price * (1-price), capped at 1.5625%
Fee = size * price * fee_rate / 100

For BUYs: cost = size * price + fee
For SELLs: proceeds = size * price - fee

P&L = (payout from winning shares) + (sell proceeds) - (total buy cost)
"""

import csv
import re
import glob
from collections import defaultdict
from datetime import datetime

# ──────────────────────────────────────────────────────
# 1. Load market outcomes from our fixed CSVs
# ──────────────────────────────────────────────────────
def load_outcomes():
    outcomes = {}
    for fpath in sorted(glob.glob('/home/lehash/development/combine-fetcher/221data/fixed/btc-updown-15m-*.csv')):
        slug = fpath.split('/')[-1].replace('.csv', '')
        with open(fpath) as f:
            reader = csv.DictReader(f)
            for row in reader:
                o = row.get('outcome', '').strip()
                if o:
                    outcomes[slug] = o
                    break
    return outcomes

# ──────────────────────────────────────────────────────
# 2. Parse our backtest per-market results
# ──────────────────────────────────────────────────────
def parse_backtest():
    """Extract per-market combined P&L, DN P&L, DIR P&L from backtest."""
    backtest = {}
    bt_path = '/home/lehash/development/poly-mock-trading/src/data/zv691_feb21unfitFixed.txt'

    with open(bt_path) as f:
        content = f.read()

    # Parse combined results: slug: $XX.XX (YY.Y%) | DN:... | DIR:... | YES/NO
    combined_pattern = re.compile(
        r'(btc-updown-15m-\d+):\s*\$([-\d.]+)\s*\(([-\d.]+)%\)\s*\|'
        r'\s*DN:[^|]+\$([-\d.]+)\s*\|'
        r'\s*DIR:[^|]+\$([-\d.]+)\s*\|\s*(YES|NO)'
    )
    for m in combined_pattern.finditer(content):
        slug = m.group(1)
        if slug not in backtest:
            backtest[slug] = {
                'combined_pnl': float(m.group(2)),
                'combined_roi': float(m.group(3)),
                'dn_pnl': float(m.group(4)),
                'dir_pnl': float(m.group(5)),
                'outcome': m.group(6),
            }

    # Also parse DN-only and DIR-only sections for markets not in combined
    dn_pattern = re.compile(
        r'(btc-updown-15m-\d+):\s*\$([-\d.]+)\s*\(ROI:([-\d.]+)%\)\s*\|'
        r'\s*(\d+)Y/(\d+)N\s+CA:[^|]+\|'
        r'\s*cost=\$([\d.]+)\s+profit=\$([-\d.]+)\s+trades=(\d+)[^|]*\|\s*(YES|NO)'
    )
    # Parse DN section
    dn_section = content[content.find('[DN Book]'):content.find('[DIR Book]')] if '[DN Book]' in content else ''
    for m in dn_pattern.finditer(dn_section):
        slug = m.group(1)
        if slug not in backtest:
            backtest[slug] = {
                'combined_pnl': float(m.group(7)),  # DN profit as combined
                'combined_roi': float(m.group(3)),
                'dn_pnl': float(m.group(7)),
                'dir_pnl': 0.0,
                'outcome': m.group(9),
            }
        else:
            # Update DN details
            pass

    # Parse DIR section
    dir_section = content[content.find('[DIR Book]'):] if '[DIR Book]' in content else ''
    for m in dn_pattern.finditer(dir_section):
        slug = m.group(1)
        if slug not in backtest:
            backtest[slug] = {
                'combined_pnl': float(m.group(7)),
                'combined_roi': float(m.group(3)),
                'dn_pnl': 0.0,
                'dir_pnl': float(m.group(7)),
                'outcome': m.group(9),
            }

    return backtest

# ──────────────────────────────────────────────────────
# 3. Calculate fee
# ──────────────────────────────────────────────────────
def calc_fee(size, price):
    """Polymarket fee: fee_rate = 6.25 * p * (1-p), capped at 1.5625%"""
    fee_rate = 6.25 * price * (1.0 - price)
    fee_rate = min(fee_rate, 1.5625)  # cap at 1.5625%
    fee = size * price * fee_rate / 100.0
    return fee

# ──────────────────────────────────────────────────────
# 4. Load and process trader CSV
# ──────────────────────────────────────────────────────
def load_trader_trades():
    """Load trader trades for Feb 21, grouped by market slug."""
    trader_path = '/home/lehash/development/poly-trader/trader1/filtered_events/btc_15min_trades.csv'

    markets = defaultdict(list)
    all_timestamps = []

    with open(trader_path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            ts_readable = row['timestamp_readable']
            date = ts_readable[:10]
            if date != '2026-02-21':
                continue

            slug = row['slug']
            trade = {
                'timestamp': int(row['timestamp']),
                'timestamp_readable': ts_readable,
                'side': row['side'],
                'size': float(row['size']),
                'price': float(row['price']),
                'outcome_index': int(row['outcome_index']),
                'outcome_name': row['outcome'],  # "Up" or "Down"
            }
            markets[slug].append(trade)
            all_timestamps.append(trade['timestamp'])

    return markets, all_timestamps

# ──────────────────────────────────────────────────────
# 5. Calculate P&L per market
# ──────────────────────────────────────────────────────
def calc_market_pnl(trades, winning_outcome_index):
    """
    Calculate P&L for a set of trades in one market.

    winning_outcome_index: 0 = YES/Up wins, 1 = NO/Down wins

    Returns dict with detailed P&L breakdown.
    """
    # Track per outcome_index
    buy_cost = {0: 0.0, 1: 0.0}  # total cost including fees
    buy_shares = {0: 0.0, 1: 0.0}
    sell_proceeds = {0: 0.0, 1: 0.0}
    sell_shares = {0: 0.0, 1: 0.0}
    total_fees = 0.0
    trade_count = {'BUY': {0: 0, 1: 0}, 'SELL': {0: 0, 1: 0}}
    buy_cost_no_fee = {0: 0.0, 1: 0.0}

    for t in trades:
        oi = t['outcome_index']
        size = t['size']
        price = t['price']
        fee = calc_fee(size, price)
        total_fees += fee

        if t['side'] == 'BUY':
            cost = size * price + fee
            buy_cost[oi] += cost
            buy_cost_no_fee[oi] += size * price
            buy_shares[oi] += size
            trade_count['BUY'][oi] += 1
        else:  # SELL
            proceeds = size * price - fee
            sell_proceeds[oi] += proceeds
            sell_shares[oi] += size
            trade_count['SELL'][oi] += 1

    # Net shares held = bought - sold
    net_shares = {
        0: buy_shares[0] - sell_shares[0],
        1: buy_shares[1] - sell_shares[1],
    }

    # Payout: winning shares × $1.00
    payout = max(0, net_shares[winning_outcome_index]) * 1.00

    # Total P&L
    total_buy_cost = buy_cost[0] + buy_cost[1]
    total_sell_proceeds = sell_proceeds[0] + sell_proceeds[1]
    pnl = payout + total_sell_proceeds - total_buy_cost

    # Capital deployed (total money spent on buys)
    capital = total_buy_cost

    roi = (pnl / capital * 100) if capital > 0 else 0.0

    return {
        'pnl': pnl,
        'capital': capital,
        'roi': roi,
        'payout': payout,
        'total_buy_cost': total_buy_cost,
        'total_sell_proceeds': total_sell_proceeds,
        'total_fees': total_fees,
        'buy_shares': buy_shares,
        'sell_shares': sell_shares,
        'net_shares': net_shares,
        'buy_cost': buy_cost,
        'sell_proceeds': sell_proceeds,
        'trade_count': trade_count,
        'num_trades': len(trades),
        'winning_outcome_index': winning_outcome_index,
        'avg_buy_price_0': buy_cost_no_fee[0] / buy_shares[0] if buy_shares[0] > 0 else 0,
        'avg_buy_price_1': buy_cost_no_fee[1] / buy_shares[1] if buy_shares[1] > 0 else 0,
    }


def ts_to_readable(ts):
    return datetime.utcfromtimestamp(ts).strftime('%H:%M:%S')

def slug_to_time(slug):
    """Extract time from slug like btc-updown-15m-1771632000"""
    ts = int(slug.split('-')[-1])
    return datetime.utcfromtimestamp(ts).strftime('%H:%M')


# ──────────────────────────────────────────────────────
# MAIN
# ──────────────────────────────────────────────────────
def main():
    print("Loading data...")
    outcomes = load_outcomes()
    backtest = parse_backtest()
    trader_markets, all_timestamps = load_trader_trades()

    # Determine overlap with backtest
    backtest_slugs = set(outcomes.keys())
    trader_slugs = set(trader_markets.keys())
    overlap = backtest_slugs & trader_slugs
    trader_only = trader_slugs - backtest_slugs
    backtest_only = backtest_slugs - trader_slugs

    print(f"Backtest markets: {len(backtest_slugs)}")
    print(f"Trader Feb 21 markets: {len(trader_slugs)}")
    print(f"Overlap: {len(overlap)}")
    print(f"Trader only: {len(trader_only)}")
    print(f"Backtest only: {len(backtest_only)}")

    # Calculate P&L for overlap markets
    results = []
    for slug in sorted(overlap):
        outcome_str = outcomes[slug]
        winning_oi = 0 if outcome_str == 'YES' else 1  # YES=Up=0, NO=Down=1
        trades = trader_markets[slug]
        pnl_data = calc_market_pnl(trades, winning_oi)
        pnl_data['slug'] = slug
        pnl_data['outcome'] = outcome_str
        pnl_data['time'] = slug_to_time(slug)

        # Add backtest comparison
        if slug in backtest:
            pnl_data['bt_combined'] = backtest[slug]['combined_pnl']
            pnl_data['bt_dn'] = backtest[slug]['dn_pnl']
            pnl_data['bt_dir'] = backtest[slug]['dir_pnl']
        else:
            pnl_data['bt_combined'] = None
            pnl_data['bt_dn'] = None
            pnl_data['bt_dir'] = None

        results.append(pnl_data)

    # Also calculate P&L for trader-only markets (we don't know outcome)
    # For now, compute under BOTH assumptions
    trader_only_results = []
    for slug in sorted(trader_only):
        trades = trader_markets[slug]
        pnl_yes = calc_market_pnl(trades, 0)
        pnl_no = calc_market_pnl(trades, 1)
        trader_only_results.append({
            'slug': slug,
            'time': slug_to_time(slug),
            'pnl_if_yes': pnl_yes['pnl'],
            'pnl_if_no': pnl_no['pnl'],
            'capital': pnl_yes['capital'],
            'num_trades': len(trades),
            'net_yes_shares': pnl_yes['net_shares'][0],
            'net_no_shares': pnl_yes['net_shares'][1],
        })

    # Sort by P&L
    results.sort(key=lambda x: x['pnl'], reverse=True)

    # ──────────────────────────────────────────────────
    # Identify trading halt gap
    # ──────────────────────────────────────────────────
    all_timestamps.sort()
    max_gap = 0
    gap_start = gap_end = 0
    for i in range(1, len(all_timestamps)):
        gap = all_timestamps[i] - all_timestamps[i-1]
        if gap > max_gap:
            max_gap = gap
            gap_start = all_timestamps[i-1]
            gap_end = all_timestamps[i]

    # P&L before vs after halt
    pre_halt_results = [r for r in results if int(r['slug'].split('-')[-1]) < gap_start]
    post_halt_results = [r for r in results if int(r['slug'].split('-')[-1]) >= gap_end]

    # ──────────────────────────────────────────────────
    # Generate report
    # ──────────────────────────────────────────────────
    total_pnl = sum(r['pnl'] for r in results)
    total_capital = sum(r['capital'] for r in results)
    profitable = [r for r in results if r['pnl'] > 0]
    losing = [r for r in results if r['pnl'] < 0]
    breakeven = [r for r in results if r['pnl'] == 0]

    report = []
    report.append("# Blushing-Fine Trader P&L Report — Feb 21, 2026")
    report.append("")
    report.append(f"> Generated: {datetime.utcnow().strftime('%Y-%m-%d %H:%M:%S')} UTC")
    report.append(f"> Trader: Blushing-Fine (0x63ce...9a)")
    report.append(f"> Data: BTC 15-min Up/Down markets")
    report.append(f"> Outcomes: 23 YES / 27 NO across 50 backtest markets")
    report.append(f"> **NOTE**: Team lead stated 'all resolved NO' — this is INCORRECT. Actual: 23 YES / 27 NO.")
    report.append("")

    report.append("## Executive Summary")
    report.append("")
    report.append(f"| Metric | Trader (Blushing-Fine) | Our Backtest (zv691) |")
    report.append(f"|--------|----------------------|---------------------|")

    bt_total = sum(backtest[s]['combined_pnl'] for s in overlap if s in backtest)
    bt_capital = 1214.75  # from backtest report
    report.append(f"| Markets traded (overlap) | {len(overlap)} | 49 |")
    report.append(f"| Total P&L | **${total_pnl:+.2f}** | **${bt_total:+.2f}** |")
    report.append(f"| Capital deployed | ${total_capital:,.2f} | ${bt_capital:,.2f} |")
    report.append(f"| ROI | {total_pnl/total_capital*100:+.1f}% | {bt_total/bt_capital*100:+.1f}% |")
    report.append(f"| Win rate | {len(profitable)}/{len(results)} ({len(profitable)/len(results)*100:.1f}%) | 42.9% |")
    report.append(f"| Total fees paid | ${sum(r['total_fees'] for r in results):,.2f} | N/A |")
    report.append("")

    # ──────────────────────────────────────────────────
    # Trading halt analysis
    # ──────────────────────────────────────────────────
    report.append("## Trading Halt Analysis")
    report.append("")
    report.append(f"**Longest gap**: {max_gap}s ({max_gap/3600:.1f}h)")
    report.append(f"**From**: {datetime.utcfromtimestamp(gap_start).strftime('%Y-%m-%d %H:%M:%S')} UTC")
    report.append(f"**To**: {datetime.utcfromtimestamp(gap_end).strftime('%Y-%m-%d %H:%M:%S')} UTC")
    report.append("")

    pre_pnl = sum(r['pnl'] for r in pre_halt_results)
    pre_cap = sum(r['capital'] for r in pre_halt_results)
    post_pnl = sum(r['pnl'] for r in post_halt_results)
    post_cap = sum(r['capital'] for r in post_halt_results)

    report.append(f"| Period | Markets | P&L | Capital | ROI |")
    report.append(f"|--------|---------|-----|---------|-----|")
    if pre_cap > 0:
        report.append(f"| Pre-halt | {len(pre_halt_results)} | ${pre_pnl:+.2f} | ${pre_cap:,.2f} | {pre_pnl/pre_cap*100:+.1f}% |")
    else:
        report.append(f"| Pre-halt | {len(pre_halt_results)} | ${pre_pnl:+.2f} | $0.00 | N/A |")
    if post_cap > 0:
        report.append(f"| Post-halt | {len(post_halt_results)} | ${post_pnl:+.2f} | ${post_cap:,.2f} | {post_pnl/post_cap*100:+.1f}% |")
    else:
        report.append(f"| Post-halt | {len(post_halt_results)} | ${post_pnl:+.2f} | $0.00 | N/A |")
    report.append("")

    # ──────────────────────────────────────────────────
    # Per-market P&L table (sorted by P&L desc)
    # ──────────────────────────────────────────────────
    report.append("## Per-Market P&L (Overlap Markets, sorted by P&L)")
    report.append("")
    report.append("| # | Market (time ET) | Outcome | Trader P&L | Trader ROI | Trades | Net YES | Net NO | Backtest P&L | Same Dir? |")
    report.append("|---|-----------------|---------|-----------|-----------|--------|---------|--------|-------------|-----------|")

    for i, r in enumerate(results, 1):
        bt_pnl = r.get('bt_combined')
        bt_str = f"${bt_pnl:+.2f}" if bt_pnl is not None else "N/A"
        same_dir = ""
        if bt_pnl is not None:
            if (r['pnl'] > 0 and bt_pnl > 0) or (r['pnl'] < 0 and bt_pnl < 0):
                same_dir = "YES"
            elif r['pnl'] == 0 or bt_pnl == 0:
                same_dir = "—"
            else:
                same_dir = "**NO**"

        report.append(
            f"| {i} | `{r['slug']}` ({r['time']}) | {r['outcome']} | "
            f"${r['pnl']:+.2f} | {r['roi']:+.1f}% | {r['num_trades']} | "
            f"{r['net_shares'][0]:.1f} | {r['net_shares'][1]:.1f} | "
            f"{bt_str} | {same_dir} |"
        )

    report.append("")

    # ──────────────────────────────────────────────────
    # Top 10 / Bottom 10
    # ──────────────────────────────────────────────────
    report.append("## Top 10 Profitable Markets")
    report.append("")
    report.append("| Market | Outcome | Trader P&L | Capital | Strategy | Backtest |")
    report.append("|--------|---------|-----------|---------|----------|----------|")
    for r in results[:10]:
        bt_str = f"${r.get('bt_combined', 0):+.2f}" if r.get('bt_combined') is not None else "N/A"
        # Determine strategy: mostly YES buys, mostly NO buys, or mixed
        net_y = r['net_shares'][0]
        net_n = r['net_shares'][1]
        if net_y > 0 and net_n > 0:
            strategy = f"DN ({net_y:.0f}Y/{net_n:.0f}N)"
        elif net_y > 0:
            strategy = f"Long YES ({net_y:.0f})"
        elif net_n > 0:
            strategy = f"Long NO ({net_n:.0f})"
        else:
            strategy = "Flat/Exited"
        report.append(f"| `{r['slug']}` ({r['time']}) | {r['outcome']} | ${r['pnl']:+.2f} | ${r['capital']:.2f} | {strategy} | {bt_str} |")

    report.append("")
    report.append("## Bottom 10 Losing Markets")
    report.append("")
    report.append("| Market | Outcome | Trader P&L | Capital | Strategy | Backtest |")
    report.append("|--------|---------|-----------|---------|----------|----------|")
    for r in results[-10:]:
        bt_str = f"${r.get('bt_combined', 0):+.2f}" if r.get('bt_combined') is not None else "N/A"
        net_y = r['net_shares'][0]
        net_n = r['net_shares'][1]
        if net_y > 0 and net_n > 0:
            strategy = f"DN ({net_y:.0f}Y/{net_n:.0f}N)"
        elif net_y > 0:
            strategy = f"Long YES ({net_y:.0f})"
        elif net_n > 0:
            strategy = f"Long NO ({net_n:.0f})"
        else:
            strategy = "Flat/Exited"
        report.append(f"| `{r['slug']}` ({r['time']}) | {r['outcome']} | ${r['pnl']:+.2f} | ${r['capital']:.2f} | {strategy} | {bt_str} |")

    report.append("")

    # ──────────────────────────────────────────────────
    # Correlation analysis
    # ──────────────────────────────────────────────────
    report.append("## Correlation: Trader vs Backtest")
    report.append("")

    both_profit = 0
    both_loss = 0
    trader_profit_bt_loss = 0
    trader_loss_bt_profit = 0
    no_bt = 0

    for r in results:
        bt = r.get('bt_combined')
        if bt is None:
            no_bt += 1
            continue
        if r['pnl'] > 0 and bt > 0:
            both_profit += 1
        elif r['pnl'] < 0 and bt < 0:
            both_loss += 1
        elif r['pnl'] > 0 and bt <= 0:
            trader_profit_bt_loss += 1
        elif r['pnl'] <= 0 and bt > 0:
            trader_loss_bt_profit += 1

    report.append(f"| Category | Count |")
    report.append(f"|----------|-------|")
    report.append(f"| Both profitable | {both_profit} |")
    report.append(f"| Both losing | {both_loss} |")
    report.append(f"| Trader wins, we lose | {trader_profit_bt_loss} |")
    report.append(f"| We win, trader loses | {trader_loss_bt_profit} |")
    report.append(f"| No backtest data | {no_bt} |")
    report.append("")

    # ──────────────────────────────────────────────────
    # Backtest worst 6 comparison
    # ──────────────────────────────────────────────────
    report.append("## Backtest Worst 6 Markets — Did the Trader Also Get Hurt?")
    report.append("")
    bt_worst = sorted(
        [(s, backtest[s]['combined_pnl']) for s in overlap if s in backtest],
        key=lambda x: x[1]
    )[:6]

    report.append("| Market | Outcome | Backtest P&L | Trader P&L | Trader Strategy |")
    report.append("|--------|---------|-------------|-----------|----------------|")
    for slug, bt_pnl in bt_worst:
        r = next((x for x in results if x['slug'] == slug), None)
        if r:
            net_y = r['net_shares'][0]
            net_n = r['net_shares'][1]
            if net_y > 0 and net_n > 0:
                strategy = f"DN ({net_y:.0f}Y/{net_n:.0f}N)"
            elif net_y > 0:
                strategy = f"Long YES ({net_y:.0f})"
            elif net_n > 0:
                strategy = f"Long NO ({net_n:.0f})"
            else:
                strategy = "Flat/Exited"
            report.append(f"| `{slug}` ({r['time']}) | {r['outcome']} | ${bt_pnl:+.2f} | ${r['pnl']:+.2f} | {strategy} |")

    report.append("")

    # ──────────────────────────────────────────────────
    # Strategy analysis: what does the trader bet on?
    # ──────────────────────────────────────────────────
    report.append("## Trader Strategy Profile")
    report.append("")

    dn_markets = [r for r in results if r['net_shares'][0] > 0 and r['net_shares'][1] > 0]
    yes_only = [r for r in results if r['net_shares'][0] > 0 and r['net_shares'][1] <= 0]
    no_only = [r for r in results if r['net_shares'][0] <= 0 and r['net_shares'][1] > 0]
    flat = [r for r in results if r['net_shares'][0] <= 0 and r['net_shares'][1] <= 0]

    report.append(f"| Strategy Type | Markets | Total P&L | Win Rate |")
    report.append(f"|--------------|---------|----------|----------|")

    for label, group in [("Delta-Neutral (both sides)", dn_markets),
                          ("Long YES only", yes_only),
                          ("Long NO only", no_only),
                          ("Flat/Exited", flat)]:
        if group:
            grp_pnl = sum(r['pnl'] for r in group)
            grp_wins = sum(1 for r in group if r['pnl'] > 0)
            report.append(f"| {label} | {len(group)} | ${grp_pnl:+.2f} | {grp_wins}/{len(group)} ({grp_wins/len(group)*100:.0f}%) |")

    report.append("")

    # Average position sizes
    report.append("### Position Size Analysis")
    report.append("")
    all_caps = [r['capital'] for r in results]
    all_caps.sort()
    report.append(f"- **Avg capital/market**: ${sum(all_caps)/len(all_caps):.2f}")
    report.append(f"- **Median capital/market**: ${all_caps[len(all_caps)//2]:.2f}")
    report.append(f"- **Min capital**: ${min(all_caps):.2f}")
    report.append(f"- **Max capital**: ${max(all_caps):.2f}")
    report.append(f"- **Total capital across all markets**: ${total_capital:,.2f}")
    report.append("")

    # By outcome
    report.append("### P&L by Market Outcome")
    report.append("")
    yes_markets = [r for r in results if r['outcome'] == 'YES']
    no_markets = [r for r in results if r['outcome'] == 'NO']
    report.append(f"| Outcome | Markets | Total P&L | Avg P&L | Win Rate |")
    report.append(f"|---------|---------|----------|---------|----------|")
    for label, group in [("YES (Up won)", yes_markets), ("NO (Down won)", no_markets)]:
        if group:
            grp_pnl = sum(r['pnl'] for r in group)
            grp_wins = sum(1 for r in group if r['pnl'] > 0)
            report.append(f"| {label} | {len(group)} | ${grp_pnl:+.2f} | ${grp_pnl/len(group):+.2f} | {grp_wins}/{len(group)} ({grp_wins/len(group)*100:.0f}%) |")
    report.append("")

    # ──────────────────────────────────────────────────
    # Non-overlap markets summary
    # ──────────────────────────────────────────────────
    report.append("## Trader-Only Markets (No Backtest Overlap)")
    report.append("")
    report.append(f"The trader traded {len(trader_only)} additional markets on Feb 21 not covered by our backtest.")
    report.append(f"These are earlier/later markets outside our backtest window.")
    report.append("")
    if trader_only_results:
        total_cap_only = sum(r['capital'] for r in trader_only_results)
        total_trades_only = sum(r['num_trades'] for r in trader_only_results)
        pnl_if_all_no = sum(r['pnl_if_no'] for r in trader_only_results)
        pnl_if_all_yes = sum(r['pnl_if_yes'] for r in trader_only_results)
        report.append(f"- **Markets**: {len(trader_only_results)}")
        report.append(f"- **Total capital**: ${total_cap_only:,.2f}")
        report.append(f"- **Total trades**: {total_trades_only}")
        report.append(f"- **P&L if all resolved YES**: ${pnl_if_all_yes:+.2f}")
        report.append(f"- **P&L if all resolved NO**: ${pnl_if_all_no:+.2f}")
        report.append("")

    # ──────────────────────────────────────────────────
    # Backtest-only markets (we traded, trader didn't)
    # ──────────────────────────────────────────────────
    report.append("## Backtest-Only Markets (We Traded, Trader Didn't)")
    report.append("")
    report.append(f"Our backtest traded {len(backtest_only)} markets the trader skipped:")
    report.append("")
    for slug in sorted(backtest_only):
        if slug in backtest:
            bt = backtest[slug]
            report.append(f"- `{slug}` ({slug_to_time(slug)}): {outcomes.get(slug, '?')} — Backtest P&L: ${bt['combined_pnl']:+.2f}")
        else:
            report.append(f"- `{slug}` ({slug_to_time(slug)}): {outcomes.get(slug, '?')}")
    report.append("")

    # ──────────────────────────────────────────────────
    # Key insights
    # ──────────────────────────────────────────────────
    report.append("## Key Insights")
    report.append("")
    report.append(f"1. **Trader total P&L on overlap markets**: ${total_pnl:+.2f} vs our backtest ${bt_total:+.2f}")

    if total_pnl > bt_total:
        report.append(f"   - Trader outperformed our backtest by ${total_pnl - bt_total:.2f}")
    else:
        report.append(f"   - Our backtest outperformed the trader by ${bt_total - total_pnl:.2f}")

    report.append(f"2. **Capital efficiency**: Trader deployed ${total_capital:,.2f} vs our ${bt_capital:,.2f}")
    report.append(f"   - Trader's capital is {total_capital/bt_capital:.1f}x our backtest")

    report.append(f"3. **Win rate**: Trader {len(profitable)/len(results)*100:.1f}% vs backtest 42.9%")

    report.append(f"4. **Strategy overlap**: {both_profit + both_loss} / {len(results) - no_bt} markets had same outcome direction ({(both_profit+both_loss)/(len(results)-no_bt)*100:.0f}%)")

    report.append(f"5. **Trading halt**: {max_gap/3600:.1f}h gap from {datetime.utcfromtimestamp(gap_start).strftime('%H:%M')} to {datetime.utcfromtimestamp(gap_end).strftime('%H:%M')} UTC")
    if pre_cap > 0 and post_cap > 0:
        report.append(f"   - Pre-halt ROI: {pre_pnl/pre_cap*100:+.1f}% ({len(pre_halt_results)} markets)")
        report.append(f"   - Post-halt ROI: {post_pnl/post_cap*100:+.1f}% ({len(post_halt_results)} markets)")

    report.append(f"6. **Fee impact**: Trader paid ${sum(r['total_fees'] for r in results):,.2f} in fees ({sum(r['total_fees'] for r in results)/total_capital*100:.2f}% of capital)")
    report.append("")

    # Write report
    report_path = '/home/lehash/development/combine-fetcher/trader_study/trader_pnl_report.md'
    with open(report_path, 'w') as f:
        f.write('\n'.join(report))

    print(f"\nReport saved to: {report_path}")
    print(f"\n=== SUMMARY ===")
    print(f"Trader P&L: ${total_pnl:+.2f} across {len(overlap)} overlap markets")
    print(f"Backtest P&L: ${bt_total:+.2f}")
    print(f"Trader capital: ${total_capital:,.2f}")
    print(f"Trader win rate: {len(profitable)}/{len(results)} ({len(profitable)/len(results)*100:.1f}%)")
    print(f"Profitable: {len(profitable)}, Losing: {len(losing)}, Breakeven: {len(breakeven)}")


if __name__ == '__main__':
    main()
