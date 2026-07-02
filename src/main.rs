mod binance;
mod config;
mod date_utils;
mod dca;
mod dca_stats_mongo;
mod exchange;
mod kraken;
mod notion_integration;

use anyhow::Result;
use chrono::Utc;
use config::{Config, ExchangeKind};
use dca::DcaTrader;
use dotenv::dotenv;
use exchange::Exchange;
use std::env;
use std::str::FromStr;
use std::sync::Arc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};
use tracing_subscriber;

/// Construct the active exchange backend from config. Both backends are always
/// compiled in; `EXCHANGE` selects which one is used at runtime.
fn build_exchange(config: &Config) -> Arc<dyn Exchange> {
    match config.exchange {
        ExchangeKind::Binance => Arc::new(binance::BinanceClient::new(
            config.binance.api_key.clone(),
            config.binance.secret_key.clone(),
            config.binance.base_url.clone(),
        )),
        ExchangeKind::Kraken => Arc::new(kraken::KrakenClient::new(
            config.kraken.api_key.clone(),
            config.kraken.secret_key.clone(),
            config.kraken.base_url.clone(),
        )),
    }
}

fn calculate_next_execution(cron_expr: &str, timezone_str: &str) -> Result<String> {
    use chrono::TimeZone;
    use chrono_tz::Tz;
    use cron::Schedule;

    // Parse timezone
    let tz = if timezone_str == "UTC" || timezone_str.is_empty() {
        chrono_tz::UTC
    } else {
        Tz::from_str(timezone_str).unwrap_or(chrono_tz::Europe::Paris)
    };

    let schedule = Schedule::from_str(cron_expr)?;
    let now_utc = Utc::now();

    // For timezone-aware scheduling, we use the timezone directly with the cron library
    let mut upcoming = schedule.upcoming(tz);

    if let Some(next_local) = upcoming.next() {
        // Convert to UTC for duration calculation
        let next_utc = next_local.with_timezone(&Utc);

        let duration_until = next_utc.signed_duration_since(now_utc);
        let total_seconds = duration_until.num_seconds();

        if total_seconds <= 0 {
            return Ok("Now".to_string());
        }

        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;

        let time_str = if hours > 0 {
            format!("{}h {}m {}s", hours, minutes, seconds)
        } else if minutes > 0 {
            format!("{}m {}s", minutes, seconds)
        } else {
            format!("{}s", seconds)
        };

        Ok(format!(
            "{} (next: {} {})",
            time_str,
            next_local.format("%Y-%m-%d %H:%M:%S"),
            timezone_str
        ))
    } else {
        Ok("Unable to calculate".to_string())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    dotenv().ok();

    info!("Starting ETH DCA Bot...");
    let config = load_config()?;
    validate_config(&config)?;

    let sched = JobScheduler::new().await?;

    // Build the selected exchange backend once and share it across workflows.
    let exchange = build_exchange(&config);
    info!("Using exchange backend: {}", exchange.name());

    // ETH workflow (always on — preserves the original behavior).
    setup_asset_trader(config.eth_asset(), exchange.clone(), &sched).await?;

    // BTC workflow (optional — runs alongside ETH in the same process).
    if let Some(btc_cfg) = config.btc.clone() {
        if let Err(e) = setup_asset_trader(btc_cfg, exchange.clone(), &sched).await {
            error!("Failed to set up BTC DCA workflow: {}", e);
        }
    }

    sched.start().await?;
    info!("DCA Bot is running.");

    info!("Press Ctrl+C to stop the bot");

    // Keep the application running
    tokio::signal::ctrl_c().await?;
    info!("Shutting down DCA Bot");

    Ok(())
}

/// Build a [`DcaTrader`] for one asset, run its startup checks, and register its
/// recurring purchase job on the shared scheduler.
async fn setup_asset_trader(
    asset_cfg: config::AssetDcaConfig,
    exchange: Arc<dyn Exchange>,
    sched: &JobScheduler,
) -> Result<()> {
    let asset = asset_cfg.asset.clone();
    let exchange_name = exchange.name();

    let trader = DcaTrader::new(
        asset.clone(),
        &asset_cfg.mongo_collection,
        exchange,
        asset_cfg.trading.clone(),
        asset_cfg.withdrawal.clone(),
        Some(&asset_cfg.notion),
        asset_cfg.schedule.timezone.clone(),
        asset_cfg.schedule.cron_expression.clone(),
    )
    .await?;

    info!("[{}] Testing {} API connection...", asset, exchange_name);
    match trader.exchange.get_usdc_balance().await {
        Ok(balance) => {
            info!("[{}] Current USDC balance: {}", asset, balance);
            trader.show_dca_summary().await.unwrap_or_else(|e| {
                error!("[{}] Failed to load DCA summary: {}", asset, e);
            });

            // Check for withdrawal on startup if we're in the right time period
            info!(
                "[{}] 🔍 Checking if withdrawal is needed at startup...",
                asset
            );
            trader
                .check_and_execute_withdrawal()
                .await
                .unwrap_or_else(|e| {
                    error!("[{}] Startup withdrawal check failed: {}", asset, e);
                });

            // Check if DCA is needed (no purchase in last 24h)
            info!("[{}] 🔍 Checking if startup DCA is needed...", asset);
            trader
                .check_and_execute_startup_dca()
                .await
                .unwrap_or_else(|e| {
                    error!("[{}] Startup DCA check failed: {}", asset, e);
                });
        }
        Err(e) => {
            error!(
                "[{}] Failed to connect to {} API: {}",
                asset, exchange_name, e
            );
            return Err(e);
        }
    };

    let cron = asset_cfg.schedule.cron_expression.clone();
    let timezone_str = asset_cfg.schedule.timezone.clone();

    let job = if timezone_str == "UTC" || timezone_str.is_empty() {
        let trader = trader.clone();
        let asset_for_job = asset.clone();
        Job::new_async(cron.as_str(), move |_uuid, _l| {
            let trader = trader.clone();
            let asset = asset_for_job.clone();
            Box::pin(async move {
                info!("[{}] Executing scheduled DCA purchase", asset);
                match trader.execute_dca_purchase().await {
                    Ok(()) => info!("[{}] Scheduled DCA purchase completed successfully", asset),
                    Err(e) => error!("[{}] Scheduled DCA purchase failed: {}", asset, e),
                }
            })
        })?
    } else {
        use chrono_tz::Tz;
        use std::str::FromStr;
        let tz = Tz::from_str(&timezone_str).unwrap_or(chrono_tz::Europe::Berlin);
        let trader = trader.clone();
        let asset_for_job = asset.clone();
        Job::new_async_tz(cron.as_str(), tz, move |_uuid, _l| {
            let trader = trader.clone();
            let asset = asset_for_job.clone();
            Box::pin(async move {
                info!("[{}] Executing scheduled DCA purchase", asset);
                match trader.execute_dca_purchase().await {
                    Ok(()) => info!("[{}] Scheduled DCA purchase completed successfully", asset),
                    Err(e) => error!("[{}] Scheduled DCA purchase failed: {}", asset, e),
                }
            })
        })?
    };

    sched.add(job).await?;
    info!(
        "[{}] DCA job scheduled for: {} (timezone: {})",
        asset, cron, timezone_str
    );

    match calculate_next_execution(&cron, &timezone_str) {
        Ok(time_until) => info!(
            "[{}] ⏰ Next DCA batch will execute in: {}",
            asset, time_until
        ),
        Err(e) => error!("[{}] Failed to calculate next execution time: {}", asset, e),
    }

    Ok(())
}

fn load_config() -> Result<Config> {
    let mut config = Config::default();

    // Select the exchange backend (defaults to Binance for backwards compatibility).
    if let Ok(exchange) = env::var("EXCHANGE") {
        config.exchange = ExchangeKind::parse(&exchange).ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid EXCHANGE '{}' (expected 'binance' or 'kraken')",
                exchange
            )
        })?;
    }

    // Load credentials for whichever backends are configured. Only the selected
    // exchange's credentials are required (validated in validate_config).
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
    if let Ok(v) = env::var("BINANCE_BASE_URL") {
        config.binance.base_url = v;
    }
    if let Ok(v) = env::var("KRAKEN_BASE_URL") {
        config.kraken.base_url = v;
    }

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

    // Load optional BTC DCA workflow (additive — leaves the ETH workflow untouched).
    if env::var("BTC_DCA_ENABLED")
        .map(|v| v == "true")
        .unwrap_or(false)
    {
        let mut btc = config::AssetDcaConfig::btc_default();

        if let Ok(amount) = env::var("BTC_DCA_AMOUNT_EUR") {
            btc.trading.buy_amount_eur = amount.parse()?;
        }
        if let Ok(min_balance) = env::var("BTC_MIN_BALANCE_USDC") {
            btc.trading.min_balance_usdc = min_balance.parse()?;
        }
        if let Ok(cron) = env::var("BTC_SCHEDULE_CRON") {
            btc.schedule.cron_expression = cron;
        }
        // BTC reuses the global TIMEZONE unless a BTC-specific one is provided.
        btc.schedule.timezone = env::var("BTC_TIMEZONE")
            .ok()
            .or_else(|| env::var("TIMEZONE").ok())
            .unwrap_or(btc.schedule.timezone);
        if let Ok(collection) = env::var("BTC_MONGO_COLLECTION") {
            btc.mongo_collection = collection;
        }

        // Notion (its own database; token may be shared with the ETH integration).
        if let Ok(token) = env::var("BTC_NOTION_TOKEN").or_else(|_| env::var("NOTION_TOKEN")) {
            btc.notion.token = token;
        }
        if let Ok(database_id) = env::var("BTC_NOTION_DATABASE_ID") {
            btc.notion.database_id = database_id;
        }
        if let Ok(cold_wallet) = env::var("BTC_COLD_WALLET_ADDRESS") {
            btc.notion.cold_wallet_address = cold_wallet.clone();
            btc.withdrawal.cold_wallet_address = cold_wallet;
        }

        // Withdrawal (BTC-specific network/address/threshold).
        if let Ok(enabled) = env::var("BTC_WITHDRAWAL_ENABLED") {
            btc.withdrawal.enabled = enabled.parse().unwrap_or(false);
        }
        if let Ok(wallet) = env::var("BTC_WITHDRAWAL_WALLET_ADDRESS") {
            btc.withdrawal.cold_wallet_address = wallet;
        }
        if let Ok(network) = env::var("BTC_WITHDRAWAL_NETWORK") {
            btc.withdrawal.network = network;
        }
        if let Ok(threshold) = env::var("BTC_WITHDRAWAL_MIN_THRESHOLD") {
            btc.withdrawal.min_eth_threshold = threshold.parse()?;
        }
        if let Ok(amount) = env::var("BTC_WITHDRAWAL_AMOUNT") {
            btc.withdrawal.withdrawal_amount = Some(amount.parse()?);
        }

        config.btc = Some(btc);
    }

    Ok(config)
}

fn validate_config(config: &Config) -> Result<()> {
    match config.exchange {
        ExchangeKind::Binance => {
            if config.binance.api_key.is_empty() || config.binance.secret_key.is_empty() {
                return Err(anyhow::anyhow!(
                    "Invalid Binance API credentials (set BINANCE_API_KEY and BINANCE_SECRET_KEY)"
                ));
            }
        }
        ExchangeKind::Kraken => {
            if config.kraken.api_key.is_empty() || config.kraken.secret_key.is_empty() {
                return Err(anyhow::anyhow!(
                    "Invalid Kraken API credentials (set KRAKEN_API_KEY and KRAKEN_SECRET_KEY)"
                ));
            }
        }
    }
    if config.trading.buy_amount_eur <= rust_decimal::Decimal::ZERO {
        return Err(anyhow::anyhow!("Invalid DCA_AMOUNT_EUR"));
    }
    if config.trading.min_balance_usdc < rust_decimal::Decimal::ZERO {
        return Err(anyhow::anyhow!("Invalid MIN_BALANCE_USDC"));
    }

    // Validate Notion configuration if provided
    if !config.notion.token.is_empty() && config.notion.database_id.is_empty() {
        return Err(anyhow::anyhow!(
            "NOTION_DATABASE_ID is required when NOTION_TOKEN is provided"
        ));
    }
    if !config.notion.database_id.is_empty() && config.notion.token.is_empty() {
        return Err(anyhow::anyhow!(
            "NOTION_TOKEN is required when NOTION_DATABASE_ID is provided"
        ));
    }

    // Validate Withdrawal configuration if enabled
    if config.withdrawal.enabled {
        if config.withdrawal.cold_wallet_address.is_empty() {
            return Err(anyhow::anyhow!(
                "WITHDRAWAL_WALLET_ADDRESS is required when withdrawal is enabled"
            ));
        }
        if config.withdrawal.network.is_empty() {
            return Err(anyhow::anyhow!(
                "WITHDRAWAL_NETWORK is required when withdrawal is enabled"
            ));
        }
        if config.withdrawal.min_eth_threshold < rust_decimal::Decimal::ZERO {
            return Err(anyhow::anyhow!(
                "WITHDRAWAL_MIN_ETH_THRESHOLD must be positive"
            ));
        }
        if let Some(amount) = config.withdrawal.withdrawal_amount {
            if amount <= rust_decimal::Decimal::ZERO {
                return Err(anyhow::anyhow!(
                    "WITHDRAWAL_AMOUNT must be positive if specified"
                ));
            }
        }
        info!(
            "Withdrawal configuration validated - enabled for {} network",
            config.withdrawal.network
        );
    } else {
        info!("Withdrawal is disabled");
    }

    // Validate the optional BTC workflow.
    if let Some(btc) = &config.btc {
        if btc.trading.buy_amount_eur <= rust_decimal::Decimal::ZERO {
            return Err(anyhow::anyhow!("Invalid BTC_DCA_AMOUNT_EUR"));
        }
        if btc.trading.min_balance_usdc < rust_decimal::Decimal::ZERO {
            return Err(anyhow::anyhow!("Invalid BTC_MIN_BALANCE_USDC"));
        }
        if !btc.notion.token.is_empty() && btc.notion.database_id.is_empty() {
            return Err(anyhow::anyhow!(
                "BTC_NOTION_DATABASE_ID is required when a Notion token is provided"
            ));
        }
        if !btc.notion.database_id.is_empty() && btc.notion.token.is_empty() {
            return Err(anyhow::anyhow!(
                "BTC_NOTION_TOKEN (or NOTION_TOKEN) is required when BTC_NOTION_DATABASE_ID is provided"
            ));
        }
        if btc.withdrawal.enabled {
            if btc.withdrawal.cold_wallet_address.is_empty() {
                return Err(anyhow::anyhow!(
                    "BTC_WITHDRAWAL_WALLET_ADDRESS is required when BTC withdrawal is enabled"
                ));
            }
            if btc.withdrawal.network.is_empty() {
                return Err(anyhow::anyhow!(
                    "BTC_WITHDRAWAL_NETWORK is required when BTC withdrawal is enabled"
                ));
            }
        }
        info!(
            "BTC DCA workflow enabled (symbol {}, schedule {}, collection {})",
            btc.trading.symbol, btc.schedule.cron_expression, btc.mongo_collection
        );
    }

    info!("Configuration validated successfully");
    Ok(())
}
