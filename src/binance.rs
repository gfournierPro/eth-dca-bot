use serde::Deserialize;
use std::collections::HashMap;

use anyhow::{Ok, Result, anyhow};
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::Client;
use rust_decimal::Decimal;
use sha2::Sha256;
use tracing::{error, info, warn};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct BinanceClient {
    client: Client,
    api_key: String,
    secret_key: String,
    base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct AccountInfo {
    pub balances: Vec<Balance>,
}

#[derive(Debug, Deserialize)]
pub struct Balance {
    pub asset: String,
    pub free: String,
    pub locked: String,
}

#[derive(Debug, Deserialize)]
pub struct OrderResponse {
    pub symbol: String,
    #[serde(rename = "orderId")]
    pub order_id: u64,
    pub status: String,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
    #[serde(rename = "executedQty")]
    pub executed_qty: String,
    #[serde(rename = "cummulativeQuoteQty")]
    pub cummulative_quote_qty: String,
    pub fills: Option<Vec<Fill>>,
}

pub struct Fill {
    pub price: String,
    pub qty: String,
    pub commission: String,
    #[serde(rename = "commissionAsset")]
    pub commission_asset: String,
}

#[derive(Debug, Deserialize)]
pub struct TickerPrice {
    pub symbol: String,
    pub price: String,
}

impl BinanceClient {
    pub fn new(api_key: String, secret_key: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            secret_key,
            base_url,
        }
    }

    fn create_signature(&self, query_string: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(self.secret_key.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(query_string.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }
    async fn signed_request<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        endpoint: &str,
        params: HashMap<String, String>,
    ) -> Result<T> {
        let mut params = params;
        params.insert(
            "timestamp".to_string(),
            Utc::now().timestamp_millis().to_string(),
        );

        let query_string = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        let signature = self.create_signature(&query_string);
        let url = format!(
            "{}{}?{}&signature={}",
            self.base_url, endpoint, query_string, signature
        );

        let response = match method {
            "GET" => {
                self.client
                    .get(&url)
                    .header("X-MBX-APIKEY", &self.api_key)
                    .send()
                    .await?
            }
            "POST" => {
                self.client
                    .post(&url)
                    .header("X-MBX-APIKEY", &self.api_key)
                    .send()
                    .await?
            }
            _ => return Err(anyhow!("Unsupported HTTP method: {}", method)),
        };
        if !response.status().is_success() {
            let error_text = response.text().await?;
            error!("Binance API request failed: {}", error_text);
            return Err(anyhow!("Binance API request failed: {}", error_text));
        }
        let result = response.json().await?;
        Ok(result)
    }

    pub async fn get_account_info(&self) -> Result<AccountInfo> {
        info!("Fetching account balance information");
        self.signed_request("GET", "/api/v3/account", HashMap::new())
            .await
    }

    pub async fn get_usdc_balanc(&self) -> Result<Decimal> {
        let account_info = self.get_account_info().await?;

        for balance in account_info.balances {
            if balance.asset == "USDC" {
                let free_balance = balance.free.parse::<Decimal>()?;
                info!("USDC balance: {}", free_balance);
                return Ok(free_balance);
            }
        }
        warn!("USDC balance not found, returning 0");
        Ok(Decimal::ZERO)
    }

    pub async fn get_symbol_price(&self, symbol: &str) -> Result<Decimal> {
        let url = format!("{}/api/v3/ticker/price?symbol={}", self.base_url, symbol);
        let response: TickerPrice = self.client.get(&url).send().await?.json().await?;

        let price = response.price.parse::<Decimal>()?;
        info!("Current {} price {}", symbol, price);
        Ok(price)
    }
    pub async fn place_market_buy_order(
        &self,
        symbol: &str,
        quote_order_qty: Decimal,
    ) -> Result<OrderResponse> {
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        params.insert("side".to_string(), "BUY".to_string());
        params.insert("type".to_string(), "MARKET".to_string());
        params.insert("quoteOrderQty".to_string(), quote_order_qty.to_string());
        info!(
            "Placing market buy order for {} {} worth of {}",
            quote_order_qty, "USDC", symbol
        );
        let order_response: OrderResponse =
            self.signed_request("POST", "/api/v3/order", params).await?;
        info!(
            "Order placed successfully. Order ID: {}, Status: {}",
            order_response.order_id, order_response.status
        );
        Ok(order_response)
    }
}

impl OrderResponse {
    pub fn calculate_total_fees_in_usdc(&self, eth_price: Decimal) -> Decimal {
        if let Some(fills) = &self.fills {
            let mut total_fee_usdc = dec!(0);

            for fill in fills {
                let commission: Decimal = fill.commission.parse().unwrap_or(dec!(0));

                match fill.commission_asset.as_str() {
                    "USDC" => total_fee_usdc += commission,
                    "ETH" => total_fee_usdc += commission * eth_price,
                    "BNB" => {
                        // You might want to get BNB price and convert
                        // For now, we'll estimate BNB at ~$300 (update as needed)
                        total_fee_usdc += commission * dec!(300);
                    }
                    _ => {
                        // Unknown asset, log warning
                        tracing::warn!("Unknown commission asset: {}", fill.commission_asset);
                    }
                }
            }
            total_fee_usdc
        } else {
            // Fallback: estimate 0.1% trading fee if no fills data
            let quote_qty: Decimal = self.cummulative_quote_qty.parse().unwrap_or(dec!(0));
            quote_qty * dec!(0.001)
        }
    }
}
