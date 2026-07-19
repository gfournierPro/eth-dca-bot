//! Manual OKX smoke test (mirrors test_kraken).
//!
//!   cargo run --bin test_okx                          # read-only: USDC balance + ETH price + EUR rate
//!   cargo run --bin test_okx -- --buy 5               # ~5 USDC MARKET buy of ETH
//!   cargo run --bin test_okx -- --limit 5             # ~5 USDC patient-maker LIMIT buy
//!   cargo run --bin test_okx -- --verify-withdraw 0.001
//!                                                     # read-only: checks 0.001 ETH can go to
//!                                                     # WITHDRAWAL_WALLET_ADDRESS on WITHDRAWAL_NETWORK
//!   cargo run --bin test_okx -- --withdraw 0.001      # REAL withdrawal of 0.001 ETH
//!
//! Run the read-only mode first after creating the OKX API key — the client has
//! never been exercised against a live account.

use anyhow::Result;
use eth_dca_bot::exchange::{Exchange, LimitBuyConfig};
use eth_dca_bot::okx::OkxClient;
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

    let api_key = env::var("OKX_API_KEY").map_err(|_| anyhow::anyhow!("OKX_API_KEY not set"))?;
    let secret =
        env::var("OKX_SECRET_KEY").map_err(|_| anyhow::anyhow!("OKX_SECRET_KEY not set"))?;
    let passphrase =
        env::var("OKX_PASSPHRASE").map_err(|_| anyhow::anyhow!("OKX_PASSPHRASE not set"))?;
    let base_url = env::var("OKX_BASE_URL").unwrap_or_else(|_| "https://www.okx.com".to_string());

    let client = OkxClient::new(api_key, secret, passphrase, base_url);
    let symbol = "ETHUSDC";

    let usdc = client.get_usdc_balance().await?;
    let price = client.get_price(symbol).await?;
    let usdc_per_eur = client.get_usdc_per_eur().await?;
    println!("--------------------------------------------------");
    println!("OKX USDC balance  : {}", usdc);
    println!("ETH price ({}) : {} USDC", symbol, price);
    println!("USDC per EUR      : {}", usdc_per_eur);
    println!("--------------------------------------------------");

    let args: Vec<String> = env::args().collect();
    let parse_amount = |flag: &str| -> Result<Option<Decimal>> {
        match args.iter().position(|a| a == flag) {
            Some(pos) => {
                let s = args.get(pos + 1).ok_or_else(|| {
                    anyhow::anyhow!("{flag} requires an amount, e.g. {flag} 5")
                })?;
                Ok(Some(Decimal::from_str(s)?))
            }
            None => Ok(None),
        }
    };

    if let Some(amount) = parse_amount("--buy")? {
        if amount > usdc {
            return Err(anyhow::anyhow!(
                "Requested {} USDC exceeds balance {}",
                amount,
                usdc
            ));
        }
        println!(">>> Placing MARKET BUY for {} USDC of ETH...", amount);
        let outcome = client.place_market_buy(symbol, amount).await?;
        print_outcome("MARKET BUY RESULT", &outcome);
    } else if let Some(amount) = parse_amount("--limit")? {
        if amount > usdc {
            return Err(anyhow::anyhow!(
                "Requested {} USDC exceeds balance {}",
                amount,
                usdc
            ));
        }
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
    } else if let Some(amount) = parse_amount("--verify-withdraw")? {
        let addr = env::var("WITHDRAWAL_WALLET_ADDRESS")
            .map_err(|_| anyhow::anyhow!("WITHDRAWAL_WALLET_ADDRESS not set"))?;
        let network = env::var("WITHDRAWAL_NETWORK").unwrap_or_default();
        let eth_balance = client.get_asset_balance("ETH").await?;
        println!("ETH balance          : {}", eth_balance);
        println!(
            ">>> Verifying withdrawal of {} ETH to '{}' (network: {})...",
            amount, addr, network
        );
        let ok = client
            .verify_withdrawal("ETH", &addr, &network, amount)
            .await?;
        println!(
            "--------------------------------------------------\n\
             Verification result : {}\n\
             --------------------------------------------------",
            if ok { "OK — see limits/fee above in logs" } else { "FAILED — see warning above" }
        );
    } else if let Some(amount) = parse_amount("--withdraw")? {
        let addr = env::var("WITHDRAWAL_WALLET_ADDRESS")
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
            ">>> REAL WITHDRAWAL: {} ETH to '{}' (network: {}). This moves real funds \
             and cannot be undone.",
            amount, addr, network
        );
        let wd_id = client.withdraw("ETH", &addr, amount, &network).await?;
        println!(
            "--------------------------------------------------\n\
             Withdrawal submitted. wdId: {}\n\
             --------------------------------------------------",
            wd_id
        );
    } else {
        println!(
            "(read-only) pass `--buy <usdc>` / `--limit <usdc>` to buy, \
             `--verify-withdraw <eth>` to check the withdrawal address, or \
             `--withdraw <eth>` to actually withdraw."
        );
    }

    Ok(())
}
