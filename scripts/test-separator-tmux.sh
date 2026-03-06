#!/bin/bash
# Test separator with tmux
set -e

SESSION="rite-sep-test"

# Kill existing session if it exists
tmux kill-session -t $SESSION 2>/dev/null || true

# Create temp directory
TEST_DIR=$(mktemp -d)
echo "Test dir: $TEST_DIR"

# Setup
cd ~/src/rite
cargo build

# Create initial messages
export RITE_DATA_DIR="$TEST_DIR"
export RITE_AGENT="test-agent"
./target/debug/rite init
./target/debug/rite send general "Old message 1"
./target/debug/rite send general "Old message 2"
./target/debug/rite send general "Old message 3"

echo "Created 3 old messages"

# Create tmux session with TUI
tmux new-session -d -s $SESSION
tmux send-keys -t $SESSION "cd ~/src/rite && RITE_DATA_DIR=$TEST_DIR RITE_AGENT=viewer ./target/debug/rite ui" C-m

# Wait for TUI to start and capture initial sizes
sleep 3

# NOW send new message (after TUI has started and captured initial_sizes)
export RITE_AGENT="sender"
./target/debug/rite send general "NEW MESSAGE!"

echo "Sent new message"
sleep 1

# Capture TUI output
tmux capture-pane -t $SESSION -p >/tmp/tui-output.txt

echo ""
echo "=== TUI Output ==="
cat /tmp/tui-output.txt

echo ""
echo "=== Checking for separator ==="
if grep -q "New Messages" /tmp/tui-output.txt; then
	echo "✓ Separator found!"
else
	echo "✗ Separator NOT found"
fi

echo ""
echo "Tmux session '$SESSION' is still running. Attach with: tmux attach -t $SESSION"
echo "Press Ctrl+C to keep session, or wait 5s to auto-cleanup..."

sleep 5

tmux kill-session -t $SESSION
rm -rf "$TEST_DIR"
echo "Cleaned up"
