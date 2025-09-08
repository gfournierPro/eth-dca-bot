#!/bin/bash
# Alternative cross-compilation script using rustls-only approach

echo "🔒 Cross-compiling with rustls-only approach..."

# Create a temporary Cargo.toml with rustls-only dependencies
cp Cargo.toml Cargo.toml.backup

# Create rustls-only version
cat > Cargo.toml.rustls << 'EOF'
[package]
name = "eth-dca-bot"
version = "0.1.0"
edition = "2024"

[dependencies]
anyhow = "1.0.99"
chrono = {version = "0.4.41",  features = ["serde"]}
chrono-tz = "0.10.0"
dotenv = "0.15.0"
hex = "0.4.3"
hmac = "0.12.1"
reqwest = { version = "0.12.23", default-features = false, features = ["json", "rustls-tls"] }
rust_decimal = "1.37.2"
serde = "1.0.219"
sha2 = "0.10.9"
tokio-cron-scheduler = "0.14.0"
tracing = "0.1.41"
tracing-subscriber = "0.3.19"
tokio = { version = "1.47.1", features = ["full"] }
sqlx = { version = "0.8.6", features = ["runtime-tokio-rustls", "sqlite", "chrono", "bigdecimal"] }
uuid = { version = "1.18.0", features = ["v4"] }

rust_decimal_macros = "1.37.1"
# Use mongodb with rustls instead of default OpenSSL
mongodb = { version = "3.2.5", default-features = false, features = ["rustls-tls"] }
futures = "0.3.31"
bson = { version = "2.15", features = ["chrono-0_4"] }
# For notion-client, we'll try to use it without TLS features first
notion-client = { version = "1.0.10", default-features = false }
serde_json = "1.0.143"
cron = "0.15.0"

# Use rustls-native-certs instead of system certs
rustls-native-certs = "0.8"
EOF

# Use the rustls-only version
cp Cargo.toml.rustls Cargo.toml

echo "🧹 Cleaning previous builds..."
cargo clean

echo "📦 Updating dependencies with rustls-only config..."
cargo update

echo "🔨 Building for Android with rustls-only..."
cross build --target aarch64-linux-android --release

if [ $? -eq 0 ]; then
    echo "✅ Cross-compilation successful with rustls!"
    echo "📱 Binary: target/aarch64-linux-android/release/eth-dca-bot"
    
    # Restore original Cargo.toml
    cp Cargo.toml.backup Cargo.toml
    rm Cargo.toml.rustls Cargo.toml.backup
    
    echo ""
    echo "📦 Binary info:"
    ls -lh target/aarch64-linux-android/release/eth-dca-bot
    file target/aarch64-linux-android/release/eth-dca-bot
else
    echo "❌ Cross-compilation with rustls failed!"
    # Restore original Cargo.toml
    cp Cargo.toml.backup Cargo.toml
    rm Cargo.toml.rustls Cargo.toml.backup
    exit 1
fi
