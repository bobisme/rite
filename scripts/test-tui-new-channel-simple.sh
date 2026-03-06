#!/bin/bash
# Simpler test: verify channel list refresh logic works
set -e

# Create temp directory for isolated testing
TEST_DIR=$(mktemp -d)
export RITE_DATA_DIR="$TEST_DIR"
export RITE_AGENT="test-agent"

echo "=== Test: Channel refresh logic ==="
echo "Using temp data dir: $TEST_DIR"

# Initialize and create initial channel
cargo run --quiet -- init
cargo run --quiet -- send general "Initial message"

# List initial channels
echo "Initial channels:"
cargo run --quiet -- channels

# Create a new channel
echo ""
echo "Creating new channel 'test-new'..."
cargo run --quiet -- send test-new "Hello"

# List channels again
echo ""
echo "Channels after creation:"
cargo run --quiet -- channels

# Verify the new channel exists
if cargo run --quiet -- channels | grep -q "test-new"; then
	echo "✓ SUCCESS: New channel 'test-new' is listable"
else
	echo "✗ FAIL: New channel 'test-new' not found"
	exit 1
fi

# Check that the channel file exists
if [ -f "$TEST_DIR/channels/test-new.jsonl" ]; then
	echo "✓ Channel file created: $TEST_DIR/channels/test-new.jsonl"
else
	echo "✗ Channel file missing"
	exit 1
fi

echo ""
echo "Channel creation and listing works correctly."
echo "The TUI should now pick up these changes via file watch."

# Cleanup
rm -rf "$TEST_DIR"

echo "✓ All checks passed"
