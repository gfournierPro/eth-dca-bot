#!/bin/bash
# Find which dependency is causing OpenSSL issues

echo "🔍 Debugging OpenSSL dependency issues..."

# Create minimal Cargo.toml to test
echo "📝 Testing minimal dependencies..."
cp Cargo.toml Cargo.toml.debug-backup

cat > Cargo.toml << 'EOF'
[package]
name = "eth-dca-bot"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1.0.99"
tokio = { version = "1.47.1", features = ["full"] }
reqwest = { version = "0.12.23", default-features = false, features = ["json", "rustls-tls"] }
serde = { version = "1.0.219", features = ["derive"] }
serde_json = "1.0.143"
EOF

echo "🔨 Testing minimal build..."
if cross build --target aarch64-linux-android --release; then
    echo "✅ Minimal build works!"
    echo "🔍 Now testing each dependency..."
    
    # Test adding dependencies one by one
    dependencies=(
        'chrono = { version = "0.4.41", features = ["serde"] }'
        'chrono-tz = "0.10.0"'
        'dotenv = "0.15.0"'
        'hex = "0.4.3"'
        'hmac = "0.12.1"'
        'rust_decimal = "1.37.2"'
        'sha2 = "0.10.9"'
        'sqlx = { version = "0.8.6", features = ["runtime-tokio-rustls", "sqlite", "chrono", "bigdecimal"] }'
        'mongodb = "3.2.5"'
        'notion-client = "1.0.10"'
    )
    
    for dep in "${dependencies[@]}"; do
        echo "🧪 Testing: $dep"
        
        # Add dependency
        echo "$dep" >> Cargo.toml
        
        # Try building
        if cross build --target aarch64-linux-android --release --quiet; then
            echo "✅ $dep - OK"
        else
            echo "❌ $dep - CAUSES OPENSSL ISSUE"
            # Remove the problematic dependency
            head -n -1 Cargo.toml > temp && mv temp Cargo.toml
        fi
    done
    
    echo ""
    echo "✅ Final working Cargo.toml:"
    cat Cargo.toml
    
else
    echo "❌ Even minimal build fails!"
    echo "💡 There might be a cross-compilation setup issue"
fi

# Restore original
echo "🔄 Restoring original Cargo.toml..."
cp Cargo.toml.debug-backup Cargo.toml
