#!/usr/bin/env node
import { spawn } from 'child_process';
import { readFile, stat, appendFile, truncate } from 'fs/promises';
import { existsSync } from 'fs';
import { parseArgs } from 'util';

// --- Defaults ---
let MAX_LOOPS = 20;
let LOOP_PAUSE = 2;
let CLAUDE_TIMEOUT = 600;
let MODEL = '';
let WORKER_MODEL = '';
let PROJECT = '';
let AGENT = '';
let PUSH_MAIN = false;
let REVIEW = true;
let MISSIONS_ENABLED = true;
let MAX_MISSION_WORKERS = 4;
let MAX_MISSION_CHILDREN = 12;
let CHECKPOINT_INTERVAL_SEC = 30;

// --- Load config from .botbox.json ---
async function loadConfig() {
	if (existsSync('.botbox.json')) {
		try {
			const config = JSON.parse(await readFile('.botbox.json', 'utf-8'));
			const project = config.project || {};
			const agents = config.agents || {};
			const dev = agents.dev || {};
			const worker = agents.worker || {};

			// Project identity (can be overridden by CLI args)
			PROJECT = project.channel || project.name || '';
			AGENT = project.defaultAgent || project.default_agent || '';

			// Agent settings
			MODEL = dev.model || '';
			WORKER_MODEL = worker.model || '';
			CLAUDE_TIMEOUT = dev.timeout || 600;
			PUSH_MAIN = config.pushMain || false;
			REVIEW = config.review?.enabled ?? true;

			// Mission coordination config
			let missions = agents.dev?.missions || {};
			MISSIONS_ENABLED = missions.enabled ?? true;
			MAX_MISSION_WORKERS = missions.maxWorkers ?? 4;
			MAX_MISSION_CHILDREN = missions.maxChildren ?? 12;
			CHECKPOINT_INTERVAL_SEC = missions.checkpointIntervalSec ?? 30;
		} catch (err) {
			console.error('Warning: Failed to load .botbox.json:', err.message);
		}
	}
}

// --- Parse CLI args ---
function parseCliArgs() {
	const { values, positionals } = parseArgs({
		options: {
			'max-loops': { type: 'string' },
			pause: { type: 'string' },
			model: { type: 'string' },
			review: { type: 'boolean' },
			help: { type: 'boolean', short: 'h' },
		},
		allowPositionals: true,
	});

	if (values.help) {
		console.log(`Usage: dev-loop.mjs [options] <project> [agent-name]

Lead dev agent. Triages inbox, dispatches work to multiple workers in parallel
when appropriate, monitors progress, merges completed work.

Options:
  --max-loops N   Max iterations (default: ${MAX_LOOPS})
  --pause N       Seconds between iterations (default: ${LOOP_PAUSE})
  --model M       Model for the lead dev (default: system default)
  --review        Enable code review (default: ${REVIEW})
  --no-review     Disable code review
  -h, --help      Show this help

Arguments:
  project         Project name (default: from .botbox.json)
  agent-name      Agent identity (default: from .botbox.json or auto-generated)`);
		process.exit(0);
	}

	if (values['max-loops']) MAX_LOOPS = parseInt(values['max-loops'], 10);
	if (values.pause) LOOP_PAUSE = parseInt(values.pause, 10);
	if (values.model) MODEL = values.model;
	if (values.review !== undefined) REVIEW = values.review;

	// CLI args override config values
	if (positionals.length >= 1) {
		PROJECT = positionals[0];
	}
	if (positionals.length >= 2) {
		AGENT = positionals[1];
	}

	// Require project (either from CLI or config)
	if (!PROJECT) {
		console.error('Error: Project name required (provide as argument or configure in .botbox.json)');
		console.error('Usage: dev-loop.mjs [options] <project> [agent-name]');
		process.exit(1);
	}
}

// --- Helper: get commits on main since origin ---
async function getCommitsSinceOrigin() {
	try {
		const { stdout } = await runInDefault('jj', [
			'log',
			'-r',
			'main@origin..main',
			'--no-graph',
			'--template',
			'commit_id.short() ++ " " ++ description.first_line() ++ "\\n"',
		]);
		return stdout.trim().split('\n').filter(Boolean);
	} catch {
		return [];
	}
}

// --- Helper: run command and get output ---
async function runCommand(cmd, args = []) {
	return new Promise((resolve, reject) => {
		const proc = spawn(cmd, args);
		let stdout = '';
		let stderr = '';

		proc.stdout?.on('data', (data) => (stdout += data));
		proc.stderr?.on('data', (data) => (stderr += data));

		proc.on('close', (code) => {
			if (code === 0) resolve({ stdout: stdout.trim(), stderr: stderr.trim() });
			else reject(new Error(`${cmd} exited with code ${code}: ${stderr}`));
		});
	});
}

// --- Helper: run command in default workspace (for br, bv, jj on main) ---
function runInDefault(cmd, args = []) {
	return runCommand('maw', ['exec', 'default', '--', cmd, ...args]);
}

// --- Helper: generate agent name if not provided ---
async function getAgentName() {
	if (AGENT) return AGENT;
	try {
		const { stdout } = await runCommand('bus', ['generate-name']);
		return stdout.trim();
	} catch (err) {
		console.error('Error generating agent name:', err.message);
		process.exit(1);
	}
}

// --- Helper: check for unfinished work owned by this agent ---
async function getUnfinishedBeads() {
	try {
		const result = await runInDefault('br', ['list', '--status', 'in_progress', '--assignee', AGENT, '--json']);
		const beads = JSON.parse(result.stdout || '[]');
		return Array.isArray(beads) ? beads : [];
	} catch (err) {
		console.error('Error checking for unfinished beads:', err.message);
		return [];
	}
}

// --- Helper: check if a review is pending (don't run Claude, just wait) ---
async function hasPendingReview() {
	let unfinished = await getUnfinishedBeads();
	for (let bead of unfinished) {
		try {
			let result = await runInDefault('br', ['comments', bead.id, '--json']);
			let comments = JSON.parse(result.stdout || '[]');
			let arr = Array.isArray(comments) ? comments : comments.comments || [];

			// Look for "Review created:" or "Review requested:" comment
			let hasReview = arr.some(
				(/** @type {any} */ c) =>
					c.body?.includes('Review created:') ||
					c.body?.includes('Review requested:') ||
					c.content?.includes('Review created:') ||
					c.content?.includes('Review requested:'),
			);
			if (!hasReview) continue;

			// Check if bead was already completed (has "Completed" comment)
			let hasCompleted = arr.some(
				(/** @type {any} */ c) =>
					c.body?.includes('Completed by') || c.content?.includes('Completed by'),
			);
			if (hasCompleted) continue;

			// Has a review comment but no completion — review is still pending
			return bead.id;
		} catch {
			// Can't read comments, skip
		}
	}
	return null;
}

// --- Helper: check if there is work ---
async function hasWork() {
	try {
		// Check for unfinished beads owned by this agent (crash recovery)
		const unfinished = await getUnfinishedBeads();
		if (unfinished.length > 0) return true;

		// Check claims (dispatched workers, in-progress beads, pending reviews)
		const claimsResult = await runCommand('bus', [
			'claims',
			'--agent',
			AGENT,
			'list',
			'--mine',
			'--format',
			'json',
		]);
		const claims = JSON.parse(claimsResult.stdout || '{}');
		const claimList = claims.claims || [];
		// bead:// or workspace:// claims mean active work (don't count agent:// identity claim)
		const workClaims = claimList.filter(
			(/** @type {any} */ c) =>
				Array.isArray(c.patterns) &&
				c.patterns.some((/** @type {string} */ p) => p.startsWith('bead://') || p.startsWith('workspace://')),
		);
		if (workClaims.length > 0) return true;

		// Check inbox
		const inboxResult = await runCommand('bus', [
			'inbox',
			'--agent',
			AGENT,
			'--channels',
			PROJECT,
			'--count-only',
			'--format',
			'json',
		]);
		const inboxParsed = JSON.parse(inboxResult.stdout || '0');
		const unreadCount = typeof inboxParsed === 'number' ? inboxParsed : (inboxParsed.total_unread ?? 0);
		if (unreadCount > 0) return true;

		// Check ready beads
		const readyResult = await runInDefault('br', ['ready', '--json']);
		const ready = JSON.parse(readyResult.stdout || '[]');
		const readyCount = Array.isArray(ready) ? ready.length : ready.issues?.length || ready.beads?.length || 0;
		if (readyCount > 0) return true;

		return false;
	} catch (err) {
		console.error('Error checking for work:', err.message);
		return false;
	}
}

// --- Journal file for iteration history ---
const JOURNAL_PATH = '.agents/botbox/dev-loop.txt';

// --- Truncate journal at start of loop session ---
async function truncateJournal() {
	if (!existsSync(JOURNAL_PATH)) return;
	try {
		await truncate(JOURNAL_PATH, 0);
	} catch {
		// Ignore errors - file may not exist
	}
}

// --- Get jj change ID for current working copy ---
async function getJjChangeId() {
	try {
		const { stdout } = await runInDefault('jj', ['log', '-r', '@', '--no-graph', '-T', 'change_id.short()']);
		return stdout.trim();
	} catch {
		return null;
	}
}

// --- Append entry to journal ---
async function appendJournal(entry) {
	try {
		const timestamp = new Date().toISOString();
		const changeId = await getJjChangeId();
		let header = `\n--- ${timestamp}`;
		if (changeId) {
			header += ` | jj:${changeId}`;
		}
		header += ' ---\n';
		await appendFile(JOURNAL_PATH, header + entry.trim() + '\n');
	} catch (err) {
		console.error('Warning: Failed to append to journal:', err.message);
	}
}

// --- Read previous iteration summary ---
async function readLastIteration() {
	if (!existsSync(JOURNAL_PATH)) return null;

	try {
		const content = await readFile(JOURNAL_PATH, 'utf-8');
		const stats = await stat(JOURNAL_PATH);
		const ageMs = Date.now() - stats.mtime.getTime();
		const ageMinutes = Math.floor(ageMs / 60000);
		const ageHours = Math.floor(ageMinutes / 60);
		const ageStr = ageHours > 0 ? `${ageHours}h ago` : `${ageMinutes}m ago`;
		return { content: content.trim(), age: ageStr };
	} catch {
		return null;
	}
}

// --- Build dev lead prompt ---
function buildPrompt(lastIteration) {
	const pushMainStep = PUSH_MAIN ? '\n  14. Push to GitHub: maw push (if fails, announce issue).' : '';

	const reviewInstructions = REVIEW ? 'REVIEW is true' : 'REVIEW is false';

	const previousContext = lastIteration
		? `\n\n## PREVIOUS ITERATION (${lastIteration.age}, may be stale)\n\n${lastIteration.content}\n`
		: '';

	return `You are lead dev agent "${AGENT}" for project "${PROJECT}".

IMPORTANT: Use --agent ${AGENT} on ALL bus and crit commands. Use --actor ${AGENT} on ALL mutating br commands. Use --author ${AGENT} on br comments add. Set BOTBOX_PROJECT=${PROJECT}. ${reviewInstructions}.

CRITICAL - HUMAN MESSAGE PRIORITY: If you see a system reminder with "STOP:" showing unread botbus messages, these are from humans or other agents trying to reach you. IMMEDIATELY check inbox and respond before continuing your current task. Human questions, clarifications, and redirects take priority over heads-down work.

COMMAND PATTERN — maw exec: All br/bv commands run in the default workspace. All crit/jj commands run in their workspace.
  br/bv: maw exec default -- br <args>       or  maw exec default -- bv <args>
  crit:  maw exec \$WS -- crit <args>
  jj:    maw exec \$WS -- jj <args>
  other: maw exec \$WS -- <command>           (cargo test, etc.)
Inside \`maw exec <ws>\`, CWD is already \`ws/<ws>/\`. Use \`maw exec default -- ls src/\`, NOT \`maw exec default -- ls ws/default/src/\`.
For file reads/edits outside maw exec, use the full absolute path: \`ws/<ws>/src/...\`
${previousContext}
Execute exactly ONE dev cycle. Triage inbox, assess ready beads, either work on one yourself
or dispatch multiple workers in parallel, monitor progress, merge results. Then STOP.

At the end of your work, output:
1. A summary for the next iteration: <iteration-summary>Brief summary of what you did: beads worked on, workers dispatched, reviews processed, etc.</iteration-summary>
2. Completion signal:
   - <promise>COMPLETE</promise> if you completed work or determined no work available
   - <promise>END_OF_STORY</promise> if iteration done but more work remains

## 1. UNFINISHED WORK CHECK (do this FIRST — crash recovery)

Run: maw exec default -- br list --status in_progress --assignee ${AGENT} --json

If any in_progress beads are owned by you, you have unfinished work from a previous session that was interrupted.

For EACH unfinished bead:
1. Read the bead and its comments: maw exec default -- br show <id> and maw exec default -- br comments <id>
2. Check if you still hold claims: bus claims list --agent ${AGENT} --mine
3. Determine state:
   - If "Review created: <review-id>" comment exists:
     * Find the review: maw exec $WS -- crit review <review-id>
     * Check review status: maw exec \$WS -- crit review <review-id>
     * If LGTM (approved): Proceed to merge/finish (step 7 — use "Already reviewed and approved" path)
     * If BLOCKED (changes requested): fix the issues, then re-request review:
       1. Read threads: maw exec $WS -- crit review <review-id> (threads show inline with comments)
       2. For each unresolved thread with reviewer feedback:
          - Fix the code in the workspace (use absolute WS_PATH for file edits)
          - Reply: maw exec $WS -- crit reply <thread-id> --agent ${AGENT} "Fixed: <what you did>"
          - Resolve: maw exec $WS -- crit threads resolve <thread-id> --agent ${AGENT}
       3. Update commit: maw exec $WS -- jj describe -m "<id>: <summary> (addressed review feedback)"
       4. Re-request: maw exec $WS -- crit reviews request <review-id> --reviewers ${PROJECT}-security --agent ${AGENT}
       5. Announce: bus send --agent ${AGENT} ${PROJECT} "Review updated: <review-id> — addressed feedback @${PROJECT}-security" -L review-response
       STOP this iteration — wait for re-review
     * If PENDING (no votes yet): STOP this iteration — wait for reviewer
     * If review not found: DO NOT merge or create a new review. The reviewer may still be starting up (hooks have latency). STOP this iteration and wait. Only create a new review if the workspace was destroyed AND 3+ iterations have passed since the review comment.
   - If workspace comment exists but no review comment (work was in progress when session died):
     * Extract workspace name from comments
     * Verify workspace still exists: maw ws list
     * If workspace exists: Resume work in that workspace, complete the task, then proceed to review/finish
     * If workspace was destroyed: Re-create workspace and resume from scratch (check comments for what was done)
   - If no workspace comment (bead was just started):
     * Re-create workspace and start fresh

After handling all unfinished beads, proceed to step 2 (RESUME CHECK).

## 2. RESUME CHECK (check for active claims)

Run: bus claims list --agent ${AGENT} --mine

If you hold any claims not covered by unfinished beads in step 1:
- bead:// claim with review comment: Check crit review status. If LGTM, proceed to merge/finish.
- bead:// claim without review: Complete the work, then review or finish.
- workspace:// claims: These are dispatched workers. Skip to step 7 (MONITOR).

If no additional claims: proceed to step 3 (INBOX).

## 3. INBOX

Run: bus inbox --agent ${AGENT} --channels ${PROJECT} --mark-read

Process each message:
- Task requests (-L task-request): create beads with maw exec default -- br create
- Feedback (-L feedback): if it contains a bug report, feature request, or actionable work — create a bead. Evaluate critically: is this a real issue? Is it well-scoped? Set priority accordingly. Then acknowledge on bus.
- Status/questions: reply on bus
- Announcements ("Working on...", "Completed...", "online"): ignore, no action
- Duplicate requests: note existing bead, don't create another

## 4. TRIAGE

Run: maw exec default -- br ready --json

Count ready beads. If 0 and inbox created none: output <promise>COMPLETE</promise> and stop.
${MISSIONS_ENABLED ? `
### Mission-Aware Triage

Check for active missions (beads with label "mission" that are in_progress):
  maw exec default -- br list -l mission --status in_progress --json
${process.env.BOTBOX_MISSION ? `BOTBOX_MISSION="${process.env.BOTBOX_MISSION}" — prioritize this mission's children.` : ''}
For each active mission:
  1. List children: maw exec default -- br list -l "mission:<mission-id>" --json
  2. Count status: N open, M in_progress, K closed, J blocked
  3. If any children are ready (open, unblocked): include them in the dispatch plan
  4. If all children are done: close the mission bead (see step 5c "Closing a Mission")
  5. If children are blocked: investigate — can you unblock them? Reassign?
` : ''}
GROOM each ready bead:
- maw exec default -- br show <id> — ensure clear title, description, acceptance criteria, priority, and risk label
- Evaluate as lead dev: is this worth doing now? Is the approach sound? Reprioritize, close as wontfix, or ask for clarification if needed.
- RISK ASSESSMENT: If the bead lacks a risk label, assign one based on: blast radius, data sensitivity, reversibility, dependency uncertainty.
  - risk:low — typo fixes, doc updates, config tweaks (maw exec default -- br label add --actor ${AGENT} -l risk:low <id>)
  - risk:medium — standard features/bugs (default, no label needed)
  - risk:high — security-sensitive, data integrity, user-visible behavior changes
  - risk:critical — irreversible actions, migrations, regulated changes
  Risk can be escalated upward by any agent. Downgrades require lead approval with justification comment.
- Comment what you changed: maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "..."
- If bead is claimed (check bus claims), skip it

## EXECUTION LEVEL DECISION

After grooming, decide the execution level for this iteration:

| Level | Name | When to use |
|-------|------|-------------|
| 2 | Sequential | 1 small clear bead, or tightly coupled beads (same files, must be done in order) |
| 3 | Parallel dispatch | 2+ independent beads unrelated to each other. Different bugs, unrelated features. |
| 4 | Mission | Large task needing decomposition into related beads with shared context${MISSIONS_ENABLED ? '' : ' (DISABLED — missionsEnabled is false)'} |

**Level 3 vs 4:** Level 3 dispatches workers for *pre-existing independent beads*. Level 4 *creates the beads as part of planning* under a mission envelope with shared outcome, constraints, and sibling awareness.
${MISSIONS_ENABLED ? `
**Level 4 signals:** Task mentions multiple components, description reads like a spec/PRD, human explicitly requested coordinated work (BOTBOX_MISSION env), or beads share a common feature/goal.` : ''}
Assess bead count:
- 0 ready beads (but dispatched workers pending): just monitor, skip to step 7.
- 1 ready bead: do it yourself sequentially (follow steps 5a below).
- 2+ independent ready beads: dispatch workers in parallel (follow steps 5b below). Do NOT work on them yourself sequentially — parallel dispatch is REQUIRED.${MISSIONS_ENABLED ? `
- Large task needing decomposition: create a mission (follow step 5c below). Mission children MUST be dispatched to workers — solo sequential work defeats the purpose.` : ''}

## 5a. SEQUENTIAL (1 bead — do it yourself)

Same as the standard worker loop:
1. maw exec default -- br update --actor ${AGENT} <id> --status=in_progress --owner=${AGENT}
2. bus claims stake --agent ${AGENT} "bead://${PROJECT}/<id>" -m "<id>"
3. maw ws create --random — note workspace NAME and absolute PATH
4. bus claims stake --agent ${AGENT} "workspace://${PROJECT}/\$WS" -m "<id>"
5. maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Started in workspace \$WS (\$WS_PATH)"
6. bus statuses set --agent ${AGENT} "Working: <id>" --ttl 30m
7. Announce: bus send --agent ${AGENT} ${PROJECT} "Working on <id>: <title>" -L task-claim
8. Implement the task. All file operations use absolute WS_PATH.
   For commands in workspace: maw exec \$WS -- <command>. Do NOT cd into workspace and stay there.
9. maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Progress: ..."
10. Describe: maw exec \$WS -- jj describe -m "<id>: <summary>"

11. REVIEW (risk-aware):
  Check the bead's risk label (maw exec default -- br show <id>). No risk label = risk:medium.

  RISK:LOW (and REVIEW is true) — Lightweight review:
    Same as RISK:MEDIUM below. risk:low still gets reviewed when REVIEW is true — the reviewer can fast-track it.

  RISK:LOW (and REVIEW is false) — Self-review:
    Add self-review comment: maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Self-review (risk:low): <what you verified>"
    Proceed directly to merge/finish below.

  RISK:MEDIUM — Standard review (if REVIEW is true):
    CHECK for existing review: maw exec default -- br comments <id> | grep "Review created:"
    Create review with reviewer (if none exists): maw exec \$WS -- crit reviews create --agent ${AGENT} --title "<id>: <title>" --description "<summary>" --reviewers ${PROJECT}-security
    IMMEDIATELY record: maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Review created: <review-id> in workspace \$WS"
    Spawn reviewer via @mention: bus send --agent ${AGENT} ${PROJECT} "Review requested: <review-id> for <id> @${PROJECT}-security" -L review-request
    STOP this iteration — wait for reviewer.

  RISK:HIGH — Security review + failure-mode checklist:
    Same as risk:medium, but add to review description: "risk:high — failure-mode checklist required."
    MUST request security reviewer. STOP.

  RISK:CRITICAL — Security review + human approval:
    Same as risk:high, but also post: bus send --agent ${AGENT} ${PROJECT} "risk:critical review for <id>: requires human approval before merge" -L review-request
    STOP.

  If REVIEW is false:
    Merge: maw ws merge \$WS --destroy (produces linear squashed history and auto-moves main)
    maw exec default -- br close --actor ${AGENT} <id> --reason="Completed"
    bus send --agent ${AGENT} ${PROJECT} "Completed <id>: <title>" -L task-done
    bus claims release --agent ${AGENT} --all
    maw exec default -- br sync --flush-only${pushMainStep}

## 5b. PARALLEL DISPATCH (2+ beads)

For EACH independent ready bead, assess and dispatch:

### Model Selection
Read each bead (maw exec default -- br show <id>) and select a model based on complexity:
- **${WORKER_MODEL || 'default'}**: Use for most tasks unless signals suggest otherwise.
- **haiku**: Clear acceptance criteria, small scope (<~50 lines), well-groomed. E.g., add endpoint, fix typo, update config.
- **sonnet**: Multiple files, design decisions, moderate complexity. E.g., refactor module, add feature with tests.
- **opus**: Deep debugging, architecture changes, subtle correctness issues. E.g., fix race condition, redesign data flow.

### For each bead being dispatched:
1. maw ws create --random — note NAME and PATH
2. bus generate-name — get a worker identity
3. maw exec default -- br update --actor ${AGENT} <id> --status=in_progress --owner=${AGENT}
4. bus claims stake --agent ${AGENT} "bead://${PROJECT}/<id>" -m "dispatched to <worker-name>"
5. bus claims stake --agent ${AGENT} "workspace://${PROJECT}/\$WS" -m "<id>"
6. maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Dispatched worker <worker-name> (model: <model>) in workspace \$WS (\$WS_PATH)"
7. bus statuses set --agent ${AGENT} "Dispatch: <id>" --ttl 5m
8. bus send --agent ${AGENT} ${PROJECT} "Dispatching <worker-name> for <id>: <title>" -L task-claim

### Spawning Workers

IMPORTANT: You MUST use \`botty spawn\` to create workers. Do NOT use Claude Code's built-in Task tool for worker dispatch.
Why: botty workers are independently observable (\`botty tail\`, \`botty list\`), survive your session crashing,
have independent timeouts, participate in botbus coordination (claims, messages, status), and respect maxWorkers limits.
The Task tool creates in-process subagents that bypass all of this infrastructure — no crash recovery, no observability, no coordination.

For each dispatched bead, spawn a worker via botty with hierarchical naming:

  botty spawn --name "${AGENT}/<worker-suffix>" \\
    --label worker --label "bead:<id>" \\
    --env-inherit BOTBUS_CHANNEL,BOTBUS_DATA_DIR \\
    --env "BOTBUS_AGENT=${AGENT}/<worker-suffix>" \\
    --env "BOTBOX_BEAD=<id>" \\
    --env "BOTBOX_WORKSPACE=\$WS" \\
    --env "BOTBUS_CHANNEL=${PROJECT}" \\
    --env "BOTBOX_PROJECT=${PROJECT}" \\
    --timeout ${CLAUDE_TIMEOUT} \\
    --cwd $(pwd) \\
    -- bun .agents/botbox/scripts/agent-loop.mjs --model <selected-model> ${PROJECT} ${AGENT}/<worker-suffix>

The hierarchical name (${AGENT}/<suffix>) lets you find all your workers via \`botty list\`.
The BOTBOX_BEAD and BOTBOX_WORKSPACE env vars tell agent-loop.mjs to skip triage and go straight to the assigned work.

After dispatching all workers, skip to step 6 (MONITOR).
${MISSIONS_ENABLED ? `
## 5c. MISSION (Level 4 — large task decomposition)

Use when: a large coherent task needs decomposition into related beads with shared context.
${process.env.BOTBOX_MISSION ? `\nBOTBOX_MISSION is set to "${process.env.BOTBOX_MISSION}" — focus on this mission.\n` : ''}
### Creating a Mission

1. Create the mission bead (if not already created by !mission handler):
   maw exec default -- br create --actor ${AGENT} --owner ${AGENT} \\
     --title="<mission title>" --labels mission --type=task --priority=2 \\
     --description="Outcome: <what done looks like>\\nSuccess metric: <how to verify>\\nConstraints: <scope/budget/forbidden>\\nStop criteria: <when to stop>"
2. Plan decomposition: break the mission into ${MAX_MISSION_CHILDREN} or fewer child beads.
   Consider dependencies between children — which can run in parallel, which are sequential.
3. Create child beads:
   For each child:
   maw exec default -- br create --actor ${AGENT} --owner ${AGENT} \\
     --title="<child title>" --parent <mission-id> \\
     --labels "mission:<mission-id>" --type=task --priority=2
4. Wire dependencies between children if needed:
   maw exec default -- br dep add --actor ${AGENT} <blocked-child> <blocker-child>
5. Post plan to channel:
   bus send --agent ${AGENT} ${PROJECT} "Mission <mission-id>: <title> — created N child beads" -L task-claim

### Dispatch Mission Workers

IMPORTANT: You MUST dispatch workers for independent children. Do NOT implement them yourself sequentially.
The whole point of missions is parallel execution — doing children sequentially defeats the purpose and wastes time.
Use \`botty spawn\` for mission workers — NOT the Task tool. See step 5b for why.

For independent children (unblocked), dispatch workers (max ${MAX_MISSION_WORKERS} concurrent):
- Follow the same dispatch pattern as step 5b — INCLUDING claim staking for EACH worker:
  bus claims stake --agent ${AGENT} "bead://${PROJECT}/<child-id>" -m "dispatched to <worker-name>"
  bus claims stake --agent ${AGENT} "workspace://${PROJECT}/\$WS" -m "<child-id>"
- Add mission labels and sibling context env vars:
    --label "mission:<mission-id>" \\
    --env "BOTBOX_MISSION=<mission-id>" \\
    --env "BOTBOX_MISSION_OUTCOME=<outcome from mission bead description>" \\
    --env "BOTBOX_SIBLINGS=<sibling-id> (<title>) [owner:<owner>, status:<status>]\\n..." \\
    --env "BOTBOX_FILE_HINTS=<sibling-id>: likely edits <files>\\n..." \\

Build the sibling context BEFORE dispatching:
1. List all children: maw exec default -- br list -l "mission:<mission-id>" --json
2. For each child: extract id, title, owner, status
3. Format BOTBOX_SIBLINGS as one line per child: "<id> (<title>) [owner:<owner>, status:<status>]"
4. Estimate file ownership hints from bead titles/descriptions (advisory, not enforced)
5. Extract the Outcome line from the mission bead description for BOTBOX_MISSION_OUTCOME

- Include mission context in each worker's bead comment:
  maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <child-id> \\
    "Mission context: <mission-id> — <outcome>. Siblings: <sibling-ids>."

### Checkpoint Loop (step 17)

After dispatching workers, enter a checkpoint loop. Run checkpoints every ${CHECKPOINT_INTERVAL_SEC} seconds.

Each checkpoint:
1. Count children by status:
   maw exec default -- br list --all -l "mission:<mission-id>" --json
   Tally: N open, M in_progress, K closed, J blocked
2. Check alive workers:
   botty list --format json
   Cross-reference with dispatched worker names (${AGENT}/<suffix>)
3. Check for completions (cursor-based — track last-seen message ID to avoid rescanning):
   bus history ${PROJECT} -n 20 -L task-done --since <last-checkpoint-time>
   Look for "Completed <bead-id>" messages from workers
4. Post checkpoint to channel (REQUIRED — crash recovery depends on this):
   bus send --agent ${AGENT} ${PROJECT} "Mission <mission-id> checkpoint: K/${'\$'}TOTAL done, J blocked, M active" -L feedback
   If this session crashes, the next iteration uses these messages to reconstruct mission state.
5. Detect failures:
   If a worker is not in botty list but its bead is still in_progress → crash recovery (see step 6)
6. Decide:
   - All children closed → exit checkpoint loop, proceed to Mission Close (step 18)
   - Some blocked, none in_progress → investigate blockers or rescope
   - Workers still alive → continue checkpoint loop

Exit the checkpoint loop when: all children are closed, OR no workers alive and all remaining beads are blocked.

### Mission Close and Synthesis (step 18)

When all children are closed:
1. Verify: maw exec default -- br list -l "mission:<mission-id>" — all should be closed
2. Write mission log as a bead comment (synthesis of what happened):
   maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <mission-id> \\
     "Mission complete.\\n\\nChildren: N total, all closed.\\nKey decisions: <what changed during execution>\\nWhat worked: <patterns that succeeded>\\nWhat to avoid: <patterns that failed>\\nKey artifacts: <files/modules created or modified>"
3. Close the mission: maw exec default -- br close --actor ${AGENT} <mission-id> --reason="All children completed"
4. Announce: bus send --agent ${AGENT} ${PROJECT} "Mission <mission-id> complete: <title> — N children, all done" -L task-done
` : ''}
## 6. MONITOR (if workers are dispatched)

Check for completion messages:
- bus inbox --agent ${AGENT} --channels ${PROJECT} -n 20
- Look for task-done messages from workers
- Check workspace status: maw ws list

For each completed worker:
- Read their progress comments: maw exec default -- br comments <id>
- Verify the work looks reasonable (spot check key files)

### Crash Recovery (dead worker detection)

Check which workers are still alive: botty list --format json
Cross-reference with your dispatched beads (check your bead:// claims).

For each dispatched bead where the worker is NOT in botty list but the bead is still in_progress:
1. Check bead comments for a "RETRY:1" marker (from a previous crash recovery attempt).
2. If NO retry marker — first failure, reassign once:
   - maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Worker <worker-name> died. RETRY:1 — reassigning."
   - Check if workspace still exists (maw ws list). If destroyed, create a new one.
   - Re-dispatch following step 5b (new worker name, same or new workspace).
3. If "RETRY:1" marker already exists — second failure, block the bead:
   - maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Worker died again after retry. Blocking bead."
   - maw exec default -- br update --actor ${AGENT} <id> --status=blocked
   - bus send --agent ${AGENT} ${PROJECT} "Bead <id> blocked: worker died twice" -L task-blocked
   - If workspace still exists: maw ws destroy <ws> (don't merge broken work)
   - bus claims release --agent ${AGENT} "bead://${PROJECT}/<id>"

## 7. FINISH (merge completed work)

For each completed bead with a workspace, check the bead's risk label first:

Already reviewed and approved (LGTM — reached from unfinished work check step 1):
  maw exec default -- crit reviews mark-merged <review-id> --agent ${AGENT}
  maw ws merge \$WS --destroy
  maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Completed by ${AGENT}"
  maw exec default -- br close --actor ${AGENT} <id> --reason="Completed"
  bus send --agent ${AGENT} ${PROJECT} "Completed <id>: <title>" -L task-done
  maw exec default -- br sync --flush-only${pushMainStep}

Not yet reviewed — RISK:LOW or RISK:MEDIUM (REVIEW is true):
  CHECK for existing review: maw exec default -- br comments <id> | grep "Review created:"
  Create review with reviewer (if none exists): maw exec \$WS -- crit reviews create --agent ${AGENT} --title "<id>: <title>" --description "<summary>" --reviewers ${PROJECT}-security
  IMMEDIATELY record: maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Review created: <review-id> in workspace <ws-name>"
  Spawn reviewer via @mention: bus send --agent ${AGENT} ${PROJECT} "Review requested: <review-id> for <id> @${PROJECT}-security" -L review-request
  STOP — wait for reviewer

Not yet reviewed — RISK:HIGH — Security review + failure-mode checklist:
  Same as risk:medium, add "risk:high — failure-mode checklist required" to review description.

Not yet reviewed — RISK:CRITICAL — Security review + human approval:
  Same as risk:high, plus post human approval request to bus.

If REVIEW is false (regardless of risk):
  maw ws merge \$WS --destroy
  maw exec default -- br close --actor ${AGENT} <id>
  bus send --agent ${AGENT} ${PROJECT} "Completed <id>: <title>" -L task-done
  maw exec default -- br sync --flush-only${pushMainStep}

After finishing all ready work:
  bus claims release --agent ${AGENT} --all

## 8. RELEASE CHECK (before signaling COMPLETE)

Before outputting COMPLETE, check if a release is needed:

1. Check for unreleased commits: maw exec default -- jj log -r 'tags()..main' --no-graph -T 'description.first_line() ++ "\\n"'
2. If any commits start with "feat:" or "fix:" (user-visible changes), a release is needed:
   - Bump version in Cargo.toml/package.json (semantic versioning)
   - Update changelog if one exists
   - Release: maw release vX.Y.Z (this tags, pushes, and updates bookmarks)
   - Announce: bus send --agent ${AGENT} ${PROJECT} "<project> vX.Y.Z released - <summary>" -L release
3. If only "chore:", "docs:", "refactor:" commits, no release needed.

Output: <promise>END_OF_STORY</promise> if more beads remain, else <promise>COMPLETE</promise>

Key rules:
- Triage first, then decide: sequential vs parallel
- Monitor dispatched workers, merge when ready
- All bus/crit commands use --agent ${AGENT}
- All br/bv commands: maw exec default -- br/bv ...
- All crit/jj commands in a workspace: maw exec \$WS -- crit/jj ...
- For parallel dispatch, note limitations of this prompt-based approach
- RISK LABELS: Always assess risk during grooming. When REVIEW is true, ALL risk levels go through review (risk:low gets lightweight review, risk:medium standard, risk:high failure-mode checklist, risk:critical human approval). When REVIEW is false, risk:low can self-review.${MISSIONS_ENABLED ? `
- MISSIONS: Enabled. Max ${MAX_MISSION_WORKERS} concurrent workers, max ${MAX_MISSION_CHILDREN} children per mission. Checkpoint every ${CHECKPOINT_INTERVAL_SEC}s.${process.env.BOTBOX_MISSION ? ` Focus on mission: ${process.env.BOTBOX_MISSION}` : ''}
- COORDINATION: Watch for coord:interface, coord:blocker, coord:handoff labels on bus messages from workers. React to coord:blocker by unblocking or reassigning.` : ''}
- Output completion signal at end`;
}

// --- Run agent via botbox run-agent ---
async function runClaude(prompt) {
	return new Promise((resolve, reject) => {
		const args = ['run-agent', 'claude', '-p', prompt];
		if (MODEL) {
			args.push('-m', MODEL);
		}
		args.push('-t', CLAUDE_TIMEOUT.toString());

		const proc = spawn('botbox', args);
		let output = '';

		proc.stdout?.on('data', (data) => {
			const chunk = data.toString();
			output += chunk;
			process.stdout.write(chunk); // Pass through to stdout
		});

		proc.stderr?.on('data', (data) => {
			process.stderr.write(data); // Pass through to stderr
		});

		proc.on('close', (code) => {
			if (code === 0) {
				resolve({ output, code: 0 });
			} else {
				reject(new Error(`botbox run-agent exited with code ${code}`));
			}
		});

		proc.on('error', (err) => {
			reject(err);
		});
	});
}

// Track if we already announced sign-off (to avoid duplicate messages)
let alreadySignedOff = false;

// --- Kill child workers (hierarchical name pattern: AGENT/suffix) ---
async function killChildWorkers() {
	try {
		let { stdout } = await runCommand('botty', ['list', '--format', 'json']);
		let parsed = JSON.parse(stdout || '{}');
		let agents = parsed.agents || [];
		let prefix = AGENT + '/';
		for (let agent of agents) {
			if (agent.name?.startsWith(prefix)) {
				try {
					await runCommand('botty', ['kill', agent.name]);
					console.log(`Killed child worker: ${agent.name}`);
				} catch {
					// Worker may have already exited
				}
			}
		}
	} catch {
		// botty list failed — no workers to kill
	}
}

// --- Cleanup handler ---
async function cleanup() {
	console.log('Cleaning up...');

	// Kill any child workers spawned by this dev-loop
	await killChildWorkers();

	if (!alreadySignedOff) {
		try {
			await runCommand('bus', [
				'send',
				'--agent',
				AGENT,
				PROJECT,
				`Dev agent ${AGENT} signing off.`,
				'-L',
				'agent-idle',
			]);
		} catch {}
	}
	try {
		await runCommand('bus', ['statuses', 'clear', '--agent', AGENT]);
	} catch {}
	try {
		await runCommand('bus', ['claims', 'release', '--agent', AGENT, `agent://${AGENT}`]);
	} catch {}
	try {
		await runCommand('bus', ['claims', 'release', '--agent', AGENT, '--all']);
	} catch {}
	try {
		await runInDefault('br', ['sync', '--flush-only']);
	} catch {}
	console.log(`Cleanup complete for ${AGENT}.`);
}

process.on('SIGINT', async () => {
	await cleanup();
	process.exit(0);
});

process.on('SIGTERM', async () => {
	await cleanup();
	process.exit(0);
});

// --- Main ---
async function main() {
	await loadConfig();
	parseCliArgs();

	AGENT = await getAgentName();

	console.log(`Agent:     ${AGENT}`);
	console.log(`Project:   ${PROJECT}`);
	console.log(`Max loops: ${MAX_LOOPS}`);
	console.log(`Pause:     ${LOOP_PAUSE}s`);
	console.log(`Model:     ${MODEL || 'system default'}`);
	console.log(`Review:    ${REVIEW}`);

	// Confirm identity
	try {
		await runCommand('bus', ['whoami', '--agent', AGENT]);
	} catch (err) {
		console.error('Error confirming agent identity:', err.message);
		process.exit(1);
	}

	// Stake agent claim (ignore failure — may already be held from previous run)
	try {
		await runCommand('bus', [
			'claims',
			'stake',
			'--agent',
			AGENT,
			`agent://${AGENT}`,
			'-m',
			`dev-loop for ${PROJECT}`,
		]);
	} catch {
		// Already held — will refresh in the loop
	}

	// Announce
	await runCommand('bus', [
		'send',
		'--agent',
		AGENT,
		PROJECT,
		`Dev agent ${AGENT} online, starting dev loop`,
		'-L',
		'spawn-ack',
	]);

	// Set starting status
	await runCommand('bus', ['statuses', 'set', '--agent', AGENT, 'Starting loop', '--ttl', '10m']);

	// Capture baseline commits for release tracking
	const baselineCommits = await getCommitsSinceOrigin();

	// Truncate journal at start of loop session
	await truncateJournal();

	// Main loop
	for (let i = 1; i <= MAX_LOOPS; i++) {
		console.log(`\n--- Dev loop ${i}/${MAX_LOOPS} ---`);

		// Refresh agent claim TTL (ignore failure)
		try {
			await runCommand('bus', ['claims', 'refresh', '--agent', AGENT, `agent://${AGENT}`]);
		} catch {
			// Claim may have expired or been released — not fatal
		}

		if (!(await hasWork())) {
			await runCommand('bus', ['statuses', 'set', '--agent', AGENT, 'Idle']);
			console.log('No work available. Exiting cleanly.');
			await runCommand('bus', [
				'send',
				'--agent',
				AGENT,
				PROJECT,
				`No work remaining. Dev agent ${AGENT} signing off.`,
				'-L',
				'agent-idle',
			]);
			alreadySignedOff = true;
			break;
		}

		// Guard: if a review is pending, don't run Claude — just wait
		let pendingBeadId = await hasPendingReview();
		if (pendingBeadId) {
			console.log(`Review pending for ${pendingBeadId} — waiting (not running Claude)`);
			try {
				await runCommand('bus', [
					'statuses',
					'set',
					'--agent',
					AGENT,
					`Waiting: review for ${pendingBeadId}`,
					'--ttl',
					'10m',
				]);
			} catch {}
			// Wait longer than normal pause — reviews take time
			await new Promise((resolve) => setTimeout(resolve, 30_000));
			continue;
		}

		// Run Claude
		try {
			const lastIteration = await readLastIteration();
			const prompt = buildPrompt(lastIteration);
			const result = await runClaude(prompt);

			// Check for completion signals
			if (result.output.includes('<promise>COMPLETE</promise>')) {
				console.log('✓ Dev cycle complete - no more work');
				alreadySignedOff = true; // Agent likely sent its own sign-off
				break;
			} else if (result.output.includes('<promise>END_OF_STORY</promise>')) {
				console.log('✓ Iteration complete - more work remains');
				// Safety check: verify work actually remains (agent may say END_OF_STORY but have finished everything)
				if (!(await hasWork())) {
					console.log('No remaining work found despite END_OF_STORY — exiting cleanly');
					alreadySignedOff = true;
					break;
				}
			} else {
				console.log('Warning: No completion signal found in output');
			}

			// Extract and append iteration summary to journal
			const summaryMatch = result.output.match(/<iteration-summary>([\s\S]*?)<\/iteration-summary>/);
			if (summaryMatch) {
				await appendJournal(summaryMatch[1]);
			}
		} catch (err) {
			console.error('Error running Claude:', err.message);

			// Check for fatal API errors and post to botbus
			const isFatalError =
				err.message.includes('API Error') ||
				err.message.includes('rate limit') ||
				err.message.includes('overloaded');

			if (isFatalError) {
				console.error('Fatal error detected, posting to botbus and exiting...');
				try {
					await runCommand('bus', [
						'send',
						'--agent',
						AGENT,
						PROJECT,
						`Dev loop error: ${err.message}. Agent ${AGENT} going offline.`,
						'-L',
						'agent-error',
					]);
				} catch {
					// Ignore bus errors during shutdown
				}
				break; // Exit loop on fatal error
			}
			// Continue to next iteration on non-fatal errors
		}

		if (i < MAX_LOOPS) {
			await new Promise((resolve) => setTimeout(resolve, LOOP_PAUSE * 1000));
		}
	}

	// Show what landed since session start (for release decisions)
	const finalCommits = await getCommitsSinceOrigin();
	const newCommits = finalCommits.filter((c) => !baselineCommits.includes(c));
	if (newCommits.length > 0) {
		console.log('\n--- Commits landed this session ---');
		for (const commit of newCommits) {
			console.log(`  ${commit}`);
		}
		console.log('\nIf any are user-visible (feat/fix), consider a release.');
	}

	await cleanup();
}

main().catch((err) => {
	console.error('Fatal error:', err);
	cleanup().finally(() => process.exit(1));
});
