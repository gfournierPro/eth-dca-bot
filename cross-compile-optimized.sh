#!/bin/bash
# Optimized cross-compilation script with better dependency handling

echo "🚀 Starting optimized cross-compilation for Android..."

# Function to restore Cargo.toml on exit
cleanup() {
    if [ -f "Cargo.toml.backup" ]; then
        echo "🔄 Restoring original Cargo.toml..."
        cp Cargo.toml.backup Cargo.toml
        rm -f Cargo.toml.backup Cargo.toml.minimal
    fi
}
trap cleanup EXIT

# Backup current Cargo.toml
cp Cargo.toml Cargo.toml.backup

# Create a minimal version with only essential dependencies
cat > Cargo.toml.minimal << 'EOF'
[package]
name = "eth-dca-bot"
version = "0.1.0"
edition = "2024"

[dependencies]
anyhow = "1.0.99"
chrono = { version = "0.4.41", features = ["serde"] }
chrono-tz = "0.10.0"
dotenv = "0.15.0"
hex = "0.4.3"
hmac = "0.12.1"
reqwest = { version = "0.12.23", default-features = false, features = ["json", "rustls-tls"] }
rust_decimal = "1.37.2"
serde = "1.0.219"
sha2 = "0.10.9"
tokio = { version = "1.47.1", features = ["macros", "rt-multi-thread", "fs", "net", "time"] }
tracing = "0.1.41"
tracing-subscriber = "0.3.19"
uuid = { version = "1.18.0", features = ["v4"] }
serde_json = "1.0.143"
cron = "0.15.0"

# Simplified database - use SQLite for mobile
sqlx = { version = "0.8.6", features = ["runtime-tokio-rustls", "sqlite", "chrono"] }

# Remove heavy dependencies for initial build
# mongodb = { version = "3.2.5", default-features = false, features = ["rustls-tls"] }
# notion-client = "1.0.10"
# tokio-cron-scheduler = "0.14.0"

EOF

# Use minimal configuration
cp Cargo.toml.minimal Cargo.toml

# Check if cross is installed
if ! command -v cross &> /dev/null; then
    echo "📦 Installing cross..."
    cargo install cross --git https://github.com/cross-rs/cross
fi

# Stop any existing cross containers
echo "🐳 Stopping existing cross containers..."
docker ps --filter ancestor=ghcr.io/cross-rs/aarch64-linux-android:main -q | xargs -r docker stop

# Clean everything
echo "🧹 Deep cleaning..."
cargo clean
rm -rf target/
rm -f Cargo.lock

# Update with minimal dependencies
echo "📦 Updating minimal dependencies..."
cargo update

# Set cross environment variables
export CARGO_NET_RETRY=3
export CARGO_HTTP_TIMEOUT=30

# Build with verbose output and error catching
echo "🔨 Cross-compiling with minimal dependencies..."
echo "   This may take 10-15 minutes for the first build..."

# Use timeout to prevent hanging
timeout 1800 cross build --target aarch64-linux-android --release --verbose 2>&1 | while IFS= read -r line; do
    echo "$line"
    # Show progress for long-running compilations
    if [[ "$line" == *"Compiling"* ]]; then
        echo "   ⏳ Still compiling dependencies..."
    fi
done

BUILD_EXIT_CODE=${PIPESTATUS[0]}

if [ $BUILD_EXIT_CODE -eq 0 ]; then
    echo ""
    echo "✅ Cross-compilation successful!"
    echo "📱 Binary: target/aarch64-linux-android/release/eth-dca-bot"
    echo ""
    echo "⚠️  Note: This is a minimal version without MongoDB and Notion integration"
    echo "   Full features require additional setup on the target device"
    echo ""
    echo "📦 Binary info:"
    ls -lh target/aarch64-linux-android/release/eth-dca-bot 2>/dev/null || echo "Binary not found"
    file target/aarch64-linux-android/release/eth-dca-bot 2>/dev/null || echo "Cannot analyze binary"
    echo ""
    echo "📋 Transfer to phone:"
    echo "1. adb push target/aarch64-linux-android/release/eth-dca-bot /data/data/com.termux/files/home/"
    echo "2. Or upload to cloud storage and download in Termux"
elif [ $BUILD_EXIT_CODE -eq 124 ]; then
    echo ""
    echo "⏱️  Build timed out after 30 minutes"
    echo "💡 Try building individual dependencies first:"
    echo "   cargo fetch --target aarch64-linux-android"
else
    echo ""
    echo "❌ Cross-compilation failed with exit code: $BUILD_EXIT_CODE"
    echo ""
    echo "🔍 Troubleshooting:"
    echo "1. Make sure Docker has enough memory (at least 4GB)"
    echo "2. Check internet connection for downloading dependencies"
    echo "3. Try: docker system prune to free up space"
    echo "4. Consider building on a machine with more resources"
fi

echo ""
echo "🧹 Cleaning up containers..."
docker ps --filter ancestor=ghcr.io/cross-rs/aarch64-linux-android:main -q | xargs -r docker stop

exit $BUILD_EXIT_CODE
