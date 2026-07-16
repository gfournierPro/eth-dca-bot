//! Diagnostic: dump every open order on the account with its userref, to spot
//! orders the sleeve's userref-scoped reconcile can't see (userref 0/absent, or a
//! stale value from before tagging existed).
//!
//!   cargo run --bin list_open_orders

use anyhow::Result;
use eth_dca_bot::kraken::KrakenClient;
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    let api_key = env::var("KRAKEN_API_KEY")?;
    let secret = env::var("KRAKEN_SECRET_KEY")?;
    let base_url =
        env::var("KRAKEN_BASE_URL").unwrap_or_else(|_| "https://api.kraken.com".to_string());
    let client = KrakenClient::new(api_key, secret, base_url);

    let orders = client.debug_list_all_open_orders().await?;
    println!(
        "{:<20} {:<10} {:>14} {:>14} {:>14} {:>12}",
        "txid", "pair", "price", "vol", "vol_exec", "userref"
    );
    for (txid, pair, price, vol, vol_exec, userref) in orders {
        println!(
            "{:<20} {:<10} {:>14} {:>14} {:>14} {:>12}",
            txid, pair, price, vol, vol_exec, userref
        );
    }
    Ok(())
}
