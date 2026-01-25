#!/usr/bin/env bash
# Generate demo data for TUI screenshots
# Creates a temporary data directory with realistic-looking messages
#
# Usage:
#   ./scripts/generate-demo.sh           # Creates demo, prints BOTBUS_DATA_DIR to set
#   eval $(./scripts/generate-demo.sh)   # Creates demo and exports the env var
#
# Then run: botbus ui

set -euo pipefail

cd "$(dirname "$0")/.."

# Create temp directory for demo data
DEMO_DIR=$(mktemp -d /tmp/botbus-demo-XXXXXX)
export BOTBUS_DATA_DIR="$DEMO_DIR"

# Build if needed
if [[ ! -x target/release/botbus ]]; then
	cargo build --release --quiet
fi
BOTBUS="./target/release/botbus"

# Initialize
$BOTBUS init >/dev/null 2>&1

# Helper to send messages with specific agents
send_as() {
	local agent="$1"
	local target="$2"
	local message="$3"
	shift 3
	BOTBUS_AGENT="$agent" $BOTBUS send "$target" "$message" "$@" >/dev/null
}

# --- Generate realistic multi-agent conversation ---

# General channel - main coordination
send_as "swift-falcon" general "Starting work on the authentication refactor"
send_as "bold-tiger" general "Sounds good! I'll handle the frontend components"
send_as "swift-falcon" general "Can you check src/api/auth.rs before I modify it?" -L question
send_as "bold-tiger" general "Sure, it's all yours - I'm working on components/"
send_as "swift-falcon" general "Thanks! Claiming the file now"

# Claim some files
BOTBUS_AGENT="swift-falcon" $BOTBUS claim "src/api/**" -m "Auth refactor" >/dev/null 2>&1 || true
BOTBUS_AGENT="bold-tiger" $BOTBUS claim "src/components/**" -m "UI updates" >/dev/null 2>&1 || true

send_as "swift-falcon" general "Claimed src/api/** - working on OAuth integration"
send_as "quiet-owl" general "I can help with the database migrations when you're ready" -L offer
send_as "swift-falcon" general "That would be great @quiet-owl - probably in about an hour"
send_as "bold-tiger" general "FYI: found a bug in the login form validation" -L bug
send_as "quiet-owl" general "Is that blocking anyone?"
send_as "bold-tiger" general "No, just a minor UX issue - I'll fix it after the auth work merges"

# Backend channel - technical discussion
send_as "swift-falcon" backend "Question: should we use JWT or session tokens for the new auth?" -L question
send_as "quiet-owl" backend "JWT for the API, session for web UI - that's our current pattern"
send_as "swift-falcon" backend "Makes sense. I'll add refresh token rotation too"
send_as "quiet-owl" backend "Good call. Don't forget to update the middleware tests"

# DM conversation
send_as "bold-tiger" @swift-falcon "Hey, quick question about the API changes"
send_as "swift-falcon" @bold-tiger "Sure, what's up?"
send_as "bold-tiger" @swift-falcon "Will the auth endpoints be backwards compatible?"
send_as "swift-falcon" @bold-tiger "Yes - old tokens will work for 30 days during migration"
send_as "bold-tiger" @swift-falcon "Perfect, thanks!"

# More general activity
send_as "swift-falcon" general "Auth refactor PR is up for review" -L review
send_as "quiet-owl" general "I'll take a look now"
send_as "bold-tiger" general "Same - reviewing the frontend integration parts"

# Output the export command
echo "export BOTBUS_DATA_DIR=\"$DEMO_DIR\""
echo "# Demo data created in: $DEMO_DIR" >&2
echo "# Run: botbus ui" >&2
