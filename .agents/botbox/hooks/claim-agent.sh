#!/bin/bash
# botbox hook: claim agent:// advisory lock, set status, release on exit
# Events: SessionStart (claim+status), PostToolUse (refresh), SessionEnd (release+clear)
# Only active when BOTBUS_AGENT is set. All errors silently ignored.

[ -z "$BOTBUS_AGENT" ] && exit 0

CLAIM_URI="agent://$BOTBUS_AGENT"
CLAIM_TTL=600
REFRESH_THRESHOLD=120

# Read hook event from stdin JSON
INPUT=$(cat 2>/dev/null)
EVENT=$(echo "$INPUT" | jq -r '.hook_event_name // empty' 2>/dev/null)

# SessionEnd: release claim and clear status
if [ "$EVENT" = "SessionEnd" ]; then
  bus claims release --agent "$BOTBUS_AGENT" "$CLAIM_URI" -q 2>/dev/null
  bus statuses clear --agent "$BOTBUS_AGENT" -q 2>/dev/null
  exit 0
fi

# PostToolUse: refresh only if claim is within REFRESH_THRESHOLD seconds of expiring
if [ "$EVENT" = "PostToolUse" ]; then
  EXPIRES=$(bus claims list --mine --agent "$BOTBUS_AGENT" --format json 2>/dev/null \
    | jq -r ".claims[] | select(.patterns[] == \"$CLAIM_URI\") | .expires_in_secs" 2>/dev/null)

  if [ -n "$EXPIRES" ] && [ "$EXPIRES" -lt "$REFRESH_THRESHOLD" ] 2>/dev/null; then
    bus claims refresh --agent "$BOTBUS_AGENT" "$CLAIM_URI" --ttl "$CLAIM_TTL" -q 2>/dev/null
  fi
  exit 0
fi

# SessionStart / PreCompact / other: stake claim and set status
bus claims stake --agent "$BOTBUS_AGENT" "$CLAIM_URI" --ttl "$CLAIM_TTL" -q 2>/dev/null
if [ $? -eq 0 ]; then
  bus statuses set --agent "$BOTBUS_AGENT" "Claude Code" -q 2>/dev/null
fi

exit 0
