//! Backtest for the `market_indicators` DCA sizing multipliers. Drives the REAL
//! production calculators (`MarketIndicators::multiplier_from_history`) over
//! historical daily candles: one simulated buy per week (the live cadence), each
//! sized `base × multiplier`, executed at that day's open.
//!
//! Answers: does the 0.7×–1.3× modulation actually lower the average entry price
//! versus flat DCA, and which of the four signals carries the result?
//!
//! `#[ignore]`d because it needs candle files fetched to /tmp first:
//!
//!   curl -s "https://api.kraken.com/0/public/OHLC?pair=XBTUSD&interval=1440" -o /tmp/btc_daily.json
//!   curl -s "https://api.kraken.com/0/public/OHLC?pair=ETHUSD&interval=1440" -o /tmp/eth_daily.json
//!   cargo test --test dca_modulation_backtest -- --ignored --nocapture
//!
//! Metric note: average entry (spent/qty) is invariant to a signal that scales
//! every buy uniformly — only *differential* sizing (more when cheap, less when
//! expensive) can move it. That is the fair test of a modulation layer.

use chrono::{DateTime, Datelike, Utc, Weekday};
use eth_dca_bot::market_indicators::{MarketIndicators, MarketIndicatorsConfig, PriceData};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;

struct Candle {
    ts: i64,
    open: Decimal,
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
                open: r[1].as_str().unwrap().parse().unwrap(),
                close: r[4].as_str().unwrap().parse().unwrap(),
            }
        })
        .collect()
}

fn when(ts: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts, 0).unwrap()
}

/// One weekly-DCA replay. Returns (avg_entry, total_spent, min_mult, max_mult).
fn replay(
    candles: &[Candle],
    days: usize,
    config: &MarketIndicatorsConfig,
) -> (Decimal, Decimal, Decimal, Decimal) {
    let start_idx = candles.len().saturating_sub(days).max(61);
    let mut spent = Decimal::ZERO;
    let mut qty = Decimal::ZERO;
    let (mut lo, mut hi) = (dec!(99), dec!(0));

    for i in start_idx..candles.len() {
        let c = &candles[i];
        if when(c.ts).weekday() != Weekday::Mon {
            continue;
        }
        // History exactly as production builds it: trailing daily closes (oldest
        // first), then the buy-moment price appended as the "current tick".
        let mut history: Vec<PriceData> = candles[i - 60..i]
            .iter()
            .map(|k| PriceData {
                timestamp: when(k.ts),
                price: k.close,
                volume: None,
            })
            .collect();
        history.push(PriceData {
            timestamp: when(c.ts),
            price: c.open,
            volume: None,
        });

        let mi = MarketIndicators::with_history(config.clone(), history);
        let mult = mi.multiplier_from_history().unwrap();
        lo = lo.min(mult);
        hi = hi.max(mult);

        let buy = mult; // base 1.0 per week
        spent += buy;
        qty += buy / c.open;
    }
    (spent / qty, spent, lo, hi)
}

fn cfg(vol: bool, rsi: bool, ma: bool, mom: bool) -> MarketIndicatorsConfig {
    MarketIndicatorsConfig {
        volatility_scaling_enabled: vol,
        rsi_enabled: rsi,
        price_deviation_enabled: ma,
        momentum_enabled: mom,
        ..Default::default()
    }
}

#[test]
#[ignore = "needs /tmp candle files; see module docs"]
fn dca_modulation_backtest() {
    for (asset, path) in [("BTC", "/tmp/btc_daily.json"), ("ETH", "/tmp/eth_daily.json")] {
        let candles = load(path);
        for days in [365usize, 630] {
            let (flat_avg, _, _, _) = replay(&candles, days, &cfg(false, false, false, false));
            println!("\n===== {asset} last {days}d | flat DCA avg entry {flat_avg:.2} =====");
            for (label, c) in [
                ("all four (live config)   ", cfg(true, true, true, true)),
                ("RSI+MA+momentum (no vol) ", cfg(false, true, true, true)),
                ("volatility only          ", cfg(true, false, false, false)),
                ("RSI only                 ", cfg(false, true, false, false)),
                ("MA deviation only        ", cfg(false, false, true, false)),
                ("momentum only            ", cfg(false, false, false, true)),
            ] {
                let (avg, spent, lo, hi) = replay(&candles, days, &c);
                let delta = ((flat_avg - avg) / flat_avg * dec!(100)).to_f64().unwrap();
                println!(
                    "  {label} avg {avg:>9.2}  ({delta:+.2}% vs flat)  spent {spent:.1} wk-units  mult range [{lo}, {hi}]"
                );
            }
        }
    }
}
