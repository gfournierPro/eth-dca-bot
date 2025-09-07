# Quick fix for MongoDB feature error

echo "🔧 Fixing MongoDB dependency issue..."

# Option 1: Use simple MongoDB without TLS features
echo "📝 Creating simple Cargo.toml (Option 1)..."
cp Cargo.toml.simple Cargo.toml

# Try building
echo "🚀 Trying to build with simple dependencies..."
cargo clean
if cargo build --release; then
    echo "✅ Build successful with simple dependencies!"
    exit 0
fi

echo "❌ Simple build failed, trying alternative..."

# Option 2: Keep original MongoDB but fix reqwest
echo "📝 Creating alternative Cargo.toml (Option 2)..."
cat > Cargo.toml << 'EOF'
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
# Use rustls for reqwest only
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
# Keep MongoDB simple - it handles TLS internally
mongodb = "3.2.5"
futures = "0.3.31"
bson = { version = "2.15", features = ["chrono-0_4"] }
notion-client = "1.0.10"
serde_json = "1.0.143"
cron = "0.15.0"
EOF

echo "🚀 Trying to build with alternative config..."
cargo clean
if cargo build --release; then
    echo "✅ Build successful with alternative config!"
    exit 0
fi

echo "❌ Both options failed. You may need to use MongoDB Atlas (cloud) instead."
echo "💡 Try: Remove 'mongodb' dependency and use a cloud database."
