//! Limit-order sleeve: rests post-only bids at volume-profile levels below spot,
//! funded by a fixed USDC war chest that drains as dips fill. Fully isolated from
//! the DCA core — own budget, own Mongo collection, tagged Notion body blocks — so
//! DCA stats stay pure. Kraken-only (reuses its validated post-only order path).
//!
//! The heart is [`LimitSleeve::reconcile`], run on a cron: it recomputes the ladder,
//! records any fills, and brings the resting orders in line with the fresh levels.
//!
//! ## Money-safety invariants (see the design memory)
//!
//! - **War chest is derived, never stored**: `remaining = war_chest − Σ(recorded
//!   fill values)`, recomputed each reconcile from the fills collection. It cannot
//!   drift from the fills and survives a crash mid-reconcile.
//! - **Fills are recorded idempotently** from Kraken's `ClosedOrders` (deduped by
//!   txid), which is the crash-safe source of truth — a fill on Kraken is never lost
//!   even if we crash before writing it; the next reconcile picks it up.
//! - **A partial fill is realised before its order is cancelled** (and the
//!   ClosedOrders scan is the backstop), so cancelling a partially-filled bid can
//!   never lose the bought quantity.
//! - **Never over-commit**: resting reservations + recorded fills ≤ war chest.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::LimitSleeveConfig;
use crate::dca_stats_mongo::{DcaPurchase, DcaStatsDB};
use crate::exchange::Exchange;
use crate::kraken::{ClosedSleeveFill, KrakenClient, OpenSleeveOrder, PairSpec};
use crate::levels::BidLevel;
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

/// Round a volume DOWN to the pair's lot precision (truncation, so we never size a
/// bid above the budget that produced it).
fn round_volume_to_lot(volume: Decimal, lot_decimals: u32) -> Decimal {
    volume.trunc_with_scale(lot_decimals)
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
    let volume = round_volume_to_lot(value / price, spec.lot_decimals);
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

/// The ideal, full-size bid for each ladder level given the deployable budget.
/// Used to decide which resting orders to keep vs cancel (by price). Placement
/// re-sizes against the *still-available* budget so kept reservations aren't
/// double-spent.
fn desired_bids(levels: &[BidLevel], deployable: Decimal, spec: &PairSpec) -> Vec<SizedBid> {
    levels
        .iter()
        .filter_map(|lvl| size_bid(lvl.price, deployable * lvl.weight, spec))
        .collect()
}

#[derive(Clone)]
pub struct LimitSleeve {
    kraken: KrakenClient,
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
    pub async fn new(kraken: KrakenClient, config: LimitSleeveConfig) -> Result<Self> {
        let fills_db = DcaStatsDB::with_collection(&config.mongo_collection).await?;
        Ok(Self {
            kraken,
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

    /// One reconcile pass: record fills, recompute the ladder, and align resting
    /// orders with fresh levels within the remaining war chest.
    pub async fn reconcile(&self) -> Result<()> {
        let symbol = self.config.symbol.clone();

        // 1. Fresh ladder + the spot it was derived against (reuse it; don't re-read).
        let (ladder, spot) = self
            .kraken
            .build_bid_ladder(
                &symbol,
                self.config.interval_minutes,
                &self.config.volume_profile,
            )
            .await?;
        let spec = self.kraken.fetch_pair_spec(&symbol).await?;

        // 2. Record any new fills FIRST, so the war-chest math sees them and partials
        //    from prior cancels are captured before we size anything.
        self.record_new_fills().await?;

        // 3. Derive the remaining war chest from recorded fills.
        let spent = self.fills_db.get_summary(spot).await?.total_usdc_invested;
        let deployable = self.config.war_chest_usdc - spent;

        let open = self
            .kraken
            .get_open_sleeve_orders(self.config.userref)
            .await?;

        if deployable <= Decimal::ZERO {
            info!(
                "[sleeve:{}] war chest cap reached (spent {:.2} / {:.2} USDC); cancelling {} resting order(s)",
                self.config.asset,
                spent,
                self.config.war_chest_usdc,
                open.len()
            );
            // Deliberate flatten (not a side effect): the war chest is a hard spend
            // cap, and every recorded fill has consumed it. Any still-resting bid, if
            // it filled, would spend *beyond* the cap — so cancel them all, realising
            // any partial first. The sleeve keeps computing levels next tick; it just
            // holds no bids until fresh budget appears (e.g. the cap is raised).
            for o in &open {
                self.realize_before_cancel(o).await;
                self.kraken.cancel_resting_order(&o.txid).await;
            }
            return Ok(());
        }

        // 4. Ideal bids for the fresh levels, and which resting orders to keep.
        let desired = desired_bids(&ladder.levels, deployable, &spec);

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
                self.kraken.cancel_resting_order(&o.txid).await;
            }
        }

        // 6. Re-read spent AFTER the cancel loop: `realize_before_cancel` may have
        //    recorded partial fills to Mongo, so the placement budget must reflect
        //    them (fills recorded before the war-chest recompute that placement reads).
        //    This closes a one-cycle over-commit where a just-realised partial wasn't
        //    yet subtracted. Sizing uses this fresh figure; keep/cancel decisions above
        //    used the pre-cancel one, which only affects marginal ordermin gating.
        let spent = self.fills_db.get_summary(spot).await?.total_usdc_invested;
        let deployable = (self.config.war_chest_usdc - spent).max(Decimal::ZERO);

        // 7. Place desired bids that aren't already resting, sized against what's left
        //    of the budget after fills + kept reservations. Never over-commits.
        let mut available = (deployable - kept_reserved).max(Decimal::ZERO);

        for lvl in &ladder.levels {
            let price = round_price_to_tick(lvl.price, spec.tick_size);
            let is_desired = desired
                .iter()
                .any(|d| prices_match(d.price, price, spec.tick_size));
            let already_resting = open
                .iter()
                .any(|o| prices_match(o.price, price, spec.tick_size));
            if !is_desired || already_resting {
                continue; // sub-ordermin at full size, or already resting (keep queue)
            }
            let target = deployable * lvl.weight;
            let capped = target.min(available);
            let Some(bid) = size_bid(lvl.price, capped, &spec) else {
                continue; // budget left can't fund an ordermin-sized bid here
            };
            if bid.value() > available {
                continue;
            }
            match self
                .kraken
                .place_resting_limit_buy(&symbol, bid.price, bid.volume, self.config.userref)
                .await
            {
                Ok(txid) => {
                    available -= bid.value();
                    info!(
                        "[sleeve:{}] placed bid {} @ {} (txid {})",
                        self.config.asset, bid.volume, bid.price, txid
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
        match self.kraken.query_resting_order(&o.txid).await {
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
            .kraken
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
            let eur = match self.kraken.get_usdc_per_eur().await {
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

    fn spec(tick: Decimal, lot_decimals: u32, ordermin: Decimal) -> PairSpec {
        PairSpec {
            tick_size: tick,
            lot_decimals,
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
        assert_eq!(round_volume_to_lot(dec!(0.123456789), 8), dec!(0.12345678));
        assert_eq!(round_volume_to_lot(dec!(1.9999), 2), dec!(1.99));
    }

    #[test]
    fn size_bid_applies_tick_lot_and_ordermin() {
        let s = spec(dec!(0.01), 8, dec!(0.002));
        // $30 at ~$3000.5 -> price floored to 3000.5? tick 0.01 keeps it; vol ~0.009998.
        let bid = size_bid(dec!(3000.5), dec!(30), &s).unwrap();
        assert_eq!(bid.price, dec!(3000.5));
        // 30 / 3000.5 = 0.0099983..., truncated to 8dp.
        assert_eq!(bid.volume, dec!(0.00999833));
    }

    #[test]
    fn size_bid_skips_below_ordermin() {
        let s = spec(dec!(0.01), 8, dec!(0.01));
        // $5 at $3000 -> ~0.00166 base, below ordermin 0.01 -> skipped.
        assert!(size_bid(dec!(3000), dec!(5), &s).is_none());
    }

    #[test]
    fn size_bid_skips_nonpositive_value() {
        let s = spec(dec!(0.01), 8, dec!(0.001));
        assert!(size_bid(dec!(3000), dec!(0), &s).is_none());
        assert!(size_bid(dec!(3000), dec!(-10), &s).is_none());
    }

    #[test]
    fn desired_bids_sizes_each_level_by_weight() {
        let s = spec(dec!(0.01), 8, dec!(0.001));
        let levels = vec![
            BidLevel {
                price: dec!(2900),
                weight: dec!(0.6),
                source_volume: dec!(100),
            },
            BidLevel {
                price: dec!(2800),
                weight: dec!(0.4),
                source_volume: dec!(80),
            },
        ];
        // Deploy $100: level 1 gets $60 @2900, level 2 gets $40 @2800.
        let bids = desired_bids(&levels, dec!(100), &s);
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0].price, dec!(2900));
        assert_eq!(
            bids[0].volume,
            round_volume_to_lot(dec!(60) / dec!(2900), 8)
        );
        assert_eq!(bids[1].price, dec!(2800));
        assert_eq!(
            bids[1].volume,
            round_volume_to_lot(dec!(40) / dec!(2800), 8)
        );
    }

    #[test]
    fn desired_bids_drops_sub_ordermin_levels() {
        // Large ordermin so a small per-level budget can't meet it -> empty ladder.
        let s = spec(dec!(0.01), 8, dec!(1)); // needs >= 1 whole unit per order
        let levels = vec![BidLevel {
            price: dec!(3000),
            weight: dec!(1),
            source_volume: dec!(100),
        }];
        // $100 at $3000 -> 0.033 units, below ordermin 1 -> dropped.
        assert!(desired_bids(&levels, dec!(100), &s).is_empty());
    }
}
