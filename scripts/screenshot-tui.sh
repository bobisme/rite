#!/usr/bin/env bash
# Automatically capture a screenshot of the TUI for documentation
# Requires: kitty, hyprctl (Hyprland), grim, jq, pngquant

set -euo pipefail

WIDTH="${1:-1200}"
HEIGHT="${2:-800}"
OUTPUT="${3:-images/tui.png}"

cd "$(dirname "$0")/.."

# Launch kitty with the TUI
kitty --title "rite-screenshot" -e bash -c "./target/release/rite ui; read" &
KITTY_PID=$!

# Wait for window to appear
sleep 0.5

# Make it floating and set specific size
hyprctl dispatch focuswindow "title:rite-screenshot" >/dev/null
hyprctl dispatch togglefloating >/dev/null
hyprctl dispatch resizeactive exact "$WIDTH" "$HEIGHT" >/dev/null
hyprctl dispatch centerwindow >/dev/null

# Wait for resize to apply and TUI to re-render
sleep 0.3

# Get geometry and capture
GEOMETRY=$(hyprctl clients -j | jq -r '.[] | select(.title == "rite-screenshot") | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"')

if [[ -z "$GEOMETRY" ]]; then
	echo "Error: Could not find window" >&2
	kill $KITTY_PID 2>/dev/null || true
	exit 1
fi

echo "Capturing ${WIDTH}x${HEIGHT} window..."
grim -g "$GEOMETRY" /tmp/rite-tui-auto.png

# Kill the kitty window
kill $KITTY_PID 2>/dev/null || true

# Compress with pngquant
mkdir -p "$(dirname "$OUTPUT")"
pngquant --force --output "$OUTPUT" /tmp/rite-tui-auto.png

SIZE=$(du -h "$OUTPUT" | cut -f1)
echo "Saved to $OUTPUT ($SIZE)"
