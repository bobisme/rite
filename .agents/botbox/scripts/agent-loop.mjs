#!/usr/bin/env node
import { spawn } from 'child_process';
import { readFile } from 'fs/promises';
import { existsSync } from 'fs';
import { parseArgs } from 'util';

// --- Defaults ---
let MAX_LOOPS = 20;
let LOOP_PAUSE = 2;
let CLAUDE_TIMEOUT = 600;
let MODEL = '';
let PROJECT = '';
let AGENT = '';
let PUSH_MAIN = false;
let REVIEW = true;
let CRITICAL_APPROVERS = [];

// --- Dispatched worker env (set by dev-loop when spawning workers) ---
let DISPATCHED_BEAD = process.env.BOTBOX_BEAD || '';
let DISPATCHED_WORKSPACE = process.env.BOTBOX_WORKSPACE || '';
let DISPATCHED_MISSION = process.env.BOTBOX_MISSION || '';
let DISPATCHED_SIBLINGS = process.env.BOTBOX_SIBLINGS || '';
let DISPATCHED_MISSION_OUTCOME = process.env.BOTBOX_MISSION_OUTCOME || '';
let DISPATCHED_FILE_HINTS = process.env.BOTBOX_FILE_HINTS || '';

// --- Load config from .botbox.json ---
async function loadConfig() {
	if (existsSync('.botbox.json')) {
		try {
			const config = JSON.parse(await readFile('.botbox.json', 'utf-8'));
			const project = config.project || {};
			const agents = config.agents || {};
			const worker = agents.worker || {};

			// Project identity (can be overridden by CLI args)
			PROJECT = project.channel || project.name || '';
			// Workers get auto-generated names by default (AGENT stays empty)

			// Agent settings
			MODEL = worker.model || '';
			CLAUDE_TIMEOUT = worker.timeout || 600;
			PUSH_MAIN = config.pushMain || false;
			REVIEW = config.review?.enabled ?? true;
			CRITICAL_APPROVERS = project.criticalApprovers || [];
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
			help: { type: 'boolean', short: 'h' },
		},
		allowPositionals: true,
	});

	if (values.help) {
		console.log(`Usage: agent-loop.mjs [options] <project> [agent-name]

Worker agent. Picks one task per iteration, implements it, requests review,
and finishes. Sequential — one bead at a time.

Options:
  --max-loops N   Max iterations (default: ${MAX_LOOPS})
  --pause N       Seconds between iterations (default: ${LOOP_PAUSE})
  --model M       Model for the worker agent (default: system default)
  -h, --help      Show this help

Arguments:
  project         Project name (default: from .botbox.json)
  agent-name      Agent identity (default: auto-generated)`);
		process.exit(0);
	}

	if (values['max-loops']) MAX_LOOPS = parseInt(values['max-loops'], 10);
	if (values.pause) LOOP_PAUSE = parseInt(values.pause, 10);
	if (values.model) MODEL = values.model;

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
		console.error('Usage: agent-loop.mjs [options] <project> [agent-name]');
		process.exit(1);
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

// --- Helper: run command in default workspace (for br, bv) ---
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

// --- Helper: check if a review is pending (don't run Claude, just wait) ---
async function hasPendingReview() {
	try {
		let result = await runInDefault('br', ['list', '--status', 'in_progress', '--assignee', AGENT, '--json']);
		let beads = JSON.parse(result.stdout || '[]');
		if (!Array.isArray(beads)) beads = [];

		for (let bead of beads) {
			try {
				let commentsResult = await runInDefault('br', ['comments', bead.id, '--json']);
				let comments = JSON.parse(commentsResult.stdout || '[]');
				let arr = Array.isArray(comments) ? comments : comments.comments || [];

				let hasReview = arr.some(
					(/** @type {any} */ c) =>
						c.body?.includes('Review created:') ||
						c.body?.includes('Review requested:') ||
						c.content?.includes('Review created:') ||
						c.content?.includes('Review requested:'),
				);
				if (!hasReview) continue;

				let hasCompleted = arr.some(
					(/** @type {any} */ c) =>
						c.body?.includes('Completed by') || c.content?.includes('Completed by'),
				);
				if (hasCompleted) continue;

				return bead.id;
			} catch {
				// Can't read comments, skip
			}
		}
	} catch {
		// Can't list beads, skip
	}
	return null;
}

// --- Helper: check if there is work ---
async function hasWork() {
	if (DISPATCHED_BEAD) return true;
	try {
		// Check claims
		const claimsResult = await runCommand('bus', [
			'claims',
			'list',
			'--agent',
			AGENT,
			'--mine',
			'--format',
			'json',
		]);
		const claims = JSON.parse(claimsResult.stdout || '{}');
		if (claims.claims && claims.claims.length > 0) return true;

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

// --- Build worker prompt ---
function buildPrompt() {
	const pushMainStep = PUSH_MAIN ? '\n   Push to GitHub: maw push (if fails, announce issue).' : '';

	return `You are worker agent "${AGENT}" for project "${PROJECT}".

IMPORTANT: Use --agent ${AGENT} on ALL bus and crit commands. Use --actor ${AGENT} on ALL mutating br commands (create, update, close, comments add, dep add, label add). Also use --owner ${AGENT} on br create and --author ${AGENT} on br comments add. Set BOTBOX_PROJECT=${PROJECT}.

CRITICAL - HUMAN MESSAGE PRIORITY: If you see a system reminder with "STOP:" showing unread botbus messages, these are from humans or other agents trying to reach you. IMMEDIATELY check inbox and respond before continuing your current task. Human questions, clarifications, and redirects take priority over heads-down work.

COMMAND PATTERN — maw exec: All br/bv commands run in the default workspace. All crit/jj commands run in their workspace.
  br/bv: maw exec default -- br <args>       or  maw exec default -- bv <args>
  crit:  maw exec \$WS -- crit <args>
  jj:    maw exec \$WS -- jj <args>
  other: maw exec \$WS -- <command>           (cargo test, etc.)

${DISPATCHED_BEAD && DISPATCHED_WORKSPACE ? `## DISPATCHED WORKER — FAST PATH

You were dispatched by a lead dev agent with a pre-assigned bead and workspace.
Skip steps 0 (RESUME CHECK), 1 (INBOX), and 2 (TRIAGE) entirely.

Pre-assigned bead: ${DISPATCHED_BEAD}
Pre-assigned workspace: ${DISPATCHED_WORKSPACE}
Workspace path: ${process.cwd()}/ws/${DISPATCHED_WORKSPACE}
${DISPATCHED_MISSION ? `Mission: ${DISPATCHED_MISSION}
${DISPATCHED_MISSION_OUTCOME ? `Mission outcome: ${DISPATCHED_MISSION_OUTCOME}` : `Read mission context: maw exec default -- br show ${DISPATCHED_MISSION}`}
${DISPATCHED_SIBLINGS ? `\nSibling beads (other workers in this mission):\n${DISPATCHED_SIBLINGS}` : ''}
${DISPATCHED_FILE_HINTS ? `\nAdvisory file ownership (avoid editing files owned by siblings):\n${DISPATCHED_FILE_HINTS}` : ''}` : ''}

Go directly to:
1. Verify your bead: maw exec default -- br show ${DISPATCHED_BEAD}
2. Verify your workspace: maw ws list (confirm ${DISPATCHED_WORKSPACE} exists)
3. Your bead is already in_progress and claimed. Proceed to step 4 (WORK).
   Use absolute workspace path: ${process.cwd()}/ws/${DISPATCHED_WORKSPACE}
   For commands in workspace: maw exec ${DISPATCHED_WORKSPACE} -- <command>

` : ''}${DISPATCHED_BEAD ? 'You are a dispatched worker — follow the FAST PATH section below.' : `Execute exactly ONE cycle of the worker loop. Complete one task (or determine there is no work),
then STOP. Do not start a second task — the outer loop handles iteration.`}

At the end of your work, output exactly one of these completion signals:
- <promise>COMPLETE</promise> if you completed a task or determined there is no work
- <promise>BLOCKED</promise> if you are stuck and cannot proceed

0. RESUME CHECK (do this FIRST):
   Run: bus claims list --agent ${AGENT} --mine
   If you hold a bead:// claim, you have an in-progress bead from a previous iteration.
   - Run: maw exec default -- br comments <bead-id> to understand what was done before and what remains.
   - Look for workspace info in comments (workspace name and path).
   - If a "Review created: <review-id>" comment exists:
     * Find the review: maw exec $WS -- crit review <review-id>
     * Check review status: maw exec \$WS -- crit review <review-id>
     * If LGTM (approved): proceed to FINISH (step 7) — merge the review and close the bead.
     * If BLOCKED (changes requested): fix the issues, then re-request review:
       1. Read threads: maw exec $WS -- crit review <review-id> (threads show inline with comments)
       2. For each unresolved thread with reviewer feedback:
          - Fix the code in the workspace (use absolute WS_PATH for file edits)
          - Reply: maw exec $WS -- crit reply <thread-id> --agent ${AGENT} "Fixed: <what you did>"
          - Resolve: maw exec $WS -- crit threads resolve <thread-id> --agent ${AGENT}
       3. Update commit: maw exec $WS -- jj describe -m "<id>: <summary> (addressed review feedback)"
       4. Re-request: maw exec $WS -- crit reviews request <review-id> --reviewers ${PROJECT}-security --agent ${AGENT}
       5. Announce: bus send --agent ${AGENT} ${PROJECT} "Review updated: <review-id> — addressed feedback @${PROJECT}-security" -L review-response
       STOP this iteration — wait for re-review.
     * If PENDING (no votes yet): STOP this iteration. Wait for the reviewer.
     * If review not found: DO NOT merge or create a new review. The reviewer may still be starting up (hooks have latency). STOP this iteration and wait. Only create a new review if the workspace was destroyed AND 3+ iterations have passed since the review comment.
   - If no review comment (work was in progress when session ended):
     * Read the workspace code to see what's already done.
     * Complete the remaining work in the EXISTING workspace — do NOT create a new one.
     * After completing: maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Resumed and completed: <what you finished>".
     * Then proceed to step 6 (REVIEW REQUEST) or step 7 (FINISH).
   If no active claims: proceed to step 1 (INBOX).

1. INBOX (do this before triaging):
   Run: bus inbox --agent ${AGENT} --channels ${PROJECT} --mark-read
   For each message:
   - Task request (-L task-request or asks for work): create a bead with maw exec default -- br create.
   - Status check or question: reply on bus, do NOT create a bead.
   - Feedback (-L feedback): if it contains a bug report, feature request, or actionable work — create a bead. Evaluate critically: is this a real issue? Is it well-scoped? Set priority accordingly. Then acknowledge on bus.
   - Announcements from other agents ("Working on...", "Completed...", "online"): ignore, no action.
   - Duplicate of existing bead: do NOT create another bead, note it covers the request.

2. TRIAGE: Check maw exec default -- br ready. If no ready beads and inbox created none, say "NO_WORK_AVAILABLE" and stop.
   GROOM each ready bead (maw exec default -- br show <id>): ensure clear title, description with acceptance criteria
   and testing strategy, appropriate priority, and risk label. Fix anything missing, comment what you changed.
   RISK LABELS: Assess each bead for risk using these dimensions: blast radius, data sensitivity, reversibility, dependency uncertainty.
   - risk:low — typo fixes, doc updates, config tweaks (add label: br label add --actor ${AGENT} -l risk:low <id>)
   - risk:medium — standard features/bugs (default, no label needed)
   - risk:high — security-sensitive, data integrity, user-visible behavior changes (add label)
   - risk:critical — irreversible actions, migrations, regulated changes (add label)
   Any agent can escalate risk upward. Downgrades require lead approval with justification comment.
   Use maw exec default -- bv --robot-next to pick exactly one small task. If the task is large, break it down with
   maw exec default -- br create + br dep add, then bv --robot-next again. If a bead is claimed
   (bus claims check --agent ${AGENT} "bead://${PROJECT}/<id>"), skip it.

   MISSION CONTEXT: After picking a bead, check if it has a mission:bd-xxx label (visible in br show output).
   If it does, read the mission bead for shared context:
     maw exec default -- br show <mission-id>
   Note the mission's Outcome, Constraints, and Stop criteria. Check siblings:
     maw exec default -- br list -l "mission:<mission-id>"
   Use this context to understand how your work fits into the larger effort.

   COORDINATION LABELS: When working on a mission, use these labels on bus messages:
   - coord:interface — Share API/interface contracts with siblings: bus send --agent ${AGENT} ${PROJECT} "Interface: <details>" -L coord:interface -L "mission:${DISPATCHED_MISSION || '<mission-id>'}"
   - coord:blocker — Flag a blocking dependency on a sibling: bus send --agent ${AGENT} ${PROJECT} "Blocked by <sibling-bead>: <reason>" -L coord:blocker -L "mission:${DISPATCHED_MISSION || '<mission-id>'}"
   - task-done — Signal completion: bus send --agent ${AGENT} ${PROJECT} "Completed <id>" -L task-done -L "mission:${DISPATCHED_MISSION || '<mission-id>'}"

3. START: maw exec default -- br update --actor ${AGENT} <id> --status=in_progress --owner=${AGENT}.
   bus claims stake --agent ${AGENT} "bead://${PROJECT}/<id>" -m "<id>".
   Create workspace: run maw ws create --random. Note the workspace name AND absolute path
   from the output (e.g., name "frost-castle", path "/abs/path/ws/frost-castle").
   Store the name as WS and the absolute path as WS_PATH.
   IMPORTANT: All file operations (Read, Write, Edit) must use the absolute WS_PATH.
   For commands in the workspace: maw exec \$WS -- <command>.
   Do NOT cd into the workspace and stay there — the workspace is destroyed during finish.
   bus claims stake --agent ${AGENT} "workspace://${PROJECT}/\$WS" -m "<id>".
   maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Started in workspace \$WS (\$WS_PATH)".
   bus statuses set --agent ${AGENT} "Working: <id>" --ttl 30m.
   Announce: bus send --agent ${AGENT} ${PROJECT} "Working on <id>: <title>" -L task-claim.

4. WORK: maw exec default -- br show <id>, then implement the task in the workspace.
   Add at least one progress comment: maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Progress: ...".

5. STUCK CHECK: If same approach tried twice, info missing, or tool fails repeatedly — you are
   stuck. maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Blocked: <details>".
   bus statuses set --agent ${AGENT} "Blocked: <short reason>".
   bus send --agent ${AGENT} ${PROJECT} "Stuck on <id>: <reason>" -L task-blocked.
   maw exec default -- br update --actor ${AGENT} <id> --status=blocked.
   Release: bus claims release --agent ${AGENT} "bead://${PROJECT}/<id>".
   Output: <promise>BLOCKED</promise>
   Stop this cycle.

6. REVIEW REQUEST (risk-aware):
${REVIEW ? `   First, check the bead's risk label: maw exec default -- br show <id> — look for risk:low, risk:high, or risk:critical labels.
   No risk label = risk:medium (standard review, current default).

   RISK:LOW PATH — Lightweight review:
     Same as RISK:MEDIUM below. risk:low still gets reviewed when REVIEW is true — the reviewer can fast-track it.

   RISK:MEDIUM PATH — Standard review (current default):
     Describe the change: maw exec \$WS -- jj describe -m "<id>: <summary>".
     CHECK for existing review first:
       - Run: maw exec default -- br comments <id> | grep "Review created:"
       - If found, extract <review-id> and skip to requesting review (don't create duplicate)
     Create review with reviewer assignment (only if none exists):
       - maw exec \$WS -- crit reviews create --agent ${AGENT} --title "<id>: <title>" --description "<summary>" --reviewers ${PROJECT}-security
       - IMMEDIATELY record: maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Review created: <review-id> in workspace \$WS"
     bus statuses set --agent ${AGENT} "Review: <review-id>".
     Spawn reviewer via @mention: bus send --agent ${AGENT} ${PROJECT} "Review requested: <review-id> for <id> @${PROJECT}-security" -L review-request
     Do NOT close the bead. Do NOT merge. Do NOT release claims.
     Output: <promise>COMPLETE</promise>
     STOP this iteration.

   RISK:HIGH PATH — Security review + failure-mode checklist:
     Same as risk:medium, but when creating the review, add to description: "risk:high — failure-mode checklist required."
     The security reviewer will include the 5 failure-mode questions in their review:
       1. What could fail in production?  2. How would we detect it quickly?
       3. What is the fastest safe rollback?  4. What dependency could invalidate this plan?
       5. What assumption is least certain?
     MUST request security reviewer. Do not skip.
     STOP this iteration.

   RISK:CRITICAL PATH — Security review + human approval required:
     Same as risk:high, but ALSO:
     - Add to review description: "risk:critical — REQUIRES HUMAN APPROVAL before merge."
     - Post to bus requesting human approval:
       bus send --agent ${AGENT} ${PROJECT} "risk:critical review for <id>: requires human approval before merge. ${CRITICAL_APPROVERS.length > 0 ? 'Approvers: ' + CRITICAL_APPROVERS.join(', ') : 'Check project.criticalApprovers in .botbox.json'}" -L review-request
     STOP this iteration.` : `   REVIEW is disabled. Skip code review.
   Describe the change: maw exec \$WS -- jj describe -m "<id>: <summary>".
   Proceed directly to step 7 (FINISH).`}

7. FINISH (only reached after LGTM from step 0, or after step 6 when REVIEW is false):
   If a review was conducted:
     maw exec default -- crit reviews mark-merged <review-id> --agent ${AGENT}.
   RISK:CRITICAL CHECK — Before merging a risk:critical bead:
     Verify human approval exists: bus history ${PROJECT} -n 50 -L review-request | look for approval message referencing this bead/review from an authorized approver.
     If no approval found, do NOT merge. Post: bus send --agent ${AGENT} ${PROJECT} "Waiting for human approval on risk:critical <id>" -L review-request. STOP.
     If approval found, record it: maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Human approval: <approver> via bus message <msg-id>"
   maw exec default -- br comments add --actor ${AGENT} --author ${AGENT} <id> "Completed by ${AGENT}".
   maw exec default -- br close --actor ${AGENT} <id> --reason="Completed" --suggest-next.
   bus send --agent ${AGENT} ${PROJECT} "Completed <id>: <title>" -L task-done.
   maw ws merge \$WS --destroy (produces linear squashed history and auto-moves main; if conflict, preserve and announce).
   bus claims release --agent ${AGENT} --all.
   maw exec default -- br sync --flush-only.${pushMainStep}
   Then proceed to step 8 (RELEASE CHECK).

8. RELEASE CHECK (before signaling COMPLETE):
   Check for unreleased commits: maw exec default -- jj log -r 'tags()..main' --no-graph -T 'description.first_line() ++ "\\n"'
   If any commits start with "feat:" or "fix:" (user-visible changes), a release is needed:
   - Bump version in Cargo.toml/package.json (semantic versioning)
   - Update changelog if one exists
   - Release: maw release vX.Y.Z (this tags, pushes, and updates bookmarks)
   - Announce: bus send --agent ${AGENT} ${PROJECT} "<project> vX.Y.Z released - <summary>" -L release
   If only "chore:", "docs:", "refactor:" commits, no release needed.
   Output: <promise>COMPLETE</promise>

Key rules:
- Exactly one small task per cycle.
- Always finish or release before stopping.
- If claim denied, pick something else.
- All bus and crit commands use --agent ${AGENT}.
- All file operations use the absolute workspace path from maw ws create output. Do NOT cd into the workspace and stay there.
- All br/bv commands: maw exec default -- br/bv ...
- All crit/jj commands in a workspace: maw exec \$WS -- crit/jj ...
- If a tool behaves unexpectedly, report it: bus send --agent ${AGENT} ${PROJECT} "Tool issue: <details>" -L tool-issue.
- STOP after completing one task or determining no work. Do not loop.
- Always output <promise>COMPLETE</promise> or <promise>BLOCKED</promise> at the end.
- RISK LABELS: Check bead risk labels before review. REVIEW=${REVIEW}. ${REVIEW ? 'ALL risk levels go through review (risk:low gets lightweight review, risk:medium standard, risk:high failure-mode checklist, risk:critical human approval).' : 'Review is disabled. Skip review and proceed to FINISH after describing commit.'}`;

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

// --- Cleanup handler ---
async function cleanup() {
	console.log('Cleaning up...');
	if (!alreadySignedOff) {
		try {
			await runCommand('bus', [
				'send',
				'--agent',
				AGENT,
				PROJECT,
				`Agent ${AGENT} signing off.`,
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

	if (DISPATCHED_BEAD) {
		MAX_LOOPS = 1;
	}

	AGENT = await getAgentName();

	console.log(`Agent:     ${AGENT}`);
	console.log(`Project:   ${PROJECT}`);
	console.log(`Max loops: ${MAX_LOOPS}`);
	console.log(`Pause:     ${LOOP_PAUSE}s`);
	console.log(`Model:     ${MODEL || 'system default'}`);
	if (DISPATCHED_BEAD) {
		console.log(`Dispatched: bead=${DISPATCHED_BEAD} workspace=${DISPATCHED_WORKSPACE} mission=${DISPATCHED_MISSION || 'none'}`);
	}
	if (DISPATCHED_SIBLINGS) {
		console.log(`Siblings:   ${DISPATCHED_SIBLINGS.split('\n').length} sibling(s)`);
	}

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
			`worker-loop for ${PROJECT}`,
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
		`Agent ${AGENT} online, starting worker loop`,
		'-L',
		'spawn-ack',
	]);

	// Set starting status
	await runCommand('bus', ['statuses', 'set', '--agent', AGENT, 'Starting loop', '--ttl', '10m']);

	// Main loop
	for (let i = 1; i <= MAX_LOOPS; i++) {
		console.log(`\n--- Loop ${i}/${MAX_LOOPS} ---`);

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
				`No work remaining. Agent ${AGENT} signing off.`,
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
			await new Promise((resolve) => setTimeout(resolve, 30_000));
			continue;
		}

		// Run Claude
		try {
			const prompt = buildPrompt();
			const result = await runClaude(prompt);

			// Check for completion signals
			if (result.output.includes('<promise>COMPLETE</promise>')) {
				console.log('✓ Task cycle complete');
				alreadySignedOff = true; // Agent likely sent its own sign-off
			} else if (result.output.includes('<promise>BLOCKED</promise>')) {
				console.log('⚠ Agent blocked');
			} else {
				console.log('Warning: No completion signal found in output');
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
						`Worker error: ${err.message}. Agent ${AGENT} going offline.`,
						'-L',
						'agent-error',
					]);
				} catch {
					// Ignore bus errors during shutdown
				}
				break; // Exit loop on fatal error
			}

			// Handle timeout separately
			if (err.message.includes('Timeout')) {
				console.error('Claude timed out. Session may be stuck.');
			}
			// Continue to next iteration on non-fatal errors
		}

		if (i < MAX_LOOPS) {
			await new Promise((resolve) => setTimeout(resolve, LOOP_PAUSE * 1000));
		}
	}

	await cleanup();
}

main().catch((err) => {
	console.error('Fatal error:', err);
	cleanup().finally(() => process.exit(1));
});
