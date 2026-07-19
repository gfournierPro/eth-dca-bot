use anyhow::Result;
use eth_dca_bot::binance::BinanceClient;
use eth_dca_bot::config::{Config, ExchangeKind};
use eth_dca_bot::dca_stats_mongo::{DcaPurchase, DcaStatsDB};
use eth_dca_bot::exchange::Exchange;
use eth_dca_bot::kraken::KrakenClient;
use eth_dca_bot::notion_integration::NotionDCATracker;
use rust_decimal_macros::dec;
use std::env;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

fn load_config() -> Result<Config> {
    let mut config = Config::default();

    if let Ok(exchange) = env::var("EXCHANGE") {
        config.exchange = ExchangeKind::parse(&exchange)
            .ok_or_else(|| anyhow::anyhow!("Invalid EXCHANGE '{}'", exchange))?;
    }
    if let Ok(v) = env::var("BINANCE_API_KEY") {
        config.binance.api_key = v;
    }
    if let Ok(v) = env::var("BINANCE_SECRET_KEY") {
        config.binance.secret_key = v;
    }
    if let Ok(v) = env::var("KRAKEN_API_KEY") {
        config.kraken.api_key = v;
    }
    if let Ok(v) = env::var("KRAKEN_SECRET_KEY") {
        config.kraken.secret_key = v;
    }
    if let Ok(v) = env::var("OKX_API_KEY") {
        config.okx.api_key = v;
    }
    if let Ok(v) = env::var("OKX_SECRET_KEY") {
        config.okx.secret_key = v;
    }
    if let Ok(v) = env::var("OKX_PASSPHRASE") {
        config.okx.passphrase = v;
    }

    if let Ok(amount) = env::var("DCA_AMOUNT_EUR") {
        config.trading.buy_amount_eur = amount.parse()?;
    }
    if let Ok(min_balance) = env::var("MIN_BALANCE_USDC") {
        config.trading.min_balance_usdc = min_balance.parse()?;
    }

    // Notion configuration
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

fn build_exchange(config: &Config) -> Arc<dyn Exchange> {
    match config.exchange {
        ExchangeKind::Binance => Arc::new(BinanceClient::new(
            config.binance.api_key.clone(),
            config.binance.secret_key.clone(),
            config.binance.base_url.clone(),
        )),
        ExchangeKind::Kraken => Arc::new(KrakenClient::new(
            config.kraken.api_key.clone(),
            config.kraken.secret_key.clone(),
            config.kraken.base_url.clone(),
        )),
        ExchangeKind::Okx => Arc::new(eth_dca_bot::okx::OkxClient::new(
            config.okx.api_key.clone(),
            config.okx.secret_key.clone(),
            config.okx.passphrase.clone(),
            config.okx.base_url.clone(),
        )),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    dotenv::dotenv().ok();

    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!(
            "Usage: {} <order_id_to_remove> <order_id_to_replace_with>",
            args[0]
        );
        eprintln!("Example: {} 6863767683 OQCLML-BW3P3-BUCMWZ", args[0]);
        std::process::exit(1);
    }

    // Order ids are strings now: numeric for legacy Binance orders, txids for Kraken.
    let order_id_to_remove = args[1].clone();
    let order_id_to_replace_with = args[2].clone();

    info!("🔄 Starting purchase replacement process");
    info!("   Removing order ID: {}", order_id_to_remove);
    info!("   Replacing with order ID: {}", order_id_to_replace_with);

    let config = load_config()?;
    let db = DcaStatsDB::new().await?;
    let exchange = build_exchange(&config);
    info!("Using exchange backend: {}", exchange.name());

    // Step 1: Remove the existing purchase from MongoDB if present.
    info!(
        "🔍 Checking if order {} exists in MongoDB...",
        order_id_to_remove
    );
    let removed_purchase = match db.get_purchase_by_order_id(&order_id_to_remove).await? {
        Some(purchase) => {
            info!("✅ Found existing purchase: {:?}", purchase);
            info!(
                "🗑️  Removing purchase with order ID: {}",
                order_id_to_remove
            );
            if !db.remove_purchase_by_order_id(&order_id_to_remove).await? {
                return Err(anyhow::anyhow!(
                    "Failed to remove purchase with order ID: {}",
                    order_id_to_remove
                ));
            }
            Some(purchase)
        }
        None => {
            info!(
                "⚠️  No existing purchase found with order ID: {}",
                order_id_to_remove
            );
            None
        }
    };

    // Step 2: Find the replacement order on the exchange (current month's orders).
    info!(
        "📡 Fetching order details from {} for order ID: {}",
        exchange.name(),
        order_id_to_replace_with
    );
    let purchases = exchange
        .get_current_month_purchases(&config.trading.symbol)
        .await?;

    let source = purchases
        .into_iter()
        .find(|p| p.order_id == order_id_to_replace_with)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Order with ID {} not found on {}",
                order_id_to_replace_with,
                exchange.name()
            )
        })?;

    if source.eth_amount <= dec!(0) || source.usdc_amount <= dec!(0) {
        return Err(anyhow::anyhow!(
            "Invalid order data: qty={}, value={}",
            source.eth_amount,
            source.usdc_amount
        ));
    }

    // Step 3: Build a fresh purchase record from the exchange order.
    let new_purchase = DcaPurchase {
        id: Uuid::new_v4().to_string(),
        timestamp: source.timestamp,
        symbol: source.symbol.clone(),
        side: source.side.clone(),
        usdc_amount: source.usdc_amount,
        eth_amount: source.eth_amount,
        eth_price: source.eth_price,
        fees_usdc: source.fees_usdc,
        order_id: source.order_id.clone(),
        status: source.status.clone(),
    };

    info!("📦 Created new purchase record: {:?}", new_purchase);

    // Step 4: Store the new purchase.
    info!("💾 Adding new purchase to MongoDB...");
    db.record_purchase(&new_purchase).await?;

    // Step 5: Notion, if configured.
    if config.notion.token.is_empty() || config.notion.database_id.is_empty() {
        info!("⚠️  Notion integration not configured, skipping Notion updates");
    } else {
        info!("📝 Updating Notion...");
        let notion_client = NotionDCATracker::new(&config.notion, "ETH", exchange.name())?;
        // Approximate EUR using a default rate; adjust if precise EUR is needed.
        let eur_amount = new_purchase.usdc_amount * dec!(0.85);
        notion_client
            .record_dca_purchase(&new_purchase, eur_amount)
            .await?;
        info!("✅ Notion updated successfully");
    }

    // Step 6: Summary.
    info!("🎉 Purchase replacement completed successfully!");
    if let Some(removed) = &removed_purchase {
        info!(
            "REMOVED: order {} — {} USDC / {} {}",
            removed.order_id,
            removed.usdc_amount.round_dp(2),
            removed.eth_amount.round_dp(6),
            removed.symbol
        );
    }
    info!(
        "ADDED: order {} — {} USDC / {} at ${} (fees ${})",
        new_purchase.order_id,
        new_purchase.usdc_amount.round_dp(2),
        new_purchase.eth_amount.round_dp(6),
        new_purchase.eth_price.round_dp(2),
        new_purchase.fees_usdc.round_dp(4)
    );

    Ok(())
}
