#!/bin/bash

# Replace DCA Purchase Utility
# This script helps you remove a DCA purchase from MongoDB/Notion and replace it with another one

echo "🔄 DCA Purchase Replacement Utility"
echo "=================================="
echo ""

# Check if the required environment variables are set
if [ -z "$BINANCE_API_KEY" ] || [ -z "$BINANCE_SECRET_KEY" ]; then
    echo "❌ Error: BINANCE_API_KEY and BINANCE_SECRET_KEY environment variables must be set"
    echo ""
    echo "Please set them by running:"
    echo "export BINANCE_API_KEY='your_api_key'"
    echo "export BINANCE_SECRET_KEY='your_secret_key'"
    echo ""
    exit 1
fi

# Check if MongoDB is accessible
if [ -z "$MONGODB_URL" ]; then
    echo "⚠️  Warning: MONGODB_URL not set, using default: mongodb://dca_user:dca_password@localhost:27017/dca_bot"
fi

echo "🎯 Task: Remove order 6863767683 and replace with order 6863022335"
echo ""

# Run the replacement utility
cargo run --bin replace_purchase 6863767683 6863022335

echo ""
echo "✅ Replacement process completed!"
echo ""
echo "📊 To verify the changes, you can:"
echo "   1. Check your MongoDB database for the updated records"
echo "   2. Check your Notion database for the updated entries"
echo "   3. Run the main DCA bot to see the updated summary"
