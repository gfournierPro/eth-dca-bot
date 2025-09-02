mod binance;
mod config;
mod dca;
mod dca_stats_mongo;
mod notion_integration;
mod date_utils;

use anyhow::Result;
use config::Config;
use dca::DcaTrader;
use dotenv::dotenv;
use std::env;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};
use tracing_subscriber;
use chrono::Utc;
use std::str::FromStr;

fn calculate_next_execution(cron_expr: &str) -> Result<String> {
    use cron::Schedule;
    
    let schedule = Schedule::from_str(cron_expr)?;
    let now = Utc::now();
    
    if let Some(next) = schedule.upcoming(Utc).next() {
        let duration_until = next.signed_duration_since(now);
        let total_seconds = duration_until.num_seconds();
        
        if total_seconds <= 0 {
            return Ok("Now".to_string());
        }
        
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;
        
        if hours > 0 {
            Ok(format!("{}h {}m {}s", hours, minutes, seconds))
        } else if minutes > 0 {
            Ok(format!("{}m {}s", minutes, seconds))
        } else {
            Ok(format!("{}s", seconds))
        }
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

    let binance_client = binance::BinanceClient::new(
        config.binance.api_key.clone(),
        config.binance.secret_key.clone(),
        config.binance.base_url.clone(),
    );

    let dca_trader = DcaTrader::new(
        binance_client, 
        config.trading.clone(),
        config.withdrawal.clone(),
        Some(&config.notion),
        config.schedule.timezone.clone(),
        config.schedule.cron_expression.clone(),
    ).await?;

    info!("Testing Binance API connection...");
    match dca_trader.binance_client.get_usdc_balanc().await {
        Ok(balance) => {
            info!("Current USDC balance: {}", balance);
            dca_trader.show_dca_summary().await.unwrap_or_else(|e| {
                error!("Failed to load DCA summary: {}", e);
            });

            // Check for withdrawal on startup if we're in the right time period
            info!("🔍 Checking if withdrawal is needed at startup...");
            dca_trader.check_and_execute_withdrawal().await.unwrap_or_else(|e| {
                error!("Startup withdrawal check failed: {}", e);
            });

            // Check if DCA is needed (no purchase in last 24h)
            info!("🔍 Checking if startup DCA is needed...");
            dca_trader.check_and_execute_startup_dca().await.unwrap_or_else(|e| {
                error!("Startup DCA check failed: {}", e);
            });
        }
        Err(e) => {
            error!("Failed to connect to Binance API: {}", e);
            return Err(e);
        }
    };

    let sched = JobScheduler::new().await?;

    // Create timezone-aware job
    let timezone_str = config.schedule.timezone.clone();
    let job = if timezone_str == "UTC" || timezone_str.is_empty() {
        Job::new_async(
            config.schedule.cron_expression.as_str(),
            move |_uuid, _l| {
                let trader = dca_trader.clone();
                Box::pin(async move {
                    info!("Executing scheduled DCA purchase");
                    match trader.execute_dca_purchase().await {
                        Ok(()) => {
                            info!("Scheduled DCA purchase completed successfully");
                        }
                        Err(e) => {
                            error!("Scheduled DCA purchase failed: {}", e);
                        }
                    }
                })
            },
        )?
    } else {
        // Use timezone-aware job
        use chrono_tz::Tz;
        use std::str::FromStr;
        let tz = Tz::from_str(&timezone_str).unwrap_or(chrono_tz::Europe::Berlin);
        Job::new_async_tz(
            config.schedule.cron_expression.as_str(),
            tz,
            move |_uuid, _l| {
                let trader = dca_trader.clone();
                Box::pin(async move {
                    info!("Executing scheduled DCA purchase");
                    match trader.execute_dca_purchase().await {
                        Ok(()) => {
                            info!("Scheduled DCA purchase completed successfully");
                        }
                        Err(e) => {
                            error!("Scheduled DCA purchase failed: {}", e);
                        }
                    }
                })
            },
        )?
    };

    sched.add(job).await?;
    sched.start().await?;
    info!(
        "DCA Bot is running. Scheduled for: {} (timezone: {})",
        config.schedule.cron_expression,
        config.schedule.timezone
    );
    
    // Log when the next DCA batch will happen
    match calculate_next_execution(&config.schedule.cron_expression) {
        Ok(time_until) => {
            info!("⏰ Next DCA batch will execute in: {}", time_until);
        }
        Err(e) => {
            error!("Failed to calculate next execution time: {}", e);
        }
    }
    
    info!("Press Ctrl+C to stop the bot");

    // Keep the application running
    tokio::signal::ctrl_c().await?;
    info!("Shutting down DCA Bot");

    Ok(())
}

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

fn validate_config(config: &Config) -> Result<()> {
    if config.binance.api_key.is_empty() || config.binance.secret_key.is_empty() {
        return Err(anyhow::anyhow!("Invalid Binance API credentials"));
    }
    if config.trading.buy_amount_eur <= rust_decimal::Decimal::ZERO {
        return Err(anyhow::anyhow!("Invalid DCA_AMOUNT_EUR"));
    }
    if config.trading.min_balance_usdc < rust_decimal::Decimal::ZERO {
        return Err(anyhow::anyhow!("Invalid MIN_BALANCE_USDC"));
    }
    
    // Validate Notion configuration if provided
    if !config.notion.token.is_empty() && config.notion.database_id.is_empty() {
        return Err(anyhow::anyhow!("NOTION_DATABASE_ID is required when NOTION_TOKEN is provided"));
    }
    if !config.notion.database_id.is_empty() && config.notion.token.is_empty() {
        return Err(anyhow::anyhow!("NOTION_TOKEN is required when NOTION_DATABASE_ID is provided"));
    }
    
    // Validate Withdrawal configuration if enabled
    if config.withdrawal.enabled {
        if config.withdrawal.cold_wallet_address.is_empty() {
            return Err(anyhow::anyhow!("WITHDRAWAL_WALLET_ADDRESS is required when withdrawal is enabled"));
        }
        if config.withdrawal.network.is_empty() {
            return Err(anyhow::anyhow!("WITHDRAWAL_NETWORK is required when withdrawal is enabled"));
        }
        if config.withdrawal.min_eth_threshold < rust_decimal::Decimal::ZERO {
            return Err(anyhow::anyhow!("WITHDRAWAL_MIN_ETH_THRESHOLD must be positive"));
        }
        if let Some(amount) = config.withdrawal.withdrawal_amount {
            if amount <= rust_decimal::Decimal::ZERO {
                return Err(anyhow::anyhow!("WITHDRAWAL_AMOUNT must be positive if specified"));
            }
        }
        info!("Withdrawal configuration validated - enabled for {} network", config.withdrawal.network);
    } else {
        info!("Withdrawal is disabled");
    }
    
    info!("Configuration validated successfully");
    Ok(())
}
