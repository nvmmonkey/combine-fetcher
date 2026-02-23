#!/usr/bin/env python3
"""Analyze 6 losing market CSVs from Feb 21 (zv68 backtest)."""

import pandas as pd
import numpy as np
import glob
import os
import sys

DATA_DIR = "/home/lehash/development/combine-fetcher/221data/selected/"

# Known loss context from backtest zv68
LOSS_CONTEXT = {
    "1771632000": {"loss": -14.08, "note": "DIR YES→resolved NO, z_flips=6"},
    "1771634700": {"loss": -60.09, "note": "WORST, DIR YES→resolved NO, 200Y shares, z_flips=4"},
    "1771685100": {"loss": -22.12, "note": "DIR YES→resolved NO, z_flips=5"},
    "1771686900": {"loss": -25.34, "note": "DIR YES→resolved NO, z_flips=1"},
    "1771706700": {"loss": -17.50, "note": "DN one-sided NO, emergency hedge lost"},
    "1771713900": {"loss": -0.00, "note": "DIR YES→resolved NO, z_flips=8"},
}
# Update 1771713900 loss from task description
LOSS_CONTEXT["1771713900"]["loss"] = -25.0  # approximate from task context

FILES = sorted(glob.glob(os.path.join(DATA_DIR, "btc-updown-15m-*.csv")))

def extract_epoch(f):
    return os.path.basename(f).replace("btc-updown-15m-", "").replace(".csv", "")

def compute_z_score(row):
    """Compute z = (binance_price - strike_price) / (strike * rv * sqrt(tte_years))"""
    bp = row["binance_price"]
    sp = row["strike_price"]
    rv = row["realized_volatility"]
    tte_min = row["time_to_expiry_minutes"]
    if rv <= 0 or tte_min <= 0 or sp <= 0:
        return np.nan
    tte_years = tte_min / (365.25 * 24 * 60)
    denom = sp * rv * np.sqrt(tte_years)
    if denom <= 0:
        return np.nan
    return (bp - sp) / denom

def analyze_file(filepath):
    epoch = extract_epoch(filepath)
    ctx = LOSS_CONTEXT.get(epoch, {"loss": 0, "note": "unknown"})

    df = pd.read_csv(filepath, low_memory=False)
    n = len(df)

    print(f"\n{'='*80}")
    print(f"MARKET: {epoch}  |  Loss: ${ctx['loss']:.2f}  |  {ctx['note']}")
    print(f"Rows: {n}  |  Duration: {df['time_to_expiry_minutes'].max():.1f} → {df['time_to_expiry_minutes'].min():.2f} min")
    print(f"{'='*80}")

    # Compute time_pct (0% = start, 100% = expiry)
    max_tte = df["time_to_expiry_minutes"].max()
    min_tte = df["time_to_expiry_minutes"].min()
    df["time_pct"] = 1.0 - (df["time_to_expiry_minutes"] - min_tte) / (max_tte - min_tte + 1e-9)

    # Compute z-score
    df["z_score"] = df.apply(compute_z_score, axis=1)

    # =====================================================
    # 1. Z-SCORE TRAJECTORY
    # =====================================================
    print("\n--- 1. Z-Score Trajectory ---")
    pcts = [0.0, 0.10, 0.25, 0.50, 0.75, 0.90, 0.95, 0.99]
    for p in pcts:
        mask = (df["time_pct"] >= p - 0.02) & (df["time_pct"] <= p + 0.02)
        subset = df.loc[mask, "z_score"].dropna()
        if len(subset) > 0:
            print(f"  time_pct={p:.0%}: z_mean={subset.mean():+.3f}, z_median={subset.median():+.3f}, z_std={subset.std():.3f}, n={len(subset)}")
        else:
            print(f"  time_pct={p:.0%}: NO DATA in window")

    # Final z-score (last 1% of data)
    last_pct = df[df["time_pct"] >= 0.98]
    if len(last_pct) > 0:
        final_z = last_pct["z_score"].dropna()
        if len(final_z) > 0:
            print(f"  FINAL z (last 2%): mean={final_z.mean():+.3f}, min={final_z.min():+.3f}, max={final_z.max():+.3f}")

    # =====================================================
    # 2. RTDS PATTERNS
    # =====================================================
    print("\n--- 2. RTDS Patterns ---")
    rtds_cols = ["rtds_binance_dev_bps", "rtds_chainlink_dev_bps", "rtds_price_advantage_bps", "rtds_binance_stale_ms"]
    for col in rtds_cols:
        if col in df.columns:
            s = df[col].dropna()
            if len(s) > 0:
                print(f"  {col}:")
                print(f"    mean={s.mean():.3f}, std={s.std():.3f}, min={s.min():.3f}, max={s.max():.3f}")
                print(f"    p25={s.quantile(0.25):.3f}, p50={s.quantile(0.50):.3f}, p75={s.quantile(0.75):.3f}, p95={s.quantile(0.95):.3f}")
                # By time window
                for window_name, lo, hi in [("early 0-25%", 0, 0.25), ("mid 25-75%", 0.25, 0.75), ("late 75-100%", 0.75, 1.01)]:
                    wmask = (df["time_pct"] >= lo) & (df["time_pct"] < hi)
                    ws = df.loc[wmask, col].dropna()
                    if len(ws) > 0:
                        print(f"    {window_name}: mean={ws.mean():.3f}, p95={ws.quantile(0.95):.3f}")
            else:
                print(f"  {col}: ALL NaN")

    # RTDS stale percentage
    if "rtds_binance_stale_ms" in df.columns:
        stale_s = df["rtds_binance_stale_ms"].dropna()
        if len(stale_s) > 0:
            stale_pct = (stale_s > 1000).mean() * 100
            print(f"  Binance stale >1s: {stale_pct:.1f}% of ticks")
            very_stale_pct = (stale_s > 5000).mean() * 100
            print(f"  Binance stale >5s: {very_stale_pct:.1f}% of ticks")

    # =====================================================
    # 3. VOLATILITY REGIME TRANSITIONS
    # =====================================================
    print("\n--- 3. Volatility Regime Transitions ---")
    if "volatility_regime" in df.columns:
        regimes = df["volatility_regime"].dropna()
        regime_counts = regimes.value_counts()
        print(f"  Regime distribution: {dict(regime_counts)}")

        # Find transitions
        shifts = df["volatility_regime"].ne(df["volatility_regime"].shift())
        transition_idx = df.index[shifts & ~df["volatility_regime"].isna()]
        transitions = []
        for idx in transition_idx[1:]:  # skip first
            prev_regime = df.loc[idx-1, "volatility_regime"] if idx > 0 else "?"
            curr_regime = df.loc[idx, "volatility_regime"]
            t_pct = df.loc[idx, "time_pct"]
            transitions.append((t_pct, prev_regime, curr_regime))

        if transitions:
            print(f"  Transitions ({len(transitions)}):")
            for t_pct, fr, to in transitions[:20]:  # limit output
                print(f"    time_pct={t_pct:.2%}: {fr} → {to}")
            if len(transitions) > 20:
                print(f"    ... and {len(transitions)-20} more")
        else:
            print("  No regime transitions")

        # Check for EXTREME regime
        extreme_mask = df["volatility_regime"].str.contains("EXTREME", case=False, na=False)
        if extreme_mask.any():
            extreme_start = df.loc[extreme_mask, "time_pct"].min()
            extreme_pct = extreme_mask.mean() * 100
            print(f"  EXTREME regime: starts at time_pct={extreme_start:.2%}, covers {extreme_pct:.1f}% of ticks")
        else:
            print("  No EXTREME regime detected")

    # =====================================================
    # 4. ASK PRICE DISTRIBUTIONS
    # =====================================================
    print("\n--- 4. Ask Price Distributions ---")
    for window_name, lo, hi in [("early 0-25%", 0, 0.25), ("mid 25-75%", 0.25, 0.75), ("late 75-100%", 0.75, 1.01)]:
        wmask = (df["time_pct"] >= lo) & (df["time_pct"] < hi)
        for side in ["yes_ask", "no_ask"]:
            if side in df.columns:
                ws = df.loc[wmask, side].dropna()
                if len(ws) > 0:
                    print(f"  {window_name} {side}: mean={ws.mean():.4f}, min={ws.min():.4f}, max={ws.max():.4f}, std={ws.std():.4f}")

    # Combined ask (yes_ask + no_ask) - key for DN profitability
    if "yes_ask" in df.columns and "no_ask" in df.columns:
        df["combined_ask"] = df["yes_ask"] + df["no_ask"]
        combined = df["combined_ask"].dropna()
        below_1 = (combined < 1.0).mean() * 100
        below_095 = (combined < 0.95).mean() * 100
        print(f"  Combined ask < $1.00: {below_1:.1f}% of ticks")
        print(f"  Combined ask < $0.95: {below_095:.1f}% of ticks")

        # By time window
        for window_name, lo, hi in [("early", 0, 0.25), ("mid", 0.25, 0.75), ("late", 0.75, 1.01)]:
            wmask = (df["time_pct"] >= lo) & (df["time_pct"] < hi)
            ws = df.loc[wmask, "combined_ask"].dropna()
            if len(ws) > 0:
                print(f"  {window_name} combined_ask: mean={ws.mean():.4f}, <1.0={( ws < 1.0).mean()*100:.1f}%")

    # =====================================================
    # 5. Z-FLIP ANALYSIS
    # =====================================================
    print("\n--- 5. Z-Flip Analysis ---")
    z = df["z_score"].dropna()
    z_sign = np.sign(z)
    z_flips = (z_sign != z_sign.shift()).sum()
    print(f"  Total z-sign flips: {z_flips}")

    # z-flip timing
    flip_mask = z_sign != z_sign.shift()
    flip_indices = z.index[flip_mask]
    if len(flip_indices) > 1:
        flip_times = df.loc[flip_indices, "time_pct"].values
        print(f"  Flip times (time_pct): first={flip_times[1]:.2%}, last={flip_times[-1]:.2%}")

        # Flips by time window
        for window_name, lo, hi in [("0-35%", 0, 0.35), ("35-75%", 0.35, 0.75), ("75-100%", 0.75, 1.01)]:
            wflips = ((flip_times >= lo) & (flip_times < hi)).sum()
            print(f"  Flips in {window_name}: {wflips}")

    # Z-score sign duration statistics
    if len(z) > 100:
        sign_changes = np.where(np.diff(np.sign(z.values)))[0]
        if len(sign_changes) > 1:
            run_lengths = np.diff(sign_changes)
            print(f"  Z-run length (ticks): mean={run_lengths.mean():.0f}, min={run_lengths.min()}, max={run_lengths.max()}, median={np.median(run_lengths):.0f}")

    # =====================================================
    # 6. LATE REVERSAL DETECTION
    # =====================================================
    print("\n--- 6. Late Reversal Detection ---")
    # Was z positive early (0-50%) but negative near expiry (80-100%)?
    early_z = df.loc[(df["time_pct"] >= 0.10) & (df["time_pct"] <= 0.50), "z_score"].dropna()
    late_z = df.loc[(df["time_pct"] >= 0.80), "z_score"].dropna()

    if len(early_z) > 0 and len(late_z) > 0:
        early_mean = early_z.mean()
        late_mean = late_z.mean()
        early_pos_pct = (early_z > 0).mean() * 100
        late_pos_pct = (late_z > 0).mean() * 100
        print(f"  Early (10-50%): z_mean={early_mean:+.3f}, positive={early_pos_pct:.1f}%")
        print(f"  Late (80-100%): z_mean={late_mean:+.3f}, positive={late_pos_pct:.1f}%")

        if early_mean > 0 and late_mean < 0:
            print(f"  >>> LATE REVERSAL DETECTED: z flipped from positive to negative <<<")
        elif early_mean < 0 and late_mean > 0:
            print(f"  >>> LATE REVERSAL (opposite): z flipped from negative to positive <<<")
        elif np.sign(early_mean) == np.sign(late_mean):
            print(f"  No reversal: z maintained same sign (early={early_mean:+.2f} → late={late_mean:+.2f})")

        # More granular: 60% and 90% windows
        mid_z = df.loc[(df["time_pct"] >= 0.50) & (df["time_pct"] <= 0.75), "z_score"].dropna()
        very_late_z = df.loc[(df["time_pct"] >= 0.90), "z_score"].dropna()
        if len(mid_z) > 0:
            print(f"  Mid (50-75%): z_mean={mid_z.mean():+.3f}")
        if len(very_late_z) > 0:
            print(f"  Very late (90-100%): z_mean={very_late_z.mean():+.3f}")

    # Did the committed direction (YES=positive z) hold?
    # All 5 DIR markets committed YES → check if z was mostly positive when DIR would commit (35-75% window)
    commit_window = df.loc[(df["time_pct"] >= 0.35) & (df["time_pct"] <= 0.75), "z_score"].dropna()
    if len(commit_window) > 0:
        commit_pos = (commit_window > 0).mean() * 100
        print(f"  DIR commit window (35-75%): z positive={commit_pos:.1f}%, mean={commit_window.mean():+.3f}")

    # =====================================================
    # 7. CHAINLINK DEVIATION NEAR EXPIRY
    # =====================================================
    print("\n--- 7. Chainlink Deviation Near Expiry (last 20%) ---")
    late_mask = df["time_pct"] >= 0.80
    if "rtds_chainlink_dev_bps" in df.columns:
        late_cl = df.loc[late_mask, "rtds_chainlink_dev_bps"].dropna()
        if len(late_cl) > 0:
            print(f"  chainlink_dev_bps (80-100%): mean={late_cl.mean():.3f}, std={late_cl.std():.3f}")
            print(f"    |dev| > 5bps: {(late_cl.abs() > 5).mean()*100:.1f}%")
            print(f"    |dev| > 10bps: {(late_cl.abs() > 10).mean()*100:.1f}%")
            print(f"    |dev| > 20bps: {(late_cl.abs() > 20).mean()*100:.1f}%")

            # Direction of Chainlink deviation
            cl_pos = (late_cl > 0).mean() * 100
            print(f"    Positive (Chainlink > Binance): {cl_pos:.1f}%")

            # By sub-window
            for sub_name, lo, hi in [("80-90%", 0.80, 0.90), ("90-95%", 0.90, 0.95), ("95-100%", 0.95, 1.01)]:
                smask = (df["time_pct"] >= lo) & (df["time_pct"] < hi)
                ss = df.loc[smask, "rtds_chainlink_dev_bps"].dropna()
                if len(ss) > 0:
                    print(f"    {sub_name}: mean={ss.mean():.3f}, |dev|>5bps={( ss.abs() > 5).mean()*100:.1f}%")

    # Chainlink stale ms near expiry
    if "rtds_chainlink_stale_ms" in df.columns:
        late_cl_stale = df.loc[late_mask, "rtds_chainlink_stale_ms"].dropna()
        if len(late_cl_stale) > 0:
            print(f"  chainlink_stale_ms (80-100%): mean={late_cl_stale.mean():.0f}, p95={late_cl_stale.quantile(0.95):.0f}")

    # =====================================================
    # 8. ADDITIONAL: BINANCE PRICE TRAJECTORY
    # =====================================================
    print("\n--- 8. Binance Price vs Strike ---")
    strike = df["strike_price"].iloc[0]
    print(f"  Strike: ${strike:.2f}")
    for p in [0.0, 0.25, 0.50, 0.75, 0.90, 0.99]:
        mask = (df["time_pct"] >= p - 0.02) & (df["time_pct"] <= p + 0.02)
        subset = df.loc[mask, "binance_price"].dropna()
        if len(subset) > 0:
            diff = subset.mean() - strike
            print(f"  time_pct={p:.0%}: binance=${subset.mean():.2f} (diff={diff:+.2f}, {diff/strike*10000:+.1f}bps)")

    # Final outcome: was binance above or below strike?
    last_rows = df.tail(100)
    final_bp = last_rows["binance_price"].mean()
    print(f"  FINAL: binance=${final_bp:.2f}, strike=${strike:.2f}, diff={final_bp-strike:+.2f}")
    if final_bp > strike:
        print(f"  >>> Market ended ABOVE strike (YES should win)")
    else:
        print(f"  >>> Market ended BELOW strike (NO should win)")

    # =====================================================
    # 9. SIGNAL QUALITY & TRADE CONFIDENCE
    # =====================================================
    print("\n--- 9. Signal Quality & Trade Confidence ---")
    for col in ["signal_quality", "trade_confidence"]:
        if col in df.columns:
            s = pd.to_numeric(df[col], errors='coerce').dropna()
            if len(s) > 0:
                print(f"  {col}: mean={s.mean():.3f}, min={s.min():.3f}, max={s.max():.3f}")
                for window_name, lo, hi in [("early", 0, 0.35), ("commit", 0.35, 0.75), ("late", 0.75, 1.01)]:
                    wmask = (df["time_pct"] >= lo) & (df["time_pct"] < hi)
                    ws = pd.to_numeric(df.loc[wmask, col], errors='coerce').dropna()
                    if len(ws) > 0:
                        print(f"    {window_name}: mean={ws.mean():.3f}")

    # =====================================================
    # 10. SPREAD ANALYSIS
    # =====================================================
    print("\n--- 10. Spread Analysis ---")
    for col in ["yes_spread_bps", "no_spread_bps", "binance_spread_bps"]:
        if col in df.columns:
            s = pd.to_numeric(df[col], errors='coerce').dropna()
            if len(s) > 0:
                print(f"  {col}: mean={s.mean():.1f}, p50={s.quantile(0.5):.1f}, p95={s.quantile(0.95):.1f}, max={s.max():.1f}")

    return df

def cross_market_summary(all_dfs):
    """Summary across all 6 losing markets."""
    print(f"\n{'#'*80}")
    print(f"CROSS-MARKET SUMMARY (6 losing markets)")
    print(f"{'#'*80}")

    # Z-score at key moments across all markets
    print("\n--- Z-Score at Critical Decision Points ---")
    for epoch, df in all_dfs.items():
        ctx = LOSS_CONTEXT.get(epoch, {"loss": 0, "note": ""})

        # Z at commit window (35-75%)
        commit_z = df.loc[(df["time_pct"] >= 0.35) & (df["time_pct"] <= 0.75), "z_score"].dropna()
        # Z at late (80-100%)
        late_z = df.loc[(df["time_pct"] >= 0.80), "z_score"].dropna()
        # Z at very late (95-100%)
        vlate_z = df.loc[(df["time_pct"] >= 0.95), "z_score"].dropna()

        commit_mean = commit_z.mean() if len(commit_z) > 0 else np.nan
        late_mean = late_z.mean() if len(late_z) > 0 else np.nan
        vlate_mean = vlate_z.mean() if len(vlate_z) > 0 else np.nan

        reversal = "YES→NO" if commit_mean > 0 and vlate_mean < 0 else "stable+" if commit_mean > 0 and vlate_mean > 0 else "NO→YES" if commit_mean < 0 and vlate_mean > 0 else "stable-"

        print(f"  {epoch} (${ctx['loss']:+.2f}): commit_z={commit_mean:+.3f}, late_z={late_mean:+.3f}, vlate_z={vlate_mean:+.3f} [{reversal}]")

    # RTDS summary
    print("\n--- RTDS Deviation Summary (absolute values) ---")
    for col in ["rtds_binance_dev_bps", "rtds_chainlink_dev_bps", "rtds_price_advantage_bps"]:
        print(f"\n  {col}:")
        for epoch, df in all_dfs.items():
            if col in df.columns:
                s = df[col].dropna().abs()
                if len(s) > 0:
                    above_3 = (s > 3).mean() * 100
                    above_10 = (s > 10).mean() * 100
                    print(f"    {epoch}: mean={s.mean():.2f}, p95={s.quantile(0.95):.2f}, >3bps={above_3:.1f}%, >10bps={above_10:.1f}%")

    # Late Chainlink deviation — resolution arb opportunity
    print("\n--- Chainlink Deviation in Last 5% (resolution arb window) ---")
    for epoch, df in all_dfs.items():
        if "rtds_chainlink_dev_bps" in df.columns:
            late = df.loc[df["time_pct"] >= 0.95, "rtds_chainlink_dev_bps"].dropna()
            if len(late) > 0:
                ctx = LOSS_CONTEXT.get(epoch, {"loss": 0})
                print(f"  {epoch} (${ctx['loss']:+.2f}): mean={late.mean():.2f}, |mean|={late.abs().mean():.2f}, >5bps={( late.abs() > 5).mean()*100:.1f}%")

    # Volatility regime summary
    print("\n--- Volatility Regime Summary ---")
    for epoch, df in all_dfs.items():
        if "volatility_regime" in df.columns:
            regimes = df["volatility_regime"].value_counts()
            extreme_pct = (df["volatility_regime"].str.contains("EXTREME", case=False, na=False)).mean() * 100
            print(f"  {epoch}: {dict(regimes)}, EXTREME={extreme_pct:.1f}%")

    # Combined ask viability
    print("\n--- DN Viability: Combined Ask < $1.00 ---")
    for epoch, df in all_dfs.items():
        if "yes_ask" in df.columns and "no_ask" in df.columns:
            combined = df["yes_ask"] + df["no_ask"]
            below_1 = (combined < 1.0).mean() * 100
            below_095 = (combined < 0.95).mean() * 100
            print(f"  {epoch}: <$1.00={below_1:.1f}%, <$0.95={below_095:.1f}%, mean=${combined.mean():.4f}")

    # Z-flip density by time
    print("\n--- Z-Flip Density by Window ---")
    for epoch, df in all_dfs.items():
        z = df["z_score"].dropna()
        z_sign = np.sign(z)
        total_flips = (z_sign != z_sign.shift()).sum()

        # Flips in commit window vs post-commit
        commit_z = df.loc[(df["time_pct"] >= 0.35) & (df["time_pct"] <= 0.75), "z_score"].dropna()
        post_z = df.loc[(df["time_pct"] >= 0.75), "z_score"].dropna()

        commit_flips = (np.sign(commit_z) != np.sign(commit_z).shift()).sum() if len(commit_z) > 1 else 0
        post_flips = (np.sign(post_z) != np.sign(post_z).shift()).sum() if len(post_z) > 1 else 0

        ctx = LOSS_CONTEXT.get(epoch, {"loss": 0})
        print(f"  {epoch} (${ctx['loss']:+.2f}): total={total_flips}, commit_window={commit_flips}, post_commit={post_flips}")

    # Binance-strike relationship at expiry
    print("\n--- Final Binance vs Strike (Resolution Direction) ---")
    for epoch, df in all_dfs.items():
        strike = df["strike_price"].iloc[0]
        final_bp = df.tail(100)["binance_price"].mean()
        diff_bps = (final_bp - strike) / strike * 10000
        ctx = LOSS_CONTEXT.get(epoch, {"loss": 0, "note": ""})
        outcome = "ABOVE (YES)" if final_bp > strike else "BELOW (NO)"
        print(f"  {epoch}: strike=${strike:.2f}, final=${final_bp:.2f}, diff={diff_bps:+.1f}bps → {outcome} | {ctx['note']}")

    # Stale binance near commit decisions
    print("\n--- Binance Staleness at Commit Window (35-75%) ---")
    for epoch, df in all_dfs.items():
        if "rtds_binance_stale_ms" in df.columns:
            commit_stale = df.loc[(df["time_pct"] >= 0.35) & (df["time_pct"] <= 0.75), "rtds_binance_stale_ms"].dropna()
            if len(commit_stale) > 0:
                print(f"  {epoch}: mean={commit_stale.mean():.0f}ms, p95={commit_stale.quantile(0.95):.0f}ms, >1s={( commit_stale > 1000).mean()*100:.1f}%")


def main():
    all_dfs = {}
    for f in FILES:
        epoch = extract_epoch(f)
        df = analyze_file(f)
        all_dfs[epoch] = df

    cross_market_summary(all_dfs)

    print(f"\n{'#'*80}")
    print("ANALYSIS COMPLETE")
    print(f"{'#'*80}")

if __name__ == "__main__":
    main()
