#!/usr/bin/env bash
set -euo pipefail

# --- Defaults ---
MAX_LOOPS=20
LOOP_PAUSE=2
CLAUDE_TIMEOUT=600
MODEL=""
PROJECT=""
AGENT=""

# --- Load config from .botbox.json if available ---
if [ -f .botbox.json ] && command -v jq >/dev/null 2>&1; then
	MODEL=$(jq -r '.agents.reviewer.model // ""' .botbox.json)
	MAX_LOOPS=$(jq -r '.agents.reviewer.max_loops // 20' .botbox.json)
	LOOP_PAUSE=$(jq -r '.agents.reviewer.pause // 2' .botbox.json)
	CLAUDE_TIMEOUT=$(jq -r '.agents.reviewer.timeout // 600' .botbox.json)
fi

# --- Usage ---
usage() {
	cat <<EOF
Usage: reviewer-loop.sh [options] <project> [agent-name]

Reviewer agent. Picks one open review per iteration, reads the diff,
leaves comments, and votes LGTM or BLOCKED.

Options:
  --max-loops N   Max iterations (default: $MAX_LOOPS)
  --pause N       Seconds between iterations (default: $LOOP_PAUSE)
  --model M       Model for the reviewer agent (default: system default)
  -h, --help      Show this help

Arguments:
  project         Project name (required)
  agent-name      Agent identity (default: auto-generated)
EOF
	exit 0
}

# --- Parse flags ---
while [[ $# -gt 0 ]]; do
	case "$1" in
	--max-loops)
		MAX_LOOPS="$2"
		shift 2
		;;
	--pause)
		LOOP_PAUSE="$2"
		shift 2
		;;
	--model)
		MODEL="$2"
		shift 2
		;;
	-h | --help)
		usage
		;;
	--)
		shift
		break
		;;
	-*)
		echo "Unknown option: $1" >&2
		usage
		;;
	*)
		break
		;;
	esac
done

# --- Positional arguments ---
PROJECT="${1:?Usage: reviewer-loop.sh [options] <project> [agent-name]}"
shift
AGENT="${1:-$(bus generate-name)}"

echo "Reviewer:  $AGENT"
echo "Project:   $PROJECT"
echo "Max loops: $MAX_LOOPS"
echo "Pause:     ${LOOP_PAUSE}s"
echo "Model:     ${MODEL:-system default}"

# --- Confirm identity ---
bus whoami --agent "$AGENT"

# --- Refresh or stake the agent lease ---
# Try refresh first (hook may have created it), fall back to stake
if ! bus claims refresh --agent "$AGENT" "agent://$AGENT" 2>/dev/null; then
	if ! bus claims stake --agent "$AGENT" "agent://$AGENT" -m "reviewer-loop for $PROJECT"; then
		echo "Claim denied. Reviewer $AGENT is already running."
		exit 0
	fi
fi

# --- Cleanup on exit ---
cleanup() {
	bus statuses clear --agent "$AGENT" >/dev/null 2>&1 || true
	bus claims release --agent "$AGENT" "agent://$AGENT" >/dev/null 2>&1 || true
	echo "Cleanup complete for $AGENT."
}

trap cleanup EXIT

# --- Announce ---
bus send --agent "$AGENT" "$PROJECT" "Reviewer $AGENT online, starting review loop" \
	-L spawn-ack

# --- Python check ---
PYTHON_BIN=${PYTHON_BIN:-python3}
if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
	PYTHON_BIN=python
fi
if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
	echo "python is required for parsing JSON output."
	exit 1
fi

# --- Helper: check if there are reviews to process ---
has_work() {
	local inbox_count review_count

	# Check bus inbox for review-request or re-review messages
	inbox_count=$(bus inbox --agent "$AGENT" --channels "$PROJECT" --count-only --format json 2>/dev/null \
		| "$PYTHON_BIN" -c \
			'import json,sys; d=json.load(sys.stdin); print(d.get("total_unread",0) if isinstance(d,dict) else d)' \
		2>/dev/null || echo "0")

	# Check for open reviews in crit
	review_count=$(crit reviews list --format json 2>/dev/null \
		| "$PYTHON_BIN" -c \
			'import json,sys; d=json.load(sys.stdin); r=d if isinstance(d,list) else d.get("reviews",[]); print(len([x for x in r if x.get("status")=="open"]))' \
		2>/dev/null || echo "0")

	if [[ "$inbox_count" -gt 0 ]] || [[ "$review_count" -gt 0 ]]; then
		return 0
	fi
	return 1
}

# --- Main loop ---
bus statuses set --agent "$AGENT" "Starting loop" --ttl 10m

for ((i = 1; i <= MAX_LOOPS; i++)); do
	echo "--- Review loop $i/$MAX_LOOPS ---"

	if ! has_work; then
		bus statuses set --agent "$AGENT" "Idle"
		echo "No reviews pending. Exiting cleanly."
		bus send --agent "$AGENT" "$PROJECT" \
			"No reviews pending. Reviewer $AGENT signing off." \
			-L agent-idle
		break
	fi

	if ! timeout "$CLAUDE_TIMEOUT" claude ${MODEL:+--model "$MODEL"} --dangerously-skip-permissions --allow-dangerously-skip-permissions -p "$(
		cat <<EOF
You are reviewer agent "$AGENT" for project "$PROJECT".

IMPORTANT: Use --agent $AGENT on ALL bus and crit commands. Set BOTBOX_PROJECT=$PROJECT.

Execute exactly ONE review cycle, then STOP. Do not process multiple reviews.

1. INBOX:
   Run: bus inbox --agent $AGENT --channels $PROJECT --mark-read
   Note any review-request or review-response messages. Ignore task-claim, task-done, spawn-ack, etc.

2. FIND REVIEWS:
   Run: crit reviews list --format json
   Look for open reviews (status: "open"). Pick one to process.
   If no open reviews exist, say "NO_REVIEWS_PENDING" and stop.
   bus statuses set --agent $AGENT "Review: <review-id>" --ttl 30m

3. REVIEW (follow .agents/botbox/review-loop.md):
   a. Read the review and diff: crit review <id> and crit diff <id>
   b. Read the full source files changed in the diff — use absolute paths
   c. Check project config (e.g., Cargo.toml, package.json) for dependencies and settings
   d. Run static analysis if applicable (e.g., cargo clippy, oxlint) — cite warnings in comments
   e. Cross-file consistency: compare similar functions across files for uniform security/validation.
      If one function does it right and another doesn't, that's a bug.
   f. Boundary checks: trace user-supplied values through to where they're used.
      Check arithmetic for edge cases: 0, 1, MAX, negative, empty.
   g. For each issue found, comment with severity:
      - CRITICAL: Security vulnerabilities, data loss, crashes in production
      - HIGH: Correctness bugs, race conditions, resource leaks
      - MEDIUM: Error handling gaps, missing validation at boundaries
      - LOW: Code quality, naming, structure
      - INFO: Suggestions, style preferences, minor improvements
      Use: crit comment <id> "SEVERITY: <feedback>" --file <path> --line <line-or-range>
   h. Vote:
      - crit block <id> --reason "..." if any CRITICAL or HIGH issues exist
      - crit lgtm <id> if no CRITICAL or HIGH issues

4. ANNOUNCE:
   bus send --agent $AGENT $PROJECT "Review complete: <review-id> — <LGTM|BLOCKED>" -L review-done

5. RE-REVIEW (if a review-response message indicates the author addressed feedback):
   The author's fixes are in their workspace, not the main branch.
   Check the review-response bus message for the workspace path.
   Read files from the workspace path (e.g., .workspaces/\$WS/src/...).
   Verify fixes against original issues — read actual code, don't just trust replies.
   Run static analysis in the workspace: cd <workspace-path> && <analysis-command>
   If all resolved: crit lgtm <id>. If not: reply on threads explaining what's still wrong.

Key rules:
- Process exactly one review per cycle, then STOP.
- Focus on security and correctness. Ground findings in evidence — compiler output,
  documentation, or source code — not assumptions about API behavior.
- All bus and crit commands use --agent $AGENT.
- STOP after completing one review. Do not loop.
EOF
	)"; then
		exit_code=$?
		if [[ $exit_code -eq 124 ]]; then
			echo "Claude timed out after ${CLAUDE_TIMEOUT}s on review loop $i"
			bus send --agent "$AGENT" "$PROJECT" \
				"Reviewer Claude iteration timed out after ${CLAUDE_TIMEOUT}s on loop $i" \
				-L tool-issue >/dev/null 2>&1 || true
		else
			echo "Claude exited with code $exit_code on review loop $i"
		fi
	fi

	sleep "$LOOP_PAUSE"
done

# --- Shutdown ---
bus send --agent "$AGENT" "$PROJECT" \
	"Reviewer $AGENT shutting down after $((i - 1)) loops." \
	-L agent-shutdown
echo "Reviewer $AGENT finished."
