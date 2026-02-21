# Dashboard Implementation Quick Reference
## Fast Lookup During Development

---

## File Structure

```
src/dashboard/
├── colors.rs                 [NEW] Accessibility palette
├── modes.rs                  [NEW] 3-mode system
├── config.rs                 [NEW] .toml configuration
├── update_tracking.rs        [NEW] Update highlighting
├── data_quality.rs           [NEW] Feed health tracking
├── event_handler.rs          [NEW] Keyboard commands
│
├── renderers/                [NEW] Mode-specific renders
│   ├── primary_signal.rs     Trading mode primary
│   ├── confirmation.rs       Trading mode confirmation
│   ├── context.rs            Trading mode context
│   ├── analytical.rs         Full analytical mode
│   └── risk.rs               Risk/monitoring mode
│
├── layout.rs                 [MODIFY] Add mode-aware layout
└── mod.rs                    [MODIFY] Export new modules

dashboard.rs                  [MODIFY] Main dashboard (use new renderers)
```

---

## Color Palette Reference

### Semantic Colors (WCAG AAA)
```rust
Bullish:  rgb(0, 216, 115)    // Bright green
Bearish:  rgb(255, 70, 70)    // Bright red
Neutral:  rgb(184, 184, 184)  // Medium gray
Warning:  rgb(255, 210, 40)   // Yellow-gold

Positive: rgb(77, 217, 255)   // Cyan-blue
Negative: rgb(255, 155, 35)   // Orange
Primary:  rgb(0, 195, 155)    // Teal
Inactive: rgb(90, 90, 111)    // Dark gray

Deep BG:  rgb(10, 10, 16)     // Near-black
Panel BG: rgb(24, 24, 32)     // Slightly lighter
Sep:      rgb(58, 58, 80)     // Divider
```

### Color Selection Rules
```rust
match value_type {
    Direction if value > 0 => colors.bullish,
    Direction if value < 0 => colors.bearish,
    Direction if value == 0 => colors.neutral,

    Confidence(c) if c >= 0.8 => colors.bullish,
    Confidence(c) if c >= 0.6 => colors.warning,
    Confidence(c) if c >= 0.4 => colors.negative,
    Confidence(_) => colors.bearish,

    Signal(s) if s >= 4 => colors.bullish,
    Signal(s) if s >= 1 => colors.positive,
    Signal(0) => colors.neutral,
    Signal(s) if s <= -4 => colors.bearish,
    Signal(_) => colors.negative,
}
```

---

## Mode Allocation Quick Reference

### Trading Mode (40/35/25 split)
```
PRIMARY SIGNAL (40%)  - Z-Score, Confidence, Recommendation
CONFIRMATION (35%)    - OBI, CVD, Volatility, Execution
CONTEXT (25%)         - Freshness, Regime, Toggle help
```

### Analytical Mode (15/25/60 split)
```
PRIMARY SIGNAL (15%)  - Minimal
CONFIRMATION (25%)    - Order book detail
CONTEXT (60%)         - Distribution, CVD history, Greeks, all detail
```

### Risk Mode (20/15/65 split)
```
PRIMARY SIGNAL (20%)  - Signal status
CONFIRMATION (15%)    - OBI, CVD status
CONTEXT (65%)         - Latency matrix, RTDS detail, feed health
```

---

## Keyboard Command Map

```
1          Trading Mode (default)
2          Analytical Mode
3          Risk Mode

d          Toggle detail level
l          Toggle latency breakdown
g          Toggle Greeks
h          Toggle history charts

?          Show help
q / Esc    Exit
```

---

## Configuration Keys

```toml
[display]
color_scheme = "default"              # or "high_contrast", "legacy"
layout_mode = "trading"               # startup mode
update_rate_hz = 60
show_greek_details = false
show_latency_detail = false

[trading]
rtds_advantage_threshold_bps = 3.0    # Alert threshold
z_score_threshold_alert = 2.5
volatility_threshold_alert = 0.15
confidence_threshold_alert = 0.60

[alerts]
enable_audio_alerts = true
enable_update_highlighting = true
enable_stale_warnings = true
stale_threshold_ms = 1500
```

---

## Metric Field IDs (Update Tracking)

```rust
enum MetricField {
    // Primary signal tier
    ZScore,
    ZConfidence,
    SignalScore,
    VolatilityRegime,

    // Confirmation tier
    ObiValue,
    CvdOneM,
    CvdFiveM,
    ExpectedMove,
    Delta,

    // Context tier
    PriceSpot,
    YesBid, YesAsk,
    NoBid, NoAsk,
    Strike,
    TimeToExpiry,
    Outcome,

    // Extended (detail mode)
    Gamma, Vega, Theta,
    YesSpread, NoSpread,
    YesFee, NoFee,
    VolSpike,
    // ... more as needed
}
```

---

## Data Quality Levels

```rust
Fresh    → max_latency <= 200ms   → All feeds live (Green)
Lagging  → 200ms < max <= 800ms   → One feed behind (Yellow)
Stale    → 800ms < max <= 3000ms  → One feed old (Orange)
Critical → max_latency > 3000ms   → Multiple down (Red)
```

---

## Update Tracker Usage

```rust
// In your render loop:
let mut tracker = UpdateTracker::new();

// After fetching new data:
if z_score != old_z_score {
    tracker.record_update(
        MetricField::ZScore,
        old_z_score.to_i64(),
        new_z_score.to_i64(),
    );
}

// In render function:
if tracker.should_highlight(MetricField::ZScore) {
    let fade = tracker.highlight_fade(MetricField::ZScore);
    let age_ms = tracker.age_ms(MetricField::ZScore);

    // Apply highlight styling
    // Use fade factor for opacity (1.0 = full, 0.0 = none)
    // Use age_ms for "updated N ms ago" display
}
```

---

## Event Handling Pattern

```rust
// In main loop:
if event::poll(Duration::from_millis(16))? {
    if let Event::Key(k) = event::read()? {
        if let Some(new_mode) = event_handler.handle_event(Event::Key(k)) {
            // Mode switched
            config.display.layout_mode = format!("{:?}", new_mode);
        }
    }
}

// Later in render:
let mode_config = ModeConfig::from_mode(event_handler.current_mode());
let panel_alloc = event_handler.current_mode().panel_allocation();
```

---

## Common Render Patterns

### Redundant Signal Encoding
```rust
// Instead of: Color alone
Span::styled("BULLISH", Style::default().fg(CG))

// Do this: Arrow + Color + Text
Line::from(vec![
    Span::styled("▲", Style::default().fg(colors.bullish)),
    Span::styled(" BULLISH", Style::default()
        .fg(colors.bullish)
        .add_modifier(Modifier::BOLD)),
])
```

### Age Display
```rust
fn format_age(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

// Usage:
let age = tracker.age_ms(MetricField::ZScore);
Span::styled(
    format!("({} ago)", format_age(age)),
    Style::default().fg(colors.inactive),
)
```

### Update Highlighting
```rust
let highlight_style = if tracker.should_highlight(field) {
    Style::default()
        .fg(color)
        .bg(colors.panel_bg)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
} else {
    Style::default().fg(color)
};
```

### Data Quality Status Bar
```rust
let (quality_str, quality_color) = match tracker.assess_quality() {
    DataQuality::Fresh => ("✓ All feeds live", colors.bullish),
    DataQuality::Lagging => ("! Poly lagging", colors.warning),
    DataQuality::Stale => ("! FEED STALE", colors.negative),
    DataQuality::Critical => ("✗ UNRELIABLE", colors.bearish),
};

let status_line = Line::from(vec![
    Span::styled(quality_str, Style::default()
        .fg(quality_color)
        .add_modifier(Modifier::BOLD)),
]);
```

---

## Testing Checklist

### Color Palette Tests
```rust
#[test]
fn test_bullish_bearish_contrast() {
    let colors = DashboardColors::default();
    // Calculate contrast ratio
    // Verify >= 7:1 (WCAG AAA)
}

#[test]
fn test_color_blind_simulation() {
    let colors = DashboardColors::default();
    // Simulate deuteranopia (red-green)
    // Verify all colors still distinguishable
}
```

### Mode Tests
```rust
#[test]
fn test_mode_allocation() {
    let trading = DashboardMode::Trading.panel_allocation();
    assert_eq!(trading.primary_signal, 40);
    assert_eq!(trading.confirmation, 35);
    assert_eq!(trading.context, 25);
}
```

### Update Tracker Tests
```rust
#[test]
fn test_update_highlighting() {
    let mut tracker = UpdateTracker::new();
    tracker.record_update(MetricField::ZScore, 100, 123);

    assert!(tracker.should_highlight(MetricField::ZScore));
    std::thread::sleep(Duration::from_millis(600));
    assert!(!tracker.should_highlight(MetricField::ZScore));
}
```

### Data Quality Tests
```rust
#[test]
fn test_quality_assessment() {
    let mut tracker = DataQualityTracker::new();
    tracker.update_feed("Binance", 100);
    assert_eq!(tracker.assess(), DataQuality::Fresh);

    tracker.update_feed("Polymarket YES", 500);
    assert_eq!(tracker.assess(), DataQuality::Lagging);
}
```

---

## Performance Considerations

### Render Performance (Target: 16ms per frame @ 60Hz)

```
TIME BUDGET:
├─ Event handling:      2ms (keyboard input)
├─ Data updates:        3ms (fetch new metrics)
├─ Calculate styling:   4ms (color, formatting)
├─ Layout calculation:  3ms (ratatui constraints)
└─ Rasterization:       4ms (terminal output)
   TOTAL:               16ms ✓

OPTIMIZATION TACTICS:
1. Cache color objects (don't recreate per frame)
2. Lazy-render invisible panels (mode-specific)
3. Batch layout calculations (run once per mode change)
4. Use Rc<Style> for repeated styles
5. Minimize string allocations (use String::with_capacity)
```

### Memory Usage (Target: <50MB)

```
History tracking:      ~10MB (500 z-scores + prices)
CVD accumulation:      ~2MB (300 entries × 3 windows)
Update tracker:        ~1MB (100 recent updates)
Config + colors:       <1MB
Config strings:        ~2MB (metric names, labels)
         ────────────────
         TOTAL:        ~16MB ✓
```

---

## Debugging Tips

### Print Debug Info
```rust
#[cfg(debug_assertions)]
eprintln!("Mode: {:?}, Quality: {:?}, Highlight: {}",
    event_handler.current_mode(),
    data_quality.assess(),
    update_tracker.should_highlight(MetricField::ZScore)
);
```

### Test Color Rendering
```bash
# Set RUST_BACKTRACE for full error context
RUST_BACKTRACE=1 cargo run --bin btc_dashboard

# Capture terminal output to file for inspection
script -q output.txt -c "cargo run --bin btc_dashboard"
```

### Verify Config Loading
```rust
let config = DashboardConfig::load_or_default()
    .expect("Config load failed");
println!("Config loaded: {:?}", config);
println!("Layout mode: {}", config.display.layout_mode);
```

---

## Quick Wins (Implement First)

### 1. Add Age Indicators (1 hour)
```rust
// Every metric shows: "Z-Score: +1.23 (0.3s ago)"
// Files: colors.rs, renderers/primary_signal.rs
```

### 2. Group Related Metrics (2 hours)
```rust
// Consolidate volatility indicators into single subsection
// Files: renderers/confirmation.rs
```

### 3. Signal Confidence Breakdown (1.5 hours)
```rust
// Show which signals agree/disagree
// Files: renderers/primary_signal.rs
```

### 4. Color-Blind Mode Toggle (1 hour)
```rust
// Ctrl+C toggles high_contrast palette
// Files: colors.rs, event_handler.rs
```

### 5. Terminal Width Check (30 min)
```rust
// Graceful degradation for <100 char width
// Files: layout.rs
```

---

## References

**Files in This Repository:**
- `/home/lehash/development/combine-fetcher/UI_UX_ANALYSIS.md` - Full analysis
- `/home/lehash/development/combine-fetcher/UI_UX_IMPLEMENTATION_GUIDE.md` - Code examples
- `/home/lehash/development/combine-fetcher/TEAM_LEAD_BRIEFING.md` - Executive summary

**External Standards:**
- WCAG 2.1 AAA Color Contrast: https://www.w3.org/WAI/WCAG21/Understanding/contrast-enhanced
- Color-Blind Simulation: https://www.color-blindness.com/coblis-color-blindness-simulator/
- TUI Best Practices: https://ratatui.rs/

---

## Common Mistakes to Avoid

```
❌ DON'T: Hardcode colors (use DashboardColors everywhere)
✓ DO: Pass &colors reference through render functions

❌ DON'T: Ignore color-blind accessibility (costs 8% of users)
✓ DO: Test with color-blind simulator before shipping

❌ DON'T: Calculate layouts on every frame (slow)
✓ DO: Cache layout constraints, recalc only on mode change

❌ DON'T: Track updates without bounding memory (grows unbounded)
✓ DO: Use VecDeque with max_history limit

❌ DON'T: Mix metric consolidation with other refactoring
✓ DO: Keep each sprint focused on one goal

❌ DON'T: Assume all traders want same layout
✓ DO: Provide configuration system early
```

---

**Last Updated:** February 20, 2026
**Status:** Ready for Development
**Questions?** See full analysis documents or schedule discussion
