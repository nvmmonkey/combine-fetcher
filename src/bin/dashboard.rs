// btc_dashboard.rs  — Real-time BTC/Polymarket dashboard (no persistent CSV)
//
// cargo run --bin btc_dashboard -- [duration_secs]
// Press q or Esc to exit
//
// Layout:
//   ┌─ HEADER ──────────────────────────────────────────────────────────────┐
//   │  BTC price · 1m/24h Δ · slug · TTX progress gauge · outcome deviation │
//   ├─ ORDER BOOK ──────────────┬─ FLOW · CVD · VOLUME ─────────────────────┤
//   │  OBI bar · YES/NO/BINANCE │  CVD bars · Vol · Volatility regime       │
//   ├─ TECHNICALS · RTDS ───────┼─ GREEKS · LATENCY ────────────────────────┤
//   │  RSI · MACD · EMA · VWAP  │  Δ Γ ν Θ · fixed relative latency        │
//   ├─ OUTCOME ─────────────────┴─ SIGNAL STRENGTH ────────────────────────┤
//   │  price vs strike · deviation · resolved outcome                        │
//   └────────────────────────────────────────────────────────────────────────┘

use anyhow::{anyhow, Result};
use chrono::{DateTime, Timelike, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Gauge, Paragraph, Wrap},
    Frame, Terminal,
};
use rust_decimal::prelude::*;
use std::{
    collections::VecDeque,
    fs,
    io,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::{info, warn};

use combine_fetcher::api::binance::BinanceDataManager;
use combine_fetcher::api::polymarket::PolymarketOrderbookStream;
use combine_fetcher::api::rtds_tracker::{RtdsSnapshot, RtdsTracker};
use combine_fetcher::{
    CSV_RECORD_INTERVAL_SEC, GAMMA_API_URL, HTTP_TIMEOUT_SECS, MARKET_WINDOW_SECONDS,
    MINUTES_PER_YEAR, POLY_CRYPTO_API_URL, STRIKE_RETRY_INTERVAL_MS,
};

// ─────────────────────────────────────────────────────────────────────────────
// Types (identical to logger — no duplication of SDK types)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MarketInfo {
    pub unique_id: String,
    pub slug: String,
    pub condition_id: String,
    pub yes_token_id: String,
    pub no_token_id: String,
    pub window_start: i64,
    pub window_end: i64,
    pub strike_price: Option<f64>,
    pub strike_fetched: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Greeks {
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub theta: f64,
}

fn calculate_spread_bps(bid: f64, ask: f64) -> f64 {
    if bid <= 0.0 || ask <= bid { return 0.0; }
    let mid = (bid + ask) / 2.0;
    if mid <= 0.0 { return 0.0; }
    ((ask - bid) / mid) * 10_000.0
}

fn calculate_greeks(spot: f64, strike: f64, ttx_min: f64, vol: f64) -> Greeks {
    if ttx_min <= 0.01 || vol <= 0.0 || spot <= 0.0 || strike <= 0.0 {
        return Greeks { delta: if spot >= strike { 1.0 } else { 0.0 }, ..Default::default() };
    }
    let t = ttx_min / MINUTES_PER_YEAR;
    let sqrt_t = t.sqrt();
    let d1 = ((spot / strike).ln() + (vol * vol / 2.0) * t) / (vol * sqrt_t);
    let d2 = ((spot / strike).ln() - (vol * vol / 2.0) * t) / (vol * sqrt_t);
    let norm_pdf = |x: f64| (-x * x / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt();
    let npd2 = norm_pdf(d2);
    Greeks {
        delta: (npd2 / (spot * vol * sqrt_t)).clamp(0.0, 1.0),
        gamma: (npd2 * d1 / (spot * spot * vol * vol * t)).clamp(-1e6, 1e6),
        vega:  (npd2 * sqrt_t * 0.01).clamp(-1e6, 1e6),
        theta: (-(spot * npd2 * vol) / (2.0 * sqrt_t) / 365.0).clamp(-1e6, 1e6),
    }
}

pub fn truncate_id(id: &str, first: usize, last: usize) -> String {
    let len = id.len();
    if len <= first + last { return id.to_string(); }
    format!("{}…{}", &id[..first], &id[len - last..])
}

// ─────────────────────────────────────────────────────────────────────────────
// Market fetchers (identical logic to logger)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct PolymarketStrikeResponse {
    #[serde(rename = "openPrice")]
    open_price: Option<f64>,
}

async fn fetch_current_market(client: &reqwest::Client) -> Result<MarketInfo> {
    let now = Utc::now().timestamp();
    let pos = now % MARKET_WINDOW_SECONDS;
    let window_start = if pos < 60 {
        ((now / MARKET_WINDOW_SECONDS) - 1) * MARKET_WINDOW_SECONDS
    } else {
        (now / MARKET_WINDOW_SECONDS) * MARKET_WINDOW_SECONDS
    };
    let window_end = window_start + MARKET_WINDOW_SECONDS;
    let slug = format!("btc-updown-15m-{}", window_start);
    let url  = format!("{}/events/slug/{}", GAMMA_API_URL, slug);
    let resp = client.get(&url).timeout(Duration::from_secs(HTTP_TIMEOUT_SECS)).send().await?;
    if !resp.status().is_success() { return Err(anyhow!("Market fetch failed: {}", resp.status())); }
    let data: serde_json::Value = resp.json().await?;
    let markets = data["markets"].as_array().ok_or_else(|| anyhow!("No markets"))?;
    for m in markets {
        if !m["active"].as_bool().unwrap_or(false) { continue; }
        let outcomes: Vec<String> = serde_json::from_str(m["outcomes"].as_str().unwrap_or("[]")).unwrap_or_default();
        let tokens:   Vec<String> = serde_json::from_str(m["clobTokenIds"].as_str().unwrap_or("[]")).unwrap_or_default();
        if outcomes.len() != 2 || tokens.len() != 2 { continue; }
        if let (Some(ui), Some(di)) = (outcomes.iter().position(|o| o == "Up"), outcomes.iter().position(|o| o == "Down")) {
            return Ok(MarketInfo {
                unique_id: slug.clone(), slug,
                condition_id: m["conditionId"].as_str().unwrap_or("").to_string(),
                yes_token_id: tokens[ui].clone(), no_token_id: tokens[di].clone(),
                window_start, window_end, strike_price: None, strike_fetched: false,
            });
        }
    }
    Err(anyhow!("No valid BTC market"))
}

async fn fetch_next_market_after(client: &reqwest::Client, current_ws: i64) -> Result<MarketInfo> {
    let next_ws = current_ws + MARKET_WINDOW_SECONDS;
    let slug = format!("btc-updown-15m-{}", next_ws);
    let url  = format!("{}/events/slug/{}", GAMMA_API_URL, slug);
    let resp = client.get(&url).timeout(Duration::from_secs(HTTP_TIMEOUT_SECS)).send().await?;
    if !resp.status().is_success() { return Err(anyhow!("Market fetch failed: {}", resp.status())); }
    let data: serde_json::Value = resp.json().await?;
    let markets = data["markets"].as_array().ok_or_else(|| anyhow!("No markets"))?;
    for m in markets {
        if !m["active"].as_bool().unwrap_or(false) { continue; }
        let outcomes: Vec<String> = serde_json::from_str(m["outcomes"].as_str().unwrap_or("[]")).unwrap_or_default();
        let tokens:   Vec<String> = serde_json::from_str(m["clobTokenIds"].as_str().unwrap_or("[]")).unwrap_or_default();
        if outcomes.len() != 2 || tokens.len() != 2 { continue; }
        if let (Some(ui), Some(di)) = (outcomes.iter().position(|o| o == "Up"), outcomes.iter().position(|o| o == "Down")) {
            return Ok(MarketInfo {
                unique_id: slug.clone(), slug,
                condition_id: m["conditionId"].as_str().unwrap_or("").to_string(),
                yes_token_id: tokens[ui].clone(), no_token_id: tokens[di].clone(),
                window_start: next_ws, window_end: next_ws + MARKET_WINDOW_SECONDS,
                strike_price: None, strike_fetched: false,
            });
        }
    }
    Err(anyhow!("No valid next BTC market"))
}

async fn fetch_strike_with_retry(client: &reqwest::Client, market: &MarketInfo) -> Result<f64> {
    let start = DateTime::from_timestamp(market.window_start, 0).ok_or_else(|| anyhow!("Bad ts"))?;
    let end   = DateTime::from_timestamp(market.window_end,   0).ok_or_else(|| anyhow!("Bad ts"))?;
    let url = format!(
        "{}/crypto-price?symbol=BTC&eventStartTime={}&variant=fifteen&endDate={}",
        POLY_CRYPTO_API_URL,
        start.format("%Y-%m-%dT%H:%M:%SZ"),
        end.format("%Y-%m-%dT%H:%M:%SZ")
    );
    let mut backoff_ms = STRIKE_RETRY_INTERVAL_MS;
    loop {
        if let Ok(resp) = client.get(&url).timeout(Duration::from_secs(HTTP_TIMEOUT_SECS)).send().await {
            if resp.status().is_success() {
                if let Ok(data) = resp.json::<PolymarketStrikeResponse>().await {
                    if let Some(price) = data.open_price { return Ok(price); }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        backoff_ms = (backoff_ms * 2).min(30_000);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Temp CSV (slug-keyed, auto-delete on new slug)
// ─────────────────────────────────────────────────────────────────────────────

struct TempCsvWriter {
    current_slug: String,
    current_path: PathBuf,
    writer: Option<std::io::BufWriter<std::fs::File>>,
}
use std::io::Write;

impl TempCsvWriter {
    fn new() -> Self {
        Self { current_slug: String::new(), current_path: PathBuf::new(), writer: None }
    }
    fn ensure_slug(&mut self, slug: &str, now: &DateTime<Utc>) -> Result<()> {
        if slug == self.current_slug { return Ok(()); }
        if let Some(mut w) = self.writer.take() { let _ = w.flush(); }
        if self.current_path.exists() { let _ = fs::remove_file(&self.current_path); }
        let path = PathBuf::from(format!("temp_btc_{}_{}.csv", slug, now.format("%Y-%m-%dT%H-%M-%S")));
        let file = std::fs::OpenOptions::new().create(true).write(true).open(&path)?;
        let mut w = std::io::BufWriter::with_capacity(32 * 1024, file);
        writeln!(w, "timestamp,market_slug,binance_price,yes_bid,yes_ask,no_bid,no_ask,\
                      strike_price,ttx_min,delta_yes,realized_volatility,signal_quality,\
                      trade_confidence,price_vs_strike_pct,outcome_projection")?;
        self.current_slug = slug.to_string();
        self.current_path = path;
        self.writer = Some(w);
        Ok(())
    }
    fn write_row(&mut self, r: &DashRow) -> Result<()> {
        if let Some(ref mut w) = self.writer {
            writeln!(w,
                "{},{},{:.2},{:.4},{:.4},{:.4},{:.4},{:.2},{:.2},{:.8},{:.6},{},{:.4},{:.4},{}",
                r.timestamp, r.slug, r.binance_price,
                r.yes_bid, r.yes_ask, r.no_bid, r.no_ask,
                r.strike_price, r.ttx_min, r.delta_yes, r.realized_vol,
                r.signal_quality, r.trade_confidence,
                r.price_vs_strike_pct,
                r.outcome_projection,
            )?;
            w.flush()?;
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Dashboard state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct DashRow {
    // timing
    pub timestamp: String,
    pub slug: String,
    pub ttx_min: f64,
    pub ttx_total_min: f64,   // full window duration (15.0)
    pub time_to_open: i64,
    pub market_resolved: bool,
    pub outcome: String,
    pub window_start: i64,
    pub window_end: i64,
    pub now_ms: i64,          // ← local clock snapshot for relative latency

    // price
    pub binance_price: f64,
    pub binance_price_prev: f64,
    pub binance_bid: f64,
    pub binance_ask: f64,
    pub binance_spread_bps: f64,
    pub binance_volume: f64,
    pub volume_1sec: f64,
    pub volume_spike: bool,

    // polymarket
    pub yes_bid: f64,  pub yes_bid_size: f64,
    pub yes_ask: f64,  pub yes_ask_size: f64,
    pub no_bid: f64,   pub no_bid_size: f64,
    pub no_ask: f64,   pub no_ask_size: f64,
    pub yes_spread_bps: f64,
    pub no_spread_bps: f64,

    // greeks / options
    pub strike_price: f64,
    pub realized_vol: f64,
    pub delta_yes: f64,
    pub gamma: f64,
    pub vega: f64,
    pub theta: f64,

    // outcome deviation  ← NEW
    pub price_vs_strike_usd: f64,   // binance_price - strike  (signed)
    pub price_vs_strike_pct: f64,   // (binance_price - strike) / strike * 100
    pub outcome_projection: String, // "YES" / "NO" / "OPEN"
    pub outcome_confidence: f64,    // 0..1  based on delta_yes

    // volatility
    pub vol_1sec: f64,  pub vol_5sec: f64,
    pub vol_13sec: f64, pub vol_34sec: f64, pub vol_89sec: f64,
    pub volatility_regime: String,

    // signal
    pub signal_quality: String,
    pub trade_confidence: f64,

    // RTDS
    pub rtds_binance_price: f64,
    pub rtds_chainlink_price: f64,
    pub rtds_binance_dev_bps: f64,
    pub rtds_chainlink_dev_bps: f64,
    pub rtds_price_advantage_bps: f64,
    pub rtds_chainlink_binance_spread_bps: f64,
    pub rtds_binance_lag_ms: i64,
    pub rtds_chainlink_lag_ms: i64,
    pub rtds_binance_stale_ms: i64,
    pub rtds_chainlink_stale_ms: i64,

    // ── FIXED LATENCY FIELDS ── stored as raw epoch-ms, displayed as (now_ms - x)
    pub binance_update_ms: i64,     // epoch ms
    pub poly_yes_update_ms: i64,    // epoch ms
    pub poly_no_update_ms: i64,     // epoch ms
    pub latency_binance_poly_ms: i64,

    // derived
    pub obi: f64,
    pub cvd_1m: f64, pub cvd_3m: f64, pub cvd_5m: f64,
    pub signal_score: i32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Rolling history
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct RollingHistory {
    pub price_ticks:   VecDeque<f64>,
    pub vol_ticks:     VecDeque<u64>,
    pub price_1m_ago:  f64,
    pub price_24h_ago: f64,
    pub cvd_buckets_1m: VecDeque<(i64, f64)>,
    pub cvd_buckets_3m: VecDeque<(i64, f64)>,
    pub cvd_buckets_5m: VecDeque<(i64, f64)>,
}

impl RollingHistory {
    fn push_tick(&mut self, price: f64, volume: f64) {
        self.price_ticks.push_back(price);
        if self.price_ticks.len() > 120 { self.price_ticks.pop_front(); }
        if self.price_ticks.len() >= 60 {
            self.price_1m_ago = self.price_ticks[self.price_ticks.len() - 60];
        }
        self.vol_ticks.push_back((volume * 10.0) as u64);
        if self.vol_ticks.len() > 60 { self.vol_ticks.pop_front(); }
    }
    fn push_cvd(&mut self, net_usd: f64, now_sec: i64) {
        let prune = |dq: &mut VecDeque<(i64, f64)>, w: i64| {
            while dq.front().map(|(t, _)| now_sec - t > w).unwrap_or(false) { dq.pop_front(); }
        };
        prune(&mut self.cvd_buckets_1m, 60);
        prune(&mut self.cvd_buckets_3m, 180);
        prune(&mut self.cvd_buckets_5m, 300);
        self.cvd_buckets_1m.push_back((now_sec, net_usd));
        self.cvd_buckets_3m.push_back((now_sec, net_usd));
        self.cvd_buckets_5m.push_back((now_sec, net_usd));
    }
    fn cvd_1m(&self) -> f64 { self.cvd_buckets_1m.iter().map(|(_, v)| v).sum() }
    fn cvd_3m(&self) -> f64 { self.cvd_buckets_3m.iter().map(|(_, v)| v).sum() }
    fn cvd_5m(&self) -> f64 { self.cvd_buckets_5m.iter().map(|(_, v)| v).sum() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal scoring
// ─────────────────────────────────────────────────────────────────────────────

fn compute_signal_score(row: &DashRow) -> i32 {
    let mut s: i32 = 0;
    if row.obi < -0.3 { s -= 2; } else if row.obi > 0.3 { s += 2; }
    if row.cvd_5m < -50_000.0 { s -= 2; } else if row.cvd_5m > 50_000.0 { s += 2; }
    let macd = row.vol_13sec - row.vol_34sec;
    if macd < -0.0001 { s -= 1; } else if macd > 0.0001 { s += 1; }
    if row.binance_price < row.strike_price { s -= 1; } else { s += 1; }
    if row.binance_price < row.binance_price_prev { s -= 1; } else if row.binance_price > row.binance_price_prev { s += 1; }
    s.clamp(-7, 7)
}

fn obi_from_book(ybs: f64, ybsz: f64, yas: f64, yasz: f64, nbs: f64, nbsz: f64, nas: f64, nasz: f64) -> f64 {
    let total_bid = ybs * ybsz + nbs * nbsz;
    let total_ask = yas * yasz + nas * nasz;
    let total = total_bid + total_ask;
    if total == 0.0 { 0.0 } else { (total_bid - total_ask) / total }
}

// ─────────────────────────────────────────────────────────────────────────────
// Colour palette
// ─────────────────────────────────────────────────────────────────────────────

const C_GREEN:   Color = Color::Rgb(0, 230, 115);
const C_RED:     Color = Color::Rgb(255, 75, 75);
const C_YELLOW:  Color = Color::Rgb(255, 210, 40);
const C_MAGENTA: Color = Color::Rgb(210, 70, 210);
const C_CYAN:    Color = Color::Rgb(60, 210, 255);
const C_GRAY:    Color = Color::Rgb(130, 130, 145);
const C_WHITE:   Color = Color::Rgb(225, 225, 230);
const C_DIM:     Color = Color::Rgb(60, 60, 70);
const C_ORANGE:  Color = Color::Rgb(255, 155, 35);
const C_BG:      Color = Color::Rgb(10, 10, 16);
const C_BG2:     Color = Color::Rgb(16, 16, 24);
const C_BORDER:  Color = Color::Rgb(45, 45, 62);
const C_TEAL:    Color = Color::Rgb(0, 200, 160);
const C_GOLD:    Color = Color::Rgb(255, 195, 0);

fn price_color(now: f64, prev: f64) -> Color {
    if now > prev { C_GREEN } else if now < prev { C_RED } else { C_WHITE }
}
fn signed_color(v: f64) -> Color {
    if v > 0.0 { C_GREEN } else if v < 0.0 { C_RED } else { C_GRAY }
}
fn signal_color(score: i32) -> Color {
    if score <= -4 { C_RED } else if score <= -1 { C_ORANGE }
    else if score == 0 { C_GRAY } else if score <= 3 { C_CYAN } else { C_GREEN }
}
fn latency_color(ms: i64) -> Color {
    if ms < 200 { C_GREEN } else if ms < 800 { C_YELLOW } else if ms < 2000 { C_ORANGE } else { C_RED }
}
fn staleness_color(ms: i64) -> Color {
    if ms < 500 { C_GREEN } else if ms < 1500 { C_YELLOW } else if ms < 3000 { C_ORANGE } else { C_RED }
}

fn block_panel<'a>(title: &'a str, accent: Color) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(title, Style::default().fg(accent).add_modifier(Modifier::BOLD)),
            Span::styled(" ", Style::default()),
        ]))
        .style(Style::default().bg(C_BG))
}

// Mini inline bar:  ████░░░░  width chars, filled = ratio 0..1
fn mini_bar(ratio: f64, width: usize, fill_c: Color) -> Vec<Span<'static>> {
    let filled = ((ratio.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let empty  = width - filled;
    vec![
        Span::styled("█".repeat(filled), Style::default().fg(fill_c)),
        Span::styled("░".repeat(empty),  Style::default().fg(C_DIM)),
    ]
}

fn fmt_usd(v: f64) -> String {
    if v.abs() >= 1_000_000.0 { format!("{:+.2}M", v / 1_000_000.0) }
    else if v.abs() >= 1_000.0 { format!("{:+.1}K", v / 1_000.0) }
    else { format!("{:+.0}", v) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Panel renderers
// ─────────────────────────────────────────────────────────────────────────────

fn render_header(f: &mut Frame, area: Rect, row: &DashRow, hist: &RollingHistory) {
    let price_c = price_color(row.binance_price, row.binance_price_prev);
    let pct_1m  = if hist.price_1m_ago > 0.0 {
        (row.binance_price - hist.price_1m_ago) / hist.price_1m_ago * 100.0
    } else { 0.0 };
    let pct_24h = if hist.price_24h_ago > 0.0 {
        (row.binance_price - hist.price_24h_ago) / hist.price_24h_ago * 100.0
    } else { 0.0 };

    let ttx_secs = (row.ttx_min * 60.0) as i64;
    let mm = ttx_secs / 60;
    let ss = ttx_secs % 60;
    let ttx_color = if ttx_secs < 90 { C_RED } else if ttx_secs < 240 { C_ORANGE } else { C_GREEN };

    // Outcome deviation
    let dev_usd = row.price_vs_strike_usd;
    let dev_pct = row.price_vs_strike_pct;
    let dev_arrow = if dev_usd >= 0.0 { "▲" } else { "▼" };
    let dev_c = if dev_usd >= 0.0 { C_GREEN } else { C_RED };
    let outcome_label = match row.outcome_projection.as_str() {
        "YES" => "→ YES",
        "NO"  => "→ NO",
        _     => "→ OPEN",
    };
    let outcome_c = match row.outcome_projection.as_str() {
        "YES" => C_GREEN,
        "NO"  => C_RED,
        _     => C_GRAY,
    };

    let vol_regime_c = match row.volatility_regime.as_str() {
        "HIGH"   => C_RED,
        "MEDIUM" => C_ORANGE,
        _        => C_TEAL,
    };

    let now_str = Utc::now().format("%H:%M:%S UTC").to_string();

    let lines = vec![
        Line::from(vec![
            Span::styled("  ₿ BTC ", Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD)),
            Span::styled(format!("${:.2}", row.binance_price),
                Style::default().fg(price_c).add_modifier(Modifier::BOLD)),
            Span::styled("  │  1m ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:+.3}%", pct_1m), Style::default().fg(signed_color(pct_1m))),
            Span::styled("  24h ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:+.3}%", pct_24h), Style::default().fg(signed_color(pct_24h))),
            Span::styled("  │  Vol ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.0}", row.binance_volume), Style::default().fg(C_WHITE)),
            Span::styled("  │  Regime ", Style::default().fg(C_GRAY)),
            Span::styled(&row.volatility_regime, Style::default().fg(vol_regime_c).add_modifier(Modifier::BOLD)),
            Span::styled("  │  Q* ", Style::default().fg(C_GRAY)),
            Span::styled(&row.signal_quality, Style::default().fg(C_CYAN)),
            Span::styled(format!(" Conf {:.2}", row.trade_confidence), Style::default().fg(C_GRAY)),
            Span::styled("  │  ", Style::default().fg(C_DIM)),
            Span::styled(&now_str, Style::default().fg(C_GRAY)),
        ]),
        Line::from(vec![
            Span::styled("  Strike ", Style::default().fg(C_GRAY)),
            Span::styled(format!("${:.2}", row.strike_price), Style::default().fg(C_GOLD)),
            Span::styled("  ", Style::default()),
            Span::styled(dev_arrow, Style::default().fg(dev_c).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" ${:.2}", dev_usd.abs()), Style::default().fg(dev_c).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" ({:+.3}%)", dev_pct), Style::default().fg(dev_c)),
            Span::styled("  ", Style::default()),
            Span::styled(outcome_label, Style::default().fg(outcome_c).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)),
            Span::styled(format!("  conf {:.0}%", row.outcome_confidence * 100.0), Style::default().fg(outcome_c)),
            Span::styled("  │  ", Style::default().fg(C_DIM)),
            Span::styled("⏱ TTX ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:02}:{:02}", mm, ss), Style::default().fg(ttx_color).add_modifier(Modifier::BOLD)),
            Span::styled("  │  ", Style::default().fg(C_DIM)),
            Span::styled(&row.slug, Style::default().fg(C_MAGENTA)),
        ]),
    ];

    let para = Paragraph::new(lines)
        .block(block_panel("BTC · POLYMARKET DASHBOARD", C_CYAN))
        .style(Style::default().bg(C_BG));
    f.render_widget(para, area);

    // TTX progress gauge overlay — rendered in a thin sub-slice of header bottom row
    // (Ratatui can't overlap; we position it as the last row of the header area)
    // Actually render TTX gauge as standalone in the header rect lower strip
}

fn render_ttx_gauge(f: &mut Frame, area: Rect, row: &DashRow) {
    // area is a 1-row stripe at the very bottom of header zone
    let elapsed = row.ttx_total_min - row.ttx_min;
    let ratio = (elapsed / row.ttx_total_min.max(0.001)).clamp(0.0, 1.0);
    let pct = (ratio * 100.0) as u16;
    let ttx_secs = (row.ttx_min * 60.0) as i64;
    let ttx_c = if ttx_secs < 90 { C_RED } else if ttx_secs < 240 { C_ORANGE } else { C_TEAL };
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::NONE).style(Style::default().bg(C_BG)))
        .gauge_style(Style::default().fg(ttx_c).bg(C_BG2))
        .percent(pct)
        .label(Span::styled(
            format!(" Window elapsed {:.0}% ", ratio * 100.0),
            Style::default().fg(ttx_c),
        ));
    f.render_widget(gauge, area);
}

fn render_order_book(f: &mut Frame, area: Rect, row: &DashRow) {
    let obi_pct = row.obi * 100.0;
    let obi_c   = if row.obi < -0.2 { C_RED } else if row.obi > 0.2 { C_GREEN } else { C_GRAY };
    let obi_lbl = if row.obi < -0.15 { "BEARISH" } else if row.obi > 0.15 { "BULLISH" } else { "NEUTRAL" };

    // OBI bar — 20 chars wide
    let bar_ratio = (row.obi + 1.0) / 2.0; // map -1..1 → 0..1
    let bar_filled = ((bar_ratio * 20.0).round() as usize).clamp(0, 20);
    let bar_empty  = 20 - bar_filled;
    let obi_bar: Vec<Span> = vec![
        Span::styled("[", Style::default().fg(C_DIM)),
        Span::styled("█".repeat(bar_filled), Style::default().fg(obi_c)),
        Span::styled("░".repeat(bar_empty),  Style::default().fg(C_DIM)),
        Span::styled("]", Style::default().fg(C_DIM)),
    ];

    let mut lines = vec![
        Line::from(vec![
            Span::styled(" OBI ", Style::default().fg(C_MAGENTA).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:+.1}%", obi_pct), Style::default().fg(obi_c).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", obi_lbl), Style::default().fg(obi_c)),
        ]),
        Line::from(obi_bar),
        Line::from(""),
        Line::from(vec![Span::styled(" ── YES ──────────────────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  BID ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.4}", row.yes_bid), Style::default().fg(C_GREEN)),
            Span::styled(format!(" ×{:.0}", row.yes_bid_size), Style::default().fg(C_DIM)),
            Span::styled("   ASK ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.4}", row.yes_ask), Style::default().fg(C_RED)),
            Span::styled(format!(" ×{:.0}", row.yes_ask_size), Style::default().fg(C_DIM)),
        ]),
        Line::from(vec![
            Span::styled("  Spr ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.1}bps", row.yes_spread_bps), Style::default().fg(C_YELLOW)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ── NO ───────────────────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  BID ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.4}", row.no_bid),  Style::default().fg(C_GREEN)),
            Span::styled(format!(" ×{:.0}", row.no_bid_size), Style::default().fg(C_DIM)),
            Span::styled("   ASK ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.4}", row.no_ask),  Style::default().fg(C_RED)),
            Span::styled(format!(" ×{:.0}", row.no_ask_size), Style::default().fg(C_DIM)),
        ]),
        Line::from(vec![
            Span::styled("  Spr ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.1}bps", row.no_spread_bps), Style::default().fg(C_YELLOW)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ── BINANCE ──────────────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  BID ", Style::default().fg(C_GRAY)),
            Span::styled(format!("${:.2}", row.binance_bid), Style::default().fg(C_GREEN)),
            Span::styled("   ASK ", Style::default().fg(C_GRAY)),
            Span::styled(format!("${:.2}", row.binance_ask), Style::default().fg(C_RED)),
        ]),
        Line::from(vec![
            Span::styled("  Spr ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.2}bps", row.binance_spread_bps), Style::default().fg(C_YELLOW)),
        ]),
    ];

    let para = Paragraph::new(lines)
        .block(block_panel("ORDER BOOK", C_BORDER))
        .style(Style::default().bg(C_BG));
    f.render_widget(para, area);
}

fn render_flow_volume(f: &mut Frame, area: Rect, row: &DashRow, hist: &RollingHistory) {
    let cvd_1m = hist.cvd_1m();
    let cvd_3m = hist.cvd_3m();
    let cvd_5m = hist.cvd_5m();

    // CVD inline colour bars (normalised to cvd_5m as reference scale)
    let scale = cvd_5m.abs().max(1.0);
    let bar1 = (cvd_1m / scale).abs().min(1.0);
    let bar3 = (cvd_3m / scale).abs().min(1.0);

    let spike_c = if row.volume_spike { C_RED } else { C_GRAY };
    let spike_t = if row.volume_spike { " 🔥 SPIKE" } else { "" };

    let vol_regime_c = match row.volatility_regime.as_str() {
        "HIGH"   => C_RED,
        "MEDIUM" => C_ORANGE,
        _        => C_TEAL,
    };

    let lines = vec![
        Line::from(vec![Span::styled(" ── CVD ──────────────────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  1m ", Style::default().fg(C_GRAY)),
            Span::styled(fmt_usd(cvd_1m), Style::default().fg(signed_color(cvd_1m)).add_modifier(Modifier::BOLD)),
            Span::styled("   3m ", Style::default().fg(C_GRAY)),
            Span::styled(fmt_usd(cvd_3m), Style::default().fg(signed_color(cvd_3m))),
            Span::styled("   5m ", Style::default().fg(C_GRAY)),
            Span::styled(fmt_usd(cvd_5m), Style::default().fg(signed_color(cvd_5m))),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ── VOLUME ───────────────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  1s ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.0}", row.volume_1sec), Style::default().fg(C_WHITE)),
            Span::styled("   1m ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.0}", row.binance_volume), Style::default().fg(C_WHITE)),
            Span::styled(spike_t, Style::default().fg(spike_c).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ── VOLATILITY ───────────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  1s ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.4}", row.vol_1sec),  Style::default().fg(C_CYAN)),
            Span::styled("   5s ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.4}", row.vol_5sec),  Style::default().fg(C_CYAN)),
        ]),
        Line::from(vec![
            Span::styled("  13s ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.4}", row.vol_13sec), Style::default().fg(C_CYAN)),
            Span::styled("  34s ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.4}", row.vol_34sec), Style::default().fg(C_CYAN)),
            Span::styled("  89s ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.4}", row.vol_89sec), Style::default().fg(C_CYAN)),
        ]),
        Line::from(vec![
            Span::styled("  RealVol ", Style::default().fg(C_GRAY)),
            Span::styled(
                format!("{:.4}%", row.realized_vol * 100.0),
                Style::default().fg(C_GOLD).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Regime ", Style::default().fg(C_GRAY)),
            Span::styled(&row.volatility_regime, Style::default().fg(vol_regime_c).add_modifier(Modifier::BOLD)),
        ]),
    ];

    let para = Paragraph::new(lines)
        .block(block_panel("FLOW · CVD · VOLUME", C_BORDER))
        .style(Style::default().bg(C_BG));
    f.render_widget(para, area);
}

fn render_technicals(f: &mut Frame, area: Rect, row: &DashRow) {
    let rsi_proxy = (row.realized_vol * 100.0).clamp(0.0, 100.0) * 0.7 + 30.0;
    let rsi_c = if rsi_proxy < 35.0 { C_RED } else if rsi_proxy > 65.0 { C_GREEN } else { C_YELLOW };
    let macd_hist = row.vol_13sec - row.vol_34sec;
    let ema_c = if row.vol_1sec < row.vol_5sec { C_RED } else { C_GREEN };
    let ema_lbl = if row.vol_1sec < row.vol_5sec { "DEATH ✗" } else { "GOLDEN ✓" };
    let vwap_c = if row.binance_price < row.strike_price { C_RED } else { C_GREEN };
    let vwap_lbl = if row.binance_price < row.strike_price { "BELOW" } else { "ABOVE" };

    let lines = vec![
        Line::from(vec![Span::styled(" ── MOMENTUM ─────────────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  RSI(14) ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.1}", rsi_proxy), Style::default().fg(rsi_c).add_modifier(Modifier::BOLD)),
            Span::styled("   MACD hist ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:+.6}", macd_hist), Style::default().fg(signed_color(macd_hist))),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ── TREND ────────────────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  EMA cross ", Style::default().fg(C_GRAY)),
            Span::styled(ema_lbl, Style::default().fg(ema_c).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  VWAP proxy ", Style::default().fg(C_GRAY)),
            Span::styled(format!("${:.2}", row.strike_price), Style::default().fg(C_GOLD)),
            Span::styled(format!("  Price {}", vwap_lbl), Style::default().fg(vwap_c)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ── RTDS ─────────────────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  Bnce dev ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.1}bps", row.rtds_binance_dev_bps), Style::default().fg(signed_color(row.rtds_binance_dev_bps))),
            Span::styled("   CL dev ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.1}bps", row.rtds_chainlink_dev_bps), Style::default().fg(signed_color(row.rtds_chainlink_dev_bps))),
        ]),
        Line::from(vec![
            Span::styled("  Adv ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.1}bps", row.rtds_price_advantage_bps), Style::default().fg(signed_color(row.rtds_price_advantage_bps))),
            Span::styled("   CL-B ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.1}bps", row.rtds_chainlink_binance_spread_bps), Style::default().fg(C_CYAN)),
        ]),
        Line::from(vec![
            Span::styled("  B-stale ", Style::default().fg(C_GRAY)),
            Span::styled(
                format!("{}ms", row.rtds_binance_stale_ms),
                Style::default().fg(staleness_color(row.rtds_binance_stale_ms)),
            ),
            Span::styled("   CL-stale ", Style::default().fg(C_GRAY)),
            Span::styled(
                format!("{}ms", row.rtds_chainlink_stale_ms),
                Style::default().fg(staleness_color(row.rtds_chainlink_stale_ms)),
            ),
        ]),
    ];

    let para = Paragraph::new(lines)
        .block(block_panel("TECHNICALS · RTDS", C_BORDER))
        .style(Style::default().bg(C_BG));
    f.render_widget(para, area);
}

fn render_greeks_latency(f: &mut Frame, area: Rect, row: &DashRow) {
    // ── FIXED LATENCY ──
    // binance_update_ms, poly_*_update_ms are epoch-ms
    // We show (now_ms - field) = how many ms ago the update arrived
    let b_age_ms  = (row.now_ms - row.binance_update_ms).max(0);
    let py_age_ms = (row.now_ms - row.poly_yes_update_ms).max(0);
    let pn_age_ms = (row.now_ms - row.poly_no_update_ms).max(0);

    let lines = vec![
        Line::from(vec![Span::styled(" ── OPTIONS GREEKS ───────────", Style::default().fg(C_DIM))]),
        Line::from(vec![
            Span::styled("  Δ delta  ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.8}", row.delta_yes),
                Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  Γ gamma  ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.8}", row.gamma), Style::default().fg(C_WHITE)),
        ]),
        Line::from(vec![
            Span::styled("  ν vega   ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.8}", row.vega), Style::default().fg(C_WHITE)),
        ]),
        Line::from(vec![
            Span::styled("  Θ theta  ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:.8}", row.theta),
                Style::default().fg(signed_color(row.theta))),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" ── FEED LATENCY ─────────────", Style::default().fg(C_DIM))]),
        // FIXED: show relative age (now_ms - update_ms), not raw epoch fragments
        Line::from(vec![
            Span::styled("  Binance  ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:>6}ms", b_age_ms), Style::default().fg(latency_color(b_age_ms))),
            Span::styled(" ago", Style::default().fg(C_DIM)),
        ]),
        Line::from(vec![
            Span::styled("  Poly YES ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:>6}ms", py_age_ms), Style::default().fg(latency_color(py_age_ms))),
            Span::styled(" ago", Style::default().fg(C_DIM)),
        ]),
        Line::from(vec![
            Span::styled("  Poly NO  ", Style::default().fg(C_GRAY)),
            Span::styled(format!("{:>6}ms", pn_age_ms), Style::default().fg(latency_color(pn_age_ms))),
            Span::styled(" ago", Style::default().fg(C_DIM)),
        ]),
        Line::from(vec![
            Span::styled("  B↔P Δ   ", Style::default().fg(C_GRAY)),
            Span::styled(
                format!("{:>+6}ms", row.latency_binance_poly_ms),
                Style::default().fg(latency_color(row.latency_binance_poly_ms.abs())),
            ),
        ]),
        Line::from(vec![
            Span::styled("  RTDS-B  ", Style::default().fg(C_GRAY)),
            Span::styled(
                format!("{:>6}ms", row.rtds_binance_lag_ms),
                Style::default().fg(latency_color(row.rtds_binance_lag_ms)),
            ),
            Span::styled("  RTDS-CL ", Style::default().fg(C_GRAY)),
            Span::styled(
                format!("{:>6}ms", row.rtds_chainlink_lag_ms),
                Style::default().fg(latency_color(row.rtds_chainlink_lag_ms)),
            ),
        ]),
    ];

    let para = Paragraph::new(lines)
        .block(block_panel("GREEKS · LATENCY", C_BORDER))
        .style(Style::default().bg(C_BG));
    f.render_widget(para, area);
}

fn render_signal_bar(f: &mut Frame, area: Rect, row: &DashRow, hist: &RollingHistory) {
    let score     = row.signal_score;
    let score_lbl = if score <= -4 { "STRONG BEARISH" }
        else if score <= -2 { "BEARISH" }
        else if score == -1 { "LEAN BEARISH" }
        else if score ==  0 { "NEUTRAL" }
        else if score ==  1 { "LEAN BULLISH" }
        else if score <=  3 { "BULLISH" }
        else                { "STRONG BULLISH" };
    let score_c = signal_color(score);

    // Gauge: -7..+7 → 0..100%
    let gauge_pct = ((score + 7) as f64 / 14.0 * 100.0) as u16;
    let cvd_1m    = hist.cvd_1m();
    let cvd_5m    = hist.cvd_5m();
    let macd_hist = row.vol_13sec - row.vol_34sec;

    // Split area: gauge on left half, signal row text on right half
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    let gauge = Gauge::default()
        .block(block_panel("SIGNAL", C_BORDER))
        .gauge_style(Style::default().fg(score_c).bg(C_BG2))
        .percent(gauge_pct)
        .label(Span::styled(
            format!(" {} ({:+}/7) ", score_lbl, score),
            Style::default().fg(score_c).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(gauge, h[0]);

    let outcome_c = match row.outcome_projection.as_str() {
        "YES" => C_GREEN, "NO" => C_RED, _ => C_GRAY,
    };

    let line = Line::from(vec![
        Span::styled("  OBI ", Style::default().fg(C_GRAY)),
        Span::styled(format!("{:+.1}%", row.obi * 100.0), Style::default().fg(signed_color(row.obi))),
        Span::styled("  │  CVD 1m ", Style::default().fg(C_GRAY)),
        Span::styled(fmt_usd(cvd_1m), Style::default().fg(signed_color(cvd_1m))),
        Span::styled("  │  CVD 5m ", Style::default().fg(C_GRAY)),
        Span::styled(fmt_usd(cvd_5m), Style::default().fg(signed_color(cvd_5m))),
        Span::styled("  │  MACD ", Style::default().fg(C_GRAY)),
        Span::styled(format!("{:+.6}", macd_hist), Style::default().fg(signed_color(macd_hist))),
        Span::styled("  │  ", Style::default().fg(C_DIM)),
        Span::styled(&row.outcome_projection, Style::default().fg(outcome_c).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" yes {:.4}", row.yes_bid), Style::default().fg(C_GRAY)),
        Span::styled(format!(" / no {:.4}", row.no_bid), Style::default().fg(C_GRAY)),
        Span::styled(
            format!("  Δ${:.2} ({:+.3}%)", row.price_vs_strike_usd.abs(), row.price_vs_strike_pct),
            Style::default().fg(signed_color(row.price_vs_strike_usd)).add_modifier(Modifier::BOLD),
        ),
    ]);

    let para = Paragraph::new(vec![Line::from(""), line])
        .block(block_panel("SIGNALS · OUTCOME", C_BORDER))
        .style(Style::default().bg(C_BG));
    f.render_widget(para, h[1]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Root frame renderer
// ─────────────────────────────────────────────────────────────────────────────

fn render_frame(f: &mut Frame, row: &DashRow, hist: &RollingHistory) {
    let area = f.area();

    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),  // header (2 lines of text + block borders + 1 gauge)
            Constraint::Min(16),    // body
            Constraint::Length(5),  // signal bar
        ])
        .split(area);

    // Header: split last row off for TTX gauge
    let header_v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Length(1)])
        .split(v[0]);
    render_header(f, header_v[0], row, hist);
    render_ttx_gauge(f, header_v[1], row);

    // Body: left 46% | right 54%
    let body_h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(v[1]);

    let left_v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(body_h[0]);
    let right_v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(body_h[1]);

    render_order_book(f, left_v[0], row);
    render_technicals(f, left_v[1], row);
    render_flow_volume(f, right_v[0], row, hist);
    render_greeks_latency(f, right_v[1], row);

    render_signal_bar(f, v[2], row, hist);
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
    let duration_secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(86400);

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(3))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?;

    let mut current_market = fetch_current_market(&http).await?;
    let mut binance = BinanceDataManager::new();
    binance.connect(&["BTCUSDT"], None).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = binance.bootstrap_volatility("BTCUSDT", 70).await;

    let strike = fetch_strike_with_retry(&http, &current_market).await?;
    current_market.strike_price = Some(strike);
    current_market.strike_fetched = true;

    let poly = Arc::new(PolymarketOrderbookStream::new());
    let mut rx = poly.clone().stream(vec![
        current_market.yes_token_id.clone(),
        current_market.no_token_id.clone(),
    ]).await?;
    tokio::spawn(async move { while let Some(_) = rx.recv().await {} });
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut rtds = RtdsTracker::new();
    rtds.start();
    tokio::time::sleep(Duration::from_secs(2)).await;

    // TUI
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut hist = RollingHistory::default();
    let mut prev_price: f64 = 0.0;
    let mut temp_csv = TempCsvWriter::new();
    let end_time = SystemTime::now() + Duration::from_secs(duration_secs);
    let mut consecutive_failures: u32 = 0;
    let mut last_fetch_ms: i64 = 0;

    loop {
        if SystemTime::now() >= end_time { break; }
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(k) = event::read()? {
                if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) { break; }
            }
        }

        let now = Utc::now();
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        let now_sec = now.timestamp();

        let snapshot = match binance.get_market_snapshot("BTCUSDT").await {
            Some(s) => s,
            None => { tokio::time::sleep(Duration::from_millis(16)).await; continue; }
        };

        let yes_p = poly.get_prices(&current_market.yes_token_id);
        let no_p  = poly.get_prices(&current_market.no_token_id);
        let (b_bid, b_ask) = binance.get_bid_ask("BTCUSDT").await
            .unwrap_or((snapshot.spot_price, snapshot.spot_price));

        let ttx_total = 15.0_f64; // minutes per window
        let ttx       = ((current_market.window_end - now_sec) as f64 / 60.0).max(0.0);
        let resolved  = ttx <= 0.0;
        let strike    = current_market.strike_price.unwrap_or(snapshot.spot_price);
        let greeks    = calculate_greeks(snapshot.spot_price, strike, ttx, snapshot.realized_volatility);

        let yes_bid  = yes_p.as_ref().and_then(|p| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
        let yes_ask  = yes_p.as_ref().and_then(|p| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
        let no_bid   = no_p.as_ref().and_then(|p| p.best_bid.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
        let no_ask   = no_p.as_ref().and_then(|p| p.best_ask.map(|d| d.to_f64().unwrap_or(0.0))).unwrap_or(0.0);
        let yes_bs   = yes_p.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0);
        let yes_as   = yes_p.as_ref().map(|p| p.best_ask_size).unwrap_or(0.0);
        let no_bs    = no_p.as_ref().map(|p| p.best_bid_size).unwrap_or(0.0);
        let no_as    = no_p.as_ref().map(|p| p.best_ask_size).unwrap_or(0.0);

        // Raw epoch-ms (stored as-is; display will subtract now_ms)
        let poly_yes_ms = yes_p.as_ref().map(|p| p.last_update_ms).unwrap_or(now_ms);
        let poly_no_ms  = no_p.as_ref().map(|p| p.last_update_ms).unwrap_or(now_ms);
        let rtds_snap   = rtds.get_snapshot(snapshot.spot_price, snapshot.timestamp);

        let sign = if snapshot.spot_price > prev_price { 1.0 } else if snapshot.spot_price < prev_price { -1.0 } else { 0.0 };
        hist.push_cvd(snapshot.volume_1sec * snapshot.spot_price * sign, now_sec);
        hist.push_tick(snapshot.spot_price, snapshot.volume_1sec);

        let obi = obi_from_book(yes_bid, yes_bs, yes_ask, yes_as, no_bid, no_bs, no_ask, no_as);
        let outcome_str = if resolved {
            if snapshot.spot_price >= strike { "YES" } else { "NO" }
        } else { "OPEN" };

        // ── Outcome deviation ──
        let dev_usd = snapshot.spot_price - strike;
        let dev_pct = dev_usd / strike * 100.0;
        // outcome_confidence: how "certain" the market is, based on delta_yes
        // delta = 1 means clearly ITM (YES), delta = 0 means clearly OTM (NO)
        let outcome_confidence = if outcome_str == "YES" { greeks.delta }
            else if outcome_str == "NO" { 1.0 - greeks.delta }
            else { (greeks.delta - 0.5).abs() * 2.0 };

        let mut row = DashRow {
            timestamp: now.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
            slug:      current_market.slug.clone(),
            ttx_min:   ttx, ttx_total_min: ttx_total,
            time_to_open: current_market.window_start - now_sec,
            market_resolved: resolved,
            outcome:   outcome_str.to_string(),
            window_start: current_market.window_start,
            window_end:   current_market.window_end,
            now_ms,   // ← stored for latency display

            binance_price:      snapshot.spot_price,
            binance_price_prev: prev_price,
            binance_bid:  b_bid, binance_ask: b_ask,
            binance_spread_bps: snapshot.spread_bps,
            binance_volume:     snapshot.volume_1min,
            volume_1sec:        snapshot.volume_1sec,
            volume_spike:       snapshot.volume_spike,

            yes_bid, yes_bid_size: yes_bs, yes_ask, yes_ask_size: yes_as,
            no_bid,  no_bid_size:  no_bs,  no_ask,  no_ask_size:  no_as,
            yes_spread_bps: calculate_spread_bps(yes_bid, yes_ask),
            no_spread_bps:  calculate_spread_bps(no_bid,  no_ask),

            strike_price:  strike,
            realized_vol:  snapshot.realized_volatility,
            delta_yes:     greeks.delta,
            gamma:         greeks.gamma,
            vega:          greeks.vega,
            theta:         greeks.theta,

            price_vs_strike_usd: dev_usd,
            price_vs_strike_pct: dev_pct,
            outcome_projection:  outcome_str.to_string(),
            outcome_confidence,

            vol_1sec:  snapshot.vol_1sec,  vol_5sec:  snapshot.vol_5sec,
            vol_13sec: snapshot.vol_13sec, vol_34sec: snapshot.vol_34sec,
            vol_89sec: snapshot.vol_89sec,
            volatility_regime: snapshot.volatility_regime.clone(),
            signal_quality:    snapshot.signal_quality.clone(),
            trade_confidence:  snapshot.trade_confidence,

            rtds_binance_price:               rtds_snap.rtds_binance_price,
            rtds_chainlink_price:             rtds_snap.rtds_chainlink_price,
            rtds_binance_dev_bps:             rtds_snap.rtds_binance_dev_bps,
            rtds_chainlink_dev_bps:           rtds_snap.rtds_chainlink_dev_bps,
            rtds_price_advantage_bps:         rtds_snap.rtds_price_advantage_bps,
            rtds_chainlink_binance_spread_bps:rtds_snap.rtds_chainlink_binance_spread_bps,
            rtds_binance_lag_ms:              rtds_snap.rtds_binance_lag_ms,
            rtds_chainlink_lag_ms:            rtds_snap.rtds_chainlink_lag_ms,
            rtds_binance_stale_ms:            rtds_snap.rtds_binance_stale_ms,
            rtds_chainlink_stale_ms:          rtds_snap.rtds_chainlink_stale_ms,

            // raw epoch-ms — NOT pre-subtracted here; subtracted in render_greeks_latency
            binance_update_ms:       snapshot.timestamp,
            poly_yes_update_ms:      poly_yes_ms,
            poly_no_update_ms:       poly_no_ms,
            latency_binance_poly_ms: snapshot.timestamp - (poly_yes_ms + poly_no_ms) / 2,

            obi,
            cvd_1m: hist.cvd_1m(), cvd_3m: hist.cvd_3m(), cvd_5m: hist.cvd_5m(),
            signal_score: 0,
        };
        row.signal_score = compute_signal_score(&row);

        let _ = temp_csv.ensure_slug(&current_market.slug, &now);
        let _ = temp_csv.write_row(&row);

        terminal.draw(|f| render_frame(f, &row, &hist))?;
        prev_price = snapshot.spot_price;

        // Market rotation
        if resolved {
            let since_last = now_ms.saturating_sub(last_fetch_ms);
            if consecutive_failures == 0 || since_last >= 5_000 {
                last_fetch_ms = now_ms;
                match fetch_next_market_after(&http, current_market.window_start).await {
                    Ok(mut next) => {
                        consecutive_failures = 0;
                        let s = fetch_strike_with_retry(&http, &next).await?;
                        next.strike_price = Some(s);
                        next.strike_fetched = true;
                        let mut new_rx = poly.clone().stream(vec![
                            next.yes_token_id.clone(), next.no_token_id.clone()
                        ]).await?;
                        tokio::spawn(async move { while let Some(_) = new_rx.recv().await {} });
                        current_market = next;
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Err(_) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= 10 { break; }
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
