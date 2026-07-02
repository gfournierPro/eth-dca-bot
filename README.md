# ETH DCA Bot 🤖⚡

A sophisticated Ethereum Dollar-Cost Averaging (DCA) bot built in Rust that automatically purchases ETH on Binance or Kraken using a scheduled strategy. The bot includes advanced features like automated withdrawals to cold storage, comprehensive tracking with MongoDB, and optional Notion integration for portfolio management.

## 🌟 Features

- **Pluggable Exchange Backend**: Trade on Binance or Kraken, selected with a single `EXCHANGE` env var — switch between them without code changes (see [Choosing an Exchange](#choosing-an-exchange))
- **Automated DCA Trading**: Schedule regular ETH purchases using EUR amounts (automatically converted to USDC)
- **Multi-Asset Support**: Optionally run a BTC DCA workflow alongside ETH in the same process, with independent schedules, stats, and withdrawals (see [BTC DCA Workflow](#btc-dca-workflow-optional))
- **Smart Withdrawal System**: Automatically withdraw ETH/BTC to cold storage when thresholds are met
- **MongoDB Integration**: Track all purchases, statistics, and performance metrics
- **Notion Integration**: Optional integration with Notion for portfolio tracking and management
- **Configurable Scheduling**: Flexible cron-based scheduling for DCA purchases
- **Safety Checks**: Multiple validation layers and minimum balance protection
- **Comprehensive Logging**: Detailed tracing and error handling
- **Docker Support**: Easy deployment with MongoDB using Docker Compose

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
docker compose up -d
```

### 4. Build and Run

```bash
cargo build --release
cargo run
```

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

The project includes Docker Compose configuration for MongoDB:

```bash
# Start services
docker compose up -d

# View logs
docker compose logs -f

# Stop services
docker compose down
```

## 🔧 Development

### Build

```bash
cargo build
```

### Run Tests

```bash
cargo test
```

### Code Formatting

```bash
cargo fmt
```

### Linting

```bash
cargo clippy
```

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
