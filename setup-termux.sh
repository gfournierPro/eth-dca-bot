#!/bin/bash
# Setup script for running ETH DCA Bot in Termux

echo "🚀 Setting up ETH DCA Bot for Termux..."

# Update packages
echo "📦 Updating packages..."
pkg update && pkg upgrade -y

# Install required packages
echo "🔧 Installing required packages..."
pkg install -y rust git mongodb openssl openssl-tool libcrypt pkg-config clang

# Set up OpenSSL environment variables for Rust compilation
echo "⚙️ Setting up OpenSSL environment..."
export OPENSSL_DIR=$PREFIX
export OPENSSL_LIB_DIR=$PREFIX/lib
export OPENSSL_INCLUDE_DIR=$PREFIX/include
export PKG_CONFIG_PATH=$PREFIX/lib/pkgconfig

# Add to shell profile for persistence
echo "export OPENSSL_DIR=\$PREFIX" >> ~/.bashrc
echo "export OPENSSL_LIB_DIR=\$PREFIX/lib" >> ~/.bashrc
echo "export OPENSSL_INCLUDE_DIR=\$PREFIX/include" >> ~/.bashrc
echo "export PKG_CONFIG_PATH=\$PREFIX/lib/pkgconfig" >> ~/.bashrc

# Create MongoDB directory
echo "📁 Creating MongoDB data directory..."
mkdir -p ~/mongodb/data
mkdir -p ~/mongodb/logs

# Create MongoDB startup script with compatible options
echo "🚀 Creating MongoDB startup script..."
cat > ~/start-mongodb.sh << 'EOF'
#!/bin/bash
echo "🍃 Starting MongoDB..."

# Ensure directories exist
mkdir -p ~/mongodb/data
mkdir -p ~/mongodb/logs

# Check if MongoDB is already running
if pgrep mongod > /dev/null; then
    echo "⚠️  MongoDB is already running (PID: $(pgrep mongod))"
    echo "Stop it first with: pkill mongod"
    exit 1
fi

# Start MongoDB with compatible options
echo "Starting MongoDB server..."
mongod \
  --dbpath ~/mongodb/data \
  --logpath ~/mongodb/logs/mongod.log \
  --logappend \
  --port 27017 \
  --bind_ip 127.0.0.1 \
  --fork \
  --pidfilepath ~/mongodb/mongod.pid

# Check if MongoDB started successfully
sleep 2
if pgrep mongod > /dev/null; then
    echo "✅ MongoDB started successfully (PID: $(pgrep mongod))"
    echo "📝 Logs: tail -f ~/mongodb/logs/mongod.log"
    echo "🔍 Status: pgrep mongod"
    echo "🛑 Stop: pkill mongod"
else
    echo "❌ Failed to start MongoDB. Check logs:"
    echo "   tail ~/mongodb/logs/mongod.log"
fi
EOF

chmod +x ~/start-mongodb.sh

# Create MongoDB stop script
echo "🛑 Creating MongoDB stop script..."
cat > ~/stop-mongodb.sh << 'EOF'
#!/bin/bash
echo "🛑 Stopping MongoDB..."

if pgrep mongod > /dev/null; then
    pkill mongod
    sleep 2
    if ! pgrep mongod > /dev/null; then
        echo "✅ MongoDB stopped successfully"
        rm -f ~/mongodb/mongod.pid
    else
        echo "⚠️  Force killing MongoDB..."
        pkill -9 mongod
        rm -f ~/mongodb/mongod.pid
        echo "✅ MongoDB force stopped"
    fi
else
    echo "ℹ️  MongoDB is not running"
fi
EOF

chmod +x ~/stop-mongodb.sh

# Create MongoDB status script
echo "📊 Creating MongoDB status script..."
cat > ~/mongodb-status.sh << 'EOF'
#!/bin/bash
echo "📊 MongoDB Status:"
echo "=================="

if pgrep mongod > /dev/null; then
    echo "✅ Status: RUNNING (PID: $(pgrep mongod))"
    echo "🌐 Port: 27017"
    echo "📁 Data: ~/mongodb/data"
    echo "📝 Logs: ~/mongodb/logs/mongod.log"
    echo ""
    echo "📊 Process info:"
    ps aux | grep mongod | grep -v grep
    echo ""
    echo "🔍 Connection test:"
    if command -v mongosh >/dev/null 2>&1; then
        timeout 5 mongosh --eval "db.adminCommand('ping')" --quiet 2>/dev/null && echo "✅ Connection: OK" || echo "❌ Connection: FAILED"
    else
        echo "⚠️  mongosh not found - install with: pkg install mongodb"
    fi
else
    echo "❌ Status: NOT RUNNING"
fi

echo ""
echo "🔧 Management commands:"
echo "  Start:  ~/start-mongodb.sh"
echo "  Stop:   ~/stop-mongodb.sh"
echo "  Status: ~/mongodb-status.sh"
echo "  Logs:   tail -f ~/mongodb/logs/mongod.log"
EOF

chmod +x ~/mongodb-status.sh

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
echo "📋 Next steps to run your ETH DCA Bot:"
echo "======================================="
echo ""
echo "1. 🚀 Start MongoDB:"
echo "   ~/start-mongodb.sh"
echo ""
echo "2. 🔧 Initialize database (in another terminal):"
echo "   mongosh < ~/init-dca-db.js"
echo ""
echo "3. ⚙️  Configure environment:"
echo "   cp .env.termux .env"
echo "   nano .env  # Edit with your real API keys"
echo ""
echo "4. 🤖 Download and run the binary:"
echo "   # Method A: Download from GitHub releases"
echo "   # Method B: Copy from /sdcard/Download/eth-dca-bot-android"
echo "   chmod +x ~/eth-dca-bot"
echo "   ./eth-dca-bot"
echo ""
echo "�️  MongoDB management commands:"
echo "  Status: ~/mongodb-status.sh"
echo "  Start:  ~/start-mongodb.sh"
echo "  Stop:   ~/stop-mongodb.sh"
echo "  Logs:   tail -f ~/mongodb/logs/mongod.log"
echo ""
echo "🔍 Troubleshooting:"
echo "  Check MongoDB: ~/mongodb-status.sh"
echo "  Test connection: mongosh --eval 'db.adminCommand(\"ping\")'"
echo "  Kill all MongoDB: pkill -9 mongod"
