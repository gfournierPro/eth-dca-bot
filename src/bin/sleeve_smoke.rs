//! Live smoke-test harness for the limit sleeve (see docs/limit-sleeve-smoke-test.md).
//!
//! Deliberately bypasses `main.rs`: it never touches the DCA scheduler, so running
//! this for hours/days while waiting for a natural fill can't collide with (or
//! duplicate) a live DCA cron running elsewhere on the same account. It talks to
//! Kraken and Mongo directly through the same `LimitSleeve` production code path.
//!
//!   cargo run --bin sleeve_smoke -- reconcile --chest 1.0 --collection limit_sleeve_smoke
//!   cargo run --bin sleeve_smoke -- validate --price 2999.5 --volume 0.0015
//!   cargo run --bin sleeve_smoke -- teardown

use anyhow::{Result, anyhow};
use eth_dca_bot::config::LimitSleeveConfig;
use eth_dca_bot::kraken::KrakenClient;
use eth_dca_bot::limit_sleeve::LimitSleeve;
use eth_dca_bot::notion_integration::NotionDCATracker;
use rust_decimal::Decimal;
use std::env;
use std::str::FromStr;

const SLEEVE_USERREF: i32 = 770_077;

fn kraken_client() -> Result<KrakenClient> {
    let api_key = env::var("KRAKEN_API_KEY").map_err(|_| anyhow!("KRAKEN_API_KEY not set"))?;
    let secret = env::var("KRAKEN_SECRET_KEY").map_err(|_| anyhow!("KRAKEN_SECRET_KEY not set"))?;
    let base_url = env::var("KRAKEN_BASE_URL").unwrap_or_else(|_| "https://api.kraken.com".to_string());
    Ok(KrakenClient::new(api_key, secret, base_url))
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn decimal_arg(args: &[String], flag: &str, default: &str) -> Result<Decimal> {
    let s = arg_value(args, flag).unwrap_or_else(|| default.to_string());
    Decimal::from_str(&s).map_err(|e| anyhow!("bad value for {flag}: {e}"))
}

async fn cmd_reconcile(args: &[String]) -> Result<()> {
    let chest = decimal_arg(args, "--chest", "1.0")?;
    let collection = arg_value(args, "--collection").unwrap_or_else(|| "limit_sleeve_smoke".to_string());
    let bucket_size = decimal_arg(args, "--bucket", "5.0")?;
    let hvn_ratio = decimal_arg(args, "--hvn-ratio", "0.7")?;
    let ladder_steps: usize = arg_value(args, "--ladder-steps")
        .unwrap_or_else(|| "4".to_string())
        .parse()?;
    let interval_minutes: u32 = arg_value(args, "--interval")
        .unwrap_or_else(|| "60".to_string())
        .parse()?;

    let mut cfg = LimitSleeveConfig::eth_default();
    cfg.war_chest_usdc = chest;
    cfg.mongo_collection = collection.clone();
    cfg.interval_minutes = interval_minutes;
    cfg.volume_profile.bucket_size = bucket_size;
    cfg.volume_profile.hvn_threshold_ratio = hvn_ratio;
    cfg.volume_profile.ladder_steps = ladder_steps;

    println!(
        "--------------------------------------------------\n\
         reconcile: symbol={} chest={} collection={} bucket={} hvn_ratio={} ladder_steps={} interval={}m\n\
         --------------------------------------------------",
        cfg.symbol, cfg.war_chest_usdc, cfg.mongo_collection, cfg.volume_profile.bucket_size,
        cfg.volume_profile.hvn_threshold_ratio, cfg.volume_profile.ladder_steps, cfg.interval_minutes
    );

    let kraken = kraken_client()?;
    let notion = if !env::var("NOTION_TOKEN").unwrap_or_default().is_empty()
        && !env::var("NOTION_DATABASE_ID").unwrap_or_default().is_empty()
    {
        let notion_cfg = eth_dca_bot::config::NotionConfig {
            token: env::var("NOTION_TOKEN").unwrap_or_default(),
            database_id: env::var("NOTION_DATABASE_ID").unwrap_or_default(),
            cold_wallet_address: String::new(),
        };
        match NotionDCATracker::new(&notion_cfg, &cfg.asset, "Kraken") {
            Ok(t) => Some(t),
            Err(e) => {
                println!("(notion mirror disabled: {e})");
                None
            }
        }
    } else {
        println!("(notion mirror disabled: NOTION_TOKEN/NOTION_DATABASE_ID not set)");
        None
    };

    let sleeve = LimitSleeve::new(kraken, cfg).await?.with_notion(notion);
    sleeve.reconcile().await?;
    println!("reconcile complete.");
    Ok(())
}

async fn cmd_ladder(args: &[String]) -> Result<()> {
    let bucket_size = decimal_arg(args, "--bucket", "5.0")?;
    let hvn_ratio = decimal_arg(args, "--hvn-ratio", "0.7")?;
    let ladder_steps: usize = arg_value(args, "--ladder-steps")
        .unwrap_or_else(|| "4".to_string())
        .parse()?;
    let interval_minutes: u32 = arg_value(args, "--interval")
        .unwrap_or_else(|| "60".to_string())
        .parse()?;
    let vp = eth_dca_bot::levels::VolumeProfileConfig {
        bucket_size,
        hvn_threshold_ratio: hvn_ratio,
        ladder_steps,
        require_local_maxima: true,
    };
    let kraken = kraken_client()?;
    let (ladder, spot) = kraken.build_bid_ladder("ETHUSDC", interval_minutes, &vp).await?;
    let spec = kraken.fetch_pair_spec("ETHUSDC").await?;
    println!("spot: {spot}  tick_size: {}  ordermin: {}", spec.tick_size, spec.ordermin);
    for (i, l) in ladder.levels.iter().enumerate() {
        println!(
            "  level {i}: price={} weight={} source_volume={}",
            l.price, l.weight, l.source_volume
        );
    }
    if ladder.levels.is_empty() {
        println!("  (empty ladder — no qualifying HVN below spot)");
    }
    Ok(())
}

async fn cmd_validate(args: &[String]) -> Result<()> {
    let price = decimal_arg(args, "--price", "0")?;
    let volume = decimal_arg(args, "--volume", "0.0015")?;
    if price <= Decimal::ZERO {
        return Err(anyhow!("--price <tick-rounded HVN price> is required"));
    }
    let kraken = kraken_client()?;
    println!(">>> validate=true AddOrder: buy {volume} ETHUSDC @ {price} (post-only)");
    match kraken.validate_resting_limit_buy("ETHUSDC", price, volume).await {
        Ok(v) => println!("PASS — Kraken accepted (no error):\n{}", serde_json::to_string_pretty(&v)?),
        Err(e) => println!("FAIL — Kraken rejected:\n{e}"),
    }
    Ok(())
}

async fn cmd_list() -> Result<()> {
    let kraken = kraken_client()?;
    let open = kraken.get_open_sleeve_orders(SLEEVE_USERREF).await?;
    if open.is_empty() {
        println!("no resting sleeve orders (userref {SLEEVE_USERREF}).");
        return Ok(());
    }
    for o in &open {
        println!(
            "{} price={} volume={} executed_qty={}",
            o.txid, o.price, o.volume, o.executed_qty
        );
    }
    Ok(())
}

async fn cmd_teardown() -> Result<()> {
    let kraken = kraken_client()?;
    let open = kraken.get_open_sleeve_orders(SLEEVE_USERREF).await?;
    if open.is_empty() {
        println!("no resting sleeve orders (userref {SLEEVE_USERREF}) — nothing to cancel.");
        return Ok(());
    }
    for o in &open {
        println!("cancelling {} (price {}, vol {}, executed {})", o.txid, o.price, o.volume, o.executed_qty);
        kraken.cancel_resting_order(&o.txid).await;
    }
    let remaining = kraken.get_open_sleeve_orders(SLEEVE_USERREF).await?;
    if remaining.is_empty() {
        println!("confirmed: no resting sleeve orders remain.");
    } else {
        println!("WARNING: {} order(s) still open after cancel — re-check manually.", remaining.len());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    dotenv::dotenv().ok();

    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("reconcile") => cmd_reconcile(&args).await,
        Some("ladder") => cmd_ladder(&args).await,
        Some("validate") => cmd_validate(&args).await,
        Some("list") => cmd_list().await,
        Some("teardown") => cmd_teardown().await,
        _ => {
            println!(
                "usage:\n  sleeve_smoke reconcile --chest <usdc> --collection <name> [--bucket 5.0] [--hvn-ratio 0.7] [--ladder-steps 4] [--interval 60]\n  sleeve_smoke ladder [--bucket 5.0] [--hvn-ratio 0.7] [--ladder-steps 4] [--interval 60]\n  sleeve_smoke validate --price <p> --volume <v>\n  sleeve_smoke teardown"
            );
            Ok(())
        }
    }
}
