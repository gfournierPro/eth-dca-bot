#!/bin/bash
# Incremental cross-compilation - much faster for subsequent builds

echo "⚡ Starting incremental cross-compilation..."

# Don't clean everything - just update what's needed
echo "📦 Fetching dependencies for target..."
cargo fetch --target aarch64-linux-android

echo "🔨 Building incrementally (keeping cache)..."
# Use the existing cache and build incrementally
cross build --target aarch64-linux-android --release

if [ $? -eq 0 ]; then
    echo "✅ Incremental build successful!"
    echo "📱 Binary: target/aarch64-linux-android/release/eth-dca-bot"
    ls -lh target/aarch64-linux-android/release/eth-dca-bot
else
    echo "❌ Incremental build failed. You may need to run 'cargo clean' and start fresh."
fi
