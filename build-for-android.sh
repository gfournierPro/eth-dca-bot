#!/bin/bash
# Cross-compile ETH DCA Bot for Android/Termux

echo "🚀 Cross-compiling ETH DCA Bot for Android..."

# Check if Android target is installed
if ! rustup target list --installed | grep -q "aarch64-linux-android"; then
    echo "📦 Installing Android target..."
    rustup target add aarch64-linux-android
fi

# Check if Android NDK is available
if ! command -v aarch64-linux-android21-clang &> /dev/null; then
    echo "❌ Android NDK not found!"
    echo "💡 Install with: brew install android-ndk"
    echo "   Or download from: https://developer.android.com/ndk/downloads"
    exit 1
fi

# Build for Android
echo "🔨 Building for Android (aarch64-linux-android)..."
cargo build --target aarch64-linux-android --release

if [ $? -eq 0 ]; then
    echo "✅ Build successful!"
    echo "📱 Binary location: target/aarch64-linux-android/release/eth-dca-bot"
    echo ""
    echo "📋 Next steps:"
    echo "1. Transfer binary to your phone:"
    echo "   adb push target/aarch64-linux-android/release/eth-dca-bot /data/data/com.termux/files/home/"
    echo ""
    echo "2. Or use any file transfer method (email, cloud storage, etc.)"
    echo ""
    echo "3. On your phone in Termux:"
    echo "   chmod +x ~/eth-dca-bot"
    echo "   ./eth-dca-bot"
    echo ""
    echo "📦 Binary size:"
    ls -lh target/aarch64-linux-android/release/eth-dca-bot
else
    echo "❌ Build failed!"
    echo "💡 Try fixing dependencies first or use a simpler version"
fi
