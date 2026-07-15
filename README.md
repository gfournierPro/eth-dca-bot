# ETH DCA Bot 🤖⚡

A sophisticated Ethereum Dollar-Cost Averaging (DCA) bot built in Rust that automatically purchases ETH on Binance or Kraken using a scheduled strategy. The bot includes advanced features like automated withdrawals to cold storage, comprehensive tracking with MongoDB, and optional Notion integration for portfolio management.

## 🌟 Features

- **Pluggable Exchange Backend**: Trade on Binance or Kraken, selected with a single `EXCHANGE` env var — switch between them without code changes (see [Choosing an Exchange](#choosing-an-exchange))
- **Automated DCA Trading**: Schedule regular ETH purchases using EUR amounts (automatically converted to USDC)
- **Dynamic DCA Sizing**: Smart purchase amount adjustments based on market conditions (volatility, RSI, moving averages, momentum)
- **Multi-Asset Support**: Optionally run a BTC DCA workflow alongside ETH in the same process, with independent schedules, stats, and withdrawals (see [BTC DCA Workflow](#btc-dca-workflow-optional))
- **Limit-Order Sleeve** *(Kraken only, not yet live-validated)*: Optional volume-profile resting-bid sleeve layered on top of DCA (see [Limit-Order Sleeve](#limit-order-sleeve-optional-kraken-only))
- **Smart Withdrawal System**: Automatically withdraw ETH/BTC to cold storage when thresholds are met
- **MongoDB Integration**: Track all purchases, statistics, and performance metrics
- **Notion Integration**: Optional integration with Notion for portfolio tracking and management
- **Configurable Scheduling**: Flexible cron-based scheduling for DCA purchases
- **Safety Checks**: Multiple validation layers and minimum balance protection
- **Comprehensive Logging**: Detailed tracing and error handling
- **Docker Support**: Easy deployment with MongoDB using Docker Compose

## 🧠 Dynamic DCA Sizing

The bot now includes advanced market-based DCA amount adjustments:

### Market Indicators
- **Volatility-based Scaling**: Increases purchase amounts by up to 10% during high volatility periods (2+ standard deviations)
- **RSI-based Adjustments**: Buys 7% more when RSI < 30 (oversold conditions)
- **Price Deviation Strategy**: Increases amounts by 5% when price is >5% below 20-day moving average
- **Momentum-based Adjustments**: Buys 8% more during negative momentum periods (-5% over 7 days)

### Safety Features
- **Maximum Multiplier**: Caps total multiplier at 1.3x to limit increases to +30% maximum
- **Minimum Multiplier**: Ensures at least 0.8x purchase occurs (minimum -20% reduction)
- **Individual Controls**: Each indicator can be enabled/disabled independently
- **Configurable Thresholds**: All parameters are customizable via configuration

## 🏗️ Architecture

```
src/
├── main.rs              # Application entry point, scheduling, exchange factory
├── exchange.rs          # Exchange trait + unified order/withdrawal types
├── binance.rs           # Binance API client (implements Exchange)
├── kraken.rs            # Kraken API client (implements Exchange)
├── dca.rs              # Core DCA logic and trade execution
├── config.rs           # Configuration structures and defaults
├── dca_stats_mongo.rs  # MongoDB integration for statistics
├── notion_integration.rs # Notion API integration
└── date_utils.rs       # Date/time utilities for withdrawals
```

## Choosing an Exchange

Set `EXCHANGE=binance` or `EXCHANGE=kraken` (defaults to `binance` if unset). Only
the selected exchange's credentials are required. Both integrations trade the same
USDC-quoted pairs (`ETHUSDC`, `BTCUSDC`) and size buys from an EUR amount.

Key differences to be aware of on Kraken:

- **Credentials**: `KRAKEN_API_KEY` / `KRAKEN_SECRET_KEY` (the secret is the base64
  "Private Key" shown when the API key is created).
- **Withdrawals**: Kraken cannot withdraw to an arbitrary address via API. Register
  your cold wallet as a **withdrawal key** in the Kraken UI (Funding → Withdraw) and
  set `WITHDRAWAL_WALLET_ADDRESS` to that key's name. `WITHDRAWAL_NETWORK` is
  informational on Kraken (the network is implied by the key).
- **BTC**: referred to as `XBT` internally by Kraken; the bot handles the mapping, so
  keep using `BTCUSDC` in config.

To switch back to Binance later, set `EXCHANGE=binance` and provide the Binance
credentials — no rebuild needed beyond restarting with the new env.

## 🚀 Quick Start

### Prerequisites

- Rust (latest stable version)
- Docker and Docker Compose
- A Binance or Kraken account with API access
- MongoDB (provided via Docker Compose)
- (Optional) Notion integration token and database

### 1. Clone and Setup

```bash
git clone <your-repo-url>
cd eth-dca-bot
```

### 2. Environment Configuration

Create a `.env` file in the project root:

```env
# Exchange selection: "binance" or "kraken"
EXCHANGE=kraken

# Binance API Configuration (used when EXCHANGE=binance)
BINANCE_API_KEY=your_binance_api_key
BINANCE_SECRET_KEY=your_binance_secret_key

# Kraken API Configuration (used when EXCHANGE=kraken)
KRAKEN_API_KEY=your_kraken_api_key
KRAKEN_SECRET_KEY=your_kraken_private_key

# Trading Configuration
DCA_AMOUNT_EUR=50.0              # Amount in EUR to purchase ETH with (converted to USDC)
MIN_BALANCE_USDC=10.0            # Minimum USDC balance to maintain (safety buffer)
SCHEDULE_CRON=0 0 12 * * * *     # Daily at noon

# Withdrawal Configuration
WITHDRAWAL_ENABLED=true
# Binance: on-chain address. Kraken: name of a pre-registered withdrawal key.
WITHDRAWAL_WALLET_ADDRESS=0x1234567890123456789012345678901234567890
WITHDRAWAL_NETWORK=ETH
WITHDRAWAL_MIN_ETH_THRESHOLD=0.1
# WITHDRAWAL_AMOUNT=0.05  # Optional: specific amount, otherwise withdraws all available

# Notion Integration (Optional)
NOTION_TOKEN=secret_your_notion_integration_token
NOTION_DATABASE_ID=your_notion_database_id
COLD_WALLET_ADDRESS=0x1234567890123456789012345678901234567890
```

### 3. Start MongoDB

```bash
make mongo-up
```

### 4. Build and Run

```bash
make build-release
make run
```

See [Makefile Commands](#-makefile-commands) below for the full list of `make` targets (tests, linting, Docker, production stack, maintenance binaries).

## ⚙️ Configuration

### Trading Parameters

- `DCA_AMOUNT_EUR`: Amount in EUR to purchase ETH with each DCA execution (automatically converted to USDC)
- `MIN_BALANCE_USDC`: Minimum USDC balance to maintain (safety buffer)
- `SCHEDULE_CRON`: Cron expression for DCA scheduling

### Withdrawal Settings

- `WITHDRAWAL_ENABLED`: Enable/disable automatic withdrawals
- `WITHDRAWAL_WALLET_ADDRESS`: Target cold wallet address
- `WITHDRAWAL_NETWORK`: Network for withdrawal (e.g., "ETH")
- `WITHDRAWAL_MIN_ETH_THRESHOLD`: Minimum ETH balance to trigger withdrawal
- `WITHDRAWAL_AMOUNT`: Optional fixed withdrawal amount (if not set, withdraws all available ETH)

### Market Indicators Configuration

All market indicators are configurable and can be enabled/disabled independently:

#### Volatility-based Scaling
- `volatility_scaling_enabled`: Enable volatility-based purchase scaling (default: true)
- `volatility_period`: Lookback period in days for volatility calculation (default: 30)
- `high_volatility_multiplier`: Purchase multiplier during high volatility (default: 1.1)
- `volatility_threshold`: Standard deviation threshold for "high" volatility (default: 2.0)

#### RSI-based Adjustments
- `rsi_enabled`: Enable RSI-based purchase adjustments (default: true)
- `rsi_period`: Period for RSI calculation (default: 14)
- `rsi_oversold_threshold`: RSI level considered oversold (default: 30)
- `rsi_oversold_multiplier`: Purchase multiplier when oversold (default: 1.07)

#### Price Deviation Strategy
- `price_deviation_enabled`: Enable moving average deviation strategy (default: true)
- `moving_average_period`: Period for moving average calculation (default: 20)
- `deviation_threshold_percent`: Percentage below MA to trigger increase (default: 5%)
- `below_ma_multiplier`: Purchase multiplier when below MA (default: 1.05)

#### Momentum-based Adjustments
- `momentum_enabled`: Enable momentum-based adjustments (default: true)
- `momentum_period`: Period for momentum calculation (default: 7)
- `negative_momentum_threshold`: Negative momentum threshold (default: -5%)
- `negative_momentum_multiplier`: Purchase multiplier during negative momentum (default: 1.08)

#### Safety Limits
- `max_total_multiplier`: Maximum combined multiplier to prevent excessive purchases (default: 1.3)
- `min_total_multiplier`: Minimum multiplier to ensure some purchase occurs (default: 0.8)

### BTC DCA Workflow (Optional)

The bot can run a **BTC DCA workflow alongside ETH** in the same process. It mirrors
the ETH workflow exactly (scheduled buys, missed-run catch-up, withdrawals, MongoDB
stats, Notion tracking) but is fully independent: BTC buys `BTCUSDC`, stores its
purchases in a **separate MongoDB collection** (`btc_purchases`), and uses its **own
Notion database**, so ETH and BTC statistics never mix.

Enable it by setting `BTC_DCA_ENABLED=true`. All other `BTC_*` variables are optional
and fall back to the defaults below:

- `BTC_DCA_ENABLED`: Set to `true` to activate the BTC workflow (default `false`)
- `BTC_DCA_AMOUNT_EUR`: EUR amount per BTC purchase (default `100`)
- `BTC_MIN_BALANCE_USDC`: Minimum USDC balance to maintain (default `50`)
- `BTC_SCHEDULE_CRON`: Cron expression for BTC purchases (default `0 30 5 * * MON`)
- `BTC_TIMEZONE`: Timezone for the BTC schedule (defaults to the global `TIMEZONE`)
- `BTC_MONGO_COLLECTION`: MongoDB collection for BTC purchases (default `btc_purchases`)
- `BTC_NOTION_TOKEN`: Notion token for BTC (defaults to `NOTION_TOKEN` if unset)
- `BTC_NOTION_DATABASE_ID`: Separate Notion database for BTC tracking
- `BTC_COLD_WALLET_ADDRESS`: BTC cold wallet address (used for Notion + withdrawals)
- `BTC_WITHDRAWAL_ENABLED`: Enable/disable automatic BTC withdrawals (default `false`)
- `BTC_WITHDRAWAL_WALLET_ADDRESS`: Target BTC cold wallet address
- `BTC_WITHDRAWAL_NETWORK`: Withdrawal network for BTC (default `BTC` — native Bitcoin)
- `BTC_WITHDRAWAL_MIN_THRESHOLD`: Minimum BTC balance to trigger a withdrawal (default `0.0001`)
- `BTC_WITHDRAWAL_AMOUNT`: Optional fixed withdrawal amount (otherwise withdraws all available BTC)

> **Note:** The BTC Notion database must use the same property schema as the ETH one
> (`Name`, `From`, `When`, `Currency`, `eur`, `Network Fee`, `Trading Fee`, `Link`).

### Limit-Order Sleeve (Optional, Kraken only)

> ⚠️ **Status: code-complete, not yet validated against a live authenticated
> Kraken account.** The compute path (volume profile, ladder, tick/lot rounding)
> and public-API parsing are validated live; order placement, cancellation, fill
> recording, and the war-chest math have only been logic/unit-tested so far. Run
> the [live smoke-test runbook](docs/limit-sleeve-smoke-test.md) before funding a
> real war chest — see [`docs/limit-sleeve-roadmap.md`](docs/limit-sleeve-roadmap.md)
> for the full picture of what's done vs. open.

On top of scheduled DCA buys, the bot can optionally run a **limit-order sleeve**
that rests post-only bids at volume-profile levels below spot, funded by a fixed
USDC "war chest" that drains as dips fill (never auto-replenished). It's fully
isolated from the DCA core — own Mongo collection, own budget — so DCA stats stay
pure. Kraken only (reuses Kraken's post-only order path); on Binance it's skipped
with a warning if enabled.

Enable it by setting `LIMIT_SLEEVE_ENABLED=true` (and `BTC_LIMIT_SLEEVE_ENABLED=true`
for the BTC sleeve). Key variables — see `.env.example` for the full list with
defaults:

- `LIMIT_SLEEVE_WAR_CHEST_USDC`: fixed USDC budget the sleeve can deploy
- `LIMIT_SLEEVE_REFRESH_CRON`: cron for recomputing levels and reconciling bids (default every 6h)
- `LIMIT_SLEEVE_INTERVAL_MINUTES`: OHLC candle interval used to build the volume profile
- `VP_BUCKET_SIZE_ETH`, `VP_HVN_THRESHOLD_RATIO`, `VP_LADDER_STEPS`, `VP_REQUIRE_LOCAL_MAXIMA`: volume-profile tunables (see `src/levels.rs`)

The BTC sleeve mirrors this with `BTC_LIMIT_SLEEVE_*` / `BTC_VP_*` and its own
Kraken `userref`, so the two sleeves never see or cancel each other's orders.
Use `make sleeve-smoke` to drive the validation runbook.

### Cron Expression Examples

```bash
# Every day at noon
"0 0 12 * * * *"

# Every Monday at 9 AM
"0 0 9 * * 1 *"

# Every hour
"0 0 * * * * *"

# Every 6 hours
"0 0 */6 * * * *"
```

## 💡 Dynamic DCA Example

Here's how the dynamic sizing works in practice:

**Base Setup**: 100 EUR DCA amount
- **Normal conditions**: Buys 100 EUR worth of ETH (multiplier: 1.0)
- **High volatility + RSI oversold**: Buys 117.7 EUR worth (1.1 × 1.07 = 1.177 multiplier)
- **Price 8% below MA + negative momentum**: Buys 113.4 EUR worth (1.05 × 1.08 = 1.134 multiplier)
- **All conditions triggered**: Buys 133.8 EUR worth (1.1 × 1.07 × 1.05 × 1.08 = 1.338 multiplier, capped at 1.3)
- **Maximum increase**: Capped at 130 EUR maximum (1.3x safety limit, +30% from base)
- **Market confidence low**: Minimum 80 EUR (0.8x safety floor, -20% from base)

The bot logs each multiplier calculation:
```
[INFO] Volatility multiplier: 1.1
[INFO] RSI multiplier: 1.07
[INFO] Price deviation multiplier: 1.0
[INFO] Momentum multiplier: 1.08
[INFO] Final DCA multiplier: 1.3 (capped from 1.338)
[INFO] Dynamic DCA multiplier: 1.30 - Adjusted target amount: 130.0 USDC
```

## 📊 MongoDB Schema

The bot stores comprehensive trading data in MongoDB:

```rust
struct DcaPurchase {
    id: String,
    timestamp: DateTime<Utc>,
    symbol: String,
    usdc_amount: Decimal,
    eth_amount: Decimal,
    price: Decimal,
    commission: Decimal,
    commission_asset: String,
    order_id: i64,
    usdc_balance_before: Decimal,
    usdc_balance_after: Decimal,
    eth_balance_before: Decimal,
    eth_balance_after: Decimal,
}
```

## 🔄 Database Sync Feature

When `EXCHANGE=binance`, the bot automatically checks your MongoDB purchase history
against Binance's own order history **on every startup** (`check_and_sync_database`
in `main.rs`) — there is no env var to opt in or out; it always runs for Binance.
It is skipped when `EXCHANGE=kraken`, since Kraken's order ids are opaque string
txids rather than Binance's sequential numeric ids that the integrity check relies on.

### What It Does

- **Fetches Historical Data**: Retrieves all `ETHUSDC` trades (both BUY and SELL orders) from Binance since the bot's first recorded order
- **Compares Records**: Checks existing MongoDB records against Binance history
- **Identifies Missing Orders**: Finds any orders that exist on Binance but not in your database
- **Syncs Missing Data**: Adds missing orders (purchases and sales) to your MongoDB database
- **Provides Detailed Reports**: Logs exactly what was synced, with order type information

### Safety Features

- **No Duplicates**: Only adds orders that don't already exist
- **Read-Only**: Uses only read operations on Binance API
- **Data Validation**: Verifies integrity before and after sync
- **Comprehensive Logging**: Detailed logs of all operations

## 🔗 Notion Integration

The bot can optionally integrate with Notion to track your DCA strategy:

1. Create a Notion integration at https://developers.notion.com/
2. Create a database in Notion for tracking DCA purchases
3. Add the integration token and database ID to your `.env` file

The integration will automatically create entries for each DCA purchase with comprehensive metadata.

## 🛡️ Security Features

- **API Key Management**: Secure environment variable configuration
- **Balance Validation**: Prevents trades below minimum balance thresholds
- **Error Handling**: Comprehensive error handling and recovery
- **Withdrawal Safety**: Multiple validation checks before executing withdrawals
- **Rate Limiting**: Respects Binance API rate limits

## 📈 Monitoring and Logging

The bot provides detailed logging including:

- DCA purchase execution details
- Balance changes and portfolio updates
- Withdrawal operations and confirmations
- Error tracking and recovery attempts
- Performance metrics and statistics

View logs in real-time:
```bash
tail -f dca_bot.log
```

## 🐳 Docker Deployment

`make docker-build` builds the production image (multi-stage, Rust builder →
`debian:bookworm-slim` runtime). For the full stack (bot + MongoDB), see
[Makefile Commands](#-makefile-commands) → Production stack below.

## 🔧 Makefile Commands

Run `make help` at any time for the full, authoritative list. Quick reference:

### Local development (cargo)

| Command | What it does |
|---|---|
| `make setup` | Copy `.env.example` → `.env` if `.env` doesn't exist yet |
| `make build` / `make build-release` | Compile the bot (debug / release) |
| `make run` | Run the bot locally (needs `make mongo-up` first) |
| `make test` | Run the unit test suite |
| `make fmt` / `make fmt-check` | Format the codebase / check formatting (CI-safe) |
| `make clippy` | Lint with clippy (all targets) |
| `make check` | Full verification: `fmt-check` + `clippy` + `test` |

### MongoDB (local dev)

| Command | What it does |
|---|---|
| `make mongo-up` | Start local MongoDB via `docker-compose.yml` |
| `make mongo-down` | Stop it |
| `make mongo-logs` | Tail MongoDB logs |
| `make mongo-shell` | Open a `mongosh` shell into the dev database |

### Docker image / production stack

| Command | What it does |
|---|---|
| `make docker-build` | Build the production image (`eth-dca-bot`) |
| `make docker-run` | Run the built image standalone with `--env-file .env` |
| `make prod-up` | Build + start the full stack (bot + Mongo) from `docker-compose.prod.yml`, reading `.env.prod` |
| `make prod-down` | Stop the production stack |
| `make prod-logs` | Tail production logs |
| `make prod-restart` | Restart just the bot container (e.g. after an env change) |

For production, copy `.env.prod.example` → `.env.prod` and fill in real values
(`MONGO_PASSWORD`, exchange keys, withdrawal address, etc.) before `make prod-up`.

### Maintenance / diagnostic binaries

These are standalone tools in `src/bin/`, separate from the main scheduler:

| Command | What it does |
|---|---|
| `make sync-notion` | One-off check/sync of the latest DCA purchase against Notion |
| `make test-kraken` | Manual Kraken smoke test — read-only by default; `ARGS="--buy 5"` or `ARGS="--limit 5"` places a small real order |
| `make replace-purchase OLD=<id> NEW=<id>` | Remove one recorded purchase and replace it with another, by order id |
| `make sleeve-smoke ARGS="reconcile --chest 1.0 --collection limit_sleeve_smoke"` | Limit-sleeve live-validation harness — see [`docs/limit-sleeve-smoke-test.md`](docs/limit-sleeve-smoke-test.md) before touching a real war chest |

### Android cross-compilation

`make android-build` cross-compiles for `aarch64-linux-android` using
[`cross`](https://github.com/cross-rs/cross) (`cargo install cross --git https://github.com/cross-rs/cross`),
matching what `.github/workflows/cross-compile.yml` runs in CI. See `setup-termux.sh`
for running the resulting binary under Termux on-device.

### Housekeeping

`make clean` removes build artifacts (`cargo clean`).

## 📝 Dependencies

Key dependencies include:

- **tokio**: Async runtime for Rust
- **reqwest**: HTTP client for API calls
- **mongodb**: MongoDB driver
- **chrono**: Date and time handling
- **rust_decimal**: Precise decimal arithmetic
- **tokio-cron-scheduler**: Cron-based job scheduling
- **tracing**: Structured logging
- **notion-client**: Notion API integration
- **sqlx**: SQL toolkit (for potential SQLite support)

## ⚠️ Disclaimer

This bot is for educational and personal use. Cryptocurrency trading involves significant risk. Always:

- Test thoroughly with small amounts first
- Understand the risks involved
- Keep your API keys secure
- Monitor the bot's operations regularly
- Use at your own risk

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Submit a pull request

## 📄 License

This project is open source. Please check the license file for details.

## 🆘 Support

If you encounter issues:

1. Check the logs for error details
2. Verify your environment configuration
3. Ensure MongoDB is running
4. Validate your Binance API permissions
5. Create an issue with detailed error information

---

**Happy DCA-ing! 🚀📈**
