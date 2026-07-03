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
    /// Optional second asset (BTC) DCA workflow, run alongside ETH.
    pub btc: Option<AssetDcaConfig>,
    /// Optional limit-order sleeve for the primary (ETH) asset. Off by default;
    /// fully isolated from the DCA core (own budget, own Mongo collection).
    pub limit_sleeve: Option<LimitSleeveConfig>,
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
            volume_profile: VolumeProfileConfig {
                bucket_size: Decimal::new(5, 0),         // $5 buckets for ETH
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
                cold_wallet_address: "0xa416610975634033374EEdAE26D0FCa7A7360b70".to_string(),
            },
            withdrawal: WithdrawalConfig {
                enabled: false,
                cold_wallet_address: "0xa416610975634033374EEdAE26D0FCa7A7360b70".to_string(),
                network: "ARBITRUM".to_string(), // Correct network name for Arbitrum One
                min_eth_threshold: Decimal::new(3, 4), // 0.0003 ETH minimum
                withdrawal_amount: None,         // Withdraw all available ETH
            },
            btc: None,
            limit_sleeve: None,
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
        }
    }
}
