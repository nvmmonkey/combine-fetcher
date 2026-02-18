VOLATILITY PIPELINE AUDIT — 5 issues for production
✅ Stable market fallback works correctly: When BTC barely moves → dedup filter blocks most ticks → fast_vol returns None → all timeframes collapse to cached_vol_1min from bootstrap klines → reasonable vol (~0.28). This is correct.
The 5 production issues, ranked by severity:

🔴 P0 — 1min buffer never refreshes (multi_market_logger_hourly.rs)
bootstrap_volatility("BTCUSDT", 70) runs once at startup (line 630). After that, add_price_1min() is never called again. The 70-candle buffer decays in relevance over hours. If BTC vol regime shifts (40% → 15% during a weekend lull, or 20% → 60% on a news event), the fallback is stale.
Fix in the main loop — add periodic refresh every 5 minutes:
rust// In the main loop, after the tick logging:
if tick % (300 * 90) == 0 { // Every ~5 minutes at 90 ticks/sec
    let _ = binance_manager.bootstrap_volatility("BTCUSDT", 10).await;
}
Or cleaner: add a refresh_1min_klines() method that fetches only the last 5-10 candles and appends them.

🔴 P0 — No production floor on blended vol output (volatility.rs line 631-637)
The Welford clamp is [0.001, 3.0]. If the 1min buffer contains flat-price candles (BTC stuck at exactly the same price for minutes), vol computes to 0.001. For your z-score:

vol=0.001, BTC $50 off strike, 7.5min TTX → z=136.5 (triggers every DIR gate)
vol=0.10 → z=1.37 (sane)

The MIN_REALTIME_VOL_THRESHOLD at 0.02 only guards the realtime→1min fallback decision. If 1min vol itself is tiny, nothing catches it.
Fix in calculate_all() — add a floor after the blend:
rust// After line 637:
const MIN_PRODUCTION_VOL: f64 = 0.10; // 10% annualized floor for BTCUSDT
self.cached_vol_blended = self.cached_vol_blended.max(MIN_PRODUCTION_VOL);
Also apply to the individual timeframe caches when they're set from 1min fallback (lines 623-628), since the backtest reads vol_89sec etc. directly.

🟡 P1 — vol_data_source always says "REALTIME" (multi_market_logger_hourly.rs line 779-789)
rustif snapshot.volatility_regime == "BOOTSTRAP" {  // NEVER true
volatility_regime is set to EXTREME/HIGH/LOW/NORMAL (binance.rs:711-718), never "BOOTSTRAP". And is_volatility_ready() is always true after bootstrap. So every CSV row says vol_data_source=REALTIME even when calculate_all internally used 1min fallback.
Fix: Add a field to MarketSnapshot indicating which path calculate_all actually took:
rustpub vol_source_path: String, // "REALTIME_FIBONACCI" | "1MIN_FALLBACK" | "DEFAULT"
Set it inside calculate_all based on the use_realtime branch.

🟡 P1 — stale_threshold_ms field is dead code (volatility.rs)
Line 179 sets stale_threshold_ms: 3000 (3s), but is_realtime_stale() (line 345) hard-codes 120_000 (2 minutes):
rustreturn (current_time - latest.timestamp) > 120_000; // Hard-coded!
Fix: Use the field:
rustreturn (current_time - latest.timestamp) > self.stale_threshold_ms;
And set stale_threshold_ms: 120_000 in the constructor (or whatever you want the actual threshold to be).

🟢 P2 — Window size / annualization factor mismatch (volatility.rs)
The "vol_1sec" label uses a 5s window with annualization_1sec (assumes 1s return intervals). The actual return interval depends on the dedup filter (~50ms to 60s). It works empirically because the dedup filter accidentally produces spacing near the assumed intervals, but it's a hidden coupling.
Not blocking production, but worth documenting. If you ever change min_price_change_bps from 0.1, the vol output will shift even though nothing about BTC changed.

Summary for production readiness:
IssueWhereSeverityFix effort1min buffer never refresheslogger main loop🔴 P05 linesNo vol floor on blended outputvolatility.rs calculate_all🔴 P03 linesvol_data_source always "REALTIME"logger + snapshot struct🟡 P110 linesstale_threshold_ms dead codevolatility.rs🟡 P11 lineAnnualization mismatchvolatility.rs🟢 P2Document only
The two P0s are the ones that can cause real money loss: stale vol → wrong z-scores → bad DIR trades, and zero-vol → infinite z-scores → trigger on noise. Fix those before combining with the trading bot.