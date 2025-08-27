use anyhow::{Result, anyhow};
use chrono::{DateTime, Datelike, Utc};
use notion_client::endpoints::Client as NotionClient;
use notion_client::endpoints::databases::query::request::QueryDatabaseRequestBuilder;
use notion_client::endpoints::pages::create::request::CreateAPageRequest;
use notion_client::endpoints::pages::update::request::UpdatePagePropertiesRequest;
use notion_client::endpoints::blocks::append::request::AppendBlockChildrenRequest;
use notion_client::objects::page::{DateOrDateTime, DatePropertyValue, PageProperty};
use notion_client::objects::parent::Parent;
use notion_client::objects::rich_text::{RichText, Text};
use notion_client::objects::block::{Block, BlockType, ParagraphValue, TextColor};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use std::collections::BTreeMap;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::dca_stats_mongo::DcaPurchase;
use crate::config::NotionConfig;

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

#[derive(Debug, Clone)]
pub struct NotionDCATracker {
    client: NotionClient,
    database_id: String,
    cold_wallet_address: String,
}

impl NotionDCATracker {
    pub fn new(config: &NotionConfig) -> Result<Self> {
        if config.token.is_empty() {
            return Err(anyhow!("Notion token is required"));
        }
        if config.database_id.is_empty() {
            return Err(anyhow!("Notion database ID is required"));
        }

        let client = NotionClient::new(config.token.clone(), None)?;
        info!("Notion integration initialized");
        info!("Database ID: {}", &config.database_id[..8]);
        info!("Cold wallet: {}", config.cold_wallet_address);

        Ok(Self {
            client,
            database_id: config.database_id.clone(),
            cold_wallet_address: config.cold_wallet_address.clone(),
        })
    }

    pub async fn record_dca_purchase(
        &self,
        purchase: &DcaPurchase,
        eur_amount: Decimal,
    ) -> Result<()> {
        let month_name = self.get_month_name(&purchase.timestamp);
        info!("Recording DCA purchase to Notion page: {}", month_name);

        let page_id = self
            .get_or_create_monthtly_page(&month_name, &purchase.timestamp)
            .await?;

        // Update the page with actual purchase data
        self.update_monthly_page(&page_id, purchase, eur_amount).await?;
        
        // Add detailed purchase content to the page
        self.add_dca_purchase_content(&page_id, purchase, eur_amount).await?;

        Ok(())
    }

    pub fn get_month_name(&self, date: &DateTime<Utc>) -> String {
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

    pub async fn find_existing_page(&self, month_name: &str) -> Result<String> {
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

    // Public method for testing - creates a monthly page with fake data
    pub async fn create_test_monthly_page(&self, month_name: &str, date: &DateTime<Utc>) -> Result<String> {
        self.create_monthly_page(month_name, date).await
    }

    // Method to inspect the database schema for debugging
    pub async fn inspect_database_schema(&self) -> Result<()> {
        info!("🔍 Inspecting database schema...");
        
        let response = self
            .client
            .databases
            .retrieve_a_database(&self.database_id)
            .await
            .map_err(|e| anyhow!("Failed to retrieve database schema: {}", e))?;

        info!("📋 Database properties:");
        for (name, property) in response.properties {
            let prop_type = match property {
                notion_client::objects::database::DatabaseProperty::Title { .. } => "Title",
                notion_client::objects::database::DatabaseProperty::Number { .. } => "Number",
                notion_client::objects::database::DatabaseProperty::Select { .. } => "Select",
                notion_client::objects::database::DatabaseProperty::Date { .. } => "Date",
                notion_client::objects::database::DatabaseProperty::Url { .. } => "URL",
                notion_client::objects::database::DatabaseProperty::RichText { .. } => "RichText",
                _ => "Other",
            };
            info!("  - {}: {}", name, prop_type);
        }
        
        Ok(())
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
            "Network Fee".to_string(),
            PageProperty::Number {
                id: None,
                number: Some(serde_json::Number::from_f64(0.0).unwrap()),
            },
        );

        properties.insert(
            "Trading Fee".to_string(),
            PageProperty::Number {
                id: None,
                number: Some(serde_json::Number::from_f64(0.0).unwrap()),
            },
        );

        properties.insert(
            "When".to_string(),
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

        // Create some sample content blocks for the page
        // For now, we'll create the page without content and add it later
        // as the notion-client library's block creation is quite complex
        
        let create_request = CreateAPageRequest {
            parent: Parent::DatabaseId {
                database_id: self.database_id.clone(),
            },
            icon: None,
            cover: None,
            properties,
            children: None, // We'll add content after page creation
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
        
        // Convert trading fee from USDC to ETH using the purchase price
        let trading_fee_eth = if purchase.eth_price > Decimal::ZERO {
            purchase.fees_usdc / purchase.eth_price
        } else {
            Decimal::ZERO
        };
        let new_trading_fee = current_data.trading_fee_eth + trading_fee_eth;
        
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
                    "Network Fee".to_string(),
                    Some(PageProperty::Number {
                        id: None,
                        number: new_network_fee
                            .to_f64()
                            .map(|f| serde_json::Number::from_f64(f).unwrap()),
                    }),
                );

                props.insert(
                    "Trading Fee".to_string(),
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
        info!("     Total trading fees: {} ETH", new_trading_fee.round_dp(6));

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
            .extract_number_property(&properties, "Network Fee")
            .unwrap_or(dec!(0));
        let trading_fee_eth = self
            .extract_number_property(&properties, "Trading Fee")
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

    // Method to add actual DCA purchase content to a page
    async fn add_dca_purchase_content(&self, page_id: &str, purchase: &DcaPurchase, eur_amount: Decimal) -> Result<()> {
        info!("Adding DCA purchase content to page...");
        
        // Calculate trading fee in ETH
        let trading_fee_eth = if purchase.eth_price > Decimal::ZERO {
            purchase.fees_usdc / purchase.eth_price
        } else {
            Decimal::ZERO
        };
        
        // Format the purchase details with real data
        let title_line = format!("📈 DCA Purchase - {}", purchase.timestamp.format("%Y-%m-%d %H:%M UTC"));
        let amount_line = format!("💰 Amount: ${} USDC → {} ETH", 
            purchase.usdc_amount.round_dp(2), 
            purchase.eth_amount.round_dp(6)
        );
        let price_line = format!("🏷️ ETH Price: ${}", purchase.eth_price.round_dp(2));
        let fees_line = format!("💸 Trading Fee: {} ETH (${} USDC)", 
            trading_fee_eth.round_dp(6), 
            purchase.fees_usdc.round_dp(4)
        );
        let eur_line = format!("🇪🇺 EUR Equivalent: €{}", eur_amount.round_dp(2));
        let order_line = format!("🔗 Order ID: {}", purchase.order_id);
        let status_line = format!("✅ Status: {}", purchase.status);
        
        let content_lines = vec![
            title_line.as_str(),
            amount_line.as_str(),
            price_line.as_str(),
            fees_line.as_str(),
            eur_line.as_str(),
            order_line.as_str(),
            status_line.as_str(),
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", // Separator
        ];
        
        // Create paragraph blocks for each line
        let mut blocks = Vec::new();
        
        for line in content_lines {
            let paragraph_block = Block {
                object: None,
                id: None,
                parent: None,
                block_type: BlockType::Paragraph {
                    paragraph: ParagraphValue {
                        rich_text: vec![RichText::Text {
                            text: Text {
                                content: line.to_string(),
                                link: None,
                            },
                            annotations: None,
                            plain_text: None,
                            href: None,
                        }],
                        color: Some(TextColor::Default),
                        children: None,
                    },
                },
                created_time: None,
                created_by: None,
                last_edited_time: None,
                last_edited_by: None,
                archived: None,
                has_children: None,
            };
            blocks.push(paragraph_block);
        }
        
        // Create the append request
        let append_request = AppendBlockChildrenRequest {
            children: blocks,
            after: None,
        };
        
        // Add the blocks to the page
        match self.client.blocks.append_block_children(page_id, append_request).await {
            Ok(_) => {
                info!("✅ Successfully added DCA purchase details to Notion page!");
            }
            Err(e) => {
                warn!("⚠️  Failed to add purchase content to page: {}", e);
                // Log what we would have added for debugging
                info!("📝 Purchase details that we attempted to add:");
                for line in &[&title_line, &amount_line, &price_line, &fees_line, &eur_line, &order_line, &status_line] {
                    info!("   {}", line);
                }
                return Err(anyhow!("Failed to add purchase content: {}", e));
            }
        }
        
        Ok(())
    }
}
