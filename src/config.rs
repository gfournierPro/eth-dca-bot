use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::levels::VolumeProfileConfig;

/// Which exchange backend the bot trades on. Both are kept so the active exchange
/// can be flipped via the `EXCHANGE` env var without code changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ExchangeKind {
    Binance,
    Kraken,
}

impl ExchangeKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "binance" => Some(Self::Binance),
            "kraken" => Some(Self::Kraken),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Selected exchange backend (`binance` or `kraken`).
    pub exchange: ExchangeKind,
    pub binance: BinanceConfig,
    pub kraken: KrakenConfig,
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
    /// Volume-profile tunables handed to [`crate::levels`].
    pub volume_profile: VolumeProfileConfig,
}

impl LimitSleeveConfig {
    /// Sensible ETH defaults for the sleeve. Off unless `LIMIT_SLEEVE_ENABLED=true`
    /// flips it on in `load_config`, where these get overridden from env.
    pub fn eth_default() -> Self {
        Self {
            asset: "ETH".to_string(),
            symbol: "ETHUSDC".to_string(),
            war_chest_usdc: Decimal::new(500, 0), // $500 war chest
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
            war_chest_usdc: Decimal::new(500, 0), // $500 war chest
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
    /// Volatility threshold to consider "high" (standard deviations)
    pub volatility_threshold: Decimal,
    /// Multiplier when volatility is low (<1.0 decreases purchase amount)
    pub low_volatility_multiplier: Decimal,
    /// Low volatility threshold (below this reduces purchase)
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
                volatility_scaling_enabled: true,
                volatility_period: 30,
                high_volatility_multiplier: Decimal::new(110, 2), // 1.1x (10% increase)
                volatility_threshold: Decimal::new(2, 0), // 2 standard deviations
                low_volatility_multiplier: Decimal::new(95, 2), // 0.95x (5% decrease)
                low_volatility_threshold: Decimal::new(15, 1), // 1.5 standard deviations

                rsi_enabled: true,
                rsi_period: 14,
                rsi_oversold_threshold: Decimal::new(30, 0),
                rsi_oversold_multiplier: Decimal::new(107, 2), // 1.07x (7% increase)
                rsi_overbought_threshold: Decimal::new(70, 0),
                rsi_overbought_multiplier: Decimal::new(93, 2), // 0.93x (7% decrease)

                price_deviation_enabled: true,
                moving_average_period: 20,
                deviation_threshold_percent: Decimal::new(5, 0), // 5%
                below_ma_multiplier: Decimal::new(105, 2), // 1.05x (5% increase)
                above_ma_threshold_percent: Decimal::new(8, 0), // 8%
                above_ma_multiplier: Decimal::new(92, 2), // 0.92x (8% decrease)

                momentum_enabled: true,
                momentum_period: 7,
                negative_momentum_threshold: Decimal::new(-5, 0), // -5%
                negative_momentum_multiplier: Decimal::new(108, 2), // 1.08x (8% increase)
                positive_momentum_threshold: Decimal::new(15, 0), // 15%
                positive_momentum_multiplier: Decimal::new(90, 2), // 0.90x (10% decrease)

                max_total_multiplier: Decimal::new(130, 2), // 1.3x maximum (30% increase)
                min_total_multiplier: Decimal::new(70, 2), // 0.7x minimum (30% decrease)
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
                volatility_scaling_enabled: true,
                volatility_period: 30,
                high_volatility_multiplier: Decimal::new(110, 2), // 1.1x (10% increase)
                volatility_threshold: Decimal::new(2, 0), // 2 standard deviations
                low_volatility_multiplier: Decimal::new(95, 2), // 0.95x (5% decrease)
                low_volatility_threshold: Decimal::new(15, 1), // 1.5 standard deviations
                
                rsi_enabled: true,
                rsi_period: 14,
                rsi_oversold_threshold: Decimal::new(30, 0),
                rsi_oversold_multiplier: Decimal::new(107, 2), // 1.07x (7% increase)
                rsi_overbought_threshold: Decimal::new(70, 0),
                rsi_overbought_multiplier: Decimal::new(93, 2), // 0.93x (7% decrease)
                
                price_deviation_enabled: true,
                moving_average_period: 20,
                deviation_threshold_percent: Decimal::new(5, 0), // 5%
                below_ma_multiplier: Decimal::new(105, 2), // 1.05x (5% increase)
                above_ma_threshold_percent: Decimal::new(8, 0), // 8%
                above_ma_multiplier: Decimal::new(92, 2), // 0.92x (8% decrease)
                
                momentum_enabled: true,
                momentum_period: 7,
                negative_momentum_threshold: Decimal::new(-5, 0), // -5%
                negative_momentum_multiplier: Decimal::new(108, 2), // 1.08x (8% increase)
                positive_momentum_threshold: Decimal::new(15, 0), // 15%
                positive_momentum_multiplier: Decimal::new(90, 2), // 0.90x (10% decrease)
                
                max_total_multiplier: Decimal::new(130, 2), // 1.3x maximum (30% increase)
                min_total_multiplier: Decimal::new(70, 2), // 0.7x minimum (30% decrease)
            },
        }
    }
}
