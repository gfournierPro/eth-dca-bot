//! OKX spot REST client (v5 API).
//!
//! Mirrors the surface the DCA bot needs from an exchange (see [`crate::exchange`]).
//! OKX differs from Binance/Kraken in a few ways handled here so the rest of the
//! bot stays exchange-agnostic:
//!
//! * Auth is base64(HMAC-SHA256(secret, timestamp + method + path + body)) with an
//!   extra passphrase header (`OK-ACCESS-PASSPHRASE`).
//! * Instruments are dash-separated: `ETHUSDC` -> `ETH-USDC`.
//! * Market buys can be sized directly in the quote currency (`tgtCcy=quote_ccy`),
//!   but spot buy fees are charged in the *base* asset, so they are converted back
//!   to USDC via the average fill price.
//! * Funds live in two sub-accounts: trading (where buys settle) and funding (the
//!   only one that can withdraw), so `withdraw` transfers trading -> funding first.
//! * Withdrawals go to a raw on-chain address (`dest=4`) with an explicit chain
//!   name like `ETH-Arbitrum One`.
//!
//! Validated live on OKX EEA (my.okx.com): read-only path and market buys.
//! Smoke-test remaining paths with `cargo run --bin test_okx` (`--limit`,
//! `--verify-withdraw`) before relying on them.

use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::{Datelike, TimeZone, Utc};
use hmac::{Hmac, Mac};
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::Sha256;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};
use uuid::Uuid;

use crate::dca_stats_mongo::DcaPurchase;
use crate::exchange::{Exchange, LimitBuyConfig, OrderOutcome};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct OkxClient {
    client: Client,
    api_key: String,
    secret_key: String,
    passphrase: String,
    base_url: String,
}

/// Every OKX v5 response: `{"code":"0","msg":"","data":[...]}`. Non-"0" code is an
/// error; `data` is always an array even for single objects.
#[derive(Debug, Deserialize)]
struct OkxEnvelope<T> {
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct TickerData {
    last: String,
}

#[derive(Debug, Deserialize)]
struct BalanceDetail {
    ccy: String,
    #[serde(rename = "availBal")]
    avail_bal: String,
}

#[derive(Debug, Deserialize)]
struct BalanceData {
    #[serde(default)]
    details: Vec<BalanceDetail>,
}

/// Per-order result inside a trade/order placement response. The envelope `code`
/// can be "0" while the individual order failed, so `sCode` must be checked too.
#[derive(Debug, Deserialize)]
struct PlaceOrderData {
    #[serde(rename = "ordId", default)]
    ord_id: String,
    #[serde(rename = "sCode", default)]
    s_code: String,
    #[serde(rename = "sMsg", default)]
    s_msg: String,
}

#[derive(Debug, Deserialize)]
struct OrderData {
    state: String,
    #[serde(default)]
    side: String,
    #[serde(rename = "accFillSz", default)]
    acc_fill_sz: String,
    #[serde(rename = "avgPx", default)]
    avg_px: String,
    #[serde(default)]
    fee: String,
    #[serde(rename = "feeCcy", default)]
    fee_ccy: String,
    #[serde(rename = "ordId", default)]
    ord_id: String,
    /// Last update time, unix ms — the fill time for a filled order.
    #[serde(rename = "uTime", default)]
    u_time: String,
}

#[derive(Debug, Deserialize)]
struct CurrencyChainData {
    chain: String,
    #[serde(rename = "canWd", default)]
    can_wd: bool,
    #[serde(rename = "minWd", default)]
    min_wd: String,
    #[serde(rename = "minFee", default)]
    min_fee: String,
}

#[derive(Debug, Deserialize)]
struct WithdrawalData {
    #[serde(rename = "wdId")]
    wd_id: String,
}

/// Raw order-book response: each level is `[price, size, liquidatedOrders, numOrders]`
/// as strings.
#[derive(Debug, Deserialize)]
struct BookData {
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
}

/// Top of the order book. `bid_str` is kept verbatim from the API so the limit
/// price we post back respects OKX's tick size for the instrument.
#[derive(Debug, Clone)]
struct BookTop {
    bid: Decimal,
    bid_str: String,
    ask: Decimal,
}

#[derive(Debug, Deserialize)]
struct InstrumentData {
    /// Order size granularity (base ccy for spot limit orders).
    #[serde(rename = "lotSz", default)]
    lot_sz: String,
    /// Minimum order size (base ccy).
    #[serde(rename = "minSz", default)]
    min_sz: String,
}

impl OkxClient {
    pub fn new(api_key: String, secret_key: String, passphrase: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            secret_key,
            passphrase,
            base_url,
        }
    }

    /// Translate a generic USDC pair symbol (`ETHUSDC`, `BTCUSDC`) into OKX's
    /// dash-separated instrument id (`ETH-USDC`).
    fn okx_inst(symbol: &str) -> String {
        match symbol.strip_suffix("USDC") {
            Some(base) if !base.is_empty() => format!("{base}-USDC"),
            _ => symbol.to_string(),
        }
    }

    /// OKX chain name for a withdrawal: `{ccy}-{network label}`, e.g.
    /// `ETH-Arbitrum One`, `BTC-Bitcoin`, `USDC-ERC20`. Maps the bot's generic
    /// network names; anything unrecognised is passed through verbatim so new
    /// networks can be configured without a code change.
    fn okx_chain(asset: &str, network: &str) -> String {
        let ccy = asset.to_uppercase();
        let label = match network.to_uppercase().as_str() {
            "ARBITRUM" | "ARBITRUM ONE" => "Arbitrum One".to_string(),
            "BTC" | "BITCOIN" => "Bitcoin".to_string(),
            "ETH" | "ERC20" => "ERC20".to_string(),
            _ => network.to_string(),
        };
        format!("{ccy}-{label}")
    }

    /// ISO-8601 millisecond timestamp OKX expects in `OK-ACCESS-TIMESTAMP`.
    fn timestamp() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    /// Sign a private request: base64(HMAC-SHA256(secret, ts + method + path + body)).
    /// `path` must include the query string for GETs; `body` is empty for GETs.
    fn sign(&self, timestamp: &str, method: &str, path: &str, body: &str) -> Result<String> {
        let mut mac = HmacSha256::new_from_slice(self.secret_key.trim().as_bytes())
            .map_err(|e| anyhow!("Failed to init OKX HMAC: {}", e))?;
        mac.update(timestamp.as_bytes());
        mac.update(method.as_bytes());
        mac.update(path.as_bytes());
        mac.update(body.as_bytes());
        Ok(BASE64.encode(mac.finalize().into_bytes()))
    }

    fn unwrap_envelope<T>(env: OkxEnvelope<T>, path: &str) -> Result<Vec<T>> {
        if env.code != "0" {
            return Err(anyhow!(
                "OKX API error on {}: code {} ({})",
                path,
                env.code,
                env.msg
            ));
        }
        Ok(env.data)
    }

    /// Unauthenticated GET (market data). `path` includes any query string.
    async fn public_get<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        let url = format!("{}{}", self.base_url, path);
        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(anyhow!("OKX public request to {} failed: {}", path, text));
        }
        Self::unwrap_envelope(response.json().await?, path)
    }

    /// Signed request. GETs pass `None` body (query lives in `path`); POSTs pass
    /// the JSON body.
    async fn private_request<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Vec<T>> {
        let body_str = body.map(|b| b.to_string()).unwrap_or_default();
        let timestamp = Self::timestamp();
        let signature = self.sign(&timestamp, method, path, &body_str)?;
        let url = format!("{}{}", self.base_url, path);

        let mut req = match method {
            "GET" => self.client.get(&url),
            "POST" => self
                .client
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body_str),
            other => return Err(anyhow!("Unsupported OKX method {}", other)),
        };
        req = req
            .header("OK-ACCESS-KEY", &self.api_key)
            .header("OK-ACCESS-SIGN", signature)
            .header("OK-ACCESS-TIMESTAMP", timestamp)
            .header("OK-ACCESS-PASSPHRASE", &self.passphrase);

        let response = req.send().await?;
        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(anyhow!("OKX private request to {} failed: {}", path, text));
        }
        Self::unwrap_envelope(response.json().await?, path)
    }

    /// Free balance of `ccy` in the *trading* account (where spot buys settle).
    async fn trading_balance(&self, ccy: &str) -> Result<Decimal> {
        let ccy = ccy.to_uppercase();
        let path = format!("/api/v5/account/balance?ccy={ccy}");
        let data: Vec<BalanceData> = self.private_request("GET", &path, None).await?;
        let balance = data
            .into_iter()
            .flat_map(|d| d.details)
            .find(|d| d.ccy == ccy)
            .map(|d| parse_dec(&d.avail_bal))
            .unwrap_or(Decimal::ZERO);
        Ok(balance)
    }

    /// Free balance of `ccy` in the *funding* account (the only one that can
    /// withdraw; spot buys settle in trading, so funds land here only via the
    /// pre-withdrawal transfer or manual moves in the app).
    async fn funding_balance(&self, ccy: &str) -> Result<Decimal> {
        #[derive(Debug, Deserialize)]
        struct FundingBal {
            ccy: String,
            #[serde(rename = "availBal", default)]
            avail_bal: String,
        }
        let ccy = ccy.to_uppercase();
        let path = format!("/api/v5/asset/balances?ccy={ccy}");
        let data: Vec<FundingBal> = self.private_request("GET", &path, None).await?;
        Ok(data
            .into_iter()
            .find(|d| d.ccy == ccy)
            .map(|d| parse_dec(&d.avail_bal))
            .unwrap_or(Decimal::ZERO))
    }

    /// Fetch one order by id, or `None` if OKX doesn't return it.
    async fn query_order(&self, inst_id: &str, ord_id: &str) -> Result<Option<OrderData>> {
        let path = format!("/api/v5/trade/order?instId={inst_id}&ordId={ord_id}");
        let mut data: Vec<OrderData> = self.private_request("GET", &path, None).await?;
        Ok(if data.is_empty() {
            None
        } else {
            Some(data.remove(0))
        })
    }

    /// Fetch the top of the order book (best bid / best ask) for an instrument.
    async fn get_order_book(&self, inst_id: &str) -> Result<BookTop> {
        let data: Vec<BookData> = self
            .public_get(&format!("/api/v5/market/books?instId={inst_id}&sz=1"))
            .await?;
        let book = data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("OKX returned no order book for {}", inst_id))?;
        let bid_str = book
            .bids
            .first()
            .and_then(|l| l.first())
            .ok_or_else(|| anyhow!("OKX order book for {} has no bids", inst_id))?
            .clone();
        let ask = book
            .asks
            .first()
            .and_then(|l| l.first())
            .map(|s| parse_dec(s))
            .ok_or_else(|| anyhow!("OKX order book for {} has no asks", inst_id))?;
        Ok(BookTop {
            bid: parse_dec(&bid_str),
            bid_str,
            ask,
        })
    }

    /// Fetch the instrument's size constraints: (lot size, minimum size), both in
    /// base ccy. Limit orders with a size not on the lot grid are rejected.
    async fn fetch_size_spec(&self, inst_id: &str) -> Result<(Decimal, Decimal)> {
        let data: Vec<InstrumentData> = self
            .public_get(&format!(
                "/api/v5/public/instruments?instType=SPOT&instId={inst_id}"
            ))
            .await?;
        let inst = data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("OKX returned no instrument spec for {}", inst_id))?;
        let lot_sz = parse_dec(&inst.lot_sz);
        if lot_sz <= Decimal::ZERO {
            return Err(anyhow!("OKX gave non-positive lotSz for {}", inst_id));
        }
        Ok((lot_sz, parse_dec(&inst.min_sz)))
    }

    /// Post a maker-only limit buy of `sz` base asset at `px`. OKX's `post_only`
    /// order type guarantees the maker fee — but unlike Kraken (which rejects a
    /// crossing order at placement), OKX *accepts* it and immediately cancels it,
    /// so the caller must treat a fresh `canceled` state as "would have crossed,
    /// repost", not as an error.
    async fn place_post_only_limit(&self, inst_id: &str, px: &str, sz: Decimal) -> Result<String> {
        let body = serde_json::json!({
            "instId": inst_id,
            "tdMode": "cash",
            "side": "buy",
            "ordType": "post_only",
            "px": px,
            "sz": sz.normalize().to_string(),
        });
        let data: Vec<PlaceOrderData> = self
            .private_request("POST", "/api/v5/trade/order", Some(body))
            .await?;
        let placed = data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("OKX order placement returned no data"))?;
        if placed.s_code != "0" {
            return Err(anyhow!(
                "OKX rejected post-only order: code {} ({})",
                placed.s_code,
                placed.s_msg
            ));
        }
        Ok(placed.ord_id)
    }

    /// Cancel an order, tolerating the common "already filled / unknown" races.
    async fn cancel_order(&self, inst_id: &str, ord_id: &str) {
        let body = serde_json::json!({ "instId": inst_id, "ordId": ord_id });
        match self
            .private_request::<PlaceOrderData>("POST", "/api/v5/trade/cancel-order", Some(body))
            .await
        {
            Ok(data) => match data.first() {
                Some(d) if d.s_code == "0" => info!("Canceled OKX order {}", ord_id),
                Some(d) => warn!(
                    "Cancel of {} not applied (may already be filled): code {} ({})",
                    ord_id, d.s_code, d.s_msg
                ),
                None => warn!("Cancel of {} returned no data", ord_id),
            },
            Err(e) => warn!("Cancel of {} failed (may already be filled): {}", ord_id, e),
        }
    }

    /// Read back how much of `ord_id` actually filled: (qty, quote value, fee USDC).
    /// Works for any state — OKX reports `accFillSz`/`avgPx` on live, partially
    /// filled, filled and canceled orders alike.
    async fn realized_fill(&self, inst_id: &str, ord_id: &str) -> (Decimal, Decimal, Decimal) {
        match self.query_order(inst_id, ord_id).await {
            Ok(Some(o)) => {
                let qty = parse_dec(&o.acc_fill_sz);
                let avg = parse_dec(&o.avg_px);
                (qty, qty * avg, fee_to_usdc(&o.fee, &o.fee_ccy, avg))
            }
            Ok(None) => (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO),
            Err(e) => {
                warn!("Could not read fill for {}: {}", ord_id, e);
                (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO)
            }
        }
    }

    /// Buy `quote_usdc` worth of `symbol` while paying the maker fee whenever
    /// possible: rest a post-only limit at the best bid, re-peg it as the bid
    /// moves, and fall back to a market order only if the ask drifts beyond
    /// `cfg.max_drift` or `cfg.hard_timeout` elapses. Handles partial fills by
    /// accumulating across re-pegs. Same shape as the Kraken patient-maker loop;
    /// differences: sizes snap to the instrument's lot grid, and a crossing
    /// post-only lands as `canceled` (OKX) instead of being rejected at placement.
    async fn run_patient_maker_buy(
        &self,
        symbol: &str,
        quote_usdc: Decimal,
        cfg: &LimitBuyConfig,
    ) -> Result<OrderOutcome> {
        let inst = Self::okx_inst(symbol);
        let min_remaining = cfg.min_remaining; // stop once the unspent budget is dust
        let (lot_sz, min_sz) = self.fetch_size_spec(&inst).await?;

        // Reference: what a taker would pay right now. Drift is measured off this.
        let start = self.get_order_book(&inst).await?;
        let drift_ceiling = start.ask * (Decimal::ONE + cfg.max_drift);
        let deadline = Instant::now() + cfg.hard_timeout;

        info!(
            "Patient maker buy: {} USDC of {} | best bid {} / ask {} | drift ceiling {} | timeout {}s",
            quote_usdc,
            inst,
            start.bid,
            start.ask,
            drift_ceiling,
            cfg.hard_timeout.as_secs()
        );

        let mut acc_qty = Decimal::ZERO;
        let mut acc_value = Decimal::ZERO;
        let mut acc_fee = Decimal::ZERO;
        // Order ids that actually contributed a fill (composite order id).
        let mut filled_ids: Vec<String> = Vec::new();
        // The order currently resting on the book, if any: (ordId, its limit price).
        let mut resting: Option<(String, Decimal)> = None;

        loop {
            if quote_usdc - acc_value <= min_remaining {
                if let Some((ord_id, _)) = resting.take() {
                    self.cancel_order(&inst, &ord_id).await;
                    let (q, v, f) = self.realized_fill(&inst, &ord_id).await;
                    acc_qty += q;
                    acc_value += v;
                    acc_fee += f;
                    if q > Decimal::ZERO {
                        filled_ids.push(ord_id);
                    }
                }
                break;
            }

            let book = self.get_order_book(&inst).await?;

            // Give up on maker fills if price ran away or we're out of time, and
            // guarantee the fill with a market order for whatever's left.
            let drifted = book.ask > drift_ceiling;
            let timed_out = Instant::now() >= deadline;
            if drifted || timed_out {
                if let Some((ord_id, _)) = resting.take() {
                    self.cancel_order(&inst, &ord_id).await;
                    let (q, v, f) = self.realized_fill(&inst, &ord_id).await;
                    acc_qty += q;
                    acc_value += v;
                    acc_fee += f;
                    if q > Decimal::ZERO {
                        filled_ids.push(ord_id);
                    }
                }
                let remaining = quote_usdc - acc_value;
                if remaining > min_remaining {
                    warn!(
                        "{} — market fallback for remaining {} USDC (ask {}, ceiling {})",
                        if drifted { "price drift" } else { "timeout" },
                        remaining,
                        book.ask,
                        drift_ceiling
                    );
                    let fb = self.place_market_buy(symbol, remaining).await?;
                    acc_qty += fb.executed_qty;
                    acc_value += fb.executed_value;
                    acc_fee += fb.fees_usdc;
                    filled_ids.push(fb.order_id);
                }
                break;
            }

            match resting.clone() {
                None => {
                    let remaining = quote_usdc - acc_value;
                    let volume = round_to_lot(remaining / book.bid, lot_sz);
                    if volume < min_sz || volume <= Decimal::ZERO {
                        // Remainder too small for another order — realize what we have.
                        break;
                    }
                    match self
                        .place_post_only_limit(&inst, &book.bid_str, volume)
                        .await
                    {
                        Ok(ord_id) => {
                            info!(
                                "Posted maker buy {} {} @ {} (ordId {})",
                                volume, inst, book.bid_str, ord_id
                            );
                            resting = Some((ord_id, book.bid));
                        }
                        Err(e) => {
                            // Transient placement failure; re-read the book next tick.
                            warn!("Post-only placement failed, retrying: {}", e);
                        }
                    }
                }
                Some((ord_id, price)) => match self.query_order(&inst, &ord_id).await? {
                    Some(order) if order.state == "filled" => {
                        let (q, avg) = (parse_dec(&order.acc_fill_sz), parse_dec(&order.avg_px));
                        let v = q * avg;
                        acc_qty += q;
                        acc_value += v;
                        acc_fee += fee_to_usdc(&order.fee, &order.fee_ccy, avg);
                        if q > Decimal::ZERO {
                            filled_ids.push(ord_id.clone());
                        }
                        info!("Maker order {} filled: {} for {} USDC", ord_id, q, v);
                        resting = None;
                    }
                    Some(order) if order.state == "live" || order.state == "partially_filled" => {
                        // Re-peg only when the best bid has moved off our resting
                        // price; otherwise keep our queue priority and wait.
                        if book.bid != price {
                            self.cancel_order(&inst, &ord_id).await;
                            let (q, v, f) = self.realized_fill(&inst, &ord_id).await;
                            acc_qty += q;
                            acc_value += v;
                            acc_fee += f;
                            if q > Decimal::ZERO {
                                filled_ids.push(ord_id.clone());
                            }
                            info!(
                                "Bid moved {} -> {}, re-pegging (filled {} so far)",
                                price, book.bid, q
                            );
                            resting = None;
                        }
                    }
                    _ => {
                        // canceled (incl. a post-only that would have crossed) or
                        // unknown: realize any fill and repost.
                        let (q, v, f) = self.realized_fill(&inst, &ord_id).await;
                        acc_qty += q;
                        acc_value += v;
                        acc_fee += f;
                        if q > Decimal::ZERO {
                            filled_ids.push(ord_id.clone());
                        }
                        resting = None;
                    }
                },
            }

            sleep(cfg.poll_interval).await;
        }

        if acc_qty <= Decimal::ZERO {
            return Err(anyhow!("Limit buy for {} filled no quantity", symbol));
        }

        let avg_price = acc_value / acc_qty;
        info!(
            "Limit buy complete: {} {} for {} USDC (avg {}, fees {})",
            acc_qty, inst, acc_value, avg_price, acc_fee
        );
        Ok(OrderOutcome {
            order_id: filled_ids.join("+"),
            status: "FILLED".to_string(),
            executed_qty: acc_qty,
            executed_value: acc_value,
            avg_price,
            fees_usdc: acc_fee,
        })
    }

    /// Turn a filled OKX order into the bot's normalised outcome.
    fn outcome_from_order(ord_id: String, o: &OrderData) -> OrderOutcome {
        let executed_qty = parse_dec(&o.acc_fill_sz);
        let avg_price = parse_dec(&o.avg_px);
        OrderOutcome {
            order_id: ord_id,
            status: "FILLED".to_string(),
            executed_qty,
            executed_value: executed_qty * avg_price,
            avg_price,
            fees_usdc: fee_to_usdc(&o.fee, &o.fee_ccy, avg_price),
        }
    }
}

fn parse_dec(s: &str) -> Decimal {
    s.parse::<Decimal>().unwrap_or(dec!(0))
}

/// Cheap sanity check that `destination` is a raw on-chain address and not a
/// Kraken-style withdrawal key *name* (the bot's `WITHDRAWAL_WALLET_ADDRESS` holds
/// a key name like "White Ledger 2" when EXCHANGE=kraken — passing that to OKX
/// would fail only *after* the trading->funding transfer, stranding funds).
/// No address on any chain contains whitespace or is this short.
fn looks_like_address(destination: &str) -> bool {
    destination.len() >= 20 && !destination.chars().any(|c| c.is_whitespace())
}

/// Round a base-asset size down onto the instrument's lot grid (`lotSz`); OKX
/// rejects limit orders whose size isn't a lot multiple. Rounding down keeps the
/// spend at or under budget.
fn round_to_lot(sz: Decimal, lot_sz: Decimal) -> Decimal {
    if lot_sz <= Decimal::ZERO {
        return sz;
    }
    (sz / lot_sz).floor() * lot_sz
}

/// Convert an OKX order fee to USDC. OKX reports fees as *negative* numbers in
/// `feeCcy`; for a spot buy that's the base asset received, so convert at the
/// average fill price. Quote-denominated fees pass through as-is.
fn fee_to_usdc(fee: &str, fee_ccy: &str, avg_price: Decimal) -> Decimal {
    let fee = parse_dec(fee).abs();
    if fee_ccy.eq_ignore_ascii_case("USDC") {
        fee
    } else {
        fee * avg_price
    }
}

#[async_trait]
impl Exchange for OkxClient {
    fn name(&self) -> &'static str {
        "OKX"
    }

    async fn get_usdc_balance(&self) -> Result<Decimal> {
        let balance = self.trading_balance("USDC").await?;
        info!("USDC balance: {}", balance);
        Ok(balance)
    }

    async fn get_asset_balance(&self, asset: &str) -> Result<Decimal> {
        // Withdrawal sizing must see the *whole* holding: funds can sit in either
        // the trading account (where buys settle) or the funding account (after a
        // pre-withdrawal transfer, e.g. from an earlier failed withdrawal attempt).
        // Only trading would understate; the DCA core uses this for withdrawals only.
        let trading = self.trading_balance(asset).await?;
        let funding = self.funding_balance(asset).await?;
        let balance = trading + funding;
        info!(
            "{} balance: {} (trading {} + funding {})",
            asset, balance, trading, funding
        );
        Ok(balance)
    }

    async fn get_price(&self, symbol: &str) -> Result<Decimal> {
        let inst = Self::okx_inst(symbol);
        let data: Vec<TickerData> = self
            .public_get(&format!("/api/v5/market/ticker?instId={inst}"))
            .await?;
        let price = data
            .first()
            .map(|t| parse_dec(&t.last))
            .ok_or_else(|| anyhow!("OKX returned no ticker for {}", symbol))?;
        if price <= Decimal::ZERO {
            return Err(anyhow!("Invalid {} price from OKX", symbol));
        }
        info!("Current {} price {}", symbol, price);
        Ok(price)
    }

    async fn get_usdc_per_eur(&self) -> Result<Decimal> {
        // OKX quotes EUR pairs with EUR as quote: USDC-EUR gives EUR per USDC,
        // so invert it (same shape as Kraken's USDCEUR).
        let data: Vec<TickerData> = self.public_get("/api/v5/market/ticker?instId=USDC-EUR").await?;
        let eur_per_usdc = data
            .first()
            .map(|t| parse_dec(&t.last))
            .unwrap_or(Decimal::ZERO);
        if eur_per_usdc <= Decimal::ZERO {
            return Err(anyhow!("Invalid USDC-EUR price from OKX"));
        }
        Ok(Decimal::ONE / eur_per_usdc)
    }

    async fn place_market_buy(&self, symbol: &str, quote_usdc: Decimal) -> Result<OrderOutcome> {
        let inst = Self::okx_inst(symbol);

        info!("Placing OKX market buy: {} USDC of {}", quote_usdc, inst);

        // Market buys sized directly in the quote currency (tgtCcy=quote_ccy), so
        // no price-based volume conversion is needed.
        let body = serde_json::json!({
            "instId": inst,
            "tdMode": "cash",
            "side": "buy",
            "ordType": "market",
            "sz": quote_usdc.normalize().to_string(),
            "tgtCcy": "quote_ccy",
        });
        let data: Vec<PlaceOrderData> = self
            .private_request("POST", "/api/v5/trade/order", Some(body))
            .await?;
        let placed = data
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("OKX order placement returned no data"))?;
        if placed.s_code != "0" {
            return Err(anyhow!(
                "OKX rejected order: code {} ({})",
                placed.s_code,
                placed.s_msg
            ));
        }
        let ord_id = placed.ord_id;
        info!("OKX order submitted, ordId {}", ord_id);

        // Placement only returns the id; poll for the fill details.
        for attempt in 0..10 {
            if let Some(order) = self.query_order(&inst, &ord_id).await? {
                match order.state.as_str() {
                    "filled" => return Ok(Self::outcome_from_order(ord_id, &order)),
                    "canceled" | "mmp_canceled" => {
                        return Err(anyhow!("OKX order {} was {}", ord_id, order.state));
                    }
                    _ => {}
                }
            }
            if attempt < 9 {
                sleep(Duration::from_millis(1000)).await;
            }
        }

        Err(anyhow!("Timed out waiting for OKX order {} to fill", ord_id))
    }

    async fn place_limit_buy(
        &self,
        symbol: &str,
        quote_usdc: Decimal,
        cfg: &LimitBuyConfig,
    ) -> Result<OrderOutcome> {
        self.run_patient_maker_buy(symbol, quote_usdc, cfg).await
    }

    async fn get_current_month_purchases(&self, symbol: &str) -> Result<Vec<DcaPurchase>> {
        let inst = Self::okx_inst(symbol);
        let now = Utc::now();
        let start_ms = Utc
            .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
            .unwrap()
            .timestamp_millis();

        // ponytail: single page (100 orders) — far beyond a month of DCA buys;
        // paginate via the `after` cursor if that ever stops being true.
        let path = format!(
            "/api/v5/trade/orders-history-archive?instType=SPOT&instId={inst}&state=filled&begin={start_ms}"
        );
        let orders: Vec<OrderData> = self.private_request("GET", &path, None).await?;

        let mut purchases = Vec::new();
        for order in orders {
            if order.side != "buy" {
                continue;
            }
            let executed_qty = parse_dec(&order.acc_fill_sz);
            let avg_price = parse_dec(&order.avg_px);
            if executed_qty <= dec!(0) || avg_price <= dec!(0) {
                continue;
            }
            let timestamp = Utc
                .timestamp_millis_opt(parse_dec(&order.u_time).try_into().unwrap_or(0))
                .single()
                .unwrap_or_else(Utc::now);

            purchases.push(DcaPurchase {
                id: Uuid::new_v4().to_string(),
                timestamp,
                symbol: symbol.to_string(),
                side: "BUY".to_string(),
                usdc_amount: executed_qty * avg_price,
                eth_amount: executed_qty,
                eth_price: avg_price,
                fees_usdc: fee_to_usdc(&order.fee, &order.fee_ccy, avg_price),
                order_id: order.ord_id,
                status: "FILLED".to_string(),
            });
        }

        purchases.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        info!(
            "Found {} DCA purchases from current month on OKX",
            purchases.len()
        );
        Ok(purchases)
    }

    async fn verify_withdrawal(
        &self,
        asset: &str,
        destination: &str,
        network: &str,
        amount: Decimal,
    ) -> Result<bool> {
        if !looks_like_address(destination) {
            warn!(
                "OKX withdrawal destination '{}' does not look like an on-chain address \
                 (is WITHDRAWAL_WALLET_ADDRESS still a Kraken withdrawal key name?)",
                destination
            );
            return Ok(false);
        }
        let ccy = asset.to_uppercase();
        let chain = Self::okx_chain(asset, network);
        let path = format!("/api/v5/asset/currencies?ccy={ccy}");
        let chains: Vec<CurrencyChainData> = self.private_request("GET", &path, None).await?;

        match chains.iter().find(|c| c.chain == chain) {
            Some(c) if c.can_wd && amount >= parse_dec(&c.min_wd) => {
                info!(
                    "OKX withdrawal available for {} on {}: min {} (fee ~{})",
                    asset, chain, c.min_wd, c.min_fee
                );
                Ok(true)
            }
            Some(c) => {
                warn!(
                    "OKX withdrawal not available for {} {} on {} (canWd {}, minWd {})",
                    amount, asset, chain, c.can_wd, c.min_wd
                );
                Ok(false)
            }
            None => {
                warn!(
                    "OKX has no chain '{}' for {} (available: {:?})",
                    chain,
                    asset,
                    chains.iter().map(|c| c.chain.as_str()).collect::<Vec<_>>()
                );
                Ok(false)
            }
        }
    }

    async fn withdraw(
        &self,
        asset: &str,
        destination: &str,
        amount: Decimal,
        network: &str,
    ) -> Result<String> {
        // Fail before the trading->funding transfer below: a bad destination would
        // otherwise strand the funds in the funding account.
        if !looks_like_address(destination) {
            return Err(anyhow!(
                "OKX withdrawal destination '{}' does not look like an on-chain address \
                 (set WITHDRAWAL_WALLET_ADDRESS to the raw address, not a Kraken key name)",
                destination
            ));
        }
        let ccy = asset.to_uppercase();
        let chain = Self::okx_chain(asset, network);
        let amt = amount.normalize().to_string();

        // Spot buys settle in the trading account but only the funding account can
        // withdraw, so top funding up to `amount` first — transferring only the
        // shortfall, since funds may already sit in funding from an earlier attempt
        // or a manual move. Tolerate failure: the withdrawal below then fails
        // cleanly on an insufficient funding balance.
        let funding = self.funding_balance(&ccy).await.unwrap_or(Decimal::ZERO);
        let shortfall = amount - funding;
        if shortfall > Decimal::ZERO {
            let transfer = serde_json::json!({
                "ccy": ccy,
                "amt": shortfall.normalize().to_string(),
                "from": "18", // trading account
                "to": "6",    // funding account
            });
            if let Err(e) = self
                .private_request::<serde_json::Value>(
                    "POST",
                    "/api/v5/asset/transfer",
                    Some(transfer),
                )
                .await
            {
                warn!("OKX trading->funding transfer of {} failed: {}", shortfall, e);
            }
        } else {
            info!(
                "Funding account already holds {} {} — no transfer needed",
                funding, ccy
            );
        }

        info!(
            "Initiating OKX withdrawal: {} {} to {} on {}",
            amount, asset, destination, chain
        );
        // No explicit `fee` param: OKX applies the chain's default network fee.
        let body = serde_json::json!({
            "ccy": ccy,
            "amt": amt,
            "dest": "4", // on-chain withdrawal
            "toAddr": destination,
            "chain": chain,
        });
        let data: Vec<WithdrawalData> = self
            .private_request("POST", "/api/v5/asset/withdrawal", Some(body))
            .await?;
        let wd_id = data
            .into_iter()
            .next()
            .map(|d| d.wd_id)
            .ok_or_else(|| anyhow!("OKX withdrawal returned no wdId"))?;
        info!("OKX withdrawal initiated, wdId {}", wd_id);
        Ok(wd_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn okx_inst_maps_usdc_pairs() {
        assert_eq!(OkxClient::okx_inst("ETHUSDC"), "ETH-USDC");
        assert_eq!(OkxClient::okx_inst("BTCUSDC"), "BTC-USDC");
        // Non-USDC symbols pass through untouched.
        assert_eq!(OkxClient::okx_inst("ETH-EUR"), "ETH-EUR");
    }

    #[test]
    fn okx_chain_maps_bot_networks() {
        assert_eq!(OkxClient::okx_chain("ETH", "ARBITRUM"), "ETH-Arbitrum One");
        assert_eq!(OkxClient::okx_chain("BTC", "BTC"), "BTC-Bitcoin");
        assert_eq!(OkxClient::okx_chain("USDC", "ERC20"), "USDC-ERC20");
        // Unknown networks pass through so new chains need no code change.
        assert_eq!(OkxClient::okx_chain("ETH", "Optimism"), "ETH-Optimism");
    }

    #[test]
    fn fee_to_usdc_converts_base_fees_and_passes_quote_fees() {
        // OKX spot buy: fee is negative, in the base asset -> convert at avg price.
        assert_eq!(fee_to_usdc("-0.001", "ETH", dec!(2000)), dec!(2.000));
        // Quote-denominated fee passes through.
        assert_eq!(fee_to_usdc("-1.5", "USDC", dec!(2000)), dec!(1.5));
        // Garbage parses to zero rather than corrupting stats.
        assert_eq!(fee_to_usdc("", "ETH", dec!(2000)), dec!(0));
    }

    #[test]
    fn looks_like_address_rejects_kraken_key_names() {
        // The real-world trap: a Kraken withdrawal key name left in
        // WITHDRAWAL_WALLET_ADDRESS when switching EXCHANGE to okx.
        assert!(!looks_like_address("White Ledger 2"));
        assert!(!looks_like_address(""));
        // Real addresses pass (EVM and bech32).
        assert!(looks_like_address(
            "0x48AE396B932D062B559B11d8fC4D973E730af1eB"
        ));
        assert!(looks_like_address(
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"
        ));
    }

    #[test]
    fn round_to_lot_snaps_down_to_the_lot_grid() {
        // ETH-USDC lotSz is 0.000001: a raw remaining/bid quotient must round down.
        assert_eq!(
            round_to_lot(dec!(0.0026937), dec!(0.000001)),
            dec!(0.002693)
        );
        // Already on the grid: unchanged.
        assert_eq!(round_to_lot(dec!(0.002693), dec!(0.000001)), dec!(0.002693));
        // Below one lot: rounds to zero (caller treats as dust and stops).
        assert_eq!(round_to_lot(dec!(0.0000004), dec!(0.000001)), dec!(0));
        // Degenerate lot size: pass through rather than divide by zero.
        assert_eq!(round_to_lot(dec!(1.5), dec!(0)), dec!(1.5));
    }

    #[test]
    fn sign_produces_stable_base64_hmac() {
        // No published OKX reference vector; this locks the signing input layout
        // (ts + method + path + body) so a refactor can't silently reorder it.
        let client = OkxClient::new(
            "key".into(),
            "secret".into(),
            "pass".into(),
            "https://www.okx.com".into(),
        );
        let sig = client
            .sign(
                "2020-12-08T09:08:57.715Z",
                "GET",
                "/api/v5/account/balance?ccy=USDC",
                "",
            )
            .unwrap();
        assert_eq!(sig, "6uDBAO3LnTuiyRgqrZFSTk8jeSjiYYAh51GqpcMk7EU=");
    }
}
