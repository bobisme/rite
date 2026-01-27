#!/bin/bash
# Test the "New Messages" separator feature
set -e

echo "=== New Messages Separator Feature Test ==="
echo ""
echo "This script will:"
echo "1. Create a test environment with old messages"
echo "2. Instructions to start the TUI"
echo "3. Add new messages from another terminal"
echo "4. You should see a separator line appear"
echo "5. After 2 seconds of viewing, separator auto-clears"
echo ""

# Create temp directory
TEST_DIR=$(mktemp -d)
export BOTBUS_DATA_DIR="$TEST_DIR"
export BOTBUS_AGENT="test-agent"

echo "Using temp dir: $TEST_DIR"
cd ~/src/botbus

# Initialize and create "old" messages
cargo run --quiet -- init
echo "Creating old messages..."
cargo run --quiet -- send general "Old message 1"
cargo run --quiet -- send general "Old message 2"
cargo run --quiet -- send general "Old message 3"

echo ""
echo "✓ Old messages created"
echo ""
echo "Now do the following:"
echo ""
echo "  Terminal 1 (this one):"
echo "    BOTBUS_DATA_DIR=$TEST_DIR cargo run -- ui"
echo ""
echo "  Terminal 2 (after TUI starts):"
echo "    BOTBUS_DATA_DIR=$TEST_DIR BOTBUS_AGENT=agent-2 \\"
echo "      cargo run -- send general 'New message!'"
echo ""
echo "You should see:"
echo "  - (1) indicator on #general in sidebar"
echo "  - Switch to #general"
echo "  - Separator line: ─────── New Messages ───────"
echo "  - After 2 seconds, separator disappears"
echo "  - (1) indicator clears"
echo ""
echo "When done testing, run:"
echo "  rm -rf $TEST_DIR"
echo ""
