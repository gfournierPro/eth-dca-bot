use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcaPurchase {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub symbol: String,
    pub usdc_amount: Decimal,
    pub eth_amount: Decimal,
    pub eth_price: Decimal,
    pub fees_usdc: Decimal,
    pub order_id: u64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcaSummary {
    pub total_purchases: i64,
    pub total_usdc_invested: Decimal,
    pub total_eth_acquired: Decimal,
    pub total_fees_paid: Decimal,
    pub average_eth_price: Decimal,
    pub first_purchase: Option<DateTime<Utc>>,
    pub last_purchase: Option<DateTime<Utc>>,
    pub current_eth_value: Decimal,
    pub unrealized_pnl: Decimal,
    pub unrealized_pnl_percentage: Decimal,
}
#[derive(Debug, Clone)]
pub struct DcaStatsDB {
    pool: SqlitePool,
}

impl DcaStatsDB {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePool::connect(database_url).await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS dca_purchases (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                symbol TEXT NOT NULL,
                usdc_amount TEXT NOT NULL,
                eth_amount TEXT NOT NULL,
                eth_price TEXT NOT NULL,
                fees_usdc TEXT NOT NULL,
                order_id INTEGER NOT NULL,
                status TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await?;
        info!("DCA Statistics database initialized");
        Ok(Self { pool })
    }

    pub async fn record_purchase(&self, purchase: &DcaPurchase) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dca_purchases 
            (id, timestamp, symbol, usdc_amount, eth_amount, eth_price, fees_usdc, order_id, status)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
        )
        .bind(&purchase.id)
        .bind(purchase.timestamp.to_rfc3339())
        .bind(&purchase.symbol)
        .bind(purchase.usdc_amount.to_string())
        .bind(purchase.eth_amount.to_string())
        .bind(purchase.eth_price.to_string())
        .bind(purchase.fees_usdc.to_string())
        .bind(purchase.order_id as i64)
        .bind(&purchase.status)
        .execute(&self.pool)
        .await?;
        info!("Purchase recorded in database: {}", purchase.id);

        Ok(())
    }

    pub async fn get_summary(&self, current_eth_price: Decimal) -> Result<DcaSummary> {
        let row = sqlx::query(
            r#"
            SELECT 
                COUNT(*) as total_purchases,
                SUM(CAST(usdc_amount as REAL)) as total_usdc_invested,
                SUM(CAST(eth_amount as REAL)) as total_eth_acquired,
                SUM(CAST(fees_usdc as REAL)) as total_fees_paid,
                MIN(timestamp) as first_purchase,
                MAX(timestamp) as last_purchase
            FROM dca_purchases 
            WHERE status = 'FILLED'
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        let total_purchases: i64 = row.get("total_purchases");
        let total_usdc_invested: Option<f64> = row.get("total_usdc_invested");
        let total_eth_acquired: Option<f64> = row.get("total_eth_acquired");
        let total_fees_paid: Option<f64> = row.get("total_fees_paid");
        let first_purchase: Option<String> = row.get("first_purchase");
        let last_purchase: Option<String> = row.get("last_purchase");

        let total_usdc_invested =
            Decimal::from_f64_retain(total_usdc_invested.unwrap_or(0.0)).unwrap_or(dec!(0));
        let total_eth_acquired =
            Decimal::from_f64_retain(total_eth_acquired.unwrap_or(0.0)).unwrap_or(dec!(0));
        let total_fees_paid =
            Decimal::from_f64_retain(total_fees_paid.unwrap_or(0.0)).unwrap_or(dec!(0));

        let average_eth_price = if total_eth_acquired > dec!(0) {
            total_usdc_invested / total_eth_acquired
        } else {
            dec!(0)
        };

        let current_eth_value = total_eth_acquired * current_eth_price;
        let unrealized_pnl = current_eth_value - total_usdc_invested;
        let unrealized_pnl_percentage = if total_usdc_invested > dec!(0) {
            (unrealized_pnl / total_usdc_invested) * dec!(100)
        } else {
            dec!(0)
        };

        Ok(DcaSummary {
            total_purchases,
            total_usdc_invested,
            total_eth_acquired,
            total_fees_paid,
            average_eth_price,
            first_purchase: first_purchase.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            }),
            last_purchase: last_purchase.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            }),
            current_eth_value,
            unrealized_pnl,
            unrealized_pnl_percentage,
        })
    }

    pub async fn get_recent_purchases(&self, limit: i64) -> Result<Vec<DcaPurchase>> {
        let rows = sqlx::query(
            r#"
            SELECT * FROM dca_purchases 
            ORDER BY timestamp DESC 
            LIMIT ?1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut purchases = Vec::new();
        for row in rows {
            let purchase = DcaPurchase {
                id: row.get("id"),
                timestamp: DateTime::parse_from_rfc3339(&row.get::<String, _>("timestamp"))?
                    .with_timezone(&Utc),
                symbol: row.get("symbol"),
                usdc_amount: row.get::<String, _>("usdc_amount").parse()?,
                eth_amount: row.get::<String, _>("eth_amount").parse()?,
                eth_price: row.get::<String, _>("eth_price").parse()?,
                fees_usdc: row.get::<String, _>("fees_usdc").parse()?,
                order_id: row.get::<i64, _>("order_id") as u64,
                status: row.get("status"),
            };
            purchases.push(purchase);
        }

        Ok(purchases)
    }
}

pub fn print_dca_summary(summary: &DcaSummary) {
    info!("╔═══════════════════════════════════════╗");
    info!("║            📊 DCA SUMMARY             ║");
    info!("╠═══════════════════════════════════════╣");
    info!("║ Total Purchases: {:>19} ║", summary.total_purchases);
    info!(
        "║ Total USDC Invested: ${:>13} ║",
        summary.total_usdc_invested.round_dp(2)
    );
    info!(
        "║ Total ETH Acquired: {:>14} ║",
        summary.total_eth_acquired.round_dp(6)
    );
    info!(
        "║ Total Fees Paid: ${:>16} ║",
        summary.total_fees_paid.round_dp(2)
    );
    info!(
        "║ Average ETH Price: ${:>15} ║",
        summary.average_eth_price.round_dp(2)
    );
    info!("╠═══════════════════════════════════════╣");
    info!(
        "║ Current ETH Value: ${:>15} ║",
        summary.current_eth_value.round_dp(2)
    );

    let pnl_symbol = if summary.unrealized_pnl >= dec!(0) {
        "📈"
    } else {
        "📉"
    };
    let pnl_sign = if summary.unrealized_pnl >= dec!(0) {
        "+"
    } else {
        ""
    };

    info!(
        "║ Unrealized P&L: {}{:>17} ║",
        pnl_sign,
        format!("${}", summary.unrealized_pnl.round_dp(2))
    );
    info!(
        "║ P&L Percentage: {}{:>17} ║",
        pnl_sign,
        format!("{}%", summary.unrealized_pnl_percentage.round_dp(2))
    );
    info!("║ Performance: {:>23} ║", pnl_symbol);
    info!("╠═══════════════════════════════════════╣");

    if let Some(first) = summary.first_purchase {
        info!("║ First Purchase: {:>18} ║", first.format("%Y-%m-%d"));
    }
    if let Some(last) = summary.last_purchase {
        info!("║ Last Purchase: {:>19} ║", last.format("%Y-%m-%d"));
    }

    info!("╚═══════════════════════════════════════╝");
}

pub fn print_recent_purchases(purchases: &[DcaPurchase]) {
    if purchases.is_empty() {
        info!("📝 No recent purchases found");
        return;
    }

    info!("╔════════════════════════════════════════════════════════════════╗");
    info!("║                    📝 RECENT PURCHASES                         ║");
    info!("╠════════════════════════════════════════════════════════════════╣");

    for (i, purchase) in purchases.iter().enumerate() {
        if i > 0 {
            info!("║                                                                ║");
        }
        info!(
            "║ Date: {:>55} ║",
            purchase.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
        );
        info!("║ USDC Spent: ${:>47} ║", purchase.usdc_amount.round_dp(2));
        info!("║ ETH Acquired: {:>45} ║", purchase.eth_amount.round_dp(6));
        info!("║ ETH Price: ${:>48} ║", purchase.eth_price.round_dp(2));
        info!("║ Fees: ${:>53} ║", purchase.fees_usdc.round_dp(4));
        info!("║ Order ID: {:>49} ║", purchase.order_id);
    }

    info!("╚════════════════════════════════════════════════════════════════╝");
}
