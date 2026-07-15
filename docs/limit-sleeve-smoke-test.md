# Limit Sleeve — Live Smoke Test Runbook

Purpose: validate the limit-order sleeve end-to-end against **live Kraken** with
real but tiny funds, before trusting it with a real war chest. Every path in the
sleeve is network I/O and has only been logic-verified; this runbook is what turns
"logic-verified" into "validated."

**Guiding principle:** three of the four checks complete with **zero spend**. Only
the final stage commits real money, bounded to a single `ordermin`-sized bid that
you let fill naturally.

> **Already validated (public API, no credentials).** As of 2026-07-03, the two
> zero-auth parts of check 1 were confirmed live against Kraken for `ETHUSDC`:
> `AssetPairs` returns `tick_size "0.01"`, `pair_decimals 2`, `lot_decimals 8`,
> `ordermin "0.001"` (the `tick_size: null` fallback is **not** triggered), and
> `OHLC?interval=60` returns ~721 candles with VWAP at index 5 inside `[low, high]`.
> So `fetch_pair_spec` and the OHLC parse are known-good against real data. What
> remains and needs **your** Kraken credentials: the private dry-reconcile (Stage 0
> orders path), the signed `validate` (Stage 1), and the live fill (Stage 2).
>
> **Note on sizing:** `ordermin` for ETHUSDC is **0.001 ETH** — so one minimum bid is
> ≈ `0.001 × spot` (≈ $2–3 at recent prices). The dollar figures below are recomputed
> for that; the exact "one bid" chest is data-dependent, so **verify from the logs**
> (count the `placed bid` lines) rather than trusting a fixed number.

## The four things we're proving

1. `AssetPairs` parses for `ETHUSDC` — `tick_size` / `lot_decimals` / `ordermin`
   come back sane (and the `tick_size: null` → `10^-pair_decimals` fallback, if it
   triggers, produces a sane tick).
2. A post-only bid at a rounded HVN price is **accepted** by Kraken — not rejected
   as bad-tick or "post-only would cross." Highest-risk path; the whole reason
   `fetch_pair_spec` exists.
3. A fill round-trips — shows in `get_closed_sleeve_fills`, lands in Mongo **once**
   (dedup holds across reconciles), mirrors into the tagged Notion block dated by
   `closetm`.
4. The war chest reflects the fill on the next reconcile — `deployable` drops by the
   fill value.

---

## Prerequisites

- A funded live Kraken account with API keys that have **Query** + **Create/Modify
  Orders** permissions. Withdrawal permission is **not** needed and should be off.
- ~$30 of USDC on the account (only a fraction is ever committed).
- MongoDB reachable (`docker compose up -d`), same as the DCA bot.
- A `curl` + your API signing available for the one manual validate step (Stage 1),
  OR skip Stage 1 and accept a slightly higher risk at Stage 2 (not recommended).
- The account should ideally have **no other open ETHUSDC orders** during the test,
  so nothing is ambiguous when you eyeball the book.

Keep a terminal tailing the bot logs the whole time. Every check below is read off
the logs and confirmed against Kraken's UI + Mongo.

---

## Stage 0 — Read-only: compute path, zero orders

Goal: prove check (1) and the entire compute path — OHLC → profile → ladder → spot
→ tick/lot rounding — **without placing anything**.

The trick: set the war chest so low that every level sizes below `ordermin`, so
`size_bid` returns `None` for all of them. The sleeve computes the full ladder and
logs what it *would* do, then places nothing.

> **Sanity-check the trick for your numbers.** Stage 0 relies on $1 being below one
> `ordermin`-sized bid *at the nearest level's weight*. With `ordermin` = `0.001` ETH
> (confirmed live) and ETH ≈ $2–3k, one full min bid is ≈ $2–3, and the nearest
> level only gets its *weighted* share of the $1 chest — so $1 is safely sub-`ordermin`.
> Confirm the pass the right way: the logs must show **zero** `[sleeve] placed bid`
> lines. If you ever see one, lower `LIMIT_SLEEVE_WAR_CHEST_USDC` until none place.

### Config

```dotenv
EXCHANGE=kraken
KRAKEN_API_KEY=...
KRAKEN_SECRET_KEY=...

LIMIT_SLEEVE_ENABLED=true
LIMIT_SLEEVE_SYMBOL=ETHUSDC
# Deliberately tiny: forces every level below ordermin so nothing is placed.
LIMIT_SLEEVE_WAR_CHEST_USDC=1.0
LIMIT_SLEEVE_REFRESH_CRON=0 */5 * * * *   # every 5 min so you're not waiting on 6h
LIMIT_SLEEVE_INTERVAL_MINUTES=60
LIMIT_SLEEVE_MONGO_COLLECTION=limit_sleeve_smoke   # separate from any real collection

VP_BUCKET_SIZE_ETH=5.0
VP_HVN_THRESHOLD_RATIO=0.7
VP_LADDER_STEPS=4
VP_REQUIRE_LOCAL_MAXIMA=true
```

Use a **throwaway Mongo collection** (`limit_sleeve_smoke`) so nothing here pollutes
a real sleeve's fill history / war-chest derivation. You'll drop it at the end.

### Run & watch

Start the bot. On the startup reconcile you should see, in order:

- `[sleeve] enabled for ETH on ETHUSDC (war chest 1.0 USDC)`
- Kraken OHLC line: `Kraken OHLC for ETHUSDC (60m): N usable candles` — **N should be
  a few hundred**, not 0 and not a handful. Zero or tiny = OHLC parse problem.
- A current-price log from `get_price`.
- **No** `[sleeve] placed bid …` lines. The war chest is too small to fund any bid.

### Check 1 — `AssetPairs` parse

The sleeve calls `fetch_pair_spec` every reconcile. To see the values, either add a
temporary debug log in `fetch_pair_spec`, or hit the endpoint directly:

```bash
curl -s "https://api.kraken.com/0/public/AssetPairs?pair=ETHUSDC" \
  | jq '.result[] | {tick_size, pair_decimals, lot_decimals, ordermin}'
```

**Pass:** `tick_size` is a small positive decimal (confirmed `"0.01"`),
`lot_decimals` is a small int (confirmed `8`), `ordermin` is a sane base-asset
minimum (confirmed `"0.001"`). These were verified live on 2026-07-03; re-run only if
you suspect Kraken changed them.

**Watch the fallback:** if `tick_size` comes back `null`, the sleeve computes
`10^-pair_decimals`. Confirm `pair_decimals` is present and gives a sane tick
(`pair_decimals: 2` → tick `0.01`). If both are missing/zero, `fetch_pair_spec`
errors — stop and investigate before going further.

**Stage 0 passes when:** OHLC returns a healthy candle count, a ladder is computed,
`AssetPairs` parses sane, and **nothing is placed**. Only then proceed.

---

## Stage 1 — Kraken-side validation of a real bid, still zero spend

Goal: prove check (2) — a post-only bid at a rounded HVN price passes Kraken's full
tick/lot/ordermin/post-only validation — **without placing it**. Kraken's
`validate=true` on `AddOrder` runs the entire validation path and returns what would
happen, placing nothing.

`add_post_only_limit` doesn't pass `validate`, so this stage is a **one-off manual
`curl`**, not a code path. That's deliberate: it keeps the smoke test from requiring
a code change.

> **Decide before you start:** this stage needs a signed private request (the same
> HMAC-SHA512 signing the bot does). If you don't have signing tooling handy, the
> alternative is a tiny throwaway script reusing the bot's `sign` — a small code
> detour. Sort this out up front so Stage 1 isn't a surprise.

### Get a realistic price + volume to validate

From Stage 0's logs, take the **nearest-below-spot HVN price** the ladder produced,
tick-rounded (the sleeve logs desired levels; if you didn't log them, take spot from
the price line and pick a round tick a little below it). Pick a `volume` at or just
above `ordermin` — i.e. `0.001` ETH (or a touch more, e.g. `0.0015`).

### Validate against Kraken

Using your normal signed private-request tooling, POST to `/0/private/AddOrder`
with `validate=true`:

```
pair=ETHUSDC
type=buy
ordertype=limit
price=<tick-rounded HVN price below spot>
volume=0.001          # >= ordermin (confirmed 0.001 for ETHUSDC)
oflags=post
validate=true
```

**Pass:** `error` is `[]` and `result.descr.order` echoes your order (a human-readable
"buy 0.00100000 ETHUSDC @ limit <price> …"). Nothing appears on the book.

**Failure signatures:**
- `EOrder:Invalid price` / tick error → your tick rounding disagrees with Kraken's
  actual tick. Re-check Stage 0's `tick_size` and `round_price_to_tick`.
- `EOrder:Invalid volume` / below minimum → `ordermin` gating is off; your volume is
  under Kraken's real minimum.
- `EOrder:Post only order would fill` / would-cross → your "HVN below spot" price is
  actually at/above the best ask. Pick a price clearly below spot. (In production the
  ladder guarantees below-spot, so this only bites if you hand-pick a bad price here.)

**Stage 1 passes when:** a validate-only order at a rounded HVN price returns no
error. You've now proven the single highest-risk path spends nothing.

---

## Stage 2 — One real bid, natural fill (the only spend)

Goal: prove checks (3) and (4) with a single real, `ordermin`-sized bid that you
**let fill naturally** at its HVN — no forcing. This is realistic: it exercises the
exact path production uses, and it also proves the ClosedOrders backstop across
reconciles/restarts.

**Expectation-setting:** an HVN below spot may rest for **hours or days** before
price trades down to it. This stage is **resumable** — leave the sleeve running,
check back periodically. The dedup + derived-war-chest guarantees mean the fill is
recorded correctly whenever it lands, even if you weren't watching that reconcile.

### Config change

Raise the war chest just enough to fund **exactly one** bid at the nearest HVN, and
no more. This is data-dependent: the nearest level gets only its *weighted* share of
the chest, and each level must clear `ordermin` (0.001 ETH ≈ $2–3). A chest of ~$4–5
typically funds only the top level; **verify from the logs** — you want exactly one
`[sleeve] placed bid` line. If you see two, lower the chest and restart; if you see
zero, raise it slightly (the top level's weighted share is still under `ordermin`).

```dotenv
# Start here and adjust based on how many bids actually place (target: exactly 1).
LIMIT_SLEEVE_WAR_CHEST_USDC=5.0
```

Keep the throwaway `limit_sleeve_smoke` collection. Restart the bot.

### Confirm the bid is placed (real, on the book)

On the next reconcile:

- `[sleeve] placed bid 0.001 ETH @ <price> (txid <TXID>)` — **one** such line.
- **Verify on Kraken's UI:** a single open ETHUSDC buy order at that price, tagged
  with userref `770077` (`SLEEVE_USERREF`). Confirm it's **post-only** (maker).
- Confirm no *second* bid was placed (chest funds only one).

If placement is rejected here (but Stage 1 validated clean), capture the exact
`[sleeve] failed to place bid …` error — a divergence between `validate` and live
placement is the most interesting possible result and worth understanding before you
retry.

### Wait for the natural fill (resumable)

Leave it running. Across reconciles, while unfilled you should see the bid **kept**
(not churned) — no repeated cancel/replace of the same level, thanks to
`prices_match`. That's a passive confirmation the keep-logic works.

**You may stop and restart the bot during this window.** On restart, the startup
reconcile's `record_new_fills` scans ClosedOrders `start`-bounded from the newest
recorded fill; if the bid filled while the bot was down, it gets recorded then. This
is a bonus validation of the crash-safe backstop — worth doing deliberately at least
once: stop the bot, and if the fill happens while it's down, confirm the next start
records it.

### Check 3 — fill round-trips (when it fills)

Once price trades down and the bid fills, on the next reconcile confirm **all** of:

- Log: `[sleeve] recorded fill 0.001 ETH @ <price> (<value> USDC, txid <TXID>)`.
- **Mongo — recorded once:**
  ```
  # in mongosh, against the smoke collection
  db.limit_sleeve_smoke.find({ order_id: "<TXID>" }).count()   // must be 1
  ```
  Run a second reconcile (or restart) and re-check the count is **still 1** — this is
  the dedup guarantee (`get_purchase_by_order_id`) holding.
- **Notion:** the shared monthly page has a body block headed **"📈 Limit Sleeve
  Fill - …"** (not "DCA Purchase"), and it's filed under the month matching the
  fill's **`closetm`**, not the reconcile time. If the fill was near a month boundary
  and recorded late, this is the check that `closetm` threading works.

### Check 4 — war chest reflects the fill

On the reconcile *after* the fill is recorded:

- The `spent` figure the sleeve derives (`get_summary().total_usdc_invested`) now
  includes the fill value, so `deployable = war_chest − spent` drops by ~the fill's
  USDC value.
- With the chest at $7 and one ~$6 fill, `deployable` is now ~$1 — below `ordermin` —
  so the sleeve places **nothing further** and logs the cap path
  (`[sleeve] war chest cap reached …`). That's the derived-war-chest working end to
  end: the fill consumed the budget, and the sleeve correctly stops.

**Stage 2 passes when:** one bid placed, filled naturally, recorded exactly once in
Mongo, mirrored to a correctly-dated tagged Notion block, and the war chest dropped
by the fill value with no over-placement.

---

## Teardown / abort — pull everything, verify nothing rests

Do this to end the test, **or at any point** if something looks wrong. The danger of
walking away mid-test is a live bid resting at an HVN that fills hours later
unwatched — so always confirm the book is clean before you stop caring.

1. **Stop the bot** (Ctrl-C).
2. **Find any resting sleeve orders** (by userref, so you get *only* the sleeve's):
   ```bash
   # signed private request to /0/private/OpenOrders, then:
   jq '.result.open | to_entries[] | select(.value.userref == 770077) | .key'
   ```
   Or in Kraken's UI, cancel any open ETHUSDC order tagged `770077`.
3. **Cancel each** returned txid via `/0/private/CancelOrder` (or the UI's cancel).
4. **Verify empty:** re-run the OpenOrders check — no `770077` orders remain.
5. **Confirm Kraken UI** shows no open ETHUSDC orders you didn't expect.
6. **Drop the throwaway data** so it can never feed a real war-chest derivation:
   ```
   db.limit_sleeve_smoke.drop()
   ```
7. **Disable the sleeve** for normal running until you're ready for real:
   ```dotenv
   LIMIT_SLEEVE_ENABLED=false
   ```

Only after step 4 confirms nothing rests is it safe to walk away.

---

## Go / no-go

Promote to a real war chest **only if all four passed**:

- [ ] Stage 0: healthy OHLC candle count, ladder computed, `AssetPairs` sane, nothing placed.
- [ ] Stage 1: `validate=true` order at a rounded HVN price returned no error.
- [ ] Stage 2 / check 3: one bid, natural fill, recorded exactly once, tagged Notion block dated by `closetm`.
- [ ] Stage 2 / check 4: war chest dropped by the fill value; no over-placement.

Any failure → fix, re-run from the failed stage. When you move to a real chest, use a
**fresh, real** `LIMIT_SLEEVE_MONGO_COLLECTION` (not `limit_sleeve_smoke`), set a real
`LIMIT_SLEEVE_WAR_CHEST_USDC`, and restore the 6h refresh cron.
