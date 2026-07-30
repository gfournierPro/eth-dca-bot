//! Backtest harness for the drawdown-tier sleeve. Drives the REAL production tier
//! math (`anchor_high_and_rearm`, `derive_drawdown_ladder`) over historical daily
//! candles, with the production placement semantics (per-tier allocation capped by
//! the accrued chest; a bid fills when the day's low touches its trigger).
//!
//! `#[ignore]`d because it needs candle files fetched to /tmp first:
//!
//!   curl -s "https://api.kraken.com/0/public/OHLC?pair=XBTUSD&interval=1440" -o /tmp/btc_daily.json
//!   curl -s "https://api.kraken.com/0/public/OHLC?pair=ETHUSD&interval=1440" -o /tmp/eth_daily.json
//!   cargo test --test drawdown_backtest -- --ignored --nocapture
//!
//! Use it to sanity-check tier depths/allocations before changing the live config.
//! Fill model caveats: fills at trigger price on daily lows, no fees, and assumes
//! the resting bid is always on the book (reconcile lag ignored).

use chrono::{Datelike, NaiveDate};
use eth_dca_bot::levels::{DrawdownTier, anchor_high_and_rearm, derive_drawdown_ladder};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;

struct Candle {
    ts: i64,
    high: Decimal,
    low: Decimal,
    close: Decimal,
}

fn load(path: &str) -> Vec<Candle> {
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let obj = raw["result"].as_object().unwrap();
    let arr = obj
        .iter()
        .find(|(k, v)| *k != "last" && v.is_array())
        .map(|(_, v)| v.as_array().unwrap())
        .unwrap();
    arr.iter()
        .map(|row| {
            let r = row.as_array().unwrap();
            Candle {
                ts: r[0].as_i64().unwrap(),
                high: r[2].as_str().unwrap().parse().unwrap(),
                low: r[3].as_str().unwrap().parse().unwrap(),
                close: r[4].as_str().unwrap().parse().unwrap(),
            }
        })
        .collect()
}

fn date_of(ts: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp(ts, 0).unwrap().date_naive()
}

// Mirrors limit_sleeve::months_elapsed (private there).
fn months_elapsed(start: NaiveDate, now: NaiveDate) -> i64 {
    ((now.year() as i64 - start.year() as i64) * 12 + (now.month() as i64 - start.month() as i64))
        .max(0)
}

struct Fill {
    ts: i64,
    tier: usize,
    price: Decimal,
    value: Decimal,
}

#[allow(clippy::too_many_arguments)]
fn run(
    label: &str,
    candles: &[Candle],
    days: usize,
    tiers: &[DrawdownTier],
    anchor_days: usize,
    starting_chest: Decimal,
    monthly_accrual: Decimal,
    cap: Decimal,
) {
    let start_idx = candles.len().saturating_sub(days);
    assert!(start_idx >= anchor_days, "need warmup history before window");
    let sim_start = date_of(candles[start_idx].ts);

    let mut fills: Vec<Fill> = Vec::new();

    for i in start_idx..candles.len() {
        let today = &candles[i];
        // History up to yesterday: bids derived at day start, tested against today.
        let daily: Vec<(i64, Decimal)> = candles[..i].iter().map(|c| (c.ts, c.high)).collect();
        let (anchor, rearm_ts) = anchor_high_and_rearm(&daily, anchor_days).unwrap();
        let spot = candles[i - 1].close;

        let spent_all: Decimal = fills.iter().map(|f| f.value).sum();
        let spent_since_rearm: Decimal = fills
            .iter()
            .filter(|f| f.ts >= rearm_ts)
            .map(|f| f.value)
            .sum();
        let accrued = starting_chest
            + monthly_accrual * Decimal::from(months_elapsed(sim_start, date_of(today.ts)));
        let mut available = (accrued - spent_all).clamp(Decimal::ZERO, cap);

        // Production ladder, then production placement semantics: each bid capped by
        // its own allocation and by the chest; fills when the day's low touches it.
        let bids = derive_drawdown_ladder(anchor, spot, tiers, spent_since_rearm);
        for b in &bids {
            let alloc = b.value_usdc.min(available);
            if alloc > Decimal::ZERO && today.low <= b.price {
                available -= alloc;
                fills.push(Fill {
                    ts: today.ts,
                    tier: b.tier,
                    price: b.price,
                    value: alloc,
                });
            }
        }
    }

    let last_close = candles.last().unwrap().close;
    let spent: Decimal = fills.iter().map(|f| f.value).sum();
    let qty: Decimal = fills.iter().map(|f| f.value / f.price).sum();
    let window_closes: Vec<Decimal> = candles[start_idx..].iter().map(|c| c.close).collect();
    let dca_avg: Decimal =
        window_closes.iter().copied().sum::<Decimal>() / Decimal::from(window_closes.len());

    println!("\n===== {label} =====");
    println!(
        "window {} -> {} | start chest {starting_chest} + {monthly_accrual}/mo, cap {cap}",
        sim_start,
        date_of(candles.last().unwrap().ts)
    );
    for f in &fills {
        println!(
            "  {}  tier {}  FILL {:>10.2} @ {:>10.2}",
            date_of(f.ts),
            f.tier + 1,
            f.value,
            f.price
        );
    }
    if fills.is_empty() {
        println!("  (no fills all year)");
        return;
    }
    let avg = spent / qty;
    let value_now = qty * last_close;
    let accrued_total = starting_chest
        + monthly_accrual
            * Decimal::from(months_elapsed(sim_start, date_of(candles.last().unwrap().ts)));
    println!(
        "  spent {:.2} of {:.2} accrued | avg entry {:.2} | last close {:.2}",
        spent, accrued_total, avg, last_close
    );
    println!(
        "  position value now {:.2} | P&L {:+.2} ({:+.2}%)",
        value_now,
        value_now - spent,
        ((value_now - spent) / spent * dec!(100)).to_f64().unwrap()
    );
    println!(
        "  benchmark: uniform daily DCA over the same window would average {:.2} \
         (strategy entry is {:.1}% below it)",
        dca_avg,
        ((dca_avg - avg) / dca_avg * dec!(100)).to_f64().unwrap()
    );
}

fn t(spec: &[(&str, i64)]) -> Vec<DrawdownTier> {
    spec.iter()
        .map(|(d, a)| DrawdownTier {
            depth: d.parse().unwrap(),
            allocation_usdc: Decimal::from(*a),
        })
        .collect()
}

#[test]
#[ignore = "needs /tmp candle files; see module docs"]
fn drawdown_backtest() {
    let btc = load("/tmp/btc_daily.json");
    let eth = load("/tmp/eth_daily.json");

    // Shipped defaults, last 365 days.
    run(
        "BTC last 365d — shipped defaults (-25/-35/-45, 300/350/350)",
        &btc,
        365,
        &t(&[("0.25", 300), ("0.35", 350), ("0.45", 350)]),
        90,
        dec!(500),
        dec!(83.33),
        dec!(1000),
    );
    run(
        "ETH last 365d — shipped defaults (-35/-50/-65, 150/175/175)",
        &eth,
        365,
        &t(&[("0.35", 150), ("0.50", 175), ("0.65", 175)]),
        90,
        dec!(250),
        dec!(41.67),
        dec!(500),
    );
    // ETH sensitivity: BTC-style depths, to sanity-check the "ETH must be deeper" call.
    run(
        "ETH last 365d — BTC-style depths (-25/-35/-45, 150/175/175)",
        &eth,
        365,
        &t(&[("0.25", 150), ("0.35", 175), ("0.45", 175)]),
        90,
        dec!(250),
        dec!(41.67),
        dec!(500),
    );
    // Full 2y context for BTC (matches the design-phase window).
    run(
        "BTC last 630d — shipped defaults, full-history context",
        &btc,
        630,
        &t(&[("0.25", 300), ("0.35", 350), ("0.45", 350)]),
        90,
        dec!(500),
        dec!(83.33),
        dec!(1000),
    );
}
