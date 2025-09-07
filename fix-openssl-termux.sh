#!/bin/bash
# Quick fix for OpenSSL compilation issues in Termux

echo "🔧 Fixing OpenSSL compilation issues..."

# Install missing packages
echo "📦 Installing OpenSSL packages..."
pkg install -y openssl openssl-tool libcrypt clang

# Set up environment variables
echo "⚙️ Setting up OpenSSL environment variables..."
export OPENSSL_DIR=$PREFIX
export OPENSSL_LIB_DIR=$PREFIX/lib
export OPENSSL_INCLUDE_DIR=$PREFIX/include
export PKG_CONFIG_PATH=$PREFIX/lib/pkgconfig

# Add to shell profile for persistence
echo "📝 Adding to shell profile..."
cat >> ~/.bashrc << 'EOF'

# OpenSSL configuration for Rust compilation
export OPENSSL_DIR=$PREFIX
export OPENSSL_LIB_DIR=$PREFIX/lib
export OPENSSL_INCLUDE_DIR=$PREFIX/include
export PKG_CONFIG_PATH=$PREFIX/lib/pkgconfig
EOF

# Source the profile
source ~/.bashrc

echo "✅ OpenSSL environment configured!"
echo ""
echo "🚀 Now try building again:"
echo "  source ~/.bashrc"
echo "  cargo clean"
echo "  cargo build --release"
echo ""
echo "📋 If it still fails, try using rustls instead of OpenSSL:"
echo "  Add this to your Cargo.toml dependencies:"
echo "  reqwest = { version = \"0.12.23\", default-features = false, features = [\"json\", \"rustls-tls\"] }"
