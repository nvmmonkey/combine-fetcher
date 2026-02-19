#!/usr/bin/env python3
"""
CSV Corruption Investigator
============================
Reads raw CSV files to find exactly what's wrong, where, and why.

Checks:
  1. Column count per row (should be 53)
  2. Data type violations (string in numeric column, etc.)
  3. Value range anomalies (binance_price > 1M, yes_ask > 1.5, etc.)
  4. Specific leak patterns (vol_data_source in outcome, volume in price, etc.)
  5. Shows corrupted rows with surrounding context
"""

import sys
import os
import glob
import csv
import re
from collections import defaultdict, Counter

# Expected 53 columns from csv_header()
EXPECTED_COLS = 53
HEADER_NAMES = [
    "timestamp", "market_slug", "binance_price", "binance_volume",
    "yes_bid", "yes_bid_size", "yes_ask", "yes_ask_size",
    "no_bid", "no_bid_size", "no_ask", "no_ask_size",
    "time_to_open", "time_to_expiry_minutes", "strike_price", "realized_volatility",
    "delta_yes", "gamma", "vega", "theta", "market_resolved", "outcome",
    "timestamp_unix_ms", "binance_update_ms", "poly_yes_update_ms",
    "poly_no_update_ms", "latency_binance_poly_ms",
    "binance_bid", "binance_ask", "binance_spread_bps",
    "vol_1sec", "vol_5sec", "vol_13sec", "vol_34sec", "vol_89sec", "volatility_regime",
    "volume_1sec", "volume_spike", "signal_quality", "trade_confidence", "vol_data_source",
    "yes_spread_bps", "no_spread_bps",
    "rtds_binance_price", "rtds_chainlink_price",
    "rtds_binance_dev_bps", "rtds_chainlink_dev_bps",
    "rtds_price_advantage_bps", "rtds_chainlink_binance_spread_bps",
    "rtds_binance_lag_ms", "rtds_chainlink_lag_ms",
    "rtds_binance_stale_ms", "rtds_chainlink_stale_ms",
]

# Expected data types per column
# 'ts' = ISO timestamp, 'str' = string, 'float' = numeric float,
# 'int' = integer, 'bool' = true/false
COL_TYPES = {
    0: 'ts',      # timestamp
    1: 'str',     # market_slug
    2: 'float',   # binance_price
    3: 'float',   # binance_volume
    4: 'float',   # yes_bid
    5: 'float',   # yes_bid_size
    6: 'float',   # yes_ask
    7: 'float',   # yes_ask_size
    8: 'float',   # no_bid
    9: 'float',   # no_bid_size
    10: 'float',  # no_ask
    11: 'float',  # no_ask_size
    12: 'int',    # time_to_open
    13: 'float',  # time_to_expiry_minutes
    14: 'float',  # strike_price
    15: 'float',  # realized_volatility
    16: 'float',  # delta_yes
    17: 'float',  # gamma
    18: 'float',  # vega
    19: 'float',  # theta
    20: 'bool',   # market_resolved
    21: 'str',    # outcome (YES/NO/"")
    22: 'int',    # timestamp_unix_ms
    23: 'int',    # binance_update_ms
    24: 'int',    # poly_yes_update_ms
    25: 'int',    # poly_no_update_ms
    26: 'int',    # latency_binance_poly_ms
    27: 'float',  # binance_bid
    28: 'float',  # binance_ask
    29: 'float',  # binance_spread_bps
    30: 'float',  # vol_1sec
    31: 'float',  # vol_5sec
    32: 'float',  # vol_13sec
    33: 'float',  # vol_34sec
    34: 'float',  # vol_89sec
    35: 'str',    # volatility_regime
    36: 'float',  # volume_1sec
    37: 'bool',   # volume_spike
    38: 'str',    # signal_quality
    39: 'float',  # trade_confidence
    40: 'str',    # vol_data_source
    41: 'float',  # yes_spread_bps
    42: 'float',  # no_spread_bps
    43: 'float',  # rtds_binance_price
    44: 'float',  # rtds_chainlink_price
    45: 'float',  # rtds_binance_dev_bps
    46: 'float',  # rtds_chainlink_dev_bps
    47: 'float',  # rtds_price_advantage_bps
    48: 'float',  # rtds_chainlink_binance_spread_bps
    49: 'int',    # rtds_binance_lag_ms
    50: 'int',    # rtds_chainlink_lag_ms
    51: 'int',    # rtds_binance_stale_ms
    52: 'int',    # rtds_chainlink_stale_ms
}

# Sane value ranges for key columns
RANGE_CHECKS = {
    2:  ('binance_price',    1000, 500000),
    3:  ('binance_volume',   0, 1e12),
    4:  ('yes_bid',          -0.01, 1.5),
    5:  ('yes_bid_size',     0, 1e9),
    6:  ('yes_ask',          -0.01, 1.5),
    7:  ('yes_ask_size',     0, 1e9),
    8:  ('no_bid',           -0.01, 1.5),
    10: ('no_ask',           -0.01, 1.5),
    14: ('strike_price',     1000, 500000),
    13: ('ttx',              -1, 20),
    27: ('binance_bid',      1000, 500000),
    28: ('binance_ask',      1000, 500000),
    41: ('yes_spread_bps',   -1, 50000),
    42: ('no_spread_bps',    -1, 50000),
    43: ('rtds_binance_price', 0, 500000),   # 0 is valid (no data yet)
    44: ('rtds_chainlink_price', 0, 500000),
}

# Known string field values (for detecting column shifts)
KNOWN_STRING_VALUES = {
    'vol_data_source': {'REALTIME_FIBONACCI', '1MIN_FALLBACK', 'BOOTSTRAP', 'DEFAULT', 'UNKNOWN', ''},
    'volatility_regime': {'EXTREME', 'HIGH', 'LOW', 'NORMAL', 'BOOTSTRAP', ''},
    'signal_quality': {'GOOD', 'LOW', ''},
    'outcome': {'YES', 'NO', ''},
}


def is_float(s):
    """Check if string is a valid float (including negative, scientific notation)."""
    try:
        float(s)
        return True
    except (ValueError, TypeError):
        return False


def is_int(s):
    """Check if string is a valid integer (including negative)."""
    try:
        int(s)
        return True
    except (ValueError, TypeError):
        # Could be float representation of int
        try:
            v = float(s)
            return v == int(v)
        except:
            return False


def check_type(value, expected_type):
    """Check if value matches expected type. Returns (ok, detail)."""
    value = value.strip()
    if expected_type == 'float':
        if value.lower() in ('nan', 'inf', '-inf'):
            return False, f"non-finite float: '{value}'"
        if is_float(value) or value == '':
            return True, None
        return False, f"expected float, got '{value}'"
    elif expected_type == 'int':
        if value.lower() in ('nan', 'inf', '-inf'):
            return False, f"non-finite in int column: '{value}'"
        if is_int(value) or value == '':
            return True, None
        return False, f"expected int, got '{value}'"
    elif expected_type == 'bool':
        if value.lower() in ('true', 'false', ''):
            return True, None
        return False, f"expected bool, got '{value}'"
    elif expected_type == 'ts':
        # ISO timestamp: 2026-02-18T11:39:37.325
        if re.match(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}', value):
            return True, None
        return False, f"expected timestamp, got '{value}'"
    elif expected_type == 'str':
        return True, None  # anything goes for string columns
    return True, None


def check_range(value, col_idx):
    """Check if numeric value is in sane range."""
    if col_idx not in RANGE_CHECKS:
        return True, None
    name, lo, hi = RANGE_CHECKS[col_idx]
    try:
        v = float(value)
        if v < lo or v > hi:
            return False, f"{name}={v} outside [{lo}, {hi}]"
    except (ValueError, TypeError):
        pass  # type check handles this
    return True, None


def detect_shift_pattern(fields):
    """Try to detect if columns are shifted and by how much."""
    if len(fields) != EXPECTED_COLS:
        return None

    # Check if outcome (col 21) contains a vol_data_source value
    outcome_val = fields[21].strip()
    if outcome_val in KNOWN_STRING_VALUES['vol_data_source'] - {'', 'YES', 'NO'}:
        # vol_data_source is col 40, outcome is col 21
        # Check: is field[20] (market_resolved) looking like vol_data_source?
        return f"outcome='{outcome_val}' (looks like vol_data_source leaked)"

    # Check if binance_price looks like volume
    try:
        bp = float(fields[2])
        if bp > 1e9:  # binance_price > 1B → definitely volume
            return f"binance_price={bp:.0f} (looks like binance_volume leaked into col 2)"
    except:
        pass

    # Check if market_resolved (col 20) has non-bool
    mr = fields[20].strip()
    if mr.lower() not in ('true', 'false', ''):
        return f"market_resolved='{mr}' (expected bool)"

    return None


def investigate_file(filepath, max_bad_display=10, context_lines=1):
    """Investigate a single CSV file for corruption."""
    filename = os.path.basename(filepath)
    print(f"\n{'='*70}")
    print(f"FILE: {filename}")
    print(f"{'='*70}")

    total_rows = 0
    bad_colcount = 0
    bad_type = 0
    bad_range = 0
    nan_count = defaultdict(int)  # col_idx -> count of NaN/inf values
    truncated_lines = 0           # lines that look like partial overwrites
    bad_rows = []  # (line_num, reason, fields)
    colcount_dist = Counter()
    type_violations = defaultdict(int)  # col_idx -> count
    range_violations = defaultdict(int)  # col_idx -> count
    shift_patterns = Counter()
    all_lines = []

    # Also read raw lines for truncation detection
    with open(filepath, 'r') as raw_f:
        raw_lines = raw_f.readlines()

    with open(filepath, 'r', newline='') as f:
        reader = csv.reader(f)
        for line_num, row in enumerate(reader):
            all_lines.append(row)
            if line_num == 0:
                # Validate header
                n_header = len(row)
                # Strip whitespace from header names (Rust \-continuation may add spaces)
                cleaned = [h.strip() for h in row]
                if cleaned != HEADER_NAMES:
                    mismatches = []
                    for i, (got, exp) in enumerate(zip(cleaned, HEADER_NAMES)):
                        if got != exp:
                            mismatches.append(f"  col {i}: got '{got}', expected '{exp}'")
                    if mismatches:
                        print(f"  ⚠️  Header mismatch ({len(mismatches)} columns):")
                        for m in mismatches[:5]:
                            print(m)
                    if n_header != EXPECTED_COLS:
                        print(f"  ⚠️  Header has {n_header} columns, expected {EXPECTED_COLS}")
                continue

            total_rows += 1
            n_cols = len(row)
            colcount_dist[n_cols] += 1
            reasons = []

            # Check 1: Column count
            if n_cols != EXPECTED_COLS:
                bad_colcount += 1
                reasons.append(f"col_count={n_cols} (expected {EXPECTED_COLS})")

            # Only do detailed checks if column count is correct
            if n_cols == EXPECTED_COLS:
                # Check 2: Data types + NaN tracking
                for col_idx, expected_type in COL_TYPES.items():
                    val = row[col_idx].strip()
                    # Track NaN/inf specifically
                    if val.lower() in ('nan', 'inf', '-inf'):
                        nan_count[col_idx] += 1
                    ok, detail = check_type(val, expected_type)
                    if not ok:
                        bad_type += 1
                        type_violations[col_idx] += 1
                        reasons.append(f"col[{col_idx}]({HEADER_NAMES[col_idx]}): {detail}")

                # Check 3: Value ranges
                for col_idx in RANGE_CHECKS:
                    ok, detail = check_range(row[col_idx], col_idx)
                    if not ok:
                        bad_range += 1
                        range_violations[col_idx] += 1
                        reasons.append(f"col[{col_idx}]: {detail}")

                # Check 4: Shift patterns
                shift = detect_shift_pattern(row)
                if shift:
                    shift_patterns[shift] += 1
                    reasons.append(f"SHIFT: {shift}")

            if reasons:
                bad_rows.append((line_num, reasons, row))

    # ── Report ──
    print(f"  Total data rows: {total_rows:,}")
    print(f"  Corrupted rows:  {len(bad_rows):,} ({len(bad_rows)/max(total_rows,1)*100:.3f}%)")

    if colcount_dist:
        print(f"\n  Column count distribution:")
        for n, cnt in sorted(colcount_dist.items()):
            marker = " ✅" if n == EXPECTED_COLS else " ❌"
            print(f"    {n} cols: {cnt:,} rows{marker}")

    if bad_colcount > 0:
        print(f"\n  ❌ Wrong column count: {bad_colcount:,} rows")

    if type_violations:
        print(f"\n  ❌ Type violations by column:")
        for col_idx, cnt in sorted(type_violations.items(), key=lambda x: -x[1]):
            print(f"    col[{col_idx:2d}] {HEADER_NAMES[col_idx]:>35s}: {cnt:,} violations")

    if range_violations:
        print(f"\n  ❌ Range violations by column:")
        for col_idx, cnt in sorted(range_violations.items(), key=lambda x: -x[1]):
            name, lo, hi = RANGE_CHECKS[col_idx]
            print(f"    col[{col_idx:2d}] {name:>25s}: {cnt:,} out of [{lo}, {hi}]")

    if shift_patterns:
        print(f"\n  🔍 Detected shift patterns:")
        for pattern, cnt in shift_patterns.most_common(10):
            print(f"    {cnt:,}× {pattern}")

    if nan_count:
        total_nans = sum(nan_count.values())
        print(f"\n  ⚠️  NaN/inf values: {total_nans:,} total")
        for col_idx, cnt in sorted(nan_count.items(), key=lambda x: -x[1]):
            print(f"    col[{col_idx:2d}] {HEADER_NAMES[col_idx]:>35s}: {cnt:,}")

    # Check for truncated/partial lines (file overwrite artifact)
    if raw_lines:
        for i, line in enumerate(raw_lines):
            stripped = line.rstrip('\n\r')
            comma_count = stripped.count(',')
            # A valid line should have exactly 52 commas (53 fields)
            # A truncated line from file overwrite would have fewer
            if i > 0 and 0 < comma_count < 20 and len(stripped) > 5:
                truncated_lines += 1
                if truncated_lines <= 3:
                    print(f"\n  🔧 Possible truncated line {i}: '{stripped[:80]}...' ({comma_count} commas)")
        if truncated_lines > 0:
            print(f"\n  🔧 Total truncated/partial lines: {truncated_lines}")
            print(f"     → Likely caused by process restart within same hour")
            print(f"       (OpenOptions::write(true) without truncate overwrites from start)")

    # Show sample corrupted rows with context
    if bad_rows:
        n_show = min(max_bad_display, len(bad_rows))
        print(f"\n  📋 Sample corrupted rows ({n_show}/{len(bad_rows)}):")
        print(f"  {'─'*65}")

        for i in range(n_show):
            line_num, reasons, fields = bad_rows[i]
            print(f"\n  Line {line_num} — {len(reasons)} issue(s):")
            for r in reasons[:5]:
                print(f"    → {r}")

            # Show the actual values for key columns
            if len(fields) >= EXPECTED_COLS:
                print(f"    Key values:")
                key_cols = [0, 1, 2, 3, 6, 10, 13, 14, 20, 21, 35, 37, 38, 40]
                for ci in key_cols:
                    if ci < len(fields):
                        expected = COL_TYPES.get(ci, '?')
                        print(f"      [{ci:2d}] {HEADER_NAMES[ci]:>25s} = '{fields[ci].strip()}'  (expect {expected})")
            elif len(fields) > 0:
                # Wrong column count - show all fields numbered
                print(f"    Raw fields ({len(fields)} total):")
                for ci, val in enumerate(fields[:15]):
                    exp = HEADER_NAMES[ci] if ci < len(HEADER_NAMES) else "???"
                    print(f"      [{ci:2d}] (should be {exp:>25s}) = '{val.strip()}'")
                if len(fields) > 15:
                    print(f"      ... {len(fields)-15} more fields ...")

            # Show surrounding context (previous good row)
            if context_lines > 0:
                prev_idx = line_num - 1
                if prev_idx > 0 and prev_idx < len(all_lines):
                    prev = all_lines[prev_idx]
                    if len(prev) == EXPECTED_COLS:
                        print(f"    Context (previous row, line {prev_idx}):")
                        print(f"      timestamp     = '{prev[0].strip()}'")
                        print(f"      market_slug   = '{prev[1].strip()}'")
                        print(f"      binance_price = '{prev[2].strip()}'")
                        print(f"      outcome       = '{prev[21].strip()}'")
                        print(f"      vol_data_src  = '{prev[40].strip()}'")

    # ── Unique corrupted market slugs ──
    if bad_rows:
        bad_slugs = Counter()
        for _, _, fields in bad_rows:
            if len(fields) > 1:
                bad_slugs[fields[1].strip()] += 1
        if bad_slugs:
            print(f"\n  Corrupted rows by market_slug:")
            for slug, cnt in bad_slugs.most_common(10):
                print(f"    {slug}: {cnt:,}")

    # ── Check for the specific '1MIN_FALLBACK' in outcome ──
    fallback_in_outcome = 0
    for _, reasons, fields in bad_rows:
        if len(fields) >= 22 and '1MIN_FALLBACK' in fields[21]:
            fallback_in_outcome += 1
    if fallback_in_outcome:
        print(f"\n  🚨 '1MIN_FALLBACK' found in outcome column: {fallback_in_outcome} rows")

    return {
        'file': filename,
        'total_rows': total_rows,
        'bad_rows': len(bad_rows),
        'bad_colcount': bad_colcount,
        'bad_type': bad_type,
        'bad_range': bad_range,
        'nan_count': dict(nan_count),
        'truncated_lines': truncated_lines,
        'type_violations': dict(type_violations),
        'range_violations': dict(range_violations),
        'shift_patterns': dict(shift_patterns),
    }


def main():
    if len(sys.argv) < 2:
        print("Usage: python3 csv_investigate.py <csv_dir_or_file> [--max-show N]")
        print("  Investigates CSV files for corruption patterns")
        sys.exit(1)

    path = sys.argv[1]
    max_show = 10
    if '--max-show' in sys.argv:
        idx = sys.argv.index('--max-show')
        max_show = int(sys.argv[idx + 1])

    if os.path.isdir(path):
        files = sorted(glob.glob(os.path.join(path, '*.csv')))
        print(f"Found {len(files)} CSV files in {path}")
    elif os.path.isfile(path):
        files = [path]
    else:
        print(f"Error: {path} not found")
        sys.exit(1)

    all_results = []
    for f in files:
        result = investigate_file(f, max_bad_display=max_show)
        all_results.append(result)

    # ── Summary ──
    print(f"\n\n{'='*70}")
    print("SUMMARY ACROSS ALL FILES")
    print(f"{'='*70}")

    total_rows = sum(r['total_rows'] for r in all_results)
    total_bad = sum(r['bad_rows'] for r in all_results)
    total_colcount = sum(r['bad_colcount'] for r in all_results)

    print(f"  Total rows:     {total_rows:,}")
    print(f"  Total corrupted:{total_bad:,} ({total_bad/max(total_rows,1)*100:.4f}%)")
    print(f"  Wrong col count:{total_colcount:,}")

    # Aggregate type violations
    agg_type = defaultdict(int)
    for r in all_results:
        for col, cnt in r['type_violations'].items():
            agg_type[col] += cnt
    if agg_type:
        print(f"\n  Type violations (all files):")
        for col, cnt in sorted(agg_type.items(), key=lambda x: -x[1]):
            print(f"    col[{col:2d}] {HEADER_NAMES[col]:>35s}: {cnt:,}")

    # Aggregate range violations
    agg_range = defaultdict(int)
    for r in all_results:
        for col, cnt in r['range_violations'].items():
            agg_range[col] += cnt
    if agg_range:
        print(f"\n  Range violations (all files):")
        for col, cnt in sorted(agg_range.items(), key=lambda x: -x[1]):
            name = RANGE_CHECKS[col][0]
            print(f"    col[{col:2d}] {name:>25s}: {cnt:,}")

    # Aggregate shift patterns
    agg_shift = Counter()
    for r in all_results:
        for pattern, cnt in r['shift_patterns'].items():
            agg_shift[pattern] += cnt
    if agg_shift:
        print(f"\n  Shift patterns (all files):")
        for pattern, cnt in agg_shift.most_common(10):
            print(f"    {cnt:,}× {pattern}")

    # Aggregate NaN counts
    agg_nan = defaultdict(int)
    for r in all_results:
        for col, cnt in r['nan_count'].items():
            agg_nan[col] += cnt
    if agg_nan:
        print(f"\n  NaN/inf values (all files): {sum(agg_nan.values()):,} total")
        for col, cnt in sorted(agg_nan.items(), key=lambda x: -x[1]):
            print(f"    col[{col:2d}] {HEADER_NAMES[col]:>35s}: {cnt:,}")

    # Aggregate truncated lines
    total_truncated = sum(r['truncated_lines'] for r in all_results)
    if total_truncated:
        print(f"\n  🔧 Truncated/partial lines (all files): {total_truncated:,}")
        print(f"     FIX: Add .truncate(true) to OpenOptions in open_new_hourly_file()")

    # Files with most corruption
    bad_files = [(r['file'], r['bad_rows'], r['total_rows'])
                 for r in all_results if r['bad_rows'] > 0]
    if bad_files:
        print(f"\n  Files with corruption:")
        for fname, bad, total in sorted(bad_files, key=lambda x: -x[1]):
            print(f"    {fname}: {bad:,}/{total:,} ({bad/max(total,1)*100:.3f}%)")
    else:
        print(f"\n  ✅ No corruption found in any file!")


if __name__ == '__main__':
    main()