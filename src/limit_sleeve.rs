//! Limit-order sleeve: rests post-only bids at volume-profile levels below spot,
//! funded by a fixed USDC war chest that drains as dips fill. Fully isolated from
//! the DCA core — own budget, own Mongo collection, tagged Notion body blocks — so
//! DCA stats stay pure. Works against any [`SleeveExchange`] backend (Kraken, OKX;
//! Binance doesn't implement one).
//!
//! The heart is [`LimitSleeve::reconcile`], run on a cron: it recomputes the ladder,
//! records any fills, and brings the resting orders in line with the fresh levels.
//!
//! ## Money-safety invariants (see the design memory)
//!
//! - **War chest is derived, never stored**: `remaining = war_chest − Σ(recorded
//!   fill values)`, recomputed each reconcile from the fills collection. It cannot
//!   drift from the fills and survives a crash mid-reconcile.
//! - **Fills are recorded idempotently** from the exchange's closed-orders history
//!   (deduped by order id), which is the crash-safe source of truth — a fill is
//!   never lost even if we crash before writing it; the next reconcile picks it up.
//! - **A partial fill is realised before its order is cancelled** (the closed-orders
//!   scan is the backstop), so cancelling a partially-filled bid can never lose the
//!   bought quantity.
//! - **Never over-commit**: resting reservations + recorded fills ≤ war chest.

use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::{DrawdownConfig, LimitSleeveConfig};
use crate::dca_stats_mongo::{DcaPurchase, DcaStatsDB};
use crate::exchange::{ClosedSleeveFill, OpenSleeveOrder, PairSpec, SleeveExchange};
use crate::levels::{TierBid, anchor_high_and_rearm, derive_drawdown_ladder};
use crate::notion_integration::NotionDCATracker;

/// A concrete, exchange-valid bid: price rounded to tick, volume rounded to lot.
#[derive(Debug, Clone, PartialEq)]
pub struct SizedBid {
    pub price: Decimal,
    pub volume: Decimal,
}

impl SizedBid {
    /// USDC reserved by this bid.
    fn value(&self) -> Decimal {
        self.price * self.volume
    }
}

/// Round a price DOWN to a multiple of `tick`. Rounding down is conservative for a
/// buy bid: it rests at or below the intended level, never crossing upward.
fn round_price_to_tick(price: Decimal, tick: Decimal) -> Decimal {
    if tick <= Decimal::ZERO {
        return price;
    }
    (price / tick).floor() * tick
}

/// Round a volume DOWN to the pair's lot grid (floor division), so we never size a
/// bid above the budget that produced it.
fn round_volume_to_lot(volume: Decimal, lot_size: Decimal) -> Decimal {
    if lot_size <= Decimal::ZERO {
        return volume;
    }
    (volume / lot_size).floor() * lot_size
}

/// Turn a target USDC `value` at a raw HVN `price` into a valid, placeable bid, or
/// `None` if it can't be placed: non-positive price/value, or a rounded volume below
/// the pair's `ordermin` (a sub-minimum level is skipped, per the design decision).
fn size_bid(raw_price: Decimal, value: Decimal, spec: &PairSpec) -> Option<SizedBid> {
    if value <= Decimal::ZERO {
        return None;
    }
    let price = round_price_to_tick(raw_price, spec.tick_size);
    if price <= Decimal::ZERO {
        return None;
    }
    let volume = round_volume_to_lot(value / price, spec.lot_size);
    if volume <= Decimal::ZERO || volume < spec.ordermin {
        return None;
    }
    Some(SizedBid { price, volume })
}

/// Whether two prices refer to the same level, tolerant to within half a tick.
///
/// A resting order's price comes from a prior reconcile (and Kraken's `descr.price`
/// string), while the desired price is freshly tick-rounded. Exact `Decimal` equality
/// is scale-sensitive (`3000.50 != 3000.5`) and brittle to a tick that shifts by a
/// hair, which would cancel-and-replace a bid we meant to keep every cycle, defeating
/// the queue-priority intent. Half-a-tick tolerance matches genuinely-equal levels
/// while still separating adjacent ticks (HVN centers are dollars apart).
fn prices_match(a: Decimal, b: Decimal, tick: Decimal) -> bool {
    if tick <= Decimal::ZERO {
        return a == b;
    }
    (a - b).abs() * Decimal::TWO < tick
}

/// The ideal, full-size bid for each tier. Used to decide which resting orders to
/// keep vs cancel (by price). Placement re-sizes against the *still-available*
/// budget so kept reservations aren't double-spent.
///
/// Each tier is sized from its own allocation — never from a share of the whole
/// remaining chest — which is what keeps the deep tiers funded while shallow ones
/// fill. Contrast [`crate::levels::derive_bid_ladder`]'s normalised weights.
fn desired_bids(bids: &[TierBid], spec: &PairSpec) -> Vec<SizedBid> {
    bids.iter()
        .filter_map(|b| size_bid(b.price, b.value_usdc, spec))
        .collect()
}

/// Whole months elapsed from `start` to `now`, floored at zero.
///
/// Day-of-month is deliberately ignored: accrual ticks over on the 1st, so a chest
/// configured mid-month is not shortchanged a partial month, and the figure never
/// depends on what time of day a reconcile happens to run.
fn months_elapsed(start: NaiveDate, now: NaiveDate) -> i64 {
    let months = (now.year() as i64 - start.year() as i64) * 12
        + (now.month() as i64 - start.month() as i64);
    months.max(0)
}

/// USDC the sleeve may have committed in total right now: the starting chest plus
/// accrual to date, less everything already spent, clamped to `[0, cap]`.
///
/// Capping the *available* figure (rather than lifetime spend) is what lets the
/// sleeve revive in later years instead of flattening permanently once the original
/// war chest is gone, while still bounding exposure at any single moment.
fn accrued_chest(
    cfg: &DrawdownConfig,
    cap: Decimal,
    spent_all_time: Decimal,
    now: NaiveDate,
) -> Decimal {
    let accrued = cfg.starting_chest_usdc
        + cfg.monthly_accrual_usdc * Decimal::from(months_elapsed(cfg.accrual_start, now));
    (accrued - spent_all_time).clamp(Decimal::ZERO, cap)
}

#[derive(Clone)]
pub struct LimitSleeve {
    exchange: Arc<dyn SleeveExchange>,
    config: LimitSleeveConfig,
    /// Own Mongo collection (isolated from DCA). Reuses [`DcaStatsDB`]/[`DcaPurchase`]:
    /// a fill is just a purchase, and `total_usdc_invested` gives spent-so-far.
    fills_db: DcaStatsDB,
    /// Shared Notion DB, tagged "Limit Sleeve Fill" in the body block.
    notion: Option<NotionDCATracker>,
}

impl LimitSleeve {
    /// Build a sleeve over its own Mongo collection. Notion is attached separately
    /// via [`LimitSleeve::with_notion`] so the caller owns config resolution, exactly
    /// as `DcaTrader` does.
    pub async fn new(exchange: Arc<dyn SleeveExchange>, config: LimitSleeveConfig) -> Result<Self> {
        let fills_db = DcaStatsDB::with_collection(&config.mongo_collection).await?;
        Ok(Self {
            exchange,
            config,
            fills_db,
            notion: None,
        })
    }

    /// Attach an already-built Notion tracker; it is re-tagged "Limit Sleeve Fill"
    /// so its body blocks are distinct inside the shared monthly page. `None` leaves
    /// the sleeve recording to Mongo only.
    pub fn with_notion(mut self, notion: Option<NotionDCATracker>) -> Self {
        self.notion = notion.map(|n| n.with_fill_label("Limit Sleeve Fill"));
        self
    }

    /// One reconcile pass: record fills, recompute the drawdown tiers, and align
    /// resting orders with them inside the accrued chest.
    pub async fn reconcile(&self) -> Result<()> {
        let symbol = self.config.symbol.clone();
        let dd = &self.config.drawdown;

        // 1. Anchor high + the last time price set one, from daily candles, plus spot
        //    (only needed to avoid resting a post-only buy at or above the market).
        let daily = self.exchange.fetch_daily_highs(&symbol).await?;
        let (anchor_high, rearm_ts) = anchor_high_and_rearm(&daily, dd.anchor_days)?;
        let spot = self.exchange.fetch_spot_price(&symbol).await?;
        let spec = self.exchange.fetch_pair_spec(&symbol).await?;

        // 2. Record any new fills FIRST, so the chest math sees them and partials from
        //    prior cancels are captured before we size anything.
        self.record_new_fills().await?;

        // 3. Budget. Two different spend figures, for two different jobs:
        //    - `spent_all_time` bounds the chest (what's left of the accrued budget).
        //    - `spent_since_rearm` decides which tiers are still armed. Measuring from
        //      the last new anchor high is what stops a long bear from re-buying the
        //      same tier every cycle as the anchor decays.
        //    `total_usdc_invested` is a signed net cash-flow (negative for BUY fills,
        //    see dca_stats_mongo.rs) so it must be abs()'d to get a spend magnitude.
        let spent_all_time = self.fills_db.get_summary(spot).await?.total_usdc_invested.abs();
        let rearm_at = DateTime::<Utc>::from_timestamp(rearm_ts, 0).unwrap_or_else(Utc::now);
        let spent_since_rearm = self.fills_db.total_spent_since(rearm_at).await?;
        let deployable = accrued_chest(
            dd,
            self.config.war_chest_usdc,
            spent_all_time,
            Utc::now().date_naive(),
        );

        let tier_bids = derive_drawdown_ladder(anchor_high, spot, &dd.tiers, spent_since_rearm);
        // anchor_high is always positive (both backends drop non-positive highs).
        let drawdown_pct = (spot - anchor_high) / anchor_high * Decimal::ONE_HUNDRED;
        info!(
            "[sleeve:{}] anchor high {:.2} ({}d), spot {:.2} ({:+.1}%), armed since {}; \
             chest {:.2} of {:.2} (spent {:.2} lifetime / {:.2} since arming); {} tier(s) in range",
            self.config.asset,
            anchor_high,
            dd.anchor_days,
            spot,
            drawdown_pct,
            rearm_at.date_naive(),
            deployable,
            self.config.war_chest_usdc,
            spent_all_time,
            spent_since_rearm,
            tier_bids.len()
        );
        for b in &tier_bids {
            info!(
                "[sleeve:{}]   tier {} @ {:.2} (-{:.0}%) wants {:.2} USDC",
                self.config.asset,
                b.tier + 1,
                b.price,
                dd.tiers[b.tier].depth * Decimal::ONE_HUNDRED,
                b.value_usdc
            );
        }

        let open = self
            .exchange
            .get_open_sleeve_orders(self.config.userref)
            .await?;

        if deployable <= Decimal::ZERO {
            info!(
                "[sleeve:{}] chest exhausted (spent {:.2}, cap {:.2} USDC); cancelling {} resting order(s)",
                self.config.asset,
                spent_all_time,
                self.config.war_chest_usdc,
                open.len()
            );
            // Deliberate flatten (not a side effect): the chest is a hard spend cap and
            // every recorded fill has consumed it. Any still-resting bid, if it filled,
            // would spend *beyond* the cap — so cancel them all, realising any partial
            // first. Next month's accrual re-funds the chest and the tiers come back.
            for o in &open {
                self.realize_before_cancel(o).await;
                self.exchange.cancel_resting_order(&symbol, &o.txid).await;
            }
            return Ok(());
        }

        // 4. Ideal bids for the armed tiers, and which resting orders to keep.
        let desired = desired_bids(&tier_bids, &spec);

        // 5. Cancel resting orders whose level is gone/moved; sum the reservation held
        //    by the ones we keep so we don't double-spend their budget. Level matching
        //    is half-a-tick tolerant, so a bid we mean to keep isn't churned by a
        //    scale/tick hair (see `prices_match`).
        let mut kept_reserved = Decimal::ZERO;
        for o in &open {
            let still_desired = desired
                .iter()
                .any(|d| prices_match(d.price, o.price, spec.tick_size));
            if still_desired {
                // Unfilled remainder still reserves budget.
                kept_reserved += o.price * (o.volume - o.executed_qty).max(Decimal::ZERO);
            } else {
                self.realize_before_cancel(o).await;
                self.exchange.cancel_resting_order(&symbol, &o.txid).await;
            }
        }

        // 6. Re-read spent AFTER the cancel loop: `realize_before_cancel` may have
        //    recorded partial fills to Mongo, so the placement budget must reflect
        //    them (fills recorded before the chest recompute that placement reads).
        //    This closes a one-cycle over-commit where a just-realised partial wasn't
        //    yet subtracted. Sizing uses this fresh figure; keep/cancel decisions above
        //    used the pre-cancel one, which only affects marginal ordermin gating.
        let spent_all_time = self.fills_db.get_summary(spot).await?.total_usdc_invested.abs();
        let deployable = accrued_chest(
            dd,
            self.config.war_chest_usdc,
            spent_all_time,
            Utc::now().date_naive(),
        );

        // 7. Place desired bids that aren't already resting. Each is capped by its own
        //    tier allocation AND by what's left of the chest after fills + kept
        //    reservations, so the sleeve never over-commits — but a shallow tier can
        //    never reach past its allocation into a deeper tier's money either.
        let mut available = (deployable - kept_reserved).max(Decimal::ZERO);

        // Clamp to the USDC actually free on the account. A chest configured above
        // the funded balance otherwise places bids the venue rejects one by one
        // (seen live 2026-07-29: $100+$100 chests vs $103 on account → OKX "code 1"
        // on every bid past the balance). OKX's available balance already excludes
        // resting-order reservations; Kraken's Balance is a total, so the clamp is
        // looser there — either way never worse than no clamp. A failed balance
        // read degrades to unclamped (the venue still rejects, just noisily).
        match self.exchange.get_usdc_balance().await {
            Ok(free) if free < available => {
                warn!(
                    "[sleeve:{}] only {:.2} USDC free on the account but the chest wants {:.2} \
                     for new bids; clamping — fund the account or lower the chest/allocations",
                    self.config.asset, free, available
                );
                available = free.max(Decimal::ZERO);
            }
            Ok(_) => {}
            Err(e) => warn!(
                "[sleeve:{}] could not read free USDC balance ({}); placing without the \
                 funded-balance clamp",
                self.config.asset, e
            ),
        }

        for tb in &tier_bids {
            let price = round_price_to_tick(tb.price, spec.tick_size);
            let is_desired = desired
                .iter()
                .any(|d| prices_match(d.price, price, spec.tick_size));
            let already_resting = open
                .iter()
                .any(|o| prices_match(o.price, price, spec.tick_size));
            if !is_desired || already_resting {
                continue; // sub-ordermin at full size, or already resting (keep queue)
            }
            let capped = tb.value_usdc.min(available);
            let Some(bid) = size_bid(tb.price, capped, &spec) else {
                continue; // budget left can't fund an ordermin-sized bid here
            };
            if bid.value() > available {
                continue;
            }
            match self
                .exchange
                .place_resting_limit_buy(&symbol, bid.price, bid.volume, self.config.userref)
                .await
            {
                Ok(txid) => {
                    available -= bid.value();
                    info!(
                        "[sleeve:{}] placed bid {} @ {} (~{} USDC if filled) (txid {})",
                        self.config.asset,
                        bid.volume,
                        bid.price,
                        bid.value(),
                        txid
                    );
                }
                Err(e) => warn!(
                    "[sleeve:{}] failed to place bid {} @ {}: {}",
                    self.config.asset, bid.volume, bid.price, e
                ),
            }
        }

        Ok(())
    }

    /// Record a partial fill (if any) before cancelling an order, so the bought
    /// quantity can never be lost to the cancel. The ClosedOrders scan is the
    /// backstop; this just narrows the window and is deduped by txid.
    async fn realize_before_cancel(&self, o: &OpenSleeveOrder) {
        if o.executed_qty <= Decimal::ZERO {
            return;
        }
        match self
            .exchange
            .query_resting_order(&self.config.symbol, &o.txid)
            .await
        {
            Ok(Some(state)) if state.executed_qty > Decimal::ZERO => {
                // The order is still open (we're about to cancel it), so it has no
                // close time yet — the partial is realised now.
                if let Err(e) = self
                    .record_fill(
                        &o.txid,
                        state.executed_value,
                        state.executed_qty,
                        state.fee,
                        Utc::now(),
                    )
                    .await
                {
                    warn!(
                        "[sleeve:{}] failed to realise partial before cancel {}: {}",
                        self.config.asset, o.txid, e
                    );
                }
            }
            _ => {}
        }
    }

    /// Scan Kraken for sleeve fills that have left the book and record any not yet in
    /// Mongo (deduped by txid). This is the authoritative, crash-safe fill recorder.
    ///
    /// The scan is bounded by our newest recorded fill's time: every unrecorded fill
    /// is necessarily newer than that (fills are only ever recorded as a contiguous
    /// newest-prefix), and the Kraken call paginates the whole `[start, now]` window,
    /// so this never misses a fill regardless of how many orders closed in between.
    async fn record_new_fills(&self) -> Result<()> {
        // A small margin below the newest recorded fill guards against boundary
        // effects at the exact `start` second.
        let start = self
            .fills_db
            .latest_purchase_timestamp()
            .await?
            .map(|t| t.timestamp() - 300);

        let fills: Vec<ClosedSleeveFill> = self
            .exchange
            .get_closed_sleeve_fills(self.config.userref, start)
            .await?;
        for f in fills {
            let when = DateTime::<Utc>::from_timestamp(f.closetm, 0).unwrap_or_else(Utc::now);
            self.record_fill(&f.txid, f.executed_value, f.executed_qty, f.fee, when)
                .await?;
        }
        Ok(())
    }

    /// Idempotently record one fill to Mongo (and mirror to Notion). Returns without
    /// acting if the txid is already recorded, so it's safe to call from both the
    /// ClosedOrders scan and realise-before-cancel. `when` is the fill's actual close
    /// time, so month-bucketing in Notion is correct even when the ClosedOrders
    /// backstop records a fill hours later.
    async fn record_fill(
        &self,
        txid: &str,
        value_usdc: Decimal,
        qty: Decimal,
        fee_usdc: Decimal,
        when: DateTime<Utc>,
    ) -> Result<()> {
        if qty <= Decimal::ZERO {
            return Ok(());
        }
        if self
            .fills_db
            .get_purchase_by_order_id(txid)
            .await?
            .is_some()
        {
            return Ok(()); // already recorded
        }

        let avg_price = if qty > Decimal::ZERO {
            value_usdc / qty
        } else {
            Decimal::ZERO
        };
        let purchase = DcaPurchase {
            id: Uuid::new_v4().to_string(),
            timestamp: when,
            symbol: self.config.symbol.clone(),
            side: "BUY".to_string(),
            usdc_amount: value_usdc,
            eth_amount: qty,
            eth_price: avg_price,
            fees_usdc: fee_usdc,
            order_id: txid.to_string(),
            status: "FILLED".to_string(),
        };
        self.fills_db.record_purchase(&purchase).await?;
        info!(
            "[sleeve:{}] recorded fill {} @ {} ({:.2} USDC, txid {})",
            self.config.asset, qty, avg_price, value_usdc, txid
        );

        // Mirror to Notion (shared monthly page, tagged). Best-effort: a Notion
        // hiccup must not fail the reconcile or the Mongo record.
        if let Some(notion) = &self.notion {
            let eur = match self.exchange.get_usdc_per_eur().await {
                Ok(rate) if rate > Decimal::ZERO => value_usdc / rate,
                _ => value_usdc, // fall back to ~USDC if the rate is unavailable
            };
            if let Err(e) = notion.record_dca_purchase(&purchase, eur).await {
                warn!(
                    "[sleeve:{}] Notion mirror failed for {}: {}",
                    self.config.asset, txid, e
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn spec(tick: Decimal, lot_size: Decimal, ordermin: Decimal) -> PairSpec {
        PairSpec {
            tick_size: tick,
            lot_size,
            ordermin,
        }
    }

    #[test]
    fn prices_match_is_scale_insensitive_and_tick_tolerant() {
        let tick = dec!(0.01);
        // Scale difference only: 3000.50 vs 3000.5 — equal level, must match.
        assert!(prices_match(dec!(3000.50), dec!(3000.5), tick));
        // Within half a tick (0.005): a hair of drift still matches.
        assert!(prices_match(dec!(3000.502), dec!(3000.5), tick));
        // A full tick apart: genuinely different levels, must NOT match.
        assert!(!prices_match(dec!(3000.51), dec!(3000.5), tick));
        // Far apart (HVN centers are dollars apart): must not match.
        assert!(!prices_match(dec!(2995), dec!(3000), tick));
    }

    #[test]
    fn price_rounds_down_to_tick() {
        // HVN centers are bucket midpoints, so half-values are common.
        assert_eq!(
            round_price_to_tick(dec!(2999.567), dec!(0.01)),
            dec!(2999.56)
        );
        assert_eq!(round_price_to_tick(dec!(2997.5), dec!(0.1)), dec!(2997.5));
        assert_eq!(round_price_to_tick(dec!(3002.5), dec!(1)), dec!(3002));
        // Non-positive tick is a no-op (defensive).
        assert_eq!(round_price_to_tick(dec!(100), dec!(0)), dec!(100));
    }

    #[test]
    fn volume_truncates_to_lot_precision() {
        assert_eq!(
            round_volume_to_lot(dec!(0.123456789), dec!(0.00000001)),
            dec!(0.12345678)
        );
        assert_eq!(round_volume_to_lot(dec!(1.9999), dec!(0.01)), dec!(1.99));
        // Non-positive lot size is a no-op (defensive).
        assert_eq!(round_volume_to_lot(dec!(1.5), dec!(0)), dec!(1.5));
    }

    #[test]
    fn size_bid_applies_tick_lot_and_ordermin() {
        let s = spec(dec!(0.01), dec!(0.00000001), dec!(0.002));
        // $30 at ~$3000.5 -> price floored to 3000.5? tick 0.01 keeps it; vol ~0.009998.
        let bid = size_bid(dec!(3000.5), dec!(30), &s).unwrap();
        assert_eq!(bid.price, dec!(3000.5));
        // 30 / 3000.5 = 0.0099983..., truncated to 8dp.
        assert_eq!(bid.volume, dec!(0.00999833));
    }

    #[test]
    fn size_bid_skips_below_ordermin() {
        let s = spec(dec!(0.01), dec!(0.00000001), dec!(0.01));
        // $5 at $3000 -> ~0.00166 base, below ordermin 0.01 -> skipped.
        assert!(size_bid(dec!(3000), dec!(5), &s).is_none());
    }

    #[test]
    fn size_bid_skips_nonpositive_value() {
        let s = spec(dec!(0.01), dec!(0.00000001), dec!(0.001));
        assert!(size_bid(dec!(3000), dec!(0), &s).is_none());
        assert!(size_bid(dec!(3000), dec!(-10), &s).is_none());
    }

    fn tier_bid(tier: usize, price: Decimal, value_usdc: Decimal) -> TierBid {
        TierBid {
            tier,
            price,
            value_usdc,
        }
    }

    #[test]
    fn desired_bids_sizes_each_tier_from_its_own_allocation() {
        let s = spec(dec!(0.01), dec!(0.00000001), dec!(0.001));
        let bids = desired_bids(
            &[
                tier_bid(0, dec!(2900), dec!(60)),
                tier_bid(1, dec!(2800), dec!(40)),
            ],
            &s,
        );
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0].price, dec!(2900));
        assert_eq!(
            bids[0].volume,
            round_volume_to_lot(dec!(60) / dec!(2900), dec!(0.00000001))
        );
        assert_eq!(bids[1].price, dec!(2800));
        assert_eq!(
            bids[1].volume,
            round_volume_to_lot(dec!(40) / dec!(2800), dec!(0.00000001))
        );
    }

    #[test]
    fn desired_bids_drops_sub_ordermin_tiers() {
        // Large ordermin so a small allocation can't meet it -> tier dropped.
        let s = spec(dec!(0.01), dec!(0.00000001), dec!(1)); // needs >= 1 whole unit per order
        // $100 at $3000 -> 0.033 units, below ordermin 1 -> dropped.
        assert!(desired_bids(&[tier_bid(0, dec!(3000), dec!(100))], &s).is_empty());
    }

    /// The BTC production accrual: $500 start, $83.33/mo, $1000 cap.
    fn dd(start: NaiveDate) -> DrawdownConfig {
        DrawdownConfig {
            anchor_days: 90,
            tiers: vec![],
            monthly_accrual_usdc: dec!(83.33),
            starting_chest_usdc: dec!(500),
            accrual_start: start,
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn months_elapsed_counts_calendar_months_and_never_goes_negative() {
        let start = date(2026, 8, 1);
        assert_eq!(months_elapsed(start, date(2026, 8, 31)), 0);
        assert_eq!(months_elapsed(start, date(2026, 9, 1)), 1);
        assert_eq!(months_elapsed(start, date(2027, 8, 1)), 12);
        // A clock before the accrual start must not produce negative accrual.
        assert_eq!(months_elapsed(start, date(2026, 1, 1)), 0);
    }

    #[test]
    fn chest_accrues_monthly_and_stops_at_the_cap() {
        let cfg = dd(date(2026, 8, 1));
        let cap = dec!(1000);
        // Month 0: just the starting chest.
        assert_eq!(accrued_chest(&cfg, cap, dec!(0), date(2026, 8, 15)), dec!(500));
        // Month 3: 500 + 3*83.33.
        assert_eq!(
            accrued_chest(&cfg, cap, dec!(0), date(2026, 11, 1)),
            dec!(749.99)
        );
        // Far future: clamped to the cap, never above it.
        assert_eq!(accrued_chest(&cfg, cap, dec!(0), date(2030, 1, 1)), cap);
    }

    #[test]
    fn spending_reduces_the_chest_and_it_cannot_go_negative() {
        let cfg = dd(date(2026, 8, 1));
        let cap = dec!(1000);
        // $200 spent out of a $500 chest.
        assert_eq!(
            accrued_chest(&cfg, cap, dec!(200), date(2026, 8, 15)),
            dec!(300)
        );
        // Overspent (shouldn't happen, but must clamp rather than wrap negative —
        // a negative "deployable" would flip the exhausted-chest branch's meaning).
        assert_eq!(
            accrued_chest(&cfg, cap, dec!(9999), date(2026, 8, 15)),
            Decimal::ZERO
        );
    }

    #[test]
    fn accrual_revives_the_chest_after_a_full_drawdown_spend() {
        // The whole point of accrual over a static pot: spend it all in month 1,
        // and the sleeve is funded again later instead of flattened forever.
        let cfg = dd(date(2026, 8, 1));
        let cap = dec!(1000);
        assert_eq!(
            accrued_chest(&cfg, cap, dec!(500), date(2026, 8, 20)),
            Decimal::ZERO,
            "fully spent in month 0"
        );
        assert_eq!(
            accrued_chest(&cfg, cap, dec!(500), date(2026, 11, 1)),
            dec!(249.99),
            "three months of accrual has re-funded it"
        );
    }
}
