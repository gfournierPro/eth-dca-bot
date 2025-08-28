# ETH DCA Bot 🤖⚡

A sophisticated Ethereum Dollar-Cost Averaging (DCA) bot built in Rust that automatically purchases ETH on Binance using a scheduled strategy. The bot includes advanced features like automated withdrawals to cold storage, comprehensive tracking with MongoDB, and optional Notion integration for portfolio management.

## 🌟 Features

- **Automated DCA Trading**: Schedule regular ETH purchases using EUR amounts (automatically converted to USDC) on Binance
- **Smart Withdrawal System**: Automatically withdraw ETH to cold storage when thresholds are met
- **MongoDB Integration**: Track all purchases, statistics, and performance metrics
- **Notion Integration**: Optional integration with Notion for portfolio tracking and management
- **Configurable Scheduling**: Flexible cron-based scheduling for DCA purchases
- **Safety Checks**: Multiple validation layers and minimum balance protection
- **Comprehensive Logging**: Detailed tracing and error handling
- **Docker Support**: Easy deployment with MongoDB using Docker Compose

## 🏗️ Architecture

```
src/
├── main.rs              # Application entry point and scheduling
├── binance.rs           # Binance API client and trading operations
├── dca.rs              # Core DCA logic and trade execution
├── config.rs           # Configuration structures and defaults
├── dca_stats_mongo.rs  # MongoDB integration for statistics
├── notion_integration.rs # Notion API integration
└── date_utils.rs       # Date/time utilities for withdrawals
```

## 🚀 Quick Start

### Prerequisites

- Rust (latest stable version)
- Docker and Docker Compose
- Binance account with API access
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
# Binance API Configuration
BINANCE_API_KEY=your_binance_api_key
BINANCE_SECRET_KEY=your_binance_secret_key

# Trading Configuration
DCA_AMOUNT_EUR=50.0              # Amount in EUR to purchase ETH with (converted to USDC)
MIN_BALANCE_USDC=10.0            # Minimum USDC balance to maintain (safety buffer)
SCHEDULE_CRON=0 0 12 * * * *     # Daily at noon

# Withdrawal Configuration
WITHDRAWAL_ENABLED=true
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
