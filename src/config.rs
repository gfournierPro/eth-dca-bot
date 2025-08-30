use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub binance: BinanceConfig,
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
            binance: BinanceConfig {
                api_key: String::new(),
                secret_key: String::new(),
                base_url: "https://api.binance.com".to_string(),
            },
            trading: TradingConfig {
                symbol: "ETHUSDC".to_string(),
                buy_amount_eur: Decimal::new(100, 0), // Default to €100
                min_balance_usdc: Decimal::new(50, 0), // Default to $50
                max_slippage: Decimal::new(1, 2),      // Default to 1%
            },
            schedule: ScheduleConfig {
                cron_expression: "0 30 7 * * MON".to_string(),
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
                withdrawal_amount: None, // Withdraw all available ETH
            },
        }
    }
}
