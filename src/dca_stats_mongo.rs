// src/dca_stats_mongo.rs
use anyhow::Result;
use bson::doc;
use chrono::{DateTime, Utc};
use futures::stream::TryStreamExt;
use mongodb::{Client, Collection, bson::Document};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
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
    collection: Collection<DcaPurchase>,
}

impl DcaStatsDB {
    /// Connect using the default ETH collection (`dca_purchases`).
    pub async fn new() -> Result<Self> {
        Self::with_collection("dca_purchases").await
    }

    /// Connect to a specific purchases collection so each asset (ETH/BTC) keeps
    /// its own, independent history and stats.
    pub async fn with_collection(collection_name: &str) -> Result<Self> {
        let mongodb_url = std::env::var("MONGODB_URL").unwrap_or_else(|_| {
            "mongodb://dca_user:dca_password@localhost:27017/dca_bot".to_string()
        });

        let client = Client::with_uri_str(&mongodb_url).await?;
        let database = client.database("dca_bot");
        let collection = database.collection(collection_name);

        info!("🍃 Connected to MongoDB collection '{}'", collection_name);
        Ok(Self { collection })
    }

    pub async fn record_purchase(&self, purchase: &DcaPurchase) -> Result<()> {
        self.collection.insert_one(purchase).await?;
        info!("💾 Purchase recorded in database: {}", purchase.id);
        Ok(())
    }

    pub async fn get_summary(&self, current_eth_price: Decimal) -> Result<DcaSummary> {
        let pipeline = vec![
            Document::from(doc! {
                "$match": {
                    "status": "FILLED"
                }
            }),
            Document::from(doc! {
                "$group": {
                    "_id": null,
                    "total_purchases": { "$sum": 1 },
                    "total_usdc_invested": { "$sum": { "$toDouble": "$usdc_amount" } },
                    "total_eth_acquired": { "$sum": { "$toDouble": "$eth_amount" } },
                    "total_fees_paid": { "$sum": { "$toDouble": "$fees_usdc" } },
                    "first_purchase": { "$min": "$timestamp" },
                    "last_purchase": { "$max": "$timestamp" }
                }
            }),
        ];

        let mut cursor = self.collection.aggregate(pipeline).await?;

        if let Some(doc) = cursor.try_next().await? {
            let total_purchases = doc.get_i32("total_purchases").unwrap_or(0) as i64;
            
            // Try multiple field access methods to handle different MongoDB number types
            let total_usdc_invested = if let Ok(val) = doc.get_f64("total_usdc_invested") {
                Decimal::from_f64_retain(val).unwrap_or(dec!(0))
            } else if let Ok(val) = doc.get_str("total_usdc_invested") {
                val.parse::<Decimal>().unwrap_or(dec!(0))
            } else {
                dec!(0)
            };
            
            let total_eth_acquired = if let Ok(val) = doc.get_f64("total_eth_acquired") {
                Decimal::from_f64_retain(val).unwrap_or(dec!(0))
            } else if let Ok(val) = doc.get_str("total_eth_acquired") {
                val.parse::<Decimal>().unwrap_or(dec!(0))
            } else {
                dec!(0)
            };
            
            let total_fees_paid = if let Ok(val) = doc.get_f64("total_fees_paid") {
                Decimal::from_f64_retain(val).unwrap_or(dec!(0))
            } else if let Ok(val) = doc.get_str("total_fees_paid") {
                val.parse::<Decimal>().unwrap_or(dec!(0))
            } else {
                dec!(0)
            };

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
                first_purchase: doc
                    .get_datetime("first_purchase")
                    .ok()
                    .map(|dt| dt.to_chrono()),
                last_purchase: doc
                    .get_datetime("last_purchase")
                    .ok()
                    .map(|dt| dt.to_chrono()),
                current_eth_value,
                unrealized_pnl,
                unrealized_pnl_percentage,
            })
        } else {
            // No purchases yet
            Ok(DcaSummary {
                total_purchases: 0,
                total_usdc_invested: dec!(0),
                total_eth_acquired: dec!(0),
                total_fees_paid: dec!(0),
                average_eth_price: dec!(0),
                first_purchase: None,
                last_purchase: None,
                current_eth_value: dec!(0),
                unrealized_pnl: dec!(0),
                unrealized_pnl_percentage: dec!(0),
            })
        }
    }

    pub async fn get_recent_purchases(&self, limit: i64) -> Result<Vec<DcaPurchase>> {
        let mut cursor = self.collection.find(Document::new()).await?;

        let mut purchases = Vec::new();
        while let Some(purchase) = cursor.try_next().await? {
            purchases.push(purchase);
            if purchases.len() >= limit as usize {
                break;
            }
        }

        // Sort by timestamp in descending order
        purchases.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        Ok(purchases)
    }

    pub async fn has_purchase_in_last_24h(&self) -> Result<bool> {
        let twenty_four_hours_ago = chrono::Utc::now() - chrono::Duration::hours(24);
        
        let filter = doc! {
            "timestamp": {
                "$gte": twenty_four_hours_ago.to_rfc3339()
            },
            "status": "FILLED"
        };
        
        let count = self.collection.count_documents(filter).await?;
        Ok(count > 0)
    }

    pub async fn has_purchase_in_time_window(
        &self, 
        start: chrono::DateTime<chrono::Utc>, 
        end: chrono::DateTime<chrono::Utc>
    ) -> Result<bool> {
        let filter = doc! {
            "timestamp": {
                "$gte": start.to_rfc3339(),
                "$lte": end.to_rfc3339()
            },
            "status": "FILLED"
        };
        
        let count = self.collection.count_documents(filter).await?;
        Ok(count > 0)
    }
}

// Include the same print functions from the SQLite version
pub fn print_dca_summary(asset: &str, summary: &DcaSummary) {
    info!("╔═══════════════════════════════════════╗");
    info!("║          📊 {:>3} DCA SUMMARY           ║", asset);
    info!("╠═══════════════════════════════════════╣");
    info!("║ Total Purchases: {:>19} ║", summary.total_purchases);
    info!(
        "║ Total USDC Invested: ${:>13} ║",
        summary.total_usdc_invested.round_dp(2)
    );
    info!(
        "║ Total {:>3} Acquired: {:>14} ║",
        asset,
        summary.total_eth_acquired.round_dp(6)
    );
    info!(
        "║ Total Fees Paid: ${:>16} ║",
        summary.total_fees_paid.round_dp(2)
    );
    info!(
        "║ Average {:>3} Price: ${:>15} ║",
        asset,
        summary.average_eth_price.round_dp(2)
    );
    info!("╠═══════════════════════════════════════╣");
    info!(
        "║ Current {:>3} Value: ${:>15} ║",
        asset,
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

pub fn print_recent_purchases(asset: &str, purchases: &[DcaPurchase]) {
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
        info!("║ {:>3} Acquired: {:>45} ║", asset, purchase.eth_amount.round_dp(6));
        info!("║ {:>3} Price: ${:>48} ║", asset, purchase.eth_price.round_dp(2));
        info!("║ Fees: ${:>53} ║", purchase.fees_usdc.round_dp(4));
        info!("║ Order ID: {:>49} ║", purchase.order_id);
    }

    info!("╚════════════════════════════════════════════════════════════════╝");
}
