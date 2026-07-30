//! Volume Profile level computation for the limit-order sleeve.
//!
//! This module is **pure computation**: it takes executed-volume observations
//! `(price, volume)` in and returns a [`VolumeProfile`] and a [`BidLadder`] out.
//! It does not place orders, hold an exchange client, or touch Mongo/Notion —
//! that orchestration lives in the (separate) sleeve module. Keeping this
//! boundary clean means the level math is independently unit-testable with
//! synthetic data and no network (see the `tests` module below).
//!
//! The premise: price levels that have historically traded the most volume act
//! as "fair value magnets" and support zones. Resting a buy bid at a High Volume
//! Node (HVN) below spot is a disciplined way to accumulate on dips at
//! statistically meaningful prices, without the second-to-second timing
//! decisions that would break DCA.
//!
//! ## Locked design decisions (see the implementation brief §9)
//!
//! 1. **Fixed `bucket_size`** (price width per asset), not a fixed bucket count —
//!    ETH and BTC have very different absolute prices, so a fixed count would
//!    give inconsistent resolution.
//! 2. **Price attribution is the caller's job.** This module takes
//!    `(price, volume)` pairs; the caller decides whether a kline contributes its
//!    close, typical price, or per-trade prices. Nothing is fetched here.
//! 3. **HVN = threshold + local maxima.** A bucket is an HVN if its volume is at
//!    least `hvn_threshold_ratio` of the VPOC volume *and* (when
//!    `require_local_maxima`) strictly exceeds both immediate neighbours. This
//!    yields distinct levels instead of a solid block of adjacent buckets.
//! 4. **Ladder weighting is volume-weighted:** stronger nodes receive a larger
//!    share of the (sleeve-owned) budget. Weights are normalised to sum to 1 and
//!    this module never sees the budget itself.
//! 5. **Empty input is caller misuse → `Err`.** Valid data with no qualifying HVN
//!    below spot is a normal outcome → `Ok` with an empty ladder.
//!
//! Bucket boundaries are **half-open `[low, high)`**, except the final bucket,
//! which is inclusive of `max_price` so the maximum observation always lands
//! somewhere.

use anyhow::{Result, anyhow};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};

/// Upper bound on bucket count, to fail fast on a mis-configured `bucket_size`
/// rather than attempt a huge allocation.
const MAX_BUCKETS: usize = 5_000_000;

/// Tunables for volume-profile computation. Populated per asset by the caller
/// (ultimately from env in `config.rs`), never hardcoded here.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VolumeProfileConfig {
    /// Price width of a single bucket, in quote currency (per asset).
    pub bucket_size: Decimal,
    /// A bucket is a High Volume Node if its volume is at least this fraction of
    /// the VPOC bucket's volume, e.g. `0.7`.
    pub hvn_threshold_ratio: Decimal,
    /// Maximum number of bids in the derived ladder.
    pub ladder_steps: usize,
    /// When true, an HVN must also be a strict local maximum (volume greater than
    /// both immediate neighbours) to be reported as a distinct level.
    pub require_local_maxima: bool,
}

/// One price bucket of the volume profile. Boundaries are half-open `[low, high)`.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeBucket {
    pub price_low: Decimal,
    pub price_high: Decimal,
    pub center: Decimal,
    pub volume: Decimal,
}

/// A High Volume Node: an HVN bucket's representative price plus its volume.
///
/// Carrying the volume alongside the price means the ladder derivation never has
/// to search back into `buckets` by float-ish `Decimal` equality on a computed
/// center — the volume it needs for weighting travels with the level.
#[derive(Debug, Clone, PartialEq)]
pub struct Hvn {
    /// Representative (center) price of the HVN bucket.
    pub price: Decimal,
    /// Total volume accumulated in the HVN bucket.
    pub volume: Decimal,
}

/// The computed volume profile: every bucket plus the extracted VPOC and HVNs.
#[derive(Debug, Clone)]
pub struct VolumeProfile {
    /// All buckets, ascending by price. Empty buckets (zero volume) are retained
    /// so neighbour comparisons for local maxima are meaningful.
    pub buckets: Vec<VolumeBucket>,
    /// Representative (center) price of the highest-volume bucket.
    pub vpoc: Decimal,
    /// The HVN buckets (price + volume), ascending by price.
    pub hvns: Vec<Hvn>,
}

/// A single resting bid derived from an HVN below spot.
#[derive(Debug, Clone, PartialEq)]
pub struct BidLevel {
    pub price: Decimal,
    /// Normalised budget weight; the weights across a [`BidLadder`] sum to 1.
    pub weight: Decimal,
    /// Volume of the originating HVN bucket (context; not a budget figure).
    pub source_volume: Decimal,
}

/// The ladder of resting bids, nearest-below-spot first. May be empty.
#[derive(Debug, Clone, Default)]
pub struct BidLadder {
    pub levels: Vec<BidLevel>,
}

/// Compute the volume profile from executed-volume observations.
///
/// `observations` is a slice of `(price, volume)` pairs over the lookback window.
/// Returns `Err` when the input is empty (caller misuse) or the config is invalid
/// (`bucket_size <= 0`, or a non-finite price range).
pub fn compute_volume_profile(
    observations: &[(Decimal, Decimal)],
    config: &VolumeProfileConfig,
) -> Result<VolumeProfile> {
    if observations.is_empty() {
        return Err(anyhow!(
            "cannot compute volume profile from empty observations"
        ));
    }
    if config.bucket_size <= Decimal::ZERO {
        return Err(anyhow!(
            "bucket_size must be positive, got {}",
            config.bucket_size
        ));
    }

    // Price range across all observations.
    let mut min_price = observations[0].0;
    let mut max_price = observations[0].0;
    for (price, _) in observations {
        if *price < min_price {
            min_price = *price;
        }
        if *price > max_price {
            max_price = *price;
        }
    }

    // Fixed-width bucketing. num_buckets guarantees the max observation lands in
    // the final bucket; a zero-width range (all prices equal) yields one bucket.
    let span = max_price - min_price;
    let num_buckets = (span / config.bucket_size)
        .floor()
        .to_usize()
        .ok_or_else(|| anyhow!("price range too large for bucket_size"))?
        + 1;

    // Guard against a mis-set bucket_size (env typos happen: config is env-driven).
    // e.g. a tiny bucket_size against a wide range would otherwise allocate an
    // enormous number of buckets before anything downstream failed.
    if num_buckets > MAX_BUCKETS {
        return Err(anyhow!(
            "bucket_size {} over range [{}, {}] yields {} buckets (max {}); \
             pick a larger bucket_size",
            config.bucket_size,
            min_price,
            max_price,
            num_buckets,
            MAX_BUCKETS
        ));
    }

    let mut buckets: Vec<VolumeBucket> = (0..num_buckets)
        .map(|i| {
            let idx = Decimal::from(i);
            let price_low = min_price + idx * config.bucket_size;
            let price_high = price_low + config.bucket_size;
            VolumeBucket {
                price_low,
                price_high,
                center: (price_low + price_high) / Decimal::TWO,
                volume: Decimal::ZERO,
            }
        })
        .collect();

    // Accumulate volume. Half-open [low, high); the max observation clamps into
    // the final bucket.
    for (price, volume) in observations {
        let raw = ((*price - min_price) / config.bucket_size)
            .floor()
            .to_usize()
            .ok_or_else(|| anyhow!("bucket index out of range for price {}", price))?;
        let idx = raw.min(num_buckets - 1);
        buckets[idx].volume += *volume;
    }

    // VPOC: highest-volume bucket; ties resolve to the lowest price. Buckets are
    // ascending by price, so keeping the first strict maximum (update only on
    // strictly greater volume) yields the lowest-priced bucket among any ties.
    // (`Iterator::max_by` would return the *last* max, i.e. the highest price.)
    let mut vpoc_idx = 0;
    for i in 1..buckets.len() {
        if buckets[i].volume > buckets[vpoc_idx].volume {
            vpoc_idx = i;
        }
    }
    let vpoc = buckets[vpoc_idx].center;
    let vpoc_volume = buckets[vpoc_idx].volume;

    // HVN threshold as a fraction of VPOC volume.
    let threshold = vpoc_volume * config.hvn_threshold_ratio;

    let mut hvns = Vec::new();
    for i in 0..buckets.len() {
        let vol = buckets[i].volume;
        if vol <= Decimal::ZERO || vol < threshold {
            continue;
        }
        if config.require_local_maxima {
            let higher_than_left = i == 0 || vol > buckets[i - 1].volume;
            let higher_than_right = i + 1 == buckets.len() || vol > buckets[i + 1].volume;
            if !(higher_than_left && higher_than_right) {
                continue;
            }
        }
        hvns.push(Hvn {
            price: buckets[i].center,
            volume: vol,
        });
    }

    Ok(VolumeProfile {
        buckets,
        vpoc,
        hvns,
    })
}

/// Derive the bid ladder from a profile and the current spot price.
///
/// Only HVNs strictly below `current_price` become bids (we only rest *buy*
/// orders below spot). They are ordered nearest-below-spot first, capped at
/// `ladder_steps`, and volume-weighted with weights normalised to sum to 1.
///
/// The returned weights sum to **exactly** 1: each is `volume / total_volume`
/// except the last (farthest-below-spot) level, which receives the residual
/// `1 - Σ(previous)`. This matters because `Decimal` division of non-terminating
/// ratios (e.g. three equal nodes → `1/3` each) truncates and would otherwise
/// leave a sub-cent crumb of the sleeve budget unallocated.
///
/// Returns `Ok` with an empty ladder when no HVN qualifies below spot.
pub fn derive_bid_ladder(
    profile: &VolumeProfile,
    current_price: Decimal,
    config: &VolumeProfileConfig,
) -> Result<BidLadder> {
    // HVNs strictly below spot, nearest-first. Volume travels with each level, so
    // no lookup back into `buckets` is needed.
    let mut candidates: Vec<&Hvn> = profile
        .hvns
        .iter()
        .filter(|h| h.price < current_price)
        .collect();
    candidates.sort_by(|a, b| b.price.cmp(&a.price)); // descending: nearest first
    candidates.truncate(config.ladder_steps);

    if candidates.is_empty() {
        return Ok(BidLadder::default());
    }

    let n = candidates.len();
    let total_volume: Decimal = candidates.iter().map(|h| h.volume).sum();

    // Volume-weighted, with the last level taking the residual so the weights sum
    // to exactly 1. Every HVN has positive volume, so `total_volume > 0` always;
    // the equal-split branch only guards a degenerate (all-zero) profile.
    let mut assigned = Decimal::ZERO;
    let mut levels = Vec::with_capacity(n);
    for (i, hvn) in candidates.into_iter().enumerate() {
        let weight = if i + 1 == n {
            Decimal::ONE - assigned
        } else if total_volume > Decimal::ZERO {
            hvn.volume / total_volume
        } else {
            Decimal::ONE / Decimal::from(n)
        };
        assigned += weight;
        levels.push(BidLevel {
            price: hvn.price,
            weight,
            source_volume: hvn.volume,
        });
    }

    Ok(BidLadder { levels })
}

// --- Drawdown-tier ladder -----------------------------------------------------
//
// The volume-profile ladder above answers "where did volume trade recently?",
// which turns out to be a mean-reversion question: its levels track spot and sit
// 1-2% below it, so a routine wiggle fills the whole war chest and nothing is left
// for a real drawdown. The tier ladder below answers a different question — "how
// far below the cycle high are we?" — and is what the sleeve actually places.
//
// Two properties do the work, and both are load-bearing:
//
// 1. **Levels are anchored to a rolling high, not to spot.** They do not follow
//    price down, so shallow noise cannot reach the deep tiers.
// 2. **Allocations are absolute USDC, never normalised.** `derive_bid_ladder`
//    returns weights summing to 1, which by construction commits 100% of the
//    remaining chest to whatever levels are currently in range — the exact bug
//    that drains the chest early. A tier gets its dollars and nothing more.

/// One drawdown tier: how far below the anchor high it rests, and the fixed USDC
/// it may ever deploy per arming.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DrawdownTier {
    /// Fractional drawdown below the anchor high, e.g. `0.25` for -25%.
    pub depth: Decimal,
    /// Fixed USDC this tier deploys. Not a weight: it is never re-normalised
    /// against the other tiers or the remaining chest.
    pub allocation_usdc: Decimal,
}

/// A tier's concrete resting bid: trigger price plus the USDC it still has to
/// spend (its allocation less whatever already filled since the last arming).
#[derive(Debug, Clone, PartialEq)]
pub struct TierBid {
    /// Index into the configured tier list, for logging.
    pub tier: usize,
    /// Trigger price: `anchor_high * (1 - depth)`.
    pub price: Decimal,
    /// USDC still unspent for this tier.
    pub value_usdc: Decimal,
}

/// The anchor high and the arming time, from a daily `(timestamp, high)` series.
///
/// Returns `(anchor_high, rearm_ts)` where `anchor_high` is the maximum high over
/// the trailing `anchor_days`, and `rearm_ts` is the timestamp of the most recent
/// candle that set a new `anchor_days` high — the moment every tier re-armed.
///
/// Spend is measured from `rearm_ts`, which is what stops a bear market from
/// re-buying the same tier forever: the tiers only reset once price has recovered
/// to a fresh high. Falls back to the oldest candle when no new high occurs in the
/// series (a long bear), so tiers stay disarmed rather than silently re-arming.
///
/// `Err` on empty input (caller misuse) or a non-positive `anchor_days`.
pub fn anchor_high_and_rearm(
    daily: &[(i64, Decimal)],
    anchor_days: usize,
) -> Result<(Decimal, i64)> {
    if daily.is_empty() {
        return Err(anyhow!("cannot derive a drawdown anchor from no candles"));
    }
    if anchor_days == 0 {
        return Err(anyhow!("anchor_days must be greater than 0"));
    }

    // Anchor: highest high in the trailing window (the whole series if shorter).
    let window_start = daily.len().saturating_sub(anchor_days);
    let anchor_high = daily[window_start..]
        .iter()
        .map(|(_, h)| *h)
        .max()
        .expect("non-empty by the guard above");

    // Most recent candle whose high matched or exceeded every high in the
    // `anchor_days` before it. Candles earlier than a full window can't be judged
    // against one, so they're skipped; the fallback covers that case.
    let mut rearm_ts = daily[0].0;
    for i in anchor_days..daily.len() {
        let prior_max = daily[i - anchor_days..i]
            .iter()
            .map(|(_, h)| *h)
            .max()
            .expect("non-empty range");
        if daily[i].1 >= prior_max {
            rearm_ts = daily[i].0;
        }
    }

    Ok((anchor_high, rearm_ts))
}

/// Derive the resting bids for the drawdown tiers.
///
/// `spent_since_rearm` is absorbed **shallowest tier first**, which is the correct
/// attribution without storing per-tier state anywhere: price cannot reach tier 2's
/// trigger without first passing tier 1's, so any spend since the last arming
/// necessarily filled the shallower tiers first. A tier whose allocation is only
/// partly consumed (a partial fill) keeps the remainder and re-bids for it.
///
/// Only tiers with unspent allocation **and** a trigger strictly below `spot` are
/// returned — a post-only buy at or above spot would be rejected by the exchange.
/// An empty result is a normal outcome (price near its high, or all tiers spent).
pub fn derive_drawdown_ladder(
    anchor_high: Decimal,
    spot: Decimal,
    tiers: &[DrawdownTier],
    spent_since_rearm: Decimal,
) -> Vec<TierBid> {
    let mut unabsorbed = spent_since_rearm.max(Decimal::ZERO);
    let mut bids = Vec::new();

    for (i, tier) in tiers.iter().enumerate() {
        let alloc = tier.allocation_usdc;
        // Consume this tier's allocation from the spend still to be accounted for.
        let consumed = unabsorbed.min(alloc);
        unabsorbed -= consumed;
        let remaining = alloc - consumed;

        let price = anchor_high * (Decimal::ONE - tier.depth);
        if remaining > Decimal::ZERO && price > Decimal::ZERO && price < spot {
            bids.push(TierBid {
                tier: i,
                price,
                value_usdc: remaining,
            });
        }
    }

    bids
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn cfg(require_local_maxima: bool) -> VolumeProfileConfig {
        VolumeProfileConfig {
            bucket_size: dec!(10),
            hvn_threshold_ratio: dec!(0.7),
            ladder_steps: 4,
            require_local_maxima,
        }
    }

    /// Sum of all ladder weights, for normalisation assertions.
    fn weight_sum(ladder: &BidLadder) -> Decimal {
        ladder.levels.iter().map(|l| l.weight).sum()
    }

    /// HVN prices only, for asserting level extraction independent of volume.
    fn hvn_prices(profile: &VolumeProfile) -> Vec<Decimal> {
        profile.hvns.iter().map(|h| h.price).collect()
    }

    #[test]
    fn vpoc_is_the_highest_volume_bucket() {
        // min_price anchors the buckets at 100, so bucket i covers [100+10i, 110+10i)
        // with center 105+10i. Peak volume (80) lands in [120,130), center 125.
        let obs = vec![
            (dec!(100), dec!(1)),
            (dec!(105), dec!(1)),
            (dec!(125), dec!(50)),
            (dec!(126), dec!(30)),
            (dec!(145), dec!(5)),
        ];
        let profile = compute_volume_profile(&obs, &cfg(false)).unwrap();
        assert_eq!(profile.vpoc, dec!(125));
    }

    #[test]
    fn hvn_thresholding_includes_and_excludes_by_ratio() {
        // Anchor at 100. VPOC bucket volume 100; threshold 0.7 => >= 70 qualifies.
        // [100,110) c105 = 100 (vpoc), [120,130) c125 = 80 (qualifies),
        // [140,150) c145 = 50 (excluded).
        let obs = vec![
            (dec!(100), dec!(100)),
            (dec!(125), dec!(80)),
            (dec!(145), dec!(50)),
        ];
        // No local-maxima requirement: pure threshold behaviour.
        let profile = compute_volume_profile(&obs, &cfg(false)).unwrap();
        let prices = hvn_prices(&profile);
        assert!(prices.contains(&dec!(105)));
        assert!(prices.contains(&dec!(125)));
        assert!(!prices.contains(&dec!(145)));
    }

    #[test]
    fn local_maxima_filtering_drops_adjacent_plateau_buckets() {
        // Anchor at 100. A clear single peak flanked by lower neighbours:
        // [100,110) c105 = 90, [110,120) c115 = 100 (peak), [120,130) c125 = 90.
        let obs = vec![
            (dec!(100), dec!(90)),
            (dec!(115), dec!(100)),
            (dec!(125), dec!(90)),
        ];
        let with_maxima = compute_volume_profile(&obs, &cfg(true)).unwrap();
        // Only the strict peak at center 115 is reported.
        assert_eq!(hvn_prices(&with_maxima), vec![dec!(115)]);

        // Without the local-maxima requirement all three clear the 0.7 threshold.
        let without_maxima = compute_volume_profile(&obs, &cfg(false)).unwrap();
        assert_eq!(
            hvn_prices(&without_maxima),
            vec![dec!(105), dec!(115), dec!(125)]
        );
    }

    #[test]
    fn ladder_filters_below_spot_sorts_nearest_first_and_caps() {
        // Anchor at 100; one observation per bucket => centers 105..155.
        let obs = vec![
            (dec!(100), dec!(100)),
            (dec!(110), dec!(100)),
            (dec!(120), dec!(100)),
            (dec!(130), dec!(100)),
            (dec!(140), dec!(100)),
            (dec!(150), dec!(100)),
        ];
        let mut config = cfg(false);
        config.ladder_steps = 3;
        let profile = compute_volume_profile(&obs, &config).unwrap();
        assert_eq!(
            hvn_prices(&profile),
            vec![
                dec!(105),
                dec!(115),
                dec!(125),
                dec!(135),
                dec!(145),
                dec!(155)
            ]
        );

        // Spot at 150 -> HVNs below are 105,115,125,135,145; nearest-first capped at 3.
        let ladder = derive_bid_ladder(&profile, dec!(150), &config).unwrap();
        let prices: Vec<Decimal> = ladder.levels.iter().map(|l| l.price).collect();
        assert_eq!(prices, vec![dec!(145), dec!(135), dec!(125)]);
    }

    #[test]
    fn ladder_weights_are_volume_weighted_and_sum_to_one() {
        // Anchor at 100. Two HVNs: [100,110) c105 = 60, [120,130) c125 = 40.
        // Threshold ratio 0.5 so both clear the VPOC-relative threshold (30).
        let obs = vec![(dec!(100), dec!(60)), (dec!(120), dec!(40))];
        let mut config = cfg(false);
        config.hvn_threshold_ratio = dec!(0.5);
        let profile = compute_volume_profile(&obs, &config).unwrap();

        // current_price is independent of the observations; 200 sits above both.
        let ladder = derive_bid_ladder(&profile, dec!(200), &config).unwrap();

        // Nearest first: 125 (vol 40), then 105 (vol 60). Weights ∝ volume.
        assert_eq!(ladder.levels[0].price, dec!(125));
        assert_eq!(ladder.levels[0].weight, dec!(0.4));
        assert_eq!(ladder.levels[1].price, dec!(105));
        assert_eq!(ladder.levels[1].weight, dec!(0.6));
        assert_eq!(weight_sum(&ladder), dec!(1));
    }

    #[test]
    fn three_equal_nodes_weights_sum_to_exactly_one() {
        // Three equal-volume HVNs below spot -> 1/3 each, which does NOT terminate
        // in Decimal. Without the residual-on-last-level rule the weights would sum
        // to 0.999…9, leaving a crumb of the sleeve budget unallocated. This pins
        // the exact-summation invariant the other weighting tests satisfy only by
        // luck of terminating numbers.
        let obs = vec![
            (dec!(100), dec!(50)),
            (dec!(120), dec!(50)),
            (dec!(140), dec!(50)),
        ];
        let mut config = cfg(false);
        config.hvn_threshold_ratio = dec!(0.9);
        let profile = compute_volume_profile(&obs, &config).unwrap();
        let ladder = derive_bid_ladder(&profile, dec!(200), &config).unwrap();
        assert_eq!(ladder.levels.len(), 3);
        assert_eq!(weight_sum(&ladder), dec!(1));
    }

    #[test]
    fn empty_input_is_an_error() {
        let err = compute_volume_profile(&[], &cfg(true));
        assert!(err.is_err());
    }

    #[test]
    fn no_hvn_below_spot_yields_empty_ladder() {
        // All volume above spot -> nothing to bid.
        let obs = vec![(dec!(105), dec!(100)), (dec!(115), dec!(80))];
        let profile = compute_volume_profile(&obs, &cfg(false)).unwrap();
        let ladder = derive_bid_ladder(&profile, dec!(100), &cfg(false)).unwrap();
        assert!(ladder.levels.is_empty());
    }

    #[test]
    fn current_price_below_whole_profile_yields_empty_ladder() {
        let obs = vec![(dec!(105), dec!(100)), (dec!(125), dec!(80))];
        let profile = compute_volume_profile(&obs, &cfg(false)).unwrap();
        let ladder = derive_bid_ladder(&profile, dec!(50), &cfg(false)).unwrap();
        assert!(ladder.levels.is_empty());
    }

    #[test]
    fn single_bucket_profile_has_valid_vpoc() {
        // All observations at one price -> zero-width range -> one bucket.
        let obs = vec![(dec!(100), dec!(10)), (dec!(100), dec!(5))];
        let profile = compute_volume_profile(&obs, &cfg(false)).unwrap();
        assert_eq!(profile.buckets.len(), 1);
        assert_eq!(profile.buckets[0].volume, dec!(15));
        assert_eq!(profile.vpoc, profile.buckets[0].center);
    }

    #[test]
    fn bucket_boundaries_are_half_open_low_inclusive_high_exclusive() {
        // bucket_size 10, min 100. Price exactly on the 110 boundary lands in the
        // NEXT bucket ([110,120)), not [100,110).
        let obs = vec![
            (dec!(100), dec!(1)), // -> [100,110)
            (dec!(110), dec!(1)), // -> [110,120)
        ];
        let profile = compute_volume_profile(&obs, &cfg(false)).unwrap();
        // Two buckets, one unit of volume in each.
        let b0 = profile
            .buckets
            .iter()
            .find(|b| b.price_low == dec!(100))
            .unwrap();
        let b1 = profile
            .buckets
            .iter()
            .find(|b| b.price_low == dec!(110))
            .unwrap();
        assert_eq!(b0.volume, dec!(1));
        assert_eq!(b1.volume, dec!(1));
    }

    // --- Drawdown-tier ladder ---------------------------------------------------

    /// The backtested production shape: -25/-35/-45% at $300/$350/$350.
    fn tiers() -> Vec<DrawdownTier> {
        vec![
            DrawdownTier {
                depth: dec!(0.25),
                allocation_usdc: dec!(300),
            },
            DrawdownTier {
                depth: dec!(0.35),
                allocation_usdc: dec!(350),
            },
            DrawdownTier {
                depth: dec!(0.45),
                allocation_usdc: dec!(350),
            },
        ]
    }

    #[test]
    fn tier_triggers_are_fixed_fractions_of_the_anchor_not_of_spot() {
        // Anchor 100k, spot far below at 90k: triggers stay pinned to the anchor.
        let bids = derive_drawdown_ladder(dec!(100000), dec!(90000), &tiers(), Decimal::ZERO);
        assert_eq!(bids.len(), 3);
        assert_eq!(bids[0].price, dec!(75000));
        assert_eq!(bids[1].price, dec!(65000));
        assert_eq!(bids[2].price, dec!(55000));
        // Allocations are the configured dollars, NOT weights summing to anything.
        assert_eq!(bids[0].value_usdc, dec!(300));
        assert_eq!(bids[1].value_usdc, dec!(350));
        assert_eq!(bids[2].value_usdc, dec!(350));
    }

    #[test]
    fn spend_disarms_shallowest_tiers_first_and_leaves_the_deep_ones_funded() {
        // $300 spent == exactly tier 1's allocation: it drops out, 2 and 3 untouched.
        // This is the property the whole redesign exists for — a -25% dip must not
        // be able to consume the budget reserved for -35% and -45%.
        let bids = derive_drawdown_ladder(dec!(100000), dec!(90000), &tiers(), dec!(300));
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0].tier, 1);
        assert_eq!(bids[0].value_usdc, dec!(350));
        assert_eq!(bids[1].tier, 2);
        assert_eq!(bids[1].value_usdc, dec!(350));
    }

    #[test]
    fn a_partial_fill_leaves_the_tier_bidding_for_the_remainder() {
        // $120 of tier 1's $300 filled -> it re-bids for the $180 balance.
        let bids = derive_drawdown_ladder(dec!(100000), dec!(90000), &tiers(), dec!(120));
        assert_eq!(bids.len(), 3);
        assert_eq!(bids[0].tier, 0);
        assert_eq!(bids[0].value_usdc, dec!(180));
        assert_eq!(bids[1].value_usdc, dec!(350));
    }

    #[test]
    fn spend_spilling_past_a_tier_boundary_partly_disarms_the_next() {
        // $400 = all of tier 1 ($300) + $100 of tier 2 -> tier 2 bids its $250 rest.
        let bids = derive_drawdown_ladder(dec!(100000), dec!(90000), &tiers(), dec!(400));
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0].tier, 1);
        assert_eq!(bids[0].value_usdc, dec!(250));
        assert_eq!(bids[1].tier, 2);
        assert_eq!(bids[1].value_usdc, dec!(350));
    }

    #[test]
    fn fully_spent_tiers_yield_no_bids() {
        let bids = derive_drawdown_ladder(dec!(100000), dec!(90000), &tiers(), dec!(1000));
        assert!(bids.is_empty());
        // Over-spend (fees, rounding) must not wrap around into new bids.
        let bids = derive_drawdown_ladder(dec!(100000), dec!(90000), &tiers(), dec!(5000));
        assert!(bids.is_empty());
    }

    #[test]
    fn triggers_at_or_above_spot_are_skipped() {
        // Spot 70k: tier 1's trigger (75k) is above it — a post-only buy there would
        // be rejected (and wouldn't be a dip buy). Deeper tiers still rest.
        let bids = derive_drawdown_ladder(dec!(100000), dec!(70000), &tiers(), Decimal::ZERO);
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0].tier, 1);
        assert_eq!(bids[1].tier, 2);

        // Price at its high: nothing is in range at all. Normal, empty result.
        let bids = derive_drawdown_ladder(dec!(100000), dec!(100000), &tiers(), Decimal::ZERO);
        assert_eq!(bids.len(), 3, "all three triggers sit below a spot at the high");
    }

    /// Daily series helper: `(day_index_as_ts, high)`.
    fn daily(highs: &[i64]) -> Vec<(i64, Decimal)> {
        highs
            .iter()
            .enumerate()
            .map(|(i, h)| (i as i64, Decimal::from(*h)))
            .collect()
    }

    #[test]
    fn anchor_is_the_trailing_window_max() {
        // Window of 3: only the last three candles count, so the early 500 spike is
        // excluded once it rolls out of the window.
        let series = daily(&[500, 100, 110, 120]);
        let (anchor, _) = anchor_high_and_rearm(&series, 3).unwrap();
        assert_eq!(anchor, dec!(120));
        // A window longer than the series just uses the whole series.
        let (anchor, _) = anchor_high_and_rearm(&series, 99).unwrap();
        assert_eq!(anchor, dec!(500));
    }

    #[test]
    fn rearm_is_the_last_candle_to_set_a_new_window_high() {
        // Rally to a peak at index 4, then a decline that never reclaims it.
        // Window 2: index 4 (140) beat both priors, indices 5-7 did not.
        let series = daily(&[100, 110, 120, 130, 140, 120, 110, 100]);
        let (anchor, rearm) = anchor_high_and_rearm(&series, 2).unwrap();
        assert_eq!(rearm, 4, "the peak is the last arming, not the recent lows");
        assert_eq!(anchor, dec!(110), "trailing 2 highs are 110 and 100");
    }

    #[test]
    fn rearm_falls_back_to_the_oldest_candle_in_an_unbroken_decline() {
        // A monotonic bear: no candle ever sets a new 3-day high, so there is no
        // arming event in the window. Falling back to the oldest timestamp keeps
        // tiers DISARMED (all spend since then counts) rather than resetting them,
        // which would let a bear market re-buy the same tier over and over.
        let series = daily(&[200, 190, 180, 170, 160, 150]);
        let (_, rearm) = anchor_high_and_rearm(&series, 3).unwrap();
        assert_eq!(rearm, 0);
    }

    #[test]
    fn anchor_rejects_empty_series_and_zero_window() {
        assert!(anchor_high_and_rearm(&[], 90).is_err());
        assert!(anchor_high_and_rearm(&daily(&[100]), 0).is_err());
    }

    #[test]
    fn max_observation_clamps_into_final_bucket() {
        // With min 100, size 10 and a max at 120, the range spans buckets
        // [100,110),[110,120),[120,130); the max at 120 must be counted, not dropped.
        let obs = vec![(dec!(100), dec!(1)), (dec!(120), dec!(7))];
        let profile = compute_volume_profile(&obs, &cfg(false)).unwrap();
        let last = profile.buckets.last().unwrap();
        assert_eq!(last.price_low, dec!(120));
        assert_eq!(last.volume, dec!(7));
    }
}
