#!/bin/bash
# Pipe rite mentions for one agent into a live Codex session via codex queue.
# Usage: bridge2.sh <agent> <session-id> <log>
AGENT="$1"; SID="$2"; LOG="$3"
rite mentions follow --agent "$AGENT" --format json | while IFS= read -r line; do
  text=$(printf '%s' "$line" | jq -r '"[rite] channel=\(.channel) from=\(.message.agent) id=\(.message.id) reply_target=\(.reply_target) route=\(.route)\n\(.message.body)"')
  ts=$(date -u +%FT%T.%3NZ)
  if out=$(codex queue --thread "$SID" --message "$text" 2>&1); then
    echo "$ts queued ok id=$(printf '%s' "$line" | jq -r .message.id)" >> "$LOG"
  else
    echo "$ts queue FAILED id=$(printf '%s' "$line" | jq -r .message.id): $out" >> "$LOG"
  fi
done
