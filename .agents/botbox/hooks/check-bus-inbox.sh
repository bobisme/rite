#!/bin/bash
# botbox PostToolUse hook: check for unread bus messages and inject reminder

# Read JSON input from stdin to extract cwd
INPUT=$(cat)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)

if [ -z "$CWD" ]; then
  # Fallback to git if jq fails or cwd not in input
  REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null)
  if [ -z "$REPO_ROOT" ]; then
    exit 0
  fi
  CWD="$REPO_ROOT"
fi

# Try to read channel from .botbox.json
CHANNEL=""
if [ -f "$CWD/.botbox.json" ] && command -v jq &>/dev/null; then
  CHANNEL=$(jq -r '.project.channel // .project.name // empty' "$CWD/.botbox.json" 2>/dev/null)
fi

# Fallback to basename if no channel in config
if [ -z "$CHANNEL" ]; then
  CHANNEL=$(basename "$CWD")
fi

# Get agent identity from env or .botbox.json
AGENT=""
if [ -n "$BOTBUS_AGENT" ]; then
  AGENT="$BOTBUS_AGENT"
elif [ -f "$CWD/.botbox.json" ] && command -v jq &>/dev/null; then
  AGENT=$(jq -r '.project.defaultAgent // .project.default_agent // empty' "$CWD/.botbox.json" 2>/dev/null)
fi

# Build bus inbox command with optional --agent flag
INBOX_CMD="bus inbox --count-only --mentions --channels \"$CHANNEL\""
if [ -n "$AGENT" ]; then
  INBOX_CMD="bus inbox --agent \"$AGENT\" --count-only --mentions --channels \"$CHANNEL\""
fi

COUNT=$(eval $INBOX_CMD 2>/dev/null)
if [ $? -ne 0 ]; then
  exit 0
fi

if [ "$COUNT" = "0" ]; then
  exit 0
fi

if [ "$COUNT" -gt 0 ]; then
  # Fetch message previews (limit 5, text format for easy parsing)
  FETCH_CMD="bus inbox --mentions --channels \"$CHANNEL\" --limit-per-channel 5 --format text"
  if [ -n "$AGENT" ]; then
    FETCH_CMD="bus inbox --agent \"$AGENT\" --mentions --channels \"$CHANNEL\" --limit-per-channel 5 --format text"
  fi

  MESSAGES=$(eval $FETCH_CMD 2>/dev/null | \
    grep -E '^\[' | \
    sed 's/\[Today [0-9:]*\] //' | \
    sed 's/\[Yesterday [0-9:]*\] //' | \
    sed 's/\[[0-9-]* [0-9:]*\] //' | \
    head -5 | \
    while IFS= read -r line; do
      # Truncate to ~80 chars
      if [ ${#line} -gt 80 ]; then
        echo "  - ${line:0:77}..."
      else
        echo "  - $line"
      fi
    done)

  MARK_READ_CMD="bus inbox --mentions --channels $CHANNEL --mark-read"
  if [ -n "$AGENT" ]; then
    MARK_READ_CMD="bus inbox --agent $AGENT --mentions --channels $CHANNEL --mark-read"
  fi

  cat << EOF
{
  "hookSpecificOutput": {
    "hookEventName": "PostToolUse",
    "additionalContext": "STOP: You have $COUNT unread botbus message(s) in #$CHANNEL. Check if any need a response:\n$MESSAGES\n\nTo read and respond: \`$MARK_READ_CMD\`"
  }
}
EOF
fi

exit 0
