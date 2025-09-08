#!/bin/bash
# Complete fix for Termux compilation issues

echo "🔧 Comprehensive fix for Termux compilation..."

# First, let's backup the current Cargo.toml
cp Cargo.toml Cargo.toml.backup

# Create a Termux-optimized Cargo.toml
echo "📝 Creating Termux-optimized Cargo.toml..."
cat > Cargo.toml << 'EOF'
[package]
name = "eth-dca-bot"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1.0.99"
chrono = { version = "0.4.41", features = ["serde"] }
chrono-tz = "0.10.0"
dotenv = "0.15.0"
hex = "0.4.3"
hmac = "0.12.1"
# Use rustls for all HTTP requests
reqwest = { version = "0.12.23", default-features = false, features = ["json", "rustls-tls"] }
rust_decimal = "1.37.2"
serde = { version = "1.0.219", features = ["derive"] }
sha2 = "0.10.9"
tokio-cron-scheduler = "0.14.0"
tracing = "0.1.41"
tracing-subscriber = "0.3.19"
tokio = { version = "1.47.1", features = ["full"] }
# Use rustls for SQLx
sqlx = { version = "0.8.6", features = ["runtime-tokio-rustls", "sqlite", "chrono", "bigdecimal"] }
uuid = { version = "1.18.0", features = ["v4"] }
rust_decimal_macros = "1.37.1"
# MongoDB - simple version
mongodb = "3.2.5"
futures = "0.3.31"
bson = { version = "2.15", features = ["chrono-0_4"] }
# Replace notion-client with a simpler HTTP client approach
# notion-client = "1.0.10"  # This might be causing OpenSSL issues
serde_json = "1.0.143"
cron = "0.15.0"
EOF

echo "🚀 Trying build without notion-client..."
cargo clean
if cargo build --release; then
    echo "✅ Build successful without notion-client!"
    echo "💡 You'll need to implement Notion integration manually using reqwest"
    exit 0
fi

echo "❌ Still failing, trying minimal dependencies..."

# Create ultra-minimal version
cat > Cargo.toml << 'EOF'
[package]
name = "eth-dca-bot"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1.0.99"
chrono = { version = "0.4.41", features = ["serde"] }
chrono-tz = "0.10.0"
dotenv = "0.15.0"
hex = "0.4.3"
hmac = "0.12.1"
reqwest = { version = "0.12.23", default-features = false, features = ["json", "rustls-tls"] }
rust_decimal = "1.37.2"
serde = { version = "1.0.219", features = ["derive"] }
sha2 = "0.10.9"
tokio-cron-scheduler = "0.14.0"
tracing = "0.1.41"
tracing-subscriber = "0.3.19"
tokio = { version = "1.47.1", features = ["full"] }
# Use SQLite instead of MongoDB for mobile
sqlx = { version = "0.8.6", features = ["runtime-tokio-rustls", "sqlite", "chrono", "bigdecimal"] }
uuid = { version = "1.18.0", features = ["v4"] }
rust_decimal_macros = "1.37.1"
futures = "0.3.31"
serde_json = "1.0.143"
cron = "0.15.0"
EOF

echo "🚀 Trying minimal build (SQLite instead of MongoDB)..."
cargo clean
if cargo build --release; then
    echo "✅ Minimal build successful!"
    echo "💡 You're using SQLite instead of MongoDB - I'll help you adapt the code"
    exit 0
fi

echo "❌ All options failed. There might be a deeper Termux compatibility issue."
echo "🔍 Try running: cargo build --release --verbose"
echo "   This will show exactly which dependency is failing."
EOF

chmod +x complete-termux-fix.sh
