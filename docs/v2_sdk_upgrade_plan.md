# combine-fetcher V2 SDK Upgrade Plan

> Companion to `../../poly-bot/docs/v2_sdk_migration_plan.md` (which covers the trading bot).
> Scope: SDK swap **only**. Logger binaries (`*_fixed*.rs`) stay byte-identical.
> Status: drafted 2026-04-28; cutover target: TBD (post-poly-bot V2 cutover stabilization).

---

## Table of contents

- [1. Goals & non-goals](#1-goals--non-goals)
- [2. Critical constraints](#2-critical-constraints)
- [3. Branch strategy](#3-branch-strategy)
- [4. SDK consumption strategy (path-dep vs vendor)](#4-sdk-consumption-strategy-path-dep-vs-vendor)
- [5. File-by-file change sketch](#5-file-by-file-change-sketch)
- [6. Column compatibility matrix](#6-column-compatibility-matrix)
- [7. Sequenced TODO list](#7-sequenced-todo-list)
- [8. Byte-comparison validation](#8-byte-comparison-validation)
- [9. Rollback plan](#9-rollback-plan)
- [10. Out of scope](#10-out-of-scope)
- [11. References](#11-references)

---

## 1. Goals & non-goals

### Goals

1. Swap the vendored V1 SDK (`polymarket-client-sdk` 0.4.1, path-patched at `polymarket-client-sdk-patched/`) for the patched V2 SDK that lives in poly-bot at `crates/polymarket-client-sdk-v2-patched/`.
2. **Preserve every public surface** in `combine_fetcher::api::*` so logger binaries (`src/bin/*_fixed*.rs` — five binaries) compile and run unchanged.
3. **Preserve every CSV/parquet column** documented in `polymarket-dev-docs/logger_columns.md` (83 columns for `multi_asset_logger_fixed_rotation_sbe_obi`).
4. Stay on the legacy `clob.polymarket.com` host throughout. The V2 SDK auto-detects v1 vs v2 protocol via `GET /version`, so the same V2 binary speaks v1 against the legacy host until Polymarket retires it. No code-level URL change needed for combine-fetcher's REST fallback path.
5. Keep RTDS, Gamma, and WebSocket behavior identical — no semantic drift in `RtdsTracker`, `MultiAssetRtdsTracker`, `MarketEventStream`, `PolymarketOrderbookStream`.

### Non-goals (explicitly punted)

1. **No changes to logger binaries.** Anything in `src/bin/` is read-only. If a logger needs editing to compile, the patch belongs in the API layer instead.
2. **No CSV column additions, removals, or reorderings.** Even if V2 ships richer message types (e.g. `LastTradePrice.fee_rate_bps` is now `Option<Decimal>` — non-`None` on V2), do **not** surface them. The 83-column schema is frozen.
3. **No order submission, no auth, no CTF split/merge, no fees logic, no signing.** combine-fetcher is a public-WS-only consumer. None of the high-risk poly-bot V2 work (R5/R7/R12/R14/R15/R16/R20/R-pUSD/EIP-712) applies.
4. **No `Config::http_client` retuning.** combine-fetcher's only REST client is `ClobClient::new(CLOB_API_URL, Config::default())` for the midpoint/last-trade fallback in `src/api/polymarket.rs:1287`. We rely on the SDK default reqwest client; no production hot-path latency budget here.
5. **No host flip in this PR.** Cutover host change (`clob.polymarket.com` → `clob-v2.polymarket.com`) is a one-line follow-up that can ride on top of this PR or stay deferred — the V2 SDK auto-detects either way.
6. **No re-vendoring of upstream V2 0.5.1.** We consume the patched copy from poly-bot (Option A in §4).

---

## 2. Critical constraints

| Constraint | Implication |
|---|---|
| **Logger binaries are read-only.** Five `*_fixed*` files in `src/bin/`. | Any V2-induced churn must land in `src/api/*.rs`. If a binary fails to compile, the fix is in the API layer. |
| **CSV schema (83 columns) is frozen.** Documented in `polymarket-dev-docs/logger_columns.md`. | Any new V2 message field (e.g. `BestBidAsk` in V2 has `fee_rate_bps: Option<Decimal>` which V1 lacked) must be **dropped** at the API boundary. The internal `TokenPrices` struct has no fee field; no work needed to enforce — but spot-check during code review. |
| **No auth, no signing, no order submission.** | Unrelated risks from poly-bot's plan are N/A: R15 (request timeouts), R16 (`ORDER_VERSION_MISMATCH` retry), R20 (user-WS unsubscribe leak), R-pUSD collateral, EIP-712 v1→v2 domain bump, `OrderType::Default = FOK` flip — none apply. |
| **`MarketResolved.slug` shape diverges between V1 (`String`) and V2 (`Option<String>`).** | The combine-fetcher already handles both via the `extract_slug(slug: impl Into<Option<String>>)` helper at `src/api/market_events.rs:34`. **Already future-proofed** — no change needed. |
| **`Side`, `OrderType`, `OrderStatus`, etc. are `#[non_exhaustive]` in V2** with new `Unknown` variants. | The only `Side` match in combine-fetcher (`src/api/polymarket.rs:1120-1131`) already uses `_ =>` arms for both the side-string lookup and the volume accumulation. **Already future-proofed** — no change needed. |
| **`Config::http_client` was REMOVED in upstream V2 0.5.1, RESTORED in poly-bot's patched fork.** | combine-fetcher uses `Config::default()` (no http_client tuning), so even unpatched V2 0.5.1 would work. We point at the patched copy anyway for consistency. |
| **Gamma API is hand-rolled JSON in combine-fetcher** (`src/api/gamma.rs`), not SDK-driven. | The poly-bot R-Gamma-schema risk (V2 `Market` struct dropped `feeSchedule`/`feeType`, requiring `FeeSchedule::default()` fallback) is N/A for combine-fetcher. |
| **`workspace.dependencies` doesn't exist** in combine-fetcher's `Cargo.toml` — it's a flat single-crate package with one `[patch.crates-io]` override and `signal-shm` as a path sub-crate. | The dep swap is a single-file edit to `Cargo.toml` with a `[patch.crates-io]` redirect, mirroring poly-bot's approach. |

---

## 3. Branch strategy

Recommend a **single migration branch** off the current `feat/signal-engine` head (or whichever branch is current at the time of execution). The work is mechanical and small enough to be one PR; a side branch lets us validate against a baseline V1 build via diff (see §8).

```
feat/signal-engine                             [current head]
└── feat/signal-engine/v2-sdk-upgrade          [migration branch]
    ├── commit 1: Add v2 dep, point [patch.crates-io] at poly-bot path
    ├── commit 2: Mechanical s/polymarket_client_sdk/polymarket_client_sdk_v2/g across src/api/
    ├── commit 3: cargo check fixes (any non_exhaustive / type-shape diffs)
    ├── commit 4: cargo build --bin <each *_fixed* binary> green
    └── commit 5: 1-hour smoke test artifacts + diff report
```

Why one branch (not two SDKs side-by-side or a long-lived fork):

- The work is small (roughly 5 SDK touch sites in 4 API files).
- The same binary speaks v1 to `clob.polymarket.com` post-swap, so we don't need a feature flag.
- A `git revert` of the merge commit is the rollback if something goes sideways.

---

## 4. SDK consumption strategy (path-dep vs vendor)

Two viable paths:

| Strategy | Pros | Cons |
|---|---|---|
| **A. Point `[patch.crates-io]` at poly-bot's patched copy** (`../poly-bot/crates/polymarket-client-sdk-v2-patched`) | Zero code copy; reuses the 11-line `Config::http_client` patch (free even though we don't tune it); single source of truth — any future SDK fix in poly-bot propagates; matches how the existing V1 patched dep works (path-relative inside the repo, but conceptually identical) | Cross-project path dep — fragile if poly-bot relocates or someone clones combine-fetcher without poly-bot adjacent; CI must clone both projects |
| **B. Vendor a fresh copy** of the patched V2 SDK (or upstream v0.5.1) into `combine-fetcher/polymarket-client-sdk-v2-patched/` | Project-local; no cross-project dep | Divergence over time; we'd need to manually port any future poly-bot SDK fix; effectively forks the SDK twice |

**Recommendation: Option A.** Both projects live under `~/development/`. If they ever need to be cloned independently (CI/CD, contributor handoff), Option B can be adopted later by `cp -r` of the patched directory and a one-line edit to `[patch.crates-io]`. Until then, Option A is the simplest and cheapest.

Concrete `Cargo.toml` change sketch:

```toml
# combine-fetcher/Cargo.toml
[dependencies]
# OLD: polymarket-client-sdk = { version = "0.4.1", features = ["clob", "ws", "rtds"] }
polymarket_client_sdk_v2 = { version = "0.5", features = ["clob", "ws", "rtds"] }

# OLD: [patch.crates-io] polymarket-client-sdk = { path = "polymarket-client-sdk-patched" }
[patch.crates-io]
polymarket_client_sdk_v2 = { path = "../poly-bot/crates/polymarket-client-sdk-v2-patched" }
```

Note on crate-name capitalization: V2 upstream publishes as `polymarket_client_sdk_v2` (underscore form, no hyphen). The `[patch.crates-io]` key must match the published crate name. Verify against the patched copy's `Cargo.toml` before committing — adjust if it lists itself under a different name.

---

## 5. File-by-file change sketch

### 5.1 `Cargo.toml` (workspace dep swap)

- Replace the single `[dependencies] polymarket-client-sdk = ...` line.
- Replace the single `[patch.crates-io] polymarket-client-sdk = { path = ... }` line.
- Keep features `["clob", "ws", "rtds"]` — V2 patched copy supports the same set.
- Optional: drop the local `polymarket-client-sdk-patched/` directory **only after** the V2 swap is verified (and possibly archived as a tag for rollback).

### 5.2 `src/api/polymarket.rs` (the heaviest touch site)

Lines that change (all mechanical):

| Old (V1) | New (V2) | Notes |
|---|---|---|
| `use polymarket_client_sdk::clob::types::request::{LastTradePriceRequest, MidpointRequest};` | `use polymarket_client_sdk_v2::clob::types::request::{LastTradePriceRequest, MidpointRequest};` | Same struct names |
| `use polymarket_client_sdk::clob::types::Side;` | `use polymarket_client_sdk_v2::clob::types::Side;` | V2 `Side` is `#[non_exhaustive]` with new `Unknown` variant — match arms already use `_ =>` (lines 1123, 1130) |
| `use polymarket_client_sdk::clob::ws::Client as WsClient;` | `use polymarket_client_sdk_v2::clob::ws::Client as WsClient;` | Same constructors and method shapes |
| `use polymarket_client_sdk::clob::ws::LastTradePrice;` | `use polymarket_client_sdk_v2::clob::ws::LastTradePrice;` | V2 adds `fee_rate_bps: Option<Decimal>` — combine-fetcher only reads `price`, `size`, `side`, `timestamp` (lines 1116-1134), so the new field is silently ignored. **No CSV column impact.** |
| `use polymarket_client_sdk::clob::{Client as ClobClient, Config};` | `use polymarket_client_sdk_v2::clob::{Client as ClobClient, Config};` | `ClobClient::new(url, Config::default())` signature unchanged |
| `use polymarket_client_sdk::types::U256;` | `use polymarket_client_sdk_v2::types::U256;` | Same alloy-backed type |

Internal logic untouched. The `WsClient::default()` calls, the `subscribe_best_bid_ask`/`subscribe_orderbook`/`subscribe_last_trade_price` invocations, the `BestBidAsk`/`BookUpdate`/`LastTradePrice` field access patterns (`asset_id`, `bids`, `asks`, `best_bid`, `best_ask`, `price`, `size`, `side`, `timestamp`) all match between V1 and V2.

### 5.3 `src/api/market_events.rs`

Mechanical rename only:

```rust
use polymarket_client_sdk::clob::ws::Client as WsClient;
use polymarket_client_sdk::types::U256;
```
becomes:
```rust
use polymarket_client_sdk_v2::clob::ws::Client as WsClient;
use polymarket_client_sdk_v2::types::U256;
```

The existing `extract_slug(slug: impl Into<Option<String>>)` helper at line 34 already handles both V1's `slug: String` and V2's `slug: Option<String>` shape on `MarketResolved`. **No further work.**

`subscribe_new_markets` and `subscribe_market_resolutions` exist with identical signatures in V2.

### 5.4 `src/api/rtds_tracker.rs` and `src/api/rtds_tracker_multi.rs`

Mechanical rename of every `polymarket_client_sdk::rtds::Client` → `polymarket_client_sdk_v2::rtds::Client` (5 hits in `rtds_tracker.rs`, 5 hits in `rtds_tracker_multi.rs`).

`Client::default()`, `subscribe_crypto_prices(Option<Vec<String>>)`, `subscribe_chainlink_prices(Option<String>)`, and the `CryptoPrice {symbol, value, timestamp}` / `ChainlinkPrice {symbol, value, timestamp}` field shapes all match V1 → V2 (verified against `poly-bot/crates/polymarket-client-sdk-v2-patched/src/rtds/types/response.rs`).

The known SDK quirk — `subscribe_crypto_prices(Some(vec!["btcusdt"]))` silently produces an empty stream, so the code uses `None` and filters server-side — applies identically in V2. The existing comments remain valid.

### 5.5 `src/api/polymarket.rs.bak` and `src/api/binance.rs.bak`

These are stale `.bak` files (not compiled). Leave alone or delete in a separate cleanup PR. **Do not migrate** — touching them would muddy the diff.

### 5.6 `src/api/gamma.rs`

**No changes.** This file makes raw HTTP calls via `reqwest::Client` directly against `https://gamma-api.polymarket.com`. It does not import the SDK. The fact that V2 SDK's `Market` struct dropped `feeSchedule`/`feeType` fields (§5.4 of poly-bot plan) is irrelevant here — combine-fetcher's gamma layer parses Gamma JSON manually with `serde_json::Value::as_str().unwrap_or(...)` patterns, so it's both forgiving and decoupled from any SDK schema.

### 5.7 `src/api/binance*.rs`, `src/api/strike.rs`, `src/api/binance_fixed*.rs`

**No changes.** None of these import the Polymarket SDK.

### 5.8 `src/main.rs`, `src/lib.rs`, `src/api/mod.rs`

**No changes.** A doc-comment in `src/main.rs:3` mentions "polymarket-client-sdk" — no functional impact. Optional cosmetic update; not required.

### 5.9 Logger binaries — `src/bin/*_fixed*.rs` (five files)

**ABSOLUTELY NO CHANGES.** Each binary imports only from `combine_fetcher::api::*` (verified — zero direct SDK imports across all 5 binaries). Since all `combine_fetcher::api::*` public surfaces stay byte-identical (same struct names, same field names, same method signatures), the binaries don't need a single touch.

The five binaries:

| Binary | Doc summary | API touch points |
|---|---|---|
| `multi_market_logger_hourly_fixed.rs` | BTC 15m, single-market hourly logger; uses `RtdsTracker` (single-asset) | `binance_fixed`, `gamma`, `market_events`, `polymarket`, `rtds_tracker`, `strike` |
| `multi_asset_logger_fixed.rs` | Multi-asset (BTC/ETH/SOL/XRP), 5m+15m, JSON Binance feeds; uses `MultiAssetRtdsTracker` | Same as above + `rtds_tracker_multi` |
| `multi_asset_logger_fixed_rotation.rs` | Same as `_fixed` plus 85% pre-warm rotation | Same set |
| `multi_asset_logger_fixed_rotation_sbe.rs` | Same as `_rotation` but uses `binance_fixed_sbe` (SBE for `@bestBidAsk` and `@trade`) | Same set, swap to `binance_fixed_sbe` |
| `multi_asset_logger_fixed_rotation_sbe_obi.rs` | **Production logger.** Same as `_sbe` plus SBE `@depth20` (replaces JSON depth) | Same set, swap to `binance_fixed_sbe_obi` |

All five touch the same `polymarket` / `market_events` / `rtds_tracker` / `rtds_tracker_multi` API surface. Since none of these surfaces change in the V2 swap, the binaries trivially recompile.

---

## 6. Column compatibility matrix

The 83-column schema for the canonical production logger (`multi_asset_logger_fixed_rotation_sbe_obi`) decomposes into three groups by data source:

| Group | Columns | Source | V1→V2 risk |
|---|---|---|---|
| Local clock & calculated | 1, 13, 14, 17-22, 23, 27, 30, 38, 40, 42-43, 48-49, 56-57, 60-61, 64-65, 67-68, 75, 80-83 | `chrono::Utc::now()`, formula calculations | **None.** No SDK touch. |
| Binance (SBE/JSON) | 3-4, 24, 28-29, 31-37, 39, 41, 76-79 | `binance_fixed*` modules — independent of Polymarket SDK | **None.** SBE decoder code untouched. |
| **Polymarket WS — orderbook** | 5-12, 25-26, 54-55, 58-59, 62-63, 66 | `PolymarketOrderbookStream` via SDK `subscribe_orderbook` | Verified: V2 `BookUpdate` has `asset_id: U256`, `bids: Vec<OrderBookLevel>`, `asks: Vec<OrderBookLevel>`, `timestamp: i64`, `hash: Option<String>` (new optional field, ignored). `OrderBookLevel { price: Decimal, size: Decimal }` unchanged. **Zero column shift.** |
| **Polymarket WS — BBA** | (subset of 5-12 + lifecycle metadata) | `subscribe_best_bid_ask` | Verified: V2 `BestBidAsk` retains `asset_id`, `best_bid: Decimal`, `best_ask: Decimal`, `timestamp: i64`. V2 adds `fee_rate_bps: Option<Decimal>` — irrelevant (combine-fetcher only reads bid/ask/timestamp). **Zero column shift.** |
| **Polymarket WS — last trade** | 69-75 | `subscribe_last_trade_price` | Verified: V2 `LastTradePrice` retains `asset_id`, `price: Decimal`, `size: Option<Decimal>`, `side: Option<Side>`, `timestamp: i64`. V2 adds `fee_rate_bps: Option<Decimal>` — irrelevant. **Zero column shift.** |
| **Polymarket RTDS** | 44-53 | `RtdsTracker` / `MultiAssetRtdsTracker` via `subscribe_crypto_prices` + `subscribe_chainlink_prices` | Verified: `CryptoPrice` and `ChainlinkPrice` shapes (`symbol: String`, `value: Decimal`, `timestamp: i64`) unchanged in V2. **Zero column shift.** |
| **Polymarket WS — market lifecycle** | 2 (`market_slug`), 22 (`outcome` cascade) | `MarketEventStream` via `subscribe_new_markets` + `subscribe_market_resolutions` | V2 `MarketResolved.slug` is `Option<String>` (was `String` in this V1 patched copy). Already absorbed by `extract_slug()` helper. The `slug` written to CSV column 2 comes from the GAMMA API via `gamma::fetch_current_market_for`, **not** the SDK — so even hypothetical lifecycle quirks have no CSV path. **Zero column shift.** |
| **Polymarket Gamma REST** | 2 (`market_slug`), 13 (`time_to_open`), 14 (`time_to_expiry_minutes`), 22 (`outcome` cascade fallback) | `gamma::fetch_market_for`, `gamma::fetch_outcome` | Hand-rolled `reqwest` JSON. Does not use the SDK. **Zero column shift.** |
| **Polymarket REST midpoint+last_trade fallback** | (none direct — only feeds same TokenPrices columns when WS is stale) | `ClobClient::midpoint(MidpointRequest)`, `ClobClient::last_trade_price(LastTradePriceRequest)` | Same struct names, same return shapes (`MidpointResponse.mid: Decimal`, `LastTradePriceResponse.price: Decimal`). **Zero column shift.** |

**Conclusion:** the V2 SDK swap produces **zero CSV column changes** for any of the 83 columns documented in `polymarket-dev-docs/logger_columns.md`. Format specifiers (`{:.2}`, `{:.4}`, `{:.6}`, `{:.8}`, etc.) are determined entirely in the binary serialization code which we don't touch.

---

## 7. Sequenced TODO list

### Step 1 — Bootstrap (15 min)

- [ ] **Pre-flight:** confirm poly-bot V2 cutover succeeded (`feat/multi-ticker-tail/v2-sdk-migrate` merged to `main`, mainnet trading green for ≥24h after 2026-04-28 cutover). If poly-bot V2 is still in flight or has issues, **defer this PR**.
- [ ] Confirm `~/development/poly-bot/crates/polymarket-client-sdk-v2-patched/Cargo.toml` package name (`polymarket_client_sdk_v2`) and version. Note the exact value for §5's `[patch.crates-io]` key.
- [ ] Branch off current head: `git checkout -b feat/v2-sdk-upgrade`.
- [ ] Tag rollback point: `git tag pre-v2-baseline`.
- **Verification:** `cargo build` green on baseline.
- **Commit msg draft:** none — bootstrap only.

### Step 2 — Workspace dep swap (10 min)

- [ ] Edit `Cargo.toml`:
  - Remove `polymarket-client-sdk = { version = "0.4.1", features = ["clob", "ws", "rtds"] }`.
  - Add `polymarket_client_sdk_v2 = { version = "0.5", features = ["clob", "ws", "rtds"] }`.
  - Replace `[patch.crates-io] polymarket-client-sdk = { path = "polymarket-client-sdk-patched" }` with `[patch.crates-io] polymarket_client_sdk_v2 = { path = "../poly-bot/crates/polymarket-client-sdk-v2-patched" }`.
- [ ] Run `cargo update -p polymarket_client_sdk_v2` to lock the path-resolved version into `Cargo.lock`.
- **Verification:** `cargo metadata --format-version 1 | jq '.packages[] | select(.name|test("polymarket"))'` shows V2 only.
- **Commit msg draft:** `chore(deps): swap V1 SDK for patched V2 (path-dep on poly-bot)`

### Step 3 — Mechanical rename across `src/api/` (10 min)

- [ ] Run a single search-replace: `polymarket_client_sdk::` → `polymarket_client_sdk_v2::` across `src/api/polymarket.rs`, `src/api/market_events.rs`, `src/api/rtds_tracker.rs`, `src/api/rtds_tracker_multi.rs`. **Avoid touching `*.rs.bak` files** (use `--include="*.rs" --exclude="*.rs.bak"` or per-file edits).
- [ ] Run `cargo check`. Expect zero or near-zero errors (the `Side`, `OrderType`, slug-shape, and `Config::http_client` differences are all already absorbed by existing code patterns).
- **Verification:** `cargo check` green; `grep -rn "polymarket_client_sdk\b" src/` returns zero hits (only `polymarket_client_sdk_v2`).
- **Commit msg draft:** `refactor(api): point SDK imports at polymarket_client_sdk_v2`

### Step 4 — Build verification per binary (15 min)

- [ ] `cargo build --release --bin multi_market_logger_hourly_fixed`
- [ ] `cargo build --release --bin multi_asset_logger_fixed`
- [ ] `cargo build --release --bin multi_asset_logger_fixed_rotation`
- [ ] `cargo build --release --bin multi_asset_logger_fixed_rotation_sbe`
- [ ] `cargo build --release --bin multi_asset_logger_fixed_rotation_sbe_obi`
- [ ] If any binary fails: the fix lands in `src/api/*.rs`, NOT in `src/bin/*.rs`. Re-derive the API shape until binaries compile unchanged.
- **Verification:** `git diff src/bin/` returns empty (no changes); all five `cargo build` invocations succeed.
- **Commit msg draft:** none if no API tweaks needed; otherwise `fix(api): preserve V1 surface for X (V2 type-shape diff Y)`

### Step 5 — Unit test smoke (5 min)

- [ ] `cargo test --lib` — runs the in-file `mod tests` blocks in `polymarket.rs` (`test_token_prices_new`, `test_connection_detail`, etc.).
- **Verification:** all tests pass.
- **Commit msg draft:** none.

### Step 6 — Live smoke test, V1 baseline vs V2 (1 hour + 30 min diff)

See §8 for the full byte-comparison protocol. Summary:

- [ ] Capture **V1 baseline**: from the `pre-v2-baseline` tag, run `multi_asset_logger_fixed_rotation_sbe_obi` against BTC 15m + ETH 15m for 1 hour. Save CSVs.
- [ ] Capture **V2 candidate**: from the migration branch, run the same binary with the same config window for 1 hour. Save CSVs.
- [ ] Run the diff harness (§8) to identify any column-level deltas.
- [ ] Verify: identical column names, identical column count (83), bounded numeric drift (timing-dependent fields acknowledged; price fields exact).
- **Verification:** diff report shows zero schema changes; numeric deltas within expected jitter (see §8).
- **Commit msg draft:** `chore(test): V2 smoke test artifacts + diff report`

### Step 7 — PR & review (async)

- [ ] Push branch, open PR titled `combine-fetcher V2 SDK upgrade`.
- [ ] Reference this plan and the poly-bot companion plan in the PR body.
- [ ] Attach the diff report from Step 6 to the PR.
- [ ] Request review.
- **Verification:** ≥1 approving review; CI green (if CI exists).
- **Commit msg draft:** PR description, not a commit.

### Step 8 — Merge & deploy (post-approval)

- [ ] Merge to `main` (or whichever branch the team uses for production).
- [ ] Restart loggers in production. Watch `logs/` for the first hour for any reconnection storms or empty CSVs.
- **Verification:** post-deploy logs show `[PRICE_STREAM] CONNECTED`, `[BOOK_STREAM] CONNECTED`, `[TRADE_STREAM] CONNECTED`, `[MARKET_EVENTS] Started new_market + market_resolved listeners`, RTDS first-update info lines, and CSVs growing at expected per-second rate.
- **Commit msg draft:** none (merge commit).

**Total budget:** ~2 hours of focused work + 1 hour of overnight live smoke for Step 6.

---

## 8. Byte-comparison validation

The hard guarantee from §1 is that V2 produces CSVs structurally identical to V1. Validate it with a direct comparison.

### Setup

1. Pick a quiet window (e.g. 02:00-03:00 UTC) on a low-volatility weekday so that artificial timing variance is minimized.
2. Use the same `--config` YAML for both runs (identical asset list, identical timeframes).
3. Use the same machine (rule out network/clock differences across hosts).

### Capture V1 baseline

```bash
git checkout pre-v2-baseline
cargo build --release --bin multi_asset_logger_fixed_rotation_sbe_obi
mkdir -p /tmp/v1_baseline
./target/release/multi_asset_logger_fixed_rotation_sbe_obi \
    --config ./multi_asset_logger.yml 3600 /tmp/v1_baseline 0
```

Wait 1 hour. Files land at `/tmp/v1_baseline/<slug>_YYYY-MM-DD_HH.csv`.

### Capture V2 candidate

In a fresh shell (don't reuse the V1 process — different start times will produce different rotations and confuse the diff):

```bash
git checkout feat/v2-sdk-upgrade
cargo build --release --bin multi_asset_logger_fixed_rotation_sbe_obi
mkdir -p /tmp/v2_candidate
./target/release/multi_asset_logger_fixed_rotation_sbe_obi \
    --config ./multi_asset_logger.yml 3600 /tmp/v2_candidate 0
```

### Diff protocol

The two runs are **not** synchronous (different windows on the wall clock), so a literal `diff` will show every row as different. The validation is structural + statistical:

```bash
# 1. Column-name and column-count check (must be identical)
head -1 /tmp/v1_baseline/btc-updown-15m-*_2026-04-29_02.csv > /tmp/v1_header.txt
head -1 /tmp/v2_candidate/btc-updown-15m-*_2026-04-29_03.csv > /tmp/v2_header.txt
diff /tmp/v1_header.txt /tmp/v2_header.txt
# Expected: empty diff. Both must declare the same 83 column names in the same order.

# 2. Column-count check (per-row)
awk -F, '{print NF}' /tmp/v1_baseline/btc-updown-15m-*_2026-04-29_02.csv | sort -u
awk -F, '{print NF}' /tmp/v2_candidate/btc-updown-15m-*_2026-04-29_03.csv | sort -u
# Expected: both print "83". Any other value is a schema break.

# 3. Format-specifier sanity (sample 100 rows, verify decimal-place patterns are unchanged)
shuf -n 100 /tmp/v1_baseline/btc-updown-15m-*_2026-04-29_02.csv > /tmp/v1_sample.csv
shuf -n 100 /tmp/v2_candidate/btc-updown-15m-*_2026-04-29_03.csv > /tmp/v2_sample.csv
# Spot-check column-by-column that {:.2}, {:.4}, etc. patterns match. A python+pandas
# script can automate this — count digits after the decimal in each column.

# 4. Statistical fidelity (price columns)
# Use a Python harness or DuckDB to compute:
#   - mean, p50, p95, p99 of yes_bid, yes_ask, no_bid, no_ask, binance_price
#   - count of NaN / 0.0 sentinel values per column
# Acceptance: counts within 5% (timing-dependent), means/medians within 0.01% (true price drift).

# 5. Reconnection log diff (no extra reconnects from V2 quirks)
grep -c "RECONNECTED" v1.log v2.log
# Expected: similar counts. Significant divergence (e.g. V2 reconnecting 5x more) is a red flag.
```

### Acceptance criteria

| Check | Pass condition | Action on fail |
|---|---|---|
| Header diff | empty | **Block merge.** Schema regression. |
| Column count | both = 83 | **Block merge.** |
| Format specifier sanity | all columns match V1 decimal patterns | **Block merge.** |
| Price column statistics | means within 0.01%, p99 within 0.5% | Investigate; likely benign but worth understanding. |
| NaN/0.0 sentinel counts | within 5% of V1 | Investigate timing-dependent drift. |
| Reconnect counts | within 2x of V1 | Investigate; likely a V2 backoff timing change. |

### Optional: parquet pipeline validation

`fix_and_slugify.py` converts CSVs to parquet for downstream consumption. Run the same script over both V1 and V2 outputs, then `parquet-tools schema` on each — schemas must match byte-for-byte at the column-list and dtype level.

---

## 9. Rollback plan

The rollback path is trivial because the change is mechanical and contained:

1. **Pre-merge fail (Steps 1-7):** delete the migration branch, no impact.
2. **Post-merge regression discovered within 24h:** `git revert <merge-sha>`, redeploy. Loggers come back up on V1 SDK with no data loss (the previous CSV files keep accumulating; new CSVs are produced under the same naming scheme).
3. **Post-merge regression discovered after the legacy `clob.polymarket.com` retires (months later):** revert is no longer fully valid since V1 SDK can't speak V2 protocol. Forward-fix in code; do not revert. (This is unlikely — the V1→V2 protocol auto-detection in V2 SDK means we never have a forced cutover for combine-fetcher.)

The `polymarket-client-sdk-patched/` directory should remain in the repo for **at least 30 days** post-merge to support clean reverts. Schedule its removal as a separate cleanup PR.

---

## 10. Out of scope

- **All goals from poly-bot's V2 plan that don't apply to a public-WS consumer:** auth, signing, EIP-712 domain, order submission, order timeouts, post_order retry, user-WS unsubscribe, USDC.e→pUSD wrap, CTF split/merge, fee logic, builder code, sub-penny pricing, geoblock host.
- **Adopting any new V2 features** (e.g. `client.fee_info`, `client.clob_market_info`, builder-code attribution, server-time-based auth headers). combine-fetcher doesn't sign anything.
- **Host flip to `clob-v2.polymarket.com`.** Defer; V2 SDK auto-detects via `GET /version`.
- **Re-tuning `Config::http_client`.** combine-fetcher uses `Config::default()` and isn't latency-sensitive on the REST fallback path.
- **CSV/parquet column additions** (e.g. `fee_rate_bps` from V2 trade messages). Schema is frozen at 83 columns.
- **Cleaning up `*.rs.bak` files.** Separate PR.
- **Cleaning up `polymarket-client-sdk-patched/` after rollback window.** Separate PR.
- **Migrating `signal-shm` sub-crate.** It doesn't import the SDK.
- **Migrating non-`*_fixed*` binaries** (`multi_market_logger_hourly`, `multi_asset_logger`, `dashboard`, `signal_engine`, etc.). They share the same `combine_fetcher::api::*` surface, so they'll recompile for free; but they're not on the production hot path and don't need explicit smoke-testing.

---

## 11. References

- **Companion plan (poly-bot, primary):** [`../../poly-bot/docs/v2_sdk_migration_plan.md`](../../poly-bot/docs/v2_sdk_migration_plan.md)
- **Companion TODO checklist:** [`../../poly-bot/docs/v2_sdk_migration_TODO.md`](../../poly-bot/docs/v2_sdk_migration_TODO.md)
- **poly-bot V2 audit (call-site catalog):** [`../../poly-bot/docs/v2_sdk_audit.md`](../../poly-bot/docs/v2_sdk_audit.md)
- **Patched V2 SDK source (consumed via `[patch.crates-io]`):** [`../../poly-bot/crates/polymarket-client-sdk-v2-patched/`](../../poly-bot/crates/polymarket-client-sdk-v2-patched/)
- **V2 SDK upstream reference doc:** [`../../polymarket-dev-docs/polymarket_v2_sdk_docs.md`](../../polymarket-dev-docs/polymarket_v2_sdk_docs.md)
- **CSV column reference (the schema we MUST preserve):** [`../../polymarket-dev-docs/logger_columns.md`](../../polymarket-dev-docs/logger_columns.md)
- **Affected combine-fetcher source (this repo):**
  - [`../src/api/polymarket.rs`](../src/api/polymarket.rs) — bulk of the SDK touch surface
  - [`../src/api/market_events.rs`](../src/api/market_events.rs) — already future-proofed for slug shape
  - [`../src/api/rtds_tracker.rs`](../src/api/rtds_tracker.rs) — RTDS single-asset
  - [`../src/api/rtds_tracker_multi.rs`](../src/api/rtds_tracker_multi.rs) — RTDS multi-asset
  - [`../src/api/gamma.rs`](../src/api/gamma.rs) — **no change** (hand-rolled reqwest)
  - [`../src/bin/*_fixed*.rs`](../src/bin/) — **NEVER touch**
- **SDK touch sites (12 total in 4 files):** confirmed via `grep -rnE "polymarket_client_sdk::|use polymarket_client_sdk\b" src/`
