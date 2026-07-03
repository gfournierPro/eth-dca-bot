//! Exchange abstraction.
//!
//! The DCA bot can run against more than one exchange. Binance was the original
//! backend; Kraken was added when regulation forced a switch. Both are kept so the
//! active exchange can be flipped with a single config value (`EXCHANGE`) — e.g. to
//! move back to Binance later — without touching the trading logic.
//!
//! Every exchange speaks in the same USDC-quoted terms the DCA logic expects:
//! balances in USDC, prices in USDC, and buys sized by a USDC amount.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::dca_stats_mongo::DcaPurchase;

/// Tuning for a "patient maker" limit buy (see [`Exchange::place_limit_buy`]).
///
/// The idea: rest a post-only limit order at the best bid to pay the lower maker
/// fee, re-peg it as the market moves, and only cross the spread with a taker
/// market order if the price drifts beyond `max_drift` or `hard_timeout` elapses —
/// so the buy always eventually fills.
#[derive(Debug, Clone)]
pub struct LimitBuyConfig {
    /// Fraction the best ask may rise above the starting ask before giving up on
    /// maker fills and falling back to a market order (e.g. `0.003` = 0.3%).
    pub max_drift: Decimal,
    /// Hard cap on how long to chase a maker fill before falling back to market.
    pub hard_timeout: Duration,
    /// How often to re-check the book / resting order state.
    pub poll_interval: Duration,
    /// Stop chasing once the unspent budget is this much USDC or less — the
    /// remainder is dust not worth another order. Was previously hardcoded.
    pub min_remaining: Decimal,
}

impl Default for LimitBuyConfig {
    fn default() -> Self {
        Self {
            max_drift: Decimal::new(3, 3), // 0.003 = 0.3%
            hard_timeout: Duration::from_secs(180),
            poll_interval: Duration::from_secs(1),
            min_remaining: Decimal::new(5, 1), // 0.5 USDC
        }
    }
}

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

    /// Buy `quote_usdc` worth of `symbol`, preferring the cheaper maker fee via a
    /// resting post-only limit order but always eventually filling (see
    /// [`LimitBuyConfig`]).
    ///
    /// The default implementation just takes liquidity with a market order, for
    /// exchanges that don't (yet) implement a maker strategy. Kraken overrides it.
    async fn place_limit_buy(
        &self,
        symbol: &str,
        quote_usdc: Decimal,
        cfg: &LimitBuyConfig,
    ) -> Result<OrderOutcome> {
        let _ = cfg;
        self.place_market_buy(symbol, quote_usdc).await
    }

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
