//! Live smoke-test harness for the limit sleeve (see docs/limit-sleeve-smoke-test.md).
//!
//! Deliberately bypasses `main.rs`: it never touches the DCA scheduler, so running
//! this for hours/days while waiting for a natural fill can't collide with (or
//! duplicate) a live DCA cron running elsewhere on the same account. It talks to
//! Kraken and Mongo directly through the same `LimitSleeve` production code path.
//!
//! Every command takes `--asset eth|btc` (default eth) to pick which sleeve's
//! defaults — symbol, userref, bucket size, fills collection — it operates on.
//!
//!   cargo run --bin sleeve_smoke -- reconcile --chest 1.0 --collection limit_sleeve_smoke
//!   cargo run --bin sleeve_smoke -- validate --price 2999.5 --volume 0.0015
//!   cargo run --bin sleeve_smoke -- teardown
//!   cargo run --bin sleeve_smoke -- reconcile --asset btc --chest 1.0

use anyhow::{Result, anyhow};
use eth_dca_bot::config::LimitSleeveConfig;
use eth_dca_bot::kraken::KrakenClient;
use eth_dca_bot::limit_sleeve::LimitSleeve;
use eth_dca_bot::notion_integration::NotionDCATracker;
use rust_decimal::Decimal;
use std::env;
use std::str::FromStr;

fn kraken_client() -> Result<KrakenClient> {
    let api_key = env::var("KRAKEN_API_KEY").map_err(|_| anyhow!("KRAKEN_API_KEY not set"))?;
    let secret = env::var("KRAKEN_SECRET_KEY").map_err(|_| anyhow!("KRAKEN_SECRET_KEY not set"))?;
    let base_url =
        env::var("KRAKEN_BASE_URL").unwrap_or_else(|_| "https://api.kraken.com".to_string());
    Ok(KrakenClient::new(api_key, secret, base_url))
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn decimal_arg(args: &[String], flag: &str, default: Decimal) -> Result<Decimal> {
    match arg_value(args, flag) {
        Some(s) => Decimal::from_str(&s).map_err(|e| anyhow!("bad value for {flag}: {e}")),
        None => Ok(default),
    }
}

/// The sleeve defaults selected by `--asset eth|btc` (default eth). Carries the
/// per-asset symbol, userref, bucket size, and fills collection, so the smoke test
/// exercises exactly what the production sleeve for that asset would.
fn sleeve_defaults(args: &[String]) -> Result<LimitSleeveConfig> {
    match arg_value(args, "--asset").as_deref().unwrap_or("eth") {
        "eth" | "ETH" => Ok(LimitSleeveConfig::eth_default()),
        "btc" | "BTC" => Ok(LimitSleeveConfig::btc_default()),
        other => Err(anyhow!("unknown --asset '{other}' (expected eth or btc)")),
    }
}

/// Default smoke collection per asset — kept separate from the production fills
/// collections so a smoke run can never pollute real war-chest accounting.
fn smoke_collection(cfg: &LimitSleeveConfig) -> String {
    match cfg.asset.as_str() {
        "BTC" => "btc_limit_sleeve_smoke".to_string(),
        _ => "limit_sleeve_smoke".to_string(),
    }
}

async fn cmd_reconcile(args: &[String]) -> Result<()> {
    let mut cfg = sleeve_defaults(args)?;
    cfg.war_chest_usdc = decimal_arg(args, "--chest", Decimal::ONE)?;
    cfg.mongo_collection =
        arg_value(args, "--collection").unwrap_or_else(|| smoke_collection(&cfg));
    cfg.volume_profile.bucket_size = decimal_arg(args, "--bucket", cfg.volume_profile.bucket_size)?;
    cfg.volume_profile.hvn_threshold_ratio =
        decimal_arg(args, "--hvn-ratio", cfg.volume_profile.hvn_threshold_ratio)?;
    if let Some(v) = arg_value(args, "--ladder-steps") {
        cfg.volume_profile.ladder_steps = v.parse()?;
    }
    if let Some(v) = arg_value(args, "--interval") {
        cfg.interval_minutes = v.parse()?;
    }

    println!(
        "--------------------------------------------------\n\
         reconcile: asset={} symbol={} userref={} chest={} collection={} bucket={} hvn_ratio={} ladder_steps={} interval={}m\n\
         --------------------------------------------------",
        cfg.asset,
        cfg.symbol,
        cfg.userref,
        cfg.war_chest_usdc,
        cfg.mongo_collection,
        cfg.volume_profile.bucket_size,
        cfg.volume_profile.hvn_threshold_ratio,
        cfg.volume_profile.ladder_steps,
        cfg.interval_minutes
    );

    let kraken = kraken_client()?;
    let notion = if !env::var("NOTION_TOKEN").unwrap_or_default().is_empty()
        && !env::var("NOTION_DATABASE_ID")
            .unwrap_or_default()
            .is_empty()
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
    let cfg = sleeve_defaults(args)?;
    let mut vp = cfg.volume_profile.clone();
    vp.bucket_size = decimal_arg(args, "--bucket", vp.bucket_size)?;
    vp.hvn_threshold_ratio = decimal_arg(args, "--hvn-ratio", vp.hvn_threshold_ratio)?;
    if let Some(v) = arg_value(args, "--ladder-steps") {
        vp.ladder_steps = v.parse()?;
    }
    let interval_minutes: u32 = match arg_value(args, "--interval") {
        Some(v) => v.parse()?,
        None => cfg.interval_minutes,
    };
    let kraken = kraken_client()?;
    let (ladder, spot) = kraken
        .build_bid_ladder(&cfg.symbol, interval_minutes, &vp)
        .await?;
    let spec = kraken.fetch_pair_spec(&cfg.symbol).await?;
    println!(
        "{}: spot: {spot}  tick_size: {}  ordermin: {}",
        cfg.symbol, spec.tick_size, spec.ordermin
    );
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
    let cfg = sleeve_defaults(args)?;
    let price = decimal_arg(args, "--price", Decimal::ZERO)?;
    let volume = decimal_arg(args, "--volume", Decimal::from_str("0.0015").unwrap())?;
    if price <= Decimal::ZERO {
        return Err(anyhow!("--price <tick-rounded HVN price> is required"));
    }
    let kraken = kraken_client()?;
    println!(
        ">>> validate=true AddOrder: buy {volume} {} @ {price} (post-only)",
        cfg.symbol
    );
    match kraken
        .validate_resting_limit_buy(&cfg.symbol, price, volume)
        .await
    {
        Ok(v) => println!(
            "PASS — Kraken accepted (no error):\n{}",
            serde_json::to_string_pretty(&v)?
        ),
        Err(e) => println!("FAIL — Kraken rejected:\n{e}"),
    }
    Ok(())
}

async fn cmd_list(args: &[String]) -> Result<()> {
    let cfg = sleeve_defaults(args)?;
    let kraken = kraken_client()?;
    let open = kraken.get_open_sleeve_orders(cfg.userref).await?;
    if open.is_empty() {
        println!("no resting sleeve orders (userref {}).", cfg.userref);
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

async fn cmd_teardown(args: &[String]) -> Result<()> {
    let cfg = sleeve_defaults(args)?;
    let kraken = kraken_client()?;
    let open = kraken.get_open_sleeve_orders(cfg.userref).await?;
    if open.is_empty() {
        println!(
            "no resting sleeve orders (userref {}) — nothing to cancel.",
            cfg.userref
        );
        return Ok(());
    }
    for o in &open {
        println!(
            "cancelling {} (price {}, vol {}, executed {})",
            o.txid, o.price, o.volume, o.executed_qty
        );
        kraken.cancel_resting_order(&o.txid).await;
    }
    let remaining = kraken.get_open_sleeve_orders(cfg.userref).await?;
    if remaining.is_empty() {
        println!("confirmed: no resting sleeve orders remain.");
    } else {
        println!(
            "WARNING: {} order(s) still open after cancel — re-check manually.",
            remaining.len()
        );
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
        Some("list") => cmd_list(&args).await,
        Some("teardown") => cmd_teardown(&args).await,
        _ => {
            println!(
                "usage (all commands take --asset eth|btc, default eth):\n  sleeve_smoke reconcile [--asset eth|btc] --chest <usdc> --collection <name> [--bucket N] [--hvn-ratio 0.7] [--ladder-steps 4] [--interval 60]\n  sleeve_smoke ladder [--asset eth|btc] [--bucket N] [--hvn-ratio 0.7] [--ladder-steps 4] [--interval 60]\n  sleeve_smoke validate [--asset eth|btc] --price <p> --volume <v>\n  sleeve_smoke list [--asset eth|btc]\n  sleeve_smoke teardown [--asset eth|btc]"
            );
            Ok(())
        }
    }
}
