#!/bin/bash
# Integration test: verify file watcher picks up new channel files
set -e

TEST_DIR=$(mktemp -d)
export BOTBUS_DATA_DIR="$TEST_DIR"
export BOTBUS_AGENT="test-agent"

echo "Test dir: $TEST_DIR"

# Initialize
cd ~/src/botbus
cargo run --quiet -- init

# Create initial channel
cargo run --quiet -- send general "Initial"

# Set up file watcher on channels directory
echo "Setting up watch on $TEST_DIR/channels/"
inotifywait -m "$TEST_DIR/channels/" -e create -e modify --format '%e %f' &
WATCH_PID=$!

sleep 0.5

# Create new channel
echo "Creating new channel..."
cargo run --quiet -- send test-new "Hello"

sleep 0.5

# Kill watcher
kill $WATCH_PID 2>/dev/null || true

# Verify channel file exists
if [ -f "$TEST_DIR/channels/test-new.jsonl" ]; then
	echo "✓ Channel file created"
else
	echo "✗ Channel file missing"
	exit 1
fi

rm -rf "$TEST_DIR"
echo "✓ Integration test passed"
