use anyhow::anyhow;
use chrono::{DateTime, Datelike, Utc};
use notion_client::endpoints::Client as NotionClient;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tracing::info;

use crate::dca_stats_mongo::DcaPurchase;

pub struct NotionDCATracker {
    client: NotionClient,
    database_id: String,
    cold_wallet_address: String,
}

impl NotionDCATracker {
    pub fn new() -> Result<Self> {
        let notion_token = std::env::var("NOTION_TOKEN")
            .map_err(|_| anyhow!("NOTION_TOKEN environment variable is required"))?;

        let database_id = std::env::var("NOTION_DATABASE_ID")
            .map_err(|_| anyhow!("NOTION_DATABASE_ID environment variable is required"))?;

        let cold_wallet_address = std::env::var("COLD_WALLET_ADDRESS")
            .unwrap_or_else(|_| "0xa416610975634033374EEdAE26D0FCa7A7360b70".to_string());

        let client = NotionClient::new(notion_token, None)?;
        info!("Notion integration initialized");
        info!("Database ID: {}", &database_id[..8]);
        info!("Cold wallet: {}", cold_wallet_address);

        Ok(Self {
            client,
            database_id,
            cold_wallet_address,
        })
    }

    pub async fn record_dca_purchase(
        &self,
        purchase: &DcaPurchase,
        eur_amount: Decimal,
    ) -> Result<()> {
        let month_name = self.get_month_name(&purchase.timestamp);
        info!("Recording DCA purchase to Notion page: {}", month_name);

        match self
            .get_or_create_monthtly_page(&month_name, &purchase.timestamp)
            .await {}

        Ok(1)
    }

    fn get_month_name(&self, date: &DateTime<Utc>) -> String {
        let month_number = date.month0();
        format!("M{}", month_number)
    }

    async fn get_or_create_monthtly_page(
        &self,
        month_name: &str,
        date: &DateTime<Utc>,
    ) -> Result<String> {
        if let Ok(page_id) = self.find_existing_page(month_name).await {
            return Ok(page_id);
        }
        self.create_monthly_page(month_name, date).await
    }

    async fn find_existing_page(&self, month_name: &str) -> Result<String> {
        let query = json!({
            "filter": {
                "property": "Name",
                "title" : {
                    "equals" : mont_name
                }
            }
        });
        let response = self
            .client
            .databases
            .query_a_database(&self.database_id, Some(query))
            .await
            .map_err(|e| anyhow!("Failed to query Notion database: {}", e))?;
        if let Some(page) = response.results.first() {
            Ok(page.id.clone())
        } else {
            Err(anyhow!("Page not found for month: {}", month_name))
        }
    }

    async fn create_monthly_page(&self, month_name: &str, date: &DateTime<Utc>) -> Result<String> {
        info!("Creating new Notion page for month: {}", month_name);
        let properties = json!({
            "Name": {
                "title": [
                    {
                        "text": {
                            "content": month_name
                        }
                    }
                ]
            },
            "From": {
                "select": {
                    "name": "Binance"
                }
            },
            "Network fee": {
                "number": 0.0
            },
            "Trading fee": {
                "number": 0.0
            },
            "when": {
                "date": {
                    "start": date.format("%Y-%m-01").to_string()
                }
            },
            "Currency": {
                "number": 0.0
            },
            "eur": {
                "number": 0.0
            }
        });

        let create_request = json!({
            "parent": {
                "database_id": self.database_id
            },
            "properties": properties
        });

        let response = self
            .client
            .pages
            .create_a_page(create_request)
            .await
            .map_err(|e| anyhow!("Failed to create Notion page: {}", e))?;

        info!(
            "Created new Notion page: {} (ID: {})",
            month_name,
            &response.id[..8]
        );
        Ok(response.id)
    }
}
