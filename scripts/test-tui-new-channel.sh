#!/bin/bash
# Test that TUI shows new channels when they're created while watching
set -e

# Create temp directory for isolated testing
TEST_DIR=$(mktemp -d)
export BOTBUS_DATA_DIR="$TEST_DIR"
export BOTBUS_AGENT="test-agent"

echo "=== Test: TUI shows new channels ==="
echo "Using temp data dir: $TEST_DIR"

# Initialize
cargo run --quiet -- init

# Start TUI in botty (background process that we can send commands to)
echo "Starting TUI..."
BOTTY_ID=$(botty spawn --name botbus-tui -- cargo run --quiet -- ui)
echo "TUI started with botty ID: $BOTTY_ID"

# Give TUI time to start and render
sleep 2

# Capture initial state
echo "Capturing initial state..."
INITIAL=$(botty snapshot "$BOTTY_ID")
echo "$INITIAL" | grep -q "general" && echo "✓ general channel visible" || echo "✗ general channel missing"

# Create a new channel while TUI is running
echo "Creating new channel 'test-new-channel'..."
cargo run --quiet -- send test-new-channel "Hello from new channel"

# Give TUI time to detect the new channel (watch + debounce)
sleep 1

# Capture state after new channel creation
echo "Capturing state after new channel creation..."
AFTER=$(botty snapshot "$BOTTY_ID")

# Check if new channel appears
if echo "$AFTER" | grep -q "test-new-channel"; then
	echo "✓ SUCCESS: New channel 'test-new-channel' appeared in TUI"
	EXIT_CODE=0
else
	echo "✗ FAIL: New channel 'test-new-channel' not visible in TUI"
	echo "--- Initial State ---"
	echo "$INITIAL"
	echo "--- After State ---"
	echo "$AFTER"
	EXIT_CODE=1
fi

# Cleanup
echo "Cleaning up..."
botty kill "$BOTTY_ID"
rm -rf "$TEST_DIR"

exit $EXIT_CODE
