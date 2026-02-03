#!/usr/bin/env bash
set -euo pipefail

REVIEWER_NAME="security-reviewer"
MAX_LOOPS=${MAX_LOOPS:-10}
INBOX_LIMIT=${INBOX_LIMIT:-200}

if [[ -z "${BOTBUS_AGENT:-}" ]]; then
	echo "BOTBUS_AGENT is required (set your identity before spawning)."
	exit 1
fi

ORIG_AGENT="$BOTBUS_AGENT"

if ! bus claims stake "agent://$REVIEWER_NAME" -m "spawn $REVIEWER_NAME"; then
	echo "Claim denied. $REVIEWER_NAME should already be spawned."
	exit 0
fi

cleanup() {
	BOTBUS_AGENT="$ORIG_AGENT" bus claims release "agent://$REVIEWER_NAME" >/dev/null 2>&1 || true
	export BOTBUS_AGENT="$ORIG_AGENT"
}

trap cleanup EXIT

PYTHON_BIN=${PYTHON_BIN:-python3}
if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
	PYTHON_BIN=python
fi
if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
	echo "python is required for parsing botbus JSON output."
	exit 1
fi

declare -A SEEN_IDS=()

collect_channels() {
	BOTBUS_AGENT="$REVIEWER_NAME" botbus --json channels --mine | "$PYTHON_BIN" -c \
		'import json,sys; data=json.load(sys.stdin); print("\n".join(c.get("name", "") for c in data.get("channels", [])))'
}

relevant_message_ids() {
	local channel="$1"
	BOTBUS_AGENT="$REVIEWER_NAME" botbus --json inbox --channel "$channel" -n "$INBOX_LIMIT" |
		CHANNEL="$channel" REVIEWER_NAME="$REVIEWER_NAME" "$PYTHON_BIN" -c \
			'import os,json,sys; channel=os.environ.get("CHANNEL",""); reviewer=os.environ.get("REVIEWER_NAME","security-reviewer"); data=json.load(sys.stdin); messages=data.get("messages", []);
def is_review(msg):
  labels=[l.lower() for l in msg.get("labels", [])]; body=(msg.get("body") or "").lower();
  return any(l in {"review","review-request","re-review","rereview"} for l in labels) or "review" in body
def is_dm(name):
  return name.startswith("_dm_")
for msg in messages:
  body=msg.get("body") or "";
  if is_dm(channel) or f"@{reviewer}" in body:
    if is_review(msg):
      print(msg.get("id",""))'
}

for ((i = 1; i <= MAX_LOOPS; i++)); do
	found_new=0
	while IFS= read -r channel; do
		[[ -z "$channel" ]] && continue
		while IFS= read -r msg_id; do
			[[ -z "$msg_id" ]] && continue
			if [[ -z "${SEEN_IDS[$msg_id]:-}" ]]; then
				SEEN_IDS[$msg_id]=1
				found_new=1
			fi
		done < <(relevant_message_ids "$channel")
	done < <(collect_channels)

	if [[ "$found_new" -eq 0 ]]; then
		break
	fi

	BOTBUS_AGENT="$REVIEWER_NAME" claude -p "$(
		cat <<'EOF'
You are the security review agent for botbox.

Tasks:
- Use botbus to find review requests addressed to @security-reviewer or DMs.
- Use crit to open the review, read diffs, and leave comments.
- Leave aggressive security feedback: threat model, auth, access control, input validation,
  injection risks, secrets handling, SSRF, path traversal, sandbox escapes, unsafe defaults.
- If asked to re-review, verify the fixes and justification. State whether concerns are resolved.

How to review:
- botbus inbox (or history) to find the request and identify the project channel + author.
- crit inbox / crit review <id> to inspect the review.
- crit comment <review-id> "..." to leave comments.
- crit lgtm <review-id> to approve, or crit block <review-id> to request changes.

Finish:
- Post a summary in the project's channel, tagging the author when done.
EOF
	)"
done

export BOTBUS_AGENT="$ORIG_AGENT"
