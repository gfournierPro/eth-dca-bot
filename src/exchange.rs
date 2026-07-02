//! Exchange abstraction.
//!
//! The DCA bot can run against more than one exchange. Binance was the original
//! backend; Kraken was added when regulation forced a switch. Both are kept so the
//! active exchange can be flipped with a single config value (`EXCHANGE`) — e.g. to
//! move back to Binance later — without touching the trading logic.
//!
//! Every exchange speaks in the same USDC-quoted terms the DCA logic expects:
//! balances in USDC, prices in USDC, and buys sized by a USDC amount.

use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::dca_stats_mongo::DcaPurchase;

/// The result of a completed market buy, normalised across exchanges.
#[derive(Debug, Clone)]
pub struct OrderOutcome {
    /// Exchange order identifier (Binance numeric id as a string, or Kraken txid).
    pub order_id: String,
    /// Normalised status; `"FILLED"` once the order is fully executed.
    pub status: String,
    /// Base asset quantity acquired (ETH/BTC).
    pub executed_qty: Decimal,
    /// Quote value spent, in USDC.
    pub executed_value: Decimal,
    /// Average fill price, in USDC.
    pub avg_price: Decimal,
    /// Total trading fees, expressed in USDC.
    pub fees_usdc: Decimal,
}

/// Operations the DCA workflow needs from a spot exchange.
///
/// All amounts and prices are USDC-denominated. Implementors are responsible for
/// translating the generic symbols/assets used by the bot (`ETHUSDC`, `BTCUSDC`,
/// `ETH`, `BTC`) into whatever the exchange expects.
#[async_trait]
pub trait Exchange: Send + Sync {
    /// Human-readable exchange name, used for logs and the Notion "From" label.
    fn name(&self) -> &'static str;

    /// Free USDC balance available for trading.
    async fn get_usdc_balance(&self) -> Result<Decimal>;

    /// Free balance of an arbitrary asset (e.g. `ETH`, `BTC`).
    async fn get_asset_balance(&self, asset: &str) -> Result<Decimal>;

    /// Last price for a USDC-quoted trading pair (e.g. `ETHUSDC`).
    async fn get_price(&self, symbol: &str) -> Result<Decimal>;

    /// How many USDC one EUR is worth, used to size EUR-denominated buys.
    async fn get_usdc_per_eur(&self) -> Result<Decimal>;

    /// Place a market buy sized by a USDC amount and return the fill details.
    async fn place_market_buy(&self, symbol: &str, quote_usdc: Decimal) -> Result<OrderOutcome>;

    /// Reconstruct this month's filled buy purchases from the exchange, used as a
    /// fallback when the local database has no record.
    async fn get_current_month_purchases(&self, symbol: &str) -> Result<Vec<DcaPurchase>>;

    /// Verify a withdrawal of `amount` `asset` to `destination` is possible.
    ///
    /// `destination` is an on-chain address + `network` on Binance, or a pre-saved
    /// withdrawal key name on Kraken (where `network` is implied by the key).
    async fn verify_withdrawal(
        &self,
        asset: &str,
        destination: &str,
        network: &str,
        amount: Decimal,
    ) -> Result<bool>;

    /// Initiate a withdrawal and return the exchange's withdrawal id/refid.
    async fn withdraw(
        &self,
        asset: &str,
        destination: &str,
        amount: Decimal,
        network: &str,
    ) -> Result<String>;
}
