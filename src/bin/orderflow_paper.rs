//! Order-flow paper trader (BTC, OKX public API, no keys, no real orders).
//!
//! Polls the order book and trade tape every few seconds, computes two
//! order-flow signals, and simulates a two-sided strategy with honest costs:
//! buys fill at best ask, sells at best bid, taker fee each side.
//!
//! Signals:
//!   - Book imbalance over top 10 levels: (bidVol - askVol) / (bidVol + askVol)
//!   - CVD: rolling 5-minute cumulative volume delta from the tape (buys - sells)
//!
//! Strategy: enter long when imbalance > OF_IMB_ENTRY and CVD > 0; enter short
//! when imbalance < -OF_IMB_ENTRY and CVD < 0. Exit on the mirrored signal
//! (long: imbalance < -0.1 or CVD < 0; short: imbalance > 0.1 or CVD > 0), or
//! on a -1% adverse stop. 60s cooldown between trades.
//!
//! Closed trades append to orderflow_paper_trades.jsonl; each close also prints
//! cumulative P&L vs buy-and-hold of the same stake from the first price seen.
//! State is in-memory only: a restart forgets an open paper position (fine for
//! paper — the JSONL keeps every closed trade).
//!
//! Env knobs: OF_PAIR (default BTC-USDC; note BTC-USDT is the deeper book if
//! the signal looks too noisy), OF_STAKE (default 300), OF_IMB_ENTRY
//! (default 0.3).
//!
//! Run: cargo run --bin orderflow_paper

use std::collections::VecDeque;
use std::io::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

const TAKER_FEE: f64 = 0.001; // OKX spot taker, base tier
const IMB_EXIT: f64 = -0.1;
const STOP_PCT: f64 = -0.01;
const CVD_WINDOW_SECS: f64 = 300.0;
const COOLDOWN_SECS: f64 = 60.0;
const POLL_SECS: u64 = 3;
const TRADES_FILE: &str = "orderflow_paper_trades.jsonl";
const OKX_BASE: &str = "https://www.okx.com/api/v5/market";

/// (bidVol - askVol) / (bidVol + askVol) over the given levels; 0 on empty book.
fn imbalance(bids: &[(f64, f64)], asks: &[(f64, f64)]) -> f64 {
    let bid_vol: f64 = bids.iter().map(|(_, v)| v).sum();
    let ask_vol: f64 = asks.iter().map(|(_, v)| v).sum();
    let total = bid_vol + ask_vol;
    if total == 0.0 { 0.0 } else { (bid_vol - ask_vol) / total }
}

/// Net P&L in quote currency for a round trip: stake bought at `entry_px`
/// (taker fee on the way in) and sold at `exit_px` (taker fee on the way out).
fn round_trip_pnl(stake: f64, entry_px: f64, exit_px: f64) -> f64 {
    let base = stake * (1.0 - TAKER_FEE) / entry_px;
    let proceeds = base * exit_px * (1.0 - TAKER_FEE);
    proceeds - stake
}

/// Net P&L in quote currency for a short round trip: base worth `stake` sold
/// at `entry_px` (taker fee on the way in), bought back at `exit_px` (taker
/// fee on the way out).
fn short_round_trip_pnl(stake: f64, entry_px: f64, exit_px: f64) -> f64 {
    let proceeds = stake * (1.0 - TAKER_FEE);
    let buyback_cost = stake * exit_px / (entry_px * (1.0 - TAKER_FEE));
    proceeds - buyback_cost
}

#[derive(Clone, Copy)]
enum Side {
    Long,
    Short,
}

impl Side {
    fn as_str(self) -> &'static str {
        match self {
            Side::Long => "long",
            Side::Short => "short",
        }
    }
}

struct Position {
    side: Side,
    entry_px: f64,
    entry_time: f64,
}

struct Stats {
    trades: u32,
    wins: u32,
    net_pnl: f64,
    first_price: f64,
}

struct Book {
    bids: Vec<(f64, f64)>, // (price, volume), best first
    asks: Vec<(f64, f64)>,
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()
}

/// OKX book levels are `["px", "sz", ...]` string arrays.
fn parse_levels(levels: &Value) -> Vec<(f64, f64)> {
    levels
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    let px = l.get(0)?.as_str()?.parse().ok()?;
                    let vol = l.get(1)?.as_str()?.parse().ok()?;
                    Some((px, vol))
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn okx_public(client: &reqwest::Client, path: &str, query: &[(&str, &str)]) -> Result<Value> {
    let body: Value = client
        .get(format!("{OKX_BASE}/{path}"))
        .query(query)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if body["code"].as_str() != Some("0") {
        return Err(anyhow!("OKX error on {path}: {}", body["msg"]));
    }
    Ok(body["data"].clone())
}

async fn fetch_book(client: &reqwest::Client, inst_id: &str) -> Result<Book> {
    let data = okx_public(client, "books", &[("instId", inst_id), ("sz", "10")]).await?;
    let book = data.get(0).ok_or_else(|| anyhow!("empty books response"))?;
    Ok(Book {
        bids: parse_levels(&book["bids"]),
        asks: parse_levels(&book["asks"]),
    })
}

/// Fetch the latest tape and return trades newer than `last_trade_id` as
/// (time_secs, signed volume): +sz for buyer-initiated, -sz for seller-initiated.
/// Also returns the new high-water-mark trade id.
async fn fetch_trades(
    client: &reqwest::Client,
    inst_id: &str,
    last_trade_id: u64,
) -> Result<(Vec<(f64, f64)>, u64)> {
    let data = okx_public(client, "trades", &[("instId", inst_id), ("limit", "100")]).await?;
    let mut max_id = last_trade_id;
    let mut out = Vec::new();
    for t in data.as_array().cloned().unwrap_or_default() {
        let Some(id) = t["tradeId"].as_str().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        if id <= last_trade_id {
            continue;
        }
        max_id = max_id.max(id);
        let (Some(sz), Some(ts), Some(side)) = (
            t["sz"].as_str().and_then(|s| s.parse::<f64>().ok()),
            t["ts"].as_str().and_then(|s| s.parse::<f64>().ok()),
            t["side"].as_str(),
        ) else {
            continue;
        };
        out.push((ts / 1000.0, if side == "buy" { sz } else { -sz }));
    }
    Ok((out, max_id))
}

fn log_trade(record: &Value) -> Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(TRADES_FILE)
        .context("open trades log")?;
    writeln!(f, "{record}")?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let pair = std::env::var("OF_PAIR").unwrap_or_else(|_| "BTC-USDC".into());
    let stake: f64 = std::env::var("OF_STAKE").ok().and_then(|v| v.parse().ok()).unwrap_or(300.0);
    let imb_entry: f64 =
        std::env::var("OF_IMB_ENTRY").ok().and_then(|v| v.parse().ok()).unwrap_or(0.3);

    println!(
        "orderflow_paper: pair={pair} stake=${stake} imb_entry={imb_entry} \
         fees={:.2}%/side (OKX taker) — PAPER ONLY, no orders are placed",
        TAKER_FEE * 100.0
    );

    let client = reqwest::Client::new();
    let mut tape: VecDeque<(f64, f64)> = VecDeque::new(); // (time, signed vol)
    let mut position: Option<Position> = None;
    let mut last_exit_time = 0.0_f64;
    let mut stats = Stats { trades: 0, wins: 0, net_pnl: 0.0, first_price: 0.0 };
    let mut ticks: u64 = 0;

    // Prime the trade-id watermark so the first CVD window only sees fresh trades.
    let mut last_trade_id = fetch_trades(&client, &pair, 0)
        .await
        .context("initial trades fetch — is the pair valid?")?
        .1;

    loop {
        tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
        ticks += 1;

        let (book, trades) = match tokio::try_join!(
            fetch_book(&client, &pair),
            fetch_trades(&client, &pair, last_trade_id)
        ) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("poll failed (will retry): {e}");
                continue;
            }
        };
        let (new_trades, max_id) = trades;
        last_trade_id = max_id;
        tape.extend(new_trades);

        let now = now_secs();
        while tape.front().is_some_and(|(t, _)| now - t > CVD_WINDOW_SECS) {
            tape.pop_front();
        }
        let cvd: f64 = tape.iter().map(|(_, v)| v).sum();

        let (Some(&(best_bid, _)), Some(&(best_ask, _))) = (book.bids.first(), book.asks.first())
        else {
            continue;
        };
        let imb = imbalance(&book.bids, &book.asks);
        if stats.first_price == 0.0 {
            stats.first_price = best_ask;
        }

        match &position {
            None => {
                if now - last_exit_time > COOLDOWN_SECS {
                    let side = if imb > imb_entry && cvd > 0.0 {
                        Some((Side::Long, best_ask))
                    } else if imb < -imb_entry && cvd < 0.0 {
                        Some((Side::Short, best_bid))
                    } else {
                        None
                    };
                    if let Some((side, entry_px)) = side {
                        println!(
                            "[ENTER] {} ${stake} @ {entry_px:.2} (imb={imb:.2} cvd={cvd:.4})",
                            side.as_str()
                        );
                        position = Some(Position { side, entry_px, entry_time: now });
                    }
                }
            }
            Some(pos) => {
                let (exit_px, unrealized, signal_exit) = match pos.side {
                    Side::Long => (
                        best_bid,
                        (best_bid - pos.entry_px) / pos.entry_px,
                        imb < IMB_EXIT || cvd < 0.0,
                    ),
                    Side::Short => (
                        best_ask,
                        (pos.entry_px - best_ask) / pos.entry_px,
                        imb > -IMB_EXIT || cvd > 0.0,
                    ),
                };
                let reason = if unrealized <= STOP_PCT {
                    Some("stop")
                } else if signal_exit {
                    Some("signal")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    let pnl = match pos.side {
                        Side::Long => round_trip_pnl(stake, pos.entry_px, exit_px),
                        Side::Short => short_round_trip_pnl(stake, pos.entry_px, exit_px),
                    };
                    stats.trades += 1;
                    if pnl > 0.0 {
                        stats.wins += 1;
                    }
                    stats.net_pnl += pnl;
                    let hold_pnl = stake * (best_bid / stats.first_price - 1.0);
                    let record = json!({
                        "side": pos.side.as_str(),
                        "entry_time": pos.entry_time,
                        "exit_time": now,
                        "entry_px": pos.entry_px,
                        "exit_px": exit_px,
                        "stake": stake,
                        "pnl": pnl,
                        "reason": reason,
                        "imb": imb,
                        "cvd": cvd,
                    });
                    if let Err(e) = log_trade(&record) {
                        eprintln!("failed to log trade: {e}");
                    }
                    println!(
                        "[EXIT:{reason}] {} @ {exit_px:.2} pnl=${pnl:.2} | total: {} trades, \
                         {} wins, net=${:.2} | buy&hold same stake: ${hold_pnl:.2}",
                        pos.side.as_str(),
                        stats.trades,
                        stats.wins,
                        stats.net_pnl
                    );
                    position = None;
                    last_exit_time = now;
                }
            }
        }

        // Heartbeat roughly every minute so a quiet market still shows life.
        if ticks % 20 == 0 {
            println!(
                "[tick] bid={best_bid:.2} ask={best_ask:.2} imb={imb:.2} cvd={cvd:.4} \
                 pos={} net=${:.2}",
                position.as_ref().map_or("flat", |p| p.side.as_str()),
                stats.net_pnl
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imbalance_signs_and_bounds() {
        // All bids, no asks -> +1; the reverse -> -1; balanced -> 0; empty -> 0.
        assert_eq!(imbalance(&[(100.0, 5.0)], &[]), 1.0);
        assert_eq!(imbalance(&[], &[(100.0, 5.0)]), -1.0);
        assert_eq!(imbalance(&[(100.0, 2.0)], &[(101.0, 2.0)]), 0.0);
        assert_eq!(imbalance(&[], &[]), 0.0);
    }

    #[test]
    fn fees_eat_a_flat_round_trip() {
        // Same entry and exit price must lose ~2x the taker fee.
        let pnl = round_trip_pnl(300.0, 50_000.0, 50_000.0);
        let expected = 300.0 * ((1.0 - TAKER_FEE) * (1.0 - TAKER_FEE) - 1.0);
        assert!((pnl - expected).abs() < 1e-9, "pnl={pnl} expected={expected}");
        assert!(pnl < 0.0);
    }

    #[test]
    fn round_trip_needs_move_bigger_than_fees() {
        // +0.1% move on 0.1%/side fees is still a loss; +0.3% is a win.
        assert!(round_trip_pnl(300.0, 50_000.0, 50_050.0) < 0.0);
        assert!(round_trip_pnl(300.0, 50_000.0, 50_150.0) > 0.0);
    }

    #[test]
    fn short_fees_eat_a_flat_round_trip() {
        // Same entry and exit price must lose ~2x the taker fee, like the long.
        let pnl = short_round_trip_pnl(300.0, 50_000.0, 50_000.0);
        let expected = 300.0 * ((1.0 - TAKER_FEE) - 1.0 / (1.0 - TAKER_FEE));
        assert!((pnl - expected).abs() < 1e-9, "pnl={pnl} expected={expected}");
        assert!(pnl < 0.0);
    }

    #[test]
    fn short_needs_drop_bigger_than_fees() {
        // -0.1% move on 0.1%/side fees is still a loss; -0.3% is a win.
        assert!(short_round_trip_pnl(300.0, 50_000.0, 49_950.0) < 0.0);
        assert!(short_round_trip_pnl(300.0, 50_000.0, 49_850.0) > 0.0);
        // Price going UP against a short loses more than fees alone.
        let flat = short_round_trip_pnl(300.0, 50_000.0, 50_000.0);
        assert!(short_round_trip_pnl(300.0, 50_000.0, 50_500.0) < flat);
    }

    #[test]
    fn parses_okx_level_arrays() {
        let levels = serde_json::json!([["50000.1", "1.5", "0", "3"], ["49999.0", "2.0", "0", "1"]]);
        assert_eq!(parse_levels(&levels), vec![(50000.1, 1.5), (49999.0, 2.0)]);
    }

    #[test]
    fn trade_id_watermark_dedupes() {
        // fetch_trades filters ids <= watermark; simulate its core filter here.
        let ids = [5_u64, 6, 7];
        let watermark = 6_u64;
        let fresh: Vec<_> = ids.iter().filter(|&&id| id > watermark).collect();
        assert_eq!(fresh, vec![&7]);
    }
}
