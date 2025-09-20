use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub binance: BinanceConfig,
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
                withdrawal_amount: None, // Withdraw all available ETH
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
