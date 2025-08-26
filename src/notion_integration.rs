use anyhow::{Result, anyhow};
use chrono::{DateTime, Datelike, Utc};
use notion_client::endpoints::Client as NotionClient;
use notion_client::endpoints::databases::query::request::QueryDatabaseRequestBuilder;
use notion_client::endpoints::pages::create::request::CreateAPageRequest;
use notion_client::endpoints::pages::update::request::UpdatePagePropertiesRequest;
use notion_client::objects::page::{DateOrDateTime, DatePropertyValue, PageProperty};
use notion_client::objects::parent::Parent;
use notion_client::objects::rich_text::{RichText, Text};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use std::collections::BTreeMap;
use std::collections::HashMap;
use tracing::info;

use crate::dca_stats_mongo::DcaPurchase;

#[derive(Debug, Clone)]
pub struct MonthlyDCAData {
    pub page_id: Option<String>,
    pub month_name: String,
    pub from: String,
    pub link: Option<String>,
    pub network_fee_eth: Decimal,
    pub trading_fee_eth: Decimal,
    pub when: DateTime<Utc>,
    pub currency_eth: Decimal,
    pub eur: Decimal,
}

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
        _eur_amount: Decimal,
    ) -> Result<()> {
        let month_name = self.get_month_name(&purchase.timestamp);
        info!("Recording DCA purchase to Notion page: {}", month_name);

        let _page_id = self
            .get_or_create_monthtly_page(&month_name, &purchase.timestamp)
            .await?;

        Ok(())
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
        let request = QueryDatabaseRequestBuilder::default();
        // For now, we'll query all pages and filter in code
        // The notion-client doesn't seem to support filter in the builder easily

        let response = self
            .client
            .databases
            .query_a_database(&self.database_id, request.build().unwrap())
            .await
            .map_err(|e| anyhow!("Failed to query Notion database: {}", e))?;

        // Look for a page with the matching title
        for page in response.results {
            if let Some(title_property) = page.properties.get("Name") {
                if let PageProperty::Title { title, .. } = title_property {
                    if let Some(first_text) = title.first() {
                        if let RichText::Text { text, .. } = first_text {
                            if text.content == month_name {
                                return Ok(page.id);
                            }
                        }
                    }
                }
            }
        }

        Err(anyhow!("Page not found for month: {}", month_name))
    }

    async fn create_monthly_page(&self, month_name: &str, date: &DateTime<Utc>) -> Result<String> {
        info!("Creating new Notion page for month: {}", month_name);

        let mut properties = BTreeMap::new();

        properties.insert(
            "Name".to_string(),
            PageProperty::Title {
                id: None,
                title: vec![RichText::Text {
                    text: Text {
                        content: month_name.to_string(),
                        link: None,
                    },
                    annotations: None,
                    plain_text: None,
                    href: None,
                }],
            },
        );

        properties.insert(
            "From".to_string(),
            PageProperty::Select {
                id: None,
                select: Some(notion_client::objects::page::SelectPropertyValue {
                    id: None,
                    name: Some("Binance".to_string()),
                    color: None,
                }),
            },
        );

        properties.insert(
            "Network fee".to_string(),
            PageProperty::Number {
                id: None,
                number: Some(serde_json::Number::from_f64(0.0).unwrap()),
            },
        );

        properties.insert(
            "Trading fee".to_string(),
            PageProperty::Number {
                id: None,
                number: Some(serde_json::Number::from_f64(0.0).unwrap()),
            },
        );

        properties.insert(
            "when".to_string(),
            PageProperty::Date {
                id: None,
                date: Some(DatePropertyValue {
                    start: Some(DateOrDateTime::Date(date.date_naive().with_day(1).unwrap())),
                    end: None,
                    time_zone: None,
                }),
            },
        );

        properties.insert(
            "Currency".to_string(),
            PageProperty::Number {
                id: None,
                number: Some(serde_json::Number::from_f64(0.0).unwrap()),
            },
        );

        properties.insert(
            "eur".to_string(),
            PageProperty::Number {
                id: None,
                number: Some(serde_json::Number::from_f64(0.0).unwrap()),
            },
        );

        let create_request = CreateAPageRequest {
            parent: Parent::DatabaseId {
                database_id: self.database_id.clone(),
            },
            icon: None,
            cover: None,
            properties,
            children: None,
        };

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

    async fn update_monthly_page(
        &self,
        page_id: &str,
        purchase: &DcaPurchase,
        eur_amount: Decimal,
    ) -> Result<()> {
        // First, get current page data to accumulate values
        let current_data = self.get_current_page_data(page_id).await?;

        // Calculate new accumulated values
        let new_network_fee = current_data.network_fee_eth; // We'll update this when we have transfer data
        let new_trading_fee = current_data.trading_fee_eth + purchase.fees_usdc; // Assuming fees are in USDC, convert if needed
        let new_currency_eth = current_data.currency_eth + purchase.eth_amount;
        let new_eur_amount = current_data.eur + eur_amount;

        // Generate Arbitrum link placeholder (you'll need to implement actual transfer tracking)
        let arbitrum_link = format!("https://arbiscan.io/address/{}", self.cold_wallet_address);

        let update_request = UpdatePagePropertiesRequest {
            properties: {
                let mut props = BTreeMap::new();

                props.insert(
                    "Link".to_string(),
                    Some(PageProperty::Url {
                        id: None,
                        url: Some(arbitrum_link),
                    }),
                );

                props.insert(
                    "Network fee".to_string(),
                    Some(PageProperty::Number {
                        id: None,
                        number: new_network_fee
                            .to_f64()
                            .map(|f| serde_json::Number::from_f64(f).unwrap()),
                    }),
                );

                props.insert(
                    "Trading fee".to_string(),
                    Some(PageProperty::Number {
                        id: None,
                        number: new_trading_fee
                            .to_f64()
                            .map(|f| serde_json::Number::from_f64(f).unwrap()),
                    }),
                );

                props.insert(
                    "Currency".to_string(),
                    Some(PageProperty::Number {
                        id: None,
                        number: new_currency_eth
                            .to_f64()
                            .map(|f| serde_json::Number::from_f64(f).unwrap()),
                    }),
                );

                props.insert(
                    "eur".to_string(),
                    Some(PageProperty::Number {
                        id: None,
                        number: new_eur_amount
                            .to_f64()
                            .map(|f| serde_json::Number::from_f64(f).unwrap()),
                    }),
                );

                props
            },
            icon: None,
            cover: None,
            archived: None,
        };

        self.client
            .pages
            .update_page_properties(page_id, update_request)
            .await
            .map_err(|e| anyhow!("Failed to update Notion page: {}", e))?;

        info!(" Updated monthly accumulation:");
        info!(
            "    Total ETH this month: {} ETH",
            new_currency_eth.round_dp(6)
        );
        info!("    Total EUR spent: €{}", new_eur_amount.round_dp(2));
        info!("     Total trading fees: {}", new_trading_fee.round_dp(6));

        Ok(())
    }

    async fn get_current_page_data(&self, page_id: &str) -> Result<MonthlyDCAData> {
        let page = self
            .client
            .pages
            .retrieve_a_page(page_id, None)
            .await
            .map_err(|e| anyhow!("Failed to retrieve Notion page: {}", e))?;

        // Extract current values from page properties
        let properties = page.properties;

        let network_fee_eth = self
            .extract_number_property(&properties, "Network fee")
            .unwrap_or(dec!(0));
        let trading_fee_eth = self
            .extract_number_property(&properties, "Trading fee")
            .unwrap_or(dec!(0));
        let currency_eth = self
            .extract_number_property(&properties, "Currency")
            .unwrap_or(dec!(0));
        let eur = self
            .extract_number_property(&properties, "eur")
            .unwrap_or(dec!(0));

        Ok(MonthlyDCAData {
            page_id: Some(page_id.to_string()),
            month_name: "".to_string(), // We don't need this for updates
            from: "Binance".to_string(),
            link: None,
            network_fee_eth,
            trading_fee_eth,
            when: Utc::now(), // We don't update the date
            currency_eth,
            eur,
        })
    }

    fn extract_number_property(
        &self,
        properties: &HashMap<String, PageProperty>,
        property_name: &str,
    ) -> Option<Decimal> {
        if let Some(property) = properties.get(property_name) {
            if let PageProperty::Number { number, .. } = property {
                if let Some(num) = number {
                    return num.as_f64().and_then(|f| Decimal::from_f64_retain(f));
                }
            }
        }
        None
    }

    pub async fn get_monthly_summary(&self, month_name: &str) -> Result<MonthlyDCAData> {
        let page_id = self.find_existing_page(month_name).await?;
        self.get_current_page_data(&page_id).await
    }
}
