//! Manual Kraken smoke test.
//!
//!   cargo run --bin test_kraken                       # read-only: USDC balance + ETH price
//!   cargo run --bin test_kraken -- --buy 5             # ~5 USDC MARKET buy of ETH (taker)
//!   cargo run --bin test_kraken -- --limit 5           # ~5 USDC patient-maker LIMIT buy
//!   cargo run --bin test_kraken -- --verify-withdraw 0.001
//!                                                       # read-only: checks the withdrawal
//!                                                       # key from .env can accept 0.001 ETH
//!   cargo run --bin test_kraken -- --withdraw 0.001    # REAL withdrawal of 0.001 ETH to the
//!                                                       # key named by WITHDRAWAL_WALLET_ADDRESS
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

    let api_key =
        env::var("KRAKEN_API_KEY").map_err(|_| anyhow::anyhow!("KRAKEN_API_KEY not set"))?;
    let secret =
        env::var("KRAKEN_SECRET_KEY").map_err(|_| anyhow::anyhow!("KRAKEN_SECRET_KEY not set"))?;
    let base_url =
        env::var("KRAKEN_BASE_URL").unwrap_or_else(|_| "https://api.kraken.com".to_string());

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

    let parse_eth_amount = |flag: &str| -> Result<Option<Decimal>> {
        match args.iter().position(|a| a == flag) {
            Some(pos) => {
                let s = args.get(pos + 1).ok_or_else(|| {
                    anyhow::anyhow!("{flag} requires an ETH amount, e.g. {flag} 0.001")
                })?;
                Ok(Some(Decimal::from_str(s)?))
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
    } else if let Some(amount) = parse_eth_amount("--verify-withdraw")? {
        let key = env::var("WITHDRAWAL_WALLET_ADDRESS")
            .map_err(|_| anyhow::anyhow!("WITHDRAWAL_WALLET_ADDRESS not set"))?;
        let network = env::var("WITHDRAWAL_NETWORK").unwrap_or_default();
        let eth_balance = client.get_asset_balance("ETH").await?;
        println!("ETH balance          : {}", eth_balance);
        println!(
            ">>> Verifying withdrawal of {} ETH via key '{}' (network hint: {})...",
            amount, key, network
        );
        let ok = client
            .verify_withdrawal("ETH", &key, &network, amount)
            .await?;
        println!(
            "--------------------------------------------------\n\
             Verification result : {}\n\
             --------------------------------------------------",
            if ok { "OK — key accepted, see limits/fee above in logs" } else { "FAILED — see warning above" }
        );
    } else if let Some(amount) = parse_eth_amount("--withdraw")? {
        let key = env::var("WITHDRAWAL_WALLET_ADDRESS")
            .map_err(|_| anyhow::anyhow!("WITHDRAWAL_WALLET_ADDRESS not set"))?;
        let network = env::var("WITHDRAWAL_NETWORK").unwrap_or_default();
        let eth_balance = client.get_asset_balance("ETH").await?;
        if amount > eth_balance {
            return Err(anyhow::anyhow!(
                "Requested {} ETH exceeds balance {}",
                amount,
                eth_balance
            ));
        }
        println!(
            ">>> REAL WITHDRAWAL: {} ETH via key '{}' (network hint: {}). This moves real funds \
             and cannot be undone.",
            amount, key, network
        );
        let refid = client.withdraw("ETH", &key, amount, &network).await?;
        println!(
            "--------------------------------------------------\n\
             Withdrawal submitted. refid: {}\n\
             --------------------------------------------------",
            refid
        );
    } else {
        println!(
            "(read-only) pass `--buy <usdc>` / `--limit <usdc>` to buy, \
             `--verify-withdraw <eth>` to check the withdrawal key, or \
             `--withdraw <eth>` to actually withdraw."
        );
    }

    Ok(())
}
