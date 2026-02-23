#!/usr/bin/env python3
"""
fix_and_slugify.py — Concat all hourly CSVs, output one file per slug with resolution row.

Pipeline:
1. Concat all hourly .csv files (solves slug-split-across-files problem)
2. Group by market_slug (each slug's rows are reunited)
3. Filter: skip incomplete slugs (never reached near-expiry, or < 30k rows)
4. For each complete slug: keep all original rows as-is, append one synthetic
   resolution row (copy of last row with market_resolved=True, outcome from Gamma API)
5. Output one clean <slug>.csv per market

Usage:
    uv run fix_and_slugify.py ./218data/ -o ./218data/slugified
    python3 fix_and_slugify.py ./218data/ ./219data/ ./220data/ -o ./all_slugified
"""

import argparse
import json
import os
import sys
import time

import pandas as pd
import requests

# ============================================================================
# CONFIG
# ============================================================================

GAMMA_API_URL = "https://gamma-api.polymarket.com"
HTTP_TIMEOUT = 10
API_DELAY = 0.2  # seconds between API calls (polite rate limiting)
MIN_ROWS_PER_SLUG = 10000  # slugs with fewer rows are incomplete — skip
TTX_COMPLETE_THRESHOLD = 2.0  # slug must have a row with ttx <= this to be considered complete

# CSV column names
COL_SLUG = "market_slug"
COL_TTX = "time_to_expiry_minutes"
COL_RESOLVED = "market_resolved"
COL_OUTCOME = "outcome"

# ============================================================================
# GAMMA API
# ============================================================================

def fetch_outcome(slug: str, session: requests.Session) -> str | None:
    """Fetch outcome from Gamma API. Returns 'YES', 'NO', or None."""
    url = f"{GAMMA_API_URL}/events/slug/{slug}"
    try:
        resp = session.get(url, timeout=HTTP_TIMEOUT)
        if resp.status_code != 200:
            print(f"    [WARN] {slug} -> HTTP {resp.status_code}")
            return None
        data = resp.json()
        for m in data.get("markets", []):
            outcomes = m.get("outcomes", "[]")
            if isinstance(outcomes, str):
                outcomes = json.loads(outcomes)
            prices = m.get("outcomePrices", "[]")
            if isinstance(prices, str):
                prices = json.loads(prices)
            if len(outcomes) >= 2 and len(prices) >= 2:
                up_idx = next((i for i, o in enumerate(outcomes) if o == "Up"), None)
                down_idx = next((i for i, o in enumerate(outcomes) if o == "Down"), None)
                if up_idx is not None and down_idx is not None:
                    if str(prices[up_idx]) == "1":
                        return "YES"
                    elif str(prices[down_idx]) == "1":
                        return "NO"
    except Exception as e:
        print(f"    [WARN] {slug} -> {e}")
    return None

# ============================================================================
# CSV LOADING
# ============================================================================

def find_csv_files(directories: list[str]) -> list[str]:
    """Find all .csv files under given directories, excluding _fixed and slugified."""
    csv_files = []
    for directory in directories:
        for root, _, files in os.walk(directory):
            if "slugified" in root:
                continue
            for fname in sorted(files):
                if fname.lower().endswith(".csv") and "_fixed" not in fname:
                    csv_files.append(os.path.join(root, fname))
    csv_files.sort()
    return csv_files


def load_all_csvs(csv_files: list[str]) -> pd.DataFrame:
    """Load and concatenate all CSVs into one DataFrame."""
    frames = []
    for path in csv_files:
        try:
            df = pd.read_csv(path, low_memory=False)
            frames.append(df)
            print(f"  {os.path.basename(path)}: {len(df):,} rows")
        except Exception as e:
            print(f"  {os.path.basename(path)}: SKIPPED ({e})")

    if not frames:
        print("No valid CSV files found.")
        sys.exit(1)

    combined = pd.concat(frames, ignore_index=True)
    print(f"\n  Total: {len(combined):,} rows\n")
    return combined

# ============================================================================
# SLUGIFY + RESOLVE
# ============================================================================

def write_per_slug(df: pd.DataFrame, output_dir: str, outcome_cache: dict) -> list[dict]:
    """
    For each slug:
    - Skip if < MIN_ROWS or never reached near-expiry (incomplete log)
    - Keep all original rows untouched
    - Append one resolution row: copy of last row with resolved=True, outcome=API
    """
    os.makedirs(output_dir, exist_ok=True)
    summary = []

    slugs = sorted(df[COL_SLUG].dropna().unique())
    print(f"Processing {len(slugs)} slugs -> {output_dir}/\n")

    skipped_rows = 0
    skipped_incomplete = 0

    for slug in slugs:
        group = df[df[COL_SLUG] == slug].copy()

        # Skip small slugs
        if len(group) < MIN_ROWS_PER_SLUG:
            skipped_rows += 1
            print(f"  {slug}: {len(group):,} rows  [SKIP < {MIN_ROWS_PER_SLUG:,}]")
            continue

        # Check if slug reached near-expiry (complete log)
        min_ttx = group[COL_TTX].min()
        if pd.isna(min_ttx) or min_ttx > TTX_COMPLETE_THRESHOLD:
            skipped_incomplete += 1
            print(f"  {slug}: {len(group):,} rows  [SKIP incomplete, min_ttx={min_ttx:.1f}m]")
            continue

        # Sort by timestamp
        if "timestamp" in group.columns:
            group.sort_values("timestamp", inplace=True)
            group.reset_index(drop=True, inplace=True)

        # Fetch outcome from API
        api_outcome = outcome_cache.get(slug)
        if not api_outcome:
            api_outcome = fetch_outcome(slug, requests.Session())
            if api_outcome:
                outcome_cache[slug] = api_outcome
            time.sleep(API_DELAY)

        # Check for wrong outcomes already in the data vs API
        if api_outcome:
            existing = group[COL_OUTCOME].dropna().astype(str).str.strip()
            existing = existing[~existing.isin(["", "nan", "None"])]
            wrong = existing[existing != api_outcome]
            if len(wrong) > 0:
                wrong_val = wrong.iloc[0]
                print(f"    [FIX] found wrong outcome {wrong_val}, api correct to {api_outcome}  (slug: {slug}, {len(wrong)} rows)")

        # Remove ALL existing resolution rows and replace with exactly ONE correct row.
        # The logger can write 1+ resolved rows (some with wrong outcome),
        # so we strip them all and add back one with the API-confirmed outcome.
        if api_outcome:
            existing_resolved = group[group[COL_RESOLVED] == True]
            if len(existing_resolved) > 0:
                # Use last resolved row as template (preserves all other column values)
                template_row = existing_resolved.iloc[[-1]].copy()
                group = group[group[COL_RESOLVED] != True].copy()
                group.reset_index(drop=True, inplace=True)
            else:
                template_row = group.iloc[[-1]].copy()

            resolution_row = template_row.copy()
            resolution_row[COL_RESOLVED] = True
            resolution_row[COL_OUTCOME] = api_outcome
            group = pd.concat([group, resolution_row], ignore_index=True)

        safe_name = slug.replace("/", "_").replace("\\", "_")
        out_path = os.path.join(output_dir, f"{safe_name}.csv")
        group.to_csv(out_path, index=False)

        print(f"  {slug}: {len(group):,} rows  outcome={api_outcome or '?'}")
        summary.append({"slug": slug, "rows": len(group), "outcome": api_outcome or "", "file": out_path})

    if skipped_rows:
        print(f"\n  Skipped {skipped_rows} slug(s) with < {MIN_ROWS_PER_SLUG:,} rows")
    if skipped_incomplete:
        print(f"  Skipped {skipped_incomplete} slug(s) that never reached expiry (min_ttx > {TTX_COMPLETE_THRESHOLD}m)")

    return summary

# ============================================================================
# MAIN
# ============================================================================

def main():
    parser = argparse.ArgumentParser(
        description="Concat hourly CSVs, output one file per slug with resolution row."
    )
    parser.add_argument(
        "input_dirs",
        nargs="+",
        help="One or more directories containing source .csv files",
    )
    parser.add_argument(
        "-o", "--output-dir",
        default="./slugified",
        help="Output directory for per-slug CSVs (default: ./slugified)",
    )
    args = parser.parse_args()

    print("=" * 60)
    print("fix_and_slugify.py")
    print("=" * 60)
    print(f"  Input:  {', '.join(args.input_dirs)}")
    print(f"  Output: {args.output_dir}")
    print()

    # 1. Find and load CSVs
    csv_files = find_csv_files(args.input_dirs)
    if not csv_files:
        print("No .csv files found.")
        sys.exit(1)
    print(f"Found {len(csv_files)} CSV files:\n")
    df = load_all_csvs(csv_files)

    # 2. Fetch all outcomes upfront (batch API calls)
    session = requests.Session()
    session.headers.update({"User-Agent": "fix_and_slugify/1.0"})
    outcome_cache = {}
    slugs = sorted(df[COL_SLUG].dropna().unique())
    print(f"Fetching outcomes for {len(slugs)} slugs...\n")
    for slug in slugs:
        result = fetch_outcome(slug, session)
        if result:
            print(f"    [OK] {slug} -> {result}")
            outcome_cache[slug] = result
        else:
            print(f"    [MISS] {slug} -> no outcome")
        time.sleep(API_DELAY)
    print()

    # 3. Write per-slug files (original rows + resolution row)
    summary = write_per_slug(df, args.output_dir, outcome_cache)

    # 4. Summary
    total_rows = sum(s["rows"] for s in summary)
    with_outcome = sum(1 for s in summary if s["outcome"])
    print(f"\n{'=' * 60}")
    print(f"  Slugs output: {len(summary)}  ({with_outcome} with outcome, {len(summary) - with_outcome} without)")
    print(f"  Total rows: {total_rows:,}")
    print(f"  Output: {args.output_dir}/")
    print(f"  Outcome cache: {len(outcome_cache)} entries")
    print(f"{'=' * 60}")


if __name__ == "__main__":
    main()
