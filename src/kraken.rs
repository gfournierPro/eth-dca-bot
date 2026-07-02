//! Kraken spot REST client.
//!
//! Mirrors the surface the DCA bot needs from an exchange (see [`crate::exchange`]).
//! Kraken differs from Binance in a few ways that are handled here so the rest of
//! the bot can stay exchange-agnostic:
//!
//! * Auth is HMAC-SHA512 over `path + SHA256(nonce + postdata)`, base64-encoded.
//! * BTC is `XBT`; USDC-quoted pairs are `ETHUSDC` / `XBTUSDC`.
//! * There is no EUR/USDC pair — the EUR rate is derived from `USDCEUR`.
//! * Market orders are sized in the base asset, and fills are read back with a
//!   follow-up `QueryOrders` call (AddOrder only returns a txid).
//! * Withdrawals target a pre-registered withdrawal key, not a raw address.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Ok, Result, anyhow};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::{Datelike, TimeZone, Utc};
use hmac::{Hmac, Mac};
use reqwest::Client;
use rust_decimal::{Decimal, RoundingStrategy};
use rust_decimal_macros::dec;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256, Sha512};
use tokio::time::{Duration, sleep};
use tracing::{info, warn};
use uuid::Uuid;

use crate::dca_stats_mongo::DcaPurchase;
use crate::exchange::{Exchange, OrderOutcome};

type HmacSha512 = Hmac<Sha512>;

/// Process-wide monotonic nonce source. Kraken requires each request's nonce to be
/// strictly greater than the previous one for the same API key.
static NONCE: AtomicU64 = AtomicU64::new(0);

fn next_nonce() -> u64 {
    let now = Utc::now().timestamp_millis() as u64;
    loop {
        let prev = NONCE.load(Ordering::SeqCst);
        let cand = if now > prev { now } else { prev + 1 };
        if NONCE
            .compare_exchange(prev, cand, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return cand;
        }
    }
}

#[derive(Debug, Clone)]
pub struct KrakenClient {
    client: Client,
    api_key: String,
    secret_key: String,
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct KrakenEnvelope<T> {
    #[serde(default)]
    error: Vec<String>,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
struct TickerInfo {
    /// Last trade closed array: [price, lot volume].
    c: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AddOrderResult {
    txid: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OrderDescr {
    pair: String,
    #[serde(rename = "type")]
    otype: String,
}

#[derive(Debug, Deserialize)]
struct KrakenOrderInfo {
    status: String,
    #[serde(default)]
    vol_exec: String,
    #[serde(default)]
    cost: String,
    #[serde(default)]
    fee: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    closetm: f64,
    descr: OrderDescr,
}

#[derive(Debug, Deserialize)]
struct ClosedOrdersResult {
    closed: HashMap<String, KrakenOrderInfo>,
}

#[derive(Debug, Deserialize)]
struct WithdrawInfoResult {
    #[serde(default)]
    limit: String,
    #[serde(default)]
    fee: String,
    #[serde(default)]
    amount: String,
}

#[derive(Debug, Deserialize)]
struct WithdrawResult {
    refid: String,
}

impl KrakenClient {
    pub fn new(api_key: String, secret_key: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            secret_key,
            base_url,
        }
    }

    /// Translate a generic USDC pair symbol (`ETHUSDC`, `BTCUSDC`) into Kraken's
    /// altname, which uses `XBT` for Bitcoin.
    fn kraken_pair(symbol: &str) -> String {
        symbol.replace("BTC", "XBT")
    }

    /// Candidate balance map keys for an asset. Kraken uses the `X`/`Z` prefixed
    /// legacy codes for some assets but plain codes for newer ones, so try both.
    fn balance_keys(asset: &str) -> Vec<&'static str> {
        match asset.to_uppercase().as_str() {
            "ETH" => vec!["XETH", "ETH"],
            "BTC" | "XBT" => vec!["XXBT", "XBT"],
            "USDC" => vec!["USDC"],
            "EUR" => vec!["ZEUR", "EUR"],
            "USD" => vec!["ZUSD", "USD"],
            _ => vec![],
        }
    }

    /// Asset code used by funding endpoints (withdraw). BTC is `XBT`.
    fn withdraw_asset_code(asset: &str) -> String {
        match asset.to_uppercase().as_str() {
            "BTC" => "XBT".to_string(),
            other => other.to_string(),
        }
    }

    /// Sign a private request: base64(HMAC-SHA512(secret, path + SHA256(nonce + postdata))).
    fn sign(&self, path: &str, nonce: &str, postdata: &str) -> Result<String> {
        let secret = BASE64
            .decode(self.secret_key.trim())
            .map_err(|e| anyhow!("Invalid Kraken API secret (not base64): {}", e))?;

        let mut sha = Sha256::new();
        sha.update(nonce.as_bytes());
        sha.update(postdata.as_bytes());
        let sha_digest = sha.finalize();

        let mut mac = HmacSha512::new_from_slice(&secret)
            .map_err(|e| anyhow!("Failed to init Kraken HMAC: {}", e))?;
        mac.update(path.as_bytes());
        mac.update(&sha_digest);
        Ok(BASE64.encode(mac.finalize().into_bytes()))
    }

    async fn public_get<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let url = format!("{}/0{}", self.base_url, endpoint);
        let response = self.client.get(&url).query(query).send().await?;
        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(anyhow!("Kraken public request to {} failed: {}", endpoint, text));
        }
        let env: KrakenEnvelope<T> = response.json().await?;
        if !env.error.is_empty() {
            return Err(anyhow!("Kraken API error: {}", env.error.join("; ")));
        }
        env.result
            .ok_or_else(|| anyhow!("Kraken API returned no result for {}", endpoint))
    }

    async fn private_post<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        mut params: Vec<(String, String)>,
    ) -> Result<T> {
        let nonce = next_nonce().to_string();
        params.insert(0, ("nonce".to_string(), nonce.clone()));

        let postdata = serde_urlencoded::to_string(&params)?;
        let path = format!("/0{}", endpoint);
        let signature = self.sign(&path, &nonce, &postdata)?;
        let url = format!("{}{}", self.base_url, path);

        let response = self
            .client
            .post(&url)
            .header("API-Key", &self.api_key)
            .header("API-Sign", signature)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(postdata)
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(anyhow!("Kraken private request to {} failed: {}", endpoint, text));
        }

        let env: KrakenEnvelope<T> = response.json().await?;
        if !env.error.is_empty() {
            return Err(anyhow!("Kraken API error: {}", env.error.join("; ")));
        }
        env.result
            .ok_or_else(|| anyhow!("Kraken API returned no result for {}", endpoint))
    }

    async fn get_balances(&self) -> Result<HashMap<String, String>> {
        self.private_post("/private/Balance", Vec::new()).await
    }

    /// Fetch a single order by txid, or `None` if Kraken doesn't return it.
    async fn query_order(&self, txid: &str) -> Result<Option<KrakenOrderInfo>> {
        let mut orders: HashMap<String, KrakenOrderInfo> = self
            .private_post(
                "/private/QueryOrders",
                vec![("txid".to_string(), txid.to_string())],
            )
            .await?;
        Ok(orders.remove(txid))
    }
}

fn parse_dec(s: &str) -> Decimal {
    s.parse::<Decimal>().unwrap_or(dec!(0))
}

#[async_trait]
impl Exchange for KrakenClient {
    fn name(&self) -> &'static str {
        "Kraken"
    }

    async fn get_usdc_balance(&self) -> Result<Decimal> {
        let balances = self.get_balances().await?;
        let balance = balances
            .get("USDC")
            .map(|b| parse_dec(b))
            .unwrap_or(Decimal::ZERO);
        info!("USDC balance: {}", balance);
        Ok(balance)
    }

    async fn get_asset_balance(&self, asset: &str) -> Result<Decimal> {
        let balances = self.get_balances().await?;
        for key in Self::balance_keys(asset) {
            if let Some(b) = balances.get(key) {
                let balance = parse_dec(b);
                info!("{} balance: {} (key {})", asset, balance, key);
                return Ok(balance);
            }
        }
        warn!("{} balance not found, returning 0", asset);
        Ok(Decimal::ZERO)
    }

    async fn get_price(&self, symbol: &str) -> Result<Decimal> {
        let pair = Self::kraken_pair(symbol);
        let result: HashMap<String, TickerInfo> = self
            .public_get("/public/Ticker", &[("pair", pair.as_str())])
            .await?;
        // Kraken keys the result by its canonical pair name, which may differ from
        // the requested altname, so just take the single entry returned.
        let info = result
            .into_values()
            .next()
            .ok_or_else(|| anyhow!("Kraken returned no ticker for {}", symbol))?;
        let price_str = info
            .c
            .first()
            .ok_or_else(|| anyhow!("Kraken ticker for {} missing last price", symbol))?;
        let price = parse_dec(price_str);
        info!("Current {} price {}", symbol, price);
        Ok(price)
    }

    async fn get_usdc_per_eur(&self) -> Result<Decimal> {
        // Kraken has no EUR/USDC pair; USDCEUR gives EUR per USDC, so invert it.
        let result: HashMap<String, TickerInfo> = self
            .public_get("/public/Ticker", &[("pair", "USDCEUR")])
            .await?;
        let info = result
            .into_values()
            .next()
            .ok_or_else(|| anyhow!("Kraken returned no ticker for USDCEUR"))?;
        let eur_per_usdc = info.c.first().map(|s| parse_dec(s)).unwrap_or(Decimal::ZERO);
        if eur_per_usdc <= Decimal::ZERO {
            return Err(anyhow!("Invalid USDCEUR price from Kraken"));
        }
        Ok(Decimal::ONE / eur_per_usdc)
    }

    async fn place_market_buy(&self, symbol: &str, quote_usdc: Decimal) -> Result<OrderOutcome> {
        let pair = Self::kraken_pair(symbol);

        // Kraken market buys are sized in the base asset, so convert the target USDC
        // spend into a base volume at the current price. Round down to 8 dp to avoid
        // overshooting the intended spend.
        let price = self.get_price(symbol).await?;
        if price <= Decimal::ZERO {
            return Err(anyhow!("Cannot size order: non-positive price for {}", symbol));
        }
        let volume = (quote_usdc / price).round_dp_with_strategy(8, RoundingStrategy::ToZero);
        if volume <= Decimal::ZERO {
            return Err(anyhow!("Computed order volume is zero for {} at {}", symbol, price));
        }

        info!(
            "Placing Kraken market buy: {} {} (~{} USDC at {})",
            volume, pair, quote_usdc, price
        );

        let add: AddOrderResult = self
            .private_post(
                "/private/AddOrder",
                vec![
                    ("pair".to_string(), pair.clone()),
                    ("type".to_string(), "buy".to_string()),
                    ("ordertype".to_string(), "market".to_string()),
                    ("volume".to_string(), volume.to_string()),
                ],
            )
            .await?;

        let txid = add
            .txid
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Kraken AddOrder returned no txid"))?;
        info!("Kraken order submitted, txid {}", txid);

        // AddOrder only returns a txid; poll QueryOrders for the fill details.
        for attempt in 0..10 {
            if let Some(order) = self.query_order(&txid).await? {
                match order.status.as_str() {
                    "closed" => {
                        let executed_qty = parse_dec(&order.vol_exec);
                        let executed_value = parse_dec(&order.cost);
                        let fees_usdc = parse_dec(&order.fee);
                        let mut avg_price = parse_dec(&order.price);
                        if avg_price <= Decimal::ZERO && executed_qty > Decimal::ZERO {
                            avg_price = executed_value / executed_qty;
                        }
                        return std::result::Result::Ok(OrderOutcome {
                            order_id: txid,
                            status: "FILLED".to_string(),
                            executed_qty,
                            executed_value,
                            avg_price,
                            fees_usdc,
                        });
                    }
                    "canceled" | "expired" => {
                        return Err(anyhow!("Kraken order {} was {}", txid, order.status));
                    }
                    _ => {}
                }
            }
            if attempt < 9 {
                sleep(Duration::from_millis(1000)).await;
            }
        }

        Err(anyhow!("Timed out waiting for Kraken order {} to fill", txid))
    }

    async fn get_current_month_purchases(&self, symbol: &str) -> Result<Vec<DcaPurchase>> {
        let pair = Self::kraken_pair(symbol);
        let now = Utc::now();
        let start_of_month = Utc
            .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
            .unwrap();
        let start_secs = start_of_month.timestamp();

        let result: ClosedOrdersResult = self
            .private_post(
                "/private/ClosedOrders",
                vec![("start".to_string(), start_secs.to_string())],
            )
            .await?;

        let mut purchases = Vec::new();
        for (txid, order) in result.closed {
            if order.status != "closed"
                || order.descr.otype != "buy"
                || order.descr.pair != pair
            {
                continue;
            }
            let executed_qty = parse_dec(&order.vol_exec);
            let executed_value = parse_dec(&order.cost);
            if executed_qty <= dec!(0) || executed_value <= dec!(0) {
                continue;
            }
            let mut avg_price = parse_dec(&order.price);
            if avg_price <= dec!(0) {
                avg_price = executed_value / executed_qty;
            }
            let timestamp = Utc
                .timestamp_millis_opt((order.closetm * 1000.0) as i64)
                .single()
                .unwrap_or_else(Utc::now);

            purchases.push(DcaPurchase {
                id: Uuid::new_v4().to_string(),
                timestamp,
                symbol: symbol.to_string(),
                usdc_amount: executed_value,
                eth_amount: executed_qty,
                eth_price: avg_price,
                fees_usdc: parse_dec(&order.fee),
                order_id: txid,
                // Normalise Kraken's "closed" to the "FILLED" the stats DB expects.
                status: "FILLED".to_string(),
            });
        }

        purchases.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        info!("Found {} DCA purchases from current month on Kraken", purchases.len());
        Ok(purchases)
    }

    async fn verify_withdrawal(
        &self,
        asset: &str,
        destination: &str,
        _network: &str,
        amount: Decimal,
    ) -> Result<bool> {
        // Kraken has no arbitrary-address withdrawal; `destination` is a saved
        // withdrawal key. WithdrawInfo both validates the key and reports limits/fees.
        let asset_code = Self::withdraw_asset_code(asset);
        match self
            .private_post::<WithdrawInfoResult>(
                "/private/WithdrawInfo",
                vec![
                    ("asset".to_string(), asset_code),
                    ("key".to_string(), destination.to_string()),
                    ("amount".to_string(), amount.to_string()),
                ],
            )
            .await
        {
            std::result::Result::Ok(info) => {
                info!(
                    "Kraken withdrawal available for {}: net {} (fee {}, limit {})",
                    asset, info.amount, info.fee, info.limit
                );
                Ok(true)
            }
            std::result::Result::Err(e) => {
                warn!("Kraken withdrawal not available for {} via key '{}': {}", asset, destination, e);
                Ok(false)
            }
        }
    }

    async fn withdraw(
        &self,
        asset: &str,
        destination: &str,
        amount: Decimal,
        _network: &str,
    ) -> Result<String> {
        let asset_code = Self::withdraw_asset_code(asset);
        info!(
            "Initiating Kraken withdrawal: {} {} to key '{}'",
            amount, asset, destination
        );
        let result: WithdrawResult = self
            .private_post(
                "/private/Withdraw",
                vec![
                    ("asset".to_string(), asset_code),
                    ("key".to_string(), destination.to_string()),
                    ("amount".to_string(), amount.to_string()),
                ],
            )
            .await?;
        info!("Kraken withdrawal initiated, refid {}", result.refid);
        Ok(result.refid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_matches_kraken_reference_vector() {
        // Reference test vector published by Kraken for the API-Sign header.
        let client = KrakenClient::new(
            "api-key".to_string(),
            "kQH5HW/8p1uGOVjbgWA7FunAmGO8lsSUXNsu3eow76sz84Q18fWxnyRzBHCd3pd5nE9qa99HAZtuZuj6F1huXg=="
                .to_string(),
            "https://api.kraken.com".to_string(),
        );
        let nonce = "1616492376594";
        let postdata = "nonce=1616492376594&ordertype=limit&pair=XBTUSD&price=37500&type=buy&volume=1.25";
        let sig = client.sign("/0/private/AddOrder", nonce, postdata).unwrap();
        assert_eq!(
            sig,
            "4/dpxb3iT4tp/ZCVEwSnEsLxx0bqyhLpdfOpc6fn7OR8+UClSV5n9E6aSS8MPtnRfp32bAb0nmbRn6H8ndwLUQ=="
        );
    }

    #[test]
    fn kraken_pair_maps_btc_to_xbt() {
        assert_eq!(KrakenClient::kraken_pair("BTCUSDC"), "XBTUSDC");
        assert_eq!(KrakenClient::kraken_pair("ETHUSDC"), "ETHUSDC");
    }
}
