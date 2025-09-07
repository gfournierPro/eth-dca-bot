#!/bin/bash
# Setup script for running ETH DCA Bot in Termux

echo "🚀 Setting up ETH DCA Bot for Termux..."

# Update packages
echo "📦 Updating packages..."
pkg update && pkg upgrade -y

# Install required packages
echo "🔧 Installing required packages..."
pkg install -y rust git mongodb openssl-dev pkg-config

# Create MongoDB directory
echo "📁 Creating MongoDB data directory..."
mkdir -p ~/mongodb/data
mkdir -p ~/mongodb/logs

# Create MongoDB startup script with direct command-line options
echo "🚀 Creating MongoDB startup script..."
cat > ~/start-mongodb.sh << 'EOF'
#!/bin/bash
echo "🍃 Starting MongoDB..."
mongod \
  --dbpath ~/mongodb/data \
  --logpath ~/mongodb/logs/mongod.log \
  --logappend \
  --port 27017 \
  --bind_ip 127.0.0.1 \
  --nojournal \
  --smallfiles \
  --storageEngine mmapv1
EOF

chmod +x ~/start-mongodb.sh

# Create MongoDB initialization script
echo "🔧 Creating MongoDB initialization script..."
cat > ~/init-dca-db.js << 'EOF'
// Switch to dca_bot database
use dca_bot;

// Create user
db.createUser({
  user: 'dca_user',
  pwd: 'dca_password',
  roles: [
    {
      role: 'readWrite',
      db: 'dca_bot',
    },
  ],
});

// Create collection with indexes
db.createCollection('dca_purchases');
db.dca_purchases.createIndex({ "timestamp": -1 });
db.dca_purchases.createIndex({ "symbol": 1 });
db.dca_purchases.createIndex({ "order_id": 1 }, { unique: true });

print("✅ Database and user created successfully!");
EOF

# Create environment template
echo "📝 Creating environment template..."
cat > .env.termux << 'EOF'
# Binance API Configuration
BINANCE_API_KEY=your_binance_api_key
BINANCE_SECRET_KEY=your_binance_secret_key

# Trading Configuration
DCA_AMOUNT_EUR=50.0
MIN_BALANCE_USDC=10.0
SCHEDULE_CRON=0 0 12 * * * *

# MongoDB Configuration (local)
MONGODB_URL=mongodb://dca_user:dca_password@localhost:27017/dca_bot

# Withdrawal Configuration
WITHDRAWAL_ENABLED=true
WITHDRAWAL_WALLET_ADDRESS=0x1234567890123456789012345678901234567890
WITHDRAWAL_NETWORK=ETH
WITHDRAWAL_MIN_ETH_THRESHOLD=0.1

# Notion Integration (Optional)
NOTION_TOKEN=secret_your_notion_integration_token
NOTION_DATABASE_ID=your_notion_database_id
COLD_WALLET_ADDRESS=0x1234567890123456789012345678901234567890
EOF

echo "✅ Setup complete!"
echo ""
echo "📋 Next steps:"
echo "1. Start MongoDB: ~/start-mongodb.sh"
echo "2. In another terminal, initialize database: mongosh < ~/init-dca-db.js"
echo "3. Configure your .env file: cp .env.termux .env"
echo "4. Edit .env with your actual API keys"
echo "5. Build and run: cargo build --release && cargo run"
echo ""
echo "🔗 Useful commands:"
echo "  Check MongoDB status: pgrep mongod"
echo "  Stop MongoDB: pkill mongod"
echo "  MongoDB logs: tail -f ~/mongodb/logs/mongod.log"
