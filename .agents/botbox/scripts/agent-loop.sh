#!/usr/bin/env bash
set -euo pipefail

# --- Defaults ---
MAX_LOOPS=20
LOOP_PAUSE=2
CLAUDE_TIMEOUT=600
MODEL=""
PROJECT=""
AGENT=""
PUSH_MAIN=false

# --- Load config from .botbox.json if available ---
if [ -f .botbox.json ] && command -v jq >/dev/null 2>&1; then
	MODEL=$(jq -r '.agents.worker.model // ""' .botbox.json)
	CLAUDE_TIMEOUT=$(jq -r '.agents.worker.timeout // 600' .botbox.json)
	PUSH_MAIN=$(jq -r '.pushMain // false' .botbox.json)
fi

# --- Usage ---
usage() {
	cat <<EOF
Usage: agent-loop.sh [options] <project> [agent-name]

Worker agent. Picks one task per iteration, implements it, requests review,
and finishes. Sequential — one bead at a time.

Options:
  --max-loops N   Max iterations (default: $MAX_LOOPS)
  --pause N       Seconds between iterations (default: $LOOP_PAUSE)
  --model M       Model for the worker agent (default: system default)
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
PROJECT="${1:?Usage: agent-loop.sh [options] <project> [agent-name]}"
shift
AGENT="${1:-$(bus generate-name)}"

echo "Agent:     $AGENT"
echo "Project:   $PROJECT"
echo "Max loops: $MAX_LOOPS"
echo "Pause:     ${LOOP_PAUSE}s"
echo "Model:     ${MODEL:-system default}"

# --- Confirm identity ---
bus whoami --agent "$AGENT"

# --- Refresh or stake the agent lease ---
# Try refresh first (hook may have created it), fall back to stake
if ! bus claims refresh --agent "$AGENT" "agent://$AGENT" 2>/dev/null; then
	if ! bus claims stake --agent "$AGENT" "agent://$AGENT" -m "worker-loop for $PROJECT"; then
		echo "Claim denied. Agent $AGENT is already running."
		exit 0
	fi
fi

# --- Cleanup on exit ---
cleanup() {
	bus claims release --agent "$AGENT" "agent://$AGENT" >/dev/null 2>&1 || true
	bus claims release --agent "$AGENT" --all >/dev/null 2>&1 || true
	br sync --flush-only >/dev/null 2>&1 || true
	echo "Cleanup complete for $AGENT."
}

trap cleanup EXIT

# --- Announce ---
bus send --agent "$AGENT" "$PROJECT" "Agent $AGENT online, starting worker loop" \
	-L mesh -L spawn-ack

# --- Python check ---
PYTHON_BIN=${PYTHON_BIN:-python3}
if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
	PYTHON_BIN=python
fi
if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
	echo "python is required for parsing JSON output."
	exit 1
fi

# --- Helper: check if there is work ---
has_work() {
	local inbox_count ready_count claims_count

	# Check for active claims (in-progress beads awaiting review)
	claims_count=$(bus claims --agent "$AGENT" --mine --format json 2>/dev/null \
		| "$PYTHON_BIN" -c \
			'import json,sys; d=json.load(sys.stdin); print(len(d.get("claims",[])))' \
		2>/dev/null || echo "0")

	inbox_count=$(bus inbox --agent "$AGENT" --channels "$PROJECT" --count-only --format json 2>/dev/null \
		| "$PYTHON_BIN" -c \
			'import json,sys; d=json.load(sys.stdin); print(d.get("total_unread",0) if isinstance(d,dict) else d)' \
		2>/dev/null || echo "0")

	ready_count=$(br ready --json 2>/dev/null \
		| "$PYTHON_BIN" -c \
			'import json,sys; d=json.load(sys.stdin); print(len(d) if isinstance(d,list) else len(d.get("issues",d.get("beads",[]))))' \
		2>/dev/null || echo "0")

	if [[ "$claims_count" -gt 0 ]] || [[ "$inbox_count" -gt 0 ]] || [[ "$ready_count" -gt 0 ]]; then
		return 0
	fi
	return 1
}

# --- Main loop ---
for ((i = 1; i <= MAX_LOOPS; i++)); do
	echo "--- Loop $i/$MAX_LOOPS ---"

	if ! has_work; then
		echo "No work available. Exiting cleanly."
		bus send --agent "$AGENT" "$PROJECT" \
			"No work remaining. Agent $AGENT signing off." \
			-L mesh -L agent-idle
		break
	fi

	if ! timeout "$CLAUDE_TIMEOUT" claude ${MODEL:+--model "$MODEL"} --dangerously-skip-permissions --allow-dangerously-skip-permissions -p "$(
		cat <<EOF
You are worker agent "$AGENT" for project "$PROJECT".

IMPORTANT: Use --agent $AGENT on ALL bus and crit commands. Use --actor $AGENT on ALL mutating br commands (create, update, close, comments add, dep add, label add). Also use --owner $AGENT on br create and --author $AGENT on br comments add. Set BOTBOX_PROJECT=$PROJECT.

Execute exactly ONE cycle of the worker loop. Complete one task (or determine there is no work),
then STOP. Do not start a second task — the outer loop handles iteration.

0. RESUME CHECK (do this FIRST):
   Run: bus claims --agent $AGENT --mine
   If you hold a bead:// claim, you have an in-progress bead from a previous iteration.
   - Run: br comments <bead-id> to understand what was done before and what remains.
   - Look for workspace info in comments (workspace name and path).
   - If a "Review requested: <review-id>" comment exists:
     * Check review status: crit review <review-id>
     * If LGTM (approved): proceed to FINISH (step 7) — merge the review and close the bead.
     * If BLOCKED (changes requested): follow .agents/botbox/review-response.md to fix issues
       in the workspace, re-request review, then STOP this iteration.
     * If PENDING (no votes yet): STOP this iteration. Wait for the reviewer.
   - If no review comment (work was in progress when session ended):
     * Read the workspace code to see what's already done.
     * Complete the remaining work in the EXISTING workspace — do NOT create a new one.
     * After completing: br comments add --actor $AGENT --author $AGENT <id> "Resumed and completed: <what you finished>".
     * Then proceed to step 6 (REVIEW REQUEST) or step 7 (FINISH).
   If no active claims: proceed to step 1 (INBOX).

1. INBOX (do this before triaging):
   Run: bus inbox --agent $AGENT --channels $PROJECT --mark-read
   For each message:
   - Task request (-L task-request or asks for work): create a bead with br create.
   - Status check or question: reply on bus, do NOT create a bead.
   - Feedback (-L feedback): review referenced beads, reply with triage result.
   - Announcements from other agents ("Working on...", "Completed...", "online"): ignore, no action.
   - Duplicate of existing bead: do NOT create another bead, note it covers the request.

2. TRIAGE: Check br ready. If no ready beads and inbox created none, say "NO_WORK_AVAILABLE" and stop.
   GROOM each ready bead (br show <id>): ensure clear title, description with acceptance criteria
   and testing strategy, appropriate priority. Fix anything missing, comment what you changed.
   Use bv --robot-next to pick exactly one small task. If the task is large, break it down with
   br create + br dep add, then bv --robot-next again. If a bead is claimed
   (bus claims check --agent $AGENT "bead://$PROJECT/<id>"), skip it.

3. START: br update --actor $AGENT <id> --status=in_progress.
   bus claims stake --agent $AGENT "bead://$PROJECT/<id>" -m "<id>".
   Create workspace: run maw ws create --random. Note the workspace name AND absolute path
   from the output (e.g., name "frost-castle", path "/abs/path/.workspaces/frost-castle").
   Store the name as WS and the absolute path as WS_PATH.
   IMPORTANT: All file operations (Read, Write, Edit) must use the absolute WS_PATH.
   For bash commands: cd \$WS_PATH && <command>. For jj commands: maw ws jj \$WS <args>.
   Do NOT cd into the workspace and stay there — the workspace is destroyed during finish.
   bus claims stake --agent $AGENT "workspace://$PROJECT/\$WS" -m "<id>".
   br comments add --actor $AGENT --author $AGENT <id> "Started in workspace \$WS (\$WS_PATH)".
   Announce: bus send --agent $AGENT $PROJECT "Working on <id>: <title>" -L mesh -L task-claim.

4. WORK: br show <id>, then implement the task in the workspace.
   Add at least one progress comment: br comments add --actor $AGENT --author $AGENT <id> "Progress: ...".

5. STUCK CHECK: If same approach tried twice, info missing, or tool fails repeatedly — you are
   stuck. br comments add --actor $AGENT --author $AGENT <id> "Blocked: <details>".
   bus send --agent $AGENT $PROJECT "Stuck on <id>: <reason>" -L mesh -L task-blocked.
   br update --actor $AGENT <id> --status=blocked.
   Release: bus claims release --agent $AGENT "bead://$PROJECT/<id>".
   Stop this cycle.

6. REVIEW REQUEST:
   Describe the change: maw ws jj \$WS describe -m "<id>: <summary>".
   Create review: crit reviews create --agent $AGENT --title "<title>" --description "<summary>".
   Add bead comment: br comments add --actor $AGENT --author $AGENT <id> "Review requested: <review-id>, workspace: \$WS (\$WS_PATH)".
   Announce: bus send --agent $AGENT $PROJECT "Review requested: <review-id> for <id>: <title>" -L mesh -L review-request.
   Do NOT close the bead. Do NOT merge the workspace. Do NOT release claims.
   STOP this iteration. The reviewer will process the review.

7. FINISH (only reached after LGTM from step 0, or if no review needed):
   IMPORTANT: Run ALL finish commands from the project root, not from inside the workspace.
   If your shell is cd'd into .workspaces/, cd back to the project root first.
   If a review was conducted:
     crit reviews merge <review-id> --agent $AGENT.
   br comments add --actor $AGENT --author $AGENT <id> "Completed by $AGENT".
   br close --actor $AGENT <id> --reason="Completed" --suggest-next.
   maw ws merge \$WS --destroy (if conflict, preserve and announce).
   bus claims release --agent $AGENT --all.
   br sync --flush-only.$([ "$PUSH_MAIN" = "true" ] && echo '
   Push to GitHub: jj bookmark set main -r @- && jj git push (if fails, announce issue).')
   bus send --agent $AGENT $PROJECT "Completed <id>: <title>" -L mesh -L task-done.

Key rules:
- Exactly one small task per cycle.
- Always finish or release before stopping.
- If claim denied, pick something else.
- All bus and crit commands use --agent $AGENT.
- All file operations use the absolute workspace path from maw ws create output. Do NOT cd into the workspace and stay there.
- Run br commands (br update, br close, br comments, br sync) from the project root, NOT from .workspaces/WS/.
- If a tool behaves unexpectedly, report it: bus send --agent $AGENT $PROJECT "Tool issue: <details>" -L mesh -L tool-issue.
- STOP after completing one task or determining no work. Do not loop.
EOF
	)"; then
		exit_code=$?
		if [[ $exit_code -eq 124 ]]; then
			echo "Claude timed out after ${CLAUDE_TIMEOUT}s on loop $i"
			bus send --agent "$AGENT" "$PROJECT" \
				"Claude iteration timed out after ${CLAUDE_TIMEOUT}s on loop $i" \
				-L mesh -L tool-issue >/dev/null 2>&1 || true
		else
			echo "Claude exited with code $exit_code on loop $i"
		fi
	fi

	# Full sync between iterations so has_work() sees bead closures
	# from the previous claude session (which only does --flush-only).
	br sync 2>/dev/null || true

	sleep "$LOOP_PAUSE"
done

# --- Final sync and shutdown ---
br sync 2>/dev/null || true
bus send --agent "$AGENT" "$PROJECT" \
	"Agent $AGENT shutting down after $((i - 1)) loops." \
	-L mesh -L agent-shutdown
echo "Agent $AGENT finished."
