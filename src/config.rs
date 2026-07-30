use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::levels::{DrawdownTier, VolumeProfileConfig};

/// Which exchange backend the bot trades on. Both are kept so the active exchange
/// can be flipped via the `EXCHANGE` env var without code changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ExchangeKind {
    Binance,
    Kraken,
    Okx,
}

impl ExchangeKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "binance" => Some(Self::Binance),
            "kraken" => Some(Self::Kraken),
            "okx" => Some(Self::Okx),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Selected exchange backend (`binance`, `kraken` or `okx`).
    pub exchange: ExchangeKind,
    pub binance: BinanceConfig,
    pub kraken: KrakenConfig,
    pub okx: OkxConfig,
    pub trading: TradingConfig,
    pub schedule: ScheduleConfig,
    pub notion: NotionConfig,
    pub withdrawal: WithdrawalConfig,
    /// Dynamic DCA sizing (volatility/RSI/moving-average/momentum) for the ETH workflow.
    pub market_indicators: MarketIndicatorsConfig,
    /// Optional second asset (BTC) DCA workflow, run alongside ETH.
    pub btc: Option<AssetDcaConfig>,
    /// Optional limit-order sleeve for the primary (ETH) asset. Off by default;
    /// fully isolated from the DCA core (own budget, own Mongo collection).
    pub limit_sleeve: Option<LimitSleeveConfig>,
    /// Optional limit-order sleeve for BTC, run alongside the ETH sleeve. Same
    /// isolation guarantees; separated on Kraken by a distinct `userref`.
    pub btc_limit_sleeve: Option<LimitSleeveConfig>,
}

/// Configuration for the optional limit-order sleeve.
///
/// The sleeve rests post-only bids at volume-profile levels below spot, funded by
/// a fixed USDC war chest that drains as dips fill (never auto-replenished). It is
/// kept isolated from the DCA core: its fills land in their own Mongo collection
/// and are tagged in the shared Notion monthly page, so DCA stats stay pure.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LimitSleeveConfig {
    /// Base asset the sleeve accumulates (e.g. "ETH"). Matches the DCA asset.
    pub asset: String,
    /// USDC-quoted trading pair the sleeve places bids on (e.g. "ETHUSDC").
    pub symbol: String,
    /// Fixed USDC war chest. Drains as bids fill; the sleeve goes quiet when empty.
    pub war_chest_usdc: Decimal,
    /// 7-field cron (with seconds) for recomputing levels and reconciling bids.
    pub refresh_cron: String,
    /// Timezone the refresh cron is evaluated in.
    pub timezone: String,
    /// OHLC candle interval in minutes. Also sets the lookback window, since
    /// Kraken's OHLC endpoint caps at ~720 candles (60 ≈ 30 days).
    pub interval_minutes: u32,
    /// Mongo collection for the sleeve's fills and persisted war-chest balance,
    /// kept separate from the DCA collections so stats never mix.
    pub mongo_collection: String,
    /// Client order reference stamped on every order this sleeve places, and the
    /// filter it uses to pick its own orders out of `OpenOrders`/`ClosedOrders`.
    /// MUST be unique per sleeve on the same Kraken account: with a shared userref
    /// each sleeve would see (and cancel) the other's bids and record the other's
    /// fills against its own war chest.
    pub userref: i32,
    /// Volume-profile tunables handed to [`crate::levels`]. Retained for the
    /// `sleeve_smoke ladder` diagnostic (and a possible later refinement that snaps
    /// tier triggers onto nearby HVNs); the live ladder is [`Self::drawdown`].
    pub volume_profile: VolumeProfileConfig,
    /// Drawdown-tier tunables — the strategy the sleeve actually places bids from.
    pub drawdown: DrawdownConfig,
}

/// Drawdown-tier strategy configuration.
///
/// The sleeve rests one post-only bid per armed tier at `anchor_high * (1 - depth)`,
/// where `anchor_high` is the rolling high over `anchor_days`. Each tier deploys a
/// fixed USDC allocation, capped by the accrued chest. See [`crate::levels`] for the
/// derivation, and why absolute allocations rather than normalised weights are the
/// whole point.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DrawdownConfig {
    /// Lookback for the rolling high the tiers hang off, in days.
    pub anchor_days: usize,
    /// Tiers, shallowest first. Depth is a fraction (`0.25` = -25%).
    pub tiers: Vec<DrawdownTier>,
    /// USDC added to the chest per elapsed month since [`Self::accrual_start`].
    /// Paces deployment so one early crash can't spend a whole year's budget, and
    /// revives the sleeve in later years instead of flattening it forever.
    pub monthly_accrual_usdc: Decimal,
    /// USDC the chest holds at [`Self::accrual_start`], before any accrual.
    pub starting_chest_usdc: Decimal,
    /// Date accrual is measured from.
    pub accrual_start: NaiveDate,
}

/// Build a tier list from `(depth, allocation_usdc)` pairs.
fn tiers(spec: &[(i64, i64)]) -> Vec<DrawdownTier> {
    spec.iter()
        .map(|(depth_pct, alloc)| DrawdownTier {
            depth: Decimal::new(*depth_pct, 2),
            allocation_usdc: Decimal::new(*alloc, 0),
        })
        .collect()
}

/// Accrual start for the shipped defaults: the month the drawdown sleeve was
/// written. Overridable via `{prefix}_ACCRUAL_START`.
fn default_accrual_start() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 8, 1).expect("valid literal date")
}

impl LimitSleeveConfig {
    /// Sensible ETH defaults for the sleeve. Off unless `LIMIT_SLEEVE_ENABLED=true`
    /// flips it on in `load_config`, where these get overridden from env.
    ///
    /// Tier depths are scaled deeper than BTC's (-35/-50/-65 vs -25/-35/-45): ETH's
    /// drawdowns run materially larger, so BTC's depths would fill on moves that are
    /// ordinary for ETH — the exact "too shallow" leak the tiers exist to avoid.
    /// These are a scaling judgement, not a backtested result (the validation run was
    /// BTC-only); revisit with an ETH backtest before funding it heavily.
    pub fn eth_default() -> Self {
        Self {
            asset: "ETH".to_string(),
            symbol: "ETHUSDC".to_string(),
            war_chest_usdc: Decimal::new(500, 0), // chest cap
            refresh_cron: "0 0 */6 * * *".to_string(), // every 6 hours
            timezone: "Europe/Berlin".to_string(),
            interval_minutes: 60, // hourly candles ≈ 30 days
            mongo_collection: "limit_sleeve_fills".to_string(),
            userref: 770_077,
            volume_profile: VolumeProfileConfig {
                bucket_size: Decimal::new(5, 0),         // $5 buckets for ETH
                hvn_threshold_ratio: Decimal::new(7, 1), // 0.7
                ladder_steps: 4,
                require_local_maxima: true,
            },
            drawdown: DrawdownConfig {
                anchor_days: 90,
                tiers: tiers(&[(35, 150), (50, 175), (65, 175)]),
                monthly_accrual_usdc: Decimal::new(4167, 2), // $500/yr
                starting_chest_usdc: Decimal::new(250, 0),
                accrual_start: default_accrual_start(),
            },
        }
    }

    /// Sensible BTC defaults for the sleeve. Mirrors [`Self::eth_default`] but
    /// targets BTCUSDC, its own fills collection, a distinct `userref` (so the two
    /// sleeves never touch each other's Kraken orders), and a BTC-scaled volume
    /// bucket — BTC trades ~30-40x ETH's price, so $5 buckets would shred its
    /// profile into noise.
    pub fn btc_default() -> Self {
        Self {
            asset: "BTC".to_string(),
            symbol: "BTCUSDC".to_string(),
            war_chest_usdc: Decimal::new(1000, 0), // chest cap
            refresh_cron: "0 0 */6 * * *".to_string(), // every 6 hours
            timezone: "Europe/Berlin".to_string(),
            interval_minutes: 60, // hourly candles ≈ 30 days
            mongo_collection: "btc_limit_sleeve_fills".to_string(),
            userref: 770_078,
            volume_profile: VolumeProfileConfig {
                bucket_size: Decimal::new(100, 0),       // $100 buckets for BTC
                hvn_threshold_ratio: Decimal::new(7, 1), // 0.7
                ladder_steps: 4,
                require_local_maxima: true,
            },
            // The backtested configuration: -25/-35/-45% at $300/$350/$350 off a 90d
            // rolling high, $1000/yr accrued monthly against a $1000 cap. Replaying
            // 2024-2026 daily candles (Feb 2025, Nov 2025 and Feb 2026 crashes) this
            // averaged ~80.2k, versus ~94k for every shallower or re-normalising
            // variant, and still held ammunition for the second and third legs.
            drawdown: DrawdownConfig {
                anchor_days: 90,
                tiers: tiers(&[(25, 300), (35, 350), (45, 350)]),
                monthly_accrual_usdc: Decimal::new(8333, 2), // $1000/yr
                starting_chest_usdc: Decimal::new(500, 0),
                accrual_start: default_accrual_start(),
            },
        }
    }
}

/// A self-contained DCA workflow for a single asset.
///
/// The original ETH workflow lives on the flat fields of [`Config`]; this struct
/// bundles the same pieces for any additional asset (currently BTC) so the bot
/// can run several DCA workflows in one process.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetDcaConfig {
    /// Base asset symbol, e.g. "ETH" or "BTC". Used for balances/withdrawals/labels.
    pub asset: String,
    /// MongoDB collection that stores this asset's purchases (kept separate per asset).
    pub mongo_collection: String,
    pub trading: TradingConfig,
    pub schedule: ScheduleConfig,
    pub notion: NotionConfig,
    pub withdrawal: WithdrawalConfig,
    pub market_indicators: MarketIndicatorsConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BinanceConfig {
    pub api_key: String,
    pub secret_key: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KrakenConfig {
    pub api_key: String,
    pub secret_key: String,
    pub base_url: String,
}

/// OKX needs a third credential: the API passphrase chosen when creating the key.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OkxConfig {
    pub api_key: String,
    pub secret_key: String,
    pub passphrase: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TradingConfig {
    pub symbol: String,
    pub buy_amount_eur: Decimal,
    pub min_balance_usdc: Decimal,
    pub max_slippage: Decimal,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScheduleConfig {
    pub cron_expression: String,
    pub timezone: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NotionConfig {
    pub token: String,
    pub database_id: String,
    pub cold_wallet_address: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WithdrawalConfig {
    pub enabled: bool,
    pub cold_wallet_address: String,
    pub network: String,
    pub min_eth_threshold: Decimal,
    pub withdrawal_amount: Option<Decimal>, // None means withdraw all available ETH
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketIndicatorsConfig {
    /// Enable volatility-based scaling
    pub volatility_scaling_enabled: bool,
    /// Volatility lookback period in days
    pub volatility_period: u32,
    /// Multiplier when volatility is high (>1.0 increases purchase amount)
    pub high_volatility_multiplier: Decimal,
    /// Volatility threshold to consider "high" (percent of mean price, e.g. 2 = 2%)
    pub volatility_threshold: Decimal,
    /// Multiplier when volatility is low (<1.0 decreases purchase amount)
    pub low_volatility_multiplier: Decimal,
    /// Low volatility threshold (percent of mean price, below this reduces purchase)
    pub low_volatility_threshold: Decimal,

    /// Enable RSI-based adjustments
    pub rsi_enabled: bool,
    /// RSI calculation period
    pub rsi_period: u32,
    /// RSI threshold below which to increase purchase (oversold)
    pub rsi_oversold_threshold: Decimal,
    /// Multiplier when RSI indicates oversold conditions
    pub rsi_oversold_multiplier: Decimal,
    /// RSI threshold above which to decrease purchase (overbought)
    pub rsi_overbought_threshold: Decimal,
    /// Multiplier when RSI indicates overbought conditions  
    pub rsi_overbought_multiplier: Decimal,

    /// Enable price deviation strategy
    pub price_deviation_enabled: bool,
    /// Moving average period for price deviation
    pub moving_average_period: u32,
    /// Percentage below MA to trigger increased purchase
    pub deviation_threshold_percent: Decimal,
    /// Multiplier when price is below moving average
    pub below_ma_multiplier: Decimal,
    /// Percentage above MA to trigger decreased purchase
    pub above_ma_threshold_percent: Decimal,
    /// Multiplier when price is above moving average
    pub above_ma_multiplier: Decimal,

    /// Enable momentum-based adjustments
    pub momentum_enabled: bool,
    /// Period for momentum calculation
    pub momentum_period: u32,
    /// Negative momentum threshold to increase purchase
    pub negative_momentum_threshold: Decimal,
    /// Multiplier during negative momentum periods
    pub negative_momentum_multiplier: Decimal,
    /// Positive momentum threshold to decrease purchase
    pub positive_momentum_threshold: Decimal,
    /// Multiplier during positive momentum periods
    pub positive_momentum_multiplier: Decimal,

    /// Maximum multiplier to prevent excessive purchases
    pub max_total_multiplier: Decimal,
    /// Minimum multiplier to ensure some purchase occurs
    pub min_total_multiplier: Decimal,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            exchange: ExchangeKind::Binance,
            binance: BinanceConfig {
                api_key: String::new(),
                secret_key: String::new(),
                base_url: "https://api.binance.com".to_string(),
            },
            kraken: KrakenConfig {
                api_key: String::new(),
                secret_key: String::new(),
                base_url: "https://api.kraken.com".to_string(),
            },
            okx: OkxConfig {
                api_key: String::new(),
                secret_key: String::new(),
                passphrase: String::new(),
                base_url: "https://www.okx.com".to_string(),
            },
            trading: TradingConfig {
                symbol: "ETHUSDC".to_string(),
                buy_amount_eur: Decimal::new(100, 0), // Default to €100
                min_balance_usdc: Decimal::new(50, 0), // Default to $50
                max_slippage: Decimal::new(1, 2),     // Default to 1%
            },
            schedule: ScheduleConfig {
                cron_expression: "0 30 5 * * MON".to_string(),
                timezone: "Europe/Berlin".to_string(),
            },
            notion: NotionConfig {
                token: String::new(),
                database_id: String::new(),
                cold_wallet_address: "0x48AE396B932D062B559B11d8fC4D973E730af1eB".to_string(),
            },
            withdrawal: WithdrawalConfig {
                enabled: false,
                cold_wallet_address: "0x48AE396B932D062B559B11d8fC4D973E730af1eB".to_string(),
                network: "ARBITRUM".to_string(), // Correct network name for Arbitrum One
                min_eth_threshold: Decimal::new(3, 4), // 0.0003 ETH minimum
                withdrawal_amount: None,         // Withdraw all available ETH
            },
            market_indicators: MarketIndicatorsConfig {
                // Off per the modulation backtest (tests/dca_modulation_backtest.rs):
                // the 30d coefficient-of-variation almost always clears the 2% "high"
                // bar, so this fired near-permanently — uniform ~1.1x on every buy,
                // no average-entry benefit, ~9% budget overspend. RSI/MA/momentum
                // carry the entry improvement without it.
                volatility_scaling_enabled: false,
                volatility_period: 30,
                high_volatility_multiplier: Decimal::new(110, 2), // 1.1x (10% increase)
                volatility_threshold: Decimal::new(2, 0),         // 2% of mean price
                low_volatility_multiplier: Decimal::new(95, 2),   // 0.95x (5% decrease)
                low_volatility_threshold: Decimal::new(15, 1),    // 1.5% of mean price

                rsi_enabled: true,
                rsi_period: 14,
                rsi_oversold_threshold: Decimal::new(30, 0),
                rsi_oversold_multiplier: Decimal::new(107, 2), // 1.07x (7% increase)
                rsi_overbought_threshold: Decimal::new(70, 0),
                rsi_overbought_multiplier: Decimal::new(93, 2), // 0.93x (7% decrease)

                price_deviation_enabled: true,
                moving_average_period: 20,
                deviation_threshold_percent: Decimal::new(5, 0), // 5%
                below_ma_multiplier: Decimal::new(105, 2),       // 1.05x (5% increase)
                above_ma_threshold_percent: Decimal::new(8, 0),  // 8%
                above_ma_multiplier: Decimal::new(92, 2),        // 0.92x (8% decrease)

                momentum_enabled: true,
                momentum_period: 7,
                negative_momentum_threshold: Decimal::new(-5, 0), // -5%
                negative_momentum_multiplier: Decimal::new(108, 2), // 1.08x (8% increase)
                positive_momentum_threshold: Decimal::new(15, 0), // 15%
                positive_momentum_multiplier: Decimal::new(90, 2), // 0.90x (10% decrease)

                max_total_multiplier: Decimal::new(130, 2), // 1.3x maximum (30% increase)
                min_total_multiplier: Decimal::new(70, 2),  // 0.7x minimum (30% decrease)
            },
            btc: None,
            limit_sleeve: None,
            btc_limit_sleeve: None,
        }
    }
}

impl Config {
    /// Build an [`AssetDcaConfig`] describing the ETH workflow from the flat
    /// config fields, so ETH and BTC can be driven through the same code path.
    pub fn eth_asset(&self) -> AssetDcaConfig {
        AssetDcaConfig {
            asset: "ETH".to_string(),
            mongo_collection: "dca_purchases".to_string(),
            trading: self.trading.clone(),
            schedule: self.schedule.clone(),
            notion: self.notion.clone(),
            withdrawal: self.withdrawal.clone(),
            market_indicators: self.market_indicators.clone(),
        }
    }
}

impl AssetDcaConfig {
    /// Sensible defaults for a BTC DCA workflow. Mirrors the ETH defaults but
    /// targets BTCUSDC, a dedicated Mongo collection, and the native BTC network.
    pub fn btc_default() -> Self {
        Self {
            asset: "BTC".to_string(),
            mongo_collection: "btc_purchases".to_string(),
            trading: TradingConfig {
                symbol: "BTCUSDC".to_string(),
                buy_amount_eur: Decimal::new(100, 0),
                min_balance_usdc: Decimal::new(50, 0),
                max_slippage: Decimal::new(1, 2),
            },
            schedule: ScheduleConfig {
                cron_expression: "0 30 5 * * MON".to_string(),
                timezone: "Europe/Berlin".to_string(),
            },
            notion: NotionConfig {
                token: String::new(),
                database_id: String::new(),
                cold_wallet_address: String::new(),
            },
            withdrawal: WithdrawalConfig {
                enabled: false,
                cold_wallet_address: String::new(),
                network: "BTC".to_string(), // Native Bitcoin network
                min_eth_threshold: Decimal::new(1, 4), // 0.0001 BTC minimum (field name is generic threshold)
                withdrawal_amount: None,               // Withdraw all available BTC
            },
            market_indicators: MarketIndicatorsConfig {
                // Off per the modulation backtest (tests/dca_modulation_backtest.rs):
                // the 30d coefficient-of-variation almost always clears the 2% "high"
                // bar, so this fired near-permanently — uniform ~1.1x on every buy,
                // no average-entry benefit, ~9% budget overspend. RSI/MA/momentum
                // carry the entry improvement without it.
                volatility_scaling_enabled: false,
                volatility_period: 30,
                high_volatility_multiplier: Decimal::new(110, 2), // 1.1x (10% increase)
                volatility_threshold: Decimal::new(2, 0),         // 2% of mean price
                low_volatility_multiplier: Decimal::new(95, 2),   // 0.95x (5% decrease)
                low_volatility_threshold: Decimal::new(15, 1),    // 1.5% of mean price

                rsi_enabled: true,
                rsi_period: 14,
                rsi_oversold_threshold: Decimal::new(30, 0),
                rsi_oversold_multiplier: Decimal::new(107, 2), // 1.07x (7% increase)
                rsi_overbought_threshold: Decimal::new(70, 0),
                rsi_overbought_multiplier: Decimal::new(93, 2), // 0.93x (7% decrease)

                price_deviation_enabled: true,
                moving_average_period: 20,
                deviation_threshold_percent: Decimal::new(5, 0), // 5%
                below_ma_multiplier: Decimal::new(105, 2),       // 1.05x (5% increase)
                above_ma_threshold_percent: Decimal::new(8, 0),  // 8%
                above_ma_multiplier: Decimal::new(92, 2),        // 0.92x (8% decrease)

                momentum_enabled: true,
                momentum_period: 7,
                negative_momentum_threshold: Decimal::new(-5, 0), // -5%
                negative_momentum_multiplier: Decimal::new(108, 2), // 1.08x (8% increase)
                positive_momentum_threshold: Decimal::new(15, 0), // 15%
                positive_momentum_multiplier: Decimal::new(90, 2), // 0.90x (10% decrease)

                max_total_multiplier: Decimal::new(130, 2), // 1.3x maximum (30% increase)
                min_total_multiplier: Decimal::new(70, 2),  // 0.7x minimum (30% decrease)
            },
        }
    }
}

// --- Env overlay ------------------------------------------------------------

/// Overlay `{prefix}_*` / `{vp_prefix}_*` env vars onto a sleeve's defaults, then
/// fail fast on nonsensical values — a startup error is far easier to diagnose than
/// one at the first reconcile tick hours later (`levels.rs` guards `bucket_size`
/// internally too). The ETH sleeve reads `LIMIT_SLEEVE_*`/`VP_*`, the BTC sleeve
/// `BTC_LIMIT_SLEEVE_*`/`BTC_VP_*`.
pub fn load_sleeve_env(
    mut sleeve: LimitSleeveConfig,
    prefix: &str,
    vp_prefix: &str,
) -> anyhow::Result<LimitSleeveConfig> {
    if let Ok(v) = std::env::var(format!("{prefix}_SYMBOL")) {
        sleeve.symbol = v;
    }
    if let Ok(v) = std::env::var(format!("{prefix}_WAR_CHEST_USDC")) {
        sleeve.war_chest_usdc = v.parse()?;
    }
    if let Ok(v) = std::env::var(format!("{prefix}_REFRESH_CRON")) {
        sleeve.refresh_cron = v;
    }
    // Reuse the global TIMEZONE unless a sleeve-specific one is provided.
    sleeve.timezone = std::env::var(format!("{prefix}_TIMEZONE"))
        .ok()
        .or_else(|| std::env::var("TIMEZONE").ok())
        .unwrap_or(sleeve.timezone);
    if let Ok(v) = std::env::var(format!("{prefix}_INTERVAL_MINUTES")) {
        sleeve.interval_minutes = v.parse()?;
    }
    if let Ok(v) = std::env::var(format!("{prefix}_MONGO_COLLECTION")) {
        sleeve.mongo_collection = v;
    }

    // Volume-profile tunables. The bucket size accepts the asset-suffixed spelling
    // first for backwards compatibility (the ETH sleeve shipped as
    // `VP_BUCKET_SIZE_ETH`), then the plain `{vp_prefix}_BUCKET_SIZE`.
    if let Ok(v) = std::env::var(format!("{vp_prefix}_BUCKET_SIZE_{}", sleeve.asset))
        .or_else(|_| std::env::var(format!("{vp_prefix}_BUCKET_SIZE")))
    {
        sleeve.volume_profile.bucket_size = v.parse()?;
    }
    if let Ok(v) = std::env::var(format!("{vp_prefix}_HVN_THRESHOLD_RATIO")) {
        sleeve.volume_profile.hvn_threshold_ratio = v.parse()?;
    }
    if let Ok(v) = std::env::var(format!("{vp_prefix}_LADDER_STEPS")) {
        sleeve.volume_profile.ladder_steps = v.parse()?;
    }
    if let Ok(v) = std::env::var(format!("{vp_prefix}_REQUIRE_LOCAL_MAXIMA")) {
        sleeve.volume_profile.require_local_maxima = v.parse().unwrap_or(true);
    }

    let vp = &sleeve.volume_profile;
    if vp.bucket_size <= Decimal::ZERO {
        return Err(anyhow::anyhow!("{vp_prefix}_BUCKET_SIZE must be positive"));
    }
    if vp.ladder_steps == 0 {
        return Err(anyhow::anyhow!(
            "{vp_prefix}_LADDER_STEPS must be greater than 0"
        ));
    }
    if vp.hvn_threshold_ratio <= Decimal::ZERO
        || vp.hvn_threshold_ratio > Decimal::ONE
    {
        return Err(anyhow::anyhow!(
            "{vp_prefix}_HVN_THRESHOLD_RATIO must be in (0, 1]"
        ));
    }
    if sleeve.war_chest_usdc <= Decimal::ZERO {
        return Err(anyhow::anyhow!("{prefix}_WAR_CHEST_USDC must be positive"));
    }
    if sleeve.interval_minutes == 0 {
        return Err(anyhow::anyhow!(
            "{prefix}_INTERVAL_MINUTES must be greater than 0"
        ));
    }

    // Drawdown-tier tunables — the strategy that actually places bids.
    if let Ok(v) = std::env::var(format!("{prefix}_ANCHOR_DAYS")) {
        sleeve.drawdown.anchor_days = v.parse()?;
    }
    if let Ok(v) = std::env::var(format!("{prefix}_TIERS")) {
        sleeve.drawdown.tiers = parse_tiers(&v)
            .map_err(|e| anyhow::anyhow!("{prefix}_TIERS is invalid ({e}); expected \
                 comma-separated depth:allocation pairs like \"0.25:300,0.35:350,0.45:350\""))?;
    }
    if let Ok(v) = std::env::var(format!("{prefix}_MONTHLY_ACCRUAL_USDC")) {
        sleeve.drawdown.monthly_accrual_usdc = v.parse()?;
    }
    if let Ok(v) = std::env::var(format!("{prefix}_STARTING_CHEST_USDC")) {
        sleeve.drawdown.starting_chest_usdc = v.parse()?;
    }
    if let Ok(v) = std::env::var(format!("{prefix}_ACCRUAL_START")) {
        sleeve.drawdown.accrual_start = NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("{prefix}_ACCRUAL_START must be YYYY-MM-DD: {e}"))?;
    }

    let dd = &sleeve.drawdown;
    if dd.anchor_days == 0 {
        return Err(anyhow::anyhow!("{prefix}_ANCHOR_DAYS must be greater than 0"));
    }
    if dd.tiers.is_empty() {
        return Err(anyhow::anyhow!("{prefix}_TIERS must list at least one tier"));
    }
    // Depths must be a strictly increasing fraction in (0,1): equal-or-decreasing
    // depths would break the shallowest-first spend attribution that derives which
    // tiers are still armed, silently mis-funding tiers.
    let mut previous = Decimal::ZERO;
    for t in &dd.tiers {
        if t.depth <= Decimal::ZERO || t.depth >= Decimal::ONE {
            return Err(anyhow::anyhow!(
                "{prefix}_TIERS depths must be fractions in (0, 1), got {}",
                t.depth
            ));
        }
        if t.depth <= previous {
            return Err(anyhow::anyhow!(
                "{prefix}_TIERS depths must increase (shallowest first), got {} after {}",
                t.depth,
                previous
            ));
        }
        if t.allocation_usdc <= Decimal::ZERO {
            return Err(anyhow::anyhow!(
                "{prefix}_TIERS allocations must be positive, got {}",
                t.allocation_usdc
            ));
        }
        previous = t.depth;
    }
    if dd.monthly_accrual_usdc < Decimal::ZERO {
        return Err(anyhow::anyhow!(
            "{prefix}_MONTHLY_ACCRUAL_USDC cannot be negative"
        ));
    }
    if dd.starting_chest_usdc < Decimal::ZERO {
        return Err(anyhow::anyhow!(
            "{prefix}_STARTING_CHEST_USDC cannot be negative"
        ));
    }

    // Tiers are funded shallowest-first from whatever the chest holds, so a cap below
    // the total allocation starves the DEEPEST tier — precisely the one meant to catch
    // a capitulation. Warn rather than fail: a deliberately tiny chest is how the
    // smoke test runs. Set the cap >= the sum to have every tier resting at once.
    let total_alloc: Decimal = dd.tiers.iter().map(|t| t.allocation_usdc).sum();
    if sleeve.war_chest_usdc < total_alloc {
        tracing::warn!(
            "[sleeve:{}] {prefix}_WAR_CHEST_USDC ({}) is below the total tier allocation ({}); \
             the deepest tier(s) may never get a resting bid",
            sleeve.asset,
            sleeve.war_chest_usdc,
            total_alloc
        );
    }

    Ok(sleeve)
}

/// Parse `"0.25:300,0.35:350,0.45:350"` into tiers (depth fraction : USDC allocation).
fn parse_tiers(spec: &str) -> anyhow::Result<Vec<DrawdownTier>> {
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let (depth, alloc) = pair
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("'{pair}' is not depth:allocation"))?;
            Ok(DrawdownTier {
                depth: depth.trim().parse()?,
                allocation_usdc: alloc.trim().parse()?,
            })
        })
        .collect()
}
