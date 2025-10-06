use anyhow::Result;
use dotenv::dotenv;
use eth_dca_bot::binance::BinanceClient;
use eth_dca_bot::config::Config;
use eth_dca_bot::dca_stats_mongo::DcaStatsDB;
use eth_dca_bot::notion_integration::NotionDCATracker;
use std::env;
use tracing::{error, info};
use tracing_subscriber;

fn load_config() -> Result<Config> {
    let api_key =
        env::var("BINANCE_API_KEY").map_err(|_| anyhow::anyhow!("BINANCE_API_KEY not set"))?;
    let secret_key = env::var("BINANCE_SECRET_KEY")
        .map_err(|_| anyhow::anyhow!("BINANCE_SECRET_KEY not set"))?;

    let mut config = Config::default();
    config.binance.api_key = api_key;
    config.binance.secret_key = secret_key;

    // Load Notion configuration
    if let Ok(token) = env::var("NOTION_TOKEN") {
        config.notion.token = token;
    }
    if let Ok(database_id) = env::var("NOTION_DATABASE_ID") {
        config.notion.database_id = database_id;
    }
    if let Ok(cold_wallet) = env::var("COLD_WALLET_ADDRESS") {
        config.notion.cold_wallet_address = cold_wallet;
    }

    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    dotenv().ok();

    info!("🔍 Checking Notion sync for latest DCA purchase...");

    // Load configuration
    let config = load_config()?;

    // Initialize components
    let binance_client = BinanceClient::new(
        config.binance.api_key.clone(),
        config.binance.secret_key.clone(),
        config.binance.base_url.clone(),
    );
    binance_client.sync_time().await?;

    let stats_db = DcaStatsDB::new().await?;
    let notion_tracker = NotionDCATracker::new(&config.notion)?;

    // Get the most recent purchase from database
    let recent_purchases = stats_db.get_recent_purchases(1).await?;
    
    info!("📊 Checking database for recent purchases...");
    if !recent_purchases.is_empty() {
        let latest_db = &recent_purchases[0];
        info!("   Latest in DB: {} ETH at {} UTC", 
              latest_db.eth_amount, 
              latest_db.timestamp.format("%Y-%m-%d %H:%M:%S"));
    } else {
        info!("   No purchases in database");
    }
    
    // Always check Binance for the latest purchase
    info!("📊 Checking Binance for current month purchases...");
    let binance_purchases = binance_client.get_current_month_purchases(&config.trading.symbol).await?;
    
    if binance_purchases.is_empty() {
        info!("❌ No purchases found on Binance for current month");
        return Ok(());
    }
    
    info!("✅ Found {} purchase(s) from Binance in current month", binance_purchases.len());
    
    // Get the most recent one from Binance
    let latest_purchase = &binance_purchases[0];
    info!("   Latest on Binance: {} ETH at {} UTC", 
          latest_purchase.eth_amount, 
          latest_purchase.timestamp.format("%Y-%m-%d %H:%M:%S"));
    
    // Check if this purchase needs to be synced to Notion
    let hours_ago = chrono::Utc::now().signed_duration_since(latest_purchase.timestamp).num_hours();
    info!("⏰ Purchase was {} hours ago", hours_ago);
    
    if hours_ago > 48 {
        info!("✅ Purchase is more than 48 hours old - likely already synced to Notion");
        return Ok(());
    }
    
    // Get EUR amount
    let eur_usdc_price = binance_client.get_symbol_price("EURUSDC").await?;
    let eur_amount = latest_purchase.usdc_amount / eur_usdc_price;
    
    info!("💰 Purchase details:");
    info!("   USDC Amount: {} USDC", latest_purchase.usdc_amount);
    info!("   EUR Amount: {} EUR", eur_amount.round_dp(2));
    info!("   ETH Amount: {} ETH", latest_purchase.eth_amount);
    info!("   ETH Price: {} USDC", latest_purchase.eth_price);
    
    // Sync to Notion
    info!("📤 Syncing purchase to Notion...");
    match notion_tracker.record_dca_purchase(latest_purchase, eur_amount).await {
        Ok(()) => {
            info!("✅ Successfully synced purchase to Notion!");
            info!("🎉 Done! Your latest purchase is now in Notion.");
        }
        Err(e) => {
            error!("❌ Failed to sync to Notion: {}", e);
            error!("💡 The purchase might already be in Notion, or there's a configuration issue");
            return Err(e);
        }
    }

    Ok(())
}
