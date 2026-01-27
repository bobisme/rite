#!/bin/bash
# Debug separator feature
set -e

TEST_DIR=$(mktemp -d)
export BOTBUS_DATA_DIR="$TEST_DIR"
export BOTBUS_AGENT="test-agent"

cd ~/src/botbus

echo "Test dir: $TEST_DIR"

# Initialize
cargo run --quiet -- init

# Create 3 old messages
echo "Creating old messages..."
cargo run --quiet -- send general "Old 1"
sleep 0.1
cargo run --quiet -- send general "Old 2"
sleep 0.1
cargo run --quiet -- send general "Old 3"

# Check file size
OLD_SIZE=$(stat -f%z "$TEST_DIR/channels/general.jsonl" 2>/dev/null || stat -c%s "$TEST_DIR/channels/general.jsonl")
echo "Old file size: $OLD_SIZE bytes"

# Now simulate starting TUI (capture initial_sizes at this point)
echo ""
echo "--- At this point, TUI would start and capture initial_sizes[$OLD_SIZE] ---"
echo ""

# Add new messages
echo "Adding new messages..."
sleep 1
cargo run --quiet -- send general "New 1"
sleep 0.1
cargo run --quiet -- send general "New 2"

# Check new file size
NEW_SIZE=$(stat -f%z "$TEST_DIR/channels/general.jsonl" 2>/dev/null || stat -c%s "$TEST_DIR/channels/general.jsonl")
echo "New file size: $NEW_SIZE bytes"
echo "Delta: $((NEW_SIZE - OLD_SIZE)) bytes"

# Count lines
TOTAL_LINES=$(wc -l <"$TEST_DIR/channels/general.jsonl")
echo "Total messages: $TOTAL_LINES"

# Show the file
echo ""
echo "Messages in file:"
cat "$TEST_DIR/channels/general.jsonl" | jq -r '"\(.ts) \(.agent): \(.body)"'

echo ""
echo "Expected: Separator should appear between 'Old 3' and 'New 1'"
echo ""

rm -rf "$TEST_DIR"
