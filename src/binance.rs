use serde::Deserialize;
use std::collections::HashMap;

use anyhow::{Ok, Result, anyhow};
use chrono::{Utc, Datelike, TimeZone};
use hmac::{Hmac, Mac};
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sha2::Sha256;
use tracing::{error, info, warn};
use crate::dca_stats_mongo::DcaPurchase;
use uuid::Uuid;

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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
pub struct Order {
    pub symbol: String,
    #[serde(rename = "orderId")]
    pub order_id: u64,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub status: String,
    #[serde(rename = "origQty")]
    pub orig_qty: String,
    #[serde(rename = "executedQty")]
    pub executed_qty: String,
    #[serde(rename = "cummulativeQuoteQty")]
    pub cummulative_quote_qty: String,
    pub price: String,
    pub time: i64,
    #[serde(rename = "updateTime")]
    pub update_time: i64,
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

    pub async fn get_order_history(
        &self,
        symbol: &str,
        start_time: Option<i64>,
        end_time: Option<i64>,
        limit: Option<u16>,
    ) -> Result<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        
        if let Some(start) = start_time {
            params.insert("startTime".to_string(), start.to_string());
        }
        
        if let Some(end) = end_time {
            params.insert("endTime".to_string(), end.to_string());
        }
        
        if let Some(lim) = limit {
            params.insert("limit".to_string(), lim.to_string());
        }

        info!("Fetching order history for symbol: {}", symbol);
        let orders: Vec<Order> = self
            .signed_request("GET", "/api/v3/allOrders", params)
            .await?;
        
        info!("Retrieved {} orders from Binance", orders.len());
        Ok(orders)
    }

    pub async fn get_current_month_purchases(&self, symbol: &str) -> Result<Vec<DcaPurchase>> {
        let now = Utc::now();
        let start_of_month = Utc
            .with_ymd_and_hms(now.year() as i32, now.month(), 1, 0, 0, 0)
            .unwrap();
        let start_timestamp = start_of_month.timestamp_millis();
        
        let orders = self.get_order_history(
            symbol,
            Some(start_timestamp),
            None,
            Some(1000), // Limit to 1000 orders
        ).await?;

        let mut purchases = Vec::new();
        
        for order in orders {
            // Only process filled buy orders
            if order.status == "FILLED" && order.side == "BUY" {
                let executed_qty: Decimal = order.executed_qty.parse().unwrap_or(dec!(0));
                let executed_value: Decimal = order.cummulative_quote_qty.parse().unwrap_or(dec!(0));
                
                if executed_qty > dec!(0) && executed_value > dec!(0) {
                    let average_price = executed_value / executed_qty;
                    let timestamp = Utc.timestamp_millis_opt(order.time).unwrap();
                    
                    // Estimate fees as 0.1% of trade value since we don't have fill details
                    let estimated_fees = executed_value * dec!(0.001);
                    
                    let purchase = DcaPurchase {
                        id: Uuid::new_v4().to_string(),
                        timestamp,
                        symbol: order.symbol.clone(),
                        usdc_amount: executed_value,
                        eth_amount: executed_qty,
                        eth_price: average_price,
                        fees_usdc: estimated_fees,
                        order_id: order.order_id,
                        status: order.status.clone(),
                    };
                    
                    purchases.push(purchase);
                }
            }
        }
        
        // Sort by timestamp in descending order (most recent first)
        purchases.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        
        info!("Found {} DCA purchases from current month on Binance", purchases.len());
        Ok(purchases)
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
