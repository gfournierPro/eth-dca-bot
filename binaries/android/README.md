# Android Binaries

This directory contains the cross-compiled Android binaries for the eth-dca-bot.

## Files
- `eth-dca-bot-android` - ARM64 Android binary, automatically built by GitHub Actions

## Usage
1. Download the binary to your Android device (via Termux)
2. Make it executable: `chmod +x eth-dca-bot-android`
3. Run: `./eth-dca-bot-android`

## Build Info
- Target: aarch64-linux-android
- Built with: GitHub Actions + cross-rs
- Dependencies: All statically linked with vendored OpenSSL
