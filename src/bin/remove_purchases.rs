use anyhow::Result;
use eth_dca_bot::dca_stats_mongo::DcaStatsDB;
use std::env;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    dotenv::dotenv().ok();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <collection> list [limit]", args[0]);
        eprintln!("       {} <collection> remove <order_id> [<order_id> ...]", args[0]);
        eprintln!("Example: {} btc_purchases list 10", args[0]);
        eprintln!("Example: {} btc_purchases remove 3792594492691488768 3792590252015263744", args[0]);
        std::process::exit(1);
    }

    let collection = &args[1];
    let db = DcaStatsDB::with_collection(collection).await?;

    match args.get(2).map(String::as_str) {
        Some("list") => {
            let limit: i64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
            for p in db.get_recent_purchases(limit).await? {
                println!(
                    "{} | {:>4} | {:>8} USDC | {:>10} {} @ {:>9} | fees {:>7} | {} | order_id={}",
                    p.timestamp,
                    p.side,
                    p.usdc_amount.round_dp(2),
                    p.eth_amount.round_dp(6),
                    p.symbol,
                    p.eth_price.round_dp(2),
                    p.fees_usdc.round_dp(4),
                    p.status,
                    p.order_id
                );
            }
        }
        Some("remove") => {
            for order_id in &args[3..] {
                // Ignore first, then delete: the backfill re-imports anything that is
                // merely deleted, since a manual app trade looks like a DCA buy.
                db.ignore_order_id(order_id).await?;
                match db.get_purchase_by_order_id(order_id).await? {
                    Some(p) => {
                        info!("Removing {} | {} | {} USDC | {}", p.timestamp, p.side, p.usdc_amount, p.order_id);
                        db.remove_purchase_by_order_id(order_id).await?;
                    }
                    None => info!("⚠️  order_id {} not in collection, ignore-listed anyway", order_id),
                }
            }
        }
        _ => {
            eprintln!("Second argument must be 'list' or 'remove'");
            std::process::exit(1);
        }
    }

    Ok(())
}
