//! Manual Kraken smoke test.
//!
//!   cargo run --bin test_kraken            # read-only: USDC balance + ETH price
//!   cargo run --bin test_kraken -- --buy 5 # place a ~5 USDC market buy of ETH
//!
//! Temporary helper for validating the live Kraken integration.

use anyhow::Result;
use eth_dca_bot::exchange::Exchange;
use eth_dca_bot::kraken::KrakenClient;
use rust_decimal::Decimal;
use std::env;
use std::str::FromStr;

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
    if let Some(pos) = args.iter().position(|a| a == "--buy") {
        let amount_str = args
            .get(pos + 1)
            .ok_or_else(|| anyhow::anyhow!("--buy requires a USDC amount, e.g. --buy 5"))?;
        let amount = Decimal::from_str(amount_str)?;

        if amount > usdc {
            return Err(anyhow::anyhow!(
                "Requested {} USDC exceeds balance {}",
                amount,
                usdc
            ));
        }

        println!(">>> Placing MARKET BUY for {} USDC of ETH...", amount);
        let outcome = client.place_market_buy(symbol, amount).await?;
        println!("--------------------------------------------------");
        println!("Order ID     : {}", outcome.order_id);
        println!("Status       : {}", outcome.status);
        println!("ETH acquired : {}", outcome.executed_qty);
        println!("USDC spent   : {}", outcome.executed_value);
        println!("Avg price    : {}", outcome.avg_price);
        println!("Fees (USDC)  : {}", outcome.fees_usdc);
        println!("--------------------------------------------------");
    } else {
        println!("(read-only) pass `--buy <usdc_amount>` to place a real market buy.");
    }

    Ok(())
}
