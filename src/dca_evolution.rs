use rust_decimal::Decimal;
use std::collections::HashMap;

/// Simulate how dynamic DCA evolves over multiple weekly purchases
pub async fn simulate_weekly_dca_evolution() {
    println!("📈 Dynamic DCA Evolution Simulation - 12 Week Analysis");
    println!("{}", "=".repeat(70));
    println!();
    
    // Base configuration
    let base_dca_amount_eur = Decimal::new(100, 0);
    let eur_usd_rate = Decimal::new(11768, 4); // 1.1768 from our live fetch
    
    println!("🔧 Configuration:");
    println!("   Weekly DCA Amount: {} EUR", base_dca_amount_eur);
    println!("   EUR/USD Rate: {}", eur_usd_rate);
    println!("   Analysis Period: 12 weeks");
    println!();
    
    // Simulate different market scenarios over 12 weeks
    let market_scenarios = create_market_scenarios();
    
    let mut total_eur_spent = Decimal::ZERO;
    let mut total_eth_accumulated = Decimal::ZERO;
    let mut week_summary = Vec::new();
    
    println!("📊 Weekly Evolution Analysis:");
    println!("{}", "-".repeat(120));
    println!("{:<4} {:<12} {:<8} {:<8} {:<8} {:<8} {:<10} {:<10} {:<15} {:<15}", 
             "Week", "Scenario", "ETH $", "RSI", "Vol%", "Mom%", "Mult", "EUR", "ETH Buy", "Cum ETH");
    println!("{}", "-".repeat(120));
    
    for week in 1..=12 {
        let scenario = &market_scenarios[week - 1];
        
        // Calculate market indicators for this week
        let config = crate::market_indicators::MarketIndicatorsConfig::default();
        
        let volatility_mult = calculate_volatility_multiplier(scenario.volatility_percent, &config);
        let rsi_mult = calculate_rsi_multiplier(scenario.rsi, &config);
        let deviation_mult = calculate_deviation_multiplier(scenario.price_vs_ma_percent, &config);
        let momentum_mult = calculate_momentum_multiplier(scenario.momentum_percent, &config);
        
        // Calculate total multiplier with safety bounds
        let mut total_multiplier = volatility_mult * rsi_mult * deviation_mult * momentum_mult;
        total_multiplier = total_multiplier.max(config.min_total_multiplier);
        total_multiplier = total_multiplier.min(config.max_total_multiplier);
        
        // Calculate purchase amounts
        let adjusted_eur = base_dca_amount_eur * total_multiplier;
        let usdc_amount = adjusted_eur * eur_usd_rate;
        let eth_purchased = usdc_amount / scenario.eth_price;
        
        // Update totals
        total_eur_spent += adjusted_eur;
        total_eth_accumulated += eth_purchased;
        
        // Store week summary
        week_summary.push(WeekSummary {
            week: week as u32,
            scenario_name: scenario.name.clone(),
            eth_price: scenario.eth_price,
            multiplier: total_multiplier,
            eur_spent: adjusted_eur,
            eth_purchased,
            cumulative_eth: total_eth_accumulated,
            market_conditions: scenario.clone(),
        });
        
        // Print week details
        println!("{:<4} {:<12} {:<8} {:<8} {:<8} {:<8} {:<10} {:<10} {:<15} {:<15}",
                week,
                scenario.name,
                format!("${}", scenario.eth_price.round_dp(0)),
                scenario.rsi.round_dp(0),
                format!("{}%", scenario.volatility_percent.round_dp(1)),
                format!("{}%", scenario.momentum_percent.round_dp(1)),
                format!("{:.2}x", total_multiplier),
                format!("€{}", adjusted_eur.round_dp(0)),
                format!("{:.4}", eth_purchased),
                format!("{:.4}", total_eth_accumulated)
        );
    }
    
    println!("{}", "-".repeat(120));
    println!();
    
    // Calculate final statistics
    let average_eth_price = calculate_average_price(&week_summary);
    let final_eth_price = week_summary.last().unwrap().market_conditions.eth_price;
    let portfolio_value_eur = total_eth_accumulated * final_eth_price / eur_usd_rate;
    let total_return = ((portfolio_value_eur - total_eur_spent) / total_eur_spent) * Decimal::new(100, 0);
    
    print_evolution_summary(&week_summary, total_eur_spent, total_eth_accumulated, portfolio_value_eur, total_return, average_eth_price);
    
    // Analyze adaptation patterns
    analyze_adaptation_patterns(&week_summary);
    
    // Compare with fixed DCA
    compare_with_fixed_dca(&week_summary, base_dca_amount_eur, eur_usd_rate);
}

#[derive(Debug, Clone)]
struct MarketScenario {
    name: String,
    eth_price: Decimal,
    volatility_percent: Decimal,
    rsi: Decimal,
    price_vs_ma_percent: Decimal,
    momentum_percent: Decimal,
}

#[derive(Debug, Clone)]
struct WeekSummary {
    week: u32,
    scenario_name: String,
    eth_price: Decimal,
    multiplier: Decimal,
    eur_spent: Decimal,
    eth_purchased: Decimal,
    cumulative_eth: Decimal,
    market_conditions: MarketScenario,
}

fn create_market_scenarios() -> Vec<MarketScenario> {
    vec![
        // Week 1: Bull market start
        MarketScenario {
            name: "Bull Start".to_string(),
            eth_price: Decimal::new(3200, 0),
            volatility_percent: Decimal::new(25, 1), // 2.5%
            rsi: Decimal::new(65, 0),
            price_vs_ma_percent: Decimal::new(5, 0), // 5% above MA
            momentum_percent: Decimal::new(8, 0), // 8% positive
        },
        // Week 2: Continued growth
        MarketScenario {
            name: "Growth".to_string(),
            eth_price: Decimal::new(3450, 0),
            volatility_percent: Decimal::new(30, 1), // 3.0%
            rsi: Decimal::new(72, 0),
            price_vs_ma_percent: Decimal::new(8, 0),
            momentum_percent: Decimal::new(12, 0),
        },
        // Week 3: Peak and correction
        MarketScenario {
            name: "Peak".to_string(),
            eth_price: Decimal::new(3800, 0),
            volatility_percent: Decimal::new(45, 1), // 4.5% - high volatility
            rsi: Decimal::new(78, 0),
            price_vs_ma_percent: Decimal::new(15, 0),
            momentum_percent: Decimal::new(6, 0),
        },
        // Week 4: Sharp correction
        MarketScenario {
            name: "Correction".to_string(),
            eth_price: Decimal::new(3100, 0),
            volatility_percent: Decimal::new(65, 1), // 6.5% - very high
            rsi: Decimal::new(35, 0), // oversold
            price_vs_ma_percent: Decimal::new(-12, 0), // 12% below MA
            momentum_percent: Decimal::new(-18, 0), // -18% negative
        },
        // Week 5: Continued drop
        MarketScenario {
            name: "Bear".to_string(),
            eth_price: Decimal::new(2800, 0),
            volatility_percent: Decimal::new(55, 1), // 5.5%
            rsi: Decimal::new(28, 0), // oversold
            price_vs_ma_percent: Decimal::new(-20, 0),
            momentum_percent: Decimal::new(-25, 0),
        },
        // Week 6: Bottoming out
        MarketScenario {
            name: "Bottom".to_string(),
            eth_price: Decimal::new(2650, 0),
            volatility_percent: Decimal::new(48, 1), // 4.8%
            rsi: Decimal::new(22, 0), // deeply oversold
            price_vs_ma_percent: Decimal::new(-25, 0),
            momentum_percent: Decimal::new(-15, 0),
        },
        // Week 7: Consolidation
        MarketScenario {
            name: "Consolidate".to_string(),
            eth_price: Decimal::new(2750, 0),
            volatility_percent: Decimal::new(35, 1), // 3.5%
            rsi: Decimal::new(45, 0),
            price_vs_ma_percent: Decimal::new(-8, 0),
            momentum_percent: Decimal::new(3, 0),
        },
        // Week 8: Recovery begins
        MarketScenario {
            name: "Recovery".to_string(),
            eth_price: Decimal::new(3000, 0),
            volatility_percent: Decimal::new(40, 1), // 4.0%
            rsi: Decimal::new(55, 0),
            price_vs_ma_percent: Decimal::new(-2, 0),
            momentum_percent: Decimal::new(9, 0),
        },
        // Week 9: Strong recovery
        MarketScenario {
            name: "Recovery+".to_string(),
            eth_price: Decimal::new(3400, 0),
            volatility_percent: Decimal::new(42, 1), // 4.2%
            rsi: Decimal::new(62, 0),
            price_vs_ma_percent: Decimal::new(5, 0),
            momentum_percent: Decimal::new(13, 0),
        },
        // Week 10: New bull phase
        MarketScenario {
            name: "Bull Phase".to_string(),
            eth_price: Decimal::new(3900, 0),
            volatility_percent: Decimal::new(38, 1), // 3.8%
            rsi: Decimal::new(68, 0),
            price_vs_ma_percent: Decimal::new(12, 0),
            momentum_percent: Decimal::new(15, 0),
        },
        // Week 11: Euphoria - overbought conditions
        MarketScenario {
            name: "Euphoria".to_string(),
            eth_price: Decimal::new(4500, 0),
            volatility_percent: Decimal::new(12, 1), // 1.2% - low volatility 
            rsi: Decimal::new(75, 0), // overbought
            price_vs_ma_percent: Decimal::new(18, 0), // 18% above MA
            momentum_percent: Decimal::new(16, 0), // strong positive momentum
        },
        // Week 12: Current levels - overpriced scenario
        MarketScenario {
            name: "Overpriced".to_string(),
            eth_price: Decimal::new(4627, 0), // Current real price
            volatility_percent: Decimal::new(10, 1), // 1.0% - very low volatility
            rsi: Decimal::new(78, 0), // overbought
            price_vs_ma_percent: Decimal::new(15, 0), // 15% above MA
            momentum_percent: Decimal::new(20, 0), // very strong momentum
        },
    ]
}

fn calculate_volatility_multiplier(volatility_percent: Decimal, config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    if volatility_percent > config.volatility_threshold {
        config.high_volatility_multiplier
    } else if volatility_percent < config.low_volatility_threshold / Decimal::new(100, 0) {
        config.low_volatility_multiplier
    } else {
        Decimal::ONE
    }
}

fn calculate_rsi_multiplier(rsi: Decimal, config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    if rsi < config.rsi_oversold_threshold {
        config.rsi_oversold_multiplier
    } else if rsi > config.rsi_overbought_threshold {
        config.rsi_overbought_multiplier
    } else {
        Decimal::ONE
    }
}

fn calculate_deviation_multiplier(price_vs_ma_percent: Decimal, config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    if price_vs_ma_percent < -config.deviation_threshold_percent {
        config.below_ma_multiplier
    } else if price_vs_ma_percent > config.above_ma_threshold_percent {
        config.above_ma_multiplier
    } else {
        Decimal::ONE
    }
}

fn calculate_momentum_multiplier(momentum_percent: Decimal, config: &crate::market_indicators::MarketIndicatorsConfig) -> Decimal {
    if momentum_percent < config.negative_momentum_threshold {
        config.negative_momentum_multiplier
    } else if momentum_percent > config.positive_momentum_threshold {
        config.positive_momentum_multiplier
    } else {
        Decimal::ONE
    }
}

fn calculate_average_price(week_summary: &[WeekSummary]) -> Decimal {
    let total_cost: Decimal = week_summary.iter()
        .map(|w| w.eth_purchased * w.eth_price)
        .sum();
    let total_eth: Decimal = week_summary.iter()
        .map(|w| w.eth_purchased)
        .sum();
    
    if total_eth > Decimal::ZERO {
        total_cost / total_eth
    } else {
        Decimal::ZERO
    }
}

fn print_evolution_summary(
    week_summary: &[WeekSummary],
    total_eur_spent: Decimal,
    total_eth_accumulated: Decimal,
    portfolio_value_eur: Decimal,
    total_return: Decimal,
    average_eth_price: Decimal,
) {
    println!("📈 12-Week Dynamic DCA Summary:");
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║                    PORTFOLIO PERFORMANCE                  ║");
    println!("╠═══════════════════════════════════════════════════════════╣");
    println!("║ Total EUR Invested: €{:>38} ║", total_eur_spent.round_dp(2));
    println!("║ Total ETH Accumulated: {:>35} ║", total_eth_accumulated.round_dp(6));
    println!("║ Average ETH Price: ${:>38} ║", average_eth_price.round_dp(2));
    println!("║ Current Portfolio Value: €{:>33} ║", portfolio_value_eur.round_dp(2));
    println!("║ Total Return: {:>44} ║", format!("{}%", total_return.round_dp(1)));
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!();
}

fn analyze_adaptation_patterns(week_summary: &[WeekSummary]) {
    println!("🧠 Adaptation Pattern Analysis:");
    println!();
    
    // Count trigger events
    let mut high_volatility_weeks = 0;
    let mut oversold_weeks = 0;
    let mut below_ma_weeks = 0;
    let mut negative_momentum_weeks = 0;
    let mut max_multiplier_weeks = 0;
    
    let mut multiplier_histogram: HashMap<String, u32> = HashMap::new();
    
    for week in week_summary {
        let config = crate::market_indicators::MarketIndicatorsConfig::default();
        
        if week.market_conditions.volatility_percent > config.volatility_threshold {
            high_volatility_weeks += 1;
        }
        if week.market_conditions.rsi < config.rsi_oversold_threshold {
            oversold_weeks += 1;
        }
        if week.market_conditions.price_vs_ma_percent < -config.deviation_threshold_percent {
            below_ma_weeks += 1;
        }
        if week.market_conditions.momentum_percent < config.negative_momentum_threshold {
            negative_momentum_weeks += 1;
        }
        if week.multiplier >= config.max_total_multiplier {
            max_multiplier_weeks += 1;
        }
        
        // Categorize multiplier
        let mult_category = if week.multiplier >= Decimal::new(125, 2) {
            "High (1.25x+)".to_string()
        } else if week.multiplier >= Decimal::new(110, 2) {
            "Medium (1.10x+)".to_string()
        } else if week.multiplier >= Decimal::new(105, 2) {
            "Low (1.05x+)".to_string()
        } else {
            "Base (1.00x)".to_string()
        };
        
        *multiplier_histogram.entry(mult_category).or_insert(0) += 1;
    }
    
    println!("📊 Trigger Frequency (out of 12 weeks):");
    println!("   🎯 High Volatility: {} weeks ({:.1}%)", high_volatility_weeks, (high_volatility_weeks as f64 / 12.0) * 100.0);
    println!("   📉 Oversold (RSI): {} weeks ({:.1}%)", oversold_weeks, (oversold_weeks as f64 / 12.0) * 100.0);
    println!("   📊 Below MA: {} weeks ({:.1}%)", below_ma_weeks, (below_ma_weeks as f64 / 12.0) * 100.0);
    println!("   ⬇️  Negative Momentum: {} weeks ({:.1}%)", negative_momentum_weeks, (negative_momentum_weeks as f64 / 12.0) * 100.0);
    println!("   🚨 Max Multiplier Hit: {} weeks ({:.1}%)", max_multiplier_weeks, (max_multiplier_weeks as f64 / 12.0) * 100.0);
    println!();
    
    println!("📈 Multiplier Distribution:");
    for (category, count) in multiplier_histogram.iter() {
        println!("   {}: {} weeks ({:.1}%)", category, count, (*count as f64 / 12.0) * 100.0);
    }
    println!();
}

fn compare_with_fixed_dca(week_summary: &[WeekSummary], base_amount: Decimal, eur_usd_rate: Decimal) {
    println!("⚖️  Dynamic vs Fixed DCA Comparison:");
    println!();
    
    // Calculate fixed DCA performance
    let mut fixed_total_eur = Decimal::ZERO;
    let mut fixed_total_eth = Decimal::ZERO;
    
    for week in week_summary {
        fixed_total_eur += base_amount;
        let usdc_amount = base_amount * eur_usd_rate;
        let eth_bought = usdc_amount / week.eth_price;
        fixed_total_eth += eth_bought;
    }
    
    // Dynamic DCA totals
    let dynamic_total_eur: Decimal = week_summary.iter().map(|w| w.eur_spent).sum();
    let dynamic_total_eth: Decimal = week_summary.iter().map(|w| w.eth_purchased).sum();
    
    // Final portfolio values
    let final_price = week_summary.last().unwrap().eth_price;
    let fixed_portfolio_value = fixed_total_eth * final_price / eur_usd_rate;
    let dynamic_portfolio_value = dynamic_total_eth * final_price / eur_usd_rate;
    
    // Calculate returns
    let fixed_return = ((fixed_portfolio_value - fixed_total_eur) / fixed_total_eur) * Decimal::new(100, 0);
    let dynamic_return = ((dynamic_portfolio_value - dynamic_total_eur) / dynamic_total_eur) * Decimal::new(100, 0);
    
    let eur_difference = dynamic_total_eur - fixed_total_eur;
    let eth_difference = dynamic_total_eth - fixed_total_eth;
    let return_difference = dynamic_return - fixed_return;
    
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║                    STRATEGY COMPARISON                    ║");
    println!("╠═══════════════════════════════════════════════════════════╣");
    println!("║                     Fixed DCA │ Dynamic DCA │ Difference ║");
    println!("╠═══════════════════════════════════════════════════════════╣");
    println!("║ EUR Invested:      €{:>10} │ €{:>10} │ €{:>9} ║", 
             fixed_total_eur.round_dp(2), 
             dynamic_total_eur.round_dp(2), 
             eur_difference.round_dp(2));
    println!("║ ETH Accumulated:   {:>11} │ {:>11} │ {:>10} ║", 
             fixed_total_eth.round_dp(6), 
             dynamic_total_eth.round_dp(6), 
             eth_difference.round_dp(6));
    println!("║ Portfolio Value:   €{:>10} │ €{:>10} │ €{:>9} ║", 
             fixed_portfolio_value.round_dp(2), 
             dynamic_portfolio_value.round_dp(2), 
             (dynamic_portfolio_value - fixed_portfolio_value).round_dp(2));
    println!("║ Total Return:      {:>11} │ {:>11} │ {:>10} ║", 
             format!("{}%", fixed_return.round_dp(1)), 
             format!("{}%", dynamic_return.round_dp(1)), 
             format!("{}%", return_difference.round_dp(1)));
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!();
    
    // Strategy effectiveness analysis
    if eth_difference > Decimal::ZERO {
        let improvement_percent = (eth_difference / fixed_total_eth) * Decimal::new(100, 0);
        println!("✅ Dynamic DCA Advantage:");
        println!("   📈 Accumulated {}% more ETH than fixed DCA", improvement_percent.round_dp(1));
        println!("   💰 Extra value: €{}", (dynamic_portfolio_value - fixed_portfolio_value).round_dp(2));
        
        if eur_difference > Decimal::ZERO {
            println!("   📊 Achieved superior accumulation despite spending €{} more", eur_difference.round_dp(2));
        } else {
            println!("   🎯 Achieved superior accumulation while spending €{} less", eur_difference.abs().round_dp(2));
        }
    } else {
        println!("❌ Fixed DCA performed better in this scenario");
        println!("   📉 Dynamic strategy accumulated {}% less ETH", (eth_difference.abs() / fixed_total_eth * Decimal::new(100, 0)).round_dp(1));
    }
}
