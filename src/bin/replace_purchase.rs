use anyhow::Result;
use chrono::{TimeZone, Utc};
use eth_dca_bot::binance::BinanceClient;
use eth_dca_bot::config::Config;
use eth_dca_bot::dca_stats_mongo::{DcaStatsDB, DcaPurchase};
use eth_dca_bot::notion_integration::NotionDCATracker;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::env;
use tracing::info;
use uuid::Uuid;

fn load_config() -> Result<Config> {
    let api_key =
        env::var("BINANCE_API_KEY").map_err(|_| anyhow::anyhow!("BINANCE_API_KEY not set"))?;
    let secret_key = env::var("BINANCE_SECRET_KEY")
        .map_err(|_| anyhow::anyhow!("BINANCE_SECRET_KEY not set"))?;

    let mut config = Config::default();
    config.binance.api_key = api_key;
    config.binance.secret_key = secret_key;

    if let Ok(amount) = env::var("DCA_AMOUNT_EUR") {
        config.trading.buy_amount_eur = amount.parse()?;
    }
    if let Ok(min_balance) = env::var("MIN_BALANCE_USDC") {
        config.trading.min_balance_usdc = min_balance.parse()?;
    }
    if let Ok(cron) = env::var("SCHEDULE_CRON") {
        config.schedule.cron_expression = cron;
    }
    if let Ok(timezone) = env::var("TIMEZONE") {
        config.schedule.timezone = timezone;
    }

    // Load Notion configuration
    if let Ok(token) = env::var("NOTION_TOKEN") {
        config.notion.token = token;
    }
    if let Ok(database_id) = env::var("NOTION_DATABASE_ID") {
        config.notion.database_id = database_id;
    }
    if let Ok(cold_wallet) = env::var("COLD_WALLET_ADDRESS") {
        config.notion.cold_wallet_address = cold_wallet.clone();
        config.withdrawal.cold_wallet_address = cold_wallet; // Also set for withdrawal
    }

    // Load Withdrawal configuration
    if let Ok(enabled) = env::var("WITHDRAWAL_ENABLED") {
        config.withdrawal.enabled = enabled.parse().unwrap_or(false);
    }
    if let Ok(wallet) = env::var("WITHDRAWAL_WALLET_ADDRESS") {
        config.withdrawal.cold_wallet_address = wallet;
    }
    if let Ok(network) = env::var("WITHDRAWAL_NETWORK") {
        config.withdrawal.network = network;
    }
    if let Ok(threshold) = env::var("WITHDRAWAL_MIN_ETH_THRESHOLD") {
        config.withdrawal.min_eth_threshold = threshold.parse()?;
    }
    if let Ok(amount) = env::var("WITHDRAWAL_AMOUNT") {
        config.withdrawal.withdrawal_amount = Some(amount.parse()?);
    }

    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .init();

    // Load environment variables
    dotenv::dotenv().ok();

    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <order_id_to_remove> <order_id_to_replace_with>", args[0]);
        eprintln!("Example: {} 6863767683 6863022335", args[0]);
        std::process::exit(1);
    }

    let order_id_to_remove: u64 = args[1].parse()
        .map_err(|_| anyhow::anyhow!("Invalid order ID to remove: {}", args[1]))?;
    
    let order_id_to_replace_with: u64 = args[2].parse()
        .map_err(|_| anyhow::anyhow!("Invalid replacement order ID: {}", args[2]))?;

    info!("🔄 Starting purchase replacement process");
    info!("   Removing order ID: {}", order_id_to_remove);
    info!("   Replacing with order ID: {}", order_id_to_replace_with);

    // Load configuration using the same method as main.rs
    let config = load_config()?;
    
    // Initialize MongoDB connection
    let db = DcaStatsDB::new().await?;
    
    println!("{:?}",config.binance.api_key );
    // Initialize Binance client
    let binance_client = BinanceClient::new(
        config.binance.api_key.clone(),
        config.binance.secret_key.clone(),
        config.binance.base_url.clone(),
    );

    // Step 1: Check if the order to remove exists in MongoDB
    info!("🔍 Checking if order {} exists in MongoDB...", order_id_to_remove);
    let existing_purchase = db.get_purchase_by_order_id(order_id_to_remove).await?;
    
    let removed_purchase = match existing_purchase {
        Some(purchase) => {
            info!("✅ Found existing purchase: {:?}", purchase);
            
            // Remove the existing purchase
            info!("🗑️  Removing purchase with order ID: {}", order_id_to_remove);
            let removed = db.remove_purchase_by_order_id(order_id_to_remove).await?;
            if !removed {
                return Err(anyhow::anyhow!("Failed to remove purchase with order ID: {}", order_id_to_remove));
            }
            
            Some(purchase)
        }
        None => {
            info!("⚠️  No existing purchase found with order ID: {}", order_id_to_remove);
            None
        }
    };

    // Step 2: Get order details from Binance for the replacement order
    info!("📡 Fetching order details from Binance for order ID: {}", order_id_to_replace_with);
    
    // Get order history and find the specific order
    let orders = binance_client.get_order_history("ETHUSDC", None, None, Some(1000)).await?;
    
    let replacement_order = orders.iter()
        .find(|order| order.order_id == order_id_to_replace_with)
        .ok_or_else(|| anyhow::anyhow!("Order with ID {} not found on Binance", order_id_to_replace_with))?;

    info!("✅ Found replacement order on Binance: {:?}", replacement_order);

    // Step 3: Create new DcaPurchase from Binance order
    let executed_qty: Decimal = replacement_order.executed_qty.parse()
        .map_err(|_| anyhow::anyhow!("Invalid executed quantity: {}", replacement_order.executed_qty))?;
    
    let executed_value: Decimal = replacement_order.cummulative_quote_qty.parse()
        .map_err(|_| anyhow::anyhow!("Invalid executed value: {}", replacement_order.cummulative_quote_qty))?;

    if executed_qty <= dec!(0) || executed_value <= dec!(0) {
        return Err(anyhow::anyhow!("Invalid order data: qty={}, value={}", executed_qty, executed_value));
    }

    let average_price = executed_value / executed_qty;
    let timestamp = Utc.timestamp_millis_opt(replacement_order.time)
        .single()
        .ok_or_else(|| anyhow::anyhow!("Invalid timestamp: {}", replacement_order.time))?;

    // Estimate fees as 0.1% of trade value (Binance standard fee)
    let estimated_fees = executed_value * dec!(0.001);

    let new_purchase = DcaPurchase {
        id: Uuid::new_v4().to_string(),
        timestamp,
        symbol: replacement_order.symbol.clone(),
        usdc_amount: executed_value,
        eth_amount: executed_qty,
        eth_price: average_price,
        fees_usdc: estimated_fees,
        order_id: replacement_order.order_id,
        status: replacement_order.status.clone(),
    };

    info!("📦 Created new purchase record: {:?}", new_purchase);

    // Step 4: Add new purchase to MongoDB
    info!("💾 Adding new purchase to MongoDB...");
    db.record_purchase(&new_purchase).await?;

    // Step 5: Handle Notion integration if configured
    if config.notion.token.is_empty() || config.notion.database_id.is_empty() {
        info!("⚠️  Notion integration not configured, skipping Notion updates");
    } else {
        info!("📝 Updating Notion...");
        
        // Initialize Notion client
        let notion_client = NotionDCATracker::new(&config.notion)?;
        
        // We need to calculate EUR amount for Notion
        // For simplicity, using a default EUR/USD rate of 0.85 (you might want to fetch this from an API)
        let eur_amount = new_purchase.usdc_amount * dec!(0.85);
        
        // Record the new purchase in Notion
        notion_client.record_dca_purchase(&new_purchase, eur_amount).await?;
        
        info!("✅ Notion updated successfully");
    }

    // Step 6: Print summary
    info!("🎉 Purchase replacement completed successfully!");
    info!("╔═══════════════════════════════════════╗");
    info!("║         REPLACEMENT SUMMARY           ║");
    info!("╠═══════════════════════════════════════╣");
    
    if let Some(removed) = &removed_purchase {
        info!("║ REMOVED:                              ║");
        info!("║ Order ID: {:>25} ║", removed.order_id);
        info!("║ Date: {:>31} ║", removed.timestamp.format("%Y-%m-%d %H:%M:%S UTC"));
        info!("║ USDC: ${:>30} ║", removed.usdc_amount.round_dp(2));
        info!("║ ETH: {:>32} ║", removed.eth_amount.round_dp(6));
        info!("╠═══════════════════════════════════════╣");
    }
    
    info!("║ ADDED:                                ║");
    info!("║ Order ID: {:>25} ║", new_purchase.order_id);
    info!("║ Date: {:>31} ║", new_purchase.timestamp.format("%Y-%m-%d %H:%M:%S UTC"));
    info!("║ USDC: ${:>30} ║", new_purchase.usdc_amount.round_dp(2));
    info!("║ ETH: {:>32} ║", new_purchase.eth_amount.round_dp(6));
    info!("║ Price: ${:>29} ║", new_purchase.eth_price.round_dp(2));
    info!("║ Fees: ${:>30} ║", new_purchase.fees_usdc.round_dp(4));
    info!("╚═══════════════════════════════════════╝");

    Ok(())
}
