#!/usr/bin/env python3
"""
fix_and_slugify.py — Concat all hourly CSVs, fix outcomes via Gamma API, output one file per slug.

Replaces both fix_csv_outcomes (Rust) and slugify.py with a single correct pipeline:
1. Concat all hourly .csv files (solves slug-split-across-files problem)
2. Group by market_slug (each slug's rows are reunited)
3. For each slug, fetch correct outcome from Gamma API
4. Fix market_resolved/outcome columns
5. Output one clean <slug>.csv per market

Usage:
    python fix_and_slugify.py ./218data/ -o ./218data/slugified
    python fix_and_slugify.py ./218data/ ./219data/ ./220data/ -o ./all_slugified
"""

import argparse
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
TTX_EXPIRY_THRESHOLD = 0.3  # minutes — rows with ttx <= this are considered expired

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
        markets = data.get("markets", [])
        for m in markets:
            outcomes = m.get("outcomes", "[]")
            if isinstance(outcomes, str):
                import json
                outcomes = json.loads(outcomes)
            prices_str = m.get("outcomePrices", "[]")
            if isinstance(prices_str, str):
                import json
                prices = json.loads(prices_str)
            else:
                prices = prices_str

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
            # Skip slugified output directories
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
# OUTCOME FIXING
# ============================================================================

def fix_outcomes(df: pd.DataFrame, outcome_cache: dict, session: requests.Session) -> pd.DataFrame:
    """Fix market_resolved and outcome columns for all slugs (vectorized)."""
    slugs = df[COL_SLUG].dropna().unique()
    print(f"Fetching outcomes for {len(slugs)} slugs...\n")

    # Fetch outcomes for all slugs (this is the only slow part — API calls)
    for slug in sorted(slugs):
        if slug in outcome_cache:
            continue
        result = fetch_outcome(slug, session)
        if result:
            print(f"    [OK] {slug} -> {result}")
            outcome_cache[slug] = result
        else:
            print(f"    [MISS] {slug} -> no outcome")
        time.sleep(API_DELAY)

    print()

    # Map each slug to its API outcome (NaN for slugs with no outcome)
    df["_api_outcome"] = df[COL_SLUG].map(outcome_cache)
    has_api = df["_api_outcome"].notna()

    # Normalize outcome column — treat NaN/None as empty
    df[COL_OUTCOME] = df[COL_OUTCOME].fillna("").astype(str).str.strip()
    df.loc[df[COL_OUTCOME].isin(["nan", "None"]), COL_OUTCOME] = ""

    # Expired rows: ttx <= threshold
    is_expired = df[COL_TTX].notna() & (df[COL_TTX] <= TTX_EXPIRY_THRESHOLD)

    # Pre-expiry rows that already have a valid outcome (from logger WS/price detection)
    has_existing_outcome = df[COL_OUTCOME] != ""

    # Log wrong outcomes (expired rows where existing outcome != API outcome)
    wrong = has_api & is_expired & has_existing_outcome & (df[COL_OUTCOME] != df["_api_outcome"])
    total_wrong = wrong.sum()
    for _, row in df.loc[wrong].drop_duplicates(subset=[COL_SLUG, COL_OUTCOME]).iterrows():
        print(f"    [FIX] found wrong outcome {row[COL_OUTCOME]}, api correct to {row['_api_outcome']}  (slug: {row[COL_SLUG]})")

    # Apply fixes — only where we have an API outcome
    # 1. Expired rows: set resolved=True, outcome=API
    expired_fix = has_api & is_expired
    old_outcomes = df.loc[expired_fix, COL_OUTCOME].copy()
    old_resolved = df.loc[expired_fix, COL_RESOLVED].copy()
    df.loc[expired_fix, COL_RESOLVED] = True
    df.loc[expired_fix, COL_OUTCOME] = df.loc[expired_fix, "_api_outcome"]

    # 2. Non-expired rows WITHOUT existing outcome: set resolved=False, outcome=""
    clear_fix = has_api & ~is_expired & ~has_existing_outcome
    df.loc[clear_fix, COL_RESOLVED] = False
    df.loc[clear_fix, COL_OUTCOME] = ""

    # 3. Non-expired rows WITH existing outcome: leave as-is (pre-expiry detection)

    # Count actual changes
    changed_outcome = (old_outcomes != df.loc[expired_fix, COL_OUTCOME]).sum()
    total_fixed = int(changed_outcome)

    # Cleanup temp column
    df.drop(columns=["_api_outcome"], inplace=True)

    print(f"  Rows fixed: {total_fixed:,}")
    print(f"  Wrong outcomes corrected: {total_wrong}")
    return df

# ============================================================================
# SLUGIFY OUTPUT
# ============================================================================

def write_per_slug(df: pd.DataFrame, output_dir: str) -> list[dict]:
    """Write one CSV file per slug, sorted by timestamp."""
    os.makedirs(output_dir, exist_ok=True)
    summary = []

    slugs = sorted(df[COL_SLUG].dropna().unique())
    print(f"\nWriting {len(slugs)} slug files to {output_dir}/\n")

    for slug in slugs:
        group = df[df[COL_SLUG] == slug].copy()

        # Sort by timestamp to ensure correct order after concat
        if "timestamp" in group.columns:
            group.sort_values("timestamp", inplace=True)

        safe_name = slug.replace("/", "_").replace("\\", "_")
        out_path = os.path.join(output_dir, f"{safe_name}.csv")
        group.to_csv(out_path, index=False)

        # Get outcome for summary
        last_outcome = ""
        outcomes = group[COL_OUTCOME].dropna().unique()
        outcomes = [str(o) for o in outcomes if str(o) not in ("", "nan", "None", "False")]
        if outcomes:
            last_outcome = outcomes[-1]

        print(f"  {slug}: {len(group):,} rows  outcome={last_outcome or '?'}")
        summary.append({"slug": slug, "rows": len(group), "outcome": last_outcome, "file": out_path})

    return summary

# ============================================================================
# MAIN
# ============================================================================

def main():
    parser = argparse.ArgumentParser(
        description="Concat hourly CSVs, fix outcomes via Gamma API, output one file per slug."
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

    # 2. Fix outcomes
    session = requests.Session()
    session.headers.update({"User-Agent": "fix_and_slugify/1.0"})
    outcome_cache = {}
    df = fix_outcomes(df, outcome_cache, session)

    # 3. Write per-slug files
    summary = write_per_slug(df, args.output_dir)

    # 4. Summary
    total_rows = sum(s["rows"] for s in summary)
    with_outcome = sum(1 for s in summary if s["outcome"])
    print(f"\n{'=' * 60}")
    print(f"  Slugs: {len(summary)}  ({with_outcome} with outcome, {len(summary) - with_outcome} without)")
    print(f"  Total rows: {total_rows:,}")
    print(f"  Output: {args.output_dir}/")
    print(f"  Outcome cache: {len(outcome_cache)} entries")
    print(f"{'=' * 60}")


if __name__ == "__main__":
    main()
