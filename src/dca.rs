use crate::config::{TradingConfig, NotionConfig, WithdrawalConfig};
use crate::dca_stats_mongo::{DcaPurchase, print_dca_summary, print_recent_purchases};
use crate::notion_integration::NotionDCATracker;
use crate::{binance::BinanceClient, dca_stats_mongo::DcaStatsDB};
use crate::date_utils::should_check_withdrawal;
use anyhow::{Result, anyhow};
use chrono::{Utc, DateTime, Duration};
use rust_decimal::Decimal;
use tracing::{error, info, warn};
use uuid::Uuid;
use cron::Schedule;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct DcaTrader {
    pub asset: String,
    pub binance_client: BinanceClient,
    trading_config: TradingConfig,
    withdrawal_config: WithdrawalConfig,
    pub stats_db: DcaStatsDB,
    notion_tracker: Option<NotionDCATracker>,
    timezone: String,
    cron_schedule: String,
}

impl DcaTrader {
    pub async fn new(
        asset: String,
        mongo_collection: &str,
        binance_client: BinanceClient,
        trading_config: TradingConfig,
        withdrawal_config: WithdrawalConfig,
        notion_config: Option<&NotionConfig>,
        timezone: String,
        cron_schedule: String,
    ) -> Result<Self> {
        let stats_db = DcaStatsDB::with_collection(mongo_collection).await?;

        let notion_tracker = if let Some(config) = notion_config {
            if !config.token.is_empty() && !config.database_id.is_empty() {
                match NotionDCATracker::new(config, &asset) {
                    Ok(tracker) => {
                        info!("[{}] Notion integration enabled", asset);
                        Some(tracker)
                    }
                    Err(e) => {
                        warn!("[{}] Notion integration disabled: {}", asset, e);
                        None
                    }
                }
            } else {
                info!("[{}] Notion integration disabled: configuration incomplete", asset);
                None
            }
        } else {
            info!("[{}] Notion integration disabled: no configuration provided", asset);
            None
        };

        Ok(Self {
            asset,
            binance_client,
            trading_config,
            withdrawal_config,
            stats_db,
            notion_tracker,
            timezone,
            cron_schedule,
        })
    }

    pub async fn execute_dca_purchase(&self) -> Result<()> {
        info!("Starting {} DCA purchase execution", self.asset);

        // First, get EUR/USDC exchange rate to convert our EUR amount to USDC
        let eur_usdc_price = self.binance_client.get_symbol_price("EURUSDC").await?;
        let target_usdc_amount = self.trading_config.buy_amount_eur * eur_usdc_price;
        
        info!("EUR/USDC rate: {} - Converting {} EUR to {} USDC", 
              eur_usdc_price, self.trading_config.buy_amount_eur, target_usdc_amount);

        let usdc_balance = self.binance_client.get_usdc_balanc().await?;
        if usdc_balance < self.trading_config.min_balance_usdc {
            let error_msg = format!(
                "Insufficient USDC balance: {}. Minimum required is {}",
                usdc_balance, self.trading_config.min_balance_usdc
            );
            error!("{}", error_msg);
            return Err(anyhow!(error_msg));
        }

        let available_balance = usdc_balance - self.trading_config.min_balance_usdc;
        let purchase_amount = if available_balance >= target_usdc_amount {
            target_usdc_amount
        } else {
            warn!(
                "Requested amount {} USDC (from {} EUR) exceeds available balance {}. Using available balance.",
                target_usdc_amount, self.trading_config.buy_amount_eur, available_balance
            );
            available_balance
        };
        if purchase_amount <= Decimal::ZERO {
            let error_msg = "No available balance for purchase after maintaining minimu balance";
            error!("{}", error_msg);
            return Err(anyhow!(error_msg));
        }

        let current_price = self
            .binance_client
            .get_symbol_price(&self.trading_config.symbol)
            .await?;

        let estimated_eth = purchase_amount / current_price;
        info!("Current {} price: {} USDC", self.asset, current_price);
        info!(
            "Purchasing {} USDC worth of {} (≈ {} {})",
            purchase_amount, self.asset, estimated_eth, self.asset
        );

        let order_result = self
            .binance_client
            .place_market_buy_order(&self.trading_config.symbol, purchase_amount)
            .await?;

        let executed_qty: Decimal = order_result.executed_qty.parse()?;
        let executed_value: Decimal = order_result.cummulative_quote_qty.parse()?;
        let average_price = if executed_qty > Decimal::ZERO {
            executed_value / executed_qty
        } else {
            Decimal::ZERO
        };
        let fees_usdc = order_result.calculate_total_fees_in_usdc(&self.asset, current_price);

        let purchase = DcaPurchase {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            symbol: self.trading_config.symbol.clone(),
            usdc_amount: executed_value,
            eth_amount: executed_qty,
            eth_price: average_price,
            fees_usdc,
            order_id: order_result.order_id,
            status: order_result.status.clone(),
        };
        self.stats_db.record_purchase(&purchase).await?;

        if let Some(ref notion_tracker) = self.notion_tracker {
            let eur_usd_price = self.binance_client.get_symbol_price("EURUSDC").await?;
            let eur_amount = executed_value / eur_usd_price;
            if let Err(e) = notion_tracker
                .record_dca_purchase(&purchase, eur_amount)
                .await
            {
                warn!("⚠️  Failed to record purchase in Notion: {}", e);
            }
        }

        // Calculate EUR amount spent for display purposes
        let eur_usd_price = self.binance_client.get_symbol_price("EURUSDC").await?;
        let actual_eur_spent = executed_value / eur_usd_price;

        // Print purchase details
        info!("✅ {} DCA purchase completed successfully!", self.asset);
        info!("╔═══════════════════════════════════════╗");
        info!("║           💰 PURCHASE DETAILS         ║");
        info!("╠═══════════════════════════════════════╣");
        info!("║ Order ID: {:>25} ║", order_result.order_id);
        info!("║ Status: {:>27} ║", order_result.status);
        info!("║ EUR Spent: €{:>23} ║", actual_eur_spent.round_dp(2));
        info!("║ USDC Spent: ${:>22} ║", executed_value.round_dp(2));
        info!("║ {:>3} Acquired: {:>21} ║", self.asset, executed_qty.round_dp(6));
        info!("║ {:>3} Price: ${:>23} ║", self.asset, average_price.round_dp(2));
        info!("║ Fees Paid: ${:>23} ║", fees_usdc.round_dp(4));
        info!("║ EUR/USDC Rate: {:>20} ║", eur_usd_price.round_dp(4));
        info!("╚═══════════════════════════════════════╝");

        if current_price > Decimal::ZERO {
            let slippage = ((average_price - current_price) / current_price).abs();
            info!("  Price slippage: {:.2}%", slippage * Decimal::new(100, 0));

            if slippage > self.trading_config.max_slippage {
                warn!(
                    "Slippage {:.2}% exceeded maximum allowed {:.2}%",
                    slippage * Decimal::new(100, 0),
                    self.trading_config.max_slippage * Decimal::new(100, 0)
                );
            }
        }

        self.show_dca_summary().await?;

        // Check if we should perform a withdrawal after the purchase
        if should_check_withdrawal(&self.timezone) {
            info!("🔍 Checking if withdrawal is needed after DCA purchase...");
            if let Err(e) = self.check_and_execute_withdrawal().await {
                warn!("Withdrawal check failed: {}", e);
            }
        }

        Ok(())
    }
    pub async fn show_dca_summary(&self) -> Result<()> {
        let current_price = self
            .binance_client
            .get_symbol_price(&self.trading_config.symbol)
            .await?;
        let summary = self.stats_db.get_summary(current_price).await?;
        print_dca_summary(&self.asset, &summary);

        let mut recent_purchases = self.stats_db.get_recent_purchases(5).await?;
        
        // If no recent purchases found in database, try to fetch from Binance
        if recent_purchases.is_empty() {
            info!("📝 No recent purchases found in database, fetching from Binance...");
            match self.binance_client.get_current_month_purchases(&self.trading_config.symbol).await {
                Ok(binance_purchases) => {
                    if !binance_purchases.is_empty() {
                        info!("✅ Found {} purchases from current month on Binance", binance_purchases.len());
                        // Take only the 5 most recent ones
                        recent_purchases = binance_purchases.into_iter().take(5).collect();
                    } else {
                        info!("📝 No purchases found for current month on Binance either");
                    }
                }
                Err(e) => {
                    warn!("⚠️  Failed to fetch purchases from Binance: {}", e);
                }
            }
        }
        
        print_recent_purchases(&self.asset, &recent_purchases);
        Ok(())
    }

    pub async fn check_and_execute_startup_dca(&self) -> Result<()> {
        info!("🔍 Checking if a scheduled DCA was missed and needs to be executed...");
        
        // Parse the cron schedule
        let schedule = match Schedule::from_str(&self.cron_schedule) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to parse cron schedule '{}': {}", self.cron_schedule, e);
                return Err(anyhow!("Invalid cron schedule"));
            }
        };

        let now = Utc::now();
        let twenty_four_hours_ago = now - Duration::hours(24);
        
        // Get all scheduled times in the last 24 hours
        let mut scheduled_times = Vec::new();
        for scheduled_time in schedule.after(&twenty_four_hours_ago).take(100) {
            if scheduled_time <= now {
                scheduled_times.push(scheduled_time);
            } else {
                break;
            }
        }

        if scheduled_times.is_empty() {
            info!("✅ No scheduled DCA executions were planned in the last 24 hours");
            return Ok(());
        }

        info!("📅 Found {} scheduled DCA time(s) in the last 24 hours", scheduled_times.len());
        
        // Check each scheduled time to see if we have a purchase around that time
        for scheduled_time in scheduled_times {
            let window_start = scheduled_time - Duration::hours(4); // 30min before
            let window_end = scheduled_time + Duration::hours(4);      // 2h after (generous window)
            
            // Check if we have any purchase in the window around this scheduled time
            let has_purchase_for_schedule = self.has_purchase_in_time_window(window_start, window_end).await?;
            
            if !has_purchase_for_schedule {
                info!("❌ Missing DCA purchase for scheduled time: {} (checking window {} to {})", 
                     scheduled_time.format("%Y-%m-%d %H:%M:%S UTC"),
                     window_start.format("%Y-%m-%d %H:%M:%S UTC"),
                     window_end.format("%Y-%m-%d %H:%M:%S UTC"));
                
                // Execute the missed DCA
                info!("🚀 Executing missed DCA for scheduled time: {}", scheduled_time.format("%Y-%m-%d %H:%M:%S UTC"));
                match self.execute_dca_purchase().await {
                    Ok(()) => {
                        info!("✅ Missed DCA purchase completed successfully!");
                        return Ok(()); // Only execute one missed DCA to avoid multiple purchases
                    }
                    Err(e) => {
                        error!("❌ Missed DCA purchase failed: {}", e);
                        return Err(e);
                    }
                }
            } else {
                info!("✅ Found DCA purchase for scheduled time: {}", scheduled_time.format("%Y-%m-%d %H:%M:%S UTC"));
            }
        }

        info!("✅ All scheduled DCA executions in the last 24h have been completed");
        Ok(())
    }

    async fn has_purchase_in_time_window(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Result<bool> {
        // First check database
        match self.stats_db.has_purchase_in_time_window(start, end).await {
            Ok(has_purchase) => Ok(has_purchase),
            Err(e) => {
                warn!("⚠️  Failed to check purchases in database: {}", e);
                // Fallback: check from Binance
                let binance_purchases = self.binance_client.get_current_month_purchases(&self.trading_config.symbol).await?;
                Ok(binance_purchases.iter().any(|p| p.timestamp >= start && p.timestamp <= end))
            }
        }
    }

    pub async fn check_and_execute_withdrawal(&self) -> Result<()> {
        if !self.withdrawal_config.enabled {
            info!("Withdrawal is disabled in configuration");
            return Ok(());
        }

        if !should_check_withdrawal(&self.timezone) {
            info!("Not the right time for withdrawal check");
            return Ok(());
        }

        info!("🔍 Checking if withdrawal is needed (last Monday of month)...");

        let coin = self.asset.as_str();

        // First check if withdrawals are available for this coin
        match self.binance_client.check_withdrawal_capability(coin).await {
            Ok(can_withdraw) => {
                if !can_withdraw {
                    warn!("⚠️  {} withdrawals are not enabled for your account", coin);
                    warn!("💡 Please check:");
                    warn!("   • Account verification status");
                    warn!("   • API key withdrawal permissions");
                    warn!("   • Regional restrictions");
                    warn!("   • Account security settings (2FA, etc.)");
                    return Ok(());
                }
            }
            Err(e) => {
                warn!("Could not verify withdrawal capability: {}", e);
                // Continue anyway, let the withdrawal attempt provide the specific error
            }
        }

        // Also check the specific network
        match self.binance_client.check_network_withdrawal_capability(coin, &self.withdrawal_config.network).await {
            Ok(network_available) => {
                if !network_available {
                    warn!("⚠️  Network '{}' is not available for {} withdrawals", self.withdrawal_config.network, coin);
                    warn!("💡 Double-check the WITHDRAWAL_NETWORK value matches a network Binance lists for {}", coin);
                    return Ok(());
                }
                info!("Network '{}' is available for {} withdrawals", self.withdrawal_config.network, coin);
            }
            Err(e) => {
                warn!("Could not verify network capability: {}", e);
            }
        }

        let asset_balance = self.binance_client.get_asset_balance(coin).await?;
        info!("Current {} balance: {} {}", coin, asset_balance, coin);

        if asset_balance < self.withdrawal_config.min_eth_threshold {
            info!(
                "{} balance {} is below minimum threshold {}. No withdrawal needed.",
                coin, asset_balance, self.withdrawal_config.min_eth_threshold
            );
            return Ok(());
        }

        let withdrawal_amount = self.withdrawal_config.withdrawal_amount
            .unwrap_or(asset_balance);

        if withdrawal_amount > asset_balance {
            warn!(
                "Requested withdrawal amount {} exceeds available balance {}. Using available balance.",
                withdrawal_amount, asset_balance
            );
        }

        let actual_withdrawal_amount = withdrawal_amount.min(asset_balance);

        info!("💸 Initiating withdrawal of {} {} to cold wallet", actual_withdrawal_amount, coin);

        match self.binance_client.withdraw_asset(
            coin,
            &self.withdrawal_config.cold_wallet_address,
            actual_withdrawal_amount,
            &self.withdrawal_config.network,
        ).await {
            Ok(response) => {
                info!("✅ Withdrawal initiated successfully!");
                info!("Withdrawal ID: {}", response.id);
                
                // Log the withdrawal details
                info!("╔═══════════════════════════════════════╗");
                info!("║         💸 WITHDRAWAL DETAILS         ║");
                info!("╠═══════════════════════════════════════╣");
                info!("║ Amount: {:>23} {:>3} ║", actual_withdrawal_amount.round_dp(6), coin);
                info!("║ Network: {:>26} ║", self.withdrawal_config.network);
                info!("║ Address: {:>26} ║", format!("{}...{}", 
                    &self.withdrawal_config.cold_wallet_address[..6],
                    &self.withdrawal_config.cold_wallet_address[self.withdrawal_config.cold_wallet_address.len()-4..]
                ));
                info!("║ Withdrawal ID: {:>21} ║", response.id);
                info!("╚═══════════════════════════════════════╝");
                
                Ok(())
            }
            Err(e) => {
                error!("❌ Failed to initiate withdrawal: {}", e);
                Err(e)
            }
        }
    }
}
