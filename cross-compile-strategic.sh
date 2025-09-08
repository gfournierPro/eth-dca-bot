#!/bin/bash
# Strategic cross-compilation: Start simple, then add complexity

echo "🎯 Strategic cross-compilation approach..."

# Clean everything first
echo "🧹 Starting fresh..."
cargo clean
rm -f Cargo.lock
docker ps --filter ancestor=ghcr.io/cross-rs/aarch64-linux-android:main -q | xargs -r docker stop 2>/dev/null

# Backup original and use simplified version
cp Cargo.toml Cargo.toml.full
cp Cargo.toml.android Cargo.toml

echo "📝 Using simplified dependencies (no MongoDB/OpenSSL/heavy deps)"
echo "   This should compile much faster..."

# Check cross installation
if ! command -v cross &> /dev/null; then
    echo "📦 Installing cross..."
    cargo install cross --git https://github.com/cross-rs/cross
fi

# Set compilation optimizations
export CARGO_NET_RETRY=3
export CARGO_HTTP_TIMEOUT=30
export CARGO_NET_GIT_FETCH_WITH_CLI=true

echo "🔨 Phase 1: Building core dependencies..."
timeout 600 cross build --target aarch64-linux-android --release

BUILD_RESULT=$?

if [ $BUILD_RESULT -eq 0 ]; then
    echo "✅ Phase 1 successful! Core build completed."
    echo "📱 Binary: target/aarch64-linux-android/release/eth-dca-bot"
    echo ""
    echo "📦 Binary info:"
    ls -lh target/aarch64-linux-android/release/eth-dca-bot
    file target/aarch64-linux-android/release/eth-dca-bot
    echo ""
    echo "🎉 You now have a working Android binary!"
    echo "💡 To add full features back:"
    echo "   1. Test this version first on your phone"
    echo "   2. Then gradually add back dependencies"
    echo "   3. Use 'cp Cargo.toml.full Cargo.toml' to restore all features"
    
elif [ $BUILD_RESULT -eq 124 ]; then
    echo "⏱️  Build timed out after 10 minutes"
    echo "💭 Even the simplified version is taking too long..."
    echo "🔧 Try these alternatives:"
    echo "   1. Build on a faster machine or cloud instance"
    echo "   2. Use GitHub Actions for cross-compilation"
    echo "   3. Consider native Android development"
    
else
    echo "❌ Build failed with exit code: $BUILD_RESULT"
    echo "🔍 This suggests a fundamental cross-compilation issue"
    echo ""
    echo "🛠️  Debug steps:"
    echo "   1. Check Docker has enough resources (4GB+ RAM)"
    echo "   2. Verify cross installation: cross --version"
    echo "   3. Test with minimal Rust project first"
fi

# Always restore the full Cargo.toml
echo "🔄 Restoring full Cargo.toml..."
cp Cargo.toml.full Cargo.toml
rm -f Cargo.toml.full
