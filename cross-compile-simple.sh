#!/bin/bash
# Super simple cross-compilation - no timeouts, no complexity

echo "🚀 Simple cross-compilation attempt..."

# Kill any existing processes
pkill -f cross 2>/dev/null || true
docker ps -q --filter ancestor=ghcr.io/cross-rs/aarch64-linux-android:main | xargs -r docker stop

# Clean start
cargo clean
rm -f Cargo.lock

echo "📦 Installing cross if needed..."
if ! command -v cross &> /dev/null; then
    cargo install cross --git https://github.com/cross-rs/cross
fi

# Simple environment
export CARGO_NET_RETRY=2

echo "🔨 Starting cross-compilation..."
echo "   Press Ctrl+C if it hangs for more than 5 minutes"

cross build --target aarch64-linux-android --release

if [ $? -eq 0 ]; then
    echo "✅ Success!"
    ls -lh target/aarch64-linux-android/release/eth-dca-bot
else
    echo "❌ Failed. Consider using GitHub Actions instead."
fi
