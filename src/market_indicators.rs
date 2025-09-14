use rust_decimal::Decimal;
use chrono::{DateTime, Utc};
use anyhow::Result;
use crate::binance::BinanceClient;
use crate::dca_stats_mongo::DcaStatsDB;
use tracing::{info, debug, warn};
use std::collections::VecDeque;
use reqwest;
use serde_json::Value;

/// Market indicators configuration for dynamic DCA sizing
#[derive(Debug, Clone)]
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

impl Default for MarketIndicatorsConfig {
    fn default() -> Self {
        Self {
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
        }
    }
}

/// Historical price data point
#[derive(Debug, Clone)]
pub struct PriceData {
    pub timestamp: DateTime<Utc>,
    pub price: Decimal,
    pub volume: Option<Decimal>,
}

/// Market indicators calculator
#[derive(Debug, Clone)]
pub struct MarketIndicators {
    config: MarketIndicatorsConfig,
    price_history: VecDeque<PriceData>,
}

impl MarketIndicators {
    pub fn new(config: MarketIndicatorsConfig) -> Self {
        Self {
            config,
            price_history: VecDeque::new(),
        }
    }

    /// Calculate dynamic DCA multiplier based on market conditions
    pub async fn calculate_dca_multiplier(
        &mut self,
        binance_client: &BinanceClient,
        _stats_db: &DcaStatsDB,
        symbol: &str,
    ) -> Result<Decimal> {
        // Update price history with latest data
        self.update_price_history(binance_client, symbol).await?;
        
        let mut total_multiplier = Decimal::ONE;
        
        // Calculate individual multipliers
        if self.config.volatility_scaling_enabled {
            let volatility_multiplier = self.calculate_volatility_multiplier()?;
            total_multiplier *= volatility_multiplier;
            debug!("Volatility multiplier: {}", volatility_multiplier);
        }
        
        if self.config.rsi_enabled {
            let rsi_multiplier = self.calculate_rsi_multiplier()?;
            total_multiplier *= rsi_multiplier;
            debug!("RSI multiplier: {}", rsi_multiplier);
        }
        
        if self.config.price_deviation_enabled {
            let deviation_multiplier = self.calculate_price_deviation_multiplier()?;
            total_multiplier *= deviation_multiplier;
            debug!("Price deviation multiplier: {}", deviation_multiplier);
        }
        
        if self.config.momentum_enabled {
            let momentum_multiplier = self.calculate_momentum_multiplier()?;
            total_multiplier *= momentum_multiplier;
            debug!("Momentum multiplier: {}", momentum_multiplier);
        }
        
        // Apply bounds
        total_multiplier = total_multiplier.max(self.config.min_total_multiplier);
        total_multiplier = total_multiplier.min(self.config.max_total_multiplier);
        
        info!("Final DCA multiplier: {}", total_multiplier);
        Ok(total_multiplier)
    }

    /// Update price history with recent data
    async fn update_price_history(
        &mut self,
        binance_client: &BinanceClient,
        symbol: &str,
    ) -> Result<()> {
        // If we have insufficient historical data, fetch from external API
        let max_period = self.config.volatility_period
            .max(self.config.rsi_period)
            .max(self.config.moving_average_period)
            .max(self.config.momentum_period);
        
        if self.price_history.len() < max_period as usize {
            info!("Insufficient historical data, fetching from external API...");
            match self.fetch_historical_prices(symbol, max_period + 5).await {
                Ok(historical_data) => {
                    info!("Successfully fetched {} historical price points", historical_data.len());
                    self.price_history = historical_data.into();
                }
                Err(e) => {
                    warn!("Failed to fetch historical data: {}, using current price only", e);
                }
            }
        }
        
        // Always add current price from Binance
        let current_price = binance_client.get_symbol_price(symbol).await?;
        let current_time = Utc::now();
        
        // Add current price to history
        self.price_history.push_back(PriceData {
            timestamp: current_time,
            price: current_price,
            volume: None,
        });
        
        // Keep only the data we need (max period + some buffer)
        let max_history_size = (max_period * 2) as usize; // 2x buffer
        
        while self.price_history.len() > max_history_size {
            self.price_history.pop_front();
        }
        
        debug!("Price history updated, size: {}", self.price_history.len());
        Ok(())
    }

    /// Fetch historical price data from external API (CoinGecko)
    async fn fetch_historical_prices(&self, symbol: &str, days: u32) -> Result<Vec<PriceData>> {
        let client = reqwest::Client::new();
        
        // Map symbol to CoinGecko format
        let coin_id = match symbol {
            "ETHUSDC" | "ETHUSDT" | "ETH" => "ethereum",
            "BTCUSDC" | "BTCUSDT" | "BTC" => "bitcoin",
            _ => {
                return Err(anyhow::anyhow!("Unsupported symbol for historical data: {}", symbol));
            }
        };
        
        let url = format!(
            "https://api.coingecko.com/api/v3/coins/{}/market_chart?vs_currency=usd&days={}&interval=daily",
            coin_id, days
        );
        
        debug!("Fetching historical data from: {}", url);
        
        let response: Value = client.get(&url)
            .header("User-Agent", "eth-dca-bot/1.0")
            .send()
            .await?
            .json()
            .await?;
        
        let mut price_data = Vec::new();
        
        if let Some(prices) = response["prices"].as_array() {
            for price_point in prices.iter() {
                if let Some(price_array) = price_point.as_array() {
                    if let (Some(timestamp), Some(price)) = (
                        price_array.get(0).and_then(|t| t.as_f64()),
                        price_array.get(1).and_then(|p| p.as_f64())
                    ) {
                        let datetime = DateTime::from_timestamp(timestamp as i64 / 1000, 0)
                            .unwrap_or_else(|| Utc::now());
                        
                        let price_decimal = Decimal::try_from(price)
                            .unwrap_or(Decimal::new(0, 0));
                        
                        price_data.push(PriceData {
                            timestamp: datetime,
                            price: price_decimal,
                            volume: None,
                        });
                    }
                }
            }
        }
        
        // Sort by timestamp (oldest first)
        price_data.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        
        if price_data.is_empty() {
            return Err(anyhow::anyhow!("No historical price data received"));
        }
        
        debug!("Fetched {} historical price points from {} to {}", 
               price_data.len(),
               price_data.first().map(|p| p.timestamp.format("%Y-%m-%d").to_string()).unwrap_or_default(),
               price_data.last().map(|p| p.timestamp.format("%Y-%m-%d").to_string()).unwrap_or_default());
        
        Ok(price_data)
    }

    /// Fetch current EUR/USD exchange rate from Binance
    pub async fn fetch_eur_usd_rate(binance_client: &BinanceClient) -> Result<Decimal> {
        let rate = binance_client.get_symbol_price("EURUSDC").await?;
        debug!("Current EUR/USD rate: {}", rate);
        Ok(rate)
    }

    /// Calculate volatility-based multiplier
    fn calculate_volatility_multiplier(&self) -> Result<Decimal> {
        if self.price_history.len() < self.config.volatility_period as usize {
            debug!("Insufficient data for volatility calculation, using 1.0 multiplier");
            return Ok(Decimal::ONE);
        }
        
        let recent_prices: Vec<Decimal> = self.price_history
            .iter()
            .rev()
            .take(self.config.volatility_period as usize)
            .map(|p| p.price)
            .collect();
        
        let volatility = self.calculate_price_volatility(&recent_prices)?;
        let mean_price = recent_prices.iter().sum::<Decimal>() / Decimal::new(recent_prices.len() as i64, 0);
        let volatility_ratio = volatility / mean_price;
        
        debug!("Price volatility: {}, Mean price: {}, Volatility ratio: {}", 
               volatility, mean_price, volatility_ratio);
        
        if volatility_ratio > self.config.volatility_threshold {
            // High volatility - increase purchase (opportunity)
            Ok(self.config.high_volatility_multiplier)
        } else if volatility_ratio < self.config.low_volatility_threshold / Decimal::new(100, 0) {
            // Low volatility - decrease purchase (potential overpricing/stagnation)
            Ok(self.config.low_volatility_multiplier)
        } else {
            // Normal volatility
            Ok(Decimal::ONE)
        }
    }

    /// Calculate RSI-based multiplier
    fn calculate_rsi_multiplier(&self) -> Result<Decimal> {
        if self.price_history.len() < (self.config.rsi_period + 1) as usize {
            debug!("Insufficient data for RSI calculation, using 1.0 multiplier");
            return Ok(Decimal::ONE);
        }
        
        let recent_prices: Vec<Decimal> = self.price_history
            .iter()
            .rev()
            .take((self.config.rsi_period + 1) as usize)
            .map(|p| p.price)
            .collect();
        
        let rsi = self.calculate_rsi(&recent_prices)?;
        debug!("Current RSI: {}", rsi);
        
        if rsi < self.config.rsi_oversold_threshold {
            // Oversold - increase purchase (good buying opportunity)
            Ok(self.config.rsi_oversold_multiplier)
        } else if rsi > self.config.rsi_overbought_threshold {
            // Overbought - decrease purchase (potentially overpriced)
            Ok(self.config.rsi_overbought_multiplier)
        } else {
            // Neutral RSI
            Ok(Decimal::ONE)
        }
    }

    /// Calculate price deviation multiplier
    fn calculate_price_deviation_multiplier(&self) -> Result<Decimal> {
        if self.price_history.len() < self.config.moving_average_period as usize {
            debug!("Insufficient data for moving average calculation, using 1.0 multiplier");
            return Ok(Decimal::ONE);
        }
        
        let recent_prices: Vec<Decimal> = self.price_history
            .iter()
            .rev()
            .take(self.config.moving_average_period as usize)
            .map(|p| p.price)
            .collect();
        
        let moving_average = recent_prices.iter().sum::<Decimal>() / Decimal::new(recent_prices.len() as i64, 0);
        let current_price = recent_prices[0]; // Most recent price
        
        let deviation_percent = ((current_price - moving_average) / moving_average) * Decimal::new(100, 0);
        
        debug!("Current price: {}, Moving average: {}, Deviation: {}%", 
               current_price, moving_average, deviation_percent);
        
        if deviation_percent < -self.config.deviation_threshold_percent {
            // Price below MA - increase purchase (good buying opportunity)
            Ok(self.config.below_ma_multiplier)
        } else if deviation_percent > self.config.above_ma_threshold_percent {
            // Price well above MA - decrease purchase (potentially overpriced)
            Ok(self.config.above_ma_multiplier)
        } else {
            // Price near MA
            Ok(Decimal::ONE)
        }
    }

    /// Calculate momentum-based multiplier
    fn calculate_momentum_multiplier(&self) -> Result<Decimal> {
        if self.price_history.len() < (self.config.momentum_period + 1) as usize {
            debug!("Insufficient data for momentum calculation, using 1.0 multiplier");
            return Ok(Decimal::ONE);
        }
        
        let recent_prices: Vec<Decimal> = self.price_history
            .iter()
            .rev()
            .take((self.config.momentum_period + 1) as usize)
            .map(|p| p.price)
            .collect();
        
        let current_price = recent_prices[0];
        let old_price = recent_prices[self.config.momentum_period as usize];
        
        let momentum_percent = ((current_price - old_price) / old_price) * Decimal::new(100, 0);
        
        debug!("Momentum over {} periods: {}%", self.config.momentum_period, momentum_percent);
        
        if momentum_percent < self.config.negative_momentum_threshold {
            // Negative momentum - increase purchase (opportunity during downtrend)
            Ok(self.config.negative_momentum_multiplier)
        } else if momentum_percent > self.config.positive_momentum_threshold {
            // Strong positive momentum - decrease purchase (potentially overheated)
            Ok(self.config.positive_momentum_multiplier)
        } else {
            // Neutral momentum
            Ok(Decimal::ONE)
        }
    }

    /// Calculate price volatility (standard deviation)
    fn calculate_price_volatility(&self, prices: &[Decimal]) -> Result<Decimal> {
        if prices.is_empty() {
            return Ok(Decimal::ZERO);
        }
        
        let mean = prices.iter().sum::<Decimal>() / Decimal::new(prices.len() as i64, 0);
        let variance = prices.iter()
            .map(|price| {
                let diff = *price - mean;
                diff * diff
            })
            .sum::<Decimal>() / Decimal::new(prices.len() as i64, 0);
        
        // Simple square root approximation for Decimal
        Ok(self.decimal_sqrt(variance))
    }

    /// Calculate RSI (Relative Strength Index)
    fn calculate_rsi(&self, prices: &[Decimal]) -> Result<Decimal> {
        if prices.len() < 2 {
            return Ok(Decimal::new(50, 0)); // Neutral RSI
        }
        
        let mut gains = Vec::new();
        let mut losses = Vec::new();
        
        for i in 1..prices.len() {
            let change = prices[i-1] - prices[i]; // Note: reversed because prices are in reverse order
            if change > Decimal::ZERO {
                gains.push(change);
                losses.push(Decimal::ZERO);
            } else {
                gains.push(Decimal::ZERO);
                losses.push(-change);
            }
        }
        
        let avg_gain = gains.iter().sum::<Decimal>() / Decimal::new(gains.len() as i64, 0);
        let avg_loss = losses.iter().sum::<Decimal>() / Decimal::new(losses.len() as i64, 0);
        
        if avg_loss == Decimal::ZERO {
            return Ok(Decimal::new(100, 0)); // Maximum RSI
        }
        
        let rs = avg_gain / avg_loss;
        let rsi = Decimal::new(100, 0) - (Decimal::new(100, 0) / (Decimal::ONE + rs));
        
        Ok(rsi)
    }

    /// Simple square root approximation for Decimal
    fn decimal_sqrt(&self, value: Decimal) -> Decimal {
        if value <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        
        // Newton's method for square root
        let mut x = value / Decimal::new(2, 0);
        let epsilon = Decimal::new(1, 10); // 0.0000000001
        
        for _ in 0..50 { // Max iterations
            let new_x = (x + value / x) / Decimal::new(2, 0);
            if (new_x - x).abs() < epsilon {
                break;
            }
            x = new_x;
        }
        
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volatility_calculation() {
        let indicators = MarketIndicators::new(MarketIndicatorsConfig::default());
        
        // Add some sample price data
        let prices = vec![
            Decimal::new(2000, 0),
            Decimal::new(2100, 0),
            Decimal::new(1950, 0),
            Decimal::new(2200, 0),
            Decimal::new(1800, 0),
        ];
        
        let volatility = indicators.calculate_price_volatility(&prices).unwrap();
        assert!(volatility > Decimal::ZERO);
    }

    #[test]
    fn test_rsi_calculation() {
        let indicators = MarketIndicators::new(MarketIndicatorsConfig::default());
        
        // Add sample prices (declining trend should give low RSI)
        let prices = vec![
            Decimal::new(2000, 0), // Most recent
            Decimal::new(2050, 0),
            Decimal::new(2100, 0),
            Decimal::new(2150, 0),
            Decimal::new(2200, 0), // Oldest
        ];
        
        let rsi = indicators.calculate_rsi(&prices).unwrap();
        assert!(rsi >= Decimal::ZERO && rsi <= Decimal::new(100, 0));
    }

    #[test]
    fn test_decimal_sqrt() {
        let indicators = MarketIndicators::new(MarketIndicatorsConfig::default());
        
        let sqrt_4 = indicators.decimal_sqrt(Decimal::new(4, 0));
        assert!((sqrt_4 - Decimal::new(2, 0)).abs() < Decimal::new(1, 5)); // Within 0.00001
        
        let sqrt_9 = indicators.decimal_sqrt(Decimal::new(9, 0));
        assert!((sqrt_9 - Decimal::new(3, 0)).abs() < Decimal::new(1, 5)); // Within 0.00001
    }
}
