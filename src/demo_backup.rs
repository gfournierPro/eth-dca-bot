use rust_decimal::Decimal;
use reqwest;
use serde_json::Value;
use anyhow::Result;

/// Fetch real current ETH market conditions for demonstration
pub async fn simulate_current_dca_calculation() {
    println!("🔍 ETH Dynamic DCA Simulation - LIVE Market Analysis");
    println!("{}", "=".repeat(60));
    
    match fetch_live_market_data().await {
        Ok(market_data) => {
            analyze_market_conditions(market_data).await;
        }
        Err(e) => {
            println!("❌ Failed to fetch live market data: {}", e);
            println!("🔄 Falling back to simulated data...");
            fallback_demo_with_simulated_data().await;
        }
    }
}

#[derive(Debug)]
struct LiveMarketData {
    current_price: Decimal,
    price_24h_ago: Decimal,
    price_7d_ago: Decimal,
    prices_30d: Vec<Decimal>,
    volume_24h: Decimal,
}

async fn fetch_live_market_data() -> Result<LiveMarketData> {
    println!("📡 Fetching live ETH market data from CoinGecko API...");
    
    let client = reqwest::Client::new();
    
    // Fetch current price and 24h data
    let current_url = "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=usd&include_24hr_change=true&include_24hr_vol=true";
    let current_response: Value = client.get(current_url)
        .send()
        .await?
        .json()
        .await?;
    
    let current_price = Decimal::try_from(
        current_response["ethereum"]["usd"].as_f64().unwrap_or(2400.0)
    ).unwrap_or(Decimal::new(2400, 0));
    
    let volume_24h = Decimal::try_from(
        current_response["ethereum"]["usd_24h_vol"].as_f64().unwrap_or(10000000.0)
    ).unwrap_or(Decimal::new(10000000, 0));
    
    // Fetch 30-day historical data
    let historical_url = "https://api.coingecko.com/api/v3/coins/ethereum/market_chart?vs_currency=usd&days=30&interval=daily";
    let historical_response: Value = client.get(historical_url)
        .send()
        .await?
        .json()
        .await?;
    
    let mut prices_30d = Vec::new();
    if let Some(prices) = historical_response["prices"].as_array() {
        for price_point in prices.iter().take(30) {
            if let Some(price_array) = price_point.as_array() {
                if let Some(price) = price_array.get(1).and_then(|p| p.as_f64()) {
                    prices_30d.push(Decimal::try_from(price).unwrap_or(Decimal::new(2400, 0)));
                }
            }
        }
    }
    
    // Calculate 7d and 24h ago prices
    let price_7d_ago = prices_30d.get(prices_30d.len().saturating_sub(7))
        .copied()
        .unwrap_or(current_price);
    
    let price_24h_ago = prices_30d.get(prices_30d.len().saturating_sub(1))
        .copied()
        .unwrap_or(current_price);
    
    println!("✅ Successfully fetched live market data!");
    
    Ok(LiveMarketData {
        current_price,
        price_24h_ago,
        price_7d_ago,
        prices_30d,
        volume_24h,
    })
}

async fn analyze_market_conditions(market_data: LiveMarketData) {
    let base_dca_amount_eur = Decimal::new(100, 0); // 100 EUR base DCA
    let eur_usd_rate = Decimal::new(110, 2); // 1.10 EUR/USD (approximate)
    
    println!("📊 Base Configuration:");
    println!("   Base DCA Amount: {} EUR", base_dca_amount_eur);
    println!("   Current ETH Price: ${}", market_data.current_price);
    println!("   EUR/USD Rate: {}", eur_usd_rate);
    println!("   Base USDC Amount: {} USDC", base_dca_amount_eur * eur_usd_rate);
    println!("   24h Volume: ${}", market_data.volume_24h);
    println!();
    
    println!("🧠 LIVE Market Indicator Analysis:");
    
    // Calculate each multiplier using real market data
    let config = crate::market_indicators::MarketIndicatorsConfig::default();
    
    let volatility_multiplier = calculate_real_volatility(&market_data, &config);
    let rsi_multiplier = calculate_real_rsi(&market_data, &config);
    let deviation_multiplier = calculate_real_deviation(&market_data, &config);
    let momentum_multiplier = calculate_real_momentum(&market_data, &config);
    
    // Calculate total multiplier with safety limits
    let mut total_multiplier = volatility_multiplier * rsi_multiplier * deviation_multiplier * momentum_multiplier;
    total_multiplier = total_multiplier.max(config.min_total_multiplier);
    total_multiplier = total_multiplier.min(config.max_total_multiplier);
    
    println!();
    println!("📈 Final Calculation:");
    println!("   Combined Multiplier: {:.3}", total_multiplier);
    
    let base_usdc_amount = base_dca_amount_eur * eur_usd_rate;
    let adjusted_usdc_amount = base_usdc_amount * total_multiplier;
    let adjusted_eur_amount = adjusted_usdc_amount / eur_usd_rate;
    
    let eth_amount = adjusted_usdc_amount / market_data.current_price;
    
    println!("   Adjusted EUR Amount: {} EUR", adjusted_eur_amount.round_dp(2));
    println!("   Adjusted USDC Amount: {} USDC", adjusted_usdc_amount.round_dp(2));
    println!("   ETH to Purchase: {} ETH", eth_amount.round_dp(6));
    
    let difference = adjusted_eur_amount - base_dca_amount_eur;
    let percentage_change = (difference / base_dca_amount_eur) * Decimal::new(100, 0);
    
    println!();
    if percentage_change > Decimal::ZERO {
        println!("✅ INCREASE: +{} EUR (+{}%)", difference.round_dp(2), percentage_change.round_dp(1));
        println!("   Market conditions suggest buying MORE due to favorable indicators");
    } else if percentage_change < Decimal::ZERO {
        println!("⚠️  DECREASE: {} EUR ({}%)", difference.round_dp(2), percentage_change.round_dp(1));
        println!("   Market conditions suggest buying LESS due to unfavorable indicators");
    } else {
        println!("➡️  NO CHANGE: Market conditions are neutral");
    }
    
    println!();
    println!("🎯 Summary:");
    println!("   Instead of buying {} EUR worth of ETH,", base_dca_amount_eur);
    println!("   the dynamic DCA would buy {} EUR worth of ETH", adjusted_eur_amount.round_dp(2));
    println!("   Purchasing {} ETH at current price", eth_amount.round_dp(6));
    
    // Additional market context
    let change_24h = ((market_data.current_price - market_data.price_24h_ago) / market_data.price_24h_ago) * Decimal::new(100, 0);
    let change_7d = ((market_data.current_price - market_data.price_7d_ago) / market_data.price_7d_ago) * Decimal::new(100, 0);
    
    println!();
    println!("📈 Market Context:");
    println!("   24h Price Change: {}%", change_24h.round_dp(2));
    println!("   7d Price Change: {}%", change_7d.round_dp(2));
    println!("   Data Source: CoinGecko API (Live)");
}

fn calculate_real_volatility(market_data: &LiveMarketData, config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    if market_data.prices_30d.len() < 10 {
        println!("   📊 Volatility Analysis: Insufficient data, using 1.0x multiplier");
        return Decimal::ONE;
    }
    
    // Calculate 30-day standard deviation
    let mean = market_data.prices_30d.iter().sum::<Decimal>() / Decimal::new(market_data.prices_30d.len() as i64, 0);
    let variance = market_data.prices_30d.iter()
        .map(|price| (*price - mean) * (*price - mean))
        .sum::<Decimal>() / Decimal::new(market_data.prices_30d.len() as i64, 0);
    
    let std_dev = decimal_sqrt(variance);
    let volatility_ratio = std_dev / mean;
    
    println!("   📊 Volatility Analysis:");
    println!("      30-day price std dev: ${}", std_dev.round_dp(2));
    println!("      Volatility ratio: {}", volatility_ratio.round_dp(3));
    
    if volatility_ratio > config.volatility_threshold / Decimal::new(100, 0) { // Convert threshold to ratio
        println!("      ✅ HIGH volatility detected -> {}x multiplier", config.high_volatility_multiplier);
        config.high_volatility_multiplier
    } else {
        println!("      ➡️  Normal volatility -> 1.0x multiplier");
        Decimal::ONE
    }
}

fn calculate_real_rsi(market_data: &LiveMarketData, config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    if market_data.prices_30d.len() < 15 {
        println!("   📈 RSI Analysis: Insufficient data, using 1.0x multiplier");
        return Decimal::ONE;
    }
    
    // Calculate RSI using last 14 periods
    let recent_prices = &market_data.prices_30d[market_data.prices_30d.len().saturating_sub(15)..];
    
    let mut gains = Vec::new();
    let mut losses = Vec::new();
    
    for i in 1..recent_prices.len() {
        let change = recent_prices[i] - recent_prices[i-1];
        if change > Decimal::ZERO {
            gains.push(change);
            losses.push(Decimal::ZERO);
        } else {
            gains.push(Decimal::ZERO);
            losses.push(-change);
        }
    }
    
    let avg_gain = if gains.is_empty() { Decimal::ZERO } else { gains.iter().sum::<Decimal>() / Decimal::new(gains.len() as i64, 0) };
    let avg_loss = if losses.is_empty() { Decimal::ZERO } else { losses.iter().sum::<Decimal>() / Decimal::new(losses.len() as i64, 0) };
    
    let rsi = if avg_loss == Decimal::ZERO {
        Decimal::new(100, 0)
    } else {
        let rs = avg_gain / avg_loss;
        Decimal::new(100, 0) - (Decimal::new(100, 0) / (Decimal::ONE + rs))
    };
    
    println!("   📈 RSI Analysis:");
    println!("      Current 14-day RSI: {}", rsi.round_dp(1));
    
    if rsi < config.rsi_oversold_threshold {
        println!("      ✅ OVERSOLD detected (RSI < {}) -> {}x multiplier", 
                config.rsi_oversold_threshold, config.rsi_oversold_multiplier);
        config.rsi_oversold_multiplier
    } else {
        println!("      ➡️  RSI not oversold -> 1.0x multiplier");
        Decimal::ONE
    }
}

fn calculate_real_deviation(market_data: &LiveMarketData, config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    if market_data.prices_30d.len() < 20 {
        println!("   📉 Price Deviation Analysis: Insufficient data, using 1.0x multiplier");
        return Decimal::ONE;
    }
    
    // Calculate 20-day moving average
    let recent_20_prices = &market_data.prices_30d[market_data.prices_30d.len().saturating_sub(20)..];
    let ma_20 = recent_20_prices.iter().sum::<Decimal>() / Decimal::new(recent_20_prices.len() as i64, 0);
    
    let deviation_percent = ((market_data.current_price - ma_20) / ma_20) * Decimal::new(100, 0);
    
    println!("   📉 Price Deviation Analysis:");
    println!("      Current Price: ${}", market_data.current_price);
    println!("      20-day MA: ${}", ma_20.round_dp(2));
    println!("      Deviation: {}%", deviation_percent.round_dp(1));
    
    if deviation_percent < -config.deviation_threshold_percent {
        println!("      ✅ BELOW MA (>{}% below) -> {}x multiplier", 
                config.deviation_threshold_percent, config.below_ma_multiplier);
        config.below_ma_multiplier
    } else {
        println!("      ➡️  Price above/near MA -> 1.0x multiplier");
        Decimal::ONE
    }
}

fn calculate_real_momentum(market_data: &LiveMarketData, config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    let momentum_percent = ((market_data.current_price - market_data.price_7d_ago) / market_data.price_7d_ago) * Decimal::new(100, 0);
    
    println!("   🚀 Momentum Analysis:");
    println!("      7-day price change: {}%", momentum_percent.round_dp(1));
    
    if momentum_percent < config.negative_momentum_threshold {
        println!("      ✅ NEGATIVE momentum (<{}%) -> {}x multiplier", 
                config.negative_momentum_threshold, config.negative_momentum_multiplier);
        config.negative_momentum_multiplier
    } else {
        println!("      ➡️  Positive/neutral momentum -> 1.0x multiplier");
        Decimal::ONE
    }
}

// Simple square root approximation for Decimal
fn decimal_sqrt(value: Decimal) -> Decimal {
    if value <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    
    let mut x = value / Decimal::new(2, 0);
    let epsilon = Decimal::new(1, 10);
    
    for _ in 0..50 {
        let new_x = (x + value / x) / Decimal::new(2, 0);
        if (new_x - x).abs() < epsilon {
            break;
        }
        x = new_x;
    }
    
    x
}

// Fallback demo with simulated data if API fails
async fn fallback_demo_with_simulated_data() {
    println!("🔍 ETH Dynamic DCA Simulation - Simulated Market Analysis");
    println!("{}", "=".repeat(60));
    
    // Simulate current ETH price data (approximate real values as of Sept 2024)
    let current_price = Decimal::new(2400, 0); // $2400 ETH
    let base_dca_amount_eur = Decimal::new(100, 0); // 100 EUR base DCA
    let eur_usdc_rate = Decimal::new(110, 2); // 1.10 EUR/USD
    
    println!("📊 Base Configuration:");
    println!("   Base DCA Amount: {} EUR", base_dca_amount_eur);
    println!("   Current ETH Price: ${}", current_price);
    println!("   EUR/USD Rate: {}", eur_usdc_rate);
    println!("   Base USDC Amount: {} USDC", base_dca_amount_eur * eur_usdc_rate);
    println!();
    
    println!("🧠 Market Indicator Analysis:");
    
    // Calculate each multiplier individually using default config
    let config = crate::market_indicators::MarketIndicatorsConfig::default();
    
    let volatility_multiplier = calculate_volatility_demo(&config);
    let rsi_multiplier = calculate_rsi_demo(&config);
    let deviation_multiplier = calculate_deviation_demo(&config);
    let momentum_multiplier = calculate_momentum_demo(&config);
    
    // Calculate total multiplier with safety limits
    let mut total_multiplier = volatility_multiplier * rsi_multiplier * deviation_multiplier * momentum_multiplier;
    total_multiplier = total_multiplier.max(config.min_total_multiplier);
    total_multiplier = total_multiplier.min(config.max_total_multiplier);
    
    println!();
    println!("📈 Final Calculation:");
    println!("   Combined Multiplier: {:.3}", total_multiplier);
    
    let base_usdc_amount = base_dca_amount_eur * eur_usdc_rate;
    let adjusted_usdc_amount = base_usdc_amount * total_multiplier;
    let adjusted_eur_amount = adjusted_usdc_amount / eur_usdc_rate;
    
    let eth_amount = adjusted_usdc_amount / current_price;
    
    println!("   Adjusted EUR Amount: {} EUR", adjusted_eur_amount.round_dp(2));
    println!("   Adjusted USDC Amount: {} USDC", adjusted_usdc_amount.round_dp(2));
    println!("   ETH to Purchase: {} ETH", eth_amount.round_dp(6));
    
    let difference = adjusted_eur_amount - base_dca_amount_eur;
    let percentage_change = (difference / base_dca_amount_eur) * Decimal::new(100, 0);
    
    println!();
    if percentage_change > Decimal::ZERO {
        println!("✅ INCREASE: +{} EUR (+{}%)", difference.round_dp(2), percentage_change.round_dp(1));
        println!("   Market conditions suggest buying MORE due to favorable indicators");
    } else if percentage_change < Decimal::ZERO {
        println!("⚠️  DECREASE: {} EUR ({}%)", difference.round_dp(2), percentage_change.round_dp(1));
        println!("   Market conditions suggest buying LESS due to unfavorable indicators");
    } else {
        println!("➡️  NO CHANGE: Market conditions are neutral");
    }
    
    println!();
    println!("🎯 Summary:");
    println!("   Instead of buying {} EUR worth of ETH,", base_dca_amount_eur);
    println!("   the dynamic DCA would buy {} EUR worth of ETH", adjusted_eur_amount.round_dp(2));
    println!("   Purchasing {} ETH at current price", eth_amount.round_dp(6));
}
    
    // Calculate each multiplier individually using default config
    let config = crate::market_indicators::MarketIndicatorsConfig::default();
    
    let volatility_multiplier = calculate_volatility_demo(&config);
    let rsi_multiplier = calculate_rsi_demo(&config);
    let deviation_multiplier = calculate_deviation_demo(&config);
    let momentum_multiplier = calculate_momentum_demo(&config);
    
    // Calculate total multiplier with safety limits
    let mut total_multiplier = volatility_multiplier * rsi_multiplier * deviation_multiplier * momentum_multiplier;
    total_multiplier = total_multiplier.max(config.min_total_multiplier);
    total_multiplier = total_multiplier.min(config.max_total_multiplier);
    
    println!();
    println!("📈 Final Calculation:");
    println!("   Combined Multiplier: {:.3}", total_multiplier);
    
    let base_usdc_amount = base_dca_amount_eur * eur_usdc_rate;
    let adjusted_usdc_amount = base_usdc_amount * total_multiplier;
    let adjusted_eur_amount = adjusted_usdc_amount / eur_usdc_rate;
    
    let eth_amount = adjusted_usdc_amount / current_price;
    
    println!("   Adjusted EUR Amount: {} EUR", adjusted_eur_amount.round_dp(2));
    println!("   Adjusted USDC Amount: {} USDC", adjusted_usdc_amount.round_dp(2));
    println!("   ETH to Purchase: {} ETH", eth_amount.round_dp(6));
    
    let difference = adjusted_eur_amount - base_dca_amount_eur;
    let percentage_change = (difference / base_dca_amount_eur) * Decimal::new(100, 0);
    
    println!();
    if percentage_change > Decimal::ZERO {
        println!("✅ INCREASE: +{} EUR (+{}%)", difference.round_dp(2), percentage_change.round_dp(1));
        println!("   Market conditions suggest buying MORE due to favorable indicators");
    } else if percentage_change < Decimal::ZERO {
        println!("⚠️  DECREASE: {} EUR ({}%)", difference.round_dp(2), percentage_change.round_dp(1));
        println!("   Market conditions suggest buying LESS due to unfavorable indicators");
    } else {
        println!("➡️  NO CHANGE: Market conditions are neutral");
    }
    
    println!();
    println!("🎯 Summary:");
    println!("   Instead of buying {} EUR worth of ETH,", base_dca_amount_eur);
    println!("   the dynamic DCA would buy {} EUR worth of ETH", adjusted_eur_amount.round_dp(2));
    println!("   Purchasing {} ETH at current price", eth_amount.round_dp(6));
}

fn calculate_volatility_demo(config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    // For demo, let's assume moderate volatility
    let volatility_ratio = Decimal::new(15, 1); // 1.5 (moderate volatility)
    
    println!("   📊 Volatility Analysis:");
    println!("      30-day volatility ratio: {}", volatility_ratio);
    
    if volatility_ratio > config.volatility_threshold {
        println!("      ✅ HIGH volatility detected -> {}x multiplier", config.high_volatility_multiplier);
        config.high_volatility_multiplier
    } else {
        println!("      ➡️  Normal volatility -> 1.0x multiplier");
        Decimal::ONE
    }
}

fn calculate_rsi_demo(config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    // For demo, let's simulate RSI calculation
    let current_rsi = Decimal::new(25, 0); // RSI of 25 (oversold)
    
    println!("   📈 RSI Analysis:");
    println!("      Current 14-day RSI: {}", current_rsi);
    
    if current_rsi < config.rsi_oversold_threshold {
        println!("      ✅ OVERSOLD detected (RSI < {}) -> {}x multiplier", 
                config.rsi_oversold_threshold, config.rsi_oversold_multiplier);
        config.rsi_oversold_multiplier
    } else {
        println!("      ➡️  RSI not oversold -> 1.0x multiplier");
        Decimal::ONE
    }
}

fn calculate_deviation_demo(config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    // For demo, calculate moving average
    let current_price = Decimal::new(2400, 0);
    let ma_20 = Decimal::new(2550, 0); // 20-day MA higher than current (price below MA)
    let deviation_percent = ((current_price - ma_20) / ma_20) * Decimal::new(100, 0);
    
    println!("   📉 Price Deviation Analysis:");
    println!("      Current Price: ${}", current_price);
    println!("      20-day MA: ${}", ma_20);
    println!("      Deviation: {}%", deviation_percent.round_dp(1));
    
    if deviation_percent < -config.deviation_threshold_percent {
        println!("      ✅ BELOW MA (>{}% below) -> {}x multiplier", 
                config.deviation_threshold_percent, config.below_ma_multiplier);
        config.below_ma_multiplier
    } else {
        println!("      ➡️  Price above/near MA -> 1.0x multiplier");
        Decimal::ONE
    }
}

fn calculate_momentum_demo(config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    // For demo, calculate 7-day momentum
    let current_price = Decimal::new(2400, 0);
    let price_7_days_ago = Decimal::new(2580, 0); // Price was higher 7 days ago (negative momentum)
    let momentum_percent = ((current_price - price_7_days_ago) / price_7_days_ago) * Decimal::new(100, 0);
    
    println!("   🚀 Momentum Analysis:");
    println!("      7-day price change: {}%", momentum_percent.round_dp(1));
    
    if momentum_percent < config.negative_momentum_threshold {
        println!("      ✅ NEGATIVE momentum (<{}%) -> {}x multiplier", 
                config.negative_momentum_threshold, config.negative_momentum_multiplier);
        config.negative_momentum_multiplier
    } else {
        println!("      ➡️  Positive/neutral momentum -> 1.0x multiplier");
        Decimal::ONE
    }
}
