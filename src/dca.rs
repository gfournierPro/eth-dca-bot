use crate::config::{TradingConfig, NotionConfig};
use crate::dca_stats_mongo::{DcaPurchase, print_dca_summary, print_recent_purchases};
use crate::notion_integration::NotionDCATracker;
use crate::{binance::BinanceClient, dca_stats_mongo::DcaStatsDB};
use anyhow::{Result, anyhow};
use chrono::Utc;
use rust_decimal::Decimal;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DcaTrader {
    pub binance_client: BinanceClient,
    trading_config: TradingConfig,
    pub stats_db: DcaStatsDB,
    notion_tracker: Option<NotionDCATracker>,
}

impl DcaTrader {
    pub async fn new(
        binance_client: BinanceClient, 
        trading_config: TradingConfig,
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
            stats_db,
            notion_tracker,
        })
    }

    pub async fn execute_dca_purchase(&self) -> Result<()> {
        info!("Starting DCA purchase execution");

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
        let purchase_amount = if available_balance >= self.trading_config.buy_amount_usdc {
            self.trading_config.buy_amount_usdc
        } else {
            warn!(
                "Requested amount {} exceeds available balance {}. Using available balance.",
                self.trading_config.buy_amount_usdc, available_balance
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

        // Print purchase details
        info!("✅ DCA purchase completed successfully!");
        info!("╔═══════════════════════════════════════╗");
        info!("║           💰 PURCHASE DETAILS         ║");
        info!("╠═══════════════════════════════════════╣");
        info!("║ Order ID: {:>25} ║", order_result.order_id);
        info!("║ Status: {:>27} ║", order_result.status);
        info!("║ USDC Spent: ${:>22} ║", executed_value.round_dp(2));
        info!("║ ETH Acquired: {:>21} ║", executed_qty.round_dp(6));
        info!("║ ETH Price: ${:>23} ║", average_price.round_dp(2));
        info!("║ Fees Paid: ${:>23} ║", fees_usdc.round_dp(4));

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

        Ok(())
    }
    pub async fn show_dca_summary(&self) -> Result<()> {
        let current_price = self
            .binance_client
            .get_symbol_price(&self.trading_config.symbol)
            .await?;
        let summary = self.stats_db.get_summary(current_price).await?;
        print_dca_summary(&summary);

        let recent_purchases = self.stats_db.get_recent_purchases(5).await?;
        print_recent_purchases(&recent_purchases);
        Ok(())
    }
}
