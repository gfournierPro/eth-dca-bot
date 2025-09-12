use serde::Deserialize;
use std::collections::HashMap;

use anyhow::{Ok, Result, anyhow};
use chrono::{Utc, Datelike, TimeZone, DateTime};
use hmac::{Hmac, Mac};
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use sha2::Sha256;
use tracing::{error, info, warn};
use crate::dca_stats_mongo::DcaPurchase;
use uuid::Uuid;
use std::sync::Arc;
use tokio::sync::RwLock;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct BinanceClient {
    client: Client,
    api_key: String,
    secret_key: String,
    base_url: String,
    time_offset: Arc<RwLock<i64>>, // Offset in milliseconds between local time and server time
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

#[derive(Debug, Deserialize)]
pub struct WithdrawResponse {
    pub id: String,
    #[serde(rename = "withdrawOrderId")]
    pub withdraw_order_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WithdrawHistory {
    #[serde(rename = "withdrawOrderId")]
    pub withdraw_order_id: String,
    pub amount: String,
    #[serde(rename = "transactionFee")]
    pub transaction_fee: String,
    pub address: String,
    pub asset: String,
    #[serde(rename = "txId")]
    pub tx_id: String,
    #[serde(rename = "applyTime")]
    pub apply_time: i64,
    pub status: i32,
    pub network: String,
}

#[derive(Debug, Deserialize)]
pub struct ServerTime {
    #[serde(rename = "serverTime")]
    pub server_time: i64,
}

impl BinanceClient {
    pub fn new(api_key: String, secret_key: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            secret_key,
            base_url,
            time_offset: Arc::new(RwLock::new(0)),
        }
    }

    async fn get_server_time(&self) -> Result<i64> {
        let url = format!("{}/api/v3/time", self.base_url);
        let response = self.client.get(&url).send().await?;
        
        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to get server time: {}", error_text));
        }
        
        let server_time: ServerTime = response.json().await?;
        Ok(server_time.server_time)
    }

    pub async fn sync_time(&self) -> Result<()> {
        info!("Synchronizing time with Binance servers...");
        let server_time = self.get_server_time().await?;
        let local_time = Utc::now().timestamp_millis();
        let offset = server_time - local_time;
        
        info!("Time sync - Server: {}, Local: {}, Offset: {}ms", 
              server_time, local_time, offset);
        
        *self.time_offset.write().await = offset;
        Ok(())
    }

    async fn get_synchronized_timestamp(&self) -> i64 {
        let local_time = Utc::now().timestamp_millis();
        let offset = *self.time_offset.read().await;
        local_time + offset
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
            self.get_synchronized_timestamp().await.to_string(),
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

    pub async fn get_eth_balance(&self) -> Result<Decimal> {
        let account_info = self.get_account_info().await?;

        for balance in account_info.balances {
            if balance.asset == "ETH" {
                let free_balance = balance.free.parse::<Decimal>()?;
                info!("ETH balance: {}", free_balance);
                return Ok(free_balance);
            }
        }
        warn!("ETH balance not found, returning 0");
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
        // Round quote order quantity to 2 decimal places for USDC pairs
        // Binance requires specific precision for quote order quantities
        let rounded_qty = quote_order_qty.round_dp(2);
        
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        params.insert("side".to_string(), "BUY".to_string());
        params.insert("type".to_string(), "MARKET".to_string());
        params.insert("quoteOrderQty".to_string(), rounded_qty.to_string());
        info!(
            "Placing market buy order for {} {} worth of {} (rounded from {})",
            rounded_qty, "USDC", symbol, quote_order_qty
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
                        side: order.side.clone(),
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

    /// Get all DCA orders (both purchases and sales) from Binance starting from a specific date and optionally from a minimum order ID
    /// This is useful for syncing historical data with the database
    pub async fn get_historical_dca_orders(
        &self,
        symbol: &str,
        start_date: DateTime<Utc>,
        min_order_id: Option<u64>,
    ) -> Result<Vec<DcaPurchase>> {
        let start_timestamp = start_date.timestamp_millis();
        
        info!("🔍 Fetching historical DCA orders for {} from {}", symbol, start_date.format("%Y-%m-%d %H:%M:%S UTC"));
        if let Some(min_id) = min_order_id {
            info!("📋 Filtering orders from minimum order ID: {}", min_id);
        }
        
        let orders = self.get_order_history(
            symbol,
            Some(start_timestamp),
            None,
            Some(1000), // Limit to 1000 orders
        ).await?;

        let mut purchases = Vec::new();
        
        for order in orders {
            // Skip orders before the minimum order ID if specified
            if let Some(min_id) = min_order_id {
                if order.order_id < min_id {
                    continue;
                }
            }
            
            // Process both filled buy and sell orders
            if order.status == "FILLED" && (order.side == "BUY" || order.side == "SELL") {
                let executed_qty: Decimal = order.executed_qty.parse().unwrap_or(dec!(0));
                let executed_value: Decimal = order.cummulative_quote_qty.parse().unwrap_or(dec!(0));
                
                if executed_qty > dec!(0) && executed_value > dec!(0) {
                    let average_price = executed_value / executed_qty;
                    let timestamp = match Utc.timestamp_millis_opt(order.time) {
                        chrono::LocalResult::Single(dt) => dt,
                        _ => {
                            warn!("Invalid timestamp for order {}, using current time", order.order_id);
                            Utc::now()
                        }
                    };
                    
                    // Estimate fees as 0.1% of trade value since we don't have fill details
                    let estimated_fees = executed_value * dec!(0.001);
                    
                    let purchase = DcaPurchase {
                        id: Uuid::new_v4().to_string(),
                        timestamp,
                        symbol: order.symbol.clone(),
                        side: order.side.clone(),
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
        
        // Sort by timestamp (oldest first for historical sync)
        purchases.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        
        info!("✅ Found {} historical DCA orders from Binance", purchases.len());
        Ok(purchases)
    }

    pub async fn withdraw_eth(
        &self,
        address: &str,
        amount: Decimal,
        network: &str,
    ) -> Result<WithdrawResponse> {
        let mut params = HashMap::new();
        params.insert("coin".to_string(), "ETH".to_string());
        params.insert("address".to_string(), address.to_string());
        params.insert("amount".to_string(), amount.to_string());
        params.insert("network".to_string(), network.to_string());

        info!(
            "Initiating ETH withdrawal: {} ETH to {} on {} network",
            amount, address, network
        );

        let result = self.signed_request::<WithdrawResponse>("POST", "/sapi/v1/capital/withdraw/apply", params).await;
        
        match result {
            std::result::Result::Ok(response) => {
                info!(
                    "Withdrawal initiated successfully. ID: {}",
                    response.id
                );
                Ok(response)
            }
            std::result::Result::Err(e) => {
                let error_msg = format!("{}", e);
                if error_msg.contains("-4019") {
                    error!("❌ ETH withdrawal not available. Possible reasons:");
                    error!("   • Withdrawals disabled for your account/region");
                    error!("   • API key lacks withdrawal permissions");
                    error!("   • Account verification incomplete");
                    error!("   • Security restrictions (2FA, email confirmation)");
                    error!("   • Network '{}' not supported for ETH", network);
                    error!("💡 Check your Binance account settings and API permissions");
                } else if error_msg.contains("-1013") {
                    error!("❌ Invalid withdrawal amount or network configuration");
                } else if error_msg.contains("-4008") {
                    error!("❌ Withdrawal address not whitelisted");
                }
                Err(e)
            }
        }
    }

    pub async fn get_withdraw_history(
        &self,
        coin: &str,
        start_time: Option<i64>,
        end_time: Option<i64>,
        limit: Option<u16>,
    ) -> Result<Vec<WithdrawHistory>> {
        let mut params = HashMap::new();
        params.insert("coin".to_string(), coin.to_string());
        
        if let Some(start) = start_time {
            params.insert("startTime".to_string(), start.to_string());
        }
        
        if let Some(end) = end_time {
            params.insert("endTime".to_string(), end.to_string());
        }
        
        if let Some(lim) = limit {
            params.insert("limit".to_string(), lim.to_string());
        }

        info!("Fetching withdrawal history for {}", coin);
        let withdrawals: Vec<WithdrawHistory> = self
            .signed_request("GET", "/sapi/v1/capital/withdraw/history", params)
            .await?;
        
        info!("Retrieved {} withdrawal records", withdrawals.len());
        Ok(withdrawals)
    }

    pub async fn check_withdrawal_capability(&self, coin: &str) -> Result<bool> {
        info!("Checking withdrawal capability for {}", coin);
        
        let params = HashMap::new();
        
        match self.signed_request::<serde_json::Value>("GET", "/sapi/v1/capital/config/getall", params).await {
            std::result::Result::Ok(response) => {
                // Parse the response to check if the coin supports withdrawals
                if let Some(coins) = response.as_array() {
                    for coin_info in coins.iter() {
                        if let Some(coin_name) = coin_info.get("coin").and_then(|c| c.as_str()) {
                            if coin_name == coin {
                                
                                // Check the overall withdrawal capability first
                                if let Some(withdraw_all_enable) = coin_info.get("withdrawAllEnable").and_then(|w| w.as_bool()) {
                                    if !withdraw_all_enable {
                                        info!("Withdrawal disabled for {} at coin level", coin);
                                        return Ok(false);
                                    }
                                } else {
                                    warn!("withdrawAllEnable field not found for {}", coin);
                                }
                                
                                // Check if there are network-specific settings
                                if let Some(network_list) = coin_info.get("networkList").and_then(|n| n.as_array()) {
                                    // Log all available networks
                                    let mut networks = Vec::new();
                                    for network in network_list {
                                        if let Some(network_name) = network.get("network").and_then(|n| n.as_str()) {
                                            if let Some(withdraw_enable) = network.get("withdrawEnable").and_then(|w| w.as_bool()) {
                                                networks.push(format!("{}: {}", network_name, if withdraw_enable { "✅" } else { "❌" }));
                                            }
                                        }
                                    }
                                    return Ok(true);
                                } else {
                                    // Fallback to the old logic if no networkList
                                    if let Some(withdraw_enable) = coin_info.get("withdrawEnable").and_then(|w| w.as_bool()) {
                                        info!("Withdrawal enabled for {}: {}", coin, withdraw_enable);
                                        return Ok(withdraw_enable);
                                    } else {
                                        warn!("No networkList and no withdrawEnable field found for {}", coin);
                                        return Ok(false);
                                    }
                                }
                            }
                        }
                    }
                    
                    // If we didn't find the coin, let's list all available coins
                    info!("🔍 Coin '{}' not found", coin);
                } else {
                    warn!("Response is not an array");
                    info!("Response type: {:?}", response);
                }
                
                warn!("Could not determine withdrawal capability for {}", coin);
                Ok(false)
            }
            std::result::Result::Err(e) => {
                warn!("Failed to check withdrawal capability: {}", e);
                // Return false but don't fail completely
                Ok(false)
            }
        }
    }

    pub async fn check_network_withdrawal_capability(&self, coin: &str, network: &str) -> Result<bool> {
        let params = HashMap::new();
        
        match self.signed_request::<serde_json::Value>("GET", "/sapi/v1/capital/config/getall", params).await {
            std::result::Result::Ok(response) => {
                if let Some(coins) = response.as_array() {
                    for coin_info in coins {
                        if let Some(coin_name) = coin_info.get("coin").and_then(|c| c.as_str()) {
                            if coin_name == coin {
                                // Check network-specific withdrawal capability
                                if let Some(network_list) = coin_info.get("networkList").and_then(|n| n.as_array()) {
                                    for network_info in network_list {
                                        if let Some(network_name) = network_info.get("network").and_then(|n| n.as_str()) {
                                            if network_name == network {
                                                if let Some(withdraw_enable) = network_info.get("withdrawEnable").and_then(|w| w.as_bool()) {
                                                    info!("Network {} withdrawal for {}: {}", network, coin, withdraw_enable);
                                                    
                                                    // Also log withdrawal limits and fees
                                                    if let Some(withdraw_min) = network_info.get("withdrawMin").and_then(|w| w.as_str()) {
                                                        info!("Minimum withdrawal: {} {}", withdraw_min, coin);
                                                    }
                                                    if let Some(withdraw_fee) = network_info.get("withdrawFee").and_then(|w| w.as_str()) {
                                                        info!("Withdrawal fee: {} {}", withdraw_fee, coin);
                                                    }
                                                    
                                                    return Ok(withdraw_enable);
                                                }
                                            }
                                        }
                                    }
                                }
                                warn!("Network {} not found for {}", network, coin);
                                return Ok(false);
                            }
                        }
                    }
                }
                warn!("Coin {} not found", coin);
                Ok(false)
            }
            std::result::Result::Err(e) => {
                warn!("Failed to check network withdrawal capability: {}", e);
                Ok(false)
            }
        }
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
