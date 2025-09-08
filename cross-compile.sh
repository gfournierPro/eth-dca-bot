#!/bin/bash
# Cross-compile using Docker (easier setup)

echo "🐳 Cross-compiling using Docker..."

# Build using cross-rs (much easier)
if ! command -v cross &> /dev/null; then
    echo "📦 Installing cross..."
    cargo install cross --git https://github.com/cross-rs/cross
fi

# Build for Android using cross
echo "🔨 Building for Android using cross..."
cross build --target aarch64-linux-android --release

if [ $? -eq 0 ]; then
    echo "✅ Cross-compilation successful!"
    echo "📱 Binary: target/aarch64-linux-android/release/eth-dca-bot"
    echo ""
    echo "📋 Transfer to phone:"
    echo "1. Using ADB:"
    echo "   adb push target/aarch64-linux-android/release/eth-dca-bot /data/data/com.termux/files/home/"
    echo ""
    echo "2. Using file sharing (easier):"
    echo "   - Upload binary to Google Drive/Dropbox"
    echo "   - Download in Termux: wget <download-link>"
    echo "   - Make executable: chmod +x eth-dca-bot"
    echo ""
    echo "📦 Binary info:"
    ls -lh target/aarch64-linux-android/release/eth-dca-bot
    file target/aarch64-linux-android/release/eth-dca-bot
else
    echo "❌ Cross-compilation failed!"
fi
