# Dashboard Design: Tasks #5, #6, and #4 Validation
## Author: strategy-analyst | Based on backtest-zv60.py analysis

---

## TASK #5: Trade Timeline with Exploit Labels

### 5.1 Trade Event Data Structure

The backtest's `log_trade()` captures 15 fields per trade. For the dashboard, we
need to extend this with exploit attribution tags derived from the `ttype` field.

```rust
/// Single trade event in the timeline
#[derive(Debug, Clone)]
pub struct TradeEvent {
    // Core (from backtest log_trade)
    pub tick:           u32,
    pub timestamp_ms:   i64,
    pub book:           Book,        // DN | DIR
    pub side:           Side,        // YES | NO
    pub shares:         u32,
    pub price:          f64,
    pub cost:           f64,
    pub z_score:        f64,
    pub abs_z:          f64,

    // Exploit attribution (parsed from ttype string)
    pub trade_class:    TradeClass,
    pub exploit_tags:   Vec<ExploitTag>,

    // Context at execution time
    pub rtds_adv_bps:   f64,         // RTDS advantage when trade fired
    pub regime:         String,      // Market regime at execution
    pub time_pct:       f64,         // Window elapsed %
    pub confidence:     f64,         // DIR confidence score (0.0 for DN)

    // Position state after trade
    pub dn_yes:         u32,
    pub dn_no:          u32,
    pub dn_combined:    Option<f64>,
    pub dn_ew:          bool,
    pub dr_yes:         u32,
    pub dr_no:          u32,
    pub total_exposure:  f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Book { DN, DIR }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Side { YES, NO }

/// Primary classification of the trade (mutually exclusive)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TradeClass {
    // DN classes
    DnSeed,             // Initial seeding (both sides)
    DnCheap,            // Cheap accumulation
    DnLatency,          // Latency-boosted buy
    DnHedge,            // Balance hedge
    DnEmergencyPair,    // Aggressive pair completion before abandon
    DnFlipHedge,        // Z-flip triggered hedge

    // DIR classes
    DirZScore,          // Standard z-score directional
    DirEmergencyLean,   // Emergency: z agrees with abandoned DN side
    DirEmergencyHedge,  // Emergency: z disagrees, hedge opposite
    DirFlipHedge,       // Post-commit z-flip hedge
    DirChainlinkArb,    // Chainlink resolution arb (Exploit 4)
}

/// Exploit tags (additive — a single trade can have multiple)
/// These track WHICH v6.0 exploits influenced the trade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExploitTag {
    SnipeFiltered,      // Exploit 1: passed theoretical price snipe check
    EdgeScaled,         // Exploit 2: trade size was edge-scaled
    StaleGated,         // Exploit 3: trade passed stale-only gate
    ChainlinkArb,       // Exploit 4: Chainlink resolution arb trade
}
```

### 5.2 Parsing ttype → TradeClass + ExploitTags

The backtest `ttype` strings map as follows:

| ttype string pattern       | TradeClass          | Notes                                |
|---------------------------|---------------------|--------------------------------------|
| `"SEED"`                  | DnSeed              | DN initial seeding                   |
| `"CHEAP"`                 | DnCheap             | (inferred from counter in backtest)  |
| `"LATENCY"`               | DnLatency           | Latency-boosted                      |
| `"HEDGE"`                 | DnHedge             | Balance hedge                        |
| `"EMERGENCY_PAIR"`        | DnEmergencyPair     | DN aggressive pair completion        |
| `"FLIP_HEDGE"` (book=DN)  | DnFlipHedge         | Z-flip hedge in DN book              |
| `"Z=X.XX"`               | DirZScore           | Standard directional                 |
| `"EMER_LEAN Z=X.XX"`     | DirEmergencyLean    | Emergency: lean into abandoned side  |
| `"EMER_HEDGE Z=X.XX"`    | DirEmergencyHedge   | Emergency: hedge opposite            |
| `"FLIP_HEDGE Z=X.XX"` (book=DIR) | DirFlipHedge | Post-commit flip hedge           |
| `"CL_ARB dev=X.Xbps"`    | DirChainlinkArb     | Chainlink arb (always Exploit 4)     |

**Exploit tags are additive:** A standard `DirZScore` trade that also passed the snipe
filter AND was edge-scaled gets tags `[SnipeFiltered, EdgeScaled]`. The stale gate is
a prerequisite for ALL non-exempt trades when enabled, so any trade that executes when
exploit3 is active implicitly passed it (`StaleGated` tag).

**Important:** Exploits 1-3 are filters/modifiers on existing trade types, NOT separate
trade types. Only Exploit 4 (Chainlink Arb) is a distinct trade class. This means:
- Exploit 1 (Snipe): tag on any DN/DIR trade that passed `true_price - margin` check
- Exploit 2 (Edge-Scale): tag on any trade whose size was scaled by RTDS advantage
- Exploit 3 (Stale Gate): tag on any trade that passed the staleness check
- Exploit 4 (Chainlink): its own TradeClass (DirChainlinkArb)

### 5.3 Timeline UI Component Design

```
┌─ TRADE TIMELINE ──────────────────────────────────────────────────────────┐
│ Filter: [ALL] [DN] [DIR] │ Exploit: [ALL] [SNIPE] [EDGE] [STALE] [CL]  │
│ Market: btc-above-105000-feb20  │  Showing 47 trades                     │
├──────────────────────────────────────────────────────────────────────────-┤
│ T    │ Book │ Class        │ Side │ Shares│ Price │ Cost  │ |z|  │ Tags  │
│──────┼──────┼──────────────┼──────┼───────┼───────┼───────┼──────┼───────│
│ 0:32 │ DN   │ SEED         │ YES  │   40  │ 0.38  │$15.74 │ 0.8  │ S E   │
│ 0:45 │ DN   │ SEED         │ NO   │   40  │ 0.41  │$16.97 │ 0.6  │ S     │
│ 2:15 │ DN   │ CHEAP        │ YES  │   35  │ 0.32  │$11.54 │ 1.2  │ S E G │
│ 3:40 │ DIR  │ Z=+2.31      │ YES  │   30  │ 0.28  │$ 8.63 │ 2.3  │ S E G │
│ 5:10 │ DN   │ FLIP_HEDGE   │ NO   │   40  │ 0.35  │$14.32 │ 1.8  │ G     │
│ 8:22 │ DIR  │ FLIP_HEDGE   │ NO   │    8  │ 0.42  │$ 3.45 │ 0.9  │ G     │
│11:45 │ DIR  │ CL_ARB 7.2bp │ YES  │   60  │ 0.22  │$13.49 │ 3.1  │ C E G │
│──────┴──────┴──────────────┴──────┴───────┴───────┴───────┴──────┴───────│
│ ► Click to expand: RTDS adv, regime, confidence, position state after    │
└──────────────────────────────────────────────────────────────────────────-┘

Tag legend: S=Snipe  E=EdgeScaled  G=StaleGated  C=ChainlinkArb
```

**Color scheme by TradeClass:**
| Class              | Color              | Rationale                         |
|-------------------|--------------------|-----------------------------------|
| DnSeed            | Dim cyan (#60D0FF) | Foundational, low-risk            |
| DnCheap           | Cyan (#40C8E0)     | Standard DN accumulation          |
| DnLatency         | Teal (#00C39B)     | Latency-opportunistic             |
| DnHedge           | Blue (#5090FF)     | Defensive/balancing               |
| DnEmergencyPair   | Yellow (#FFD228)   | Urgent action                     |
| DnFlipHedge       | Orange (#FF9B23)   | Reactive to signal change         |
| DirZScore         | Green (#00E673)    | Confident directional             |
| DirEmergencyLean  | Yellow (#FFD228)   | Urgent rescue                     |
| DirEmergencyHedge | Orange (#FF9B23)   | Defensive rescue                  |
| DirFlipHedge      | Magenta (#D246D2)  | Signal instability response       |
| DirChainlinkArb   | Gold (#FFC300)     | High-conviction exploit           |

**Exploit tag badges (small colored dots/letters):**
| Tag            | Badge | Color         |
|---------------|-------|---------------|
| SnipeFiltered | S     | White on blue |
| EdgeScaled    | E     | White on teal |
| StaleGated    | G     | White on gray |
| ChainlinkArb  | C     | Black on gold |

### 5.4 Expanded Trade Detail (on click/select)

```
┌─ Trade #7: DIR CL_ARB ── 11:45 ─────────────────────────────────────────┐
│ Chainlink Resolution Arb │ dev=7.2bps │ threshold=7.0bps (85%+ window)   │
│                                                                           │
│ Execution:   YES × 60 shares @ $0.22 = $13.49 (incl 0.96% fee)          │
│ True price:  Phi(z=3.1) = $0.9990 → edge = $0.779/share                 │
│ Edge-scaled: base=60 × scale=1.44 (7.2/5.0 ref) → 60 (capped)          │
│                                                                           │
│ Signal State:                                                             │
│   z-score: +3.10 │ direction: YES │ flips: 2                             │
│   RTDS adv: 7.2 bps │ stale: YES │ regime: YES_EXTREME                  │
│   Confidence: 0.80 (4/5 votes) │ time: 78.3% elapsed                    │
│                                                                           │
│ Position After:                                                           │
│   DN: 75Y/80N  combined=$0.923  EW=YES  min_pnl=$6.12                   │
│   DIR: 90Y/8N  cost=$27.14  committed=YES                                │
│   Total exposure: $78.40 / $200.00 (39.2%)                               │
└──────────────────────────────────────────────────────────────────────────-┘
```

### 5.5 Performance Optimization

For 1000+ trades:
- **Virtual scrolling:** Only render visible rows (ratatui `StatefulWidget` with offset)
- **Pre-compute exploit tags** at trade creation time, not at render time
- **Index by book/exploit** for instant filtering (HashMap<ExploitTag, Vec<usize>>)
- **Lazy detail expansion:** Only compute expanded detail for selected row

---

## TASK #6: Risk Monitor Panel

### 6.1 Capital Utilization Section

```
┌─ CAPITAL ─────────────────────────────────────────────────────────────────┐
│                                                                           │
│ TOTAL  ███████████████████░░░░░░░░░░░  $78.40 / $200.00  (39.2%)        │
│                                                                           │
│ DN     ██████████░░░░░░░░░░░░░░░░░░░░  $51.26 / $1000   ( 5.1%)        │
│ DIR    ████░░░░░░░░░░░░░░░░░░░░░░░░░░  $27.14 / $1000   ( 2.7%)        │
│                                                                           │
│ Per-market cap: $200 │ DN book cap: $1000 │ DIR book cap: $1000          │
└──────────────────────────────────────────────────────────────────────────-┘
```

**Color rules for capital gauge:**
- 0-60%: Green (healthy headroom)
- 60-80%: Yellow (moderate)
- 80-95%: Orange (caution)
- 95-100%: Red (near limit)

### 6.2 Position Health Section

```
┌─ POSITION HEALTH ─────────────────────────────────────────────────────────┐
│                                                                           │
│ DN BOOK                          │ DIR BOOK                               │
│ ────────────────────────         │ ────────────────────────               │
│ Markets active:  12              │ Markets active:  8                     │
│ Two-sided:       10/12           │ Committed dir:   6 YES, 2 NO          │
│ EW (guaranteed): 7/12  ●●●●●●●○○○○○  │ Frozen:    1/8  ⚠                │
│ Abandoned:       2/12  ⚠⚠        │ Post-flip ct:  avg 1.2               │
│                                  │                                        │
│ Avg combined:    $0.941          │ Avg cost/mkt:   $34.20                │
│ Avg imbalance:   0.58            │ Avg z at entry: 2.1                   │
│ Worst min_pnl:   -$4.22         │ Z-flip avg:     2.8                   │
│                                  │                                        │
│ ALERTS:                                                                   │
│  ⚠ 2 DN positions abandoned (stuck one-sided, $18.40 at risk)            │
│  ⚠ 1 DIR position frozen (3 post-commit flips on btc-above-104500)       │
│  ● DN imbalance > 0.65 on 3 markets (hedge opportunity)                  │
└──────────────────────────────────────────────────────────────────────────-┘
```

### 6.3 Signal Health Section

```
┌─ SIGNAL HEALTH ───────────────────────────────────────────────────────────┐
│                                                                           │
│ Z-SCORE   ████████████████████  +2.31  (YES)    ← high confidence zone   │
│           ↑ flipped 2× │ stable for 45 ticks                             │
│                                                                           │
│ RTDS ADV  ██████████████░░░░░░  7.2 bps  STALE ✓  (threshold: 3.0)     │
│ CL DEV    ████████████████████  12.1 bps          (threshold: 7.0 @85%) │
│                                                                           │
│ REGIME    [YES_EXTREME]   │ CONFIDENCE  [0.80 — 4/5 votes]              │
│ Votes: ✓avg_z  ✓velocity  ✓asymmetry  ✓opening  ✓not-fading            │
│                                                                           │
│ TTX       ████████████████████░░  3:15 remaining  (78.3% elapsed)        │
│                                                                           │
│ EXPLOIT READINESS:                                                        │
│  Exploit 1 (Snipe):    ACTIVE — edge available                           │
│  Exploit 2 (Scale):    ACTIVE — scale factor 1.44×                       │
│  Exploit 3 (Gate):     OPEN   — book confirmed stale                     │
│  Exploit 4 (CL Arb):  ARMED  — CL dev 12.1bps > threshold 7.0bps       │
└──────────────────────────────────────────────────────────────────────────-┘
```

### 6.4 Alert Thresholds and Logic

```rust
#[derive(Debug, Clone)]
pub struct RiskAlert {
    pub severity:  AlertSeverity,
    pub category:  AlertCategory,
    pub message:   String,
    pub market:    Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Ord, PartialOrd, Eq)]
pub enum AlertSeverity {
    Info,       // Gray   — informational
    Warning,    // Yellow — attention needed
    Critical,   // Red    — immediate action required
}

#[derive(Debug, Clone, Copy)]
pub enum AlertCategory {
    Capital,
    Position,
    Signal,
    DataFeed,
}
```

**Alert trigger conditions:**

| Condition                                    | Severity | Message                                    |
|---------------------------------------------|----------|--------------------------------------------|
| `total_exposure / max_exposure > 0.95`      | Critical | Capital near limit (X% utilized)           |
| `dn_abandoned == true`                       | Warning  | DN position abandoned (stuck one-sided)    |
| `dr_frozen == true`                          | Warning  | DIR position frozen (post-commit flips)    |
| `post_commit_flip_count >= 3`               | Warning  | High post-commit instability               |
| `z_flip_count >= 6`                         | Warning  | Noisy z-score signal (6+ flips)            |
| `dn_imbalance > 0.70`                       | Info     | DN imbalance high — hedge opportunity      |
| `dn_combined_avg > 0.98`                    | Warning  | DN combined near breakeven (>$0.98)        |
| `rtds_adv_bps < 1.0 && time_pct > 0.30`    | Info     | RTDS advantage low — exploits may not fire |
| `confidence < 0.40 && dir_trades > 0`       | Warning  | Low confidence on active DIR position      |
| `dn_skipped_capital > 5`                    | Info     | DN hitting book capital limit frequently   |
| `dir_skipped_capital > 5`                   | Info     | DIR hitting book capital limit frequently  |
| No RTDS data for > 30s                      | Critical | RTDS data feed stale                       |
| No Binance data for > 5s                    | Critical | Binance data feed stale                    |

### 6.5 Rust Integration Pattern

Following the existing dashboard.rs pattern, the risk monitor extends the `Row` struct
and adds a new render function:

```rust
// Added to Row struct:
pub dn_abandoned: bool,
pub dn_abandoned_side: Option<Side>,
pub dr_frozen: bool,
pub dr_committed_dir: Option<Side>,
pub z_flip_count: u32,
pub post_commit_flips: u32,
pub dn_imbalance: f64,
pub dn_combined_avg: Option<f64>,
pub dn_min_pnl: f64,
pub dn_ew: bool,
pub total_exposure: f64,
pub max_exposure: f64,
pub dn_cost: f64,
pub dir_cost: f64,
pub alerts: Vec<RiskAlert>,

// New render function following existing pattern:
fn render_risk_monitor(f: &mut Frame, area: Rect, row: &Row) {
    // Split into 3 vertical sections: capital, position, signal
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),   // capital gauges
            Constraint::Min(10),     // position health
            Constraint::Length(8),   // signal health + alerts
        ])
        .split(area);
    // ... render each section
}
```

---

## TASK #4 VALIDATION: Exploit Scoreboard Attribution

### Validation Findings

After thorough analysis of the backtest code, here are corrections and clarifications
for the exploit scoreboard design:

### 4.1 CORRECT: Exploit 4 (Chainlink Arb) P&L Attribution

Chainlink arb P&L is correctly isolated in the backtest:
- `chainlink_arb_cost` tracks cost independently
- `chainlink_arb_yes_shares` / `chainlink_arb_no_shares` track shares
- `chainlink_arb_profit = payout - cost` computed at resolution
- ROI: `chainlink_arb_profit / chainlink_arb_cost * 100`

**This is the ONLY exploit with clean, isolated P&L.** The scoreboard can show
exact profit for Exploit 4.

### 4.2 IMPORTANT CORRECTION: Exploits 1-3 Cannot Show Isolated P&L

The task #4 description says to show "P&L attribution" for all four exploits.
**This is only directly possible for Exploit 4.**

For Exploits 1-3, the attribution is about *influence* not *isolated profit*:

- **Exploit 1 (Snipe):** Tracks `snipe_edge_sum` (cumulative theoretical edge in
  price units) and `snipe_dn_trades`/`snipe_dir_trades` (trade counts that passed).
  But snipe-filtered trades are regular DN/DIR trades — their profit is mixed into
  `dn_profit`/`dr_profit`. You cannot isolate "profit from sniping" because the
  same trade would have executed (with different sizing) without sniping.

  **What to show instead:** `snipe_edge_sum` as "cumulative edge captured",
  `snipe_dn_blocked + snipe_dir_blocked` as "bad entries prevented",
  `avg_edge = snipe_edge_sum / (snipe_dn_trades + snipe_dir_trades)` as "avg edge/trade".

- **Exploit 2 (Edge-Scale):** Tracks `edge_scaled_trades`, `edge_scale_sum`,
  `edge_original_size_sum`. No isolated P&L — it modifies trade size, not trade
  decision. The profit impact is diffuse across all sized trades.

  **What to show instead:** `avg_scale = edge_scale_sum / edge_scaled_trades` as
  "avg size multiplier", `edge_original_size_sum` vs actual sizes as "position
  amplification", `edge_scaled_trades` count.

- **Exploit 3 (Stale Gate):** Tracks `exploit3_dn_gated` and `exploit3_dir_gated`
  (trades BLOCKED). No P&L — it prevents trades. The value is in avoided losses,
  which is counterfactual (you'd need a baseline run without the gate).

  **What to show instead:** `exploit3_dn_gated + exploit3_dir_gated` as "trades
  blocked (capital saved)", show the gated count relative to total attempted.

### 4.3 Recommended Scoreboard Layout (Corrected)

```
┌─ EXPLOIT SCOREBOARD ──────────────────────────────────────────────────────┐
│                                                                           │
│ ┌─ 1. PRICE SNIPING ──────┐  ┌─ 2. EDGE SCALING ─────────┐             │
│ │ Passed: 23 DN, 18 DIR   │  │ Scaled trades: 41          │             │
│ │ Blocked: 8 DN, 5 DIR    │  │ Avg multiplier: 1.32×      │             │
│ │ Cumulative edge: 4.2¢   │  │ Position amplified: +28%   │             │
│ │ Avg edge/trade: 0.10¢   │  │ Base→Actual: 1230→1574 sh  │             │
│ │ Status: ACTIVE           │  │ Status: ACTIVE              │             │
│ └──────────────────────────┘  └─────────────────────────────┘             │
│                                                                           │
│ ┌─ 3. STALE GATE ─────────┐  ┌─ 4. CHAINLINK ARB ─────────┐            │
│ │ DN blocked: 34           │  │ Trades: 7                   │            │
│ │ DIR blocked: 12          │  │ Cost: $42.30                │            │
│ │ Total prevented: 46      │  │ Profit: $18.70              │            │
│ │ Est. capital saved: ~$xx │  │ ROI: 44.2%                  │            │
│ │ Gate: OPEN (7.2 bps)     │  │ Avg CL dev: 8.1 bps        │            │
│ └──────────────────────────┘  │ % of total profit: 12.3%    │            │
│                               │ Status: ARMED               │            │
│                               └─────────────────────────────┘            │
└──────────────────────────────────────────────────────────────────────────-┘
```

### 4.4 Data Flow for Attribution Calculations

```
Per-market result dict
    │
    ├─ Exploit 1: snipe_dn_trades, snipe_dir_trades, snipe_dn_blocked,
    │             snipe_dir_blocked, snipe_edge_sum
    │             → sum across markets for portfolio totals
    │             → avg_edge = edge_sum / (dn_trades + dir_trades)
    │
    ├─ Exploit 2: edge_scaled_trades, edge_scale_sum, edge_original_size_sum
    │             → avg_scale = scale_sum / scaled_trades
    │             → amplification = 1 - (original_sum / actual_total_shares)
    │
    ├─ Exploit 3: exploit3_dn_gated, exploit3_dir_gated
    │             → total_prevented = dn_gated + dir_gated
    │             → gated_rate = prevented / (prevented + total_trades)
    │
    └─ Exploit 4: chainlink_arb_trades, chainlink_arb_cost,
                  chainlink_arb_profit, chainlink_arb_edge_sum
                  → roi = profit / cost * 100
                  → pct_of_total = cl_profit / total_profit * 100
                  → avg_cl_dev = edge_sum / trades
```

### 4.5 Real-Time Update Frequency

- Exploits 1-3 counters: update on every trade event (immediate)
- Exploit 4 P&L: update on trade + at resolution (profit only known at resolution)
- During live trading: show unrealized P&L for Exploit 4 based on current prices
- Scoreboard refresh: every render cycle (matches dashboard.rs 16ms poll interval)

---

## Cross-Task Coordination Notes

### For UI Specialist (tasks #3, #7, #8):
- TradeEvent struct above is the canonical data contract between timeline and backend
- Color scheme follows existing dashboard.rs palette (CG, CR, CY, COR, etc.)
- All new panels use `panel()` helper with accent colors per section
- Risk alerts should render in the signal bar area (v[4] in draw() layout)

### For Data Analyst (task #2):
- The 130+ metrics reduce to ~20 critical ones for real-time display
- Exploit attribution requires aggregation across markets for portfolio view
- Trade log is per-market; portfolio timeline needs merge-sort by timestamp

### Layout Integration (task #7):
New draw() layout should accommodate mode switching:

```
Trading Mode:   header | ttx | zscore | [book + timeline] | [risk + signals]
Analytical Mode: header | ttx | [exploit scoreboard] | [position comparison]
Risk Mode:      header | [capital + alerts] | [position health] | [signal health]
```
