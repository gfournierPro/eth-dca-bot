use crate::config::{MarketIndicatorsConfig, NotionConfig, TradingConfig, WithdrawalConfig};
use crate::date_utils::should_check_withdrawal;
use crate::dca_stats_mongo::DcaStatsDB;
use crate::dca_stats_mongo::{DcaPurchase, print_dca_summary, print_recent_purchases};
use crate::exchange::{Exchange, LimitBuyConfig};
use crate::market_indicators;
use crate::notion_integration::NotionDCATracker;
use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use cron::Schedule;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Clone)]
pub struct DcaTrader {
    pub asset: String,
    pub exchange: Arc<dyn Exchange>,
    trading_config: TradingConfig,
    withdrawal_config: WithdrawalConfig,
    pub stats_db: DcaStatsDB,
    notion_tracker: Option<NotionDCATracker>,
    timezone: String,
    cron_schedule: String,
    market_indicators: Option<market_indicators::MarketIndicators>,
}

impl DcaTrader {
    pub async fn new(
        asset: String,
        mongo_collection: &str,
        exchange: Arc<dyn Exchange>,
        trading_config: TradingConfig,
        withdrawal_config: WithdrawalConfig,
        notion_config: Option<&NotionConfig>,
        market_indicators_config: Option<&MarketIndicatorsConfig>,
        timezone: String,
        cron_schedule: String,
    ) -> Result<Self> {
        let stats_db = DcaStatsDB::with_collection(mongo_collection).await?;

        let source = exchange.name();
        let notion_tracker = if let Some(config) = notion_config {
            if !config.token.is_empty() && !config.database_id.is_empty() {
                match NotionDCATracker::new(config, &asset, source) {
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
                info!(
                    "[{}] Notion integration disabled: configuration incomplete",
                    asset
                );
                None
            }
        } else {
            info!(
                "[{}] Notion integration disabled: no configuration provided",
                asset
            );
            None
        };

        let market_indicators = if let Some(config) = market_indicators_config {
            info!("Dynamic DCA sizing enabled with market indicators");
            // Convert config to the market_indicators module format
            let mi_config = market_indicators::MarketIndicatorsConfig {
                volatility_scaling_enabled: config.volatility_scaling_enabled,
                volatility_period: config.volatility_period,
                high_volatility_multiplier: config.high_volatility_multiplier,
                volatility_threshold: config.volatility_threshold,
                low_volatility_multiplier: config.low_volatility_multiplier,
                low_volatility_threshold: config.low_volatility_threshold,
                rsi_enabled: config.rsi_enabled,
                rsi_period: config.rsi_period,
                rsi_oversold_threshold: config.rsi_oversold_threshold,
                rsi_oversold_multiplier: config.rsi_oversold_multiplier,
                rsi_overbought_threshold: config.rsi_overbought_threshold,
                rsi_overbought_multiplier: config.rsi_overbought_multiplier,
                price_deviation_enabled: config.price_deviation_enabled,
                moving_average_period: config.moving_average_period,
                deviation_threshold_percent: config.deviation_threshold_percent,
                below_ma_multiplier: config.below_ma_multiplier,
                above_ma_threshold_percent: config.above_ma_threshold_percent,
                above_ma_multiplier: config.above_ma_multiplier,
                momentum_enabled: config.momentum_enabled,
                momentum_period: config.momentum_period,
                negative_momentum_threshold: config.negative_momentum_threshold,
                negative_momentum_multiplier: config.negative_momentum_multiplier,
                positive_momentum_threshold: config.positive_momentum_threshold,
                positive_momentum_multiplier: config.positive_momentum_multiplier,
                max_total_multiplier: config.max_total_multiplier,
                min_total_multiplier: config.min_total_multiplier,
            };
            Some(market_indicators::MarketIndicators::new(mi_config))
        } else {
            info!("Market indicators configuration not provided, using fixed DCA amounts");
            None
        };

        Ok(Self {
            asset,
            exchange,
            trading_config,
            withdrawal_config,
            stats_db,
            notion_tracker,
            timezone,
            cron_schedule,
            market_indicators,
        })
    }

    pub async fn execute_dca_purchase(&mut self) -> Result<()> {
        info!("Starting {} DCA purchase execution", self.asset);

        // First, get EUR/USDC exchange rate to convert our EUR amount to USDC
        let eur_usdc_price = self.exchange.get_usdc_per_eur().await?;
        let base_target_usdc_amount = self.trading_config.buy_amount_eur * eur_usdc_price;

        // Calculate dynamic multiplier if market indicators are enabled
        let dca_multiplier = if let Some(ref mut market_indicators) = self.market_indicators {
            market_indicators
                .calculate_dca_multiplier(
                    self.exchange.as_ref(),
                    &self.stats_db,
                    &self.trading_config.symbol,
                )
                .await?
        } else {
            Decimal::ONE
        };

        let target_usdc_amount = base_target_usdc_amount * dca_multiplier;

        info!(
            "EUR/USDC rate: {} - Converting {} EUR to {} USDC (base amount)",
            eur_usdc_price, self.trading_config.buy_amount_eur, base_target_usdc_amount
        );
        info!(
            "Dynamic DCA multiplier: {} - Adjusted target amount: {} USDC",
            dca_multiplier, target_usdc_amount
        );

        let usdc_balance = self.exchange.get_usdc_balance().await?;
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

        let current_price = self.exchange.get_price(&self.trading_config.symbol).await?;

        let estimated_eth = purchase_amount / current_price;
        info!("Current {} price: {} USDC", self.asset, current_price);
        info!(
            "Purchasing {} USDC worth of {} (≈ {} {})",
            purchase_amount, self.asset, estimated_eth, self.asset
        );

        // Prefer the cheaper maker fee: exchanges that implement a patient-maker
        // limit strategy (Kraken) will rest a post-only order and only fall back to
        // a taker market order if the price drifts or a timeout hits. Exchanges
        // without one (Binance) transparently fall back to a market order.
        let order_result = self
            .exchange
            .place_limit_buy(
                &self.trading_config.symbol,
                purchase_amount,
                &LimitBuyConfig::default(),
            )
            .await?;

        let executed_qty = order_result.executed_qty;
        let executed_value = order_result.executed_value;
        let average_price = order_result.avg_price;
        let fees_usdc = order_result.fees_usdc;

        let purchase = DcaPurchase {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            symbol: self.trading_config.symbol.clone(),
            side: "BUY".to_string(), // This is a DCA purchase, so always BUY
            usdc_amount: executed_value,
            eth_amount: executed_qty,
            eth_price: average_price,
            fees_usdc,
            order_id: order_result.order_id.clone(),
            status: order_result.status.clone(),
        };
        self.stats_db.record_purchase(&purchase).await?;

        if let Some(ref notion_tracker) = self.notion_tracker {
            let eur_usd_price = self.exchange.get_usdc_per_eur().await?;
            let eur_amount = executed_value / eur_usd_price;
            if let Err(e) = notion_tracker
                .record_dca_purchase(&purchase, eur_amount)
                .await
            {
                warn!("⚠️  Failed to record purchase in Notion: {}", e);
            }
        }

        // Calculate EUR amount spent for display purposes
        let eur_usd_price = self.exchange.get_usdc_per_eur().await?;
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
        info!(
            "║ {:>3} Acquired: {:>21} ║",
            self.asset,
            executed_qty.round_dp(6)
        );
        info!(
            "║ {:>3} Price: ${:>23} ║",
            self.asset,
            average_price.round_dp(2)
        );
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
        let current_price = self.exchange.get_price(&self.trading_config.symbol).await?;
        let summary = self.stats_db.get_summary(current_price).await?;
        print_dca_summary(&self.asset, &summary);

        let mut recent_purchases = self.stats_db.get_recent_purchases(5).await?;

        // If no recent purchases found in database, try to fetch from the exchange
        if recent_purchases.is_empty() {
            let source = self.exchange.name();
            info!(
                "📝 No recent purchases found in database, fetching from {}...",
                source
            );
            match self
                .exchange
                .get_current_month_purchases(&self.trading_config.symbol)
                .await
            {
                Ok(exchange_purchases) => {
                    if !exchange_purchases.is_empty() {
                        info!(
                            "✅ Found {} purchases from current month on {}",
                            exchange_purchases.len(),
                            source
                        );
                        // Take only the 5 most recent ones
                        recent_purchases = exchange_purchases.into_iter().take(5).collect();
                    } else {
                        info!(
                            "📝 No purchases found for current month on {} either",
                            source
                        );
                    }
                }
                Err(e) => {
                    warn!("⚠️  Failed to fetch purchases from {}: {}", source, e);
                }
            }
        }

        print_recent_purchases(&self.asset, &recent_purchases);
        Ok(())
    }

    pub async fn check_and_execute_startup_dca(&mut self) -> Result<()> {
        info!("🔍 Checking if a scheduled DCA was missed and needs to be executed...");

        // Parse the cron schedule
        let schedule = match Schedule::from_str(&self.cron_schedule) {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "Failed to parse cron schedule '{}': {}",
                    self.cron_schedule, e
                );
                return Err(anyhow!("Invalid cron schedule"));
            }
        };

        let now = Utc::now();
        // Look back 48 hours to be safe (in case of timezone issues or longer downtimes)
        let lookback_time = now - Duration::hours(48);

        info!("🕐 Current time: {} UTC", now.format("%Y-%m-%d %H:%M:%S"));
        info!(
            "🔍 Looking for scheduled times between {} UTC and {} UTC",
            lookback_time.format("%Y-%m-%d %H:%M:%S"),
            now.format("%Y-%m-%d %H:%M:%S")
        );
        info!("📋 Cron schedule: '{}'", self.cron_schedule);

        // Get all scheduled times in the lookback period
        let mut scheduled_times = Vec::new();
        for scheduled_time in schedule.after(&lookback_time).take(200) {
            if scheduled_time <= now {
                info!(
                    "📅 Found scheduled time: {} UTC",
                    scheduled_time.format("%Y-%m-%d %H:%M:%S")
                );
                scheduled_times.push(scheduled_time);
            } else {
                break;
            }
        }

        if scheduled_times.is_empty() {
            info!("✅ No scheduled DCA executions were planned in the lookback period");
            info!(
                "💡 Note: Cron schedules are evaluated in UTC. Your schedule '{}' in timezone '{}' might need adjustment.",
                self.cron_schedule, self.timezone
            );
            return Ok(());
        }

        info!(
            "📅 Found {} scheduled DCA time(s) in the lookback period",
            scheduled_times.len()
        );

        // Check each scheduled time to see if we have a purchase covering it. The
        // upper bound is `now`, not a fixed window after `scheduled_time`: a catch-up
        // buy can land arbitrarily late if the bot keeps restarting mid-purchase (the
        // patient-maker loop can take minutes to hours), and a fixed window used to
        // make the check fall permanently outside it — re-triggering a fresh buy on
        // every subsequent restart forever.
        for scheduled_time in scheduled_times {
            let window_start = scheduled_time - Duration::hours(4);

            // Check if we have any purchase since shortly before this scheduled time
            let has_purchase_for_schedule = self
                .has_purchase_in_time_window(window_start, now)
                .await?;

            if !has_purchase_for_schedule {
                info!(
                    "❌ Missing DCA purchase for scheduled time: {} (checking window {} to {})",
                    scheduled_time.format("%Y-%m-%d %H:%M:%S UTC"),
                    window_start.format("%Y-%m-%d %H:%M:%S UTC"),
                    now.format("%Y-%m-%d %H:%M:%S UTC")
                );

                // Execute the missed DCA
                info!(
                    "🚀 Executing missed DCA for scheduled time: {}",
                    scheduled_time.format("%Y-%m-%d %H:%M:%S UTC")
                );
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
                info!(
                    "✅ Found DCA purchase for scheduled time: {}",
                    scheduled_time.format("%Y-%m-%d %H:%M:%S UTC")
                );
            }
        }

        info!("✅ All scheduled DCA executions in the lookback period have been completed");
        Ok(())
    }

    async fn has_purchase_in_time_window(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<bool> {
        // First check database
        match self.stats_db.has_purchase_in_time_window(start, end).await {
            Ok(has_purchase) => Ok(has_purchase),
            Err(e) => {
                warn!("⚠️  Failed to check purchases in database: {}", e);
                // Fallback: check from the exchange
                let exchange_purchases = self
                    .exchange
                    .get_current_month_purchases(&self.trading_config.symbol)
                    .await?;
                Ok(exchange_purchases
                    .iter()
                    .any(|p| p.timestamp >= start && p.timestamp <= end))
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
        let destination = self.withdrawal_config.cold_wallet_address.as_str();

        let asset_balance = self.exchange.get_asset_balance(coin).await?;
        info!("Current {} balance: {} {}", coin, asset_balance, coin);

        if asset_balance < self.withdrawal_config.min_eth_threshold {
            info!(
                "{} balance {} is below minimum threshold {}. No withdrawal needed.",
                coin, asset_balance, self.withdrawal_config.min_eth_threshold
            );
            return Ok(());
        }

        let withdrawal_amount = self
            .withdrawal_config
            .withdrawal_amount
            .unwrap_or(asset_balance);

        if withdrawal_amount > asset_balance {
            warn!(
                "Requested withdrawal amount {} exceeds available balance {}. Using available balance.",
                withdrawal_amount, asset_balance
            );
        }

        let actual_withdrawal_amount = withdrawal_amount.min(asset_balance);

        // Verify the withdrawal is possible before attempting it. On Binance this
        // checks coin/network availability; on Kraken it validates the withdrawal key.
        match self
            .exchange
            .verify_withdrawal(
                coin,
                destination,
                &self.withdrawal_config.network,
                actual_withdrawal_amount,
            )
            .await
        {
            Ok(true) => {
                info!(
                    "Withdrawal of {} {} verified as available",
                    actual_withdrawal_amount, coin
                );
            }
            Ok(false) => {
                warn!(
                    "⚠️  {} withdrawals are not currently available for this destination",
                    coin
                );
                warn!("💡 Please check:");
                warn!("   • API key withdrawal permissions");
                warn!(
                    "   • That the destination address/withdrawal key is registered on the exchange"
                );
                warn!("   • Regional restrictions and account security settings");
                return Ok(());
            }
            Err(e) => {
                warn!("Could not verify withdrawal capability: {}", e);
                // Continue anyway, let the withdrawal attempt provide the specific error
            }
        }

        info!(
            "💸 Initiating withdrawal of {} {} to cold wallet",
            actual_withdrawal_amount, coin
        );

        match self
            .exchange
            .withdraw(
                coin,
                destination,
                actual_withdrawal_amount,
                &self.withdrawal_config.network,
            )
            .await
        {
            Ok(withdrawal_id) => {
                info!("✅ Withdrawal initiated successfully!");
                info!("Withdrawal ID: {}", withdrawal_id);

                // Log the withdrawal details
                info!("╔═══════════════════════════════════════╗");
                info!("║         💸 WITHDRAWAL DETAILS         ║");
                info!("╠═══════════════════════════════════════╣");
                info!(
                    "║ Amount: {:>23} {:>3} ║",
                    actual_withdrawal_amount.round_dp(6),
                    coin
                );
                info!("║ Network: {:>26} ║", self.withdrawal_config.network);
                // `destination` is a raw address on Binance or a withdrawal-key name on
                // Kraken; only abbreviate when it's long enough to be a real address.
                let dest_display = if destination.len() > 12 {
                    format!(
                        "{}...{}",
                        &destination[..6],
                        &destination[destination.len() - 4..]
                    )
                } else {
                    destination.to_string()
                };
                info!("║ Destination: {:>22} ║", dest_display);
                info!("║ Withdrawal ID: {:>21} ║", withdrawal_id);
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
