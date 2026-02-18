// btc_dashboard.rs  — Real-time BTC/Polymarket dashboard  v2 (fixed)
//
// FIXES:
//   1. Header layout: removed nested split — render all header content in one
//      Paragraph, TTX gauge rendered in the outer vertical split row below it.
//   2. latency_binance_poly_ms: clamped + guarded against cross-clock garbage.
//   3. All super-large numbers guarded with .clamp() / sanity checks.
//   4. snapshot field names verified against multi_market_logger_hourly.rs.
//   5. pct_above_strike guarded against div-by-zero on startup.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Gauge, Paragraph},
    Frame, Terminal,
};
use rust_decimal::prelude::*;
use std::{
    collections::VecDeque,
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use combine_fetcher::api::binance::BinanceDataManager;
use combine_fetcher::api::polymarket::PolymarketOrderbookStream;
use combine_fetcher::api::rtds_tracker::RtdsTracker;
use combine_fetcher::{
    GAMMA_API_URL, HTTP_TIMEOUT_SECS, MARKET_WINDOW_SECONDS,
    MINUTES_PER_YEAR, POLY_CRYPTO_API_URL, STRIKE_RETRY_INTERVAL_MS,
};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// BPS mode threshold for RTDS stale detection (mirrors backtest default)
const RTDS_ADV_THRESH: f64 = 3.0;
/// Max sane latency to display — anything above is cross-clock garbage
const MAX_SANE_LATENCY_MS: i64 = 10_000;

// ─────────────────────────────────────────────────────────────────────────────
// Market info
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MarketInfo {
    pub slug: String,
    pub condition_id: String,
    pub yes_token_id: String,
    pub no_token_id: String,
    pub window_start: i64,
    pub window_end: i64,
    pub strike_price: Option<f64>,
    pub strike_fetched: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Greeks
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct Greeks { pub delta: f64, pub gamma: f64, pub vega: f64, pub theta: f64 }

fn calculate_greeks(spot: f64, strike: f64, ttx_min: f64, vol: f64) -> Greeks {
    if ttx_min <= 0.01 || vol <= 0.0 || spot <= 0.0 || strike <= 0.0 {
        return Greeks { delta: if spot >= strike { 1.0 } else { 0.0 }, ..Default::default() };
    }
    let t = ttx_min / MINUTES_PER_YEAR;
    let st = t.sqrt();
    let d1 = ((spot / strike).ln() + (vol * vol / 2.0) * t) / (vol * st);
    let d2 = ((spot / strike).ln() - (vol * vol / 2.0) * t) / (vol * st);
    let npdf = |x: f64| (-x * x / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt();
    let nd2 = npdf(d2);
    Greeks {
        delta: (nd2 / (spot * vol * st)).clamp(0.0, 1.0),
        gamma: (nd2 * d1 / (spot * spot * vol * vol * t)).clamp(-1e6, 1e6),
        vega:  (nd2 * st * 0.01).clamp(-1e6, 1e6),
        theta: (-(spot * nd2 * vol) / (2.0 * st) / 365.0).clamp(-1e6, 1e6),
    }
}

fn spread_bps(bid: f64, ask: f64) -> f64 {
    if bid <= 0.0 || ask <= bid { return 0.0; }
    let mid = (bid + ask) / 2.0;
    if mid <= 0.0 { return 0.0; }
    ((ask - bid) / mid) * 10_000.0
}

/// Polymarket fee % — mirrors backtest poly_fee_rate exactly
fn poly_fee_pct(price: f64) -> f64 {
    if price <= 0.0 || price >= 1.0 { return 0.0; }
    (6.25 * price * (1.0 - price)).min(1.5625)
}

/// Z-score: (btc - strike) / (strike × vol × √T_years)
fn z_score(spot: f64, strike: f64, vol: f64, ttx_min: f64) -> (f64, f64, &'static str) {
    if strike <= 0.0 || vol <= 0.0 || ttx_min <= 0.0 { return (0.0, 0.0, "YES"); }
    let t = ttx_min / (365.25 * 24.0 * 60.0);
    let em = strike * vol * t.sqrt().max(1e-12);
    if em < 1e-10 { return (0.0, 0.0, "YES"); }
    let z = (spot - strike) / em;
    (z, z.abs(), if z > 0.0 { "YES" } else { "NO" })
}

/// Expected move as % of strike: vol × √T × 100
fn exp_move_pct(vol: f64, ttx_min: f64) -> f64 {
    let t = ttx_min / (365.25 * 24.0 * 60.0);
    vol * t.sqrt() * 100.0
}

/// Regime classification — mirrors backtest classify_regime
fn classify_regime(yes_ask: f64) -> &'static str {
    if      yes_ask >= 0.85 { "YES_EXTREME" }
    else if yes_ask >= 0.70 { "YES_TREND" }
    else if yes_ask <= 0.15 { "NO_EXTREME" }
    else if yes_ask <= 0.30 { "NO_TREND" }
    else                    { "OSCILLATING" }
}

/// Clamp a latency value: if outside plausible range, return None (displays as "---")
fn sane_latency(ms: i64) -> Option<i64> {
    if ms.abs() > MAX_SANE_LATENCY_MS { None } else { Some(ms) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-signal confidence (mirrors backtest compute_dir_confidence)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct Confidence {
    pub score:      f64,  // 0..1
    pub votes:      u8,   // out of 5
    pub avg_z_vote: bool,
    pub vel_vote:   bool,
    pub asym_vote:  bool,
    pub open_vote:  bool,
    pub fade_vote:  bool,
    pub fade_ratio: f64,
}

fn compute_confidence(z_hist: &VecDeque<f64>, is_yes: bool, opening_dev: f64) -> Confidence {
    if z_hist.len() < 20 { return Confidence::default(); }
    let arr: Vec<f64> = z_hist.iter().copied().collect();
    let n = arr.len();
    let cur_z = *arr.last().unwrap();
    let mut votes = 0u8;

    let avg_z     = arr.iter().sum::<f64>() / n as f64;
    let avg_agree = (avg_z > 0.0) == is_yes;
    if avg_agree { votes += 1; }

    let lookback = (n / 5).max(20).min(n);
    let rec = &arr[n - lookback..];
    let z_vel = if rec.len() > 5 {
        let xm = (rec.len() - 1) as f64 / 2.0;
        let ym = rec.iter().sum::<f64>() / rec.len() as f64;
        let num: f64 = rec.iter().enumerate().map(|(i, &y)| (i as f64 - xm) * (y - ym)).sum();
        let den: f64 = rec.iter().enumerate().map(|(i, _)| { let d = i as f64 - xm; d * d }).sum();
        if den > 1e-10 { num / den } else { 0.0 }
    } else { 0.0 };
    let vel_agree = (z_vel > 0.0) == is_yes;
    if vel_agree { votes += 1; }

    let pct_pos  = arr.iter().filter(|&&z| z > 0.0).count() as f64 / n as f64;
    let asym_ok  = (pct_pos > 0.5) == is_yes;
    if asym_ok { votes += 1; }

    let open_ok  = (opening_dev > 0.0) == is_yes;
    if open_ok { votes += 1; }

    let max_abs  = arr.iter().map(|z| z.abs()).fold(0.0_f64, f64::max);
    let fade     = if max_abs > 0.1 { cur_z.abs() / max_abs } else { 1.0 };
    let fade_ok  = fade >= 0.30;
    if fade_ok { votes += 1; }

    Confidence {
        score: votes as f64 / 5.0,
        votes,
        avg_z_vote: avg_agree,
        vel_vote:   vel_agree,
        asym_vote:  asym_ok,
        open_vote:  open_ok,
        fade_vote:  fade_ok,
        fade_ratio: fade,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Market fetchers
// ─────────────────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct PolyStrike { #[serde(rename = "openPrice")] open_price: Option<f64> }

async fn fetch_current_market(client: &reqwest::Client) -> Result<MarketInfo> {
    let now = Utc::now().timestamp();
    let pos = now % MARKET_WINDOW_SECONDS;
    let ws  = if pos < 60 {
        ((now / MARKET_WINDOW_SECONDS) - 1) * MARKET_WINDOW_SECONDS
    } else {
        (now / MARKET_WINDOW_SECONDS) * MARKET_WINDOW_SECONDS
    };
    let slug = format!("btc-updown-15m-{}", ws);
    let data: serde_json::Value = client
        .get(format!("{}/events/slug/{}", GAMMA_API_URL, slug))
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS)).send().await?.json().await?;
    for m in data["markets"].as_array().ok_or_else(|| anyhow!("no markets"))? {
        if !m["active"].as_bool().unwrap_or(false) { continue; }
        let outcomes: Vec<String> = serde_json::from_str(m["outcomes"].as_str().unwrap_or("[]")).unwrap_or_default();
        let tokens:   Vec<String> = serde_json::from_str(m["clobTokenIds"].as_str().unwrap_or("[]")).unwrap_or_default();
        if outcomes.len() != 2 || tokens.len() != 2 { continue; }
        if let (Some(ui), Some(di)) = (
            outcomes.iter().position(|o| o == "Up"),
            outcomes.iter().position(|o| o == "Down"),
        ) {
            return Ok(MarketInfo {
                slug, condition_id: m["conditionId"].as_str().unwrap_or("").into(),
                yes_token_id: tokens[ui].clone(), no_token_id: tokens[di].clone(),
                window_start: ws, window_end: ws + MARKET_WINDOW_SECONDS,
                strike_price: None, strike_fetched: false,
            });
        }
    }
    Err(anyhow!("No valid BTC market"))
}

async fn fetch_next_market_after(client: &reqwest::Client, ws: i64) -> Result<MarketInfo> {
    let nws  = ws + MARKET_WINDOW_SECONDS;
    let slug = format!("btc-updown-15m-{}", nws);
    let data: serde_json::Value = client
        .get(format!("{}/events/slug/{}", GAMMA_API_URL, slug))
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS)).send().await?.json().await?;
    for m in data["markets"].as_array().ok_or_else(|| anyhow!("no markets"))? {
        if !m["active"].as_bool().unwrap_or(false) { continue; }
        let outcomes: Vec<String> = serde_json::from_str(m["outcomes"].as_str().unwrap_or("[]")).unwrap_or_default();
        let tokens:   Vec<String> = serde_json::from_str(m["clobTokenIds"].as_str().unwrap_or("[]")).unwrap_or_default();
        if outcomes.len() != 2 || tokens.len() != 2 { continue; }
        if let (Some(ui), Some(di)) = (
            outcomes.iter().position(|o| o == "Up"),
            outcomes.iter().position(|o| o == "Down"),
        ) {
            return Ok(MarketInfo {
                slug, condition_id: m["conditionId"].as_str().unwrap_or("").into(),
                yes_token_id: tokens[ui].clone(), no_token_id: tokens[di].clone(),
                window_start: nws, window_end: nws + MARKET_WINDOW_SECONDS,
                strike_price: None, strike_fetched: false,
            });
        }
    }
    Err(anyhow!("No valid next BTC market"))
}

async fn fetch_strike(client: &reqwest::Client, m: &MarketInfo) -> Result<f64> {
    let start = DateTime::from_timestamp(m.window_start, 0).ok_or_else(|| anyhow!("ts"))?;
    let end   = DateTime::from_timestamp(m.window_end,   0).ok_or_else(|| anyhow!("ts"))?;
    let url   = format!(
        "{}/crypto-price?symbol=BTC&eventStartTime={}&variant=fifteen&endDate={}",
        POLY_CRYPTO_API_URL,
        start.format("%Y-%m-%dT%H:%M:%SZ"),
        end.format("%Y-%m-%dT%H:%M:%SZ"),
    );
    let mut backoff = STRIKE_RETRY_INTERVAL_MS;
    loop {
        if let Ok(r) = client.get(&url).timeout(Duration::from_secs(HTTP_TIMEOUT_SECS)).send().await {
            if r.status().is_success() {
                if let Ok(d) = r.json::<PolyStrike>().await {
                    if let Some(p) = d.open_price { return Ok(p); }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(backoff)).await;
        backoff = (backoff * 2).min(30_000);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Temp CSV
// ─────────────────────────────────────────────────────────────────────────────

struct TempCsv { slug: String, path: PathBuf, w: Option<io::BufWriter<fs::File>> }
impl TempCsv {
    fn new() -> Self { Self { slug: String::new(), path: PathBuf::new(), w: None } }
    fn ensure(&mut self, slug: &str, now: &DateTime<Utc>) -> Result<()> {
        if slug == self.slug { return Ok(()); }
        if let Some(mut w) = self.w.take() { let _ = w.flush(); }
        if self.path.exists() { let _ = fs::remove_file(&self.path); }
        let p = PathBuf::from(format!("temp_dash_{}_{}.csv", slug, now.format("%H%M%S")));
        let f = fs::OpenOptions::new().create(true).write(true).open(&p)?;
        let mut w = io::BufWriter::with_capacity(32 * 1024, f);
        writeln!(w, "timestamp,slug,price,yes_ask,no_ask,strike,ttx_min,z_score,abs_z,\
                      z_dir,exp_move_pct,regime,conf_score,conf_votes,rtds_stale,\
                      book_stale_bps,obi,cvd_1m,cvd_5m,delta,vol,dev_usd,dev_pct")?;
        self.slug = slug.into(); self.path = p; self.w = Some(w);
        Ok(())
    }
    fn write(&mut self, r: &Row) -> Result<()> {
        if let Some(ref mut w) = self.w {
            writeln!(w,
                "{},{},{:.2},{:.4},{:.4},{:.2},{:.2},{:+.4},{:.4},{},{:.5},{},{:.2},{},{},\
                 {:.2},{:+.4},{},{},{:.8},{:.6},{:+.2},{:+.4}",
                r.ts, r.slug, r.btc, r.yes_ask, r.no_ask, r.strike, r.ttx,
                r.z, r.abs_z, r.z_dir, r.exp_move, r.regime,
                r.conf.score, r.conf.votes, r.rtds_stale, r.rtds_adv,
                r.obi, r.cvd1m as i64, r.cvd5m as i64,
                r.delta, r.vol, r.dev_usd, r.dev_pct,
            )?;
            w.flush()?;
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Dashboard state row
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct Row {
    ts: String, slug: String, now_ms: i64,

    // price
    btc: f64, btc_prev: f64,
    bnc_bid: f64, bnc_ask: f64, bnc_spr_bps: f64,
    vol_1min: f64, vol_1sec: f64, vol_spike: bool,

    // poly
    yes_bid: f64, yes_bid_sz: f64, yes_ask: f64, yes_ask_sz: f64,
    no_bid:  f64, no_bid_sz:  f64, no_ask:  f64, no_ask_sz:  f64,
    yes_spr_bps: f64, no_spr_bps: f64,
    yes_fee: f64, no_fee: f64,  // %

    // greeks
    strike: f64, vol: f64, ttx: f64, ttx_total: f64,
    delta: f64, gamma: f64, vega: f64, theta: f64,

    // z-score & deviation
    z: f64, abs_z: f64, z_dir: String,
    z_flip: bool, z_flips: u32,
    exp_move: f64,   // %
    exp_move_usd: f64,
    dev_usd: f64, dev_pct: f64,
    regime: String, time_pct: f64,
    pct_above: f64,  // % of ticks btc > strike this window

    // confidence
    conf: Confidence,

    // outcome
    outcome: String,
    out_conf: f64,  // 0..1

    // volatility
    vol_1s: f64, vol_5s: f64, vol_13s: f64, vol_34s: f64, vol_89s: f64,
    vol_regime: String, realized_vol: f64,

    // signal
    sig_quality: String, sig_conf: f64,

    // RTDS
    rtds_adv: f64,
    rtds_cl_dev: f64,
    rtds_stale: bool,          // BPS-mode stale flag
    rtds_bnc_lag: i64,
    rtds_cl_lag: i64,
    rtds_bnc_stale_ms: i64,
    rtds_cl_stale_ms: i64,

    // latency (epoch-ms; display = now_ms - field)
    bnc_update_ms: i64,
    poly_yes_ms: i64,
    poly_no_ms: i64,
    // B↔P delta — raw value may be cross-clock garbage; use sane_latency()
    bvp_delta_ms: i64,

    // derived
    obi: f64,
    cvd1m: f64, cvd3m: f64, cvd5m: f64,
    sig_score: i32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Rolling history
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct History {
    price:       VecDeque<f64>,
    z_hist:      VecDeque<f64>,
    z_dir_hist:  VecDeque<bool>,
    z_flipped:   bool,
    z_flips:     u32,
    prev_z_dir:  Option<bool>,
    opening_dev: Option<f64>,
    above_count: u64,
    tick_count:  u64,
    price_1m:    f64,
    cvd_1m:      VecDeque<(i64, f64)>,
    cvd_3m:      VecDeque<(i64, f64)>,
    cvd_5m:      VecDeque<(i64, f64)>,
}

impl History {
    fn push(&mut self, btc: f64, z: f64, is_yes: bool, now_sec: i64, cvd: f64, strike: f64) {
        self.price.push_back(btc);
        if self.price.len() > 200 { self.price.pop_front(); }
        if self.price.len() >= 60 { self.price_1m = self.price[self.price.len() - 60]; }

        self.z_hist.push_back(z);
        if self.z_hist.len() > 500 { self.z_hist.pop_front(); }
        self.z_dir_hist.push_back(is_yes);
        if self.z_dir_hist.len() > 500 { self.z_dir_hist.pop_front(); }

        if let Some(p) = self.prev_z_dir {
            if p != is_yes { self.z_flipped = true; self.z_flips += 1; }
        }
        self.prev_z_dir = Some(is_yes);

        if self.opening_dev.is_none() && strike > 0.0 {
            self.opening_dev = Some((btc - strike) / strike);
        }

        if btc > strike { self.above_count += 1; }
        self.tick_count += 1;

        // prune & push CVD
        let prune = |dq: &mut VecDeque<(i64, f64)>, w: i64| {
            while dq.front().map(|(t, _)| now_sec - *t > w).unwrap_or(false) { dq.pop_front(); }
        };
        prune(&mut self.cvd_1m, 60);
        prune(&mut self.cvd_3m, 180);
        prune(&mut self.cvd_5m, 300);
        self.cvd_1m.push_back((now_sec, cvd));
        self.cvd_3m.push_back((now_sec, cvd));
        self.cvd_5m.push_back((now_sec, cvd));
    }
    fn c1(&self) -> f64 { self.cvd_1m.iter().map(|(_, v)| v).sum() }
    fn c3(&self) -> f64 { self.cvd_3m.iter().map(|(_, v)| v).sum() }
    fn c5(&self) -> f64 { self.cvd_5m.iter().map(|(_, v)| v).sum() }
    fn pct_above(&self) -> f64 {
        if self.tick_count == 0 { 0.5 } else { self.above_count as f64 / self.tick_count as f64 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OBI & signal
// ─────────────────────────────────────────────────────────────────────────────

fn calc_obi(yb: f64, ybsz: f64, ya: f64, yasz: f64, nb: f64, nbsz: f64, na: f64, nasz: f64) -> f64 {
    let tb = yb * ybsz + nb * nbsz;
    let ta = ya * yasz + na * nasz;
    let t  = tb + ta;
    if t == 0.0 { 0.0 } else { (tb - ta) / t }
}

fn calc_signal(row: &Row) -> i32 {
    let mut s: i32 = 0;
    if row.obi < -0.3 { s -= 2; } else if row.obi > 0.3 { s += 2; }
    if row.cvd5m < -50_000.0 { s -= 2; } else if row.cvd5m > 50_000.0 { s += 2; }
    let macd = row.vol_13s - row.vol_34s;
    if macd < -0.0001 { s -= 1; } else if macd > 0.0001 { s += 1; }
    if row.btc < row.strike { s -= 1; } else { s += 1; }
    if row.btc < row.btc_prev { s -= 1; } else if row.btc > row.btc_prev { s += 1; }
    s.clamp(-7, 7)
}

// ─────────────────────────────────────────────────────────────────────────────
// Colour palette
// ─────────────────────────────────────────────────────────────────────────────

const CG:   Color = Color::Rgb(0,   230, 115);
const CR:   Color = Color::Rgb(255, 70,  70);
const CY:   Color = Color::Rgb(255, 210, 40);
const CM:   Color = Color::Rgb(210, 70,  210);
const CC:   Color = Color::Rgb(60,  210, 255);
const CGR:  Color = Color::Rgb(120, 120, 135);
const CW:   Color = Color::Rgb(225, 225, 230);
const CDIM: Color = Color::Rgb(50,  50,  60);
const COR:  Color = Color::Rgb(255, 155, 35);
const CBG:  Color = Color::Rgb(10,  10,  16);
const CBG2: Color = Color::Rgb(18,  18,  28);
const CBD:  Color = Color::Rgb(45,  45,  62);
const CTL:  Color = Color::Rgb(0,   195, 155);
const CGD:  Color = Color::Rgb(255, 195, 0);
const CPK:  Color = Color::Rgb(255, 100, 180);

fn cs(v: f64) -> Color { if v > 0.0 { CG } else if v < 0.0 { CR } else { CGR } }
fn cp(n: f64, p: f64) -> Color { if n > p { CG } else if n < p { CR } else { CW } }
fn clat(ms: i64) -> Color { if ms < 200 { CG } else if ms < 800 { CY } else if ms < 2000 { COR } else { CR } }
fn cstale(ms: i64) -> Color { if ms < 500 { CG } else if ms < 1500 { CY } else if ms < 3000 { COR } else { CR } }
fn cconf(v: f64) -> Color { if v >= 0.8 { CG } else if v >= 0.6 { CY } else if v >= 0.4 { COR } else { CR } }
fn czc(abs_z: f64) -> Color { if abs_z >= 3.0 { CG } else if abs_z >= 1.5 { CY } else if abs_z >= 0.5 { COR } else { CGR } }
fn cregime(r: &str) -> Color {
    match r { "YES_EXTREME" => CG, "YES_TREND" => CTL, "NO_TREND" => COR, "NO_EXTREME" => CR, _ => CGR }
}
fn csig(s: i32) -> Color { if s <= -4 { CR } else if s <= -1 { COR } else if s == 0 { CGR } else if s <= 2 { CC } else { CG } }

fn panel<'a>(title: &'a str, accent: Color) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(title, Style::default().fg(accent).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
        ]))
        .style(Style::default().bg(CBG))
}

fn fmt_usd(v: f64) -> String {
    if v.abs() >= 1_000_000.0 { format!("{:+.2}M", v / 1e6) }
    else if v.abs() >= 1_000.0 { format!("{:+.1}K", v / 1e3) }
    else { format!("{:+.0}", v) }
}

fn lat_str(ms: Option<i64>) -> String {
    match ms { Some(v) => format!("{:>5}ms", v), None => "  ---  ".into() }
}

// Centered OBI bar: [░░░░│████░░░░] width = total chars
fn obi_bar(obi: f64, w: usize) -> Line<'static> {
    let half   = w / 2;
    let filled = ((obi.abs() * half as f64).round() as usize).min(half);
    if obi >= 0.0 {
        Line::from(vec![
            Span::styled("░".repeat(half),         Style::default().fg(CDIM)),
            Span::styled("│",                       Style::default().fg(CGR)),
            Span::styled("█".repeat(filled),        Style::default().fg(CG)),
            Span::styled("░".repeat(half - filled), Style::default().fg(CDIM)),
        ])
    } else {
        Line::from(vec![
            Span::styled("░".repeat(half - filled), Style::default().fg(CDIM)),
            Span::styled("█".repeat(filled),        Style::default().fg(CR)),
            Span::styled("│",                       Style::default().fg(CGR)),
            Span::styled("░".repeat(half),          Style::default().fg(CDIM)),
        ])
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FIX 1: render_header — NO nested layout split.
//   The outer `draw()` already reserves:
//     v[0] = Length(4)  ← header paragraph (borders included)
//     v[1] = Length(1)  ← TTX gauge (no borders)
//     v[2] = Length(3)  ← z-score panel
//     v[3] = Min(16)    ← body
//     v[4] = Length(3)  ← signal bar
//
//   render_header only renders into v[0] (a Paragraph with 2 content lines).
//   render_ttx_gauge renders into v[1] (a bare Gauge, no block).
// ─────────────────────────────────────────────────────────────────────────────

fn render_header(f: &mut Frame, area: Rect, row: &Row, hist: &History) {
    let btc_c  = cp(row.btc, row.btc_prev);
    let pct_1m = if hist.price_1m > 0.0 { (row.btc - hist.price_1m) / hist.price_1m * 100.0 } else { 0.0 };
    let ttx_s  = (row.ttx * 60.0) as i64;
    let ttx_c  = if ttx_s < 90 { CR } else if ttx_s < 240 { COR } else { CG };
    let dev_c  = cs(row.dev_usd);
    let dv     = if row.dev_usd >= 0.0 { "▲" } else { "▼" };
    let out_c  = match row.outcome.as_str() { "YES" => CG, "NO" => CR, _ => CGR };
    let vc     = match row.vol_regime.as_str() { "HIGH" => CR, "MEDIUM" => COR, _ => CTL };

    // Row 1: price | strike + deviation | outcome | TTX | slug | time
    let line1 = Line::from(vec![
        Span::styled("  ₿ ", Style::default().fg(CGD)),
        Span::styled(format!("${:.2}", row.btc),
            Style::default().fg(btc_c).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {:+.3}%", pct_1m), Style::default().fg(cs(pct_1m))),
        Span::styled("  │ Strike ", Style::default().fg(CGR)),
        Span::styled(format!("${:.2}", row.strike), Style::default().fg(CGD)),
        Span::styled(format!("  {} ${:.2} ({:+.3}%)", dv, row.dev_usd.abs(), row.dev_pct),
            Style::default().fg(dev_c).add_modifier(Modifier::BOLD)),
        Span::styled("  │ ", Style::default().fg(CDIM)),
        Span::styled(&row.outcome, Style::default().fg(out_c).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)),
        Span::styled(format!(" {:.0}%", row.out_conf * 100.0), Style::default().fg(out_c)),
        Span::styled("  │ ⏱ ", Style::default().fg(CGR)),
        Span::styled(format!("{:02}:{:02}", ttx_s / 60, ttx_s % 60),
            Style::default().fg(ttx_c).add_modifier(Modifier::BOLD)),
        Span::styled("  │ ", Style::default().fg(CDIM)),
        Span::styled(&row.slug, Style::default().fg(CM)),
        Span::styled(format!("  {}", Utc::now().format("%H:%M:%S UTC")), Style::default().fg(CGR)),
    ]);
    // Row 2: vol | regime | signal | B/A | expected move
    let line2 = Line::from(vec![
        Span::styled("  Vol ", Style::default().fg(CGR)),
        Span::styled(format!("{:.0}", row.vol_1min), Style::default().fg(CW)),
        Span::styled("  Regime ", Style::default().fg(CGR)),
        Span::styled(&row.vol_regime, Style::default().fg(vc).add_modifier(Modifier::BOLD)),
        Span::styled("  Q* ", Style::default().fg(CGR)),
        Span::styled(&row.sig_quality, Style::default().fg(CC)),
        Span::styled(format!(" {:.2}", row.sig_conf), Style::default().fg(CGR)),
        Span::styled("  B/A ", Style::default().fg(CGR)),
        Span::styled(format!("{:.2}/{:.2}", row.bnc_bid, row.bnc_ask), Style::default().fg(CW)),
        Span::styled(format!(" {:.1}bps", row.bnc_spr_bps), Style::default().fg(CGR)),
        Span::styled("  │ ExpMove ", Style::default().fg(CGR)),
        Span::styled(format!("±{:.4}%", row.exp_move),
            Style::default().fg(CY).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" (±${:.2})", row.exp_move_usd), Style::default().fg(CY)),
        Span::styled("  │ Above-strike ", Style::default().fg(CGR)),
        Span::styled(
            format!("{:.0}%", row.pct_above * 100.0),
            Style::default().fg(if row.pct_above > 0.6 { CG } else if row.pct_above < 0.4 { CR } else { CGR }),
        ),
    ]);

    let para = Paragraph::new(vec![line1, line2])
        .block(panel("BTC · POLYMARKET DASHBOARD", CC))
        .style(Style::default().bg(CBG));
    f.render_widget(para, area);
}

// FIX 1 cont.: TTX gauge rendered into its own bare row (no Block borders)
fn render_ttx_gauge(f: &mut Frame, area: Rect, row: &Row) {
    let elapsed = (row.ttx_total - row.ttx).max(0.0);
    let pct     = ((elapsed / row.ttx_total.max(0.001)) * 100.0).clamp(0.0, 100.0) as u16;
    let ttx_s   = (row.ttx * 60.0) as i64;
    let ttx_c   = if ttx_s < 90 { CR } else if ttx_s < 240 { COR } else { CTL };
    let gauge   = Gauge::default()
        .block(Block::default().borders(Borders::NONE).style(Style::default().bg(CBG)))
        .gauge_style(Style::default().fg(ttx_c).bg(CBG2))
        .percent(pct)
        .label(Span::styled(
            format!(" Window {:.0}% elapsed ", pct),
            Style::default().fg(ttx_c),
        ));
    f.render_widget(gauge, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Z-score / Confidence panel  (full-width, 1 content line)
// ─────────────────────────────────────────────────────────────────────────────

fn render_zscore(f: &mut Frame, area: Rect, row: &Row) {
    let zc   = czc(row.abs_z);
    let zdc  = if row.z_dir == "YES" { CG } else { CR };
    let cc   = cconf(row.conf.score);
    let rc   = cregime(&row.regime);
    let fc   = if row.z_flip { CG } else { COR };
    let rtdc = if row.rtds_stale { CG } else { CGR };

    // Vote bar: 5 icons ✓/✗
    let vote_spans: Vec<Span> = [
        ("avg_z", row.conf.avg_z_vote),
        ("vel",   row.conf.vel_vote),
        ("asym",  row.conf.asym_vote),
        ("open",  row.conf.open_vote),
        ("fade",  row.conf.fade_vote),
    ].iter().flat_map(|(lbl, ok)| vec![
        Span::styled(lbl.to_string(), Style::default().fg(CGR)),
        Span::styled(if *ok { "✓" } else { "✗" }, Style::default().fg(if *ok { CG } else { CR })),
        Span::raw(" "),
    ]).collect();

    let mut spans = vec![
        Span::styled("  Z ", Style::default().fg(CGR)),
        Span::styled(format!("{:+.3}", row.z),       Style::default().fg(zc).add_modifier(Modifier::BOLD)),
        Span::styled("  |z| ", Style::default().fg(CGR)),
        Span::styled(format!("{:.3}", row.abs_z),    Style::default().fg(zc)),
        Span::styled(" → ",                           Style::default().fg(CGR)),
        Span::styled(&row.z_dir,                     Style::default().fg(zdc).add_modifier(Modifier::BOLD)),
        Span::styled("  │  Conf ", Style::default().fg(CDIM)),
        Span::styled(format!("{:.0}%({}/5) ", row.conf.score * 100.0, row.conf.votes),
            Style::default().fg(cc).add_modifier(Modifier::BOLD)),
    ];
    spans.extend(vote_spans);
    spans.extend([
        Span::styled("│  Regime ", Style::default().fg(CDIM)),
        Span::styled(&row.regime, Style::default().fg(rc).add_modifier(Modifier::BOLD)),
        Span::styled("  │  z-flip ", Style::default().fg(CGR)),
        Span::styled(
            format!("{} ×{}", if row.z_flip { "YES" } else { "NO " }, row.z_flips),
            Style::default().fg(fc),
        ),
        Span::styled("  │  RTDS ", Style::default().fg(CGR)),
        Span::styled(if row.rtds_stale { "STALE⚡" } else { "fresh " }, Style::default().fg(rtdc).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {:.1}bps", row.rtds_adv), Style::default().fg(CPK)),
        Span::styled("  │  fade ", Style::default().fg(CGR)),
        Span::styled(format!("{:.0}%", row.conf.fade_ratio * 100.0),
            Style::default().fg(if row.conf.fade_ratio >= 0.3 { CG } else { CR })),
    ]);

    let para = Paragraph::new(vec![Line::from(spans)])
        .block(panel("Z-SCORE · CONFIDENCE · REGIME", CPK))
        .style(Style::default().bg(CBG));
    f.render_widget(para, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Order book panel
// ─────────────────────────────────────────────────────────────────────────────

fn render_book(f: &mut Frame, area: Rect, row: &Row) {
    let oc  = if row.obi < -0.2 { CR } else if row.obi > 0.2 { CG } else { CGR };
    let lbl = if row.obi < -0.15 { "BEARISH" } else if row.obi > 0.15 { "BULLISH" } else { "NEUTRAL" };

    let combined     = row.yes_ask + row.no_ask;
    let combined_c   = if combined < 1.0 { CG } else { CR };
    let arb_flag     = if combined < 1.0 { "  ✅ ARB!" } else { "" };

    let lines = vec![
        Line::from(vec![
            Span::styled(" OBI ", Style::default().fg(CM).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:+.1}%", row.obi * 100.0), Style::default().fg(oc).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", lbl), Style::default().fg(oc)),
        ]),
        obi_bar(row.obi, 26),
        Line::from(""),
        Line::from(vec![Span::styled(" ─ YES ────────────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  BID ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.yes_bid),    Style::default().fg(CG)),
            Span::styled(format!(" ×{:.0}", row.yes_bid_sz), Style::default().fg(CDIM)),
            Span::styled("  ASK ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.yes_ask),    Style::default().fg(CR)),
            Span::styled(format!(" ×{:.0}", row.yes_ask_sz), Style::default().fg(CDIM)),
        ]),
        Line::from(vec![
            Span::styled("  Spr ", Style::default().fg(CGR)),
            Span::styled(format!("{:.1}bps", row.yes_spr_bps), Style::default().fg(CY)),
            Span::styled("  Fee ", Style::default().fg(CGR)),
            Span::styled(format!("{:.3}%", row.yes_fee), Style::default().fg(COR)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ─ NO ─────────────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  BID ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.no_bid),  Style::default().fg(CG)),
            Span::styled(format!(" ×{:.0}", row.no_bid_sz), Style::default().fg(CDIM)),
            Span::styled("  ASK ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.no_ask),  Style::default().fg(CR)),
            Span::styled(format!(" ×{:.0}", row.no_ask_sz), Style::default().fg(CDIM)),
        ]),
        Line::from(vec![
            Span::styled("  Spr ", Style::default().fg(CGR)),
            Span::styled(format!("{:.1}bps", row.no_spr_bps), Style::default().fg(CY)),
            Span::styled("  Fee ", Style::default().fg(CGR)),
            Span::styled(format!("{:.3}%", row.no_fee), Style::default().fg(COR)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ─ COMBINED ───────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  YES+NO ask ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", combined),
                Style::default().fg(combined_c).add_modifier(Modifier::BOLD)),
            Span::styled(arb_flag, Style::default().fg(CG).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ─ BINANCE ────────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  BID ", Style::default().fg(CGR)),
            Span::styled(format!("${:.2}", row.bnc_bid), Style::default().fg(CG)),
            Span::styled("  ASK ", Style::default().fg(CGR)),
            Span::styled(format!("${:.2}", row.bnc_ask), Style::default().fg(CR)),
            Span::styled(format!("  {:.1}bps", row.bnc_spr_bps), Style::default().fg(CY)),
        ]),
    ];

    f.render_widget(
        Paragraph::new(lines).block(panel("ORDER BOOK", CBD)).style(Style::default().bg(CBG)),
        area,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Flow / CVD / RTDS panel
// ─────────────────────────────────────────────────────────────────────────────

fn render_flow(f: &mut Frame, area: Rect, row: &Row, hist: &History) {
    let c1 = hist.c1(); let c3 = hist.c3(); let c5 = hist.c5();
    let vc = match row.vol_regime.as_str() { "HIGH" => CR, "MEDIUM" => COR, _ => CTL };
    let sc = if row.vol_spike { CR } else { CGR };

    let lines = vec![
        Line::from(vec![Span::styled(" ─ CVD ─────────────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  1m ", Style::default().fg(CGR)),
            Span::styled(fmt_usd(c1), Style::default().fg(cs(c1)).add_modifier(Modifier::BOLD)),
            Span::styled("  3m ", Style::default().fg(CGR)),
            Span::styled(fmt_usd(c3), Style::default().fg(cs(c3))),
            Span::styled("  5m ", Style::default().fg(CGR)),
            Span::styled(fmt_usd(c5), Style::default().fg(cs(c5))),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ─ VOLUME ──────────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  1s ", Style::default().fg(CGR)),
            Span::styled(format!("{:.0}", row.vol_1sec), Style::default().fg(CW)),
            Span::styled("  1m ", Style::default().fg(CGR)),
            Span::styled(format!("{:.0}", row.vol_1min), Style::default().fg(CW)),
            if row.vol_spike {
                Span::styled("  🔥 SPIKE", Style::default().fg(sc).add_modifier(Modifier::BOLD))
            } else {
                Span::raw("")
            },
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ─ VOLATILITY ──────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  1s ",  Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.vol_1s),  Style::default().fg(CC)),
            Span::styled("  5s ",  Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.vol_5s),  Style::default().fg(CC)),
        ]),
        Line::from(vec![
            Span::styled("  13s ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.vol_13s), Style::default().fg(CC)),
            Span::styled("  34s ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.vol_34s), Style::default().fg(CC)),
            Span::styled("  89s ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.vol_89s), Style::default().fg(CC)),
        ]),
        Line::from(vec![
            Span::styled("  RealVol ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}%", row.realized_vol * 100.0),
                Style::default().fg(CGD).add_modifier(Modifier::BOLD)),
            Span::styled("  Regime ", Style::default().fg(CGR)),
            Span::styled(&row.vol_regime, Style::default().fg(vc).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ─ RTDS ────────────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  Adv ", Style::default().fg(CGR)),
            Span::styled(
                format!("{:.2}bps", row.rtds_adv),
                Style::default().fg(if row.rtds_stale { CG } else { CGR }),
            ),
            Span::styled("  CL-dev ", Style::default().fg(CGR)),
            Span::styled(format!("{:.2}bps", row.rtds_cl_dev), Style::default().fg(cs(row.rtds_cl_dev))),
        ]),
        Line::from(vec![
            Span::styled("  B-stale ", Style::default().fg(CGR)),
            Span::styled(format!("{}ms", row.rtds_bnc_stale_ms), Style::default().fg(cstale(row.rtds_bnc_stale_ms))),
            Span::styled("  CL-stale ", Style::default().fg(CGR)),
            Span::styled(format!("{}ms", row.rtds_cl_stale_ms), Style::default().fg(cstale(row.rtds_cl_stale_ms))),
        ]),
    ];

    f.render_widget(
        Paragraph::new(lines).block(panel("FLOW · CVD · RTDS", CBD)).style(Style::default().bg(CBG)),
        area,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Greeks + Fees + Latency panel
// FIX 2: latency = now_ms - update_ms (relative age). B↔P shows sane_latency().
// ─────────────────────────────────────────────────────────────────────────────

fn render_greeks(f: &mut Frame, area: Rect, row: &Row) {
    // FIX 2: relative ages — epoch fields minus now_ms
    let b_age  = (row.now_ms - row.bnc_update_ms).clamp(0, MAX_SANE_LATENCY_MS);
    let py_age = (row.now_ms - row.poly_yes_ms).clamp(0, MAX_SANE_LATENCY_MS);
    let pn_age = (row.now_ms - row.poly_no_ms).clamp(0, MAX_SANE_LATENCY_MS);
    // B↔P delta: only display if within sane range
    let bvp_str = match sane_latency(row.bvp_delta_ms) {
        Some(v) => format!("{:>+6}ms", v),
        None    => "  ~~~  ".into(),  // cross-clock garbage — show ~~~ not a huge number
    };
    let bvp_c = sane_latency(row.bvp_delta_ms)
        .map(|v| clat(v.abs())).unwrap_or(CGR);

    let lines = vec![
        Line::from(vec![Span::styled(" ─ GREEKS ─────────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  Δ ", Style::default().fg(CGR)),
            Span::styled(format!("{:.8}", row.delta), Style::default().fg(CC).add_modifier(Modifier::BOLD)),
            Span::styled("  Γ ", Style::default().fg(CGR)),
            Span::styled(format!("{:.8}", row.gamma), Style::default().fg(CW)),
        ]),
        Line::from(vec![
            Span::styled("  ν ", Style::default().fg(CGR)),
            Span::styled(format!("{:.8}", row.vega),  Style::default().fg(CW)),
            Span::styled("  Θ ", Style::default().fg(CGR)),
            Span::styled(format!("{:.8}", row.theta), Style::default().fg(cs(row.theta))),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ─ FEES ───────────────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  YES ask ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.yes_ask), Style::default().fg(CW)),
            Span::styled(format!("  fee {:.3}%", row.yes_fee), Style::default().fg(COR)),
        ]),
        Line::from(vec![
            Span::styled("  NO  ask ", Style::default().fg(CGR)),
            Span::styled(format!("{:.4}", row.no_ask), Style::default().fg(CW)),
            Span::styled(format!("  fee {:.3}%", row.no_fee), Style::default().fg(COR)),
        ]),
        Line::from(vec![
            Span::styled("  Combined", Style::default().fg(CGR)),
            Span::styled(
                format!(" {:.4}", row.yes_ask + row.no_ask),
                Style::default().fg(if row.yes_ask + row.no_ask < 1.0 { CG } else { CR })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if row.yes_ask + row.no_ask < 1.0 { "  ✅ ARB" } else { "" },
                Style::default().fg(CG).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ─ LATENCY (age) ──────────────", Style::default().fg(CDIM))]),
        Line::from(vec![
            Span::styled("  Binance  ", Style::default().fg(CGR)),
            Span::styled(lat_str(Some(b_age)),  Style::default().fg(clat(b_age))),
            Span::styled(" ago", Style::default().fg(CDIM)),
        ]),
        Line::from(vec![
            Span::styled("  Poly YES ", Style::default().fg(CGR)),
            Span::styled(lat_str(Some(py_age)), Style::default().fg(clat(py_age))),
            Span::styled("  NO ", Style::default().fg(CGR)),
            Span::styled(lat_str(Some(pn_age)), Style::default().fg(clat(pn_age))),
        ]),
        // FIX 2: B↔P shows ~~~ when cross-clock garbage instead of 85000000ms
        Line::from(vec![
            Span::styled("  B↔P Δ   ", Style::default().fg(CGR)),
            Span::styled(bvp_str, Style::default().fg(bvp_c)),
        ]),
        Line::from(vec![
            Span::styled("  RTDS-B  ", Style::default().fg(CGR)),
            Span::styled(format!("{:>5}ms", row.rtds_bnc_lag), Style::default().fg(clat(row.rtds_bnc_lag))),
            Span::styled("  CL ", Style::default().fg(CGR)),
            Span::styled(format!("{:>5}ms", row.rtds_cl_lag), Style::default().fg(clat(row.rtds_cl_lag))),
        ]),
    ];

    f.render_widget(
        Paragraph::new(lines).block(panel("GREEKS · FEES · LATENCY", CBD)).style(Style::default().bg(CBG)),
        area,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal bar
// ─────────────────────────────────────────────────────────────────────────────

fn render_signals(f: &mut Frame, area: Rect, row: &Row, hist: &History) {
    let sc    = csig(row.sig_score);
    let lbl   = if row.sig_score <= -4 { "STRONG BEAR" }
        else if row.sig_score <= -2 { "BEARISH" }
        else if row.sig_score == -1 { "LEAN BEAR" }
        else if row.sig_score ==  0 { "NEUTRAL" }
        else if row.sig_score ==  1 { "LEAN BULL" }
        else if row.sig_score <=  3 { "BULLISH" }
        else                        { "STRONG BULL" };
    let pct   = ((row.sig_score + 7) as f64 / 14.0 * 100.0) as u16;
    let c1    = hist.c1(); let c5 = hist.c5();
    let oc    = match row.outcome.as_str() { "YES" => CG, "NO" => CR, _ => CGR };
    let rc    = cregime(&row.regime);
    let macd  = row.vol_13s - row.vol_34s;

    let hh = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)]).split(area);

    let gauge = Gauge::default()
        .block(panel("SIGNAL", CBD))
        .gauge_style(Style::default().fg(sc).bg(CBG2))
        .percent(pct)
        .label(Span::styled(
            format!(" {} ({:+}/7) ", lbl, row.sig_score),
            Style::default().fg(sc).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(gauge, hh[0]);

    let line = Line::from(vec![
        Span::styled("  OBI ",  Style::default().fg(CGR)),
        Span::styled(format!("{:+.1}%", row.obi * 100.0), Style::default().fg(cs(row.obi))),
        Span::styled("  │ CVD1m ", Style::default().fg(CGR)),
        Span::styled(fmt_usd(c1), Style::default().fg(cs(c1))),
        Span::styled("  5m ", Style::default().fg(CGR)),
        Span::styled(fmt_usd(c5), Style::default().fg(cs(c5))),
        Span::styled("  │ Z ", Style::default().fg(CGR)),
        Span::styled(format!("{:+.3}→{}", row.z, row.z_dir),
            Style::default().fg(czc(row.abs_z)).add_modifier(Modifier::BOLD)),
        Span::styled("  Conf ", Style::default().fg(CGR)),
        Span::styled(format!("{:.0}%", row.conf.score * 100.0), Style::default().fg(cconf(row.conf.score))),
        Span::styled("  │ Regime ", Style::default().fg(CGR)),
        Span::styled(&row.regime, Style::default().fg(rc)),
        Span::styled("  │ ", Style::default().fg(CDIM)),
        Span::styled(&row.outcome, Style::default().fg(oc).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)),
        Span::styled(format!(" {:.0}%  y:{:.4} n:{:.4}", row.out_conf * 100.0, row.yes_bid, row.no_bid),
            Style::default().fg(oc)),
        Span::styled(
            format!("  {} ${:.2}({:+.3}%)", if row.dev_usd >= 0.0 { "▲" } else { "▼" }, row.dev_usd.abs(), row.dev_pct),
            Style::default().fg(cs(row.dev_usd)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  EM ±{:.4}%", row.exp_move), Style::default().fg(CY)),
    ]);

    f.render_widget(
        Paragraph::new(vec![Line::raw(""), line])
            .block(panel("SIGNALS · OUTCOME", CBD))
            .style(Style::default().bg(CBG)),
        hh[1],
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// FIX 1: Root draw — outer split owns TTX gauge row.
// Layout (top to bottom):
//   Length(4)  = header block  (2 text lines + 2 border rows)
//   Length(1)  = TTX gauge     (NO block borders — pure gauge fill)
//   Length(3)  = z-score block (1 text line + 2 border rows)
//   Min(16)    = body
//   Length(3)  = signal block  (1 text line + 2 border rows)
// ─────────────────────────────────────────────────────────────────────────────

fn draw(f: &mut Frame, row: &Row, hist: &History) {
    let area = f.area();
    let v = Layout::default().direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),  // header (2 lines + 2 border)
            Constraint::Length(1),  // TTX gauge (borderless)
            Constraint::Length(3),  // z-score panel (1 line + 2 border)
            Constraint::Min(16),    // body
            Constraint::Length(3),  // signal bar (1 line + 2 border)
        ])
        .split(area);

    render_header(f, v[0], row, hist);
    render_ttx_gauge(f, v[1], row);
    render_zscore(f, v[2], row);

    // Body: 44% order book | 56% right column
    let body = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
        .split(v[3]);

    let right = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(body[1]);

    render_book(f, body[0], row);
    render_flow(f, right[0], row, hist);
    render_greeks(f, right[1], row);

    render_signals(f, v[4], row, hist);
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("btc_dashboard=warn,combine_fetcher=warn")
        .init();

    let args: Vec<String> = std::env::args().collect();
    let dur: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(86400);

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(3))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?;

    let mut market = fetch_current_market(&http).await?;
    let mut bnc    = BinanceDataManager::new();
    bnc.connect(&["BTCUSDT"], None).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = bnc.bootstrap_volatility("BTCUSDT", 70).await;

    let sk = fetch_strike(&http, &market).await?;
    market.strike_price = Some(sk); market.strike_fetched = true;

    let poly = Arc::new(PolymarketOrderbookStream::new());
    let mut rx = poly.clone().stream(vec![market.yes_token_id.clone(), market.no_token_id.clone()]).await?;
    tokio::spawn(async move { while let Some(_) = rx.recv().await {} });
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut rtds = RtdsTracker::new();
    rtds.start();
    tokio::time::sleep(Duration::from_secs(2)).await;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;

    let mut hist     = History::default();
    let mut prev_btc = 0.0_f64;
    let mut temp_csv = TempCsv::new();
    let end_t  = SystemTime::now() + Duration::from_secs(dur);
    let mut fail_n = 0u32;
    let mut last_fetch_ms = 0i64;

    loop {
        if SystemTime::now() >= end_t { break; }
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(k) = event::read()? {
                if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) { break; }
            }
        }

        let now    = Utc::now();
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        let now_s  = now.timestamp();

        let snap = match bnc.get_market_snapshot("BTCUSDT").await {
            Some(s) => s,
            None    => { tokio::time::sleep(Duration::from_millis(16)).await; continue; }
        };

        let yes_p = poly.get_prices(&market.yes_token_id);
        let no_p  = poly.get_prices(&market.no_token_id);
        let (b_bid, b_ask) = bnc.get_bid_ask("BTCUSDT").await.unwrap_or((snap.spot_price, snap.spot_price));

        let ttx      = ((market.window_end - now_s) as f64 / 60.0).max(0.0);
        let resolved = ttx <= 0.0;
        let strike   = market.strike_price.unwrap_or(snap.spot_price);
        let g        = calculate_greeks(snap.spot_price, strike, ttx, snap.realized_volatility);

        // Extract poly prices
        let y_bid  = yes_p.as_ref().and_then(|p| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
        let y_ask  = yes_p.as_ref().and_then(|p| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
        let n_bid  = no_p.as_ref().and_then(|p| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
        let n_ask  = no_p.as_ref().and_then(|p| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
        let y_bsz  = yes_p.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0);
        let y_asz  = yes_p.as_ref().map(|p| p.best_ask_size).unwrap_or(0.0);
        let n_bsz  = no_p.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0);
        let n_asz  = no_p.as_ref().map(|p| p.best_ask_size).unwrap_or(0.0);
        // FIX 2: store raw epoch-ms for poly feeds
        let py_ms  = yes_p.as_ref().map(|p| p.last_update_ms).unwrap_or(now_ms);
        let pn_ms  = no_p.as_ref().map(|p| p.last_update_ms).unwrap_or(now_ms);

        let rtds_s = rtds.get_snapshot(snap.spot_price, snap.timestamp);

        // Z-score
        let (z, abs_z, z_dir) = z_score(snap.spot_price, strike, snap.realized_volatility, ttx);
        let is_yes = z_dir == "YES";
        let em_pct = exp_move_pct(snap.realized_volatility, ttx);

        // CVD
        let sign   = if snap.spot_price > prev_btc { 1.0 } else if snap.spot_price < prev_btc { -1.0 } else { 0.0 };
        let cvd_tk = snap.volume_1sec * snap.spot_price * sign;
        hist.push(snap.spot_price, z, is_yes, now_s, cvd_tk, strike);

        let obi_v   = calc_obi(y_bid, y_bsz, y_ask, y_asz, n_bid, n_bsz, n_ask, n_asz);
        let outcome = if resolved { if snap.spot_price >= strike { "YES" } else { "NO" } } else { "OPEN" };
        let conf    = compute_confidence(&hist.z_hist, is_yes, hist.opening_dev.unwrap_or(0.0));
        let o_conf  = if outcome != "OPEN" {
            if outcome == "YES" { g.delta } else { 1.0 - g.delta }
        } else {
            (g.delta - 0.5).abs() * 2.0
        };

        // FIX 2: B↔P delta from raw epoch-ms — may be cross-clock garbage;
        //         sane_latency() will return None if > 10s, display as ~~~
        let bvp_raw = snap.timestamp - (py_ms + pn_ms) / 2;

        let mut row = Row {
            ts:      now.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
            slug:    market.slug.clone(),
            now_ms,
            btc:     snap.spot_price,
            btc_prev: prev_btc,
            bnc_bid:  b_bid,
            bnc_ask:  b_ask,
            // FIX 3: use correct field name from CsvRecord: binance_spread_bps → spread_bps on snapshot
            bnc_spr_bps: snap.spread_bps,
            vol_1min:    snap.volume_1min,
            vol_1sec:    snap.volume_1sec,
            vol_spike:   snap.volume_spike,
            yes_bid: y_bid, yes_bid_sz: y_bsz, yes_ask: y_ask, yes_ask_sz: y_asz,
            no_bid:  n_bid, no_bid_sz:  n_bsz, no_ask:  n_ask, no_ask_sz:  n_asz,
            yes_spr_bps: spread_bps(y_bid, y_ask),
            no_spr_bps:  spread_bps(n_bid, n_ask),
            yes_fee: poly_fee_pct(y_ask),
            no_fee:  poly_fee_pct(n_ask),
            strike, vol: snap.realized_volatility, ttx, ttx_total: 15.0,
            delta: g.delta, gamma: g.gamma, vega: g.vega, theta: g.theta,
            z, abs_z, z_dir: z_dir.to_string(),
            z_flip:  hist.z_flipped,
            z_flips: hist.z_flips,
            exp_move:     em_pct,
            exp_move_usd: strike * em_pct / 100.0,
            dev_usd: snap.spot_price - strike,
            dev_pct: (snap.spot_price - strike) / strike * 100.0,
            regime:  classify_regime(y_ask).to_string(),
            time_pct: 1.0 - (ttx / 15.0).clamp(0.0, 1.0),
            // FIX 3: pct_above guarded by History::pct_above() which checks tick_count>0
            pct_above: hist.pct_above(),
            conf,
            outcome:   outcome.to_string(),
            out_conf:  o_conf,
            vol_1s:  snap.vol_1sec,
            vol_5s:  snap.vol_5sec,
            vol_13s: snap.vol_13sec,
            vol_34s: snap.vol_34sec,
            vol_89s: snap.vol_89sec,
            vol_regime:   snap.volatility_regime.clone(),
            realized_vol: snap.realized_volatility,
            sig_quality:  snap.signal_quality.clone(),
            sig_conf:     snap.trade_confidence,
            rtds_adv:         rtds_s.rtds_price_advantage_bps,
            rtds_cl_dev:      rtds_s.rtds_chainlink_dev_bps,
            // FIX 3: BPS-mode stale — sanity cap at 50bps to reject data errors
            rtds_stale:       rtds_s.rtds_price_advantage_bps >= RTDS_ADV_THRESH
                              && rtds_s.rtds_price_advantage_bps < 50.0,
            rtds_bnc_lag:     rtds_s.rtds_binance_lag_ms,
            rtds_cl_lag:      rtds_s.rtds_chainlink_lag_ms,
            rtds_bnc_stale_ms: rtds_s.rtds_binance_stale_ms,
            rtds_cl_stale_ms:  rtds_s.rtds_chainlink_stale_ms,
            bnc_update_ms: snap.timestamp,
            poly_yes_ms:   py_ms,
            poly_no_ms:    pn_ms,
            bvp_delta_ms:  bvp_raw,
            obi:   obi_v,
            cvd1m: hist.c1(), cvd3m: hist.c3(), cvd5m: hist.c5(),
            sig_score: 0,
        };
        row.sig_score = calc_signal(&row);

        let _ = temp_csv.ensure(&market.slug, &now);
        let _ = temp_csv.write(&row);

        terminal.draw(|f| draw(f, &row, &hist))?;
        prev_btc = snap.spot_price;

        // Market rotation
        if resolved {
            let since = now_ms.saturating_sub(last_fetch_ms);
            if fail_n == 0 || since >= 5_000 {
                last_fetch_ms = now_ms;
                match fetch_next_market_after(&http, market.window_start).await {
                    Ok(mut next) => {
                        fail_n = 0;
                        let s = fetch_strike(&http, &next).await?;
                        next.strike_price = Some(s); next.strike_fetched = true;
                        let mut nrx = poly.clone()
                            .stream(vec![next.yes_token_id.clone(), next.no_token_id.clone()])
                            .await?;
                        tokio::spawn(async move { while let Some(_) = nrx.recv().await {} });
                        hist    = History::default();
                        market  = next;
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Err(_) => {
                        fail_n += 1;
                        if fail_n >= 10 { break; }
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    println!("\n✅ Dashboard exited.");
    Ok(())
}
