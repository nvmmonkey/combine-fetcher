#!/usr/bin/env python3
"""
CSV Slugify by market_slug Column

Reads all .csv files from an input directory, concatenates them,
then splits and saves each unique market_slug as its own CSV file.

Output: <OUTPUT_DIR>/<market_slug>.csv
"""

import argparse
import os
import sys

import pandas as pd

# ============================================================================
# CONFIGURATION — override via CLI args or edit here
# ============================================================================

INPUT_DIR  = "./"   # Directory containing source .csv files
OUTPUT_DIR = "./slugified"    # Directory for per-slug output files

# ============================================================================
# FUNCTIONS
# ============================================================================

def find_csv_files(directory: str) -> list[str]:
    """Walk directory tree and return all .csv file paths, sorted."""
    csv_files = []
    for root, _, files in os.walk(directory):
        for fname in sorted(files):
            if fname.lower().endswith(".csv"):
                csv_files.append(os.path.join(root, fname))
    return csv_files


def load_all_csvs(csv_files: list[str]) -> pd.DataFrame:
    """Load and concatenate every CSV into one DataFrame."""
    frames = []
    for path in csv_files:
        print(f"  📂 Reading: {path}")
        try:
            df = pd.read_csv(path, low_memory=False)
            df["_source_file"] = os.path.basename(path)
            frames.append(df)
            print(f"     ✓ {len(df):,} rows")
        except Exception as e:
            print(f"     ⚠️  Skipped — {e}")

    if not frames:
        raise ValueError("No valid CSV files could be loaded.")

    combined = pd.concat(frames, ignore_index=True)
    print(f"\n  ✓ Combined total: {len(combined):,} rows\n")
    return combined


def slugify_by_market_slug(df: pd.DataFrame, output_dir: str) -> list[dict]:
    """
    Split df by the 'market_slug' column.
    Each unique value gets its own <market_slug>.csv in output_dir.
    """
    if "market_slug" not in df.columns:
        raise KeyError(
            "Column 'market_slug' not found. "
            f"Available columns: {list(df.columns)}"
        )

    os.makedirs(output_dir, exist_ok=True)
    summary = []

    unique_slugs = sorted(df["market_slug"].dropna().unique())
    print(f"📊 Found {len(unique_slugs)} unique market_slug value(s)\n")

    for slug in unique_slugs:
        group = df[df["market_slug"] == slug].copy()
        group.drop(columns=["_source_file"], inplace=True, errors="ignore")

        # Sanitize slug for use as a filename
        safe_name = slug.replace("/", "_").replace("\\", "_")
        out_path = os.path.join(output_dir, f"{safe_name}.csv")
        group.to_csv(out_path, index=False)

        print(f"  ✓  {slug}")
        print(f"     {len(group):,} rows  →  {os.path.basename(out_path)}")
        summary.append({"slug": slug, "rows": len(group), "file": out_path})

    return summary


def print_summary(summary: list[dict], output_dir: str) -> None:
    total_rows = sum(s["rows"] for s in summary)
    print(f"\n{'=' * 60}")
    print("📋 SUMMARY")
    print(f"{'=' * 60}")
    print(f"  Unique market slugs : {len(summary)}")
    print(f"  Total rows written  : {total_rows:,}")
    print(f"  Output directory    : {output_dir}")
    print(f"{'=' * 60}")
    print("\n✅ Done!")


# ============================================================================
# MAIN
# ============================================================================

def main():
    parser = argparse.ArgumentParser(
        description="Split all CSVs in a directory into per-market_slug files."
    )
    parser.add_argument(
        "input_dir",
        nargs="?",
        default=INPUT_DIR,
        help=f"Directory containing source .csv files (default: {INPUT_DIR!r})",
    )
    parser.add_argument(
        "-o", "--output-dir",
        default=OUTPUT_DIR,
        dest="output_dir",
        help=f"Output directory for per-slug CSVs (default: {OUTPUT_DIR!r})",
    )
    args = parser.parse_args()

    print("🔍 CSV Slugify by market_slug")
    print("=" * 60)
    print(f"  Input directory : {args.input_dir}")
    print(f"  Output directory: {args.output_dir}")
    print()

    csv_files = find_csv_files(args.input_dir)
    if not csv_files:
        print(f"❌ No .csv files found under: {args.input_dir}")
        sys.exit(1)

    print(f"📁 Found {len(csv_files)} CSV file(s):\n")
    df = load_all_csvs(csv_files)

    summary = slugify_by_market_slug(df, args.output_dir)
    print_summary(summary, args.output_dir)


if __name__ == "__main__":
    main()
