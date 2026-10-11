#!/bin/bash
# Install a prebuilt, approved collector and privately provisioned configuration.
set -euo pipefail
if [[ $(id -u) != 0 ]]; then echo "Run with sudo." >&2; exit 1; fi
if [[ $# != 2 ]]; then
  echo "Usage: sudo bash install-macos.sh SERVICE_USER PRIVATE_BUNDLE_DIRECTORY" >&2
  exit 1
fi
SERVICE_USER="$1"
[[ "$SERVICE_USER" =~ ^[a-zA-Z_][a-zA-Z0-9_-]*$ ]] || exit 1
[[ $(id -u "$SERVICE_USER") != 0 ]] || { echo "Use an unprivileged account." >&2; exit 1; }
SERVICE_GROUP=$(id -gn "$SERVICE_USER")
SRC="$(cd "$2" && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="/Library/Application Support/HomeLabMetrics"
for name in lab-metrics config.json server.pem server-key.pem ca.pem; do
  [[ -f "$SRC/$name" ]] || { echo "Missing bundle file: $name" >&2; exit 1; }
done
install -d -o root -g wheel -m 755 "$DEST"
install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 700 "$DEST/run"
install -o root -g wheel -m 755 "$SRC/lab-metrics" "$DEST/lab-metrics"
for name in config.json server.pem server-key.pem ca.pem; do
  install -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 600 "$SRC/$name" "$DEST/$name"
done
PLIST=$(mktemp)
trap 'rm -f "$PLIST"' EXIT
sed "s/@SERVICE_USER@/$SERVICE_USER/g" "$HERE/com.home-lab.metrics.plist.in" > "$PLIST"
plutil -lint "$PLIST"
install -o root -g wheel -m 644 "$PLIST" /Library/LaunchDaemons/com.home-lab.metrics.plist
launchctl bootout system/com.home-lab.metrics 2>/dev/null || true
launchctl enable system/com.home-lab.metrics
launchctl bootstrap system /Library/LaunchDaemons/com.home-lab.metrics.plist
echo "Installed boot-time collector; no GUI login required after macOS boots."
