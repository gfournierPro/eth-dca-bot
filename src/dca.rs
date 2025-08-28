use crate::config::{TradingConfig, NotionConfig, WithdrawalConfig};
use crate::dca_stats_mongo::{DcaPurchase, print_dca_summary, print_recent_purchases};
use crate::notion_integration::NotionDCATracker;
use crate::{binance::BinanceClient, dca_stats_mongo::DcaStatsDB};
use crate::date_utils::should_check_withdrawal;
use anyhow::{Result, anyhow};
use chrono::Utc;
use rust_decimal::Decimal;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DcaTrader {
    pub binance_client: BinanceClient,
    trading_config: TradingConfig,
    withdrawal_config: WithdrawalConfig,
    pub stats_db: DcaStatsDB,
    notion_tracker: Option<NotionDCATracker>,
}

impl DcaTrader {
    pub async fn new(
        binance_client: BinanceClient, 
        trading_config: TradingConfig,
        withdrawal_config: WithdrawalConfig,
        notion_config: Option<&NotionConfig>
    ) -> Result<Self> {
        let stats_db = DcaStatsDB::new().await?;

        let notion_tracker = if let Some(config) = notion_config {
            if !config.token.is_empty() && !config.database_id.is_empty() {
                match NotionDCATracker::new(config) {
                    Ok(tracker) => {
                        info!("Notion integration enabled");
                        Some(tracker)
                    }
                    Err(e) => {
                        warn!("Notion integration disabled: {}", e);
                        None
                    }
                }
            } else {
                info!("Notion integration disabled: configuration incomplete");
                None
            }
        } else {
            info!("Notion integration disabled: no configuration provided");
            None
        };

        Ok(Self {
            binance_client,
            trading_config,
            withdrawal_config,
            stats_db,
            notion_tracker,
        })
    }

    pub async fn execute_dca_purchase(&self) -> Result<()> {
        info!("Starting DCA purchase execution");

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
        info!("Current ETH price: {} USDC", current_price);
        info!(
            "Purchasing {} USDC worth of ETH (≈ {} ETH)",
            purchase_amount, estimated_eth
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
        let fees_usdc = order_result.calculate_total_fees_in_usdc(current_price);

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
        info!("✅ DCA purchase completed successfully!");
        info!("╔═══════════════════════════════════════╗");
        info!("║           💰 PURCHASE DETAILS         ║");
        info!("╠═══════════════════════════════════════╣");
        info!("║ Order ID: {:>25} ║", order_result.order_id);
        info!("║ Status: {:>27} ║", order_result.status);
        info!("║ EUR Spent: €{:>23} ║", actual_eur_spent.round_dp(2));
        info!("║ USDC Spent: ${:>22} ║", executed_value.round_dp(2));
        info!("║ ETH Acquired: {:>21} ║", executed_qty.round_dp(6));
        info!("║ ETH Price: ${:>23} ║", average_price.round_dp(2));
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
        if should_check_withdrawal() {
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
        print_dca_summary(&summary);

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
        
        print_recent_purchases(&recent_purchases);
        Ok(())
    }

    pub async fn check_and_execute_withdrawal(&self) -> Result<()> {
        if !self.withdrawal_config.enabled {
            info!("Withdrawal is disabled in configuration");
            return Ok(());
        }

        if !should_check_withdrawal() {
            info!("Not the right time for withdrawal check");
            return Ok(());
        }

        info!("🔍 Checking if withdrawal is needed (last Monday of month)...");
        
        // First check if withdrawals are available for ETH
        match self.binance_client.check_withdrawal_capability("ETH").await {
            Ok(can_withdraw) => {
                if !can_withdraw {
                    warn!("⚠️  ETH withdrawals are not enabled for your account");
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
        match self.binance_client.check_network_withdrawal_capability("ETH", &self.withdrawal_config.network).await {
            Ok(network_available) => {
                if !network_available {
                    warn!("⚠️  Network '{}' is not available for ETH withdrawals", self.withdrawal_config.network);
                    warn!("💡 Try these network names instead:");
                    warn!("   • ARBITRUM (for Arbitrum One)");
                    warn!("   • ETH (for Ethereum mainnet)");
                    warn!("   • BSC (for Binance Smart Chain)");
                    warn!("   • OPTIMISM (for Optimism)");
                    return Ok(());
                }
                info!("Network '{}' is available for ETH withdrawals", self.withdrawal_config.network);
            }
            Err(e) => {
                warn!("Could not verify network capability: {}", e);
            }
        }
        
        let eth_balance = self.binance_client.get_eth_balance().await?;
        info!("Current ETH balance: {} ETH", eth_balance);

        if eth_balance < self.withdrawal_config.min_eth_threshold {
            info!(
                "ETH balance {} is below minimum threshold {}. No withdrawal needed.",
                eth_balance, self.withdrawal_config.min_eth_threshold
            );
            return Ok(());
        }

        let withdrawal_amount = self.withdrawal_config.withdrawal_amount
            .unwrap_or(eth_balance);

        if withdrawal_amount > eth_balance {
            warn!(
                "Requested withdrawal amount {} exceeds available balance {}. Using available balance.",
                withdrawal_amount, eth_balance
            );
        }

        let actual_withdrawal_amount = withdrawal_amount.min(eth_balance);

        info!("💸 Initiating withdrawal of {} ETH to cold wallet", actual_withdrawal_amount);
        
        match self.binance_client.withdraw_eth(
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
                info!("║ Amount: {:>27} ETH ║", actual_withdrawal_amount.round_dp(6));
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
