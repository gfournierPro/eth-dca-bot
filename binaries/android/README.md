# Android Binaries

This directory contains the cross-compiled Android binaries for the eth-dca-bot.

## Files
- `eth-dca-bot-android` - ARM64 Android binary, automatically built by GitHub Actions

## Usage
1. Download the binary to your Android device (via Termux), along with a `.env` (see `.env.example`)
2. Make it executable: `chmod +x eth-dca-bot-android`
3. Run: `./eth-dca-bot-android`

The bot needs MongoDB reachable at `MONGODB_URL` (defaults to
`mongodb://dca_user:dca_password@localhost:27017/dca_bot`). Install it on-device
with `pkg install mongodb` (Termux TUR repo) and create the matching user:
```
mongo --eval 'db=db.getSiblingDB("dca_bot"); db.createUser({user:"dca_user",pwd:"dca_password",roles:[{role:"readWrite",db:"dca_bot"}]})'
```
If `mongod` fails with a `libyaml-cpp` symbol error, run `pkg upgrade` first.

## Running persistently (survives crashes / Termux restarts)
`services/` contains runit service definitions for `mongod` and
`eth-dca-bot`, supervised via `termux-services` (auto-restarts either
process if it dies; auto-starts next time Termux/`runsvdir` comes up).

Deploy: `scp -r services xiaomi-termux:~/services && ssh xiaomi-termux 'cd ~/services && sh install.sh'`

Useful commands on-device (after `export SVDIR=$PREFIX/var/service`, which
Termux sets automatically in interactive shells):
- `sv status mongod eth-dca-bot` — check state
- `sv restart eth-dca-bot` — pick up a new binary/`.env`
- `sv down eth-dca-bot` / `sv up eth-dca-bot` — stop/start manually
- Logs: `tail -f $PREFIX/var/log/sv/eth-dca-bot/current`

This does **not** survive a full phone reboot unless Termux is opened
afterward (Android doesn't launch apps on boot). For that, install the
separate Termux:Boot app and drop a script in `~/.termux/boot/` that starts
`runsvdir` — a manual one-time step on the device.

## Build Info
- Target: aarch64-linux-android
- Built with: GitHub Actions + cross-rs
- Dependencies: All statically linked with vendored OpenSSL
