#!/bin/bash
# Script to download the latest Android binary for Termux

echo "📱 Downloading latest ETH DCA Bot Android binary..."

# GitHub repository details
REPO="gfournieriExec/eth-dca-bot"
BINARY_NAME="eth-dca-bot-android"

# Get the latest release download URL
echo "🔍 Finding latest release..."
LATEST_RELEASE=$(curl -s "https://api.github.com/repos/$REPO/releases/latest")

if echo "$LATEST_RELEASE" | grep -q "Not Found"; then
    echo "❌ No releases found. Trying to download from repository..."
    
    # Fallback: download directly from repository
    echo "📥 Downloading from main branch..."
    curl -L "https://github.com/$REPO/raw/main/binaries/android/eth-dca-bot-android" -o ~/eth-dca-bot
    
    if [ $? -eq 0 ] && [ -f ~/eth-dca-bot ]; then
        echo "✅ Downloaded binary to ~/eth-dca-bot"
        chmod +x ~/eth-dca-bot
        echo "🔧 Made binary executable"
        
        # Check if it's actually a binary (not HTML error page)
        if file ~/eth-dca-bot | grep -q "ELF"; then
            echo "✅ Binary verification: OK"
            echo "🚀 Ready to run: ./eth-dca-bot"
        else
            echo "❌ Downloaded file doesn't appear to be a binary"
            echo "📝 Check manually: file ~/eth-dca-bot"
        fi
    else
        echo "❌ Failed to download binary"
        echo "💡 Manual steps:"
        echo "  1. Go to: https://github.com/$REPO/tree/main/binaries/android"
        echo "  2. Download eth-dca-bot-android"
        echo "  3. Copy to Termux: cp /sdcard/Download/eth-dca-bot-android ~/eth-dca-bot"
        echo "  4. Make executable: chmod +x ~/eth-dca-bot"
    fi
else
    # Try to extract download URL from release assets
    DOWNLOAD_URL=$(echo "$LATEST_RELEASE" | grep -o "https://github.com/$REPO/releases/download/[^\"]*eth-dca-bot[^\"]*" | head -1)
    
    if [ -n "$DOWNLOAD_URL" ]; then
        echo "📥 Downloading from: $DOWNLOAD_URL"
        curl -L "$DOWNLOAD_URL" -o ~/eth-dca-bot
        chmod +x ~/eth-dca-bot
        echo "✅ Downloaded and made executable: ~/eth-dca-bot"
    else
        echo "❌ Could not find binary in latest release"
        echo "💡 Try manual download from GitHub"
    fi
fi

echo ""
echo "📋 Next steps:"
echo "1. Start MongoDB: ~/start-mongodb.sh"
echo "2. Configure .env: cp .env.termux .env && nano .env"
echo "3. Run bot: ./eth-dca-bot"
