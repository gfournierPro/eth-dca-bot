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
use crate::levels::{BidLadder, VolumeProfileConfig};

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

// --- Limit-order sleeve support ---------------------------------------------
//
// The sleeve needs resting-order primitives (place/cancel/query a post-only
// limit, list its own open/closed orders) that aren't part of the core
// `Exchange` trait, since only some backends (Kraken, OKX) support them —
// Binance does not. Kept as a separate trait for that reason.

/// Per-pair trading constraints needed to place a valid resting bid: the price
/// must land on a `tick_size` multiple, the volume on a `lot_size` multiple, and
/// every order must be at least `ordermin` base units.
#[derive(Debug, Clone)]
pub struct PairSpec {
    pub tick_size: Decimal,
    /// Volume step the exchange rounds order sizes to (e.g. `0.00000001`).
    pub lot_size: Decimal,
    pub ordermin: Decimal,
}

/// One resting sleeve order, as seen in the exchange's open-orders listing.
#[derive(Debug, Clone)]
pub struct OpenSleeveOrder {
    /// Exchange order identifier (Kraken txid / OKX ordId).
    pub txid: String,
    /// Resting limit price.
    pub price: Decimal,
    /// Total ordered base volume.
    pub volume: Decimal,
    /// Base volume filled so far (a partial fill; the order is still resting).
    pub executed_qty: Decimal,
}

/// A sleeve order that has left the book (filled or canceled-after-partial),
/// with the fill it realised. Only orders with a nonzero fill are surfaced. The
/// average fill price is derived by the caller from `executed_value /
/// executed_qty`, so it isn't duplicated here.
#[derive(Debug, Clone)]
pub struct ClosedSleeveFill {
    pub txid: String,
    pub executed_qty: Decimal,
    /// USDC spent.
    pub executed_value: Decimal,
    /// USDC fee.
    pub fee: Decimal,
    /// Unix seconds the order actually closed — the *actual* fill time, so the
    /// record is dated correctly rather than to when the reconcile happened to
    /// observe it.
    pub closetm: i64,
}

/// Normalised snapshot of a resting sleeve order's current state.
///
/// IMPORTANT for reconcile logic: "did this order fill anything?" must be
/// answered by `executed_qty > 0`, independently of `status` — Kraken has no
/// distinct `partial` status (a partially filled order is still `open`), so
/// branching on status alone would silently drop partial fills.
#[derive(Debug, Clone)]
pub struct RestingOrderState {
    /// Raw exchange status string (vocabulary differs per exchange — the
    /// sleeve does not branch on it, only on `executed_qty`).
    pub status: String,
    /// Base asset filled so far (may be partial while still resting).
    pub executed_qty: Decimal,
    /// USDC spent so far.
    pub executed_value: Decimal,
    /// USDC fees charged so far.
    pub fee: Decimal,
}

/// Resting-order primitives the limit-order sleeve needs. Implemented by
/// Kraken and OKX; the sleeve holds an `Arc<dyn SleeveExchange>` so it works
/// against whichever backend `EXCHANGE` selects.
#[async_trait]
pub trait SleeveExchange: Send + Sync {
    /// Build a volume-profile bid ladder for `symbol` from live market data.
    /// Returns the ladder **and the spot price it was derived against** — the
    /// sleeve must compare bids against this same spot rather than re-reading
    /// price, since a second read could straddle an HVN center.
    async fn build_bid_ladder(
        &self,
        symbol: &str,
        interval_minutes: u32,
        config: &VolumeProfileConfig,
    ) -> Result<(BidLadder, Decimal)>;

    /// Per-pair tick/lot/minimum-order constraints.
    async fn fetch_pair_spec(&self, symbol: &str) -> Result<PairSpec>;

    /// The sleeve's currently-resting orders (those tagged with `userref`).
    async fn get_open_sleeve_orders(&self, userref: i32) -> Result<Vec<OpenSleeveOrder>>;

    /// The sleeve's orders that left the book with a nonzero fill, scanned from
    /// `start` (unix seconds, `None` for "from the beginning") — the crash-safe
    /// source of truth for realized fills.
    async fn get_closed_sleeve_fills(
        &self,
        userref: i32,
        start: Option<i64>,
    ) -> Result<Vec<ClosedSleeveFill>>;

    /// Post a fire-and-forget, post-only limit **buy** of `volume` base asset
    /// at `price` for `symbol`, tagged with `userref`, returning the order id.
    /// `price`/`volume` must already be tick/lot-rounded by the caller (see
    /// [`Self::fetch_pair_spec`]).
    async fn place_resting_limit_buy(
        &self,
        symbol: &str,
        price: Decimal,
        volume: Decimal,
        userref: i32,
    ) -> Result<String>;

    /// Cancel a resting order, tolerating the "already closed/unknown" races.
    async fn cancel_resting_order(&self, symbol: &str, id: &str);

    /// Current state of a resting order, or `None` if the exchange doesn't
    /// know the id.
    async fn query_resting_order(&self, symbol: &str, id: &str)
    -> Result<Option<RestingOrderState>>;

    /// How many USDC one EUR is worth (mirrors [`Exchange::get_usdc_per_eur`];
    /// duplicated here so the sleeve can work from a `dyn SleeveExchange` alone
    /// without also needing `dyn Exchange`).
    async fn get_usdc_per_eur(&self) -> Result<Decimal>;
}
