#!/data/data/com.termux/files/usr/bin/sh
# Pulls the latest git state and re-launches eth-dca-bot from the repo's
# checked-in binary (binaries/android/eth-dca-bot-android, auto-updated by
# CI). Run this ON the phone.
set -e

REPO="$HOME/eth-dca-bot"
BIN="$HOME/eth-dca-bot-android"

export SVDIR="$PREFIX/var/service"

cd "$REPO"
git pull --ff-only

sv stop eth-dca-bot
cp "$REPO/binaries/android/eth-dca-bot-android" "$BIN"
chmod +x "$BIN"
sv start eth-dca-bot
sleep 2
sv status eth-dca-bot
