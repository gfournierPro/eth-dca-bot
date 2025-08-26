mod binance;
mod config;
mod dca;
mod dca_stats;

use anyhow::Result;
use config::Config;
use dca::DcaTrader;
use dotenv::dotenv;
use std::env;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};
use tracing_subscriber;

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

    let dca_trader = DcaTrader::new(binance_client, config.trading.clone()).await?;

    info!("Testing Binance API connection...");
    match dca_trader.binance_client.get_usdc_balanc().await {
        Ok(balance) => {
            info!("Current USDC balance: {}", balance);
            dca_trader.show_dca_summary().await.unwrap_or_else(|e| {
                error!("Failed to load DCA summary: {}", e);
            })
        }
        Err(e) => {
            error!("Failed to connect to Binance API: {}", e);
            return Err(e);
        }
    };

    let sched = JobScheduler::new().await?;

    let job = Job::new_async(
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
    )?;

    sched.add(job).await?;
    sched.start().await?;
    info!(
        "DCA Bot is running. Scheduled for: {}",
        config.schedule.cron_expression
    );
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

    if let Ok(amount) = env::var("DCA_AMOUNT_USDC") {
        config.trading.buy_amount_usdc = amount.parse()?;
    }
    if let Ok(min_balance) = env::var("MIN_BALANCE_USDC") {
        config.trading.min_balance_usdc = min_balance.parse()?;
    }
    if let Ok(cron) = env::var("SCHEDULE_CRON") {
        config.schedule.cron_expression = cron;
    }

    Ok(config)
}

fn validate_config(config: &Config) -> Result<()> {
    if config.binance.api_key.is_empty() || config.binance.secret_key.is_empty() {
        return Err(anyhow::anyhow!("Invalid Binance API credentials"));
    }
    if config.trading.buy_amount_usdc <= rust_decimal::Decimal::ZERO {
        return Err(anyhow::anyhow!("Invalid DCA_AMOUNT_USDC"));
    }
    if config.trading.min_balance_usdc < rust_decimal::Decimal::ZERO {
        return Err(anyhow::anyhow!("Invalid MIN_BALANCE_USDC"));
    }
    info!("Configuration validated successfully");
    Ok(())
}
