//! Manual Kraken smoke test.
//!
//!   cargo run --bin test_kraken              # read-only: USDC balance + ETH price
//!   cargo run --bin test_kraken -- --buy 5   # ~5 USDC MARKET buy of ETH (taker)
//!   cargo run --bin test_kraken -- --limit 5 # ~5 USDC patient-maker LIMIT buy
//!
//! Temporary helper for validating the live Kraken integration.

use anyhow::Result;
use eth_dca_bot::exchange::{Exchange, LimitBuyConfig};
use eth_dca_bot::kraken::KrakenClient;
use rust_decimal::Decimal;
use std::env;
use std::str::FromStr;

fn print_outcome(label: &str, o: &eth_dca_bot::exchange::OrderOutcome) {
    println!("--------------------------------------------------");
    println!("{label}");
    println!("Order ID     : {}", o.order_id);
    println!("Status       : {}", o.status);
    println!("ETH acquired : {}", o.executed_qty);
    println!("USDC spent   : {}", o.executed_value);
    println!("Avg price    : {}", o.avg_price);
    println!("Fees (USDC)  : {}", o.fees_usdc);
    if o.executed_value > Decimal::ZERO {
        let fee_pct = o.fees_usdc / o.executed_value * Decimal::from(100);
        println!("Fee rate     : {:.4}%", fee_pct);
    }
    println!("--------------------------------------------------");
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    dotenv::dotenv().ok();

    let api_key = env::var("KRAKEN_API_KEY").map_err(|_| anyhow::anyhow!("KRAKEN_API_KEY not set"))?;
    let secret = env::var("KRAKEN_SECRET_KEY").map_err(|_| anyhow::anyhow!("KRAKEN_SECRET_KEY not set"))?;
    let base_url = env::var("KRAKEN_BASE_URL").unwrap_or_else(|_| "https://api.kraken.com".to_string());

    let client = KrakenClient::new(api_key, secret, base_url);
    let symbol = "ETHUSDC";

    let usdc = client.get_usdc_balance().await?;
    let price = client.get_price(symbol).await?;
    println!("--------------------------------------------------");
    println!("Kraken USDC balance : {}", usdc);
    println!("ETH price ({})   : {} USDC", symbol, price);
    println!("--------------------------------------------------");

    let args: Vec<String> = env::args().collect();
    let parse_amount = |flag: &str| -> Result<Option<Decimal>> {
        match args.iter().position(|a| a == flag) {
            Some(pos) => {
                let s = args.get(pos + 1).ok_or_else(|| {
                    anyhow::anyhow!("{flag} requires a USDC amount, e.g. {flag} 5")
                })?;
                let amount = Decimal::from_str(s)?;
                if amount > usdc {
                    return Err(anyhow::anyhow!(
                        "Requested {} USDC exceeds balance {}",
                        amount,
                        usdc
                    ));
                }
                Ok(Some(amount))
            }
            None => Ok(None),
        }
    };

    if let Some(amount) = parse_amount("--buy")? {
        println!(">>> Placing MARKET BUY for {} USDC of ETH...", amount);
        let outcome = client.place_market_buy(symbol, amount).await?;
        print_outcome("MARKET BUY RESULT", &outcome);
    } else if let Some(amount) = parse_amount("--limit")? {
        let cfg = LimitBuyConfig::default();
        println!(
            ">>> Placing PATIENT-MAKER LIMIT BUY for {} USDC of ETH \
             (drift cap {}%, timeout {}s)...",
            amount,
            cfg.max_drift * Decimal::from(100),
            cfg.hard_timeout.as_secs()
        );
        let outcome = client.place_limit_buy(symbol, amount, &cfg).await?;
        print_outcome("LIMIT BUY RESULT", &outcome);
    } else {
        println!(
            "(read-only) pass `--buy <usdc>` for a market buy or `--limit <usdc>` for a maker limit buy."
        );
    }

    Ok(())
}
