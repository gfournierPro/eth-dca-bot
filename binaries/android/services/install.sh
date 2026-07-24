#!/data/data/com.termux/files/usr/bin/sh
# Installs mongod + eth-dca-bot as runit services (auto-restart on crash,
# auto-start whenever runsvdir is running). Run this ON the phone, from the
# directory this script lives in (binaries/android/services/).
#
# Note: $SVDIR/$LOGDIR are normally exported by Termux's interactive-shell
# profile.d hook, which doesn't fire over a plain non-interactive `ssh host
# cmd`. This script sets them explicitly so it works either way.
set -e

export SVDIR="$PREFIX/var/service"
export LOGDIR="$PREFIX/var/log"

pkg list-installed 2>/dev/null | grep -q '^termux-services/' || pkg install -y termux-services

for svc in mongod eth-dca-bot; do
  dest="$SVDIR/$svc"
  mkdir -p "$dest/log"
  cp "$svc/run" "$dest/run"
  cp "$svc/log/run" "$dest/log/run"
  chmod +x "$dest/run" "$dest/log/run"
  touch "$dest/down"
done

pgrep -f "runsvdir $SVDIR" >/dev/null || nohup runsvdir "$SVDIR" >"$LOGDIR/runsvdir.log" 2>&1 &
sleep 2
sv-enable mongod
sv-enable eth-dca-bot
sleep 3
sv status mongod mongod/log eth-dca-bot eth-dca-bot/log
