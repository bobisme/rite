#!/usr/bin/env bash
set -euo pipefail

# --- Defaults ---
MAX_LOOPS=20
LOOP_PAUSE=2
CLAUDE_TIMEOUT=900
WORKER_MODEL=haiku
WORKER_TIMEOUT=600
MODEL=opus
REVIEW=false
PROJECT=""
AGENT=""
PUSH_MAIN=false

# --- Load config from .botbox.json if available ---
if [ -f .botbox.json ] && command -v jq >/dev/null 2>&1; then
	REVIEW=$(jq -r '.review.enabled // false' .botbox.json)
	MODEL=$(jq -r '.agents.dev.model // "opus"' .botbox.json)
	MAX_LOOPS=$(jq -r '.agents.dev.max_loops // 20' .botbox.json)
	LOOP_PAUSE=$(jq -r '.agents.dev.pause // 2' .botbox.json)
	CLAUDE_TIMEOUT=$(jq -r '.agents.dev.timeout // 900' .botbox.json)
	WORKER_MODEL=$(jq -r '.agents.worker.model // "haiku"' .botbox.json)
	WORKER_TIMEOUT=$(jq -r '.agents.worker.timeout // 600' .botbox.json)
	PUSH_MAIN=$(jq -r '.pushMain // false' .botbox.json)
fi

# --- Usage ---
usage() {
	cat <<EOF
Usage: dev-loop.sh [options] <project> [agent-name]

Lead dev orchestrator. Triages ready beads, dispatches workers in parallel
when appropriate, monitors progress, and merges completed work.

Options:
  --max-loops N        Max iterations (default: $MAX_LOOPS)
  --pause N            Seconds between iterations (default: $LOOP_PAUSE)
  --worker-model M     Default model for dispatched workers (default: $WORKER_MODEL)
  --worker-timeout N   Seconds before a worker is considered stuck (default: $WORKER_TIMEOUT)
  --model M            Model for the dev agent itself (default: $MODEL)
  --review             Enable crit reviews for worker output
  -h, --help           Show this help

Arguments:
  project              Project name (required)
  agent-name           Agent identity (default: auto-generated from project)
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
	--worker-model)
		WORKER_MODEL="$2"
		shift 2
		;;
	--worker-timeout)
		WORKER_TIMEOUT="$2"
		shift 2
		;;
	--model)
		MODEL="$2"
		shift 2
		;;
	--review)
		REVIEW=true
		shift
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
PROJECT="${1:?Usage: dev-loop.sh [options] <project> [agent-name]}"
shift
AGENT="${1:-}"

# --- Resolve agent identity ---
if [[ -z "$AGENT" ]]; then
	AGENT=$(bus whoami --suggest-project-suffix=dev 2>/dev/null) || {
		echo "Failed to resolve agent identity." >&2
		exit 1
	}
fi

echo "Dev agent: $AGENT"
echo "Project:   $PROJECT"
echo "Max loops: $MAX_LOOPS"
echo "Pause:     ${LOOP_PAUSE}s"
echo "Model:     $MODEL"
echo "Worker model: $WORKER_MODEL"
echo "Worker timeout: ${WORKER_TIMEOUT}s"
echo "Review:    $REVIEW"

# --- Confirm identity ---
bus whoami --agent "$AGENT"

# --- Refresh or stake the agent lease ---
# Try refresh first (hook may have created it), fall back to stake
if ! bus claims refresh --agent "$AGENT" "agent://$AGENT" 2>/dev/null; then
	if ! bus claims stake --agent "$AGENT" "agent://$AGENT" -m "dev-loop for $PROJECT"; then
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
bus send --agent "$AGENT" "$PROJECT" "Dev agent $AGENT online, starting dev loop" \
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

	# Check for active claims (in-progress beads, dispatched workers)
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
	echo "--- Dev loop $i/$MAX_LOOPS ---"

	if ! has_work; then
		echo "No work available. Exiting cleanly."
		bus send --agent "$AGENT" "$PROJECT" \
			"No work remaining. Dev agent $AGENT signing off." \
			-L mesh -L agent-idle
		break
	fi

	if ! timeout "$CLAUDE_TIMEOUT" claude --model "$MODEL" --dangerously-skip-permissions --allow-dangerously-skip-permissions -p "$(
		cat <<EOF
You are lead dev agent "$AGENT" for project "$PROJECT".

IMPORTANT: Use --agent $AGENT on ALL bus and crit commands. Use --actor $AGENT on ALL mutating br commands (create, update, close, comments add, dep add, label add). Also use --owner $AGENT on br create and --author $AGENT on br comments add. Set BOTBOX_PROJECT=$PROJECT.

Your role is the LEAD DEVELOPER — you triage, dispatch, monitor, and merge.
Execute exactly ONE iteration of the dev loop, then STOP.

Configuration:
- WORKER_MODEL=$WORKER_MODEL
- WORKER_TIMEOUT=$WORKER_TIMEOUT
- REVIEW=$REVIEW

## 0. RESUME CHECK (do this FIRST)

Run: bus claims --agent $AGENT --mine
If you hold bead:// claims, you have in-progress work from a previous iteration.
For each bead:// claim:
  - Run: br comments <bead-id>
  - Look for "Dispatched worker <name>" comments.
  - If a worker was dispatched:
    * Check if the worker announced completion: bus inbox --agent $AGENT --channels $PROJECT --mark-read
      Look for messages from that worker with -L task-done.
    * Also check bead comments for worker progress updates.
    * Also check the worker log: read /tmp/<worker-name>.log (tail the last 50 lines).
    * If worker completed: proceed to MERGE (step 6).
    * If worker still running and within timeout ($WORKER_TIMEOUT seconds): note it, move on.
    * If worker appears stuck (no progress, past timeout): log it, release claim, mark bead blocked.
  - If no dispatch comment (you're doing it yourself):
    * Run: br comments <bead-id> to see what was done and what remains.
    * Look for workspace info in comments (workspace name and path).
    * If a "Review requested" comment exists: check review status (LGTM/BLOCKED/PENDING).
    * If no review comment (work was in progress when session ended):
      Read workspace code, complete remaining work in the EXISTING workspace (do NOT create new one).
      After completing: br comments add --actor $AGENT --author $AGENT <id> "Resumed and completed: <what you finished>".
      Then proceed to merge or review.
If no active claims: proceed to step 1.

## 1. INBOX

Run: bus inbox --agent $AGENT --channels $PROJECT --mark-read
For each message:
- Task request (-L task-request or asks for work): create a bead with br create.
- Worker completion (-L task-done): note which bead was completed, proceed to merge in step 6.
- Status check or question: reply on bus, do NOT create a bead.
- Feedback (-L feedback): review referenced beads, reply with triage result.
- Announcements ("Working on...", "online"): ignore.
- Duplicate of existing bead: do NOT create another bead.

## 2. TRIAGE

Run: br ready
If no ready beads, no pending dispatched workers, and inbox created none: say "NO_WORK_AVAILABLE" and stop.

GROOM each ready bead (br show <id>): ensure clear title, description with acceptance criteria
and testing strategy, appropriate priority. Fix anything missing, comment what you changed.

Count the number of independent ready beads (not blocked by each other).
Check which are already claimed: bus claims check --agent $AGENT "bead://$PROJECT/<id>". Skip claimed ones.

## 3. DISPATCH DECISION

Based on count of unclaimed, independent ready beads:

- 0 ready beads (but dispatched workers pending): just monitor, skip to step 6.
- 1 ready bead: do it yourself sequentially (follow steps 4a below).
- 2+ ready beads: dispatch workers in parallel (follow steps 4b below).

## 4a. SEQUENTIAL (1 bead — do it yourself)

Same as the standard worker loop:
1. br update --actor $AGENT <id> --status=in_progress
2. bus claims stake --agent $AGENT "bead://$PROJECT/<id>" -m "<id>"
3. maw ws create --random — note workspace NAME and absolute PATH
4. bus claims stake --agent $AGENT "workspace://$PROJECT/\$WS" -m "<id>"
5. br comments add --actor $AGENT --author $AGENT <id> "Started in workspace \$WS (\$WS_PATH)"
6. Announce: bus send --agent $AGENT $PROJECT "Working on <id>: <title>" -L mesh -L task-claim
7. Implement the task. All file operations use absolute WS_PATH.
   For jj: maw ws jj \$WS <args>. Do NOT cd into workspace and stay there.
8. br comments add --actor $AGENT --author $AGENT <id> "Progress: ..."
9. Describe: maw ws jj \$WS describe -m "<id>: <summary>"

If REVIEW is true:
  10. Create review: crit reviews create --agent $AGENT --title "<title>" --description "<summary>"
  11. br comments add --actor $AGENT --author $AGENT <id> "Review requested: <review-id>, workspace: \$WS (\$WS_PATH)"
  12. bus send --agent $AGENT $PROJECT "Review requested: <review-id> for <id>" -L mesh -L review-request
  13. STOP this iteration — wait for reviewer.

If REVIEW is false:
  10. Merge: maw ws merge \$WS --destroy
  11. br close --actor $AGENT <id> --reason="Completed"
  12. bus claims release --agent $AGENT --all
  13. br sync --flush-only$([ "$PUSH_MAIN" = "true" ] && echo '
  14. Push to GitHub: jj bookmark set main -r @- && jj git push (if fails, announce issue)')
  $([ "$PUSH_MAIN" = "true" ] && echo "15" || echo "14"). bus send --agent $AGENT $PROJECT "Completed <id>: <title>" -L mesh -L task-done

## 4b. PARALLEL DISPATCH (2+ beads)

For EACH independent ready bead, assess and dispatch:

### Model Selection
Read each bead (br show <id>) and select a model based on complexity:
- **$WORKER_MODEL** (default): Use for most tasks unless signals suggest otherwise.
- **haiku**: Clear acceptance criteria, small scope (<~50 lines), well-groomed. E.g., add endpoint, fix typo, update config.
- **sonnet**: Multiple files, design decisions, moderate complexity. E.g., refactor module, add feature with tests.
- **opus**: Deep debugging, architecture changes, subtle correctness issues. E.g., fix race condition, redesign data flow.

### For each bead being dispatched:
1. maw ws create --random — note NAME and PATH
2. bus generate-name — get a worker identity
3. br update --actor $AGENT <id> --status=in_progress
4. bus claims stake --agent $AGENT "bead://$PROJECT/<id>" -m "dispatched to <worker-name>"
5. bus claims stake --agent $AGENT "workspace://$PROJECT/\$WS" -m "<id>"
6. br comments add --actor $AGENT --author $AGENT <id> "Dispatched worker <worker-name> (model: <model>) in workspace \$WS (\$WS_PATH)"
7. bus send --agent $AGENT $PROJECT "Dispatching <worker-name> for <id>: <title>" -L mesh -L task-claim

8. Launch worker as a BACKGROUND process:

   claude --model <model> -p "<worker-prompt>" \\
     --dangerously-skip-permissions --allow-dangerously-skip-permissions \\
     > /tmp/<worker-name>.log 2>&1 &

   Worker prompt MUST include:
   - Worker identity (--agent <worker-name>)
   - Project name ($PROJECT)
   - Bead ID and title
   - Workspace name and absolute path
   - Instructions to implement, verify, and announce — but NOT close/merge/release

### Worker prompt template:

"You are worker agent <worker-name> for project $PROJECT.
Use --agent <worker-name> on ALL bus commands. Use --actor <worker-name> on ALL mutating br commands. Use --author <worker-name> on br comments add.

Your task: bead <id> — <title>
Workspace: <ws-name> at <ws-path>

1. Read the bead: br show <id>
2. Implement the task. All file operations use absolute path <ws-path>.
   For jj: maw ws jj <ws-name> <args>.
3. Post a progress comment: br comments add --actor <worker-name> --author <worker-name> <id> 'Progress: <what you did>'
4. Verify your work (run tests, lints, or checks as appropriate for the project).
5. Describe the change: maw ws jj <ws-name> describe -m '<id>: <summary>'
6. Announce completion: bus send --agent <worker-name> $PROJECT 'Worker <worker-name> completed <id>: <title>' -L mesh -L task-done

Do NOT close the bead, merge the workspace, or release claims. The lead dev handles that."

IMPORTANT: Dispatch ALL workers BEFORE waiting for any to complete.

## 5. MONITOR

After dispatching (or if resuming with dispatched workers):
- Poll: bus inbox --agent $AGENT --channels $PROJECT --mark-read
- Check bead comments for each dispatched bead: br comments <id>
- Check worker logs: read /tmp/<worker-name>.log (tail last 50 lines)
- Wait briefly (sleep 15-30 seconds between checks) and re-poll.
- Continue monitoring until all dispatched workers have announced -L task-done or timeout is reached.
- If a worker appears stuck past $WORKER_TIMEOUT seconds with no progress:
  br comments add --actor $AGENT --author $AGENT <id> "Worker <name> timed out"
  br update --actor $AGENT <id> --status=blocked
  bus claims release --agent $AGENT "bead://$PROJECT/<id>"
  Continue with other workers.

## 6. MERGE

For each completed worker:
1. Verify: maw ws jj <ws-name> diff
2. Merge: maw ws merge <ws-name> --destroy
   If conflict: br comments add --actor $AGENT --author $AGENT <id> "Merge conflict in <ws>, preserving workspace"
   Skip destroy, move to next. Announce the conflict.
3. br close --actor $AGENT <id> --reason="Completed by <worker-name>"
4. bus claims release --agent $AGENT "bead://$PROJECT/<id>"
5. bus send --agent $AGENT $PROJECT "Merged <id>: <title>" -L mesh -L task-done

After all merges: br sync --flush-only$([ "$PUSH_MAIN" = "true" ] && echo '
Then push to GitHub: jj bookmark set main -r @- && jj git push (if fails, announce issue)')

## 7. REVIEW (if REVIEW=true)

After merging, if review is enabled:
- Check if a reviewer is already running: bus claims check --agent $AGENT "agent://reviewer"
- If reviews are needed and no reviewer is running:
  Consider spawning reviewer-loop.sh (note: this is handled in subsequent iterations)
- This is iteration-aware — review happens across iterations, not blocking this one.

## Key Rules

- You are the lead dev — coordinate and dispatch, implement only when there's 1 bead.
- Dispatch ALL workers before monitoring. True parallel dispatch.
- Workers use their own --agent identity. You use --agent $AGENT.
- All bus, br, crit, and maw commands use --agent $AGENT (except worker prompts which use their own identity).
- The dev agent holds bead claims on behalf of workers. Workers do NOT claim beads.
- Workers do NOT close beads, merge workspaces, or release claims. You do.
- If a tool behaves unexpectedly: bus send --agent $AGENT $PROJECT "Tool issue: <details>" -L mesh -L tool-issue
- STOP after completing one iteration. Do not loop — the outer bash loop handles iteration.
EOF
	)"; then
		exit_code=$?
		if [[ $exit_code -eq 124 ]]; then
			echo "Claude timed out after ${CLAUDE_TIMEOUT}s on dev loop $i"
			bus send --agent "$AGENT" "$PROJECT" \
				"Dev agent Claude iteration timed out after ${CLAUDE_TIMEOUT}s on loop $i" \
				-L mesh -L tool-issue >/dev/null 2>&1 || true
		else
			echo "Claude exited with code $exit_code on dev loop $i"
		fi
	fi

	# Full sync between iterations
	br sync 2>/dev/null || true

	sleep "$LOOP_PAUSE"
done

# --- Final sync and shutdown ---
br sync 2>/dev/null || true
bus send --agent "$AGENT" "$PROJECT" \
	"Dev agent $AGENT shutting down after $((i - 1)) loops." \
	-L mesh -L agent-shutdown
echo "Dev agent $AGENT finished."
