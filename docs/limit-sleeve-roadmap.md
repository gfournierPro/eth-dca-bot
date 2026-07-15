# Limit Sleeve (Order-Flow) — Roadmap

Status snapshot as of 2026-07-07. Covers the volume-profile resting-bid sleeve on
Kraken (`ETHUSDC`) layered on top of the DCA core. See also `docs/limit-sleeve-smoke-test.md`
for the live-validation runbook referenced below.

## Done

**Stage 0 — Patient-maker limit buys (DCA core, not the sleeve)**
- `Exchange::place_limit_buy` with a default fallback to market buy; `KrakenClient`
  overrides it with a real post-only/re-peg/partial-fill/timeout-fallback loop.
- Validated **live**: a real ~12 USDC ETH buy filled fully as maker (~52s, no
  fallback needed). Wired into the live DCA path.

**Stage 1 — Foundations**
- `levels.rs`: pure volume-profile computation (bucket → VPOC → HVN → weighted
  bid ladder). Fixed bucket width per asset, half-open bucket boundaries,
  exact-summing weights (residual on the farthest level), HVNs carry their own
  volume (no fragile price-equality lookup). 26 unit tests, no network.
- `LimitSleeveConfig` (+ embedded `VolumeProfileConfig`) in `config.rs`, fully
  env-driven (`LIMIT_SLEEVE_*`, `VP_*`), off by default, fails fast on bad values.
- Kraken order primitives: `place_resting_limit_buy`, `cancel_resting_order`,
  `query_resting_order`, `fetch_pair_spec` (tick/lot/ordermin), `get_open_sleeve_orders`,
  `get_closed_sleeve_fills` (paginated, userref-filtered).

**Stage 2 — Orchestration**
- `limit_sleeve.rs`: `LimitSleeve::reconcile()` — record fills → derive deployable
  war chest → cancel bids whose level moved (realize partial first) → re-read
  spent → place new bids capped by remaining budget.
- Wired into `main.rs` (`setup_limit_sleeve`): Kraken-only, skipped with a warning
  on any other backend; startup reconcile + recurring cron job.
- Money-safety invariants implemented and reasoned through: war chest **derived**
  from the fills collection (never stored), fills recorded **idempotently** by
  txid, partial fills **realized before cancel**, half-tick price tolerance to
  avoid churning kept orders, deliberate flatten-all when the chest is exhausted.
- Fills mirror into Mongo (own collection, isolated from DCA stats) and, optionally,
  into the shared Notion monthly page tagged "Limit Sleeve Fill".
- Review round landed: paginated/bounded `ClosedOrders` scan, half-tick
  `prices_match`, fills dated by Kraken `closetm` (not reconcile time), documented
  drained-budget flatten.
- **Public-API validation done, live**: `AssetPairs` for `ETHUSDC` confirmed
  (`tick_size 0.01`, `lot_decimals 8`, `ordermin 0.001`); `OHLC` confirmed (~721
  candles, VWAP inside `[low, high]`). Runbook (`docs/limit-sleeve-smoke-test.md`)
  written and gated.

**Stage 3 — BTC sleeve**
- Second sleeve slot (`Config::btc_limit_sleeve`) driven by `BTC_LIMIT_SLEEVE_*` /
  `BTC_VP_*` env vars (shared loader with the ETH sleeve), with
  `LimitSleeveConfig::btc_default()`: BTCUSDC, own `btc_limit_sleeve_fills`
  collection, $100 volume-profile buckets (BTC-scaled).
- **Per-sleeve `userref`** (ETH 770077, BTC 770078) moved from a hardcoded constant
  into `LimitSleeveConfig` — with a shared userref the two sleeves would see (and
  cancel) each other's bids and record each other's fills against their own war
  chest. `validate_config` fails fast if the sleeves ever share a userref or a
  Mongo collection.
- BTC sleeve fills mirror into the **BTC** Notion DB (`BTC_NOTION_*`, falling back
  to the BTC DCA workflow's config), tagged "Limit Sleeve Fill" like ETH's.
- `sleeve_smoke` takes `--asset eth|btc` on every command so the same staged
  runbook can be re-run against the BTC sleeve before funding it.

## Not done / open

**Gate — live authenticated smoke test (blocks funding beyond pocket change)**
- Stages 0–2 of the runbook (dry reconcile against a real account, signed
  `validate=true` check, one real ordermin-sized fill round-trip) have **not**
  been run. Nothing in the authenticated path — order placement, cancellation,
  fill dedup, war-chest decrement, Notion mirror — has touched live Kraken yet.
- This is the single highest-priority item before raising `LIMIT_SLEEVE_WAR_CHEST_USDC`
  above a token amount.

**Known gaps / backlog, roughly in order of relevance**
1. **Float precision leak in the war-chest hard cap.** `deployable = war_chest -
   spent` depends on `DcaStatsDB::get_summary`, which sums via MongoDB's
   `$toDouble` aggregation (`dca_stats_mongo.rs:124`) — an f64 round-trip baked
   into a Decimal-only codebase's hard spend cap. Harmless at USDC scale today,
   but worth a Decimal-native aggregation (or summing in Rust) if this pattern is
   ever reused somewhere precision-sensitive.
2. **Re-peg hysteresis carried over from the patient-maker loop** — was deferred
   there too. Not applicable to the sleeve's resting bids in the same way (they're
   meant to sit), but worth revisiting if reconcile cadence ever tightens below 6h.
3. **War-chest replenishment.** Currently a fixed chest that drains and stops —
   a deliberate choice, not a bug — but "sweep stale unfilled budget back to
   DCA" or a scheduled top-up were floated and never built. Decide only if the
   drain-and-stop behavior turns out to be the wrong shape in practice.
4. **No alerting beyond logs.** Notion mirror failures and reconcile errors are
   logged (`warn!`/`error!`) but nothing pages anyone. Acceptable for an
   unattended bot today; revisit if the war chest grows large enough that a
   silent failure matters.

## Suggested order of work

1. Run the live smoke test (`docs/limit-sleeve-smoke-test.md`, Stages 0–2) against
   a real Kraken account with a token war chest.
2. Fix the war-chest number in Stage 2 of the runbook if the corrected `ordermin`
   (0.001 ETH) hasn't already been re-verified there.
3. Once a real fill has round-tripped cleanly, raise the war chest to its intended
   size.
4. Repeat the runbook for the BTC sleeve (`sleeve_smoke ... --asset btc`) before
   funding its war chest.
5. Then, in whatever order matters to you: war-chest replenishment policy,
   Decimal-native war-chest aggregation.
