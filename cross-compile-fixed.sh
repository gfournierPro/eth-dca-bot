#!/bin/bash
# Cross-compile with OpenSSL-free dependencies

echo "🔧 Preparing OpenSSL-free cross-compilation..."

# Backup current Cargo.toml
cp Cargo.toml Cargo.toml.original

# Use the cross-compilation friendly version
echo "📝 Using OpenSSL-free Cargo.toml..."
cp Cargo.toml.cross Cargo.toml

# Check if cross is installed
if ! command -v cross &> /dev/null; then
    echo "📦 Installing cross..."
    cargo install cross --git https://github.com/cross-rs/cross
fi

# Clean previous builds
echo "🧹 Cleaning previous builds..."
cargo clean

# Build for Android using cross
echo "🔨 Cross-compiling for Android (without OpenSSL)..."
cross build --target aarch64-linux-android --release

if [ $? -eq 0 ]; then
    echo "✅ Cross-compilation successful!"
    echo "📱 Binary: target/aarch64-linux-android/release/eth-dca-bot"
    echo ""
    echo "⚠️  Note: This version uses SQLite instead of MongoDB"
    echo "   You'll need to adapt the database code (I can help with this)"
    echo ""
    echo "📋 Transfer to phone:"
    echo "1. Upload to cloud storage and download in Termux"
    echo "2. Or use ADB: adb push target/aarch64-linux-android/release/eth-dca-bot /data/data/com.termux/files/home/"
    echo ""
    echo "📦 Binary info:"
    ls -lh target/aarch64-linux-android/release/eth-dca-bot
    file target/aarch64-linux-android/release/eth-dca-bot
    
    # Restore original Cargo.toml
    echo "🔄 Restoring original Cargo.toml..."
    cp Cargo.toml.original Cargo.toml
else
    echo "❌ Cross-compilation failed!"
    echo "🔄 Restoring original Cargo.toml..."
    cp Cargo.toml.original Cargo.toml
    
    echo ""
    echo "🔍 Try this alternative approach:"
    echo "1. Comment out problematic dependencies in Cargo.toml"
    echo "2. Build core functionality first"
    echo "3. Add database functionality later"
fi
