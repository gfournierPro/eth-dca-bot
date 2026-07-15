// src/dca_stats_mongo.rs
use anyhow::Result;
use bson::doc;
use chrono::{DateTime, Utc};
use futures::stream::TryStreamExt;
use mongodb::{Client, Collection, bson::Document};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcaPurchase {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub symbol: String,
    #[serde(default = "default_side")]
    pub side: String, // "BUY" or "SELL", defaults to "BUY" for backward compatibility
    pub usdc_amount: Decimal,
    pub eth_amount: Decimal,
    pub eth_price: Decimal,
    pub fees_usdc: Decimal,
    /// Exchange order identifier. Binance order IDs are numeric while Kraken txids
    /// are strings (e.g. `OQCLML-BW3P3-BUCMWZ`), so this is stored as a string.
    /// Historical records that persisted a numeric order id are still read correctly
    /// via [`de_order_id`].
    #[serde(deserialize_with = "de_order_id")]
    pub order_id: String,
    pub status: String,
}

fn default_side() -> String {
    "BUY".to_string()
}

/// Accept both string and integer order ids so purchases written by the old
/// (numeric, Binance) schema keep deserializing after the switch to string ids.
fn de_order_id<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use std::fmt;
    struct OrderIdVisitor;
    impl<'de> serde::de::Visitor<'de> for OrderIdVisitor {
        type Value = String;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a string or integer order id")
        }
        fn visit_str<E>(self, v: &str) -> std::result::Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_string<E>(self, v: String) -> std::result::Result<String, E> {
            Ok(v)
        }
        fn visit_i64<E>(self, v: i64) -> std::result::Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_u64<E>(self, v: u64) -> std::result::Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_i32<E>(self, v: i32) -> std::result::Result<String, E> {
            Ok(v.to_string())
        }
    }
    deserializer.deserialize_any(OrderIdVisitor)
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

    /// Look up a stored purchase by its exchange order id (string txid or the
    /// stringified numeric id of a legacy Binance order).
    pub async fn get_purchase_by_order_id(&self, order_id: &str) -> Result<Option<DcaPurchase>> {
        let filter = doc! { "order_id": order_id };
        let purchase = self.collection.find_one(filter).await?;
        Ok(purchase)
    }

    /// Remove a stored purchase by its exchange order id. Returns whether a
    /// document was actually deleted.
    pub async fn remove_purchase_by_order_id(&self, order_id: &str) -> Result<bool> {
        let filter = doc! { "order_id": order_id };
        let result = self.collection.delete_one(filter).await?;
        Ok(result.deleted_count > 0)
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
                    // For BUY orders: subtract USDC spent (negative cash flow), add ETH acquired (positive ETH balance)
                    // For SELL orders: add USDC received (positive cash flow), subtract ETH sold (negative ETH balance)
                    "total_usdc_invested": { 
                        "$sum": { 
                            "$cond": [
                                { "$or": [
                                    { "$eq": ["$side", "BUY"] },
                                    { "$eq": ["$side", null] }  // Handle missing side field as BUY
                                ]},
                                { "$multiply": [{ "$toDouble": "$usdc_amount" }, -1] }, // BUY: subtract USDC spent
                                { "$toDouble": "$usdc_amount" } // SELL: add USDC received
                            ]
                        }
                    },
                    "total_eth_acquired": { 
                        "$sum": { 
                            "$cond": [
                                { "$or": [
                                    { "$eq": ["$side", "BUY"] },
                                    { "$eq": ["$side", null] }  // Handle missing side field as BUY
                                ]},
                                { "$toDouble": "$eth_amount" }, // BUY: add ETH acquired
                                { "$multiply": [{ "$toDouble": "$eth_amount" }, -1] } // SELL: subtract ETH sold
                            ]
                        }
                    },
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

            // Calculate average price based on absolute values
            let average_eth_price = if total_eth_acquired > dec!(0) {
                total_usdc_invested.abs() / total_eth_acquired
            } else {
                dec!(0)
            };

            let current_eth_value = total_eth_acquired * current_eth_price;
            // P&L = current value of ETH + net cash flow (negative cash flow means we spent money)
            let unrealized_pnl = current_eth_value + total_usdc_invested;
            let unrealized_pnl_percentage = if total_usdc_invested.abs() > dec!(0) {
                (unrealized_pnl / total_usdc_invested.abs()) * dec!(100)
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

    /// Close-time of the most recently recorded purchase, or `None` if the
    /// collection is empty. A proper sorted query (unlike `get_recent_purchases`,
    /// which sorts a bounded, unordered cursor page). The sleeve uses this to bound
    /// its Kraken `ClosedOrders` scan to "since our newest recorded fill".
    pub async fn latest_purchase_timestamp(&self) -> Result<Option<DateTime<Utc>>> {
        let latest = self
            .collection
            .find_one(Document::new())
            .sort(doc! { "timestamp": -1 })
            .await?;
        Ok(latest.map(|p| p.timestamp))
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
        end: chrono::DateTime<chrono::Utc>,
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

    /// Get all existing order IDs from the database
    pub async fn get_all_order_ids(&self) -> Result<Vec<String>> {
        use futures::StreamExt;

        let pipeline = vec![
            Document::from(doc! {
                "$project": {
                    "order_id": 1,
                    "_id": 0
                }
            })
        ];

        let mut cursor = self.collection.aggregate(pipeline).await?;
        let mut order_ids = Vec::new();

        while let Some(doc) = cursor.next().await {
            if let Ok(doc) = doc {
                if let Ok(order_id_str) = doc.get_str("order_id") {
                    order_ids.push(order_id_str.to_string());
                }
            }
        }

        Ok(order_ids)
    }

    /// Sync missing orders (both purchases and sales) from Binance to MongoDB
    /// Returns the number of orders added
    pub async fn sync_missing_orders_from_binance(
        &self,
        binance_client: &crate::binance::BinanceClient,
        symbol: &str,
        start_date: chrono::DateTime<chrono::Utc>,
        min_order_id: Option<u64>,
    ) -> Result<usize> {
        info!("🔄 Starting sync of missing orders from Binance...");
        
        // Get all historical orders from Binance
        let binance_orders = binance_client
            .get_historical_dca_orders(symbol, start_date, min_order_id)
            .await?;
        
        if binance_orders.is_empty() {
            info!("📝 No orders found on Binance for the specified criteria");
            return Ok(0);
        }
        
        // Get all existing order IDs from database
        let existing_order_ids = self.get_all_order_ids().await?;
        let existing_ids_set: std::collections::HashSet<String> = existing_order_ids.into_iter().collect();

        info!("📊 Found {} existing orders in database", existing_ids_set.len());
        info!("📊 Found {} orders from Binance", binance_orders.len());

        // Filter out orders that already exist in database
        let missing_orders: Vec<&DcaPurchase> = binance_orders
            .iter()
            .filter(|order| !existing_ids_set.contains(&order.order_id))
            .collect();
        
        if missing_orders.is_empty() {
            info!("✅ All Binance orders are already in the database - no sync needed");
            return Ok(0);
        }
        
        info!("🔄 Found {} missing orders to sync", missing_orders.len());
        
        // Add missing orders to database
        let mut added_count = 0;
        for order in missing_orders {
            match self.record_purchase(order).await {
                Ok(()) => {
                    added_count += 1;
                    let order_type = if order.side == "BUY" { "purchase" } else { "sale" };
                    info!("✅ Added missing {}: Order ID {} ({}) from {}", 
                          order_type,
                          order.order_id, 
                          order.side,
                          order.timestamp.format("%Y-%m-%d %H:%M:%S UTC"));
                }
                Err(e) => {
                    error!("❌ Failed to add order {}: {}", order.order_id, e);
                }
            }
        }
        
        info!("🎉 Sync completed! Added {} missing orders to database", added_count);
        Ok(added_count)
    }

    /// Verify database integrity by checking if all recent Binance trades are in the database
    /// Returns a summary of missing orders
    pub async fn verify_database_integrity(
        &self,
        binance_client: &crate::binance::BinanceClient,
        symbol: &str,
        start_date: chrono::DateTime<chrono::Utc>,
    ) -> Result<(usize, usize, Vec<String>)> {
        info!("🔍 Verifying database integrity against Binance records...");

        // Get all orders from Binance since start date
        let binance_orders = binance_client
            .get_historical_dca_orders(symbol, start_date, None)
            .await?;

        // Get all existing order IDs from database
        let existing_order_ids = self.get_all_order_ids().await?;
        let existing_ids_set: std::collections::HashSet<String> = existing_order_ids.into_iter().collect();

        // Find missing order IDs
        let missing_order_ids: Vec<String> = binance_orders
            .iter()
            .map(|order| order.order_id.clone())
            .filter(|order_id| !existing_ids_set.contains(order_id))
            .collect();
        
        let total_binance_orders = binance_orders.len();
        let missing_count = missing_order_ids.len();
        
        info!("📊 Database Integrity Report:");
        info!("   Total Binance orders since {}: {}", start_date.format("%Y-%m-%d"), total_binance_orders);
        info!("   Orders in database: {}", existing_ids_set.len());
        info!("   Missing orders: {}", missing_count);
        
        if !missing_order_ids.is_empty() {
            warn!("⚠️  Missing order IDs: {:?}", missing_order_ids);
        } else {
            info!("✅ Database is in sync with Binance records");
        }
        
        Ok((total_binance_orders, missing_count, missing_order_ids))
    }
}

// Include the same print functions from the SQLite version
pub fn print_dca_summary(asset: &str, summary: &DcaSummary) {
    info!("╔═══════════════════════════════════════╗");
    info!("║          📊 {:>3} DCA SUMMARY           ║", asset);
    info!("╠═══════════════════════════════════════╣");
    info!("║ Total Purchases: {:>19} ║", summary.total_purchases);
    info!(
        "║ Net USDC Spent: ${:>17} ║",
        summary.total_usdc_invested.abs().round_dp(2)
    );
    info!(
        "║ Net {:>3} Balance: {:>16} ║",
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
        
        let side_emoji = if purchase.side == "BUY" { "🟢" } else { "🔴" };
        
        info!(
            "║ Date: {:>55} ║",
            purchase.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
        );
        info!("║ Type: {} {:>53} ║", side_emoji, purchase.side);

        if purchase.side == "BUY" {
            info!("║ USDC Spent: ${:>47} ║", purchase.usdc_amount.round_dp(2));
            info!(
                "║ {:>3} Acquired: {:>45} ║",
                asset,
                purchase.eth_amount.round_dp(6)
            );
        } else {
            info!("║ USDC Received: ${:>44} ║", purchase.usdc_amount.round_dp(2));
            info!(
                "║ {:>3} Sold: {:>49} ║",
                asset,
                purchase.eth_amount.round_dp(6)
            );
        }

        info!(
            "║ {:>3} Price: ${:>48} ║",
            asset,
            purchase.eth_price.round_dp(2)
        );
        info!("║ Fees: ${:>53} ║", purchase.fees_usdc.round_dp(4));
        info!("║ Order ID: {:>49} ║", purchase.order_id);
    }

    info!("╚════════════════════════════════════════════════════════════════╝");
}
