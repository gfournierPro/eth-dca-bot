mod binance;
mod config;
mod date_utils;
mod dca;
mod dca_stats_mongo;
mod exchange;
mod kraken;
mod levels;
mod limit_sleeve;
mod market_indicators;
mod notion_integration;
mod okx;

use anyhow::Result;
use chrono::Utc;
use config::{Config, ExchangeKind};
use dca::DcaTrader;
use dotenv::dotenv;
use exchange::Exchange;
use notion_integration::NotionDCATracker;
use std::env;
use std::str::FromStr;
use std::sync::Arc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info, warn};
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
        ExchangeKind::Okx => Arc::new(okx::OkxClient::new(
            config.okx.api_key.clone(),
            config.okx.secret_key.clone(),
            config.okx.passphrase.clone(),
            config.okx.base_url.clone(),
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

    // One-time startup integrity check against Binance's own order history. Binance
    // order ids are numeric (unlike Kraken's string txids), so this only applies
    // when trading on Binance.
    if config.exchange == ExchangeKind::Binance {
        info!("🔍 Verifying database integrity with Binance records...");
        check_and_sync_database(&config).await.unwrap_or_else(|e| {
            error!("Database integrity check failed: {}", e);
        });
    }

    // BTC workflow (optional — runs alongside ETH in the same process).
    if let Some(btc_cfg) = config.btc.clone() {
        if let Err(e) = setup_asset_trader(btc_cfg, exchange.clone(), &sched).await {
            error!("Failed to set up BTC DCA workflow: {}", e);
        }
    }

    // Limit-order sleeves (optional — Kraken or OKX, isolated from the DCA core).
    if let Err(e) = setup_limit_sleeves(&config, &sched).await {
        error!("Failed to set up limit sleeves: {}", e);
    }

    sched.start().await?;
    info!("DCA Bot is running.");

    info!("Press Ctrl+C to stop the bot");

    // Keep the application running
    tokio::signal::ctrl_c().await?;
    info!("Shutting down DCA Bot");

    Ok(())
}

/// Check the local database against Binance's own order history and
/// automatically sync any trades that are missing. Binance order ids are
/// numeric (unlike Kraken's string txids), so this only applies to Binance.
async fn check_and_sync_database(config: &Config) -> Result<()> {
    use chrono::TimeZone;

    let binance_client = binance::BinanceClient::new(
        config.binance.api_key.clone(),
        config.binance.secret_key.clone(),
        config.binance.base_url.clone(),
    );
    let stats_db = dca_stats_mongo::DcaStatsDB::new().await?;

    // Define the start date and first order ID as specified by the user
    // Include the August 25th order (6683992267) which is also part of DCA history
    let start_date = Utc.with_ymd_and_hms(2025, 8, 25, 18, 11, 41).unwrap();
    let first_order_id = 6683992267_u64; // Start from the earliest DCA order

    info!(
        "📅 Checking from: {} (Order ID: {})",
        start_date.format("%Y-%m-%d %H:%M:%S UTC"),
        first_order_id
    );

    // Verify database integrity
    let (total_binance, missing_count, missing_ids) = stats_db
        .verify_database_integrity(&binance_client, "ETHUSDC", start_date)
        .await?;

    if missing_count == 0 {
        info!("✅ Database is in sync with Binance - no missing trades found");
        return Ok(());
    }

    // Missing trades found - automatically sync
    warn!("⚠️  Found {} missing trade(s) in database", missing_count);
    info!("🔄 Starting automatic sync of missing trades...");

    let added_count = stats_db
        .sync_missing_orders_from_binance(
            &binance_client,
            "ETHUSDC",
            start_date,
            Some(first_order_id),
        )
        .await?;

    info!("🎉 Database sync completed successfully!");
    info!("📊 Summary:");
    info!("   - Total Binance orders: {}", total_binance);
    info!("   - Missing orders found: {}", missing_count);
    info!("   - Orders added to database: {}", added_count);

    if !missing_ids.is_empty() {
        info!("📋 Synced order IDs: {:?}", missing_ids);
    }

    // Show updated DCA summary
    info!("📈 Updated DCA summary after sync:");
    let current_price = binance_client.get_symbol_price("ETHUSDC").await?;
    let summary = stats_db.get_summary(current_price).await?;
    dca_stats_mongo::print_dca_summary("ETH", &summary);

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

    let mut trader = DcaTrader::new(
        asset.clone(),
        &asset_cfg.mongo_collection,
        exchange,
        asset_cfg.trading.clone(),
        asset_cfg.withdrawal.clone(),
        Some(&asset_cfg.notion),
        Some(&asset_cfg.market_indicators),
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
            let mut trader = trader.clone();
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
            let mut trader = trader.clone();
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

/// Set up every configured limit-order sleeve (ETH and/or BTC). The sleeves work
/// against Kraken or OKX (both implement [`exchange::SleeveExchange`]); on any
/// other backend they're skipped with a warning. Each sleeve is isolated: one
/// failing to start doesn't take the other down.
async fn setup_limit_sleeves(config: &Config, sched: &JobScheduler) -> Result<()> {
    // Each sleeve is paired with the Notion DB it mirrors into: the ETH sleeve
    // reuses the shared (ETH) Notion DB, the BTC sleeve the BTC workflow's.
    let mut sleeves: Vec<(config::LimitSleeveConfig, config::NotionConfig)> = Vec::new();
    if let Some(s) = config.limit_sleeve.clone() {
        sleeves.push((s, config.notion.clone()));
    }
    if let Some(s) = config.btc_limit_sleeve.clone() {
        sleeves.push((s, btc_sleeve_notion(config)));
    }
    if sleeves.is_empty() {
        return Ok(());
    }

    let sleeve_exchange: Arc<dyn exchange::SleeveExchange> = match config.exchange {
        ExchangeKind::Kraken => Arc::new(kraken::KrakenClient::new(
            config.kraken.api_key.clone(),
            config.kraken.secret_key.clone(),
            config.kraken.base_url.clone(),
        )),
        ExchangeKind::Okx => Arc::new(okx::OkxClient::new(
            config.okx.api_key.clone(),
            config.okx.secret_key.clone(),
            config.okx.passphrase.clone(),
            config.okx.base_url.clone(),
        )),
        ExchangeKind::Binance => {
            warn!(
                "Limit sleeve is enabled but EXCHANGE is Binance; the sleeve requires Kraken or OKX — skipping"
            );
            return Ok(());
        }
    };
    let source_label = if config.exchange == ExchangeKind::Okx {
        "OKX"
    } else {
        "Kraken"
    };

    for (sleeve_cfg, notion_cfg) in sleeves {
        let asset = sleeve_cfg.asset.clone();
        if let Err(e) = setup_one_sleeve(
            sleeve_exchange.clone(),
            source_label,
            sleeve_cfg,
            notion_cfg,
            sched,
        )
        .await
        {
            error!("[sleeve:{}] setup failed: {}", asset, e);
        }
    }
    Ok(())
}

/// Notion DB the BTC sleeve mirrors into: the BTC DCA workflow's DB when that
/// workflow is configured, otherwise resolved from the `BTC_NOTION_*` env vars
/// directly (the sleeve can run without the BTC DCA workflow).
fn btc_sleeve_notion(config: &Config) -> config::NotionConfig {
    if let Some(btc) = &config.btc {
        return btc.notion.clone();
    }
    config::NotionConfig {
        token: env::var("BTC_NOTION_TOKEN")
            .or_else(|_| env::var("NOTION_TOKEN"))
            .unwrap_or_default(),
        database_id: env::var("BTC_NOTION_DATABASE_ID").unwrap_or_default(),
        cold_wallet_address: String::new(),
    }
}

/// Set up one limit-order sleeve: its own Mongo collection, a startup reconcile,
/// and the recurring reconcile job, against the shared sleeve-capable exchange
/// client built by [`setup_limit_sleeves`].
async fn setup_one_sleeve(
    exchange: Arc<dyn exchange::SleeveExchange>,
    source_label: &str,
    sleeve_cfg: config::LimitSleeveConfig,
    notion_cfg: config::NotionConfig,
    sched: &JobScheduler,
) -> Result<()> {
    let asset = sleeve_cfg.asset.clone();

    // Notion mirror; fills are tagged "Limit Sleeve Fill" inside the monthly page.
    // Absent config just means Mongo-only recording.
    let notion = if !notion_cfg.token.is_empty() && !notion_cfg.database_id.is_empty() {
        match NotionDCATracker::new(&notion_cfg, &sleeve_cfg.asset, source_label) {
            Ok(tracker) => Some(tracker),
            Err(e) => {
                warn!("[sleeve:{}] Notion mirror disabled: {}", asset, e);
                None
            }
        }
    } else {
        None
    };

    let sleeve = limit_sleeve::LimitSleeve::new(exchange, sleeve_cfg.clone())
        .await?
        .with_notion(notion);

    info!(
        "[sleeve:{}] enabled on {} (war chest {} USDC, userref {})",
        asset, sleeve_cfg.symbol, sleeve_cfg.war_chest_usdc, sleeve_cfg.userref
    );

    // Reconcile once at startup (records any fills, places the initial ladder).
    if let Err(e) = sleeve.reconcile().await {
        error!("[sleeve:{}] startup reconcile failed: {}", asset, e);
    }

    // Schedule the recurring reconcile.
    let cron = sleeve_cfg.refresh_cron.clone();
    let timezone_str = sleeve_cfg.timezone.clone();

    let job = if timezone_str == "UTC" || timezone_str.is_empty() {
        let sleeve = sleeve.clone();
        let asset = asset.clone();
        Job::new_async(cron.as_str(), move |_uuid, _l| {
            let sleeve = sleeve.clone();
            let asset = asset.clone();
            Box::pin(async move {
                info!("[sleeve:{}] running scheduled reconcile", asset);
                match sleeve.reconcile().await {
                    Ok(()) => info!("[sleeve:{}] reconcile complete", asset),
                    Err(e) => error!("[sleeve:{}] reconcile failed: {}", asset, e),
                }
            })
        })?
    } else {
        use chrono_tz::Tz;
        let tz = Tz::from_str(&timezone_str).unwrap_or(chrono_tz::Europe::Berlin);
        let sleeve = sleeve.clone();
        let asset = asset.clone();
        Job::new_async_tz(cron.as_str(), tz, move |_uuid, _l| {
            let sleeve = sleeve.clone();
            let asset = asset.clone();
            Box::pin(async move {
                info!("[sleeve:{}] running scheduled reconcile", asset);
                match sleeve.reconcile().await {
                    Ok(()) => info!("[sleeve:{}] reconcile complete", asset),
                    Err(e) => error!("[sleeve:{}] reconcile failed: {}", asset, e),
                }
            })
        })?
    };

    sched.add(job).await?;
    info!(
        "[sleeve:{}] reconcile job scheduled: {} (timezone: {})",
        asset, cron, timezone_str
    );
    Ok(())
}

fn load_config() -> Result<Config> {
    let mut config = Config::default();

    // Select the exchange backend (defaults to Binance for backwards compatibility).
    if let Ok(exchange) = env::var("EXCHANGE") {
        config.exchange = ExchangeKind::parse(&exchange).ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid EXCHANGE '{}' (expected 'binance', 'kraken' or 'okx')",
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
    if let Ok(v) = env::var("OKX_API_KEY") {
        config.okx.api_key = v;
    }
    if let Ok(v) = env::var("OKX_SECRET_KEY") {
        config.okx.secret_key = v;
    }
    if let Ok(v) = env::var("OKX_PASSPHRASE") {
        config.okx.passphrase = v;
    }
    if let Ok(v) = env::var("OKX_BASE_URL") {
        config.okx.base_url = v;
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

    // BTC DCA workflow runs by default alongside ETH; set BTC_DCA_ENABLED=false to opt out.
    if env::var("BTC_DCA_ENABLED")
        .map(|v| v == "true")
        .unwrap_or(true)
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

    // Load the optional limit-order sleeves (additive; off unless enabled). Fully
    // isolated from the DCA core — own budget, own Mongo collection per sleeve.
    if env::var("LIMIT_SLEEVE_ENABLED")
        .map(|v| v == "true")
        .unwrap_or(false)
    {
        config.limit_sleeve = Some(load_sleeve_env(
            config::LimitSleeveConfig::eth_default(),
            "LIMIT_SLEEVE",
            "VP",
        )?);
    }
    if env::var("BTC_LIMIT_SLEEVE_ENABLED")
        .map(|v| v == "true")
        .unwrap_or(false)
    {
        config.btc_limit_sleeve = Some(load_sleeve_env(
            config::LimitSleeveConfig::btc_default(),
            "BTC_LIMIT_SLEEVE",
            "BTC_VP",
        )?);
    }

    Ok(config)
}

/// Overlay `{prefix}_*` / `{vp_prefix}_*` env vars onto a sleeve's defaults, then
/// fail fast on nonsensical values — a startup error is far easier to diagnose than
/// one at the first reconcile tick hours later (`levels.rs` guards `bucket_size`
/// internally too). The ETH sleeve reads `LIMIT_SLEEVE_*`/`VP_*`, the BTC sleeve
/// `BTC_LIMIT_SLEEVE_*`/`BTC_VP_*`.
fn load_sleeve_env(
    mut sleeve: config::LimitSleeveConfig,
    prefix: &str,
    vp_prefix: &str,
) -> Result<config::LimitSleeveConfig> {
    if let Ok(v) = env::var(format!("{prefix}_SYMBOL")) {
        sleeve.symbol = v;
    }
    if let Ok(v) = env::var(format!("{prefix}_WAR_CHEST_USDC")) {
        sleeve.war_chest_usdc = v.parse()?;
    }
    if let Ok(v) = env::var(format!("{prefix}_REFRESH_CRON")) {
        sleeve.refresh_cron = v;
    }
    // Reuse the global TIMEZONE unless a sleeve-specific one is provided.
    sleeve.timezone = env::var(format!("{prefix}_TIMEZONE"))
        .ok()
        .or_else(|| env::var("TIMEZONE").ok())
        .unwrap_or(sleeve.timezone);
    if let Ok(v) = env::var(format!("{prefix}_INTERVAL_MINUTES")) {
        sleeve.interval_minutes = v.parse()?;
    }
    if let Ok(v) = env::var(format!("{prefix}_MONGO_COLLECTION")) {
        sleeve.mongo_collection = v;
    }

    // Volume-profile tunables. The bucket size accepts the asset-suffixed spelling
    // first for backwards compatibility (the ETH sleeve shipped as
    // `VP_BUCKET_SIZE_ETH`), then the plain `{vp_prefix}_BUCKET_SIZE`.
    if let Ok(v) = env::var(format!("{vp_prefix}_BUCKET_SIZE_{}", sleeve.asset))
        .or_else(|_| env::var(format!("{vp_prefix}_BUCKET_SIZE")))
    {
        sleeve.volume_profile.bucket_size = v.parse()?;
    }
    if let Ok(v) = env::var(format!("{vp_prefix}_HVN_THRESHOLD_RATIO")) {
        sleeve.volume_profile.hvn_threshold_ratio = v.parse()?;
    }
    if let Ok(v) = env::var(format!("{vp_prefix}_LADDER_STEPS")) {
        sleeve.volume_profile.ladder_steps = v.parse()?;
    }
    if let Ok(v) = env::var(format!("{vp_prefix}_REQUIRE_LOCAL_MAXIMA")) {
        sleeve.volume_profile.require_local_maxima = v.parse().unwrap_or(true);
    }

    let vp = &sleeve.volume_profile;
    if vp.bucket_size <= rust_decimal::Decimal::ZERO {
        return Err(anyhow::anyhow!("{vp_prefix}_BUCKET_SIZE must be positive"));
    }
    if vp.ladder_steps == 0 {
        return Err(anyhow::anyhow!(
            "{vp_prefix}_LADDER_STEPS must be greater than 0"
        ));
    }
    if vp.hvn_threshold_ratio <= rust_decimal::Decimal::ZERO
        || vp.hvn_threshold_ratio > rust_decimal::Decimal::ONE
    {
        return Err(anyhow::anyhow!(
            "{vp_prefix}_HVN_THRESHOLD_RATIO must be in (0, 1]"
        ));
    }
    if sleeve.war_chest_usdc <= rust_decimal::Decimal::ZERO {
        return Err(anyhow::anyhow!("{prefix}_WAR_CHEST_USDC must be positive"));
    }
    if sleeve.interval_minutes == 0 {
        return Err(anyhow::anyhow!(
            "{prefix}_INTERVAL_MINUTES must be greater than 0"
        ));
    }

    Ok(sleeve)
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
        ExchangeKind::Okx => {
            if config.okx.api_key.is_empty()
                || config.okx.secret_key.is_empty()
                || config.okx.passphrase.is_empty()
            {
                return Err(anyhow::anyhow!(
                    "Invalid OKX API credentials (set OKX_API_KEY, OKX_SECRET_KEY and OKX_PASSPHRASE)"
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

    // The two sleeves must never share Kraken order tags or a fills collection: a
    // shared userref means each sleeve sees (and cancels) the other's bids; a shared
    // collection means each counts the other's fills against its own war chest.
    if let (Some(eth), Some(btc)) = (&config.limit_sleeve, &config.btc_limit_sleeve) {
        if eth.userref == btc.userref {
            return Err(anyhow::anyhow!(
                "ETH and BTC limit sleeves must use distinct userrefs (got {} for both)",
                eth.userref
            ));
        }
        if eth.mongo_collection == btc.mongo_collection {
            return Err(anyhow::anyhow!(
                "ETH and BTC limit sleeves must use distinct Mongo collections \
                 (LIMIT_SLEEVE_MONGO_COLLECTION vs BTC_LIMIT_SLEEVE_MONGO_COLLECTION, got '{}')",
                eth.mongo_collection
            ));
        }
    }

    info!("Configuration validated successfully");
    Ok(())
}
